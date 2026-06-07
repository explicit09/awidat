//! `find_black_frames` — run FFmpeg blackdetect over one source asset.
//! Ported from `crates/core/src/tools/find_black_frames.rs` to the
//! in-process MCP server. Returns source-time black ranges as JSON.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

const DEFAULT_PICTURE_BLACK_RATIO_TH: f64 = 0.98;
const DEFAULT_MIN_DURATION_S: f64 = 0.2;

/// Arguments to `find_black_frames`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct FindBlackFramesArgs {
    /// Project-relative source asset path.
    pub asset: String,
    /// FFmpeg blackdetect picture-black ratio threshold. Default 0.98.
    #[serde(default)]
    pub picture_black_ratio_th: Option<f64>,
    /// Minimum black range duration in seconds. Default 0.2.
    #[serde(default)]
    pub min_duration_s: Option<f64>,
}

/// Run `find_black_frames` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; argument
/// validation or ffmpeg failures return `Err(String)`.
pub async fn run(args: FindBlackFramesArgs, ctx: McpToolCtx) -> Result<String, String> {
    validate_asset(&args.asset)?;
    let asset_path = ctx.project_root.join(&args.asset);
    if !asset_path.exists() {
        return Err(format!(
            "find_black_frames: asset '{}' not found at {}",
            args.asset,
            asset_path.display()
        ));
    }

    let picture_black_ratio_th = args
        .picture_black_ratio_th
        .unwrap_or(DEFAULT_PICTURE_BLACK_RATIO_TH);
    let min_duration_s = args.min_duration_s.unwrap_or(DEFAULT_MIN_DURATION_S);
    let ranges = montage_render::generate_black_frames(
        &asset_path,
        picture_black_ratio_th,
        min_duration_s,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .map_err(|e| format!("find_black_frames: ffmpeg blackdetect failed: {e}"))?;

    let body = serde_json::json!({
        "asset": args.asset,
        "picture_black_ratio_th": picture_black_ratio_th,
        "min_duration_s": min_duration_s,
        "ranges": ranges.iter().map(|range| serde_json::json!({
            "start_s": range.start_s,
            "end_s": range.end_s,
            "duration_s": range.duration_s,
        })).collect::<Vec<_>>(),
        "range_count": ranges.len(),
    });
    Ok(body.to_string())
}

fn validate_asset(asset: &str) -> Result<(), String> {
    if asset.trim().is_empty() {
        return Err("find_black_frames: asset must not be empty".into());
    }
    if asset.contains("..") {
        return Err(format!(
            "find_black_frames: asset '{asset}' must not contain '..' segments"
        ));
    }
    Ok(())
}

pub const DESCRIPTION: &str = "\
Detect black-frame ranges in one project source asset using FFmpeg \
blackdetect. Use this as a quality/eval inspection tool after renders or \
before accepting suspicious cuts; it is read-only and returns source-time \
ranges with start_s, end_s, and duration_s.";
