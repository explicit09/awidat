//! `read_understanding` — fused understanding + clip candidates.
//! Ported in step 5 from `crates/core/src/tools/read_understanding.rs`.

use std::path::Path;

use montage_index::media_files::{MediaScanOptions, collect_project_media_files};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::clip_candidates::build_clip_candidate_package;
use crate::montage_mcp::context::McpToolCtx;
use crate::understanding::build_understanding_package;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ReadUnderstandingArgs {
    /// Optional project-relative source asset id, e.g. raw/interview.mov.
    #[serde(default)]
    pub asset_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReadUnderstandingResponse {
    status: &'static str,
    project_root: String,
    asset_count: usize,
    moment_count: usize,
    clip_candidate_count: usize,
    summary_for_agent: String,
    understanding: montage_proto::professional::UnderstandingPackage,
    clip_candidates: montage_proto::professional::ClipCandidatePackage,
}

pub fn run(args: ReadUnderstandingArgs, ctx: McpToolCtx) -> Result<String, String> {
    let asset_ids = asset_ids(&ctx.project_root, args.asset_id.as_deref())?;
    let understanding = build_understanding_package(&ctx.project_root, &asset_ids);
    let clip_candidates = build_clip_candidate_package(&understanding.assets);
    let moment_count = understanding
        .assets
        .iter()
        .map(|asset| asset.moments.len())
        .sum();
    let clip_candidate_count = clip_candidates
        .assets
        .iter()
        .map(|asset| asset.candidates.len())
        .sum();
    let response = ReadUnderstandingResponse {
        status: "ok",
        project_root: ctx.project_root.to_string_lossy().into_owned(),
        asset_count: understanding.assets.len(),
        moment_count,
        clip_candidate_count,
        summary_for_agent: summarize_for_agent(
            understanding.assets.len(),
            moment_count,
            clip_candidate_count,
        ),
        understanding,
        clip_candidates,
    };
    serde_json::to_string(&response)
        .map_err(|e| format!("read_understanding serialization failed: {e}"))
}

fn asset_ids(project_root: &Path, only_asset: Option<&str>) -> Result<Vec<String>, String> {
    if let Some(asset_id) = only_asset {
        return Ok(vec![asset_id.to_string()]);
    }
    let files = collect_project_media_files(
        project_root,
        MediaScanOptions {
            include_raw: true,
            include_renders: false,
            max_files: None,
        },
    )
    .map_err(|e| format!("read_understanding: unable to scan raw media: {e}"))?;
    Ok(files
        .into_iter()
        .map(|file| file.project_relative_path)
        .collect())
}

fn summarize_for_agent(asset_count: usize, moment_count: usize, candidate_count: usize) -> String {
    if asset_count == 0 {
        return "No raw assets were found. Import media and run indexing before reading understanding."
            .into();
    }
    format!(
        "{asset_count} asset(s), {moment_count} fused moment(s), {candidate_count} reviewable clip candidate(s)."
    )
}

pub const DESCRIPTION: &str = "\
Read consolidated scene/moment understanding and derived short-form clip \
candidates without side effects. Fuses existing transcript, scene, topic, \
audio-energy, and editorial-moment sidecars, then returns reviewable clip \
candidates with scores, explanations, evidence ids, and one-click assembly \
metadata.";
