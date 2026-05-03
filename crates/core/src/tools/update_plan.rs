//! `update_plan` tool. Lifted near-verbatim from
//! `harnesses/codex/codex-rs/core/src/tools/handlers/plan.rs`.
//!
//! As Codex's docstring puts it: "this function doesn't do anything useful.
//! However, it gives the model a structured way to record its plan that
//! clients can read and render. So it's the *inputs* to this function that
//! are useful to clients, not the outputs."
//!
//! The handler validates args, emits [`crate::SessionEvent::EditPlanUpdate`],
//! and returns the literal string `"Plan updated"`. The TUI / REPL
//! subscribes to the event.
//!
//! Schema-side: the `at most one in_progress` invariant is documented in
//! the tool description but **not enforced in code** — Codex doesn't
//! enforce either; the model figures it out and the user catches drift.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{PlanItem, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::session::SessionEvent;

/// The `update_plan` tool.
pub struct UpdatePlanTool;

#[derive(Debug, Deserialize)]
struct UpdatePlanArgs {
    items: Vec<PlanItem>,
    #[serde(default)]
    note: Option<String>,
}

#[async_trait]
impl ToolHandler for UpdatePlanTool {
    fn name(&self) -> &'static str {
        "update_plan"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "update_plan".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "description": "Ordered list of plan steps. At most one item should be in_progress at a time.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "step": { "type": "string", "description": "Short imperative description." },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"],
                                    "description": "One of pending | in_progress | completed."
                                }
                            },
                            "required": ["step", "status"]
                        }
                    },
                    "note": {
                        "type": "string",
                        "description": "Optional one-sentence note about why the plan changed."
                    }
                },
                "required": ["items"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        // Pure event emission; no filesystem or external side-effects.
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: UpdatePlanArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "update_plan: invalid args ({e}). Required: {{ \"items\": [{{ \"step\": \
                 <str>, \"status\": \"pending|in_progress|completed\" }}, ...] }}."
            ))
        })?;

        // Validate status values up front so the model gets a clean error
        // rather than the event consumer panicking later.
        for (i, item) in args.items.iter().enumerate() {
            if !matches!(item.status.as_str(), "pending" | "in_progress" | "completed") {
                return Err(FunctionCallError::RespondToModel(format!(
                    "update_plan: items[{i}].status = {:?} — must be one of \
                     pending | in_progress | completed",
                    item.status
                )));
            }
        }

        let _ = ctx.events_tx.send(SessionEvent::EditPlanUpdate {
            items: args.items,
            note: args.note,
        });
        Ok(ToolOutput::text("Plan updated"))
    }
}

const DESCRIPTION: &str = "\
Record the current editorial plan as an ordered list of steps. Use this to \
make your reasoning visible: what you've done, what you're doing now, what's \
left. The TUI renders this as a checklist next to the timeline. \
Convention: at most one step in_progress at a time. Call again whenever the \
plan changes meaningfully (don't spam every micro-step).\
";

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    fn ctx() -> (
        ToolContext,
        broadcast::Receiver<SessionEvent>,
    ) {
        let (tx, rx) = broadcast::channel(8);
        let c = ToolContext {
            project_root: std::env::temp_dir(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),

            approval_tx: None,
        };
        (c, rx)
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "update_plan".into(),
            args,
        }
    }

    #[tokio::test]
    async fn happy_path_emits_event_and_returns_plan_updated() {
        let (c, mut rx) = ctx();
        let out = UpdatePlanTool
            .handle(
                invoke(serde_json::json!({
                    "items": [
                        { "step": "transcribe asset", "status": "completed" },
                        { "step": "find highlight", "status": "in_progress" },
                        { "step": "draft EDL", "status": "pending" },
                    ]
                })),
                c,
            )
            .await
            .unwrap();
        assert_eq!(out.content, "Plan updated");
        let ev = rx.recv().await.expect("event");
        match ev {
            SessionEvent::EditPlanUpdate { items, .. } => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[1].status, "in_progress");
            }
            other => panic!("want EditPlanUpdate, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invalid_status_is_respond_to_model() {
        let (c, _rx) = ctx();
        let err = UpdatePlanTool
            .handle(
                invoke(serde_json::json!({
                    "items": [{ "step": "x", "status": "blocked" }]
                })),
                c,
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("blocked"));
                assert!(msg.contains("pending | in_progress | completed"));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_items_is_respond_to_model() {
        let (c, _rx) = ctx();
        let err = UpdatePlanTool
            .handle(invoke(serde_json::json!({})), c)
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(_)));
    }

    #[test]
    fn is_not_mutating() {
        // update_plan is pure event emission; should be parallelizable.
        let inv = invoke(serde_json::json!({}));
        assert!(!UpdatePlanTool.is_mutating(&inv));
    }
}
