//! Centralized tool orchestration for approvals and sandbox retry policy.
//!
//! This is intentionally much smaller than Codex's runtime layer. Awidat's
//! tool surface is fixed and video-editing oriented, so the orchestrator
//! owns only the cross-cutting behavior that was previously ad hoc in
//! `Session`: mutating-tool approval, operation-keyed approval caching,
//! and sandbox-denial escalation for sandbox-capable tools.

use std::sync::Arc;

use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::FunctionCallError;
use crate::rollout::Recorder;
use crate::session::SessionError;
use crate::tool::{
    ApprovalCache, ApprovalDecision, ApprovalRequest, SandboxMode, ToolContext, ToolHandler,
    ToolInvocation, ToolOutput, ToolSandboxPolicy,
};

/// Result of an orchestrated tool call.
pub type OrchestratorResult = Result<ToolOutput, FunctionCallError>;

/// Small Awidat-specific tool orchestrator.
#[derive(Clone)]
pub struct ToolOrchestrator {
    approval_tx: Option<mpsc::Sender<ApprovalRequest>>,
    approval_cache: Arc<Mutex<ApprovalCache>>,
    recorder: Option<Recorder>,
}

impl ToolOrchestrator {
    /// Create an orchestrator over session-scoped approval state.
    pub fn new(
        approval_tx: Option<mpsc::Sender<ApprovalRequest>>,
        approval_cache: Arc<Mutex<ApprovalCache>>,
        recorder: Option<Recorder>,
    ) -> Self {
        Self {
            approval_tx,
            approval_cache,
            recorder,
        }
    }

    /// Run a tool invocation through approval, sandbox policy, and handler dispatch.
    pub async fn run(
        &self,
        handler: Arc<dyn ToolHandler>,
        invocation: ToolInvocation,
        base_ctx: ToolContext,
        cancel: &CancellationToken,
    ) -> Result<OrchestratorResult, SessionError> {
        let sandbox_policy = handler.sandbox_policy(&invocation);
        self.ensure_approved(handler.as_ref(), &invocation, None, cancel)
            .await?;

        let first_result = self
            .run_attempt(
                handler.clone(),
                invocation.clone(),
                base_ctx.clone(),
                SandboxMode::Default,
                cancel,
            )
            .await?;

        match first_result {
            Err(FunctionCallError::SandboxDenied { reason })
                if matches!(
                    sandbox_policy,
                    ToolSandboxPolicy::SandboxFirst {
                        escalate_on_denial: true
                    }
                ) =>
            {
                let retry_reason = format!(
                    "sandbox denied '{}': {reason}. Retry unsandboxed?",
                    invocation.name
                );
                self.ensure_approved(handler.as_ref(), &invocation, Some(&retry_reason), cancel)
                    .await?;
                self.run_attempt(handler, invocation, base_ctx, SandboxMode::Bypass, cancel)
                    .await
            }
            other => Ok(other),
        }
    }

    async fn run_attempt(
        &self,
        handler: Arc<dyn ToolHandler>,
        invocation: ToolInvocation,
        mut ctx: ToolContext,
        sandbox_mode: SandboxMode,
        cancel: &CancellationToken,
    ) -> Result<OrchestratorResult, SessionError> {
        ctx.sandbox_mode = sandbox_mode;
        let call_id = invocation.call_id.clone();
        let name = invocation.name.clone();
        let result = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(SessionError::Cancelled),
            res = handler.handle(invocation, ctx) => res,
        };
        if matches!(result, Err(FunctionCallError::SandboxDenied { .. }))
            && sandbox_mode == SandboxMode::Default
        {
            tracing::debug!(call_id, tool = name, "tool reported sandbox denial");
        }
        Ok(result)
    }

    async fn ensure_approved(
        &self,
        handler: &dyn ToolHandler,
        invocation: &ToolInvocation,
        retry_reason: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<(), SessionError> {
        if !handler.is_mutating(invocation) {
            return Ok(());
        }
        let Some(approval_tx) = self.approval_tx.as_ref() else {
            return Ok(());
        };

        let mut keys = handler.approval_keys(invocation);
        if retry_reason.is_some() {
            for key in &mut keys {
                key.operation.push_str(":unsandboxed");
            }
        }
        if self.approval_cache.lock().await.contains_all(&keys) {
            return Ok(());
        }

        let mut summary = handler.approval_summary(invocation);
        if let Some(reason) = retry_reason {
            summary = format!("{reason}\n{summary}");
        }
        let decision = request_approval(approval_tx, invocation, summary, cancel).await?;

        if let Some(rec) = &self.recorder {
            rec.record_decision(
                invocation.name.clone(),
                handler.approval_summary(invocation),
                format!("{decision:?}"),
            );
        }

        match decision {
            ApprovalDecision::Allow => Ok(()),
            ApprovalDecision::AllowForSession => {
                self.approval_cache.lock().await.approve_for_session(keys);
                Ok(())
            }
            ApprovalDecision::Deny => Err(SessionError::ToolDenied {
                call_id: invocation.call_id.clone(),
                tool_name: invocation.name.clone(),
                message: format!("user denied execution of '{}'", invocation.name),
            }),
        }
    }
}

async fn request_approval(
    approval_tx: &mpsc::Sender<ApprovalRequest>,
    invocation: &ToolInvocation,
    args_summary: String,
    cancel: &CancellationToken,
) -> Result<ApprovalDecision, SessionError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    let req = ApprovalRequest {
        call_id: invocation.call_id.clone(),
        tool_name: invocation.name.clone(),
        args_summary,
        args_full: invocation.args.clone(),
        reply: reply_tx,
    };
    if approval_tx.send(req).await.is_err() {
        warn!("approval channel closed mid-turn; defaulting to allow");
        return Ok(ApprovalDecision::Allow);
    }
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(SessionError::Cancelled),
        decision = reply_rx => Ok(decision.unwrap_or(ApprovalDecision::Deny)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tokio::sync::broadcast;

    use crate::anthropic::Tool as ToolSchema;
    use crate::tool::ApprovalKey;

    struct FakeMutating;

    #[async_trait]
    impl ToolHandler for FakeMutating {
        fn name(&self) -> &'static str {
            "fake_mutating"
        }

        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "fake_mutating".into(),
                description: "test".into(),
                input_schema: serde_json::json!({"type": "object"}),
                cache_control: None,
            }
        }

        fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
            vec![ApprovalKey::new(
                invocation.name.clone(),
                invocation.args["target"].as_str().unwrap_or("unknown"),
            )]
        }

        async fn handle(
            &self,
            invocation: ToolInvocation,
            _ctx: ToolContext,
        ) -> Result<ToolOutput, FunctionCallError> {
            Ok(ToolOutput::text(format!(
                "ok {}",
                invocation.args["target"]
            )))
        }
    }

    fn ctx() -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: std::env::temp_dir(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn inv(target: &str) -> ToolInvocation {
        ToolInvocation {
            call_id: format!("call-{target}"),
            name: "fake_mutating".into(),
            args: serde_json::json!({"target": target}),
        }
    }

    #[tokio::test]
    async fn allow_for_session_is_keyed_by_operation_not_tool_name() {
        let (tx, mut rx) = mpsc::channel(8);
        let orchestrator = ToolOrchestrator::new(
            Some(tx),
            Arc::new(Mutex::new(ApprovalCache::default())),
            None,
        );
        let handler: Arc<dyn ToolHandler> = Arc::new(FakeMutating);
        let cancel = CancellationToken::new();

        let approver = tokio::spawn(async move {
            let first = rx.recv().await.unwrap();
            assert!(first.args_summary.contains("alpha"));
            first.reply.send(ApprovalDecision::AllowForSession).unwrap();
            let second = rx.recv().await.unwrap();
            assert!(second.args_summary.contains("beta"));
            second.reply.send(ApprovalDecision::Allow).unwrap();
        });

        let out = orchestrator
            .run(handler.clone(), inv("alpha"), ctx(), &cancel)
            .await
            .unwrap()
            .unwrap();
        assert!(out.content.contains("alpha"));

        let out = orchestrator
            .run(handler.clone(), inv("alpha"), ctx(), &cancel)
            .await
            .unwrap()
            .unwrap();
        assert!(out.content.contains("alpha"));

        let out = orchestrator
            .run(handler, inv("beta"), ctx(), &cancel)
            .await
            .unwrap()
            .unwrap();
        assert!(out.content.contains("beta"));
        approver.await.unwrap();
    }
}
