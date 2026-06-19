//! `podcast_flow_shape` — build a blocking semantic episode-flow
//! review packet from the transcript sidecar.

use std::path::{Path, PathBuf};
use std::process::Command;

use montage_index::{sidecar_path, walk_indexer};
use montage_proto::index::AssetId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `podcast_flow_shape`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PodcastFlowShapeArgs {
    /// Optional project-relative asset path. Omit to review every
    /// whisper transcript.
    #[serde(default)]
    pub asset_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct FlowShapeReport {
    status: &'static str,
    summary_for_agent: String,
    assets: Vec<serde_json::Value>,
    missing_evidence: Vec<String>,
}

/// Run `podcast_flow_shape` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`.
pub fn run(args: PodcastFlowShapeArgs, ctx: McpToolCtx) -> Result<String, String> {
    let script = flow_shape_script_path()?;
    let assets = resolve_assets(&ctx.project_root, args.asset_id)?;
    if assets.is_empty() {
        let report = FlowShapeReport {
            status: "missing_indexes",
            summary_for_agent: "No whisper transcript sidecars found; cannot review episode flow."
                .into(),
            assets: Vec::new(),
            missing_evidence: vec!["whisper transcript sidecar".into()],
        };
        return serialize_report(&report);
    }

    let mut reports = Vec::new();
    for asset_id in assets {
        let transcript = sidecar_path(&ctx.project_root, "whisper", &AssetId::new(&asset_id))
            .map_err(|e| e.to_string())?;
        reports.push(run_planner(&script, &asset_id, &transcript)?);
    }

    let review_count = reports
        .iter()
        .filter(|report| {
            report
                .get("blocks_timeline_edits")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
        })
        .count();
    let missing_transcript_count = reports
        .iter()
        .filter(|report| {
            report.get("status").and_then(|value| value.as_str()) == Some("missing_transcript")
        })
        .count();
    if missing_transcript_count > 0 {
        let report = FlowShapeReport {
            status: "missing_transcript",
            summary_for_agent: format!(
                "{missing_transcript_count} asset(s) have whisper sidecars without transcript segments; rerun or repair transcript indexing before semantic review."
            ),
            assets: reports,
            missing_evidence: vec!["whisper transcript segments".into()],
        };
        return serialize_report(&report);
    }
    let report = FlowShapeReport {
        status: "needs_semantic_review",
        summary_for_agent: format!(
            "Episode flow shape needs semantic review for {review_count} asset(s). Resolve this before extraction, cleanup, or render."
        ),
        assets: reports,
        missing_evidence: Vec::new(),
    };
    serialize_report(&report)
}

fn resolve_assets(project_root: &Path, asset_id: Option<String>) -> Result<Vec<String>, String> {
    if let Some(asset_id) = asset_id {
        let path = sidecar_path(project_root, "whisper", &AssetId::new(&asset_id))
            .map_err(|e| e.to_string())?;
        if !path.exists() {
            return Err(format!(
                "podcast_flow_shape: no whisper sidecar at {}; run the whisper indexer first",
                path.display()
            ));
        }
        return Ok(vec![asset_id]);
    }
    let walker = walk_indexer(project_root, "whisper").map_err(|e| e.to_string())?;
    Ok(walker.map(|(asset, _)| asset).collect())
}

fn run_planner(
    script: &Path,
    asset_id: &str,
    transcript: &Path,
) -> Result<serde_json::Value, String> {
    let output = Command::new("python3")
        .arg(script)
        .arg("--transcript")
        .arg(transcript)
        .arg("--asset-id")
        .arg(asset_id)
        .output()
        .map_err(|e| {
            format!(
                "podcast_flow_shape: failed to run {}: {e}",
                script.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "podcast_flow_shape: planner failed for {asset_id}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| {
        format!("podcast_flow_shape: planner returned malformed JSON for {asset_id}: {e}")
    })
}

fn flow_shape_script_path() -> Result<PathBuf, String> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "cannot resolve repo root".to_string())?;
    let path = repo_root
        .join("skills")
        .join("auto-cutter")
        .join("scripts")
        .join("episode_flow_shape.py");
    if path.exists() {
        Ok(path)
    } else {
        Err(format!(
            "podcast_flow_shape: missing planner script at {}",
            path.display()
        ))
    }
}

fn serialize_report(report: &FlowShapeReport) -> Result<String, String> {
    serde_json::to_string(report).map_err(|e| format!("podcast_flow_shape serialize: {e}"))
}
