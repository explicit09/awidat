//! `view_frame` — extract one frame from an asset and return it as a
//! base64-encoded image payload. Ported from
//! `crates/core/src/tools/view_frame.rs` to the in-process MCP server.
//!
//! The rmcp `#[tool]` wrapper here only returns `Result<String, _>`,
//! so the response is a JSON document carrying the base64 image,
//! media type, byte length, and a textual summary the model can use.
//! Cache layout: `<project>/.montage/cache/frames/<asset-hash>/<t_ms>_<dim>_<grade>.<ext>`.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use montage_proto::otio::{StackChild, Timeline, TrackChild};
use montage_proto::project::files;
use montage_render::ffmpeg::{ImageFormat, extract_frame_complex, extract_frame_filtered};
use montage_render::{ClipGradePreview, build_clip_preview_filtergraph};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::montage_mcp::context::McpToolCtx;

/// Default longest-edge dimension for `detail = "preview"`. Big
/// enough to be useful, small enough to fit comfortably in the
/// model's vision budget.
const PREVIEW_MAX_DIM: u32 = 768;

/// Arguments to `view_frame`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ViewFrameArgs {
    /// Project-relative asset id, OR an absolute path under the project.
    pub asset: String,
    /// Time in seconds to extract.
    pub t_s: f64,
    /// `"preview"` (default; resized to ~768px on the longer edge) or
    /// `"original"` (no resize).
    #[serde(default)]
    pub detail: Option<String>,
    /// Image format: `"png"` (default) or `"jpeg"`.
    #[serde(default)]
    pub format: Option<String>,
    /// Optional clip name from the project's OTIO. When set, the
    /// frame is extracted through the same color filter chain the
    /// full timeline render would apply to that clip
    /// (`montage.color_correction` + LUT effects).
    #[serde(default)]
    pub clip: Option<String>,
}

/// Run `view_frame` against the project resolved from
/// [`McpToolCtx`]. Returns a JSON body that carries the base64 image
/// payload, media type, byte count, and a textual summary.
pub async fn run(args: ViewFrameArgs, ctx: McpToolCtx) -> Result<String, String> {
    if !args.t_s.is_finite() || args.t_s < 0.0 {
        return Err(format!(
            "view_frame: t_s ({}) must be finite and >= 0",
            args.t_s
        ));
    }

    // Detail whitelist.
    let detail = match args.detail.as_deref().unwrap_or("preview") {
        "preview" => Detail::Preview,
        "original" => Detail::Original,
        other => {
            return Err(format!(
                "view_frame: detail '{other}' not recognized. Use 'preview' or 'original'."
            ));
        }
    };

    let format = match args.format.as_deref().unwrap_or("png") {
        "png" => ImageFormat::Png,
        "jpeg" => ImageFormat::Jpeg,
        other => {
            return Err(format!(
                "view_frame: format '{other}' not recognized. Use 'png' or 'jpeg'."
            ));
        }
    };

    // Resolve the asset path. Accept project-relative or absolute,
    // but prevent absolute escape from the project root.
    let asset_path = resolve_asset_path(&ctx.project_root, &args.asset)?;
    if !asset_path.exists() {
        return Err(format!(
            "view_frame: asset '{}' not found at {}",
            args.asset,
            asset_path.display()
        ));
    }

    let max_dim = match detail {
        Detail::Preview => Some(PREVIEW_MAX_DIM),
        Detail::Original => None,
    };

    let grade = if let Some(clip_name) = args.clip.as_deref() {
        Some(resolve_grade_preview(&ctx.project_root, clip_name)?)
    } else {
        None
    };
    let grade_signature = grade
        .as_ref()
        .and_then(|p| p.graph.as_deref())
        .unwrap_or("");

    let cache_path = cache_path_for(
        &ctx.project_root,
        &asset_path,
        args.t_s,
        format,
        max_dim,
        grade_signature,
    )
    .map_err(|e| format!("view_frame: cache path build failed: {e}"))?;

    let bytes = if cache_path.exists() {
        tokio::fs::read(&cache_path).await.map_err(|e| {
            format!(
                "view_frame: cache read failed at {}: {e}",
                cache_path.display()
            )
        })?
    } else {
        let bytes = match grade.as_ref().and_then(|p| p.graph.as_deref()) {
            Some(graph) => {
                extract_frame_complex(&asset_path, args.t_s, format, graph, "[grade_out]", max_dim)
                    .await
            }
            None => extract_frame_filtered(&asset_path, args.t_s, format, max_dim, None).await,
        }
        .map_err(|e| format!("view_frame: ffmpeg failed: {e}"))?;
        if let Some(parent) = cache_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = tokio::fs::write(&cache_path, &bytes).await;
        bytes
    };

    let b64 = B64.encode(&bytes);
    let grade_summary = match grade.as_ref() {
        Some(p) => {
            let mut s = if p.graph.is_some() {
                format!(
                    " (graded via clip {:?})",
                    args.clip.as_deref().unwrap_or("")
                )
            } else {
                format!(
                    " (clip {:?} had no graded effects)",
                    args.clip.as_deref().unwrap_or("")
                )
            };
            if !p.skipped.is_empty() {
                s.push_str(&format!(" — skipped: {}", p.skipped.join("; ")));
            }
            s
        }
        None => String::new(),
    };
    let summary = format!(
        "frame {:.3}s of {} ({}, {} bytes){}",
        args.t_s,
        args.asset,
        format.media_type(),
        bytes.len(),
        grade_summary,
    );

    Ok(serde_json::json!({
        "summary": summary,
        "asset": args.asset,
        "t_s": args.t_s,
        "media_type": format.media_type(),
        "byte_len": bytes.len(),
        "image_base64": b64,
    })
    .to_string())
}

#[derive(Debug, Clone, Copy)]
enum Detail {
    Preview,
    Original,
}

fn resolve_asset_path(project_root: &Path, asset: &str) -> Result<PathBuf, String> {
    let p = Path::new(asset);
    if asset.contains("..") {
        return Err(format!(
            "view_frame: asset '{asset}' must not contain '..' segments"
        ));
    }
    if p.is_absolute() {
        // Must live under the project root.
        let canonical_root = project_root
            .canonicalize()
            .unwrap_or(project_root.to_path_buf());
        let canonical_asset = p.canonicalize().unwrap_or(p.to_path_buf());
        if !canonical_asset.starts_with(&canonical_root) {
            return Err(format!(
                "view_frame: absolute asset path '{asset}' must live under the project root"
            ));
        }
        Ok(canonical_asset)
    } else {
        Ok(project_root.join(p))
    }
}

fn cache_path_for(
    project_root: &Path,
    asset_path: &Path,
    t_s: f64,
    format: ImageFormat,
    max_dim: Option<u32>,
    grade_signature: &str,
) -> std::io::Result<PathBuf> {
    // Hash the asset *path* (cheap; doesn't read file).
    let mut h = Sha256::new();
    h.update(asset_path.to_string_lossy().as_bytes());
    let asset_hash = format!("{:x}", h.finalize());
    let asset_dir: String = asset_hash.chars().take(16).collect();
    let t_ms = (t_s * 1000.0).round() as i64;
    let dim_tag = max_dim
        .map(|d| format!("d{d}"))
        .unwrap_or_else(|| "orig".to_string());
    let ext = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
    };
    let grade_tag = if grade_signature.is_empty() {
        "raw".to_string()
    } else {
        let mut gh = Sha256::new();
        gh.update(grade_signature.as_bytes());
        let hex = format!("{:x}", gh.finalize());
        format!("g{}", hex.chars().take(12).collect::<String>())
    };
    Ok(project_root
        .join(".montage")
        .join("cache")
        .join("frames")
        .join(asset_dir)
        .join(format!("{t_ms}_{dim_tag}_{grade_tag}.{ext}")))
}

/// Look up `clip_name` in the project's OTIO and return the
/// labeled-graph preview for its effects.
fn resolve_grade_preview(project_root: &Path, clip_name: &str) -> Result<ClipGradePreview, String> {
    let otio_path = project_root.join(files::OTIO);
    let raw = std::fs::read_to_string(&otio_path).map_err(|e| {
        format!(
            "view_frame: project OTIO unreadable at {}: {e}",
            otio_path.display()
        )
    })?;
    let tl: Timeline = serde_json::from_str(&raw)
        .map_err(|e| format!("view_frame: project OTIO not valid JSON: {e}"))?;
    for stack_child in &tl.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        for track_child in &track.children {
            let TrackChild::Clip(clip) = track_child else {
                continue;
            };
            if clip.name == clip_name {
                return Ok(build_clip_preview_filtergraph(clip, project_root));
            }
        }
    }
    Err(format!(
        "view_frame: clip {clip_name:?} not found in project OTIO at {}",
        otio_path.display()
    ))
}

pub const DESCRIPTION: &str = "\
Extract a single frame from a video asset at time `t_s` and return it \
as a JSON payload carrying base64-encoded image bytes plus a textual \
summary. Use this to *see* a moment — for example, to confirm a cut \
lands on the right shot, or to read text on screen. detail='preview' \
(default, <=768px longest edge) keeps the image cheap; \
detail='original' returns source resolution. format='png' (default) | \
'jpeg'. Frames are cached under .montage/cache/frames/ keyed by \
(asset, time, format, dim, grade).\
";
