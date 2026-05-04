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
use awidat_render::RenderJobSpec;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

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

        // Fork on scope. Timeline scope walks the OTIO; the asset-based
        // scopes keep their original path-validation flow.
        let (argv, total_duration_s, asset_label, output_path) = if args.scope == "timeline" {
            let segs = collect_timeline_segments(&ctx.project_root)?;
            if segs.is_empty() {
                return Err(FunctionCallError::RespondToModel(
                    "start_render: timeline has no clips to render. \
                     Add clips via apply_edl first, or use scope=preview to render \
                     the raw asset."
                        .into(),
                ));
            }
            let total = segs.iter().map(|s| s.duration_s).sum::<f64>();
            let output_path = renders_dir.join(format!("timeline-{}.mp4", timestamp));
            let argv = build_timeline_argv(&segs, &output_path);
            (argv, Some(total), "<timeline>".to_string(), output_path)
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
            let output_path = renders_dir.join(format!(
                "{}-{}-{}.mp4", args.scope, stem, timestamp
            ));
            let argv = build_ffmpeg_argv(&args, asset, &asset_path, &output_path)?;
            (argv, range_duration(&args.range), asset.to_string(), output_path)
        };

        let spec = RenderJobSpec {
            args: argv,
            total_duration_s,
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
            "asset": asset_label,
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
    _asset_rel: &str,
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
                "start_render: scope '{other}' not recognized. Use one of: preview, segment, full, timeline."
            )));
        }
    }
    Ok(argv)
}

/// One source-media segment to feed into the timeline-render concat.
#[derive(Debug, Clone)]
struct TimelineSegment {
    /// Absolute path to the source media.
    asset_path: PathBuf,
    /// Seconds into the source media where the cut starts.
    start_s: f64,
    /// Seconds of duration to take from the source.
    duration_s: f64,
}

/// Walk `<project_root>/project.otio.json` and collect every video-track
/// clip's `(asset, source_range)` in playback order. Skips Gap, Transition,
/// and nested Stack children for v1 — those land later. Audio tracks are
/// not enumerated separately because most awidat projects keep video and
/// audio paired in the same source file; the concat filter uses the audio
/// stream from each input.
fn collect_timeline_segments(
    project_root: &std::path::Path,
) -> Result<Vec<TimelineSegment>, FunctionCallError> {
    use awidat_proto::otio::{MediaReference, StackChild, TrackChild, TrackKind};
    use awidat_proto::project::read_otio_timeline;

    let otio_path = project_root.join("project.otio.json");
    if !otio_path.exists() {
        return Err(FunctionCallError::RespondToModel(format!(
            "start_render: no project.otio.json found at {} — \
             this isn't an awidat project root.",
            otio_path.display()
        )));
    }
    let mut warnings = Vec::new();
    let timeline = read_otio_timeline(&otio_path, &mut warnings).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "start_render: failed to load timeline ({e})"
        ))
    })?;

    let mut segs = Vec::new();
    for child in &timeline.tracks.children {
        let StackChild::Track(track) = child else { continue };
        if !matches!(track.kind, TrackKind::Video) {
            continue;
        }
        for tc in &track.children {
            let TrackChild::Clip(clip) = tc else { continue };
            let MediaReference::External(ext) = &clip.media_reference else { continue };
            let Some(range) = clip.source_range.as_ref() else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "start_render: clip '{}' has no source_range — \
                     can't extract a renderable segment.",
                    clip.name
                )));
            };
            let asset_path = project_root.join(&ext.target_url);
            if !asset_path.exists() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "start_render: timeline references missing asset {} \
                     (clip '{}'). Re-import the source file.",
                    asset_path.display(),
                    clip.name
                )));
            }
            segs.push(TimelineSegment {
                asset_path,
                start_s: range.start_time.to_seconds(),
                duration_s: range.duration.to_seconds(),
            });
        }
    }
    Ok(segs)
}

/// Build the ffmpeg argv that concats `segs` into `output_path` with a
/// single re-encode. Each segment becomes one input with `-ss/-t`; the
/// concat filter glues them and the encoder writes one well-formed mp4.
/// The re-encode is what fixes the DTS-seam scratch — stream-copy concat
/// at non-keyframe-aligned cuts produces audible clicks.
fn build_timeline_argv(
    segs: &[TimelineSegment],
    output_path: &std::path::Path,
) -> Vec<String> {
    let mut argv = vec!["-y".to_string(), "-loglevel".into(), "info".into()];
    for s in segs {
        argv.extend([
            "-ss".into(),
            format!("{}", s.start_s),
            "-t".into(),
            format!("{}", s.duration_s),
            "-i".into(),
            s.asset_path.to_string_lossy().into_owned(),
        ]);
    }
    let n = segs.len();
    let mut filter = String::new();
    for i in 0..n {
        filter.push_str(&format!("[{i}:v:0][{i}:a:0]"));
    }
    filter.push_str(&format!("concat=n={n}:v=1:a=1[outv][outa]"));
    argv.extend([
        "-filter_complex".into(),
        filter,
        "-map".into(),
        "[outv]".into(),
        "-map".into(),
        "[outa]".into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "20".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        output_path.to_string_lossy().into_owned(),
    ]);
    argv
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
timeline' — it captures Trim/Untrim/Delete/Split decisions. Output \
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
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo { name: "test".into(), version: "0.0.0".into() }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
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
            asset: Some("raw/x.mp4".into()),
            range: None,
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
            range: Some(TimeRange { start_s: 1.0, end_s: 3.5 }),
        };
        let argv = build_ffmpeg_argv(&args, "raw/x.mp4", asset, out).unwrap();
        assert!(argv.iter().any(|a| a == "copy"));
        assert!(argv.iter().any(|a| a == "1"));
        assert!(argv.iter().any(|a| a == "2.5"));
    }

    #[test]
    fn argv_for_timeline_uses_concat_filter_and_reencode() {
        let segs = vec![
            TimelineSegment {
                asset_path: PathBuf::from("/proj/raw/a.mp4"),
                start_s: 0.0,
                duration_s: 5.0,
            },
            TimelineSegment {
                asset_path: PathBuf::from("/proj/raw/b.mp4"),
                start_s: 12.5,
                duration_s: 3.25,
            },
        ];
        let out = std::path::Path::new("/proj/renders/timeline-000000.mp4");
        let argv = build_timeline_argv(&segs, out);
        // Both inputs present with correct -ss/-t.
        let cmd = argv.join(" ");
        assert!(cmd.contains("-ss 0 -t 5"));
        assert!(cmd.contains("-ss 12.5 -t 3.25"));
        // Concat filter wires both streams with v+a, n=2.
        assert!(argv.iter().any(|a| a == "-filter_complex"));
        assert!(argv.iter().any(|a| a.contains("concat=n=2:v=1:a=1[outv][outa]")));
        // Re-encode (not stream-copy) — this is the scratch fix.
        assert!(argv.iter().any(|a| a == "libx264"));
        assert!(!argv.iter().any(|a| a == "copy"));
    }

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
        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("project.otio.json")));
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

    #[test]
    fn collect_timeline_segments_skips_audio_tracks_and_gaps() {
        use awidat_proto::otio::{
            Clip, ExternalReference, Gap, MediaReference, RationalTime, Stack, StackChild,
            TimeRange as OtioRange, Timeline, Track, TrackChild, TrackKind,
        };
        let dir = tempfile::tempdir().unwrap();
        let asset_rel = "raw/x.mp4";
        std::fs::create_dir_all(dir.path().join("raw")).unwrap();
        std::fs::write(dir.path().join(asset_rel), b"stub").unwrap();

        let mut clip = Clip::empty("c1".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_rel));
        clip.source_range = Some(OtioRange::new(
            RationalTime::new(2.0 * 24.0, 24.0),
            RationalTime::new(3.0 * 24.0, 24.0),
        ));
        let gap = Gap::of_duration(0.5, 24.0);
        let mut vtrack = Track::empty("V1", TrackKind::Video);
        vtrack.children.push(TrackChild::Clip(clip));
        vtrack.children.push(TrackChild::Gap(gap));

        // Audio track with a clip that should NOT show up.
        let mut atrack = Track::empty("A1", TrackKind::Audio);
        let mut aclip = Clip::empty("a-only".to_string());
        aclip.media_reference = MediaReference::External(ExternalReference::new(asset_rel));
        aclip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(24.0, 24.0),
        ));
        atrack.children.push(TrackChild::Clip(aclip));

        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(vtrack));
        stack.children.push(StackChild::Track(atrack));
        tl.tracks = stack;

        std::fs::write(
            dir.path().join("project.otio.json"),
            serde_json::to_string_pretty(&tl).unwrap(),
        )
        .unwrap();

        let segs = collect_timeline_segments(dir.path()).unwrap();
        assert_eq!(segs.len(), 1, "should have one video clip, no gap, no audio-only");
        assert!((segs[0].start_s - 2.0).abs() < 1e-9);
        assert!((segs[0].duration_s - 3.0).abs() < 1e-9);
    }
}
