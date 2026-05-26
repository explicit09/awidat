//! `update_plan` — record the agent's current editorial plan. Ported
//! from `crates/core/src/tools/update_plan.rs` to the in-process MCP
//! server.
//!
//! In the old harness this tool broadcast a `SessionEvent::EditPlanUpdate`
//! so the TUI/REPL could render the plan as a checklist next to the
//! timeline. The MCP server has no enclosing `Session` and no event
//! bus, so the broadcast goes away. The remaining contract is: validate
//! the plan, echo it back as the tool result so the model has a stable
//! record in its own transcript, and let the client decide what (if
//! anything) to do with the structured plan items.
//!
//! Annotated `read_only_hint = true`: no filesystem writes, no event
//! broadcast, no external side-effects.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// One item in the agent's editorial plan. Local to this module to keep
/// the MCP port free of `crate::tool::PlanItem` (which lives in the
/// legacy harness module slated for deletion in step 7).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct PlanItem {
    /// Short imperative description of the step.
    pub step: String,
    /// One of `pending | in_progress | completed`.
    pub status: String,
}

/// Arguments to `update_plan`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct UpdatePlanArgs {
    /// Ordered list of plan steps. Convention: at most one item should
    /// be `in_progress` at a time (not enforced in code).
    #[serde(default)]
    pub items: Vec<PlanItem>,
    /// Optional one-sentence note about why the plan changed.
    #[serde(default)]
    pub note: Option<String>,
}

pub fn run(args: UpdatePlanArgs, _ctx: McpToolCtx) -> Result<String, String> {
    for (i, item) in args.items.iter().enumerate() {
        if !matches!(
            item.status.as_str(),
            "pending" | "in_progress" | "completed"
        ) {
            return Err(format!(
                "update_plan: items[{i}].status = {:?} — must be one of \
                 pending | in_progress | completed",
                item.status
            ));
        }
    }

    serde_json::to_string_pretty(&serde_json::json!({
        "status": "plan_recorded",
        "items": args.items,
        "note": args.note,
    }))
    .map_err(|e| format!("update_plan: failed to serialize plan response: {e}"))
}

pub const DESCRIPTION: &str = "\
Record the current editorial plan as an ordered list of steps. Use this \
to make your reasoning visible: what you've done, what you're doing now, \
what's left. Convention: at most one step `in_progress` at a time. The \
MCP port echoes the plan back as a JSON record — there is no event \
broadcast or persisted file in this server; the plan exists in the \
conversation transcript. Call again whenever the plan changes \
meaningfully (don't spam every micro-step).";
