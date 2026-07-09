//! Integration tests for the `run_sound_gates` MCP tool: sound-pass gates
//! (loudness fail-closed without a render; J/L-cut coverage at whisper-
//! derived speaker turns) against seeded on-disk projects.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::run_sound_gates::{self, RunSoundGatesArgs};
use montage_proto::montage_meta::{MontageClipMetadata, SplitEditSpec};
use montage_proto::otio::{
    Clip, ClipMetadata, ExternalReference, Gap, MediaReference, RationalTime, StackChild,
    TimeRange, Track, TrackChild, TrackKind,
};
use montage_proto::project::Project;

fn clip(name: &str, asset: &str, start_s: f64, dur_s: f64, lead: Option<f64>) -> Clip {
    let mut c = Clip::empty(name.to_string());
    c.media_reference = MediaReference::External(ExternalReference::new(asset.to_string()));
    c.source_range = Some(TimeRange::new(
        RationalTime::new(start_s * 24.0, 24.0),
        RationalTime::new(dur_s * 24.0, 24.0),
    ));
    if let Some(v) = lead {
        c.metadata = ClipMetadata {
            montage: Some(MontageClipMetadata {
                split_edit: Some(SplitEditSpec {
                    audio_lead_s: Some(v),
                    ..SplitEditSpec::default()
                }),
                ..MontageClipMetadata::default()
            }),
            ..ClipMetadata::default()
        };
    }
    c
}

fn seed(dir: &std::path::Path, children: Vec<TrackChild>) {
    let mut project = Project::init(dir).expect("Project::init");
    let mut track = Track::empty("V1", TrackKind::Video);
    track.children = children;
    project
        .timeline
        .tracks
        .children
        .push(StackChild::Track(track));
    project.write(dir).expect("project write");
}

fn write_whisper(dir: &std::path::Path, asset: &str, words: serde_json::Value) {
    let path = dir.join("index/whisper").join(format!("{asset}.json"));
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(
        path,
        serde_json::json!({"data": {"words": words}}).to_string(),
    )
    .expect("write sidecar");
}

fn speaker_words(speaker: &str) -> serde_json::Value {
    // Words covering source 0..30s so any test clip range overlaps.
    serde_json::json!([
        {"text": "hello", "start_s": 0.0, "end_s": 15.0, "speaker": speaker},
        {"text": "world", "start_s": 15.0, "end_s": 30.0, "speaker": speaker},
    ])
}

fn ctx(dir: &std::path::Path) -> McpToolCtx {
    McpToolCtx {
        project_root: dir.to_path_buf(),
    }
}

fn run(dir: &std::path::Path, profile: Option<&str>) -> Result<serde_json::Value, String> {
    run_sound_gates::run(
        RunSoundGatesArgs {
            profile: profile.map(std::string::ToString::to_string),
        },
        ctx(dir),
    )
    .map(|s| serde_json::from_str(&s).expect("tool returns JSON"))
}

fn report<'a>(body: &'a serde_json::Value, gate: &str) -> &'a serde_json::Value {
    body["reports"]
        .as_array()
        .expect("reports")
        .iter()
        .find(|r| r["gate"] == gate)
        .unwrap_or_else(|| panic!("{gate} missing: {body}"))
}

#[test]
fn covered_speaker_turns_pass_the_coverage_gate() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed(
        dir.path(),
        vec![
            TrackChild::Clip(clip("c0", "raw/a.mp4", 0.0, 5.0, None)),
            TrackChild::Clip(clip("c1", "raw/b.mp4", 0.0, 5.0, Some(0.5))),
            TrackChild::Clip(clip("c2", "raw/a.mp4", 10.0, 5.0, Some(0.5))),
        ],
    );
    write_whisper(dir.path(), "raw/a.mp4", speaker_words("A"));
    write_whisper(dir.path(), "raw/b.mp4", speaker_words("B"));

    let body = run(dir.path(), None).expect("tool runs");
    assert_eq!(body["profile"], "technologia");
    assert_eq!(body["speaker_turns"]["total"], 2);
    assert_eq!(body["speaker_turns"]["covered"], 2);
    let cov = report(&body, "sound.split_edit_coverage");
    assert_eq!(cov["passed"], true, "{body}");
    assert_eq!(cov["measured"], 1.0);
    let loud = report(&body, "sound.loudness");
    assert_eq!(loud["passed"], false, "no render must fail closed: {body}");
    assert!(
        loud["detail"]
            .as_str()
            .expect("detail")
            .contains("renders/")
    );
    assert!(body["render_path"].is_null());
    assert!(body["loudness"].is_null());
    assert_eq!(body["passed"], false);
}

#[test]
fn uncovered_speaker_turns_fail_the_coverage_gate() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed(
        dir.path(),
        vec![
            TrackChild::Clip(clip("c0", "raw/a.mp4", 0.0, 5.0, None)),
            TrackChild::Clip(clip("c1", "raw/b.mp4", 0.0, 5.0, None)),
        ],
    );
    write_whisper(dir.path(), "raw/a.mp4", speaker_words("A"));
    write_whisper(dir.path(), "raw/b.mp4", speaker_words("B"));

    let body = run(dir.path(), None).expect("tool runs");
    let cov = report(&body, "sound.split_edit_coverage");
    assert_eq!(cov["passed"], false, "{body}");
    assert_eq!(cov["measured"], 0.0);
}

#[test]
fn no_whisper_sidecars_skip_the_coverage_gate() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed(
        dir.path(),
        vec![
            TrackChild::Clip(clip("c0", "raw/a.mp4", 0.0, 5.0, None)),
            TrackChild::Clip(clip("c1", "raw/b.mp4", 0.0, 5.0, None)),
        ],
    );
    let body = run(dir.path(), None).expect("tool runs");
    assert_eq!(body["speaker_turns"]["total"], 0);
    let cov = report(&body, "sound.split_edit_coverage");
    assert_eq!(cov["passed"], true, "{body}");
    assert!(cov["detail"].as_str().expect("detail").contains("skipped"));
}

#[test]
fn same_speaker_boundaries_are_not_turns() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed(
        dir.path(),
        vec![
            TrackChild::Clip(clip("c0", "raw/a.mp4", 0.0, 5.0, None)),
            TrackChild::Clip(clip("c1", "raw/b.mp4", 0.0, 5.0, None)),
        ],
    );
    write_whisper(dir.path(), "raw/a.mp4", speaker_words("A"));
    write_whisper(dir.path(), "raw/b.mp4", speaker_words("A"));
    let body = run(dir.path(), None).expect("tool runs");
    assert_eq!(body["speaker_turns"]["total"], 0);
}

#[test]
fn gap_breaks_adjacency() {
    let dir = tempfile::tempdir().expect("tempdir");
    let gap = Gap::of_duration(2.0, 24.0);
    seed(
        dir.path(),
        vec![
            TrackChild::Clip(clip("c0", "raw/a.mp4", 0.0, 5.0, None)),
            TrackChild::Gap(gap),
            TrackChild::Clip(clip("c1", "raw/b.mp4", 0.0, 5.0, None)),
        ],
    );
    write_whisper(dir.path(), "raw/a.mp4", speaker_words("A"));
    write_whisper(dir.path(), "raw/b.mp4", speaker_words("B"));
    let body = run(dir.path(), None).expect("tool runs");
    assert_eq!(body["speaker_turns"]["total"], 0);
}

#[test]
fn unknown_profile_is_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed(
        dir.path(),
        vec![TrackChild::Clip(clip("c0", "raw/a.mp4", 0.0, 5.0, None))],
    );
    let err = run(dir.path(), Some("not-a-profile")).expect_err("must error");
    assert!(err.contains("not-a-profile"));
}
