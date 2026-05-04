//! `delegate_all` — spawn N persona reviewers in PARALLEL on the same
//! task and return a consolidated review.
//!
//! Per the cross-harness survey: building blocks for parallel
//! sub-agents exist (cline `SubagentBuilder`, claude-agent-sdk
//! `AgentDefinition`, goose subagent), but no harness ships
//! parallel-personas-as-a-feature. This is the original work — the
//! infrastructure is the same as `delegate`, but parallelization +
//! consolidated output is the editorial product.
//!
//! V1 scope:
//! - parallel via `tokio::join_all` over up to N=4 personas
//! - each persona gets the SAME task; the persona's `system_suffix`
//!   tells it what lens to apply
//! - consolidated output is a single string with one `## <persona>`
//!   section per result, persona-id-tagged
//! - failures (no `attempt_completion`, sub-session error) surface
//!   per-persona, not as a whole-tool fail — partial reviews are
//!   useful

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::FunctionCallError;
use crate::anthropic::{Client, ClientConfig, Tool as ToolSchema};
use crate::subagent::SubAgentKind;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput, ToolRegistry};

/// Maximum personas spawnable per `delegate_all` call. Bounded to keep
/// API cost predictable — a 4-way persona review is enough breadth for
/// the editorial use case, and capping prevents the parent from doing
/// runaway fan-out.
const MAX_PARALLEL: usize = 4;

/// The `delegate_all` tool. Mounted on the parent session only.
pub struct DelegateAllTool {
    parent_registry: ToolRegistry,
    subagents: Arc<crate::subagent::SubAgentRegistry>,
    default_model: String,
}

impl DelegateAllTool {
    /// Construct bound to the parent registry + sub-agent catalog.
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
struct DelegateAllArgs {
    personas: Vec<String>,
    task: String,
}

#[async_trait]
impl ToolHandler for DelegateAllTool {
    fn name(&self) -> &'static str {
        "delegate_all"
    }

    fn schema(&self) -> ToolSchema {
        let catalog = self
            .subagents
            .iter_kind(SubAgentKind::Persona)
            .map(|a| format!("  - {}: {}", a.name, a.description))
            .collect::<Vec<_>>()
            .join("\n");
        let description = format!(
            "{base}\n\nAvailable personas:\n{catalog}",
            base = DESCRIPTION_BASE,
            catalog = catalog,
        );
        ToolSchema {
            name: "delegate_all".into(),
            description,
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "personas": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": MAX_PARALLEL,
                        "description": "List of persona ids to spawn in parallel. Each persona reviews the SAME task from its lens; their reports come back as a consolidated review."
                    },
                    "task": {
                        "type": "string",
                        "description": "What to review. Be specific — every persona sees this verbatim, so frame it once for all of them. Example: 'Review the current cut from 0:00–28:00 for any issues.'"
                    }
                },
                "required": ["personas", "task"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: DelegateAllArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "delegate_all: invalid args ({e}). Required: \
                 {{ \"personas\": [<id>, ...], \"task\": <str> }}."
            ))
        })?;

        // Reject delegating from inside a sub-session (matches
        // delegate's rule).
        if ctx.subagent_return.is_some() {
            return Err(FunctionCallError::RespondToModel(
                "delegate_all: sub-agents cannot spawn their own reviewers. \
                 Return your findings via attempt_completion."
                    .into(),
            ));
        }

        if args.personas.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "delegate_all: `personas` was empty. Pass at least one persona id.".into(),
            ));
        }
        if args.personas.len() > MAX_PARALLEL {
            return Err(FunctionCallError::RespondToModel(format!(
                "delegate_all: requested {} personas; the cap is {}. \
                 Pick the {} most-relevant lenses for this review.",
                args.personas.len(),
                MAX_PARALLEL,
                MAX_PARALLEL,
            )));
        }
        // Dedup so a model passing ["editor","editor"] doesn't double-pay.
        let unique: Vec<String> = {
            let mut seen = HashSet::new();
            args.personas
                .into_iter()
                .filter(|p| seen.insert(p.clone()))
                .collect()
        };

        // Resolve specs up-front; an unknown persona aborts the whole
        // call so we don't half-spawn and then surface a confusing
        // partial result.
        let mut specs = Vec::with_capacity(unique.len());
        for name in &unique {
            let Some(spec) = self.subagents.get(name) else {
                let available = self
                    .subagents
                    .iter_kind(SubAgentKind::Persona)
                    .map(|a| a.name)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(FunctionCallError::RespondToModel(format!(
                    "delegate_all: no persona named '{name}'. \
                     Available personas: {available}."
                )));
            };
            if spec.kind != SubAgentKind::Persona {
                return Err(FunctionCallError::RespondToModel(format!(
                    "delegate_all: '{name}' is a research sub-agent, not a \
                     persona. Use `delegate(name=\"{name}\", task=...)` instead."
                )));
            }
            specs.push(spec);
        }

        // Build one client per call — sub-sessions reuse it via clone.
        // (The Client itself is cheap to clone; underlying reqwest
        // pool is shared.) Failure to construct the client aborts
        // before any sub-spawn so we don't waste API calls.
        let client = Client::from_env_or_keychain(ClientConfig::default()).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "delegate_all: could not build client: {e}. \
                 Make sure ANTHROPIC_API_KEY is set or stored in the keychain."
            ))
        })?;

        // Surface progress to the parent's UI.
        let _ = ctx.events_tx.send(crate::SessionEvent::TextDelta(format!(
            "\n[delegate_all → {} personas: {}]\n{}\n",
            specs.len(),
            specs
                .iter()
                .map(|s| s.name.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            args.task,
        )));

        // Fan out. Each persona gets its own sub-session, its own
        // return slot, its own client clone.
        let mut futures = Vec::with_capacity(specs.len());
        for spec in &specs {
            let spec = spec.clone();
            let client = client.clone();
            let parent_registry = self.parent_registry.clone();
            let project_root = ctx.project_root.clone();
            let task = args.task.clone();
            let model = self.default_model.clone();

            futures.push(tokio::spawn(async move {
                run_one_persona(spec, client, parent_registry, project_root, task, model).await
            }));
        }

        // Collect. Each future returns Result<(name, body), (name, err)>.
        let results = futures::future::join_all(futures).await;

        // Render. One `## <persona>` section per result; failures get
        // a "(no review returned: <reason>)" line so the parent can
        // see what didn't come back without parsing nothing.
        let mut out = String::new();
        out.push_str("# Persona review\n\n");
        for (i, joined) in results.into_iter().enumerate() {
            let name = specs[i].name;
            match joined {
                Ok(Ok(body)) => {
                    out.push_str(&format!("## {name}\n\n{body}\n\n"));
                }
                Ok(Err(reason)) => {
                    out.push_str(&format!(
                        "## {name}\n\n(no review returned: {reason})\n\n"
                    ));
                }
                Err(join_err) => {
                    out.push_str(&format!(
                        "## {name}\n\n(no review returned: task panicked — {join_err})\n\n"
                    ));
                }
            }
        }

        Ok(ToolOutput::text(out))
    }
}

async fn run_one_persona(
    spec: Arc<crate::subagent::SubAgent>,
    client: Client,
    parent_registry: ToolRegistry,
    project_root: std::path::PathBuf,
    task: String,
    model: String,
) -> Result<String, String> {
    let mut sub_registry = parent_registry.restrict_to(spec.allowed_tools);
    sub_registry.register(Arc::new(crate::tools::attempt_completion::AttemptCompletionTool));

    let sys = format!(
        "You are persona `{}`.\n\n{}{}",
        spec.name, spec.description, spec.system_suffix,
    );

    let return_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let session = crate::Session::new(
        client,
        sub_registry,
        model,
        Some(sys),
        project_root,
    )
    .with_subagent_return(return_slot.clone());

    let cancel = CancellationToken::new();
    let _ = session
        .run_turn_capped(task, cancel, spec.max_iterations)
        .await;

    let body = return_slot.lock().await.clone();
    body.ok_or_else(|| {
        format!(
            "persona '{}' ended without calling attempt_completion",
            spec.name
        )
    })
}

const DESCRIPTION_BASE: &str = "\
Spawn multiple PERSONA reviewers in PARALLEL on the same task and \
return a consolidated review. Each persona is a read-only sub-agent \
with a specific editorial lens (editor / fact-checker / brand-voice \
/ etc.); they critique the SAME thing from different angles, and \
their reports come back as one consolidated message with a section \
per persona.\
\n\nUse for: 'review the current cut', 'what would a fact-checker \
find here', 'sanity-check this draft from multiple angles before \
shipping'. Cap is 4 parallel personas per call. Personas cannot \
mutate state (no apply_edl, no start_render). For research \
questions, use `delegate` instead.\
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
    fn schema_lists_personas_in_description() {
        let tool = DelegateAllTool::new(
            registry_with_one_tool(),
            Arc::new(SubAgentRegistry::defaults()),
            "claude-haiku-4-5-20251001".into(),
        );
        let s = tool.schema();
        assert!(s.description.contains("editor"));
        assert!(s.description.contains("fact-checker"));
        assert!(s.description.contains("brand-voice"));
        // Research sub-agents must NOT appear here.
        assert!(!s.description.contains("episode-explorer"));
    }

    #[tokio::test]
    async fn empty_personas_is_respond_to_model() {
        let tool = DelegateAllTool::new(
            registry_with_one_tool(),
            Arc::new(SubAgentRegistry::defaults()),
            "m".into(),
        );
        let inv = ToolInvocation {
            call_id: "c1".into(),
            name: "delegate_all".into(),
            args: serde_json::json!({"personas": [], "task": "x"}),
        };
        let err = tool.handle(inv, test_ctx()).await.unwrap_err();
        match err {
            FunctionCallError::RespondToModel(m) => assert!(m.contains("empty")),
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn over_cap_personas_is_respond_to_model() {
        let tool = DelegateAllTool::new(
            registry_with_one_tool(),
            Arc::new(SubAgentRegistry::defaults()),
            "m".into(),
        );
        let inv = ToolInvocation {
            call_id: "c1".into(),
            name: "delegate_all".into(),
            args: serde_json::json!({
                "personas": ["editor", "fact-checker", "brand-voice", "editor", "editor"],
                "task": "x",
            }),
        };
        let err = tool.handle(inv, test_ctx()).await.unwrap_err();
        match err {
            FunctionCallError::RespondToModel(m) => assert!(m.contains("cap is")),
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_persona_is_respond_to_model() {
        let tool = DelegateAllTool::new(
            registry_with_one_tool(),
            Arc::new(SubAgentRegistry::defaults()),
            "m".into(),
        );
        let inv = ToolInvocation {
            call_id: "c1".into(),
            name: "delegate_all".into(),
            args: serde_json::json!({
                "personas": ["editor", "ghost"],
                "task": "x",
            }),
        };
        let err = tool.handle(inv, test_ctx()).await.unwrap_err();
        match err {
            FunctionCallError::RespondToModel(m) => {
                assert!(m.contains("ghost"));
                assert!(m.contains("editor")); // listed in available
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn research_subagent_passed_as_persona_is_respond_to_model() {
        let tool = DelegateAllTool::new(
            registry_with_one_tool(),
            Arc::new(SubAgentRegistry::defaults()),
            "m".into(),
        );
        let inv = ToolInvocation {
            call_id: "c1".into(),
            name: "delegate_all".into(),
            args: serde_json::json!({
                "personas": ["episode-explorer"],
                "task": "x",
            }),
        };
        let err = tool.handle(inv, test_ctx()).await.unwrap_err();
        match err {
            FunctionCallError::RespondToModel(m) => {
                assert!(m.contains("research sub-agent"));
                assert!(m.contains("delegate"));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cannot_call_from_inside_sub_session() {
        let tool = DelegateAllTool::new(
            registry_with_one_tool(),
            Arc::new(SubAgentRegistry::defaults()),
            "m".into(),
        );
        let inv = ToolInvocation {
            call_id: "c1".into(),
            name: "delegate_all".into(),
            args: serde_json::json!({
                "personas": ["editor"],
                "task": "x",
            }),
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
