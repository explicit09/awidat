//! Contract tests for the `render_preflight` tool.

use std::sync::Arc;

use awidat_core::tools::render_preflight::RenderPreflightTool;
use awidat_core::{ToolContext, ToolHandler, ToolInvocation};
use awidat_proto::otio::{
    Clip, Effect, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange,
    Timeline, Track, TrackChild, TrackKind,
};
use tokio::sync::broadcast;

fn ctx_at(root: &std::path::Path) -> ToolContext {
    let (tx, _) = broadcast::channel(8);
    ToolContext {
        project_root: root.to_path_buf(),
        events_tx: tx,
        user_input_tx: None,
        job_manager: awidat_render::JobManager::new(),
        approval_tx: None,
        sandbox_mode: awidat_core::tool::SandboxMode::Default,
        mcp_host: awidat_core::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
            name: "test".into(),
            version: "0.0.0".into(),
        }),
        skills: Arc::new(awidat_core::skills::SkillRegistry::default()),
        subagent_return: None,
    }
}

fn invocation(args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        call_id: "c1".into(),
        name: "render_preflight".into(),
        args,
    }
}

fn write_simple_project(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = awidat_proto::project::Project::init(root)?;
    let asset = "raw/a.mp4";
    std::fs::create_dir_all(root.join("raw"))?;
    std::fs::write(root.join(asset), b"stub")?;
    let mut clip = Clip::empty("clip-a");
    clip.media_reference = MediaReference::External(ExternalReference::new(asset));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));
    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.push(TrackChild::Clip(clip));
    let mut timeline = Timeline::empty("p");
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(track));
    timeline.tracks = stack;
    project.timeline = timeline;
    project.write(root)?;
    Ok(())
}

fn write_project_with_title_overlay(
    root: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = awidat_proto::project::Project::init(root)?;
    let asset = "raw/a.mp4";
    std::fs::create_dir_all(root.join("raw"))?;
    std::fs::write(root.join(asset), b"stub")?;
    let mut clip = Clip::empty("clip-a");
    clip.media_reference = MediaReference::External(ExternalReference::new(asset));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));
    let mut v1 = Track::empty("V1", TrackKind::Video);
    v1.children.push(TrackChild::Clip(clip));

    let mut title_clip = Clip::empty("title-1");
    title_clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));
    let mut title = Effect::new("awidat.title");
    title
        .metadata
        .insert("text".into(), serde_json::json!("hello"));
    title
        .metadata
        .insert("start_s".into(), serde_json::json!(0.0));
    title
        .metadata
        .insert("end_s".into(), serde_json::json!(2.0));
    title_clip.effects.push(title);
    let mut titles = Track::empty("Titles", TrackKind::Video);
    titles
        .metadata
        .insert("awidat_track_role".into(), serde_json::json!("titles"));
    titles.children.push(TrackChild::Clip(title_clip));

    let mut timeline = Timeline::empty("p");
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(v1));
    stack.children.push(StackChild::Track(titles));
    timeline.tracks = stack;
    project.timeline = timeline;
    project.write(root)?;
    Ok(())
}

fn write_project_with_caption_overlay(
    root: &std::path::Path,
    safe_area: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = awidat_proto::project::Project::init(root)?;
    let asset = "raw/a.mp4";
    std::fs::create_dir_all(root.join("raw"))?;
    std::fs::write(root.join(asset), b"stub")?;
    let mut clip = Clip::empty("clip-a");
    clip.media_reference = MediaReference::External(ExternalReference::new(asset));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));
    let mut v1 = Track::empty("V1", TrackKind::Video);
    v1.children.push(TrackChild::Clip(clip));

    let mut caption_clip = Clip::empty("caption-1");
    caption_clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(2.0 * 24.0, 24.0),
    ));
    let mut caption = Effect::new("awidat.title");
    caption
        .metadata
        .insert("role".into(), serde_json::json!("caption"));
    caption
        .metadata
        .insert("text".into(), serde_json::json!("Hello world"));
    if let Some(safe_area) = safe_area {
        caption
            .metadata
            .insert("safe_area".into(), serde_json::json!(safe_area));
    }
    caption.metadata.insert(
        "word_timings".into(),
        serde_json::json!([
            {"text": "Hello", "start_s": 0.0, "end_s": 0.4},
            {"text": "world", "start_s": 0.4, "end_s": 0.8}
        ]),
    );
    caption_clip.effects.push(caption);
    let mut titles = Track::empty("Titles", TrackKind::Video);
    titles
        .metadata
        .insert("awidat_track_role".into(), serde_json::json!("titles"));
    titles.children.push(TrackChild::Clip(caption_clip));

    let mut timeline = Timeline::empty("p");
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(v1));
    stack.children.push(StackChild::Track(titles));
    timeline.tracks = stack;
    project.timeline = timeline;
    project.write(root)?;
    Ok(())
}

#[tokio::test]
async fn render_preflight_reports_backend_and_capability_metadata_without_starting_job()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    write_simple_project(dir.path())?;

    let output = RenderPreflightTool
        .handle(
            invocation(serde_json::json!({"scope": "timeline"})),
            ctx_at(dir.path()),
        )
        .await?;
    let body: serde_json::Value = serde_json::from_str(&output.content)?;

    assert_eq!(body["scope"], "timeline");
    assert_eq!(body["backend"], "stream_export_remux");
    assert_eq!(
        body["backend_evidence"]["remux_eligibility_reason"],
        "single_plain_timeline_segment"
    );
    assert_eq!(
        body["backend_evidence"]["render_feature_id"],
        "stream_copy_remux"
    );
    assert_eq!(
        body["backend_evidence"]["render_feature_export_supported"],
        "supported"
    );
    assert_eq!(body["artifact_policy"], "no_render_job_started");
    assert_eq!(std::fs::read_dir(dir.path().join("renders"))?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn render_preflight_can_include_bounded_preview_cache_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    write_simple_project(dir.path())?;

    let output = RenderPreflightTool
        .handle(
            invocation(serde_json::json!({
                "scope": "timeline",
                "include_preview_cache": true,
                "preview_cache_max_tasks": 2
            })),
            ctx_at(dir.path()),
        )
        .await?;
    let body: serde_json::Value = serde_json::from_str(&output.content)?;

    assert_eq!(body["artifact_policy"], "no_render_job_started");
    assert_eq!(body["preview_cache"]["asset_count"], 1);
    assert_eq!(body["preview_cache"]["refresh_task_count"], 3);
    assert_eq!(body["preview_cache"]["selected_task_count"], 2);
    assert_eq!(body["preview_cache"]["skipped_task_count"], 1);
    assert_eq!(
        body["preview_cache"]["selected_refresh_work"]["asset_count"],
        1
    );
    assert_eq!(
        body["preview_cache"]["selected_refresh_tasks"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        body["preview_cache"]["refresh_execution"]["executor"],
        "desktop_preview_cache_refresh"
    );
    assert_eq!(
        body["preview_cache"]["refresh_execution"]["status"],
        "ready_to_start"
    );
    assert_eq!(
        body["preview_cache"]["refresh_execution"]["artifact_policy"],
        "no_render_job_started"
    );
    assert_eq!(
        body["preview_cache"]["refresh_execution"]["selected_task_count"],
        2
    );
    assert_eq!(
        body["preview_cache"]["refresh_execution"]["selected_task_ids"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    Ok(())
}

#[tokio::test]
async fn render_preflight_reports_stream_copy_blockers() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    write_project_with_title_overlay(dir.path())?;

    let output = RenderPreflightTool
        .handle(
            invocation(serde_json::json!({"scope": "timeline"})),
            ctx_at(dir.path()),
        )
        .await?;
    let body: serde_json::Value = serde_json::from_str(&output.content)?;

    assert_eq!(body["backend"], "timeline_ffmpeg_reencode");
    assert_eq!(
        body["backend_evidence"]["timeline_stream_copy_eligible"],
        "false"
    );
    assert_eq!(
        body["backend_evidence"]["timeline_stream_copy_blockers"],
        "title_overlays"
    );
    assert_eq!(
        body["backend_evidence"]["timeline_stream_copy_blocker_count"],
        "1"
    );
    Ok(())
}

#[tokio::test]
async fn render_preflight_surfaces_caption_layout_readiness()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    write_project_with_caption_overlay(dir.path(), Some("mobile"))?;

    let output = RenderPreflightTool
        .handle(
            invocation(serde_json::json!({"scope": "timeline"})),
            ctx_at(dir.path()),
        )
        .await?;
    let body: serde_json::Value = serde_json::from_str(&output.content)?;

    assert_eq!(
        body["caption_summary"]["selected_authority"],
        "caption_overlays"
    );
    assert_eq!(body["caption_summary"]["caption_overlay_count"], 1);
    assert_eq!(
        body["caption_summary"]["word_timed_caption_overlay_count"],
        1
    );
    assert_eq!(
        body["caption_summary"]["safe_area_caption_overlay_count"],
        1
    );
    assert_eq!(
        body["caption_summary"]["missing_safe_area_caption_overlay_count"],
        0
    );
    assert_eq!(
        body["caption_layout_readiness"]["status"],
        "ready_for_libass_layout"
    );
    assert_eq!(body["caption_layout_readiness"]["safe_area_ready"], true);
    assert_eq!(body["caption_layout_readiness"]["word_timing_ready"], true);
    assert_eq!(body["caption_layout_readiness"]["warning_count"], 1);
    assert!(
        body["caption_layout_readiness"]["warnings"]
            .as_array()
            .is_some_and(|warnings| warnings.iter().any(|warning| warning
                .as_str()
                .is_some_and(|text| text.contains("sidecar subtitle export may be empty"))))
    );
    Ok(())
}
