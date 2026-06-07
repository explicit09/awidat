//! End-to-end integration tests for the Phase 2 continuity engine.
//!
//! Exercises `assess_continuity` against synthesized projects with
//! seeded whisper / silence / motion / scenedetect sidecars. The
//! engine itself is unit-tested in `crates/core/src/continuity.rs`;
//! these tests verify the full path: tool-side timeline resolver →
//! sidecar I/O → engine → verdict.
//!
//! No ffmpeg required — sidecars are written as JSON fixtures.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;

use montage_core::tools::assess_continuity::AssessContinuityTool;
use montage_core::{ToolContext, ToolHandler, ToolInvocation};
use montage_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Track,
    TrackChild, TrackKind,
};
use montage_proto::project::{Project, files};
use tokio::sync::broadcast;

// --- fixture builders ---

fn ctx_at(root: &Path) -> ToolContext {
    let (tx, _) = broadcast::channel(8);
    ToolContext {
        project_root: root.to_path_buf(),
        events_tx: tx,
        user_input_tx: None,
        job_manager: montage_render::JobManager::new(),
        approval_tx: None,
        sandbox_mode: montage_core::tool::SandboxMode::Default,
        mcp_host: montage_core::mcp_host::McpHost::new(montage_mcp::ClientInfo {
            name: "test".into(),
            version: "0.0.0".into(),
        }),
        skills: Arc::new(montage_core::skills::SkillRegistry::default()),
        subagent_return: None,
    }
}

fn invoke(args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        call_id: "c1".into(),
        name: "assess_continuity".into(),
        args,
    }
}

/// FNV-1a hash of the absolute asset path. Mirrors the same algorithm
/// in `apps/desktop/.../media.rs` and `crates/core/src/continuity.rs`
/// so test sidecar paths line up with the engine's lookup.
fn stable_path_hash(path: &Path) -> u32 {
    let mut hash = 0x811c9dc5_u32;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

/// Seed a project with one clip on V1 + the whisper sidecar at
/// `index/whisper/<asset>.json`. Returns the project's TempDir.
fn seed_project_with_whisper(asset_id: &str, words: &[(&str, f64, f64)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut project = Project::init(dir.path()).expect("init");

    // Stub asset file at <root>/<asset_id>.
    let asset_path = dir.path().join(asset_id);
    std::fs::create_dir_all(asset_path.parent().unwrap()).unwrap();
    std::fs::write(&asset_path, b"stub").unwrap();

    // 1 clip on V1, source range [0, 10s], track-time [0, 10s].
    let mut clip = Clip::empty("clip-0".to_string());
    clip.media_reference = MediaReference::External(ExternalReference::new(asset_id.to_string()));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(10.0 * 24.0, 24.0),
    ));
    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.push(TrackChild::Clip(clip));
    project
        .timeline
        .tracks
        .children
        .push(StackChild::Track(track));
    let otio_path = dir.path().join(files::OTIO);
    std::fs::write(
        &otio_path,
        serde_json::to_vec_pretty(&project.timeline).unwrap(),
    )
    .unwrap();

    // Whisper sidecar at index/whisper/<asset>.json.
    let whisper_path = dir
        .path()
        .join("index")
        .join("whisper")
        .join(format!("{asset_id}.json"));
    std::fs::create_dir_all(whisper_path.parent().unwrap()).unwrap();
    let payload = serde_json::json!({
        "indexer": "whisper",
        "asset_id": asset_id,
        "data": {
            "words": words.iter().map(|(t, s, e)| {
                serde_json::json!({"text": t, "start_s": s, "end_s": e})
            }).collect::<Vec<_>>(),
        }
    });
    std::fs::write(&whisper_path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();

    dir
}

/// Append a multi-clip timeline so the rhythm rule has cut points
/// to evaluate against. Replaces the single-clip timeline written
/// by `seed_project_with_whisper`.
fn replace_timeline_with_three_clips(dir: &Path, asset_id: &str) {
    let mut project = Project::read(dir).unwrap();
    project.timeline.tracks.children.clear();

    let mut track = Track::empty("V1", TrackKind::Video);
    for (i, dur) in [(0, 4.0), (1, 4.0), (2, 4.0)].iter() {
        let mut c = Clip::empty(format!("clip-{i}"));
        c.media_reference = MediaReference::External(ExternalReference::new(asset_id.to_string()));
        c.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(dur * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(c));
    }
    project
        .timeline
        .tracks
        .children
        .push(StackChild::Track(track));
    let otio_path = dir.join(files::OTIO);
    std::fs::write(
        &otio_path,
        serde_json::to_vec_pretty(&project.timeline).unwrap(),
    )
    .unwrap();
}

/// Write a motion sidecar for an asset.
fn write_motion(dir: &Path, asset_id: &str, magnitudes: &[f32]) {
    let abs = dir.join(asset_id);
    let stem = abs.file_stem().unwrap().to_str().unwrap();
    let path = dir
        .join(".montage")
        .join("motion")
        .join(format!("{stem}-{:08x}.json", stable_path_hash(&abs)));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let payload = serde_json::json!({
        "samples_per_second": 1,
        "magnitudes": magnitudes,
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
}

// --- tests ---

#[tokio::test]
async fn dirty_when_cut_lands_mid_word() {
    // Word "hello" spans 1.0..1.5; cut at 1.2 lands inside it.
    let dir = seed_project_with_whisper("raw/ep.mp4", &[("hello", 1.0, 1.5), ("world", 1.6, 2.1)]);
    let out = AssessContinuityTool
        .handle(
            invoke(serde_json::json!({"at_s": 1.2, "kind": "trim_in"})),
            ctx_at(dir.path()),
        )
        .await
        .expect("handle");
    let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
    assert_eq!(v["verdict"], "dirty");
    let rules = v["rules"].as_array().unwrap();
    let mid_sent = rules
        .iter()
        .find(|r| r["id"] == "mid_sentence")
        .expect("mid_sentence rule present");
    assert_eq!(mid_sent["verdict"], "dirty");
    assert!(mid_sent["reason"].as_str().unwrap().contains("hello"));
}

#[tokio::test]
async fn clean_when_cut_at_word_boundary() {
    // Cut at 1.5 (end of "hello") is clean.
    let dir = seed_project_with_whisper("raw/ep.mp4", &[("hello", 1.0, 1.5), ("world", 1.6, 2.1)]);
    let out = AssessContinuityTool
        .handle(
            invoke(serde_json::json!({"at_s": 1.55, "kind": "trim_in"})),
            ctx_at(dir.path()),
        )
        .await
        .expect("handle");
    let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
    let mid_sent = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "mid_sentence")
        .unwrap();
    assert_eq!(mid_sent["verdict"], "clean");
    // Aggregate may be Risky/Abstain depending on other rules' inputs;
    // the test pins only the mid_sentence verdict for this scenario.
}

#[tokio::test]
async fn risky_with_three_cuts_in_five_seconds() {
    // Three contiguous 4s clips → cut points at 0, 4, 8.
    // Proposing a TrimIn at 6.0 → existing cuts at 4 (within 2s) and
    // 8 (within 2s) → 2 nearby cuts → rhythm rule risky.
    let dir = seed_project_with_whisper("raw/ep.mp4", &[]);
    replace_timeline_with_three_clips(dir.path(), "raw/ep.mp4");

    let out = AssessContinuityTool
        .handle(
            invoke(serde_json::json!({"at_s": 6.0, "kind": "trim_in"})),
            ctx_at(dir.path()),
        )
        .await
        .expect("handle");
    let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
    let rhythm = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "rhythm")
        .unwrap();
    assert_eq!(rhythm["verdict"], "risky");
    assert!(rhythm["reason"].as_str().unwrap().contains("frantic"));
}

#[tokio::test]
async fn dirty_when_high_motion_no_scene_change() {
    // Motion sidecar: high score at second 5; no scene-change index.
    // Cut at 5.5 should fire the mid_motion rule as dirty.
    let dir = seed_project_with_whisper("raw/ep.mp4", &[]);
    write_motion(dir.path(), "raw/ep.mp4", &[0.0, 0.1, 0.05, 0.1, 0.1, 0.8]);

    let out = AssessContinuityTool
        .handle(
            invoke(serde_json::json!({"at_s": 5.5, "kind": "trim_in"})),
            ctx_at(dir.path()),
        )
        .await
        .expect("handle");
    let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
    let motion = v["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "mid_motion")
        .unwrap();
    assert_eq!(motion["verdict"], "dirty");
    assert_eq!(v["verdict"], "dirty");
}

#[tokio::test]
async fn abstain_when_no_sidecars() {
    // No whisper, no motion, no silence — every rule abstains.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut project = Project::init(dir.path()).expect("init");
    let asset_path = dir.path().join("raw/ep.mp4");
    std::fs::create_dir_all(asset_path.parent().unwrap()).unwrap();
    std::fs::write(&asset_path, b"stub").unwrap();
    let mut clip = Clip::empty("clip-0".to_string());
    clip.media_reference =
        MediaReference::External(ExternalReference::new("raw/ep.mp4".to_string()));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(10.0 * 24.0, 24.0),
    ));
    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.push(TrackChild::Clip(clip));
    project
        .timeline
        .tracks
        .children
        .push(StackChild::Track(track));
    std::fs::write(
        dir.path().join(files::OTIO),
        serde_json::to_vec_pretty(&project.timeline).unwrap(),
    )
    .unwrap();

    let out = AssessContinuityTool
        .handle(
            invoke(serde_json::json!({"at_s": 5.0, "kind": "trim_in"})),
            ctx_at(dir.path()),
        )
        .await
        .expect("handle");
    let v: serde_json::Value = serde_json::from_str(&out.content).unwrap();
    // Aggregate: no sidecars at all → all rules abstain except
    // rhythm (which has nearby_cuts_s = empty Vec, defaulting to
    // Clean). So aggregate is Clean. The interesting assertion is
    // that the abstain rules surface so the user sees what's missing.
    let rules = v["rules"].as_array().unwrap();
    let abstain_count = rules.iter().filter(|r| r["verdict"] == "abstain").count();
    assert!(
        abstain_count >= 3,
        "expected 3+ abstain rules without sidecars, got {abstain_count}",
    );
}

#[tokio::test]
async fn errors_when_at_s_past_timeline_end() {
    let dir = seed_project_with_whisper("raw/ep.mp4", &[]);
    // Timeline is 10s long; at_s=99 is off the end.
    let err = AssessContinuityTool
        .handle(
            invoke(serde_json::json!({"at_s": 99.0, "kind": "trim_in"})),
            ctx_at(dir.path()),
        )
        .await
        .unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("doesn't land on any video clip"));
}

#[tokio::test]
async fn errors_on_unknown_kind() {
    let dir = seed_project_with_whisper("raw/ep.mp4", &[]);
    let err = AssessContinuityTool
        .handle(
            invoke(serde_json::json!({"at_s": 1.0, "kind": "lol_what"})),
            ctx_at(dir.path()),
        )
        .await
        .unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("unknown kind"));
}
