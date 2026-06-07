//! `plan_scene_aware_short_form` — read-only planner for scene-aware
//! vertical short-form edits. Ported from
//! `crates/core/src/tools/plan_scene_aware_short_form.rs` to the
//! in-process MCP server.

use montage_index::{SidecarError, read_sidecar};
use montage_proto::index::AssetId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::scene_aware_short_form::{SceneAwareShortFormInput, build_scene_aware_short_form_plan};
use crate::short_form_intelligence::{apply_to_scene_aware_input, build_short_form_intelligence};

/// Arguments to `plan_scene_aware_short_form`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanSceneAwareShortFormArgs {
    /// Project-relative source asset id, e.g. raw/interview.mov.
    pub asset_id: String,
    /// Timeline clip uuid/name to use in EDL anchors.
    pub clip_id: String,
    /// Source media width in pixels.
    pub source_width: u32,
    /// Source media height in pixels.
    pub source_height: u32,
}

/// Run `plan_scene_aware_short_form`.
pub fn run(args: PlanSceneAwareShortFormArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.asset_id.trim().is_empty() {
        return Err("plan_scene_aware_short_form: asset_id must be non-empty.".into());
    }
    if args.clip_id.trim().is_empty() {
        return Err("plan_scene_aware_short_form: clip_id must be non-empty.".into());
    }

    let asset = AssetId::new(args.asset_id.clone());
    let mut input = SceneAwareShortFormInput {
        asset_id: args.asset_id,
        clip_id: args.clip_id,
        source_width: args.source_width,
        source_height: args.source_height,
        transcript: sidecar_data(&ctx, "whisper", &asset)?,
        topics: sidecar_data(&ctx, "topic", &asset)?,
        editorial_moments: sidecar_data(&ctx, "editorial-moments", &asset)?,
        audio_energy: sidecar_data(&ctx, "audio-energy", &asset)?,
        face: sidecar_data(&ctx, "face", &asset)?,
        gaze: sidecar_data(&ctx, "gaze", &asset)?,
        scenes: sidecar_data(&ctx, "scenedetect", &asset)?,
        shot: sidecar_data(&ctx, "shot", &asset)?,
        frame_quality: sidecar_data(&ctx, "frame-quality", &asset)?,
        composition: sidecar_data(&ctx, "composition", &asset)?,
        clip: sidecar_data(&ctx, "clip", &asset)?,
    };
    let intelligence = build_short_form_intelligence(&ctx.project_root, &input.asset_id);
    apply_to_scene_aware_input(&mut input, &intelligence);
    let plan = build_scene_aware_short_form_plan(input);
    serde_json::to_string_pretty(&plan).map_err(|e| format!("plan serialization failed: {e}"))
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
            "plan_scene_aware_short_form: failed to read {indexer} sidecar: {err}"
        )),
    }
}

pub const DESCRIPTION: &str = "\
Build a read-only scene-aware short-form edit plan for one candidate clip. \
The tool uses existing Montage evidence sidecars when available: transcript, \
word timings, topics, editorial moments, audio energy, face/gaze, scene and \
shot detection, frame quality, composition, and CLIP metadata. It analyzes \
shot layout, caption safety, negative space, motion intensity, weak visuals, \
and semantic support-visual opportunities, then returns structured \
recommendations with transcript, visual, pacing, safety, and confidence \
reasons plus an EDL fragment. The returned EDL is reviewable and should be \
applied separately with apply_edl only after inspection.\
";
