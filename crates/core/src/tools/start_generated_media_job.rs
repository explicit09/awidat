//! Tool for starting generated-media jobs.

use std::fs;

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::generated_media::provider::{
    GeneratedMediaProvider, MockProvider, StartGeneratedMediaRequest,
};
use crate::generated_media::registry::Registry;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Start a generated-media job and write the local registry.
pub struct StartGeneratedMediaJobTool;

#[derive(Debug, Deserialize)]
struct Args {
    provider: String,
    artifact_kind: String,
    workflow_purpose: String,
    prompt: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    aspect_ratio: Option<String>,
    #[serde(default)]
    generate_audio: Option<bool>,
}

#[async_trait]
impl ToolHandler for StartGeneratedMediaJobTool {
    fn name(&self) -> &'static str {
        "start_generated_media_job"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["provider", "artifact_kind", "workflow_purpose", "prompt"],
                "properties": {
                    "provider": { "type": "string", "enum": ["mock", "openrouter", "seedance"] },
                    "artifact_kind": { "type": "string", "enum": ["video"] },
                    "workflow_purpose": { "type": "string", "enum": ["broll"] },
                    "prompt": { "type": "string" },
                    "model": { "type": "string" },
                    "duration": { "type": "integer", "minimum": 1 },
                    "resolution": { "type": "string" },
                    "aspect_ratio": { "type": "string" },
                    "generate_audio": { "type": "boolean" }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        let provider = invocation
            .args
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing-provider>");
        let model = invocation
            .args
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("default-model");
        let artifact_kind = invocation
            .args
            .get("artifact_kind")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing-artifact-kind>");
        let workflow_purpose = invocation
            .args
            .get("workflow_purpose")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing-workflow-purpose>");
        let prompt_hash = invocation
            .args
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(crate::generated_media::registry::prompt_hash)
            .unwrap_or_else(|| "<missing-prompt>".to_string());
        vec![ApprovalKey::new(
            self.name(),
            format!(
                "generated_media:{artifact_kind}:{workflow_purpose}:{provider}:{model}:{prompt_hash}:dest=raw/generated"
            ),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_generated_media_job: invalid args ({e}). Required: provider, artifact_kind, workflow_purpose, prompt."
            ))
        })?;
        if args.prompt.trim().is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "start_generated_media_job: prompt is empty.".into(),
            ));
        }
        if args.artifact_kind != "video" || args.workflow_purpose != "broll" {
            return Err(FunctionCallError::RespondToModel(
                "start_generated_media_job: this foundation currently supports artifact_kind=video and workflow_purpose=broll.".into(),
            ));
        }

        if args.provider == "seedance" {
            return Err(FunctionCallError::RespondToModel(
                "start_generated_media_job: provider 'seedance' is not configured directly. Use provider 'openrouter' for OpenRouter video generation or 'mock' for offline tests.".into(),
            ));
        }
        if args.provider != "mock" && args.provider != "openrouter" {
            return Err(FunctionCallError::RespondToModel(format!(
                "start_generated_media_job: unsupported provider '{}'. Use 'openrouter' or 'mock'.",
                args.provider
            )));
        }

        if args.provider == "openrouter" {
            let job_id = next_job_id(&args.prompt);
            let config = crate::generated_media::openrouter::OpenRouterVideoConfig::from_env(
                args.model.clone(),
            )
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!("start_generated_media_job: {e}"))
            })?;
            let record = crate::generated_media::openrouter::submit_video_job(
                &config,
                &job_id,
                &args.prompt,
                &crate::generated_media::openrouter::OpenRouterVideoOptions {
                    duration: args.duration,
                    resolution: args.resolution,
                    aspect_ratio: args.aspect_ratio,
                    generate_audio: args.generate_audio,
                },
            )
            .await
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!("start_generated_media_job: {e}"))
            })?;
            Registry::load_or_default(&ctx.project_root)
                .and_then(|registry| registry.upsert(&ctx.project_root, record.clone()))
                .map_err(|e| {
                    FunctionCallError::RespondToModel(format!("start_generated_media_job: {e}"))
                })?;

            return Ok(ToolOutput::text(
                serde_json::json!({
                    "job_id": record.job_id,
                    "provider_job_id": record.provider_job_id,
                    "provider_status_url": record.provider_status_url,
                    "state": record.state,
                    "provider": record.provider,
                    "model": record.model,
                    "prompt_hash": record.prompt_hash,
                    "output_paths": record.output_paths,
                    "next_step": "Call poll_generated_media_job to check provider status, then use_generated_media after a local video output is recorded."
                })
                .to_string(),
            ));
        }

        let request = StartGeneratedMediaRequest {
            provider: args.provider,
            artifact_kind: args.artifact_kind,
            workflow_purpose: args.workflow_purpose,
            prompt: args.prompt,
            model: args.model,
        };
        let job_id = next_job_id(&request.prompt);
        let provider = MockProvider;
        let record = provider.start_offline(&request, &job_id).map_err(|e| {
            FunctionCallError::RespondToModel(format!("start_generated_media_job: {e}"))
        })?;
        if let Some(video_path) = record.output_video_path() {
            write_mock_output(&ctx.project_root, video_path).map_err(|e| {
                FunctionCallError::RespondToModel(format!("start_generated_media_job: {e}"))
            })?;
        }
        Registry::load_or_default(&ctx.project_root)
            .and_then(|registry| registry.upsert(&ctx.project_root, record.clone()))
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!("start_generated_media_job: {e}"))
            })?;

        Ok(ToolOutput::text(
            serde_json::json!({
                "job_id": record.job_id,
                "state": record.state,
                "provider": record.provider,
                "model": record.model,
                "prompt_hash": record.prompt_hash,
                "output_paths": record.output_paths,
                "next_step": "Call use_generated_media after confirming the generated asset should be inserted."
            })
            .to_string(),
        ))
    }
}

fn next_job_id(prompt: &str) -> String {
    let digest = crate::generated_media::registry::prompt_hash(prompt);
    let stamp = chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or_else(|| chrono::Utc::now().timestamp_micros());
    format!("gen-{}-{}", &digest[..12], stamp)
}

fn write_mock_output(project_root: &std::path::Path, output_path: &str) -> std::io::Result<()> {
    let path = project_root.join(output_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if !path.exists() {
        fs::write(path, b"montage generated media mock placeholder\n")?;
    }
    Ok(())
}

const DESCRIPTION: &str = "\
Start a generated-media job and write the local generated-media registry. \
Provider 'mock' creates an offline completed placeholder record suitable \
for tests. Provider 'openrouter' submits an asynchronous OpenRouter video \
generation job using OPENROUTER_API_KEY. Provider 'seedance' is not direct; \
use OpenRouter or a future dedicated adapter.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolHandler;

    #[tokio::test]
    async fn mock_provider_writes_succeeded_registry_record() {
        let dir = tempfile::tempdir().unwrap();
        let output = StartGeneratedMediaJobTool
            .handle(
                crate::tool::ToolInvocation {
                    call_id: "c1".into(),
                    name: "start_generated_media_job".into(),
                    args: serde_json::json!({
                        "provider": "mock",
                        "artifact_kind": "video",
                        "workflow_purpose": "broll",
                        "prompt": "wide office establishing shot"
                    }),
                },
                crate::generated_media::test_support::ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(body["state"], "succeeded");
        assert!(
            body["output_paths"]["video"]
                .as_str()
                .unwrap()
                .starts_with("raw/generated/")
        );
        assert!(
            dir.path()
                .join(".montage/generated-media/registry.json")
                .exists()
        );
        assert!(
            dir.path()
                .join(body["output_paths"]["video"].as_str().unwrap())
                .exists()
        );
    }

    #[tokio::test]
    async fn seedance_provider_points_to_openrouter_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let err = StartGeneratedMediaJobTool
            .handle(
                crate::tool::ToolInvocation {
                    call_id: "c1".into(),
                    name: "start_generated_media_job".into(),
                    args: serde_json::json!({
                        "provider": "seedance",
                        "artifact_kind": "video",
                        "workflow_purpose": "broll",
                        "prompt": "realistic people crossing a lobby"
                    }),
                },
                crate::generated_media::test_support::ctx_at(dir.path()),
            )
            .await
            .unwrap_err();

        assert!(format!("{err}").contains("Use provider 'openrouter'"));
        assert!(
            !dir.path()
                .join(".montage/generated-media/registry.json")
                .exists()
        );
    }

    #[test]
    fn approval_key_includes_prompt_hash_and_output_scope() {
        let invocation = crate::tool::ToolInvocation {
            call_id: "c1".into(),
            name: "start_generated_media_job".into(),
            args: serde_json::json!({
                "provider": "mock",
                "artifact_kind": "video",
                "workflow_purpose": "broll",
                "prompt": "quiet street",
                "model": "offline-placeholder"
            }),
        };

        let keys = StartGeneratedMediaJobTool.approval_keys(&invocation);
        assert_eq!(keys.len(), 1);
        assert!(
            keys[0]
                .operation
                .contains("generated_media:video:broll:mock")
        );
        assert!(keys[0].operation.contains("dest=raw/generated"));
        assert!(
            keys[0]
                .operation
                .contains(&crate::generated_media::registry::prompt_hash(
                    "quiet street"
                ))
        );
    }
}
