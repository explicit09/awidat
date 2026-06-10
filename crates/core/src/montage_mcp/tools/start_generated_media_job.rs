//! `start_generated_media_job` — start a generated-media job and
//! write the project-local registry. Ported from
//! `crates/core/src/tools/start_generated_media_job.rs` to the
//! in-process MCP server. The original tool only used `ctx.project_root`
//! plus shared `crate::generated_media` helpers, so the port is a
//! straight rewrite with no job_manager / approvals coupling.

use std::fs;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::generated_media::provider::{
    GeneratedMediaProvider, MockProvider, StartGeneratedMediaRequest,
};
use crate::generated_media::registry::{Registry, write_generated_description_sidecar};
use crate::montage_mcp::context::McpToolCtx;

pub const OPENROUTER_COST_CONFIRMATION: &str =
    "OpenRouter cost unknown; explicit confirmation required";

/// Arguments to `start_generated_media_job`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct StartGeneratedMediaJobArgs {
    pub provider: String,
    pub artifact_kind: String,
    pub workflow_purpose: String,
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Seedance 2.0 generated video duration in seconds. Use 4-15 for generated B-roll.
    #[serde(default)]
    pub duration: Option<u32>,
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    #[serde(default)]
    pub generate_audio: Option<bool>,
    /// Required for provider=openrouter so the desktop approval text visibly
    /// includes the paid-provider warning before the network request starts.
    #[serde(default)]
    pub cost_confirmation: Option<String>,
}

pub async fn run(args: StartGeneratedMediaJobArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.prompt.trim().is_empty() {
        return Err("start_generated_media_job: prompt is empty.".into());
    }
    if args.artifact_kind != "video" || args.workflow_purpose != "broll" {
        return Err(
            "start_generated_media_job: this foundation currently supports artifact_kind=video and workflow_purpose=broll.".into(),
        );
    }
    if let Some(duration) = args.duration
        && !(4..=15).contains(&duration)
    {
        return Err(format!(
            "start_generated_media_job: duration={duration} out of range. Use 4-15 seconds for Seedance 2.0 generated B-roll."
        ));
    }

    if args.provider == "seedance" {
        return Err(
            "start_generated_media_job: provider 'seedance' is not configured directly. Use provider 'openrouter' for OpenRouter video generation or 'mock' for offline tests.".into(),
        );
    }
    if args.provider != "mock" && args.provider != "openrouter" {
        return Err(format!(
            "start_generated_media_job: unsupported provider '{}'. Use 'openrouter' or 'mock'.",
            args.provider
        ));
    }

    if args.provider == "openrouter" {
        validate_openrouter_cost_confirmation(&args)?;
        let job_id = next_job_id(&args.prompt);
        let config =
            crate::generated_media::openrouter::OpenRouterVideoConfig::from_env(args.model.clone())
                .map_err(|e| format!("start_generated_media_job: {e}"))?;
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
        .map_err(|e| format!("start_generated_media_job: {e}"))?;
        Registry::load_or_default(&ctx.project_root)
            .and_then(|registry| registry.upsert(&ctx.project_root, record.clone()))
            .map_err(|e| format!("start_generated_media_job: {e}"))?;

        return Ok(serde_json::json!({
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
        .to_string());
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
    let record = provider
        .start_offline(&request, &job_id)
        .map_err(|e| format!("start_generated_media_job: {e}"))?;
    if let Some(video_path) = record.output_video_path() {
        write_mock_output(&ctx.project_root, video_path)
            .map_err(|e| format!("start_generated_media_job: {e}"))?;
    }
    Registry::load_or_default(&ctx.project_root)
        .and_then(|registry| registry.upsert(&ctx.project_root, record.clone()))
        .map_err(|e| format!("start_generated_media_job: {e}"))?;
    write_generated_description_sidecar(&ctx.project_root, &record)
        .map_err(|e| format!("start_generated_media_job: generated description sidecar: {e}"))?;

    Ok(serde_json::json!({
        "job_id": record.job_id,
        "state": record.state,
        "provider": record.provider,
        "model": record.model,
        "prompt_hash": record.prompt_hash,
        "output_paths": record.output_paths,
        "next_step": "Call use_generated_media after confirming the generated asset should be inserted."
    })
    .to_string())
}

pub fn validate_openrouter_cost_confirmation(
    args: &StartGeneratedMediaJobArgs,
) -> Result<(), String> {
    if args.provider != "openrouter" {
        return Ok(());
    }
    let provided = args.cost_confirmation.as_deref().unwrap_or("").trim();
    if provided == OPENROUTER_COST_CONFIRMATION {
        return Ok(());
    }
    Err(format!(
        "start_generated_media_job: provider=openrouter requires cost_confirmation=\"{OPENROUTER_COST_CONFIRMATION}\" so the desktop approval request visibly includes the cost warning."
    ))
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

pub const DESCRIPTION: &str = "\
Start a generated-media job and write the local generated-media registry. \
Provider 'mock' creates an offline completed placeholder record suitable \
for tests. Provider 'openrouter' submits an asynchronous OpenRouter video \
generation job using the configured OpenRouter key. For provider 'openrouter', \
include cost_confirmation=\"OpenRouter cost unknown; explicit confirmation required\" \
so the desktop approval text shows the paid-provider warning. Provider 'seedance' is not direct; \
use OpenRouter or a future dedicated adapter.";
