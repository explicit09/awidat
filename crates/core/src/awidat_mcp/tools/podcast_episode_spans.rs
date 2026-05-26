//! `podcast_episode_spans` — wrap the bundled episode span planner.
//! Ported from `crates/core/src/tools/podcast_episode_spans.rs` to
//! the in-process MCP server.

use std::path::{Path, PathBuf};
use std::process::Command;

use awidat_index::{sidecar_path, walk_indexer};
use awidat_proto::index::AssetId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `podcast_episode_spans`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PodcastEpisodeSpansArgs {
    /// Optional project-relative asset path. Omit to plan spans for
    /// every whisper transcript.
    #[serde(default)]
    pub asset_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct EpisodeSpanReport {
    status: &'static str,
    summary_for_agent: String,
    assets: Vec<AssetSpanReport>,
    missing_evidence: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AssetSpanReport {
    asset_id: String,
    status: String,
    episode_spans: serde_json::Value,
    rejected_spans: serde_json::Value,
    recommended_span: serde_json::Value,
    requires_user_choice: bool,
    evidence: serde_json::Value,
}

/// Run `podcast_episode_spans` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`;
/// project-read errors / planner failures return `Err(String)`.
pub fn run(args: PodcastEpisodeSpansArgs, ctx: McpToolCtx) -> Result<String, String> {
    let script = episode_span_script_path()?;
    let assets = resolve_assets(&ctx.project_root, args.asset_id)?;
    if assets.is_empty() {
        let report = EpisodeSpanReport {
            status: "missing_indexes",
            summary_for_agent:
                "No whisper transcript sidecars found; cannot plan episode spans.".into(),
            assets: Vec::new(),
            missing_evidence: vec!["whisper transcript sidecar".into()],
        };
        return serialize_report(&report);
    }

    let mut reports = Vec::new();
    let mut missing_evidence = Vec::new();
    for asset_id in assets {
        let transcript = sidecar_path(&ctx.project_root, "whisper", &AssetId::new(&asset_id))
            .map_err(|e| e.to_string())?;
        let audio_energy = optional_sidecar_path(&ctx.project_root, "audio-energy", &asset_id);
        let topic = optional_sidecar_path(&ctx.project_root, "topic", &asset_id);
        if audio_energy.is_none() {
            missing_evidence.push(format!("{asset_id}: missing audio-energy sidecar"));
        }
        if topic.is_none() {
            missing_evidence.push(format!("{asset_id}: missing topic sidecar"));
        }
        reports.push(run_planner(
            &script,
            &asset_id,
            &transcript,
            audio_energy.as_deref(),
            topic.as_deref(),
        )?);
    }

    let requires_choice = reports
        .iter()
        .filter(|report| report.requires_user_choice)
        .count();
    let span_count: usize = reports
        .iter()
        .filter_map(|report| report.episode_spans.as_array().map(Vec::len))
        .sum();
    let status = if reports.is_empty() {
        "missing_indexes"
    } else if missing_evidence.is_empty() {
        "ready"
    } else {
        "partial"
    };
    let summary_for_agent = format!(
        "Episode span status: {status}. Found {span_count} candidate span(s) across {} asset(s); {requires_choice} asset(s) require user choice.",
        reports.len()
    );
    let report = EpisodeSpanReport {
        status,
        summary_for_agent,
        assets: reports,
        missing_evidence,
    };
    serialize_report(&report)
}

fn resolve_assets(project_root: &Path, asset_id: Option<String>) -> Result<Vec<String>, String> {
    if let Some(asset_id) = asset_id {
        let path = sidecar_path(project_root, "whisper", &AssetId::new(&asset_id))
            .map_err(|e| e.to_string())?;
        if !path.exists() {
            return Err(format!(
                "podcast_episode_spans: no whisper sidecar at {}; run the whisper indexer first",
                path.display()
            ));
        }
        return Ok(vec![asset_id]);
    }
    let walker = walk_indexer(project_root, "whisper").map_err(|e| e.to_string())?;
    Ok(walker.map(|(asset, _)| asset).collect())
}

fn optional_sidecar_path(project_root: &Path, indexer: &str, asset_id: &str) -> Option<PathBuf> {
    let path = sidecar_path(project_root, indexer, &AssetId::new(asset_id)).ok()?;
    path.exists().then_some(path)
}

fn run_planner(
    script: &Path,
    asset_id: &str,
    transcript: &Path,
    audio_energy: Option<&Path>,
    topic: Option<&Path>,
) -> Result<AssetSpanReport, String> {
    let mut command = Command::new("python3");
    command.arg(script).arg("--transcript").arg(transcript);
    if let Some(path) = audio_energy {
        command.arg("--audio-energy").arg(path);
    }
    if let Some(path) = topic {
        command.arg("--topic").arg(path);
    }

    let output = command.output().map_err(|e| {
        format!(
            "podcast_episode_spans: failed to run {}: {e}",
            script.display()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "podcast_episode_spans: planner failed for {asset_id}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|e| {
        format!("podcast_episode_spans: planner returned malformed JSON for {asset_id}: {e}")
    })?;
    Ok(AssetSpanReport {
        asset_id: asset_id.to_string(),
        status: parsed
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("planned")
            .to_string(),
        episode_spans: parsed
            .get("episode_spans")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        rejected_spans: parsed
            .get("rejected_spans")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        recommended_span: parsed
            .get("recommended_span")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        requires_user_choice: parsed
            .get("requires_user_choice")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        evidence: parsed
            .get("evidence")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    })
}

fn episode_span_script_path() -> Result<PathBuf, String> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "cannot resolve repo root".to_string())?;
    let path = repo_root
        .join("skills")
        .join("auto-cutter")
        .join("scripts")
        .join("episode_span_plan.py");
    if path.exists() {
        Ok(path)
    } else {
        Err(format!(
            "podcast_episode_spans: missing planner script at {}",
            path.display()
        ))
    }
}

fn serialize_report(report: &EpisodeSpanReport) -> Result<String, String> {
    serde_json::to_string(report).map_err(|e| format!("podcast_episode_spans serialize: {e}"))
}

pub const DESCRIPTION: &str = "\
Plan candidate episode spans from existing transcript/audio/topic evidence. \
Wraps the bundled auto-cutter episode_span_plan.py script; returns \
recommended start/end spans and whether multiple high-confidence episodes \
require the user to choose before extraction or cleanup.\
";
