//! `inspect_clip` — codec/dims/duration/audio summary for an asset.
//! Ported in step 5 from `crates/core/src/tools/inspect_clip.rs`.
//!
//! Returns a one-page metadata summary projected from sidecars (whisper
//! for duration/language/speakers, scenedetect for fps/duration,
//! audio-energy for loudness/silences).

use montage_index::{SidecarError, read_sidecar};
use montage_proto::index::AssetId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct InspectClipArgs {
    /// Project-relative asset id (e.g. `raw/ep-014.mp4`).
    pub asset_id: String,
}

pub fn run(args: InspectClipArgs, ctx: McpToolCtx) -> Result<String, String> {
    let asset = AssetId::new(args.asset_id);
    let mut overview = serde_json::Map::new();
    overview.insert("asset_id".into(), serde_json::json!(asset.to_string()));

    let mut any = false;

    if let Some(scene) = try_read(&ctx.project_root, "scenedetect", &asset)? {
        let data = scene.get("data").cloned().unwrap_or_default();
        if let Some(v) = data.get("frame_rate") {
            overview.insert("frame_rate".into(), v.clone());
        }
        if let Some(v) = data.get("duration_s") {
            overview.insert("duration_s".into(), v.clone());
        }
        if let Some(shots) = data.get("shots").and_then(|v| v.as_array()) {
            overview.insert("shot_count".into(), serde_json::json!(shots.len()));
        }
        any = true;
    }

    if let Some(audio) = try_read(&ctx.project_root, "audio-energy", &asset)? {
        let data = audio.get("data").cloned().unwrap_or_default();
        // audio-energy may have duration too — prefer it if scenedetect didn't fire.
        if !overview.contains_key("duration_s") {
            if let Some(v) = data.get("duration_s") {
                overview.insert("duration_s".into(), v.clone());
            }
        }
        if let Some(v) = data.get("sample_rate") {
            overview.insert("audio_sample_rate".into(), v.clone());
        }
        if let Some(v) = data.get("loudness_integrated_lufs") {
            overview.insert("loudness_integrated_lufs".into(), v.clone());
        }
        if let Some(silences) = data.get("silences").and_then(|v| v.as_array()) {
            overview.insert("silence_count".into(), serde_json::json!(silences.len()));
        }
        any = true;
    }

    if let Some(whisper) = try_read(&ctx.project_root, "whisper", &asset)? {
        let data = whisper.get("data").cloned().unwrap_or_default();
        if let Some(v) = data.get("language") {
            overview.insert("language".into(), v.clone());
        }
        if let Some(v) = data.get("speakers") {
            overview.insert("speakers".into(), v.clone());
        }
        if let Some(v) = data.get("model") {
            overview.insert("whisper_model".into(), v.clone());
        }
        if let Some(segments) = data.get("segments").and_then(|v| v.as_array()) {
            overview.insert("segment_count".into(), serde_json::json!(segments.len()));
        }
        any = true;
    }

    if !any {
        return Err(format!(
            "inspect_clip: no indexer sidecars found for '{asset}'. Run \
             `montage index` first."
        ));
    }

    serde_json::to_string(&serde_json::Value::Object(overview))
        .map_err(|e| format!("inspect_clip: serialize failed: {e}"))
}

fn try_read(
    root: &std::path::Path,
    indexer: &str,
    asset: &AssetId,
) -> Result<Option<serde_json::Value>, String> {
    match read_sidecar(root, indexer, asset) {
        Ok(v) => Ok(Some(v)),
        Err(SidecarError::NotFound { .. }) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub const DESCRIPTION: &str = "\
Get a one-page metadata summary for an asset: duration, frame_rate, audio \
sample rate, loudness, language, speakers, shot count, segment count. \
Aggregates from whichever indexer sidecars exist (whisper / scenedetect / \
audio-energy). Use this before deciding which sidecar to read in detail \
via `read_index`.";
