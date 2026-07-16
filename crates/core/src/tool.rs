//! Small standalone types left over from the legacy in-process tool
//! dispatch tree (`ToolHandler`/`ToolRegistry`/`ToolContext`/...), which
//! was deleted once the codex subprocess in `vendor/codex-rs/` became the
//! only agent loop. Everything in the original tree was dead except these
//! two types, which still have real, non-test consumers:
//!
//! - [`ApprovalDecision`] — the desktop app's agent-initiated-proposal
//!   reply channel (`apps/desktop/src-tauri/src/commands/proposal.rs`,
//!   `state.rs`) uses this exact three-way enum for its own, unrelated
//!   accept/adjust/deny flow. It is a plain value type with no
//!   dependency on the rest of the deleted tree.
//! - [`PlanItem`] — referenced by [`crate::events::SessionEvent::EditPlanUpdate`]
//!   so that type still resolves. `events.rs` is itself legacy
//!   forward-compat scaffolding (see its module doc) kept out of scope
//!   for this cleanup; `PlanItem` stays alongside it rather than forcing
//!   an edit to `events.rs`.

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

/// User decision on a mutating tool call.
///
/// Three-way to match the Codex/Aider convention: one-shot allow, sticky
/// allow for the rest of the session, or deny (with the model seeing an
/// `is_error` tool result so it can self-correct).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Allow this single invocation.
    Allow,
    /// Allow this exact operation key for the rest of the session.
    /// Future calls to materially different operations still prompt.
    AllowForSession,
    /// Reject. The model sees a tool result with `is_error: true` and
    /// "user denied execution" text so it can route around it.
    Deny,
}
