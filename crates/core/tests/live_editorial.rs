//! Live editorial workflow against the real Anthropic API. The model
//! sees the full 12-tool registry, gets a seeded project, and is asked
//! to perform a concrete editorial task. We assert that:
//!   1. The agent actually picks tools (didn't just hallucinate).
//!   2. The chosen `apply_edl` envelope commits to disk.
//!   3. OTIO validates after the commit.
//!
//! `#[ignore]` by default. Requires `ANTHROPIC_API_KEY`. Costs ~$0.01
//! per run (Sonnet, multi-turn).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use awidat_core::anthropic::{Client, ClientConfig, models};
use awidat_core::tools::{
    apply_edl::ApplyEdlTool, bash::BashTool, find_moment::FindMomentTool,
    inspect_clip::InspectClipTool, list_assets::ListAssetsTool, poll_render::PollRenderTool,
    read_index::ReadIndexTool, request_user_input::RequestUserInputTool,
    start_render::StartRenderTool, update_plan::UpdatePlanTool, view_frame::ViewFrameTool,
    view_timeline::ViewTimelineTool,
};
use awidat_core::{Session, SessionEvent, ToolRegistry};
use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
use awidat_proto::index::{AssetId, IndexerEntry, Manifest};
use awidat_proto::otio::{
    Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
    Track, TrackChild, TrackKind,
};
use awidat_proto::project::{Project, files};
use chrono::Utc;
use tokio_util::sync::CancellationToken;

const SYSTEM_PROMPT: &str = "\
You are awidat, a video editing agent. The user gives you a one-shot \
editorial task. Use the tools available — do not just describe what \
you would do. The 12 tools are: find_moment, view_timeline, \
inspect_clip, view_frame, list_assets, read_index, start_render, \
poll_render, update_plan, request_user_input, apply_edl, bash. \
\n\n\
The EDL format for apply_edl is freeform text:\n\
*** Begin EDL\n\
*** Trim Clip|Delete Clip|Insert BRoll|Move Clip|Insert Transition\n\
@@ anchor: transcript_snippet=\"...\" or clip_uuid=...\n\
+ key: value\n\
*** End EDL\n\
\n\
Be concise. Commit edits via apply_edl directly (no dry_run).\
";

fn seed_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut project = Project::init(dir.path()).expect("Project::init");

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

    let whisper_dir = dir
        .path()
        .join(files::INDEX_DIR)
        .join("whisper")
        .join("raw");
    std::fs::create_dir_all(&whisper_dir).unwrap();
    for (i, snip) in snippets.iter().enumerate() {
        let body = serde_json::json!({
            "indexer": "whisper", "indexer_version": "0.1.0", "schema_version": "1",
            "asset_id": format!("raw/ep-{i}.mp4"), "asset_sha256": "deadbeef",
            "produced_at": Utc::now().to_rfc3339(),
            "data": { "language": "en", "speakers": ["A","B"],
                "segments": [{"text": *snip, "start_s": 0.0, "end_s": 5.0, "speaker_id": "A"}],
            }
        });
        std::fs::write(
            whisper_dir.join(format!("ep-{i}.mp4.json")),
            serde_json::to_vec_pretty(&body).unwrap(),
        )
        .unwrap();
    }

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

fn registry_with_all_tools() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Arc::new(ApplyEdlTool));
    r.register(Arc::new(BashTool));
    r.register(Arc::new(FindMomentTool));
    r.register(Arc::new(InspectClipTool));
    r.register(Arc::new(ListAssetsTool));
    r.register(Arc::new(PollRenderTool));
    r.register(Arc::new(ReadIndexTool));
    r.register(Arc::new(RequestUserInputTool));
    r.register(Arc::new(StartRenderTool));
    r.register(Arc::new(UpdatePlanTool));
    r.register(Arc::new(ViewFrameTool));
    r.register(Arc::new(ViewTimelineTool));
    r
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live API; requires ANTHROPIC_API_KEY; ~$0.01/run on Sonnet"]
async fn live_agent_deletes_kitchen_clip_via_apply_edl() {
    let client =
        Client::from_env_or_keychain(ClientConfig::default()).expect("ANTHROPIC_API_KEY missing");
    let dir = seed_project();
    let project_root = dir.path().to_path_buf();

    let session = Arc::new(Session::new(
        client,
        registry_with_all_tools(),
        models::SONNET,
        Some(SYSTEM_PROMPT.into()),
        project_root.clone(),
    ));
    assert_eq!(session.tool_count(), 12, "all 12 tools registered");

    let mut events = session.subscribe();
    let cancel = CancellationToken::new();

    let prompt = "\
There are 3 clips in the timeline. One is about going to the kitchen \
to get coffee. Delete that one — it's filler. Use apply_edl with a \
transcript_snippet anchor.\
";

    let session_clone = session.clone();
    let cancel_clone = cancel.clone();
    let task = tokio::spawn(async move {
        session_clone
            .run_turn(prompt.to_string(), cancel_clone)
            .await
    });

    // Track which tools were called.
    let mut called: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut apply_edl_succeeded = false;
    let mut text_seen = String::new();
    let deadline = tokio::time::sleep(Duration::from_secs(120));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;
            () = &mut deadline => panic!("test timed out after 120s; tools called: {called:?}"),
            ev = events.recv() => {
                match ev {
                    Ok(SessionEvent::ToolCallStart { name, .. }) => {
                        called.insert(name);
                    }
                    Ok(SessionEvent::ToolResult { name, result: Ok(out), .. }) if name == "apply_edl" => {
                        if out.contains("applied") && !out.contains("dry-run") {
                            apply_edl_succeeded = true;
                        }
                    }
                    Ok(SessionEvent::TextDelta(t)) => text_seen.push_str(&t),
                    Ok(SessionEvent::TurnEnd) => break,
                    Ok(SessionEvent::Error(msg)) => panic!("session error: {msg}"),
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    let res = task.await.expect("join");
    assert!(res.is_ok(), "turn errored: {res:?}");

    println!("\n--- agent reply ---\n{text_seen}\n---");
    println!("tools called: {called:?}");

    assert!(
        called.contains("apply_edl"),
        "agent must have called apply_edl; called: {called:?}"
    );
    assert!(
        apply_edl_succeeded,
        "apply_edl must have committed (not dry-run); reply was: {text_seen}"
    );

    // Round-trip: the kitchen clip is gone.
    let after = Project::read(&project_root).expect("re-read");
    let StackChild::Track(t) = &after.timeline.tracks.children[0] else {
        panic!("expected V1 track")
    };
    assert_eq!(
        t.children.len(),
        2,
        "kitchen clip should be removed; remaining: {}",
        t.children.len()
    );
    let names: Vec<&str> = t
        .children
        .iter()
        .filter_map(|c| {
            if let TrackChild::Clip(cc) = c {
                Some(cc.name.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(
        !names.contains(&"clip-1"),
        "clip-1 (kitchen) should be gone; got {names:?}"
    );
    assert!(names.contains(&"clip-0"));
    assert!(names.contains(&"clip-2"));

    // Clean up via the tempdir Drop. Don't keep the assertions around.
    drop(dir);
    let _ = project_root;
}

fn _suppress_unused_path_import(_: &Path) {}
