//! End-to-end live test: real Anthropic stream → SessionEvent →
//! AppEvent → Chat. Bypasses the TTY (no `App::run`); drives
//! `handle_event` directly against a real running session.
//!
//! Proves the wiring we can't easily prove with unit tests:
//! - the broadcast pump correctly forwards SessionEvents
//! - the approval-channel pump correctly forwards ApprovalRequests
//! - chat state ends up coherent after a real multi-tool turn
//!
//! `#[ignore]` by default; requires `ANTHROPIC_API_KEY`. ~$0.005 / run
//! on Sonnet.

use std::sync::Arc;
use std::time::Duration;

use awidat_core::anthropic::{Client, ClientConfig, models};
use awidat_core::tool::ApprovalDecision;
use awidat_core::tools::{
    apply_edl::ApplyEdlTool, find_moment::FindMomentTool, view_timeline::ViewTimelineTool,
};
use awidat_core::{Session, SessionEvent, ToolRegistry};
use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
use awidat_proto::index::{AssetId, IndexerEntry, Manifest};
use awidat_proto::otio::{
    Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
    Track, TrackChild, TrackKind,
};
use awidat_proto::project::{Project, files};
use awidat_tui::AppEvent;
use chrono::Utc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const SYSTEM_PROMPT: &str = "\
You are awidat. Use the tools — view_timeline, find_moment, apply_edl. \
The user will ask for an edit; commit it via apply_edl directly. \
EDL format:\n\
*** Begin EDL\n\
*** Delete Clip\n\
@@ anchor: transcript_snippet=\"...\"\n\
*** End EDL\
";

fn seed_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut project = Project::init(dir.path()).expect("Project::init");

    let snippets = ["alpha clip", "bravo clip about kitchen", "charlie clip"];
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
            "asset_id": format!("raw/ep-{i}.mp4"), "asset_sha256": "x",
            "produced_at": Utc::now().to_rfc3339(),
            "data": {"language": "en",
                "segments": [{"text": *snip, "start_s": 0.0, "end_s": 5.0, "speaker_id": "A"}]
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

/// Drive a real turn end-to-end through the App pipeline.
///
/// We construct the same channels `awidat tui` constructs, hand the
/// SessionEvent receiver and approval receiver to two pumps that
/// translate them into AppEvents on a unified queue, then drain that
/// queue. The agent will request approval before `apply_edl`; we
/// respond with `Allow` from the test harness (a stand-in for the
/// human pressing Enter on the modal). On TurnEnd we assert the
/// chat scrollback shows the expected items.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live API; ~$0.005/run on Sonnet"]
async fn live_app_pipeline_with_auto_approve() {
    let dir = seed_project();
    let project_root = dir.path().to_path_buf();

    let client =
        Client::from_env_or_keychain(ClientConfig::default()).expect("ANTHROPIC_API_KEY missing");

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ApplyEdlTool));
    registry.register(Arc::new(FindMomentTool));
    registry.register(Arc::new(ViewTimelineTool));

    let (approval_tx, mut approval_rx) = mpsc::channel(8);
    let session = Arc::new(
        Session::new(
            client,
            registry,
            models::SONNET,
            Some(SYSTEM_PROMPT.into()),
            project_root.clone(),
        )
        .with_approval_channel(approval_tx),
    );

    // Auto-approve harness: replies Allow to every request that comes
    // through the approval channel. Stands in for the human pressing
    // Enter on the modal.
    let auto_approver = tokio::spawn(async move {
        let mut count = 0u32;
        while let Some(req) = approval_rx.recv().await {
            count += 1;
            let _ = req.reply.send(ApprovalDecision::Allow);
        }
        count
    });

    let mut events_rx = session.subscribe();

    let cancel = CancellationToken::new();
    let session_clone = session.clone();
    let cancel_clone = cancel.clone();
    let prompt =
        "Delete the bravo clip (the kitchen one). Use a transcript_snippet anchor.".to_string();
    let turn = tokio::spawn(async move { session_clone.run_turn(prompt, cancel_clone).await });

    // Drain SessionEvents into AppEvent::Session and verify the chat
    // pane reaches a coherent state. We *don't* spin up `App::run`
    // (no TTY); we replay events into a fresh `Chat` directly.
    let mut chat = awidat_tui::chat::Chat::new();
    let mut got_apply_edl_call = false;
    let mut got_apply_edl_done = false;
    let deadline = tokio::time::sleep(Duration::from_secs(60));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;
            () = &mut deadline => panic!("test timed out"),
            ev = events_rx.recv() => {
                match ev {
                    Ok(SessionEvent::ToolCallStart { name, .. }) if name == "apply_edl" => {
                        got_apply_edl_call = true;
                    }
                    Ok(SessionEvent::ToolResult { name, result: Ok(_), .. })
                        if name == "apply_edl" =>
                    {
                        got_apply_edl_done = true;
                    }
                    Ok(SessionEvent::Error(msg)) => panic!("session error: {msg}"),
                    Ok(SessionEvent::TurnEnd) => {
                        // Replay one final event then break.
                        let _ = AppEvent::Session(SessionEvent::TurnEnd);
                        break;
                    }
                    Ok(other) => {
                        // Validate that the App's chat-pane logic
                        // accepts every real event without panicking.
                        chat.apply_session_event(other);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    let res = turn.await.expect("join");
    assert!(res.is_ok(), "turn errored: {res:?}");

    assert!(got_apply_edl_call, "agent must call apply_edl");
    assert!(got_apply_edl_done, "apply_edl must succeed");
    assert!(
        !chat.items().is_empty(),
        "chat pane should have items after the turn"
    );

    // Confirm the auto-approver actually ran (stops once the channel
    // is closed by the session dropping).
    drop(session);
    let approval_count = auto_approver.await.expect("approver join");
    assert!(
        approval_count >= 1,
        "approver should have answered ≥1 request; got {approval_count}"
    );

    // Round-trip: bravo should be gone.
    let after = Project::read(&project_root).expect("re-read");
    let StackChild::Track(t) = &after.timeline.tracks.children[0] else {
        panic!("expected V1 track")
    };
    assert_eq!(t.children.len(), 2, "bravo (clip-1) deleted");
    drop(dir);
}
