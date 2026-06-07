//! `plan_short_form_review` — read-only long-form to short-form review
//! planner. Ranks complete moments and returns draft EDL packages
//! without applying them. Ported from
//! `crates/core/src/tools/plan_short_form_review.rs` to the in-process
//! MCP server.

use montage_index::{SidecarError, read_sidecar};
use montage_proto::index::AssetId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::short_form_intelligence::{
    apply_to_short_form_review_input, build_short_form_intelligence,
};
use crate::short_form_review::{
    ShortFormProfile, ShortFormReviewInput, ShortFormReviewOptions, build_short_form_review,
};

/// Arguments to `plan_short_form_review`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanShortFormReviewArgs {
    /// Project-relative source asset id, e.g. raw/interview.mov.
    pub asset_id: String,
    /// Optional source media width in pixels. Defaults to 1920 if unknown.
    #[serde(default)]
    pub source_width: Option<u32>,
    /// Optional source media height in pixels. Defaults to 1080 if unknown.
    #[serde(default)]
    pub source_height: Option<u32>,
    /// Maximum ranked review candidates to return. Default 15, hard cap 50.
    #[serde(default)]
    pub max_candidates: Option<usize>,
    /// Maximum candidate duration. Default 300 seconds.
    #[serde(default)]
    pub max_duration_s: Option<f64>,
    /// Editing profile. `editorial_review` is cleaner and complete;
    /// `viral_social` optimizes TikTok/Reels retention with aggressive
    /// pacing.
    #[serde(default)]
    pub profile: Option<ShortFormProfileArg>,
}

/// Local mirror of `crate::short_form_review::ShortFormProfile` that
/// derives `JsonSchema`. The shared crate type stays free of schema deps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShortFormProfileArg {
    EditorialReview,
    ViralSocial,
}

impl From<ShortFormProfileArg> for ShortFormProfile {
    fn from(value: ShortFormProfileArg) -> Self {
        match value {
            ShortFormProfileArg::EditorialReview => ShortFormProfile::EditorialReview,
            ShortFormProfileArg::ViralSocial => ShortFormProfile::ViralSocial,
        }
    }
}

/// Run `plan_short_form_review`.
pub fn run(args: PlanShortFormReviewArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.asset_id.trim().is_empty() {
        return Err("plan_short_form_review: asset_id must be non-empty.".into());
    }

    let asset = AssetId::new(args.asset_id.clone());
    let mut input = ShortFormReviewInput {
        asset_id: args.asset_id,
        source_width: args.source_width.unwrap_or(1920),
        source_height: args.source_height.unwrap_or(1080),
        transcript: sidecar_data(&ctx, "whisper", &asset)?,
        editorial_moments: sidecar_data(&ctx, "editorial-moments", &asset)?,
        audio_energy: sidecar_data(&ctx, "audio-energy", &asset)?,
        topics: sidecar_data(&ctx, "topic", &asset)?,
        scenes: sidecar_data(&ctx, "scenedetect", &asset)?,
        shot: sidecar_data(&ctx, "shot", &asset)?,
        face: sidecar_data(&ctx, "face", &asset)?,
        gaze: sidecar_data(&ctx, "gaze", &asset)?,
        frame_quality: sidecar_data(&ctx, "frame-quality", &asset)?,
        composition: sidecar_data(&ctx, "composition", &asset)?,
        broll_assets: sidecar_data(&ctx, "broll-candidates", &asset)?,
    };
    let intelligence = build_short_form_intelligence(&ctx.project_root, &input.asset_id);
    apply_to_short_form_review_input(&mut input, &intelligence);
    let review = build_short_form_review(
        input,
        ShortFormReviewOptions {
            max_candidates: args.max_candidates.unwrap_or(15).min(50),
            max_duration_s: args.max_duration_s.unwrap_or(300.0).min(300.0),
            profile: args
                .profile
                .map(ShortFormProfile::from)
                .unwrap_or(ShortFormProfile::EditorialReview),
        },
    );
    serde_json::to_string_pretty(&review)
        .map_err(|e| format!("plan_short_form_review serialization failed: {e}"))
}

fn sidecar_data(
    ctx: &McpToolCtx,
    indexer: &str,
    asset: &AssetId,
) -> Result<serde_json::Value, String> {
    match read_sidecar(&ctx.project_root, indexer, asset) {
        Ok(sidecar) => Ok(sidecar
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null)),
        Err(SidecarError::NotFound { .. }) => Ok(serde_json::Value::Null),
        Err(err) => Err(format!(
            "plan_short_form_review: failed to read {indexer} sidecar: {err}"
        )),
    }
}

pub const DESCRIPTION: &str = "\
Build a read-only long-form to short-form review plan for one asset. \
The tool ranks complete standalone candidate moments, allows extended \
short-form up to five minutes when the idea earns it, recommends B-roll \
by default when support visuals clarify the idea, plans speaker-aware \
9:16 layouts for wide long-form sources, and returns reviewable draft EDL \
packages plus title, caption, platform, confidence, and human review \
actions. It does not apply edits; use apply_edl separately after review so \
autopilot/co-pilot/manual approval behavior remains in control.\
";
