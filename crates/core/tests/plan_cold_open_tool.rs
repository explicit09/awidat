//! Integration tests for `plan_cold_open`: the deterministic cold-open
//! assembler. The agent supplies taste (hook + blitz moments); the tool
//! validates the plan against the house profile's cold-open spec and
//! emits an apply_edl-ready fragment.
//!
//! The money test drives the full producer loop through prod entry
//! points: gates fail → plan → apply_edl → gates' cold_open passes.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::apply_edl::{self, ApplyEdlArgs};
use montage_core::montage_mcp::tools::plan_cold_open::{self, ColdOpenSegment, PlanColdOpenArgs};
use montage_core::montage_mcp::tools::run_picture_gates::{self, RunPictureGatesArgs};
use montage_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Track,
    TrackChild, TrackKind,
};
use montage_proto::project::Project;

/// Founder-Journey shape: a flat 10-minute program cut once a minute —
/// competent body, no cold open.
fn seed_flat_project(dir: &std::path::Path) {
    let mut project = Project::init(dir).expect("Project::init");
    // apply_edl validates asset existence before inserting.
    std::fs::create_dir_all(dir.join("raw")).expect("raw dir");
    std::fs::write(dir.join("raw/episode.mp4"), b"").expect("asset file");
    let mut track = Track::empty("V1", TrackKind::Video);
    for i in 0..10 {
        let mut clip = Clip::empty(format!("body-{i}"));
        clip.media_reference =
            MediaReference::External(ExternalReference::new("raw/episode.mp4".to_string()));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(f64::from(i) * 60.0 * 24.0, 24.0),
            RationalTime::new(60.0 * 24.0, 24.0),
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

fn seg(start: f64, end: f64, name: &str) -> ColdOpenSegment {
    ColdOpenSegment {
        asset: "raw/episode.mp4".to_string(),
        start,
        end,
        name: Some(name.to_string()),
    }
}

/// 35 blitz beats of ~2.4s: ≈84s montage at ≈25 cuts/min — clears the
/// technologia cold-open spec (≥20 cuts/min over the first 90s).
fn blitz_moments() -> Vec<ColdOpenSegment> {
    (0..35)
        .map(|i| {
            let src = 30.0 + f64::from(i) * 12.0;
            seg(src, src + 2.4, &format!("beat-{i}"))
        })
        .collect()
}

#[test]
fn plan_apply_flips_the_cold_open_gate() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_flat_project(dir.path());

    // Pre-condition: the flat program fails cold_open.
    let before = run_picture_gates::run(
        RunPictureGatesArgs {
            archetype: "informational".to_string(),
            profile: None,
        },
        ctx(dir.path()),
    )
    .expect("gates run");
    let before: serde_json::Value = serde_json::from_str(&before).expect("json");
    assert!(
        before["reports"]
            .as_array()
            .expect("reports")
            .iter()
            .any(|r| r["gate"] == "picture.cold_open" && r["passed"] == false),
        "seeded project must fail cold_open: {before}"
    );

    // Plan: hook (the transplanted peak line) + blitz beats.
    let plan = plan_cold_open::run(
        PlanColdOpenArgs {
            hook: seg(432.0, 436.5, "hook-peak-line"),
            moments: blitz_moments(),
            profile: None,
            track: None,
        },
        ctx(dir.path()),
    )
    .expect("plan_cold_open runs");
    let plan: serde_json::Value = serde_json::from_str(&plan).expect("plan json");
    assert_eq!(
        plan["gate_projection"]["passed"], true,
        "projected verdict must clear the profile spec: {plan}"
    );
    let fragment = plan["edl_fragment"].as_str().expect("edl_fragment");
    assert!(fragment.starts_with("*** Begin EDL"), "{fragment}");
    // The hook is the first inserted segment.
    let hook_pos = fragment.find("hook-peak-line").expect("hook in fragment");
    let beat_pos = fragment.find("beat-0").expect("first beat in fragment");
    assert!(hook_pos < beat_pos, "hook must precede beats: {fragment}");

    // Apply through the same path the agent uses.
    apply_edl::run(
        ApplyEdlArgs {
            edl: fragment.to_string(),
            dry_run: false,
            reasoning: Some("cold-open producer test".to_string()),
        },
        ctx(dir.path()),
    )
    .expect("apply_edl succeeds");

    // Post-condition: cold_open flips to pass.
    let after = run_picture_gates::run(
        RunPictureGatesArgs {
            archetype: "informational".to_string(),
            profile: None,
        },
        ctx(dir.path()),
    )
    .expect("gates run");
    let after: serde_json::Value = serde_json::from_str(&after).expect("json");
    assert!(
        after["reports"]
            .as_array()
            .expect("reports")
            .iter()
            .any(|r| r["gate"] == "picture.cold_open" && r["passed"] == true),
        "cold_open must pass after applying the plan: {after}"
    );
}

#[test]
fn sparse_plan_reports_a_failing_projection_with_warnings() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_flat_project(dir.path());

    // 4 long moments: nowhere near blitz pacing.
    let moments: Vec<ColdOpenSegment> = (0..4)
        .map(|i| {
            seg(
                f64::from(i) * 30.0,
                f64::from(i) * 30.0 + 15.0,
                &format!("slow-{i}"),
            )
        })
        .collect();
    let plan = plan_cold_open::run(
        PlanColdOpenArgs {
            hook: seg(400.0, 404.0, "hook"),
            moments,
            profile: None,
            track: None,
        },
        ctx(dir.path()),
    )
    .expect("plan still returns — agent decides");
    let plan: serde_json::Value = serde_json::from_str(&plan).expect("plan json");
    assert_eq!(plan["gate_projection"]["passed"], false);
    assert!(
        !plan["warnings"].as_array().expect("warnings").is_empty(),
        "sparse plan must carry warnings: {plan}"
    );
}

#[test]
fn empty_moments_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_flat_project(dir.path());
    let err = plan_cold_open::run(
        PlanColdOpenArgs {
            hook: seg(0.0, 4.0, "hook"),
            moments: vec![],
            profile: None,
            track: None,
        },
        ctx(dir.path()),
    )
    .expect_err("empty moments must error");
    assert!(err.contains("moments"));
}
