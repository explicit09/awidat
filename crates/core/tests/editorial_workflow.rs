//! End-to-end integration test for the editorial-tools batch.
//!
//! Hand-dispatches the five tools an agent typically chains for an
//! editing turn — find_moment, view_timeline, inspect_clip, read_index,
//! apply_edl — against a seeded project. Validates that the plumbing
//! (ToolContext, project I/O, sidecar reads, OTIO commit, manifest
//! consistency) holds together without going through the live model.
//!
//! Runs in normal `cargo test` (no `#[ignore]`); no network, no ffmpeg.
//! For a live-agent variant see `tests/live_agent.rs`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;

use awidat_core::tools::{
    apply_edl::ApplyEdlTool, find_moment::FindMomentTool, inspect_clip::InspectClipTool,
    read_index::ReadIndexTool, view_timeline::ViewTimelineTool,
};
use awidat_core::{ToolContext, ToolHandler, ToolInvocation, ToolRegistry};
use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
use awidat_proto::index::{AssetId, IndexerEntry, Manifest};
use awidat_proto::otio::{
    Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
    Track, TrackChild, TrackKind,
};
use awidat_proto::project::{Project, files};
use chrono::Utc;
use tokio::sync::broadcast;

/// Seeds a temp project with:
/// - 3 clips on V1 with transcript snippets
/// - A whisper sidecar with matching segments (so find_moment works)
/// - A manifest entry recording the indexer ran
fn seed_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut project = Project::init(dir.path()).expect("Project::init");

    // Build a 3-clip video track. Each clip gets an awidat anchor with
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
            awidat: Some(AwidatClipMetadata {
                anchor: Some(AwAnchor {
                    transcript_snippet: Some((*snip).to_string()),
                    ..AwAnchor::default()
                }),
                ..AwidatClipMetadata::default()
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

fn ctx_at(root: &Path) -> ToolContext {
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
        skills: std::sync::Arc::new(awidat_core::skills::SkillRegistry::default()),
        subagent_return: None,
    }
}

fn invoke(name: &'static str, args: serde_json::Value) -> ToolInvocation {
    ToolInvocation {
        call_id: format!("call-{name}"),
        name: name.into(),
        args,
    }
}

#[tokio::test]
async fn editorial_workflow_find_view_inspect_read_apply() {
    let dir = seed_project();

    // Step 1: Agent registers tools (mirrors what the REPL does).
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FindMomentTool));
    registry.register(Arc::new(ViewTimelineTool));
    registry.register(Arc::new(InspectClipTool));
    registry.register(Arc::new(ReadIndexTool));
    registry.register(Arc::new(ApplyEdlTool));
    assert_eq!(registry.len(), 5, "all five tools must register");

    // Step 2: find_moment locates the Stripe segment.
    let find = registry.get("find_moment").unwrap();
    let result = find
        .handle(
            invoke("find_moment", serde_json::json!({"query": "Stripe"})),
            ctx_at(dir.path()),
        )
        .await
        .expect("find_moment must succeed");
    let body: serde_json::Value =
        serde_json::from_str(&result.content).expect("find_moment returns JSON");
    let hits = body["results"].as_array().expect("results array");
    assert_eq!(hits.len(), 1, "exactly one Stripe match");
    assert_eq!(hits[0]["asset_id"], "raw/ep-0.mp4");
    assert!(hits[0]["snippet"].as_str().unwrap().contains("Stripe"));

    // Step 3: view_timeline shows all three clips.
    let view = registry.get("view_timeline").unwrap();
    let result = view
        .handle(
            invoke("view_timeline", serde_json::json!({})),
            ctx_at(dir.path()),
        )
        .await
        .expect("view_timeline must succeed");
    assert!(result.content.contains("clip \"clip-0\""));
    assert!(result.content.contains("clip \"clip-1\""));
    assert!(result.content.contains("clip \"clip-2\""));
    assert!(
        result.content.contains("total_duration=15.000s"),
        "3 clips * 5s each = 15s total"
    );

    // Step 4: inspect_clip fetches metadata for the matched asset.
    let inspect = registry.get("inspect_clip").unwrap();
    let result = inspect
        .handle(
            invoke(
                "inspect_clip",
                serde_json::json!({"asset_id": "raw/ep-0.mp4"}),
            ),
            ctx_at(dir.path()),
        )
        .await
        .expect("inspect_clip must succeed");
    let v: serde_json::Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(v["language"], "en");
    assert_eq!(v["segment_count"], 1);

    // Step 5: read_index pages the transcript channel.
    let read = registry.get("read_index").unwrap();
    let result = read
        .handle(
            invoke(
                "read_index",
                serde_json::json!({
                    "asset_id": "raw/ep-0.mp4",
                    "channel": "transcript"
                }),
            ),
            ctx_at(dir.path()),
        )
        .await
        .expect("read_index must succeed");
    let v: serde_json::Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(v["language"], "en");
    assert_eq!(v["total_segments"], 1);
    assert_eq!(v["segments"][0]["text"], snippet_for(0));

    // Step 6: apply_edl lifts the kitchen clip via transcript anchor,
    // then verifies a timing-preserving gap replaced it on disk.
    let apply = registry.get("apply_edl").unwrap();
    let edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: transcript_snippet=\"kitchen\"
*** End EDL
";
    let result = apply
        .handle(
            invoke("apply_edl", serde_json::json!({"edl": edl})),
            ctx_at(dir.path()),
        )
        .await
        .expect("apply_edl must succeed");
    assert!(result.content.contains("committed 1 op"));
    assert!(result.content.contains("deleted clip \"clip-1\""));

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

    // Step 7: re-running view_timeline reflects the new shape.
    let result = view
        .handle(
            invoke("view_timeline", serde_json::json!({})),
            ctx_at(dir.path()),
        )
        .await
        .expect("view_timeline post-edit");
    assert!(result.content.contains("clip \"clip-0\""));
    assert!(result.content.contains("clip \"clip-2\""));
    assert!(
        !result.content.contains("clip \"clip-1\""),
        "clip-1 must be gone after apply_edl"
    );
    assert!(result.content.contains("total_duration=15.000s"));
}

#[tokio::test]
async fn apply_edl_dry_run_chain_does_not_persist() {
    // Dry-run apply followed by a live view_timeline must show the
    // *unchanged* timeline.
    let dir = seed_project();
    let edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: transcript_snippet=\"Stripe\"
*** End EDL
";
    let apply_out = ApplyEdlTool
        .handle(
            invoke(
                "apply_edl",
                serde_json::json!({"edl": edl, "dry_run": true}),
            ),
            ctx_at(dir.path()),
        )
        .await
        .unwrap();
    assert!(apply_out.content.contains("DRY RUN"));

    let view_out = ViewTimelineTool
        .handle(
            invoke("view_timeline", serde_json::json!({})),
            ctx_at(dir.path()),
        )
        .await
        .unwrap();
    // All 3 clips still present.
    assert!(view_out.content.contains("clip \"clip-0\""));
    assert!(view_out.content.contains("clip \"clip-1\""));
    assert!(view_out.content.contains("clip \"clip-2\""));
}

#[tokio::test]
async fn apply_edl_anchor_miss_doesnt_corrupt_project() {
    let dir = seed_project();
    let edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: transcript_snippet=\"this clip does not exist\"
*** End EDL
";
    // First op fails → entire envelope rejected.
    let err = ApplyEdlTool
        .handle(
            invoke("apply_edl", serde_json::json!({"edl": edl})),
            ctx_at(dir.path()),
        )
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("Did you mean"));

    // Project unchanged.
    let after = Project::read(dir.path()).unwrap();
    let StackChild::Track(t) = &after.timeline.tracks.children[0] else {
        panic!()
    };
    assert_eq!(t.children.len(), 3, "no clip removed on anchor miss");
}

fn snippet_for(i: usize) -> &'static str {
    [
        "and that's when she said the thing about Stripe",
        "we went to the kitchen to get coffee",
        "the city skyline reminded me of New York",
    ][i]
}
