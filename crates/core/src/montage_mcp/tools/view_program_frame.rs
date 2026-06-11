//! `view_program_frame` — render one composed timeline frame for visual QA.
//!
//! Unlike `view_frame`, this reads the project timeline and uses the same
//! render lowering as `start_render scope=timeline`, so MotionScene layers,
//! titles, B-roll/PiP, annotations, and broadcast overlays are visible
//! together.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use montage_proto::project::files;
use montage_render::ffmpeg::{ImageFormat, extract_frame_filtered};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::montage_mcp::context::McpToolCtx;

const PREVIEW_MAX_DIM: u32 = 768;
const FRAME_WINDOW_S: f64 = 0.12;

/// Arguments to `view_program_frame`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ViewProgramFrameArgs {
    /// Timeline-local second to inspect.
    pub t_s: f64,
    /// `"preview"` (default; resized to ~768px on the longer edge) or
    /// `"original"` (no resize).
    #[serde(default)]
    pub detail: Option<String>,
    /// Image format: `"png"` (default) or `"jpeg"`.
    #[serde(default)]
    pub format: Option<String>,
}

/// Render the composed program frame at `t_s` and return a JSON body
/// carrying a base64 image payload plus cache paths/evidence.
pub async fn run(args: ViewProgramFrameArgs, ctx: McpToolCtx) -> Result<String, String> {
    if !args.t_s.is_finite() || args.t_s < 0.0 {
        return Err(format!(
            "view_program_frame: t_s ({}) must be finite and >= 0",
            args.t_s
        ));
    }

    let detail = match args.detail.as_deref().unwrap_or("preview") {
        "preview" => Detail::Preview,
        "original" => Detail::Original,
        other => {
            return Err(format!(
                "view_program_frame: detail '{other}' not recognized. Use 'preview' or 'original'."
            ));
        }
    };
    let format = match args.format.as_deref().unwrap_or("png") {
        "png" => ImageFormat::Png,
        "jpeg" => ImageFormat::Jpeg,
        other => {
            return Err(format!(
                "view_program_frame: format '{other}' not recognized. Use 'png' or 'jpeg'."
            ));
        }
    };

    let max_dim = match detail {
        Detail::Preview => Some(PREVIEW_MAX_DIM),
        Detail::Original => None,
    };
    let frame_path = cache_frame_path(&ctx.project_root, args.t_s, format, max_dim)?;
    let clip_path = frame_path.with_extension("mp4");

    if !frame_path.exists() {
        render_program_window(&ctx.project_root, args.t_s, &clip_path).await?;
        let bytes = extract_frame_filtered(&clip_path, 0.0, format, max_dim, None)
            .await
            .map_err(|e| format!("view_program_frame: frame extraction failed: {e}"))?;
        if let Some(parent) = frame_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("view_program_frame: cache mkdir failed: {e}"))?;
        }
        tokio::fs::write(&frame_path, &bytes)
            .await
            .map_err(|e| format!("view_program_frame: cache write failed: {e}"))?;
    }

    let bytes = tokio::fs::read(&frame_path)
        .await
        .map_err(|e| format!("view_program_frame: cache read failed: {e}"))?;
    let summary = format!(
        "composed program frame {:.3}s ({}, {} bytes)",
        args.t_s,
        format.media_type(),
        bytes.len()
    );
    Ok(serde_json::json!({
        "summary": summary,
        "t_s": args.t_s,
        "media_type": format.media_type(),
        "byte_len": bytes.len(),
        "image_base64": B64.encode(&bytes),
        "frame_path": frame_path,
        "render_window_path": clip_path,
        "render_window_s": FRAME_WINDOW_S,
    })
    .to_string())
}

#[derive(Debug, Clone, Copy)]
enum Detail {
    Preview,
    Original,
}

async fn render_program_window(
    project_root: &Path,
    t_s: f64,
    output_path: &Path,
) -> Result<(), String> {
    let mut spec = montage_render::build_timeline_render_spec(project_root)
        .map_err(|e| format!("view_program_frame: timeline render plan failed: {e}"))?;
    rewrite_timeline_argv_for_window(&mut spec.args, t_s, FRAME_WINDOW_S, output_path)?;
    let ffmpeg = montage_render::ffmpeg_path()
        .map_err(|e| format!("view_program_frame: failed to locate ffmpeg: {e}"))?;
    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("view_program_frame: render cache mkdir failed: {e}"))?;
    }
    let output = Command::new(ffmpeg)
        .args(&spec.args)
        .current_dir(spec.cwd.as_deref().unwrap_or(project_root))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("view_program_frame: failed to start ffmpeg: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "view_program_frame: ffmpeg failed with status {}: {}",
            output.status,
            tail(&stderr, 4000)
        ));
    }
    Ok(())
}

fn rewrite_timeline_argv_for_window(
    argv: &mut Vec<String>,
    start_s: f64,
    duration_s: f64,
    output_path: &Path,
) -> Result<(), String> {
    if argv.is_empty() {
        return Err("view_program_frame: timeline render argv is empty".into());
    }
    let output_index = argv.len() - 1;
    argv[output_index] = output_path.to_string_lossy().into_owned();
    argv.splice(
        output_index..output_index,
        [
            "-ss".to_string(),
            format!("{start_s}"),
            "-t".to_string(),
            format!("{duration_s}"),
        ],
    );
    Ok(())
}

fn cache_frame_path(
    project_root: &Path,
    t_s: f64,
    format: ImageFormat,
    max_dim: Option<u32>,
) -> Result<PathBuf, String> {
    let mut h = Sha256::new();
    h.update(project_root.to_string_lossy().as_bytes());
    let project_bytes = std::fs::read(project_root.join(files::OTIO))
        .map_err(|e| format!("view_program_frame: project OTIO unreadable: {e}"))?;
    h.update(&project_bytes);
    let root_hash = format!("{:x}", h.finalize());
    let project_tag: String = root_hash.chars().take(12).collect();
    let t_ms = (t_s * 1000.0).round() as i64;
    let dim_tag = max_dim
        .map(|d| format!("d{d}"))
        .unwrap_or_else(|| "orig".to_string());
    let ext = match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
    };
    Ok(project_root
        .join(".montage")
        .join("cache")
        .join("program-frames")
        .join(project_tag)
        .join(format!("{t_ms}_{dim_tag}.{ext}")))
}

fn tail(value: &str, max_chars: usize) -> String {
    let mut chars: Vec<char> = value.chars().rev().take(max_chars).collect();
    chars.reverse();
    chars.into_iter().collect()
}

pub const DESCRIPTION: &str = "\
Render one composed program frame at timeline-local `t_s` and return a JSON \
payload carrying base64-encoded image bytes plus cache paths. This is for \
visual QA after applying timeline visuals: it uses the same timeline render \
lowering as start_render, so MotionScene layers and broadcast overlays are \
captured together. detail='preview' (default, <=768px longest edge) keeps the \
image cheap; detail='original' returns source resolution. format='png' \
(default) | 'jpeg'.\
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_timeline_argv_replaces_output_and_clips_window() {
        let mut argv = vec![
            "-y".into(),
            "-i".into(),
            "raw/a.mp4".into(),
            "-c:v".into(),
            "libx264".into(),
            "renders/timeline.mp4".into(),
        ];

        rewrite_timeline_argv_for_window(&mut argv, 12.5, 0.12, Path::new("/tmp/frame-window.mp4"))
            .unwrap();

        assert_eq!(argv[argv.len() - 1], "/tmp/frame-window.mp4");
        assert_eq!(
            &argv[argv.len() - 5..argv.len() - 1],
            ["-ss", "12.5", "-t", "0.12"]
        );
    }

    #[test]
    fn cache_frame_path_uses_time_dimension_and_format() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(files::OTIO), "{}").unwrap();
        let path = cache_frame_path(dir.path(), 1.234, ImageFormat::Jpeg, Some(768)).unwrap();
        let rendered = path.to_string_lossy();
        assert!(rendered.contains("/.montage/cache/program-frames/"));
        assert!(rendered.ends_with("1234_d768.jpg"));
    }

    #[test]
    fn cache_frame_path_changes_when_project_changes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(files::OTIO), "{\"a\":1}").unwrap();
        let first = cache_frame_path(dir.path(), 1.0, ImageFormat::Png, Some(768)).unwrap();

        std::fs::write(dir.path().join(files::OTIO), "{\"a\":2}").unwrap();
        let second = cache_frame_path(dir.path(), 1.0, ImageFormat::Png, Some(768)).unwrap();

        assert_ne!(first, second);
    }
}
