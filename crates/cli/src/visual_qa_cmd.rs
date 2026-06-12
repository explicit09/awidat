use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::view_program_frame::{
    ViewProgramFrameArgs, run as render_program_frame,
};
use montage_core::visual_qa::audit_motion_scenes;
use montage_proto::otio::{Clip, Stack, StackChild, Track, TrackChild};
use montage_proto::project::Project;
use serde_json::Value;

pub struct MotionScenesArgs {
    pub project_root: PathBuf,
    pub scene_id: Option<String>,
    pub render_frames: bool,
    pub detail: Option<String>,
}

pub struct ProgramFrameArgs {
    pub project_root: PathBuf,
    pub t_s: f64,
    pub detail: Option<String>,
    pub format: Option<String>,
    pub include_base64: bool,
}

pub struct DesktopScreenshotArgs {
    pub project_root: PathBuf,
    pub output: Option<PathBuf>,
}

pub fn motion_scenes(args: MotionScenesArgs) -> Result<()> {
    let project = Project::read(&args.project_root)
        .with_context(|| format!("failed to read project at {}", args.project_root.display()))?;
    let duration_s = timeline_duration_s(&project.timeline);
    let mut report = serde_json::to_value(audit_motion_scenes(
        &project.timeline,
        duration_s,
        args.scene_id.as_deref(),
    ))
    .context("failed to serialize MotionScene QA report")?;

    if args.render_frames {
        let samples = report
            .get("frame_samples")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut rendered = Vec::new();
        for sample in samples {
            let Some(t_s) = sample.get("t_s").and_then(Value::as_f64) else {
                continue;
            };
            // A scene with structural problems (e.g. ending past the timeline)
            // can fail to render. Capture that failure in the report instead of
            // aborting the whole QA pass, so the structural issues still print.
            let mut frame = match render_program_frame_json(ProgramFrameArgs {
                project_root: args.project_root.clone(),
                t_s,
                detail: args.detail.clone(),
                format: Some("png".into()),
                include_base64: false,
            }) {
                Ok(frame) => frame,
                Err(error) => serde_json::json!({ "error": error.to_string() }),
            };
            if let Some(scene_id) = sample.get("scene_id") {
                frame["scene_id"] = scene_id.clone();
            }
            if let Some(label) = sample.get("label") {
                frame["sample_label"] = label.clone();
            }
            rendered.push(frame);
        }
        report["rendered_frames"] = Value::Array(rendered);
    }

    println!("{}", serde_json::to_string_pretty(&report)?);
    let issue_count = report
        .get("issues")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if issue_count > 0 {
        bail!("MotionScene visual QA found {issue_count} issue(s)");
    }
    Ok(())
}

pub fn program_frame(args: ProgramFrameArgs) -> Result<()> {
    let frame = render_program_frame_json(args)?;
    println!("{}", serde_json::to_string_pretty(&frame)?);
    Ok(())
}

pub fn desktop_screenshot(args: DesktopScreenshotArgs) -> Result<()> {
    let output = args.output.unwrap_or_else(|| {
        args.project_root
            .join(".montage")
            .join("visual-qa")
            .join(format!("desktop-screenshot-{}.png", unix_ms()))
    });
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let status = Command::new("/usr/sbin/screencapture")
        .args(["-x"])
        .arg(&output)
        .status()
        .context("failed to run screencapture")?;
    if !status.success() {
        bail!("screencapture failed with status {status}");
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "screenshot_path": output,
        }))?
    );
    Ok(())
}

fn render_program_frame_json(args: ProgramFrameArgs) -> Result<Value> {
    if !args.t_s.is_finite() || args.t_s < 0.0 {
        bail!("t_s must be finite and >= 0");
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build async runtime")?;
    let raw = runtime
        .block_on(render_program_frame(
            ViewProgramFrameArgs {
                t_s: args.t_s,
                detail: args.detail,
                format: args.format,
            },
            McpToolCtx {
                project_root: args.project_root,
            },
        ))
        .map_err(|message| anyhow::anyhow!(message))?;
    let mut value: Value =
        serde_json::from_str(&raw).context("program frame returned invalid JSON")?;
    if !args.include_base64
        && let Some(object) = value.as_object_mut()
    {
        object.remove("image_base64");
    }
    Ok(value)
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}

fn timeline_duration_s(timeline: &montage_proto::otio::Timeline) -> Option<f64> {
    // The root `tracks` Stack honors its own source_range first (mirrors
    // app_mcp/edl duration logic), then falls back to the longest child.
    let duration = stack_duration_s(&timeline.tracks);
    duration.is_finite().then_some(duration)
}

fn stack_child_duration_s(child: &StackChild) -> f64 {
    match child {
        StackChild::Track(track) => track_duration_s(track),
        StackChild::Stack(stack) => stack_duration_s(stack),
        StackChild::Clip(clip) => clip_duration_s(clip),
        StackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
    }
}

fn stack_duration_s(stack: &Stack) -> f64 {
    // A Stack composites its children in parallel, so its duration is the
    // longest child, not their sum. An explicit source_range overrides that.
    stack.source_range.as_ref().map_or_else(
        || {
            stack
                .children
                .iter()
                .map(stack_child_duration_s)
                .fold(0.0_f64, f64::max)
        },
        |range| range.duration.to_seconds(),
    )
}

fn track_duration_s(track: &Track) -> f64 {
    track.children.iter().map(track_child_duration_s).sum()
}

fn track_child_duration_s(child: &TrackChild) -> f64 {
    match child {
        TrackChild::Clip(clip) => clip_duration_s(clip),
        TrackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
        TrackChild::Stack(stack) => stack_duration_s(stack),
        TrackChild::Transition(_) => 0.0,
    }
}

fn clip_duration_s(clip: &Clip) -> f64 {
    clip.source_range
        .as_ref()
        .map_or(0.0, |range| range.duration.to_seconds())
}
