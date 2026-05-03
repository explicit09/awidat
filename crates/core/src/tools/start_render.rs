//! `start_render` tool — kick off a background ffmpeg render and
//! return a job id.
//!
//! Per the corpus survey of Codex's `unified_exec` and Crush's
//! `BackgroundShellManager`: **always async**, never sync. Renders take
//! seconds-to-minutes; we never want to block the agent loop on one.
//!
//! Scope vocabulary (v1):
//! - `preview` — re-encode an asset to a low-bitrate H.264 / 480p mp4
//!   under `<project>/renders/`. Default codec, fast.
//! - `segment` — extract a time range from an asset (no re-encode if
//!   format permits stream-copy; otherwise H.264). Future: serve as the
//!   primitive `apply_edl` outputs route through.
//! - `full` — full-quality re-encode. Same as `preview` but at higher
//!   bitrate; placeholder until v1.5 has timeline-driven renders.
//!
//! Output naming: `<project>/renders/<scope>-<asset-stem>-<job_id>.mp4`.
//! Predictable so the user can find it; job_id-suffixed to avoid clobber.

use std::path::PathBuf;

use async_trait::async_trait;
use awidat_render::RenderJobSpec;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `start_render` tool.
pub struct StartRenderTool;

#[derive(Debug, Deserialize)]
struct StartRenderArgs {
    /// `preview` | `segment` | `full`. See module doc.
    scope: String,
    /// Source asset (project-relative).
    asset: String,
    /// Optional [start_s, end_s) range. Required for `segment`.
    #[serde(default)]
    range: Option<TimeRange>,
}

#[derive(Debug, Deserialize)]
struct TimeRange {
    start_s: f64,
    end_s: f64,
}

#[async_trait]
impl ToolHandler for StartRenderTool {
    fn name(&self) -> &'static str {
        "start_render"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "start_render".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "enum": ["preview", "segment", "full"],
                        "description": "preview = low-bitrate 480p; segment = trim a range; full = full-bitrate."
                    },
                    "asset": {
                        "type": "string",
                        "description": "Project-relative source asset path."
                    },
                    "range": {
                        "type": "object",
                        "properties": {
                            "start_s": { "type": "number", "minimum": 0.0 },
                            "end_s":   { "type": "number", "minimum": 0.0 }
                        },
                        "required": ["start_s", "end_s"],
                        "description": "[start_s, end_s) time range. Required for scope=segment; ignored otherwise."
                    }
                },
                "required": ["scope", "asset"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: StartRenderArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_render: invalid args ({e}). Required: {{ \"scope\": \"preview|segment|full\", \"asset\": <str> }}."
            ))
        })?;

        if args.asset.contains("..") {
            return Err(FunctionCallError::RespondToModel(format!(
                "start_render: asset '{}' must not contain '..' segments", args.asset
            )));
        }
        let asset_path = ctx.project_root.join(&args.asset);
        if !asset_path.exists() {
            return Err(FunctionCallError::RespondToModel(format!(
                "start_render: asset '{}' not found at {}",
                args.asset, asset_path.display()
            )));
        }

        // Build the ffmpeg argv per scope.
        let renders_dir = ctx.project_root.join("renders");
        tokio::fs::create_dir_all(&renders_dir).await.ok();
        let stem = asset_stem(&args.asset);
        // Job-id stub; we replace once allocated (the file path is built
        // before .start, so use a placeholder timestamp tag).
        let timestamp = chrono::Utc::now().format("%H%M%S");
        let output_path = renders_dir.join(format!(
            "{}-{}-{}.mp4", args.scope, stem, timestamp
        ));

        let argv = build_ffmpeg_argv(&args, &asset_path, &output_path)?;

        let spec = RenderJobSpec {
            args: argv,
            total_duration_s: range_duration(&args.range),
            cwd: Some(ctx.project_root.clone()),
            output_path: output_path.clone(),
        };
        let job_id = ctx.job_manager.start(spec).await.map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_render: failed to start ffmpeg: {e}"
            ))
        })?;

        let body = serde_json::json!({
            "job_id": job_id.to_string(),
            "scope": args.scope,
            "asset": args.asset,
            "output_path": output_path.display().to_string(),
            "started_at": chrono::Utc::now().to_rfc3339(),
            "next_step": format!("Call poll_render(job_id=\"{job_id}\") to track progress."),
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

fn asset_stem(asset: &str) -> String {
    PathBuf::from(asset)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset")
        .to_string()
}

fn range_duration(range: &Option<TimeRange>) -> Option<f64> {
    range.as_ref().map(|r| (r.end_s - r.start_s).max(0.0))
}

fn build_ffmpeg_argv(
    args: &StartRenderArgs,
    asset_path: &std::path::Path,
    output_path: &std::path::Path,
) -> Result<Vec<String>, FunctionCallError> {
    let mut argv = vec!["-y".to_string(), "-loglevel".into(), "info".into()];
    match args.scope.as_str() {
        "preview" => {
            argv.extend([
                "-i".into(), asset_path.to_string_lossy().into_owned(),
                "-vf".into(), "scale=-2:480".into(),
                "-c:v".into(), "libx264".into(),
                "-preset".into(), "veryfast".into(),
                "-crf".into(), "28".into(),
                "-c:a".into(), "aac".into(),
                "-b:a".into(), "96k".into(),
                output_path.to_string_lossy().into_owned(),
            ]);
        }
        "segment" => {
            let r = args.range.as_ref().ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "start_render: scope=segment requires `range: { start_s, end_s }`".into(),
                )
            })?;
            if r.end_s <= r.start_s {
                return Err(FunctionCallError::RespondToModel(format!(
                    "start_render: range.end_s ({}) must be > range.start_s ({})",
                    r.end_s, r.start_s
                )));
            }
            argv.extend([
                "-ss".into(), format!("{}", r.start_s),
                "-i".into(), asset_path.to_string_lossy().into_owned(),
                "-t".into(), format!("{}", r.end_s - r.start_s),
                "-c".into(), "copy".into(),
                output_path.to_string_lossy().into_owned(),
            ]);
        }
        "full" => {
            argv.extend([
                "-i".into(), asset_path.to_string_lossy().into_owned(),
                "-c:v".into(), "libx264".into(),
                "-preset".into(), "medium".into(),
                "-crf".into(), "20".into(),
                "-c:a".into(), "aac".into(),
                "-b:a".into(), "192k".into(),
                output_path.to_string_lossy().into_owned(),
            ]);
        }
        other => {
            return Err(FunctionCallError::RespondToModel(format!(
                "start_render: scope '{other}' not recognized. Use one of: preview, segment, full."
            )));
        }
    }
    Ok(argv)
}

const DESCRIPTION: &str = "\
Kick off a background ffmpeg render. Returns a job_id IMMEDIATELY — the \
render runs in the background and you call `poll_render` to check status. \
Scopes: 'preview' = 480p H.264 (fast, low-bitrate); 'segment' = trim \
[start_s, end_s) via stream-copy (very fast, no re-encode); 'full' = \
high-bitrate H.264 (slow). Output lands under <project>/renders/. \
Don't await this tool — it always returns within a second.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),

            approval_tx: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "start_render".into(),
            args,
        }
    }

    #[tokio::test]
    async fn missing_asset_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = StartRenderTool
            .handle(
                invoke(serde_json::json!({"scope": "preview", "asset": "raw/missing.mp4"})),
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
    async fn segment_without_range_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/x.mp4");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"stub").unwrap();
        let err = StartRenderTool
            .handle(
                invoke(serde_json::json!({"scope": "segment", "asset": "raw/x.mp4"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("segment requires")));
    }

    #[tokio::test]
    async fn dotdot_asset_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err = StartRenderTool
            .handle(
                invoke(serde_json::json!({"scope": "preview", "asset": "../escape.mp4"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("'..'")));
    }

    #[test]
    fn argv_for_preview_includes_libx264() {
        let asset = std::path::Path::new("/proj/raw/x.mp4");
        let out = std::path::Path::new("/proj/renders/preview-x-000000.mp4");
        let args = StartRenderArgs {
            scope: "preview".into(),
            asset: "raw/x.mp4".into(),
            range: None,
        };
        let argv = build_ffmpeg_argv(&args, asset, out).unwrap();
        assert!(argv.contains(&"libx264".to_string()));
        assert!(argv.contains(&out.to_string_lossy().into_owned()));
    }

    #[test]
    fn argv_for_segment_uses_stream_copy() {
        let asset = std::path::Path::new("/proj/raw/x.mp4");
        let out = std::path::Path::new("/proj/renders/segment-x-000000.mp4");
        let args = StartRenderArgs {
            scope: "segment".into(),
            asset: "raw/x.mp4".into(),
            range: Some(TimeRange { start_s: 1.0, end_s: 3.5 }),
        };
        let argv = build_ffmpeg_argv(&args, asset, out).unwrap();
        assert!(argv.iter().any(|a| a == "copy"));
        assert!(argv.iter().any(|a| a == "1"));
        assert!(argv.iter().any(|a| a == "2.5"));
    }
}
