//! `view_frame` tool — extract one frame from an asset and return it
//! as an inline image to the model.
//!
//! Lifted from
//! `harnesses/codex/codex-rs/core/src/tools/handlers/view_image.rs` —
//! their detail-arg whitelist, ResizeToFit/Original modes, base64 data
//! URL pattern. We swap their file-read for ffmpeg single-frame
//! extraction at time `t_s`.
//!
//! Cache layout: `<project>/.montage/cache/frames/<asset-hash>/<t_ms>.<ext>`.
//! Cache hits skip ffmpeg entirely.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use montage_proto::otio::{StackChild, Timeline, TrackChild};
use montage_proto::project::files;
use montage_render::ffmpeg::{ImageFormat, extract_frame_complex, extract_frame_filtered};
use montage_render::{ClipGradePreview, build_clip_preview_filtergraph};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Default longest-edge dimension for `detail = "preview"`. Big enough to
/// be useful, small enough to fit comfortably in the model's vision
/// budget.
const PREVIEW_MAX_DIM: u32 = 768;

/// The `view_frame` tool.
pub struct ViewFrameTool;

#[derive(Debug, Deserialize)]
struct ViewFrameArgs {
    /// Project-relative asset id, OR an absolute path under the project.
    asset: String,
    /// Time in seconds to extract.
    t_s: f64,
    /// `"preview"` (default; resized to ~768px on the longer edge) or
    /// `"original"` (no resize). Mirrors codex view_image.rs detail
    /// whitelist semantics.
    #[serde(default)]
    detail: Option<String>,
    /// Image format: `"png"` (default) or `"jpeg"`.
    #[serde(default)]
    format: Option<String>,
    /// Optional clip name from the project's OTIO. When set, the
    /// frame is extracted through the same color filter chain the
    /// full timeline render would apply to that clip
    /// (`montage.color_correction` + LUT effects). Useful for graded
    /// before/after previews without running the full render.
    /// Multi-stage / partial-strength color pipelines fall back to
    /// a single-pass approximation and the response lists what was
    /// skipped.
    #[serde(default)]
    clip: Option<String>,
}

#[async_trait]
impl ToolHandler for ViewFrameTool {
    fn name(&self) -> &'static str {
        "view_frame"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "view_frame".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset": {
                        "type": "string",
                        "description": "Project-relative asset path, e.g. \"raw/ep-014.mp4\"."
                    },
                    "t_s": {
                        "type": "number",
                        "minimum": 0.0,
                        "description": "Time in seconds to extract (input-side seek; keyframe-aligned)."
                    },
                    "detail": {
                        "type": "string",
                        "enum": ["preview", "original"],
                        "description": "preview = resized to ≤768px (default); original = source resolution."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["png", "jpeg"],
                        "description": "png (default) | jpeg."
                    },
                    "clip": {
                        "type": "string",
                        "description": "Optional clip name; when set, the frame is rendered through that clip's color grade (color_correction + LUT effects) using the same flat filter chain the timeline render would apply."
                    }
                },
                "required": ["asset", "t_s"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        // Writes a cached frame to disk under .montage/cache/. Conceptually
        // read-only against the project, but we touch disk; default-true
        // keeps it sequential with apply_edl.
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: ViewFrameArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "view_frame: invalid args ({e}). Required: {{ \"asset\": <str>, \"t_s\": <number> }}."
            ))
        })?;

        if !args.t_s.is_finite() || args.t_s < 0.0 {
            return Err(FunctionCallError::RespondToModel(format!(
                "view_frame: t_s ({}) must be finite and ≥ 0",
                args.t_s
            )));
        }

        // Detail whitelist (Codex view_image.rs:80-88).
        let detail = match args.detail.as_deref().unwrap_or("preview") {
            "preview" => Detail::Preview,
            "original" => Detail::Original,
            other => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "view_frame: detail '{other}' not recognized. Use 'preview' or 'original'."
                )));
            }
        };

        let format = match args.format.as_deref().unwrap_or("png") {
            "png" => ImageFormat::Png,
            "jpeg" => ImageFormat::Jpeg,
            other => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "view_frame: format '{other}' not recognized. Use 'png' or 'jpeg'."
                )));
            }
        };

        // Resolve the asset path. Accept project-relative or absolute,
        // but prevent absolute escape from the project root.
        let asset_path = resolve_asset_path(&ctx.project_root, &args.asset)?;
        if !asset_path.exists() {
            return Err(FunctionCallError::RespondToModel(format!(
                "view_frame: asset '{}' not found at {}",
                args.asset,
                asset_path.display()
            )));
        }

        let max_dim = match detail {
            Detail::Preview => Some(PREVIEW_MAX_DIM),
            Detail::Original => None,
        };

        // Build the optional grade preview. When the agent passes
        // `clip`, we route through the labeled `-filter_complex`
        // graph that mirrors what the timeline render would emit —
        // strength<1 LUTs and multi-stage `color_pipeline` chains
        // included.
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
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!("view_frame: cache path build failed: {e}"))
        })?;

        let bytes = if cache_path.exists() {
            tokio::fs::read(&cache_path).await.map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "view_frame: cache read failed at {}: {e}",
                    cache_path.display()
                ))
            })?
        } else {
            let bytes = match grade.as_ref().and_then(|p| p.graph.as_deref()) {
                Some(graph) => {
                    extract_frame_complex(
                        &asset_path,
                        args.t_s,
                        format,
                        graph,
                        "[grade_out]",
                        max_dim,
                    )
                    .await
                }
                None => extract_frame_filtered(&asset_path, args.t_s, format, max_dim, None).await,
            }
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!("view_frame: ffmpeg failed: {e}"))
            })?;
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
        Ok(ToolOutput::text(summary).with_images(vec![(format.media_type().to_string(), b64)]))
    }
}

#[derive(Debug, Clone, Copy)]
enum Detail {
    Preview,
    Original,
}

fn resolve_asset_path(project_root: &Path, asset: &str) -> Result<PathBuf, FunctionCallError> {
    let p = Path::new(asset);
    if asset.contains("..") {
        return Err(FunctionCallError::RespondToModel(format!(
            "view_frame: asset '{asset}' must not contain '..' segments"
        )));
    }
    if p.is_absolute() {
        // Must live under the project root.
        let canonical_root = project_root
            .canonicalize()
            .unwrap_or(project_root.to_path_buf());
        let canonical_asset = p.canonicalize().unwrap_or(p.to_path_buf());
        if !canonical_asset.starts_with(&canonical_root) {
            return Err(FunctionCallError::RespondToModel(format!(
                "view_frame: absolute asset path '{asset}' must live under the project root"
            )));
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
    // Hash the asset *path* (cheap; doesn't read file). For invalidation
    // on asset change, callers can prune .montage/cache/frames/.
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
    // Fold the grade chain into the cache key so a graded frame
    // doesn't collide with the ungraded version of the same frame.
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
/// labeled-graph preview for its effects. The `extract_frame_complex`
/// pipeline handles `graph: None` by falling back to the raw
/// extract, so a clip with no graded effects is not an error —
/// the caller sees `(clip ... had no graded effects)` in the
/// summary line.
fn resolve_grade_preview(
    project_root: &Path,
    clip_name: &str,
) -> Result<ClipGradePreview, FunctionCallError> {
    let otio_path = project_root.join(files::OTIO);
    let raw = std::fs::read_to_string(&otio_path).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "view_frame: project OTIO unreadable at {}: {e}",
            otio_path.display()
        ))
    })?;
    let tl: Timeline = serde_json::from_str(&raw).map_err(|e| {
        FunctionCallError::RespondToModel(format!("view_frame: project OTIO not valid JSON: {e}"))
    })?;
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
    Err(FunctionCallError::RespondToModel(format!(
        "view_frame: clip {clip_name:?} not found in project OTIO at {}",
        otio_path.display()
    )))
}

const DESCRIPTION: &str = "\
Extract a single frame from a video asset at time `t_s` and return it \
as an inline image. Use this to *see* a moment — for example, to confirm \
a cut lands on the right shot, or to read text on screen. \
detail='preview' (default, ≤768px longest edge) keeps the image cheap; \
detail='original' returns source resolution. format='png' (default) | \
'jpeg'. Frames are cached under .montage/cache/frames/ keyed by \
(asset, time, format, dim).\
";

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    fn ctx_at(root: &Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: montage_render::JobManager::new(),

            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(montage_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "view_frame".into(),
            args,
        }
    }

    #[tokio::test]
    async fn missing_asset_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = ViewFrameTool
            .handle(
                invoke(serde_json::json!({"asset": "raw/missing.mp4", "t_s": 0.0})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => assert!(msg.contains("not found")),
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn negative_time_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = ViewFrameTool
            .handle(
                invoke(serde_json::json!({"asset": "raw/x.mp4", "t_s": -1.0})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("must be finite and ≥ 0"))
        );
    }

    #[tokio::test]
    async fn bad_detail_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = ViewFrameTool
            .handle(
                invoke(serde_json::json!({
                    "asset": "raw/x.mp4", "t_s": 0.0, "detail": "ultra"
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("'ultra'")));
    }

    #[tokio::test]
    async fn dotdot_in_asset_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err = ViewFrameTool
            .handle(
                invoke(serde_json::json!({"asset": "../escape.mp4", "t_s": 0.0})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("'..'")));
    }

    #[test]
    fn cache_path_includes_dim_and_time() {
        let p = cache_path_for(
            Path::new("/proj"),
            Path::new("/proj/raw/x.mp4"),
            12.345,
            ImageFormat::Png,
            Some(768),
            "",
        )
        .unwrap();
        let s = p.to_string_lossy();
        assert!(s.contains("/.montage/cache/frames/"));
        assert!(s.ends_with("12345_d768_raw.png"));
    }

    #[test]
    fn cache_path_for_original_uses_orig_tag() {
        let p = cache_path_for(
            Path::new("/proj"),
            Path::new("/proj/raw/x.mp4"),
            0.0,
            ImageFormat::Jpeg,
            None,
            "",
        )
        .unwrap();
        assert!(p.to_string_lossy().ends_with("0_orig_raw.jpg"));
    }

    #[test]
    fn cache_path_partitions_graded_from_raw() {
        let raw = cache_path_for(
            Path::new("/proj"),
            Path::new("/proj/raw/x.mp4"),
            1.0,
            ImageFormat::Png,
            Some(768),
            "",
        )
        .unwrap();
        let graded = cache_path_for(
            Path::new("/proj"),
            Path::new("/proj/raw/x.mp4"),
            1.0,
            ImageFormat::Png,
            Some(768),
            "lut3d=file='X':interp=tetrahedral",
        )
        .unwrap();
        assert_ne!(
            raw, graded,
            "graded preview must not collide with raw cache"
        );
        assert!(raw.to_string_lossy().contains("_raw."));
        assert!(graded.to_string_lossy().contains("_g"));
    }
}
