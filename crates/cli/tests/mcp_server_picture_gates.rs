//! End-to-end test of `run_picture_gates` through the production surface:
//! spawn the real `montage-mcp-server` binary, perform the MCP handshake
//! over stdio JSON-RPC, and call the tool against a seeded on-disk
//! project — the exact path codex takes at runtime.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use montage_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Track,
    TrackChild, TrackKind,
};
use montage_proto::project::Project;

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

/// Read JSON-RPC frames until the response with `id` arrives.
fn read_response(reader: &mut impl BufRead, id: u64) -> serde_json::Value {
    let mut line = String::new();
    for _ in 0..64 {
        line.clear();
        let read = reader.read_line(&mut line).expect("server stdout readable");
        assert!(
            read > 0,
            "server closed stdout before responding to id={id}"
        );
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if msg["id"] == id {
            return msg;
        }
    }
    panic!("no response for id={id} within 64 frames");
}

#[test]
fn run_picture_gates_end_to_end_over_stdio() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_project(dir.path(), 3, 5.0);

    let mut server = Command::new(env!("CARGO_BIN_EXE_montage-mcp-server"))
        .env("MONTAGE_PROJECT_ROOT", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("montage-mcp-server spawns");
    let mut stdin = server.stdin.take().expect("stdin piped");
    let stdout = server.stdout.take().expect("stdout piped");
    let mut reader = BufReader::new(stdout);

    // MCP handshake.
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2024-11-05","capabilities":{{}},"clientInfo":{{"name":"e2e-test","version":"0.0.0"}}}}}}"#
    )
    .expect("write initialize");
    let init = read_response(&mut reader, 1);
    assert!(
        init["result"]["serverInfo"].is_object(),
        "initialize must return serverInfo: {init}"
    );
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .expect("write initialized");

    // The production call: gates over the seeded timeline.
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"run_picture_gates","arguments":{{"archetype":"informational"}}}}}}"#
    )
    .expect("write tools/call");
    let call = read_response(&mut reader, 2);
    let text = call["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("tool must return text content: {call}"));
    let body: serde_json::Value = serde_json::from_str(text).expect("tool body is JSON");

    assert_eq!(body["profile"], "technologia");
    assert_eq!(body["cut_count"], 2);
    assert_eq!(body["passed"], false, "tiny flat timeline must fail gates");
    let reports = body["reports"].as_array().expect("reports array");
    assert!(
        reports
            .iter()
            .any(|r| r["gate"] == "picture.cold_open" && r["passed"] == false),
        "cold_open must fail on a flat opening: {body}"
    );
    assert!(
        reports
            .iter()
            .any(|r| r["gate"] == "picture.floor" && r["passed"] == true),
        "floor must pass at 8 cuts/min: {body}"
    );

    drop(stdin);
    let _ = server.wait();
}

/// The Finding-11 regression, through the full prod stack: a timeline whose
/// clip boundaries reproduce the real cut structure of our published
/// "Founder Journey" episode (measured 2026-07-06) must fail cold_open
/// (flat opening, peak buried mid-show) and floor (dead 5-min zone at
/// 35:45) when scored against the technologia profile.
#[test]
fn real_episode_cut_structure_reproduces_finding_11_over_stdio() {
    let cuts_file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../eval/fixtures/cuts/own_4h7bjBsW_Ag.cuts"
    );
    let content = std::fs::read_to_string(cuts_file).expect("study fixture readable");
    let mut cuts: Vec<f64> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().parse().expect("cut time parses"))
        .collect();
    cuts.sort_by(f64::total_cmp);
    const DURATION: f64 = 3274.0;

    // Clip durations = deltas between consecutive boundaries.
    let mut edges = vec![0.0];
    edges.extend(cuts.iter().copied());
    edges.push(DURATION);

    let dir = tempfile::tempdir().expect("tempdir");
    let mut project = Project::init(dir.path()).expect("Project::init");
    let mut track = Track::empty("V1", TrackKind::Video);
    for (i, pair) in edges.windows(2).enumerate() {
        let mut clip = Clip::empty(format!("clip-{i}"));
        clip.media_reference =
            MediaReference::External(ExternalReference::new("raw/founder.mp4".to_string()));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(pair[0] * 24.0, 24.0),
            RationalTime::new((pair[1] - pair[0]) * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(clip));
    }
    project
        .timeline
        .tracks
        .children
        .push(StackChild::Track(track));
    project.write(dir.path()).expect("project write");

    let mut server = Command::new(env!("CARGO_BIN_EXE_montage-mcp-server"))
        .env("MONTAGE_PROJECT_ROOT", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("montage-mcp-server spawns");
    let mut stdin = server.stdin.take().expect("stdin piped");
    let stdout = server.stdout.take().expect("stdout piped");
    let mut reader = BufReader::new(stdout);

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2024-11-05","capabilities":{{}},"clientInfo":{{"name":"e2e-test","version":"0.0.0"}}}}}}"#
    )
    .expect("write initialize");
    read_response(&mut reader, 1);
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .expect("write initialized");
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"run_picture_gates","arguments":{{"archetype":"informational","profile":"technologia"}}}}}}"#
    )
    .expect("write tools/call");
    let call = read_response(&mut reader, 2);
    let text = call["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("tool must return text content: {call}"));
    let body: serde_json::Value = serde_json::from_str(text).expect("tool body is JSON");

    assert_eq!(body["cut_count"], 172);
    assert_eq!(body["passed"], false);
    let reports = body["reports"].as_array().expect("reports array");
    let verdict = |g: &str| {
        reports
            .iter()
            .find(|r| r["gate"] == format!("picture.{g}"))
            .map(|r| r["passed"] == true)
            .unwrap_or_else(|| panic!("gate {g} missing: {body}"))
    };
    assert!(!verdict("cold_open"), "Finding 11: no cold open: {body}");
    assert!(!verdict("floor"), "Finding 11: dead zone at 35:45: {body}");
    // Body pacing (3.2 cuts/min) sits inside technologia's 3.0–9.0 band.
    assert!(verdict("body_pacing"), "body pacing is in band: {body}");

    drop(stdin);
    let _ = server.wait();
}
