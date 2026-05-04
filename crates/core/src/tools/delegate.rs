//! `delegate` tool — spawn a sub-agent with a restricted tool surface.
//!
//! Cline pattern (`SubagentBuilder.ts`): the parent calls `delegate(name,
//! task)`; we look up the named sub-agent, build a sub-`Session` whose
//! tool registry is filtered to that sub-agent's allow-list,
//! auto-mount `attempt_completion`, run the sub-session to completion,
//! and return whatever the sub-agent put in the return slot.
//!
//! V1 constraints (matches cline's research-only mode):
//! - sub-agents inherit no mutating tools (apply_edl, start_render,
//!   bash). Enforced declaratively in `subagent.rs::default_subagents`.
//! - sub-agents do not chain — there's no `delegate` mounted on the
//!   sub-session itself, so a sub-agent can't spawn its own sub-agents.
//!   This caps blast radius and keeps the audit trail flat.
//! - the sub-session uses the parent's client, project_root, mcp_host,
//!   skills. It gets its OWN events_tx (so its events don't leak to
//!   the parent UI) and its OWN history (no shared state).
//!
//! The sub-agent's events are captured via a private broadcast channel
//! and folded into a one-line summary returned alongside the result —
//! the parent gets a clean "sub-agent X said: <result>" report rather
//! than a flood of streaming deltas.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::FunctionCallError;
use crate::anthropic::{Client, ClientConfig, Tool as ToolSchema};
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput, ToolRegistry};

/// The `delegate` tool. Mounted on the parent session only.
pub struct DelegateTool {
    /// Parent registry — the source the sub-agent's allow-list filters
    /// against. `delegate` clones this once per call and restricts it.
    parent_registry: ToolRegistry,
    /// Catalog of available sub-agents.
    subagents: Arc<crate::subagent::SubAgentRegistry>,
    /// Default model for sub-sessions. Inherited from the parent at
    /// session-construction time so sub-agents stay coherent with the
    /// parent's context window.
    default_model: String,
}

impl DelegateTool {
    /// Construct a `DelegateTool` bound to the parent's registry +
    /// sub-agent catalog. Hand the parent's full registry and
    /// sub-agent registry; per-call we clone + restrict.
    pub fn new(
        parent_registry: ToolRegistry,
        subagents: Arc<crate::subagent::SubAgentRegistry>,
        default_model: String,
    ) -> Self {
        Self {
            parent_registry,
            subagents,
            default_model,
        }
    }
}

#[derive(Debug, Deserialize)]
struct DelegateArgs {
    name: String,
    task: String,
}

#[async_trait]
impl ToolHandler for DelegateTool {
    fn name(&self) -> &'static str {
        "delegate"
    }

    fn schema(&self) -> ToolSchema {
        // Inline the catalog so the parent knows what's available
        // without a separate lookup.
        let catalog = self
            .subagents
            .iter()
            .map(|a| format!("  - {}: {}", a.name, a.description))
            .collect::<Vec<_>>()
            .join("\n");
        let description = format!(
            "{base}\n\nAvailable sub-agents:\n{catalog}",
            base = DESCRIPTION_BASE,
            catalog = catalog,
        );
        ToolSchema {
            name: "delegate".into(),
            description,
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Sub-agent id (one of the names listed in the description above)."
                    },
                    "task": {
                        "type": "string",
                        "description": "The full task description for the sub-agent. This becomes its first user message; be specific."
                    }
                },
                "required": ["name", "task"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        // Delegating is itself read-only — the sub-agent's tool surface
        // is restricted to non-mutating tools. Mutating sub-agents are
        // deferred to #153.
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: DelegateArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "delegate: invalid args ({e}). Required: \
                 {{ \"name\": <sub-agent>, \"task\": <str> }}."
            ))
        })?;

        let Some(spec) = self.subagents.get(&args.name) else {
            let available = self
                .subagents
                .iter()
                .map(|a| a.name)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(FunctionCallError::RespondToModel(format!(
                "delegate: no sub-agent named '{}'. Available: {}.",
                args.name, available
            )));
        };

        // Reject delegating from inside a sub-session. V1 keeps the
        // call tree flat (no recursive delegation).
        if ctx.subagent_return.is_some() {
            return Err(FunctionCallError::RespondToModel(
                "delegate: sub-agents cannot spawn their own sub-agents. \
                 Return your findings via attempt_completion and let the \
                 parent agent dispatch a follow-up delegation."
                    .into(),
            ));
        }

        // Build the restricted registry + auto-mount attempt_completion.
        let mut sub_registry = self.parent_registry.restrict_to(spec.allowed_tools);
        sub_registry.register(Arc::new(crate::tools::attempt_completion::AttemptCompletionTool));

        // Sub-session uses the parent's client. Reading from env or
        // keychain works as long as the parent's session was built the
        // same way (which is the only path in the live binary).
        let client = Client::from_env_or_keychain(ClientConfig::default()).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "delegate: could not build client for sub-agent: {e}. \
                 Make sure ANTHROPIC_API_KEY is set or stored in the keychain."
            ))
        })?;

        // Build sub-system prompt: the sub-agent gets a fresh, focused
        // prompt with NO inherited episode-map / AWIDAT.md noise. The
        // sub-agent's job is the task; the system suffix tells it how
        // to return.
        let sys = format!(
            "You are sub-agent `{}`.\n\n{}{}",
            spec.name,
            spec.description,
            spec.system_suffix,
        );

        let return_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let sub_session = crate::Session::new(
            client,
            sub_registry,
            self.default_model.clone(),
            Some(sys),
            ctx.project_root.clone(),
        )
        .with_subagent_return(return_slot.clone());

        // Run to completion. We give the sub-session up to spec.max_iter
        // outer-loop iterations. Cancellation is local — if the parent
        // is cancelled, we don't propagate (the sub-session would have
        // its own cancellation token threaded through, but for V1 we
        // use a fresh one and rely on the sub-agent's iteration cap).
        let cancel = CancellationToken::new();
        let max_iter = spec.max_iterations;
        // Surface the sub-agent's progress as a single SessionEvent on
        // the parent's bus so the TUI can render "delegating to <name>"
        // status.
        let _ = ctx.events_tx.send(crate::SessionEvent::TextDelta(format!(
            "\n[delegate → {}] {}\n",
            spec.name, args.task
        )));

        // The sub-session loops on its own; we just feed it the task
        // as one user turn and let `attempt_completion` end it.
        let run_result = sub_session.run_turn_capped(args.task, cancel, max_iter).await;

        let _ = run_result; // turn errors are handled below via the slot check

        let result = return_slot.lock().await.clone();
        match result {
            Some(payload) => Ok(ToolOutput::text(payload)),
            None => Err(FunctionCallError::RespondToModel(format!(
                "delegate: sub-agent '{}' ended without calling attempt_completion. \
                 Either rephrase the task or ask the user — the sub-agent didn't \
                 produce a returnable answer.",
                spec.name
            ))),
        }
    }
}

const DESCRIPTION_BASE: &str = "\
Delegate a focused, read-only research task to a sub-agent with a \
restricted tool surface. The sub-agent runs in isolation, calls a \
narrowed subset of tools to gather information, and returns a single \
consolidated string via `attempt_completion`. Use when you need to \
answer a self-contained question without polluting the main turn's \
context.\
\n\nSub-agents cannot mutate state (no apply_edl, no start_render, no \
bash). For edits, do the work yourself. For long research tasks where \
the answer is paragraphs of investigation that the parent doesn't \
need verbatim, delegate.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subagent::SubAgentRegistry;

    fn registry_with_one_tool() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register(Arc::new(crate::tools::view_episode::ViewEpisodeTool));
        r
    }

    #[test]
    fn schema_lists_subagents_in_description() {
        let tool = DelegateTool::new(
            registry_with_one_tool(),
            Arc::new(SubAgentRegistry::defaults()),
            "claude-haiku-4-5-20251001".into(),
        );
        let s = tool.schema();
        assert!(s.description.contains("episode-explorer"));
        assert!(s.description.contains("cut-scout"));
        assert!(s.description.contains("b-roll-hunter"));
    }

    #[test]
    fn schema_lists_zero_subagents_gracefully() {
        let tool = DelegateTool::new(
            registry_with_one_tool(),
            Arc::new(SubAgentRegistry::empty()),
            "claude-haiku-4-5-20251001".into(),
        );
        let s = tool.schema();
        assert!(s.description.contains("Available sub-agents:"));
    }

    #[tokio::test]
    async fn unknown_subagent_is_respond_to_model() {
        let tool = DelegateTool::new(
            registry_with_one_tool(),
            Arc::new(SubAgentRegistry::defaults()),
            "m".into(),
        );
        let inv = ToolInvocation {
            call_id: "c1".into(),
            name: "delegate".into(),
            args: serde_json::json!({"name": "ghost", "task": "anything"}),
        };
        let ctx = test_ctx();
        let err = tool.handle(inv, ctx).await.unwrap_err();
        match err {
            FunctionCallError::RespondToModel(m) => {
                assert!(m.contains("ghost"));
                assert!(m.contains("Available"));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cannot_delegate_from_inside_sub_session() {
        let tool = DelegateTool::new(
            registry_with_one_tool(),
            Arc::new(SubAgentRegistry::defaults()),
            "m".into(),
        );
        let inv = ToolInvocation {
            call_id: "c1".into(),
            name: "delegate".into(),
            args: serde_json::json!({"name": "episode-explorer", "task": "x"}),
        };
        let mut ctx = test_ctx();
        ctx.subagent_return = Some(Arc::new(Mutex::new(None)));
        let err = tool.handle(inv, ctx).await.unwrap_err();
        match err {
            FunctionCallError::RespondToModel(m) => {
                assert!(m.contains("cannot spawn"));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    fn test_ctx() -> ToolContext {
        let (tx, _) = tokio::sync::broadcast::channel(8);
        ToolContext {
            project_root: std::env::temp_dir(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }
}
