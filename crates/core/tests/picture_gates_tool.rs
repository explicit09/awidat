//! Integration tests for the `run_picture_gates` MCP tool: a seeded
//! on-disk project scored against a built-in house profile through the
//! same entry point `montage-mcp-server` dispatches to.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::run_picture_gates::{self, RunPictureGatesArgs};
use montage_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Track,
    TrackChild, TrackKind,
};
use montage_proto::project::Project;

/// Seed a project whose main video track holds `n` clips of
/// `clip_secs` each — i.e. `n - 1` interior cut boundaries.
fn seed_project(dir: &std::path::Path, n: usize, clip_secs: f64) {
    let mut project = Project::init(dir).expect("Project::init");
    let mut track = Track::empty("V1", TrackKind::Video);
    for i in 0..n {
        let mut clip = Clip::empty(format!("clip-{i}"));
        clip.media_reference =
            MediaReference::External(ExternalReference::new(format!("raw/ep-{i}.mp4")));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(clip_secs * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(clip));
    }
    project
        .timeline
        .tracks
        .children
        .push(StackChild::Track(track));
    project.write(dir).expect("project write");
}

fn ctx(dir: &std::path::Path) -> McpToolCtx {
    McpToolCtx {
        project_root: dir.to_path_buf(),
    }
}

#[test]
fn gates_report_on_a_seeded_timeline() {
    let dir = tempfile::tempdir().expect("tempdir");
    // 3 clips x 5s: cuts at 5s and 10s, program 15s. Dense cutting, but
    // far too short/flat to satisfy a technologia cold open.
    seed_project(dir.path(), 3, 5.0);

    let out = run_picture_gates::run(
        RunPictureGatesArgs {
            archetype: "informational".to_string(),
            profile: None,
        },
        ctx(dir.path()),
    )
    .expect("tool must succeed on a valid project");
    let body: serde_json::Value = serde_json::from_str(&out).expect("tool returns JSON");

    assert_eq!(body["profile"], "technologia");
    assert_eq!(body["archetype"], "informational");
    assert_eq!(body["cut_count"], 2);
    assert!((body["duration_secs"].as_f64().expect("duration") - 15.0).abs() < 1e-6);

    let reports = body["reports"].as_array().expect("reports array");
    let by_gate = |g: &str| {
        reports
            .iter()
            .find(|r| r["gate"] == format!("picture.{g}"))
            .unwrap_or_else(|| panic!("gate {g} missing"))
    };
    // 2 cuts / 15s = 8 cuts/min everywhere: floor clears comfortably.
    assert_eq!(by_gate("floor")["passed"], true);
    // 90s opening window holds only 2 cuts (1.3/min) — no blitz.
    assert_eq!(by_gate("cold_open")["passed"], false);
    assert_eq!(body["passed"], false);
}

#[test]
fn unknown_profile_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_project(dir.path(), 2, 5.0);
    let err = run_picture_gates::run(
        RunPictureGatesArgs {
            archetype: "informational".to_string(),
            profile: Some("not-a-profile".to_string()),
        },
        ctx(dir.path()),
    )
    .expect_err("unknown profile must error");
    assert!(err.contains("not-a-profile"));
}
