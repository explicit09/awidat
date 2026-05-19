//! `poll_render` tool — read the latest status snapshot for a render job
//! started by `start_render`.
//!
//! Returns coarse state, % progress, ETA, parsed counters, and a tail-
//! truncated ffmpeg log excerpt. Frame-strip extraction (the survey's
//! "N evenly-spaced thumbs from the partial output") is deferred — until
//! a real apply_edl-driven render exists, the model can call `view_frame`
//! against the in-progress output once the file appears.

use async_trait::async_trait;
use awidat_render::{JobError, JobId};
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `poll_render` tool.
pub struct PollRenderTool;

#[derive(Debug, Deserialize)]
struct PollRenderArgs {
    job_id: String,
}

#[async_trait]
impl ToolHandler for PollRenderTool {
    fn name(&self) -> &'static str {
        "poll_render"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "poll_render".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "job_id": {
                        "type": "string",
                        "description": "Job id returned by start_render."
                    }
                },
                "required": ["job_id"]
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
        let args: PollRenderArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "poll_render: invalid args ({e}). Required: {{ \"job_id\": <str> }}."
            ))
        })?;

        let id = JobId(args.job_id);
        let status = match ctx.job_manager.status(&id).await {
            Ok(s) => s,
            Err(JobError::UnknownJob(_)) => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "poll_render: unknown job_id '{}'. Either it was \
                     never started, it completed >5 minutes ago and \
                     was reaped, or the desktop/backend restarted after \
                     the render was queued. If prior chat history includes \
                     an output_path, verify that file before assuming the \
                     render succeeded; otherwise call start_render again.",
                    id
                )));
            }
            Err(other) => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "poll_render: job-status lookup failed ({other}). The job \
                     may have been reaped or the manager is in a bad state. \
                     Re-call start_render to queue a fresh render."
                )));
            }
        };

        // Project a stable JSON shape — most fields are pass-through from
        // JobStatus, but we trim the log_excerpt cap further to fit a
        // poll-friendly token budget.
        let log_excerpt = if status.log_excerpt.len() > 8192 {
            let start = status.log_excerpt.len() - 8192;
            format!(
                "[…earlier log truncated…]\n{}",
                &status.log_excerpt[start..]
            )
        } else {
            status.log_excerpt.clone()
        };

        let body = serde_json::json!({
            "job_id": status.id.to_string(),
            "state": status.state,
            "progress_pct": status.progress_pct,
            "frames_done": status.frames_done,
            "time_done_s": status.time_done_s,
            "speed": status.speed,
            "eta_s": status.eta_s,
            "exit_code": status.exit_code,
            "command_argv": status.command_argv,
            "output_path": status.output_path.display().to_string(),
            "started_at": status.started_at.to_rfc3339(),
            "log_excerpt": log_excerpt,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
Read the latest status of a render job started by `start_render`. \
Returns: state (queued|running|done|failed|cancelled), progress_pct, \
frames_done, time_done_s, speed, eta_s, exit_code (when terminal), \
command_argv, output_path, and a tail-truncated ffmpeg log excerpt. \
Cheap to call repeatedly. Reaped 5 minutes after the job ends.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),

            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "poll_render".into(),
            args,
        }
    }

    #[tokio::test]
    async fn unknown_job_id_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = PollRenderTool
            .handle(
                invoke(serde_json::json!({"job_id": "nope"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("unknown job_id"))
        );
    }

    #[tokio::test]
    async fn missing_args_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = PollRenderTool
            .handle(invoke(serde_json::json!({})), ctx_at(dir.path()))
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(_)));
    }

    /// E2e: start_render → poll_render terminal state. Requires ffmpeg
    /// installed; auto-skips when not.
    #[tokio::test]
    async fn end_to_end_with_real_ffmpeg() {
        if awidat_render::ffmpeg_path().is_err() {
            return;
        }
        use awidat_render::{JobManager, RenderJobSpec};
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let renders = dir.path().join("renders");
        std::fs::create_dir_all(&renders).unwrap();
        let out_path = renders.join("preview-test.mp4");

        let manager = JobManager::new();
        let spec = RenderJobSpec {
            args: vec![
                "-y".into(),
                "-f".into(),
                "lavfi".into(),
                "-i".into(),
                "color=c=blue:s=160x120:d=1".into(),
                "-c:v".into(),
                "libx264".into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
                out_path.to_string_lossy().into_owned(),
            ],
            total_duration_s: Some(1.0),
            cwd: Some(dir.path().to_path_buf()),
            output_path: out_path.clone(),
            limitations: Vec::new(),
        };
        let job_id = manager.start(spec).await.unwrap();

        // Build a custom ctx that shares this manager.
        let (tx, _) = broadcast::channel(8);
        let ctx = ToolContext {
            project_root: dir.path().to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: manager,
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        };

        // Poll up to 15s for terminal state.
        let mut last: serde_json::Value = serde_json::json!({});
        for _ in 0..150 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let out = PollRenderTool
                .handle(
                    invoke(serde_json::json!({"job_id": job_id.to_string()})),
                    ctx.clone(),
                )
                .await
                .unwrap();
            let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
            if matches!(
                v["state"].as_str(),
                Some("done") | Some("failed") | Some("cancelled")
            ) {
                last = v;
                break;
            }
        }
        assert_eq!(last["state"], "done", "expected done; got {last}");
        assert_eq!(last["exit_code"], 0);
        assert_eq!(last["progress_pct"], 100.0);
        let command_argv = last["command_argv"]
            .as_array()
            .expect("poll_render should expose the render command argv");
        assert!(
            command_argv
                .iter()
                .any(|arg| arg.as_str().is_some_and(|arg| arg.contains("ffmpeg"))),
            "command argv should include the ffmpeg binary path: {command_argv:?}"
        );
        assert!(
            command_argv
                .iter()
                .any(|arg| arg.as_str() == Some("libx264")),
            "command argv should include render arguments: {command_argv:?}"
        );
    }
}
