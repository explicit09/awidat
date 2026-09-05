//! Record an editorial plan in the tool response so it remains in the
//! conversation transcript. The client decides how to display the items;
//! this tool does not write files or broadcast events.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// One item in the agent's editorial plan.
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
