//! `attempt_completion` — a sub-agent's only path back to the parent.
//!
//! Per `crates/core/src/subagent.rs` and the cline pattern (`SubagentBuilder.ts:21`):
//! the sub-agent's `result` argument is sent verbatim to the parent. We
//! implement that as a `tokio::sync::Mutex<Option<String>>` mounted on
//! the sub-session via `ToolContext.subagent_return`. Calling
//! `attempt_completion` writes the result string and returns text — the
//! sub-agent's loop sees it as a completed tool call. The `delegate`
//! tool, after running the sub-session, reads the slot to get the
//! payload to return to the parent.
//!
//! V1 contract: only the FIRST `attempt_completion` call wins.
//! Subsequent calls return RespondToModel("already completed") so a
//! confused sub-agent can't overwrite its own report.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `attempt_completion` tool. Mounted only on sub-sessions.
pub struct AttemptCompletionTool;

#[derive(Debug, Deserialize)]
struct AttemptCompletionArgs {
    result: String,
}

#[async_trait]
impl ToolHandler for AttemptCompletionTool {
    fn name(&self) -> &'static str {
        "attempt_completion"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "attempt_completion".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "result": {
                        "type": "string",
                        "description": "The full final answer for the parent agent. Sent verbatim — make it count. Lead with the answer, then bullet the evidence."
                    }
                },
                "required": ["result"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        // attempt_completion writes only to the sub-session's return
        // slot — it doesn't mutate project state, no approval needed.
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: AttemptCompletionArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "attempt_completion: invalid args ({e}). Required: {{ \"result\": <str> }}."
            ))
        })?;

        let Some(slot) = ctx.subagent_return.as_ref() else {
            return Err(FunctionCallError::RespondToModel(
                "attempt_completion was called outside a sub-agent context. \
                 This tool only makes sense inside a `delegate` call. The \
                 parent agent should answer the user directly instead."
                    .into(),
            ));
        };

        let mut guard = slot.lock().await;
        if guard.is_some() {
            return Err(FunctionCallError::RespondToModel(
                "attempt_completion was already called this sub-session. \
                 The first result has been recorded; the parent will see \
                 it. End your turn now."
                    .into(),
            ));
        }
        *guard = Some(args.result);
        Ok(ToolOutput::text(
            "Result recorded. End your turn now — the parent agent will receive it.",
        ))
    }
}

const DESCRIPTION: &str = "\
Send your final answer to the parent agent. The `result` string is \
forwarded verbatim, so make it the complete, self-contained answer the \
parent needs. After calling this, end your turn — don't call other \
tools.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;
    use std::sync::Arc;
    use tokio::sync::{Mutex, broadcast};

    fn ctx_with_slot(slot: Arc<Mutex<Option<String>>>) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: std::env::temp_dir(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: Some(slot),
        }
    }

    fn ctx_no_slot() -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: std::env::temp_dir(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "attempt_completion".into(),
            args,
        }
    }

    #[tokio::test]
    async fn writes_result_to_slot() {
        let slot = Arc::new(Mutex::new(None));
        let ctx = ctx_with_slot(slot.clone());
        let out = AttemptCompletionTool
            .handle(invoke(serde_json::json!({"result": "hello"})), ctx)
            .await
            .unwrap();
        assert!(out.content.contains("Result recorded"));
        assert_eq!(slot.lock().await.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn second_call_is_respond_to_model() {
        let slot = Arc::new(Mutex::new(Some("first".into())));
        let ctx = ctx_with_slot(slot);
        let err = AttemptCompletionTool
            .handle(invoke(serde_json::json!({"result": "second"})), ctx)
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(m) => assert!(m.contains("already called")),
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn outside_subagent_context_is_respond_to_model() {
        let err = AttemptCompletionTool
            .handle(invoke(serde_json::json!({"result": "x"})), ctx_no_slot())
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(m) => {
                assert!(m.contains("outside a sub-agent"))
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_result_arg_is_respond_to_model() {
        let slot = Arc::new(Mutex::new(None));
        let ctx = ctx_with_slot(slot);
        let err = AttemptCompletionTool
            .handle(invoke(serde_json::json!({})), ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(_)));
    }
}
