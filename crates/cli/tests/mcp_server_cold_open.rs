//! End-to-end test of the cold-open producer loop through the production
//! surface: spawn the real `montage-mcp-server`, and over stdio JSON-RPC
//! drive plan_cold_open → apply_edl → run_picture_gates. The
//! picture.cold_open gate must flip from fail to pass.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use montage_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Track,
    TrackChild, TrackKind,
};
use montage_proto::project::Project;

fn seed_flat_project(dir: &std::path::Path) {
    let mut project = Project::init(dir).expect("Project::init");
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

/// Call a tool and return its raw text content.
fn call_tool_text(
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
    id: u64,
    name: &str,
    arguments: serde_json::Value,
) -> String {
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    });
    writeln!(stdin, "{req}").expect("write tools/call");
    let resp = read_response(reader, id);
    resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("{name} must return text content: {resp}"))
        .to_string()
}

/// Call a tool whose body is JSON and parse it.
fn call_tool(
    stdin: &mut impl Write,
    reader: &mut impl BufRead,
    id: u64,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let text = call_tool_text(stdin, reader, id, name, arguments);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} body not JSON ({e}): {text}"))
}

#[test]
fn cold_open_producer_loop_over_stdio() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_flat_project(dir.path());

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

    // 1. Gates fail on the flat program.
    let gates = call_tool(
        &mut stdin,
        &mut reader,
        2,
        "run_picture_gates",
        serde_json::json!({"archetype": "informational"}),
    );
    assert_eq!(gates["passed"], false, "flat program must fail: {gates}");

    // 2. Plan the cold open: hook + 35 blitz beats.
    let moments: Vec<serde_json::Value> = (0..35)
        .map(|i| {
            let src = 30.0 + f64::from(i) * 12.0;
            serde_json::json!({
                "asset": "raw/episode.mp4",
                "start": src,
                "end": src + 2.4,
                "name": format!("beat-{i}"),
            })
        })
        .collect();
    let plan = call_tool(
        &mut stdin,
        &mut reader,
        3,
        "plan_cold_open",
        serde_json::json!({
            "hook": {"asset": "raw/episode.mp4", "start": 432.0, "end": 436.5,
                      "name": "hook-peak-line"},
            "moments": moments,
        }),
    );
    assert_eq!(plan["gate_projection"]["passed"], true, "{plan}");

    // 3. Apply the fragment. apply_edl returns a human-readable op log.
    let fragment = plan["edl_fragment"].as_str().expect("edl_fragment");
    let applied = call_tool_text(
        &mut stdin,
        &mut reader,
        4,
        "apply_edl",
        serde_json::json!({"edl": fragment, "reasoning": "cold-open producer e2e"}),
    );
    assert!(
        applied.contains("committed 36 op(s)"),
        "apply must commit all 36 inserts: {applied}"
    );

    // 4. The cold_open gate flips to pass.
    let gates = call_tool(
        &mut stdin,
        &mut reader,
        5,
        "run_picture_gates",
        serde_json::json!({"archetype": "informational"}),
    );
    let cold = gates["reports"]
        .as_array()
        .expect("reports")
        .iter()
        .find(|r| r["gate"] == "picture.cold_open")
        .expect("cold_open report");
    assert_eq!(cold["passed"], true, "cold_open must flip to pass: {gates}");

    drop(stdin);
    let _ = server.wait();
}
