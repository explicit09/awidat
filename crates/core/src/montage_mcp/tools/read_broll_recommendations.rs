//! `read_broll_recommendations` — scored B-roll opportunities.
//! Ported in step 5 from
//! `crates/core/src/tools/read_broll_recommendations.rs`.

use std::path::Path;

use montage_index::media_files::{MediaScanOptions, collect_project_media_files};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::broll_recommendations::build_broll_recommendation_package;
use crate::montage_mcp::context::McpToolCtx;
use crate::understanding::build_understanding_package;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ReadBrollRecommendationsArgs {
    /// Optional project-relative source asset id.
    #[serde(default)]
    pub asset_id: Option<String>,
    /// Optional minimum recommendation score from 0..1.
    #[serde(default)]
    pub min_score: Option<f64>,
}

#[derive(Debug, Serialize)]
struct ReadBrollRecommendationsResponse {
    status: &'static str,
    project_root: String,
    asset_count: usize,
    recommendation_count: usize,
    summary_for_agent: String,
    recommendations: montage_proto::professional::BrollRecommendationPackage,
}

pub fn run(args: ReadBrollRecommendationsArgs, ctx: McpToolCtx) -> Result<String, String> {
    let min_score = args.min_score.unwrap_or(0.0);
    if !(0.0..=1.0).contains(&min_score) {
        return Err("read_broll_recommendations: min_score must be between 0 and 1.".into());
    }

    let asset_ids = asset_ids(&ctx.project_root, args.asset_id.as_deref())?;
    let understanding = build_understanding_package(&ctx.project_root, &asset_ids);
    let mut recommendations = build_broll_recommendation_package(&understanding.assets);
    filter_by_min_score(&mut recommendations, min_score);
    let recommendation_count = recommendations
        .assets
        .iter()
        .map(|asset| asset.recommendations.len())
        .sum();
    let response = ReadBrollRecommendationsResponse {
        status: "ok",
        project_root: ctx.project_root.to_string_lossy().into_owned(),
        asset_count: recommendations.assets.len(),
        recommendation_count,
        summary_for_agent: summarize_for_agent(
            recommendations.assets.len(),
            recommendation_count,
            min_score,
        ),
        recommendations,
    };
    serde_json::to_string(&response)
        .map_err(|e| format!("read_broll_recommendations serialization failed: {e}"))
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
    .map_err(|e| format!("read_broll_recommendations: unable to scan raw media: {e}"))?;
    Ok(files
        .into_iter()
        .map(|file| file.project_relative_path)
        .collect())
}

fn filter_by_min_score(
    package: &mut montage_proto::professional::BrollRecommendationPackage,
    min_score: f64,
) {
    for asset in &mut package.assets {
        asset
            .recommendations
            .retain(|recommendation| recommendation.score >= min_score);
    }
}

fn summarize_for_agent(asset_count: usize, recommendation_count: usize, min_score: f64) -> String {
    if asset_count == 0 {
        return "No raw assets were found. Import media and run indexing before reading B-roll recommendations."
            .into();
    }
    format!(
        "{asset_count} asset(s), {recommendation_count} B-roll recommendation(s) at min_score {min_score:.2}."
    )
}

pub const DESCRIPTION: &str = "\
Read scored B-roll recommendations derived from fused understanding without \
side effects. Returns category, confidence score, asset strategy, insertion \
plan, rationale, score breakdown, and source evidence ids for review.";
