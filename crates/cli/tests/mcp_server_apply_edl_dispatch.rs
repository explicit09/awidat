//! Dispatcher smoke test for `apply_edl` through the production MCP
//! server surface: spawn the real `montage-mcp-server` binary, do the
//! MCP handshake over stdio JSON-RPC, and confirm the tool is both
//! listed and dispatchable — including that malformed arguments come
//! back as a normal JSON-RPC/MCP error rather than crashing the
//! server. Follows the same spawn/handshake pattern as
//! `mcp_server_cold_open.rs` / `mcp_server_picture_gates.rs`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use montage_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Track,
    TrackChild, TrackKind,
};
use montage_proto::project::Project;

fn seed_project(dir: &std::path::Path) {
    let mut project = Project::init(dir).expect("Project::init");
    let mut track = Track::empty("V1", TrackKind::Video);
    let mut clip = Clip::empty("clip-0".to_string());
    clip.media_reference =
        MediaReference::External(ExternalReference::new("raw/ep-0.mp4".to_string()));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, 24.0),
        RationalTime::new(5.0 * 24.0, 24.0),
    ));
    track.children.push(TrackChild::Clip(clip));
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

#[test]
fn apply_edl_is_registered_and_wrong_args_return_mcp_error_not_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_project(dir.path());

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

    // 1. apply_edl must be listed among the server's tools.
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{}}}}"#
    )
    .expect("write tools/list");
    let list = read_response(&mut reader, 2);
    let tools = list["result"]["tools"]
        .as_array()
        .expect("tools/list returns an array");
    assert!(
        tools.iter().any(|t| t["name"] == "apply_edl"),
        "apply_edl must be registered on the server: {list}"
    );

    // 2. Missing required `edl` field must come back as an MCP tool
    // error (isError / JSON-RPC error), never a crash — the server
    // must still be alive to answer the next request afterward.
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{{"name":"apply_edl","arguments":{{}}}}}}"#
    )
    .expect("write malformed tools/call");
    let bad_call = read_response(&mut reader, 3);
    let is_protocol_error = bad_call.get("error").is_some();
    let is_tool_error = bad_call["result"]["isError"] == true;
    assert!(
        is_protocol_error || is_tool_error,
        "missing required `edl` arg must surface as an error, not succeed: {bad_call}"
    );

    // 3. Server is still responsive: a valid, real apply_edl call
    // after the bad one succeeds normally — proves the wrong-args
    // call didn't panic or wedge the process.
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"apply_edl","arguments":{{"edl":"*** Begin EDL\n*** Set Volume\n@@ anchor: clip_uuid=clip-0\n+ value: 0.5\n*** End EDL\n"}}}}}}"#
    )
    .expect("write valid tools/call");
    let good_call = read_response(&mut reader, 4);
    let text = good_call["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("apply_edl must return text content after recovering: {good_call}")
        });
    assert!(
        text.contains("committed 1 op(s)"),
        "server must still process real calls after the malformed one: {text}"
    );

    drop(stdin);
    let _ = server.wait();
}
