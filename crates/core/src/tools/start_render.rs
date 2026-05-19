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
//! - `full` — full-quality re-encode of an asset. Higher bitrate.
//! - `timeline` — render the *edited* timeline. Walks
//!   `project.otio.json`, builds one ffmpeg input per video-track clip
//!   (with `-ss`/`-t` aligned to the clip's source_range), and concats
//!   them via the `concat` filter with re-encode at boundaries. The
//!   re-encode kills the DTS-seam click that stream-copy concat
//!   produces at non-keyframe-aligned cut points. `asset` is ignored
//!   in this scope.
//!
//! Output naming: `<project>/renders/<scope>-<asset-stem>-<job_id>.mp4`.
//! Predictable so the user can find it; job_id-suffixed to avoid clobber.

use std::path::PathBuf;

use async_trait::async_trait;
use awidat_render::{
    OutputPathPolicy, RenderJobSpec, RenderPlanLimitation, validate_render_output_path,
};
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `start_render` tool.
pub struct StartRenderTool;

#[derive(Debug, Deserialize)]
struct StartRenderArgs {
    /// `preview` | `segment` | `full` | `timeline`. See module doc.
    scope: String,
    /// Source asset (project-relative). Required for preview/segment/full;
    /// ignored for `timeline` (the project's OTIO is the source).
    #[serde(default)]
    asset: Option<String>,
    /// Optional [start_s, end_s) range. Required for `segment`.
    #[serde(default)]
    range: Option<TimeRange>,
    /// Optional timeline guide marker section. Only used with `scope=timeline`.
    #[serde(default)]
    guide: Option<GuideSection>,
}

#[derive(Debug, Deserialize)]
struct TimeRange {
    start_s: f64,
    end_s: f64,
}

#[derive(Debug, Deserialize)]
struct GuideSection {
    track_id: String,
    marker_id: String,
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
                        "enum": ["preview", "segment", "full", "timeline"],
                        "description": "preview = low-bitrate 480p of an asset; segment = trim a range from an asset; full = full-bitrate of an asset; timeline = render the edited OTIO timeline (asset is ignored)."
                    },
                    "asset": {
                        "type": "string",
                        "description": "Project-relative source asset path. Required for preview/segment/full; ignored for timeline."
                    },
                    "range": {
                        "type": "object",
                        "properties": {
                            "start_s": { "type": "number", "minimum": 0.0 },
                            "end_s":   { "type": "number", "minimum": 0.0 }
                        },
                        "required": ["start_s", "end_s"],
                        "description": "[start_s, end_s) time range. Required for scope=segment; ignored otherwise."
                    },
                    "guide": {
                        "type": "object",
                        "properties": {
                            "track_id": { "type": "string" },
                            "marker_id": { "type": "string" }
                        },
                        "required": ["track_id", "marker_id"],
                        "description": "Optional guide-track marker range for scope=timeline section exports. Ignored for preview/segment/full."
                    }
                },
                "required": ["scope"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        let scope = invocation
            .args
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing-scope>");
        let asset = invocation
            .args
            .get("asset")
            .and_then(|v| v.as_str())
            .unwrap_or("<timeline>");
        let range = invocation
            .args
            .get("range")
            .map(serde_json::Value::to_string)
            .unwrap_or_else(|| "<full>".to_string());
        let guide = invocation
            .args
            .get("guide")
            .map(serde_json::Value::to_string)
            .unwrap_or_else(|| "<full-timeline>".to_string());
        vec![ApprovalKey::new(
            "start_render",
            format!("{scope}:{asset}:{range}:{guide}:writes=renders"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: StartRenderArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_render: invalid args ({e}). Required: {{ \"scope\": \"preview|segment|full|timeline\", \"asset\": <str> (omit for timeline) }}."
            ))
        })?;

        let renders_dir = ctx.project_root.join("renders");
        tokio::fs::create_dir_all(&renders_dir).await.ok();
        let timestamp = chrono::Utc::now().format("%H%M%S");

        // Fork on scope. Timeline scope delegates to awidat-render's
        // shared planner so the agent and the desktop's Export button
        // produce identical specs. The asset-based scopes keep their
        // original path-validation flow.
        let (argv, total_duration_s, asset_label, output_path, limitations) = if args.scope
            == "timeline"
        {
            crate::lessons::apply_learned_project_format_defaults(&ctx.project_root)
                .map_err(|e| FunctionCallError::RespondToModel(format!("start_render: {e}")))?;
            let spec = match args.guide.as_ref() {
                Some(guide) => awidat_render::build_timeline_section_render_spec(
                    &ctx.project_root,
                    &guide.track_id,
                    &guide.marker_id,
                ),
                None => awidat_render::build_timeline_render_spec(&ctx.project_root),
            }
            .map_err(|e| {
                    use awidat_render::RenderTimelineError;
                    let msg = match e {
                        RenderTimelineError::EmptyTimeline => {
                            "start_render: timeline has no clips to render. \
                         Add clips via apply_edl first, or use scope=preview to render \
                         the raw asset."
                                .to_string()
                        }
                        RenderTimelineError::NoOtio(p) => format!(
                            "start_render: no project.otio.json found at {} — \
                         this isn't an awidat project root.",
                            p.display()
                        ),
                        RenderTimelineError::OtioParse { message } => format!(
                            "start_render: timeline parse failed ({message}). \
                         Run `awidat validate <project>` for the detailed diagnostic, \
                         then revert the most recent apply_edl that broke the OTIO."
                        ),
                        RenderTimelineError::MissingAsset { clip_name, missing } => format!(
                            "start_render: timeline references missing asset {} \
                         (clip '{clip_name}'). Re-import the source file.",
                            missing.display()
                        ),
                        RenderTimelineError::MissingLut { clip_name, missing } => format!(
                            "start_render: clip '{clip_name}' references missing LUT {}. \
                         Add the LUT under the project root or update the Apply LUT path.",
                            missing.display()
                        ),
                        RenderTimelineError::ClipMissingRange { clip_name } => format!(
                            "start_render: clip '{clip_name}' has no source_range — \
                         can't extract a renderable segment."
                        ),
                        RenderTimelineError::TransitionHandleUnavailable {
                            kind,
                            clip_name,
                            side,
                            needed_s,
                            available_s,
                        } => format!(
                            "start_render: transition {kind:?} around clip '{clip_name}' needs \
                         {needed_s:.3}s {side} handle, but only {available_s:.3}s is available. \
                         Shorten the transition, choose a different alignment, or apply Untrim Clip \
                         to widen the source range before rendering."
                        ),
                        RenderTimelineError::UnsupportedTransition { kind, message } => format!(
                            "start_render: timeline transition {kind:?} cannot be exported: \
                         {message}"
                        ),
                        RenderTimelineError::InvalidTransitionPlacement { message } => {
                            format!("start_render: timeline has an invalid transition: {message}")
                        }
                        RenderTimelineError::InvalidTransitionMetadata { kind, message } => {
                            format!(
                                "start_render: transition {kind:?} has invalid Awidat metadata: \
                         {message}. Use data-only transition primitives with bounded params; do \
                         not put raw FFmpeg, GLSL, shell, or plugin code in transition metadata."
                            )
                        }
                        RenderTimelineError::BroadcastOverlayRender(message) => {
                            format!("start_render: broadcast overlay render failed: {message}")
                        }
                        RenderTimelineError::GuideSectionNotFound {
                            guide_track_id,
                            marker_id,
                        } => format!(
                            "start_render: guide section {guide_track_id}/{marker_id} was not found"
                        ),
                        RenderTimelineError::GuideSectionMissingDuration {
                            guide_track_id,
                            marker_id,
                        } => format!(
                            "start_render: guide section {guide_track_id}/{marker_id} needs a positive duration"
                        ),
                    };
                    FunctionCallError::RespondToModel(msg)
                })?;
            (
                spec.args,
                spec.total_duration_s,
                "<timeline>".to_string(),
                spec.output_path,
                spec.limitations,
            )
        } else {
            // Asset-based scope: preview / segment / full.
            let asset = args.asset.as_deref().ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "start_render: scope='{}' requires an `asset` path",
                    args.scope
                ))
            })?;
            if asset.contains("..") {
                return Err(FunctionCallError::RespondToModel(format!(
                    "start_render: asset '{asset}' must not contain '..' segments"
                )));
            }
            let asset_path = ctx.project_root.join(asset);
            if !asset_path.exists() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "start_render: asset '{asset}' not found at {}",
                    asset_path.display()
                )));
            }
            let stem = asset_stem(asset);
            let output_path =
                renders_dir.join(format!("{}-{}-{}.mp4", args.scope, stem, timestamp));
            validate_render_output_path(
                &ctx.project_root,
                &output_path,
                std::slice::from_ref(&asset_path),
                &[],
                OutputPathPolicy::default(),
            )
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "start_render: output path preflight failed: {e}"
                ))
            })?;
            let argv = build_ffmpeg_argv(&args, asset, &asset_path, &output_path)?;
            (
                argv,
                range_duration(&args.range),
                asset.to_string(),
                output_path,
                Vec::<RenderPlanLimitation>::new(),
            )
        };

        let spec = RenderJobSpec {
            args: argv,
            total_duration_s,
            cwd: Some(ctx.project_root.clone()),
            output_path: output_path.clone(),
            limitations: limitations.clone(),
        };
        let job_id = ctx.job_manager.start(spec).await.map_err(|e| {
            FunctionCallError::RespondToModel(format!("start_render: failed to start ffmpeg: {e}"))
        })?;

        let body = serde_json::json!({
            "job_id": job_id.to_string(),
            "scope": args.scope,
            "render_kind": if args.scope == "timeline" && args.guide.is_some() {
                "timeline_section_export"
            } else if args.scope == "timeline" {
                "final_timeline_export"
            } else {
                "diagnostic_asset_render"
            },
            "asset": asset_label,
            "output_path": output_path.display().to_string(),
            "render_limitations": limitations,
            "started_at": chrono::Utc::now().to_rfc3339(),
            "guide": args.guide.as_ref().map(|guide| serde_json::json!({
                "track_id": guide.track_id,
                "marker_id": guide.marker_id,
            })),
            "next_step": if args.scope == "timeline" {
                format!("Call poll_render(job_id=\"{job_id}\") to track the final timeline export.")
            } else {
                format!("Call poll_render(job_id=\"{job_id}\") to track this diagnostic asset render; use scope=\"timeline\" for final editorial output.")
            },
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
    _asset_rel: &str,
    asset_path: &std::path::Path,
    output_path: &std::path::Path,
) -> Result<Vec<String>, FunctionCallError> {
    let mut argv = vec!["-y".to_string(), "-loglevel".into(), "info".into()];
    match args.scope.as_str() {
        "preview" => {
            argv.extend([
                "-i".into(),
                asset_path.to_string_lossy().into_owned(),
                "-vf".into(),
                "scale=-2:480".into(),
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "veryfast".into(),
                "-crf".into(),
                "28".into(),
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                "96k".into(),
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
                "-ss".into(),
                format!("{}", r.start_s),
                "-i".into(),
                asset_path.to_string_lossy().into_owned(),
                "-t".into(),
                format!("{}", r.end_s - r.start_s),
                "-c".into(),
                "copy".into(),
                output_path.to_string_lossy().into_owned(),
            ]);
        }
        "full" => {
            argv.extend([
                "-i".into(),
                asset_path.to_string_lossy().into_owned(),
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "medium".into(),
                "-crf".into(),
                "20".into(),
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                "192k".into(),
                output_path.to_string_lossy().into_owned(),
            ]);
        }
        other => {
            return Err(FunctionCallError::RespondToModel(format!(
                "start_render: scope '{other}' not recognized. Use one of: preview, segment, full, timeline."
            )));
        }
    }
    Ok(argv)
}

const DESCRIPTION: &str = "\
Kick off a background ffmpeg render. Returns a job_id IMMEDIATELY — the \
render runs in the background and you call `poll_render` to check status. \
Scopes: 'preview' = 480p H.264 of an asset (fast, low-bitrate); 'segment' \
= trim [start_s, end_s) of an asset via stream-copy (very fast, no \
re-encode); 'full' = high-bitrate H.264 of an asset (slow); 'timeline' \
= render the *edited timeline* (walks project.otio.json, concats every \
video clip's source_range with re-encode at boundaries). Use 'timeline' \
when the user wants 'render the edit' or 'export what's in the \
timeline' — it captures Trim/Untrim/Delete/Split decisions. Preview, \
segment, and full are source-asset diagnostics, not substitutes for \
graph edits or final editorial export. Output \
lands under <project>/renders/. Don't await this tool — it always \
returns within a second.\
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
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
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
        assert!(
            matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("segment requires"))
        );
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
            asset: Some("raw/x.mp4".into()),
            range: None,
            guide: None,
        };
        let argv = build_ffmpeg_argv(&args, "raw/x.mp4", asset, out).unwrap();
        assert!(argv.contains(&"libx264".to_string()));
        assert!(argv.contains(&out.to_string_lossy().into_owned()));
    }

    #[test]
    fn argv_for_segment_uses_stream_copy() {
        let asset = std::path::Path::new("/proj/raw/x.mp4");
        let out = std::path::Path::new("/proj/renders/segment-x-000000.mp4");
        let args = StartRenderArgs {
            scope: "segment".into(),
            asset: Some("raw/x.mp4".into()),
            range: Some(TimeRange {
                start_s: 1.0,
                end_s: 3.5,
            }),
            guide: None,
        };
        let argv = build_ffmpeg_argv(&args, "raw/x.mp4", asset, out).unwrap();
        assert!(argv.iter().any(|a| a == "copy"));
        assert!(argv.iter().any(|a| a == "1"));
        assert!(argv.iter().any(|a| a == "2.5"));
    }

    #[test]
    fn description_requires_timeline_scope_for_edits() {
        assert!(DESCRIPTION.contains("render the *edited timeline*"));
        assert!(DESCRIPTION.contains("not substitutes for graph edits"));
        assert!(DESCRIPTION.contains("final editorial export"));
    }

    #[test]
    fn schema_exposes_timeline_guide_section_selection() {
        let schema = StartRenderTool.schema().input_schema;
        assert_eq!(
            schema["properties"]["guide"]["properties"]["track_id"]["type"],
            "string"
        );
        assert_eq!(
            schema["properties"]["guide"]["properties"]["marker_id"]["type"],
            "string"
        );
        assert!(
            schema["properties"]["guide"]["description"]
                .as_str()
                .unwrap()
                .contains("scope=timeline")
        );
    }

    // Pure-function tests for the timeline-render planner moved to
    // `awidat-render::timeline` alongside the implementation. The
    // tests below exercise the tool's error-mapping path end-to-end.

    #[tokio::test]
    async fn timeline_scope_without_otio_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = StartRenderTool
            .handle(
                invoke(serde_json::json!({"scope": "timeline"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("project.otio.json"))
        );
    }

    #[tokio::test]
    async fn timeline_scope_with_empty_timeline_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        // Init a real project (empty timeline).
        awidat_proto::project::Project::init(dir.path()).unwrap();
        let err = StartRenderTool
            .handle(
                invoke(serde_json::json!({"scope": "timeline"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("no clips")));
    }
}
