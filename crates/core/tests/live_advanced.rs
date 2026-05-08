//! Two more ambitious live-agent tests against the real Anthropic API.
//!
//! 1. `live_render_chain_via_start_then_poll` — agent uses `start_render`
//!    + `poll_render` against real ffmpeg, assert output file lands.
//! 2. `live_multi_op_edl_envelope` — agent emits a single EDL with
//!    multiple ops (Trim + Delete on different clips); validates the
//!    per-op anchor resolution against post-prior-op state.
//!
//! Both `#[ignore]` by default; require `ANTHROPIC_API_KEY` and (test 1)
//! `ffmpeg` on PATH. ~$0.01–$0.02 per run on Sonnet.

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
You are awidat, a video editing agent. Use the tools — don't just \
describe what you would do. The 12 tools are: find_moment, \
view_timeline, inspect_clip, view_frame, list_assets, read_index, \
start_render, poll_render, update_plan, request_user_input, \
apply_edl, bash. \
\n\n\
EDL format (freeform, NOT JSON-escaped):\n\
*** Begin EDL\n\
*** Trim Clip|Delete Clip\n\
@@ anchor: transcript_snippet=\"...\" or clip_uuid=...\n\
+ key: value\n\
*** End EDL\n\
\n\
For renders: start_render returns immediately; poll_render with the \
job_id repeatedly until state is 'done' or 'failed'.\
";

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

// ============================================================
// Test 1: render chain
// ============================================================

/// Seeds a project + drops a real 2-second mp4 under raw/ via ffmpeg
/// lavfi. Skips (returns None) if ffmpeg isn't available.
fn seed_render_project() -> Option<tempfile::TempDir> {
    if awidat_render::ffmpeg_path().is_err() {
        return None;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    Project::init(dir.path()).expect("Project::init");

    let raw_dir = dir.path().join("raw");
    std::fs::create_dir_all(&raw_dir).unwrap();
    let asset = raw_dir.join("test-source.mp4");

    // Synthesize 2s of red 320x240 with sine audio.
    let bin = awidat_render::ffmpeg_path().unwrap();
    let status = std::process::Command::new(&bin)
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=320x240:d=2",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:d=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ])
        .arg(&asset)
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status()
        .expect("ffmpeg lavfi spawn");
    assert!(status.success(), "ffmpeg failed to seed test asset");
    Some(dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live API + real ffmpeg; ~$0.01/run on Sonnet"]
async fn live_render_chain_via_start_then_poll() {
    let Some(dir) = seed_render_project() else {
        eprintln!("ffmpeg not available; skipping live render chain test");
        return;
    };
    let project_root = dir.path().to_path_buf();

    let client =
        Client::from_env_or_keychain(ClientConfig::default()).expect("ANTHROPIC_API_KEY missing");
    let session = Arc::new(Session::new(
        client,
        registry_with_all_tools(),
        models::SONNET,
        Some(SYSTEM_PROMPT.into()),
        project_root.clone(),
    ));

    let mut events = session.subscribe();
    let cancel = CancellationToken::new();

    let prompt = "\
There's an asset at raw/test-source.mp4. Use start_render with \
scope='preview' to render a preview, then poll_render until it's \
done. Report the final state and the output path.\
";

    let session_clone = session.clone();
    let cancel_clone = cancel.clone();
    let task = tokio::spawn(async move {
        session_clone
            .run_turn(prompt.to_string(), cancel_clone)
            .await
    });

    let mut called: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut start_render_payload: Option<String> = None;
    let mut last_poll_payload: Option<String> = None;
    let mut text_seen = String::new();
    let deadline = tokio::time::sleep(Duration::from_secs(180));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;
            () = &mut deadline => panic!("test timed out; tools called: {called:?}"),
            ev = events.recv() => {
                match ev {
                    Ok(SessionEvent::ToolCallStart { name, .. }) => {
                        called.insert(name);
                    }
                    Ok(SessionEvent::ToolResult { name, result: Ok(out), .. }) => {
                        match name.as_str() {
                            "start_render" => start_render_payload = Some(out),
                            "poll_render" => last_poll_payload = Some(out),
                            _ => {}
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
        called.contains("start_render"),
        "agent must call start_render; called: {called:?}"
    );
    assert!(
        called.contains("poll_render"),
        "agent must call poll_render; called: {called:?}"
    );

    // The start_render result should have an output_path.
    let sr_body: serde_json::Value = serde_json::from_str(
        start_render_payload
            .as_deref()
            .expect("start_render output captured"),
    )
    .unwrap();
    let output_path = sr_body["output_path"].as_str().expect("output_path string");

    // The last poll should be terminal (done or failed).
    let poll_body: serde_json::Value = serde_json::from_str(
        last_poll_payload
            .as_deref()
            .expect("poll_render output captured"),
    )
    .unwrap();
    let final_state = poll_body["state"].as_str().expect("state string");
    assert!(
        matches!(final_state, "done" | "failed" | "cancelled"),
        "final poll state must be terminal; got {final_state}"
    );
    assert_eq!(
        final_state, "done",
        "render should succeed; got {final_state}"
    );

    // Output file landed on disk.
    assert!(
        Path::new(output_path).exists(),
        "render output should exist at {output_path}"
    );
    drop(dir);
}

// ============================================================
// Test 2: multi-op EDL envelope
// ============================================================

fn seed_four_clip_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut project = Project::init(dir.path()).expect("Project::init");

    let snippets = [
        "alpha clip about Stripe payments",
        "bravo clip about kitchen coffee",
        "charlie clip about the city skyline",
        "delta clip about the rainy weather",
    ];
    let mut track = Track::empty("V1", TrackKind::Video);
    for (i, snip) in snippets.iter().enumerate() {
        let mut clip = Clip::empty(format!("clip-{i}"));
        clip.media_reference =
            MediaReference::External(ExternalReference::new(format!("raw/ep-{i}.mp4")));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(10.0 * 24.0, 24.0),
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
            "data": { "language": "en",
                "segments": [{"text": *snip, "start_s": 0.0, "end_s": 10.0, "speaker_id": "A"}],
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live API; ~$0.01/run on Sonnet"]
async fn live_multi_op_edl_envelope() {
    let dir = seed_four_clip_project();
    let project_root = dir.path().to_path_buf();

    let client =
        Client::from_env_or_keychain(ClientConfig::default()).expect("ANTHROPIC_API_KEY missing");
    let session = Arc::new(Session::new(
        client,
        registry_with_all_tools(),
        models::SONNET,
        Some(SYSTEM_PROMPT.into()),
        project_root.clone(),
    ));

    let mut events = session.subscribe();
    let cancel = CancellationToken::new();

    // The crux: ask for two distinct edits in one EDL — one Trim + one
    // Delete, on different clips. This exercises the per-track cursor
    // (later anchors must still resolve after earlier ops landed in the
    // working clone).
    let prompt = "\
Make TWO edits in ONE apply_edl call (a single envelope with two ops):\
\n  1. Trim the alpha clip (about Stripe) to end at 5 seconds.\
\n  2. Delete the bravo clip (about kitchen coffee).\
\nUse transcript_snippet anchors. Commit (no dry_run).\
";

    let session_clone = session.clone();
    let cancel_clone = cancel.clone();
    let task = tokio::spawn(async move {
        session_clone
            .run_turn(prompt.to_string(), cancel_clone)
            .await
    });

    let mut called: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut apply_payload: Option<String> = None;
    let mut apply_args: Option<serde_json::Value> = None;
    let mut text_seen = String::new();
    let deadline = tokio::time::sleep(Duration::from_secs(120));
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            biased;
            () = &mut deadline => panic!("test timed out; tools called: {called:?}"),
            ev = events.recv() => {
                match ev {
                    Ok(SessionEvent::ToolCallStart { name, .. }) => {
                        called.insert(name);
                    }
                    Ok(SessionEvent::ToolCallArgs { name, args, .. }) if name == "apply_edl" => {
                        apply_args = Some(args);
                    }
                    Ok(SessionEvent::ToolResult { name, result: Ok(out), .. }) if name == "apply_edl" => {
                        apply_payload = Some(out);
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
    if let Some(args) = &apply_args {
        let edl_str = args
            .get("edl")
            .and_then(|v| v.as_str())
            .unwrap_or("<no edl field>");
        println!("--- raw EDL the model emitted ---\n{edl_str}\n---");
    }
    if let Some(p) = &apply_payload {
        println!("apply_edl result:\n{p}");
    }

    assert!(called.contains("apply_edl"), "agent must call apply_edl");
    let payload = apply_payload.expect("apply_edl result captured");
    assert!(
        payload.contains("committed 2 op"),
        "envelope must commit 2 ops; got: {payload}"
    );
    assert!(payload.contains("trimmed"));
    assert!(payload.contains("deleted"));

    // Round-trip on disk: alpha trimmed to 5s, bravo gone, charlie+delta
    // intact at 10s each.
    let after = Project::read(&project_root).expect("re-read");
    let StackChild::Track(t) = &after.timeline.tracks.children[0] else {
        panic!("expected V1 track")
    };
    assert_eq!(
        t.children.len(),
        3,
        "expected 3 clips after the multi-op edit"
    );
    let TrackChild::Clip(c0) = &t.children[0] else {
        panic!()
    };
    let TrackChild::Clip(c1) = &t.children[1] else {
        panic!()
    };
    let TrackChild::Clip(c2) = &t.children[2] else {
        panic!()
    };
    assert_eq!(c0.name, "clip-0", "alpha should still be first");
    assert_eq!(c1.name, "clip-2", "after delete, charlie shifts to index 1");
    assert_eq!(c2.name, "clip-3", "delta still at the end");
    let alpha_dur = c0.source_range.as_ref().unwrap().duration.to_seconds();
    assert!(
        (alpha_dur - 5.0).abs() < 0.05,
        "alpha trimmed to ~5s; got {alpha_dur}"
    );
    let charlie_dur = c1.source_range.as_ref().unwrap().duration.to_seconds();
    assert!(
        (charlie_dur - 10.0).abs() < 0.05,
        "charlie should be untouched at 10s; got {charlie_dur}"
    );
    drop(dir);
}
