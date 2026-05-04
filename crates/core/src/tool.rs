//! Tool trait + registry. Lifted from
//! `harnesses/codex/codex-rs/core/src/tools/registry.rs:44-92`, narrowed
//! to the minimum-viable trait per the corpus survey.
//!
//! Codex's full trait carries 7 methods (kind, matches_kind, is_mutating,
//! pre_tool_use_payload, post_tool_use_payload, create_diff_consumer,
//! handle). We keep `is_mutating()` and `handle()`. The hooks land in
//! week 6+ when we wire skills; the diff consumer lands in week 4 with
//! `apply_edl`.
//!
//! # Mutating-vs-not gating
//!
//! Per the survey: **default `is_mutating()` to `true`**. Codex's default
//! is `false` and they override to `true` for ShellHandler unless the
//! command is on a known-safe list — which means a brand-new tool is
//! parallelizable by default, which is the wrong-direction default. We
//! flip it: tools that are genuinely read-only override to `false`.
//!
//! For week 3 with one tool (`bash`, mutating), the `is_mutating` bit
//! buys us nothing. Wiring the trait method now means week 4+ tools slot
//! in without a refactor.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;

/// One in-flight tool call. Built by the agent loop from a parsed
/// `ToolCallEnd` event.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    /// Server-allocated id from the matching `tool_use` block. The agent
    /// loop echoes this back as `tool_use_id` on the result.
    pub call_id: String,
    /// Tool name dispatched.
    pub name: String,
    /// Args, JSON-shaped per the tool's input schema.
    pub args: serde_json::Value,
}

/// One item in the agent's editorial plan, surfaced via `update_plan`
/// and consumed by [`crate::SessionEvent::EditPlanUpdate`]. Mirrors
/// codex `plan_tool`'s shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlanItem {
    /// Step text.
    pub step: String,
    /// Status: `pending | in_progress | completed`. The `at most one
    /// in_progress` invariant lives in the tool's schema description; we
    /// don't enforce in code.
    pub status: String,
}

/// Per-call context the agent loop hands to a tool handler.
///
/// Most tools ignore most fields. Tools that need to **emit events**
/// (`update_plan`), **suspend the turn awaiting user input**
/// (`request_user_input`), or **start/poll long-running renders**
/// (`start_render` / `poll_render`) consume the relevant field.
///
/// Cheaply cloneable; handlers should clone before crossing await points
/// to satisfy `Send` bounds.
#[derive(Clone)]
pub struct ToolContext {
    /// Project root the session was opened against.
    pub project_root: std::path::PathBuf,
    /// Broadcast channel the loop publishes [`crate::SessionEvent`]s on.
    /// Tools can publish their own events here too — the REPL/TUI subscribe.
    pub events_tx: broadcast::Sender<crate::SessionEvent>,
    /// Channel for `request_user_input`-shaped suspension. Sending a
    /// [`UserInputRequest`] here causes the REPL/TUI to surface a prompt;
    /// the response arrives via the embedded `oneshot::Sender<String>`.
    /// `None` if the runtime doesn't support input suspension.
    pub user_input_tx: Option<mpsc::Sender<UserInputRequest>>,
    /// Shared render-job manager. Start jobs via `start_render`, poll
    /// them via `poll_render`. Survives across tool calls within a
    /// session.
    pub job_manager: awidat_render::JobManager,
    /// Channel the agent loop uses to ask the front-end for approval
    /// before dispatching a mutating tool. Tools normally do not touch
    /// this — the loop owns the gate. Surfaced on `ToolContext` only so
    /// future work-spawning tools (week 6+) can recursively request
    /// approvals for sub-calls. `None` ⇒ loop defaults to allow.
    pub approval_tx: Option<mpsc::Sender<ApprovalRequest>>,
    /// Session-scoped MCP host. Tools that need warm ML processes
    /// (e.g. `clip_search` calling `clip-mcp`'s `query_text`) call
    /// `mcp_host.call_tool(server, tool, args)`. The first call to any
    /// given server in the session lazily spawns it; subsequent calls
    /// reuse the warm subprocess. Empty host (no registered servers)
    /// means MCP-backed tools will return `UnknownServer` errors —
    /// pure-Rust tools are unaffected.
    pub mcp_host: crate::mcp_host::McpHost,
    /// Skills discovered at session start. Read by the `load_skill`
    /// tool to return a full L2 body on demand. The L1 catalog
    /// (one line per skill) is already in the system prompt; this
    /// Arc holds the bodies + metadata for L2 disclosure.
    pub skills: std::sync::Arc<crate::skills::SkillRegistry>,
}

/// One pending approval request emitted by the agent loop before it
/// dispatches a mutating tool. The REPL/TUI consumes it, asks the user,
/// and sends the [`ApprovalDecision`] back via the embedded oneshot.
///
/// The agent loop builds these from `ToolHandler::is_mutating(&inv) == true`
/// before calling `handle()`. If the session has no approval channel the
/// loop defaults to `Allow` so non-interactive runtimes (tests, batch CLI,
/// MCP server) keep working.
#[derive(Debug)]
pub struct ApprovalRequest {
    /// Tool call id for correlation in the UI.
    pub call_id: String,
    /// Tool name (e.g. `apply_edl`, `start_render`, `bash`).
    pub tool_name: String,
    /// Short human-readable summary of the args. Caller-built — the loop
    /// keeps the full args off the wire so the UI doesn't render a wall
    /// of JSON. Tools that want richer summaries can override later via
    /// `ToolHandler::approval_summary` (week 6+).
    pub args_summary: String,
    /// Reply channel. The receiver lives in the loop; the UI sends the
    /// user's decision here. Dropping the oneshot signals `Deny`.
    pub reply: oneshot::Sender<ApprovalDecision>,
}

/// User decision on a mutating tool call.
///
/// Three-way to match the Codex/Aider convention: one-shot allow, sticky
/// allow for the rest of the session, or deny (with the model seeing an
/// `is_error` tool result so it can self-correct).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Allow this single invocation.
    Allow,
    /// Allow this tool name for the rest of the session — future calls
    /// to the same tool skip the modal. The loop tracks this set.
    AllowForSession,
    /// Reject. The model sees a tool result with `is_error: true` and
    /// "user denied execution" text so it can route around it.
    Deny,
}

/// One pending user-input request emitted by `request_user_input`. The
/// REPL/TUI consumes it, prompts the user, and sends the response back via
/// the embedded oneshot. Dropping the oneshot signals "cancelled".
#[derive(Debug)]
pub struct UserInputRequest {
    /// Tool call id for correlation in the UI.
    pub call_id: String,
    /// Question to display.
    pub question: String,
    /// Optional choice list (multiple-choice question).
    pub options: Option<Vec<String>>,
    /// Optional default the UI can pre-fill.
    pub default: Option<String>,
    /// Reply channel. The receiver lives inside the tool handler; the
    /// REPL/TUI sends the user's chosen string here.
    pub reply: oneshot::Sender<String>,
}

/// Tool result. Most tools return text; multimodal tools (`view_frame`)
/// return text + image content blocks via [`Self::with_images`].
///
/// The agent loop converts this to a [`crate::anthropic::ContentBlock::ToolResult`]
/// before posting back to the model.
#[derive(Debug, Clone, Default)]
pub struct ToolOutput {
    /// Text payload to feed back to the model.
    pub content: String,
    /// Optional image attachments. Each is `(media_type, base64_data)`.
    /// When non-empty, the loop emits the result as a multi-block array
    /// instead of a plain string.
    pub images: Vec<(String, String)>,
}

impl ToolOutput {
    /// Text-only result. The common case.
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            images: Vec::new(),
        }
    }

    /// Attach one or more base64-encoded images. `media_type` is the
    /// MIME (`"image/png"`, `"image/jpeg"`).
    #[must_use]
    pub fn with_images(mut self, images: Vec<(String, String)>) -> Self {
        self.images = images;
        self
    }
}

/// One tool the agent can call. Object-safe via `BoxFuture` (`async_trait`).
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Tool name. Stable string the model uses to dispatch.
    fn name(&self) -> &'static str;

    /// JSON Schema for the tool's args + a model-facing description.
    /// The agent loop builds an [`crate::anthropic::Tool`] from this for
    /// every request.
    fn schema(&self) -> ToolSchema;

    /// True iff invocation might mutate the environment. Defensive
    /// default: `true`. Override to `false` only for genuinely read-only
    /// tools so the future parallel-dispatch gate (week 5+) lets them run
    /// concurrently.
    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    /// Perform the call.
    ///
    /// `ctx` carries the broadcast event channel and the user-input
    /// suspension channel. Tools that don't need either can ignore it.
    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError>;
}

/// Name → handler lookup. Cheaply cloneable (handlers are `Arc<dyn>`).
#[derive(Clone, Default)]
pub struct ToolRegistry {
    handlers: HashMap<&'static str, Arc<dyn ToolHandler>>,
}

impl ToolRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler. Panics on duplicate name (tools are
    /// statically-known; a duplicate is a bug, not a runtime condition).
    #[allow(clippy::expect_used)]
    pub fn register(&mut self, handler: Arc<dyn ToolHandler>) {
        let name = handler.name();
        let prev = self.handlers.insert(name, handler);
        assert!(prev.is_none(), "duplicate tool registration: {name}");
    }

    /// Look up a handler by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.handlers.get(name).cloned()
    }

    /// Names of registered tools, in insertion order is NOT guaranteed
    /// (HashMap). Used for building the request's `tools` array — the
    /// model doesn't care about order.
    pub fn names(&self) -> impl Iterator<Item = &&'static str> {
        self.handlers.keys()
    }

    /// Build the schema list for an outgoing Messages request.
    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.handlers.values().map(|h| h.schema()).collect()
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Returns true iff no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;

    #[async_trait]
    impl ToolHandler for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "fake".into(),
                description: "test".into(),
                input_schema: serde_json::json!({"type": "object"}),
                cache_control: None,
            }
        }
        async fn handle(
            &self,
            _i: ToolInvocation,
            _ctx: ToolContext,
        ) -> Result<ToolOutput, FunctionCallError> {
            Ok(ToolOutput::text("ok"))
        }
    }

    fn fake_ctx() -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: std::env::temp_dir(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo { name: "test".into(), version: "0.0.0".into() }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
        }
    }

    #[test]
    fn register_and_lookup() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Fake));
        assert!(reg.get("fake").is_some());
        assert!(reg.get("missing").is_none());
        assert_eq!(reg.len(), 1);
        let names: Vec<&&str> = reg.names().collect();
        assert_eq!(names, vec![&"fake"]);
        let schemas = reg.schemas();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "fake");
    }

    #[test]
    #[should_panic(expected = "duplicate tool registration")]
    fn duplicate_registration_panics() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Fake));
        reg.register(Arc::new(Fake));
    }

    #[tokio::test]
    async fn handle_via_dyn() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Fake));
        let h = reg.get("fake").unwrap();
        let out = h
            .handle(
                ToolInvocation {
                    call_id: "c".into(),
                    name: "fake".into(),
                    args: serde_json::json!({}),
                },
                fake_ctx(),
            )
            .await
            .unwrap();
        assert_eq!(out.content, "ok");
    }

    #[test]
    fn defaults_to_mutating() {
        let f = Fake;
        let inv = ToolInvocation {
            call_id: "c".into(),
            name: "fake".into(),
            args: serde_json::json!({}),
        };
        assert!(f.is_mutating(&inv), "defensive default per survey");
    }
}
