//! End-to-end integration test for the editorial-tools batch.
//!
//! Hand-dispatches the five tools an agent typically chains for an
//! editing turn — find_moment, view_timeline, inspect_clip, read_index,
//! apply_edl — against a seeded project, calling straight into the
//! production `montage_mcp::tools::*::run` functions the real
//! `montage-mcp-server` binary dispatches into (see
//! `docs/risk-register-2026-07-15.md` for why the legacy
//! `crate::tools::ToolHandler` tree this used to exercise is gone).
//!
//! Runs in normal `cargo test`; no network, no ffmpeg.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;

use chrono::Utc;
use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::apply_edl::{self, ApplyEdlArgs};
use montage_core::montage_mcp::tools::find_moment::{self, FindMomentArgs};
use montage_core::montage_mcp::tools::inspect_clip::{self, InspectClipArgs};
use montage_core::montage_mcp::tools::read_index::{self, ReadIndexArgs};
use montage_core::montage_mcp::tools::view_timeline::{self, ViewTimelineArgs};
use montage_proto::index::{AssetId, IndexerEntry, Manifest};
use montage_proto::montage_meta::{Anchor as AwAnchor, MontageClipMetadata};
use montage_proto::otio::{
    Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
    Track, TrackChild, TrackKind,
};
use montage_proto::project::{Project, files};

/// Seeds a temp project with:
/// - 3 clips on V1 with transcript snippets
/// - A whisper sidecar with matching segments (so find_moment works)
/// - A manifest entry recording the indexer ran
fn seed_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut project = Project::init(dir.path()).expect("Project::init");

    // Build a 3-clip video track. Each clip gets an montage anchor with
    // a transcript snippet so find_moment + apply_edl can resolve.
    let snippets = [
        "and that's when she said the thing about Stripe",
        "we went to the kitchen to get coffee",
        "the city skyline reminded me of New York",
    ];
    let mut track = Track::empty("V1", TrackKind::Video);
    for (i, snip) in snippets.iter().enumerate() {
        let mut clip = Clip::empty(format!("clip-{i}"));
        clip.media_reference =
            MediaReference::External(ExternalReference::new(format!("raw/ep-{i}.mp4")));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));
        clip.metadata = ClipMetadata {
            montage: Some(MontageClipMetadata {
                anchor: Some(AwAnchor {
                    transcript_snippet: Some((*snip).to_string()),
                    ..AwAnchor::default()
                }),
                ..MontageClipMetadata::default()
            }),
            ..ClipMetadata::default()
        };
        track.children.push(TrackChild::Clip(clip));
    }
    project
        .timeline
        .tracks
        .children
        .push(StackChild::Track(track));
    project.write(dir.path()).expect("project write");

    // Seed a whisper sidecar matching the snippets so find_moment finds
    // them. The path follows INDEX_SCHEMA: index/whisper/<asset>.json.
    let whisper_dir = dir
        .path()
        .join(files::INDEX_DIR)
        .join("whisper")
        .join("raw");
    std::fs::create_dir_all(&whisper_dir).unwrap();
    for (i, snip) in snippets.iter().enumerate() {
        let body = serde_json::json!({
            "indexer": "whisper",
            "indexer_version": "0.1.0",
            "schema_version": "1",
            "asset_id": format!("raw/ep-{i}.mp4"),
            "asset_sha256": "deadbeef",
            "produced_at": Utc::now().to_rfc3339(),
            "data": {
                "language": "en",
                "speakers": ["A", "B"],
                "segments": [
                    {"text": *snip, "start_s": 0.0, "end_s": 5.0, "speaker_id": "A"},
                ],
            }
        });
        std::fs::write(
            whisper_dir.join(format!("ep-{i}.mp4.json")),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();
    }

    // Update manifest to register whisper.
    let manifest = Manifest {
        version: "0.1".into(),
        indexers: vec![IndexerEntry {
            name: "whisper".into(),
            version: "0.1.0".into(),
            schema_version: "1".into(),
            assets: snippets
                .iter()
                .enumerate()
                .map(|(i, _)| AssetId::new(format!("raw/ep-{i}.mp4")))
                .collect(),
            last_run: Utc::now(),
        }],
    };
    std::fs::write(
        dir.path()
            .join(files::INDEX_DIR)
            .join(files::INDEX_MANIFEST),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    dir
}

fn ctx_at(root: &Path) -> McpToolCtx {
    McpToolCtx {
        project_root: root.to_path_buf(),
    }
}

fn apply_edl_args(edl: &str, dry_run: bool) -> ApplyEdlArgs {
    ApplyEdlArgs {
        edl: edl.to_string(),
        dry_run,
        reasoning: None,
    }
}

#[test]
fn editorial_workflow_find_view_inspect_read_apply() {
    let dir = seed_project();

    // Step 1: find_moment locates the Stripe segment.
    let result = find_moment::run(
        FindMomentArgs {
            query: "Stripe".to_string(),
            asset_id: None,
            limit: None,
        },
        ctx_at(dir.path()),
    )
    .expect("find_moment must succeed");
    let body: serde_json::Value = serde_json::from_str(&result).expect("find_moment returns JSON");
    let hits = body["results"].as_array().expect("results array");
    assert_eq!(hits.len(), 1, "exactly one Stripe match");
    assert_eq!(hits[0]["asset_id"], "raw/ep-0.mp4");
    assert!(hits[0]["snippet"].as_str().unwrap().contains("Stripe"));

    // Step 2: view_timeline shows all three clips.
    let result = view_timeline::run(
        ViewTimelineArgs {
            start_s: None,
            end_s: None,
            lines: None,
        },
        ctx_at(dir.path()),
    )
    .expect("view_timeline must succeed");
    assert!(result.contains("clip \"clip-0\""));
    assert!(result.contains("clip \"clip-1\""));
    assert!(result.contains("clip \"clip-2\""));
    assert!(
        result.contains("total_duration=15.000s"),
        "3 clips * 5s each = 15s total"
    );

    // Step 3: inspect_clip fetches metadata for the matched asset.
    let result = inspect_clip::run(
        InspectClipArgs {
            asset_id: "raw/ep-0.mp4".to_string(),
        },
        ctx_at(dir.path()),
    )
    .expect("inspect_clip must succeed");
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["language"], "en");
    assert_eq!(v["segment_count"], 1);

    // Step 4: read_index pages the transcript channel.
    let result = read_index::run(
        ReadIndexArgs {
            asset_id: "raw/ep-0.mp4".to_string(),
            channel: "transcript".to_string(),
            offset: None,
            limit: None,
        },
        ctx_at(dir.path()),
    )
    .expect("read_index must succeed");
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["language"], "en");
    assert_eq!(v["total_segments"], 1);
    assert_eq!(v["segments"][0]["text"], snippet_for(0));

    // Step 5: apply_edl lifts the kitchen clip via transcript anchor,
    // then verifies a timing-preserving gap replaced it on disk.
    let edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: transcript_snippet=\"kitchen\"
*** End EDL
";
    let result = apply_edl::run(apply_edl_args(edl, false), ctx_at(dir.path()))
        .expect("apply_edl must succeed");
    assert!(result.contains("committed 1 op"));
    assert!(result.contains("deleted clip \"clip-1\""));

    // Round-trip: re-read the project and confirm the clip is gone.
    let after = Project::read(dir.path()).expect("re-read");
    let StackChild::Track(t) = &after.timeline.tracks.children[0] else {
        panic!("expected track at index 0");
    };
    assert_eq!(t.children.len(), 3, "kitchen clip replaced by gap");
    let TrackChild::Clip(c0) = &t.children[0] else {
        panic!()
    };
    let TrackChild::Gap(gap) = &t.children[1] else {
        panic!("clip-1 slot should be a timing gap")
    };
    let TrackChild::Clip(c1) = &t.children[2] else {
        panic!()
    };
    assert_eq!(c0.name, "clip-0");
    assert!((gap.source_range.duration.to_seconds() - 5.0).abs() < 1e-9);
    assert_eq!(c1.name, "clip-2", "clip-1 (kitchen) was deleted");

    // Step 6: re-running view_timeline reflects the new shape.
    let result = view_timeline::run(
        ViewTimelineArgs {
            start_s: None,
            end_s: None,
            lines: None,
        },
        ctx_at(dir.path()),
    )
    .expect("view_timeline post-edit");
    assert!(result.contains("clip \"clip-0\""));
    assert!(result.contains("clip \"clip-2\""));
    assert!(
        !result.contains("clip \"clip-1\""),
        "clip-1 must be gone after apply_edl"
    );
    assert!(result.contains("total_duration=15.000s"));
}

#[test]
fn apply_edl_dry_run_chain_does_not_persist() {
    // Dry-run apply followed by a live view_timeline must show the
    // *unchanged* timeline.
    let dir = seed_project();
    let edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: transcript_snippet=\"Stripe\"
*** End EDL
";
    let apply_out = apply_edl::run(apply_edl_args(edl, true), ctx_at(dir.path())).unwrap();
    assert!(apply_out.contains("DRY RUN"));

    let view_out = view_timeline::run(
        ViewTimelineArgs {
            start_s: None,
            end_s: None,
            lines: None,
        },
        ctx_at(dir.path()),
    )
    .unwrap();
    // All 3 clips still present.
    assert!(view_out.contains("clip \"clip-0\""));
    assert!(view_out.contains("clip \"clip-1\""));
    assert!(view_out.contains("clip \"clip-2\""));
}

#[test]
fn apply_edl_anchor_miss_doesnt_corrupt_project() {
    let dir = seed_project();
    let edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: transcript_snippet=\"this clip does not exist\"
*** End EDL
";
    // First op fails → entire envelope rejected.
    let err = apply_edl::run(apply_edl_args(edl, false), ctx_at(dir.path())).unwrap_err();
    assert!(err.contains("Did you mean"));

    // Project unchanged.
    let after = Project::read(dir.path()).unwrap();
    let StackChild::Track(t) = &after.timeline.tracks.children[0] else {
        panic!()
    };
    assert_eq!(t.children.len(), 3, "no clip removed on anchor miss");
}

#[test]
fn apply_edl_later_op_failure_rolls_back_prior_successful_ops() {
    let dir = seed_project();
    let before = std::fs::read_to_string(dir.path().join(files::OTIO)).unwrap();
    let edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: transcript_snippet=\"we went to the kitchen to get coffee\"
*** Delete Clip
@@ anchor: transcript_snippet=\"this later clip does not exist\"
*** End EDL
";

    let err = apply_edl::run(apply_edl_args(edl, false), ctx_at(dir.path())).unwrap_err();
    assert!(err.contains("Did you mean"));

    let after = std::fs::read_to_string(dir.path().join(files::OTIO)).unwrap();
    assert_eq!(
        after, before,
        "a failure after an earlier successful op must leave project.otio.json unchanged"
    );

    let after_project = Project::read(dir.path()).unwrap();
    let StackChild::Track(track) = &after_project.timeline.tracks.children[0] else {
        panic!()
    };
    assert!(matches!(&track.children[1], TrackChild::Clip(clip) if clip.name == "clip-1"));
}

fn snippet_for(i: usize) -> &'static str {
    [
        "and that's when she said the thing about Stripe",
        "we went to the kitchen to get coffee",
        "the city skyline reminded me of New York",
    ][i]
}
