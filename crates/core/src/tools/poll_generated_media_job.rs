//! Tool for reading generated-media job state from the registry.

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::generated_media::registry::{GeneratedMediaState, Registry};
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Read a generated-media job from the local registry.
pub struct PollGeneratedMediaJobTool;

#[derive(Debug, Deserialize)]
struct Args {
    job_id: String,
}

#[async_trait]
impl ToolHandler for PollGeneratedMediaJobTool {
    fn name(&self) -> &'static str {
        "poll_generated_media_job"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["job_id"],
                "properties": { "job_id": { "type": "string" } }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "poll_generated_media_job: invalid args ({e}). Required: job_id."
            ))
        })?;
        let registry = Registry::load_or_default(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("poll_generated_media_job: {e}"))
        })?;
        let mut record = registry.get(&args.job_id).cloned().ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "poll_generated_media_job: job '{}' not found.",
                args.job_id
            ))
        })?;
        if record.provider == "openrouter"
            && !matches!(
                record.state,
                GeneratedMediaState::Succeeded
                    | GeneratedMediaState::Failed
                    | GeneratedMediaState::Cancelled
            )
            && let Some(status_url) = record.provider_status_url.as_deref()
        {
            let config = crate::generated_media::openrouter::OpenRouterVideoConfig::from_env(Some(
                record.model.clone(),
            ))
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!("poll_generated_media_job: {e}"))
            })?;
            let status = crate::generated_media::openrouter::poll_video_job(&config, status_url)
                .await
                .map_err(|e| {
                    FunctionCallError::RespondToModel(format!("poll_generated_media_job: {e}"))
                })?;
            let output_path =
                crate::generated_media::openrouter::local_video_output_path(&record.job_id);
            let should_download = matches!(status.status.as_str(), "completed" | "succeeded");
            if should_download {
                let absolute_output = ctx.project_root.join(&output_path);
                crate::generated_media::openrouter::download_completed_video(
                    &config,
                    &status,
                    &absolute_output,
                )
                .await
                .map_err(|e| {
                    FunctionCallError::RespondToModel(format!("poll_generated_media_job: {e}"))
                })?;
            }
            crate::generated_media::openrouter::apply_status_to_record(
                &mut record,
                &status,
                should_download.then_some(output_path),
            )
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!("poll_generated_media_job: {e}"))
            })?;
            Registry::load_or_default(&ctx.project_root)
                .and_then(|registry| registry.upsert(&ctx.project_root, record.clone()))
                .map_err(|e| {
                    FunctionCallError::RespondToModel(format!("poll_generated_media_job: {e}"))
                })?;
        }
        Ok(ToolOutput::text(
            serde_json::json!({
                "job_id": record.job_id,
                "state": record.state,
                "provider": record.provider,
                "model": record.model,
                "output_paths": record.output_paths,
                "cost_estimate_usd": record.cost_estimate_usd,
                "cost_actual_usd": record.cost_actual_usd,
                "uses_likeness": record.uses_likeness,
                "requires_disclosure": record.requires_disclosure,
                "failure_message": record.failure_message,
            })
            .to_string(),
        ))
    }
}

const DESCRIPTION: &str = "\
Read a generated-media registry record and return job state plus output \
metadata. OpenRouter records are polled against the provider and completed \
outputs are downloaded under raw/generated/openrouter.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolHandler;

    #[tokio::test]
    async fn poll_returns_registry_state_and_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let record = crate::generated_media::registry::GeneratedMediaRecord::mock_succeeded(
            "generated-media-test",
            "raw/generated/mock/generated-media-test.mp4",
            "office",
        );
        crate::generated_media::registry::Registry::load_or_default(dir.path())
            .unwrap()
            .upsert(dir.path(), record)
            .unwrap();

        let output = PollGeneratedMediaJobTool
            .handle(
                crate::tool::ToolInvocation {
                    call_id: "c1".into(),
                    name: "poll_generated_media_job".into(),
                    args: serde_json::json!({ "job_id": "generated-media-test" }),
                },
                crate::generated_media::test_support::ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(body["state"], "succeeded");
        assert_eq!(
            body["output_paths"]["video"],
            "raw/generated/mock/generated-media-test.mp4"
        );
    }
}
