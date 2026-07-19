//! Integration tests that exercise [`montage_mcp::Client`] against the
//! tiny `montage-mcp-test-server` binary in the same crate.
//!
//! Test fixtures (the `cfg`/`client_info` helpers and the path-resolution
//! for the test-server binary) live in `tests/common/mod.rs`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::time::Duration;

use montage_mcp::{Client, McpError, ServerConfig};

mod common;
use common::{cfg, client_info};

#[tokio::test]
async fn happy_path_initialize_list_call() {
    let mut c = Client::launch(cfg("normal")).unwrap();
    let info = c.initialize(client_info()).await.unwrap();
    assert_eq!(info.name, "montage-test");
    assert_eq!(info.version, "0.0.1");
    let tools = c.list_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"echo"));
    assert!(names.contains(&"index_asset"));
    let r = c
        .call_tool("echo", serde_json::json!({"text": "hi"}))
        .await
        .unwrap();
    assert!(!r.is_error);
    assert_eq!(r.single_text(), Some("hi"));
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn index_asset_returns_structured_sidecar() {
    let mut c = Client::launch(cfg("normal")).unwrap();
    c.initialize(client_info()).await.unwrap();
    let args = serde_json::json!({
        "asset_path": "/tmp/foo.wav",
        "asset_id": "raw/foo.wav",
        "asset_sha256": "deadbeef"
    });
    let r = c.call_tool("index_asset", args).await.unwrap();
    assert!(!r.is_error);
    let sc = r.structured_content.expect("structured_content present");
    assert_eq!(sc["indexer"], "test-indexer");
    assert_eq!(sc["asset_id"], "raw/foo.wav");
    assert_eq!(sc["data"]["echo"], "ok");
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn pre_initialize_call_is_protocol_violation() {
    let mut c = Client::launch(cfg("normal")).unwrap();
    let err = c.list_tools().await.unwrap_err();
    assert!(matches!(err, McpError::ProtocolViolation { .. }));
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn tool_error_surfaces_as_typed_error() {
    let mut c = Client::launch(cfg("tool_error")).unwrap();
    c.initialize(client_info()).await.unwrap();
    let err = c
        .call_tool("anything", serde_json::json!({}))
        .await
        .unwrap_err();
    match err {
        McpError::ToolError { message, .. } => assert!(message.contains("synthetic tool failure")),
        other => panic!("unexpected error variant: {other:?}"),
    }
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn unknown_tool_surfaces_as_tool_error() {
    let mut c = Client::launch(cfg("normal")).unwrap();
    c.initialize(client_info()).await.unwrap();
    let err = c
        .call_tool("nope", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, McpError::ToolError { .. }));
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn server_crash_mid_call_surfaces_as_server_crashed() {
    let mut c = Client::launch(cfg("crash_on_call")).unwrap();
    c.initialize(client_info()).await.unwrap();
    let err = c
        .call_tool("echo", serde_json::json!({"text": "x"}))
        .await
        .unwrap_err();
    match err {
        McpError::ServerCrashed { .. } => {}
        other => panic!("expected ServerCrashed, got {other:?}"),
    }
    let _ = c.shutdown().await;
}

#[tokio::test]
async fn malformed_initialize_response_is_protocol_violation_or_crash() {
    let mut c = Client::launch(cfg("malformed_init")).unwrap();
    let err = c
        .initialize_with_timeout(client_info(), Duration::from_millis(100))
        .await
        .unwrap_err();
    // The server emits a non-JSON line; the reader logs and drops it. The
    // initialize call then has no response and must time out OR be reported
    // as a crash (when the server's stdin EOFs and it exits). Either is
    // acceptable; both indicate the protocol failed.
    match err {
        McpError::Timeout { .. } | McpError::ServerCrashed { .. } => {}
        other => panic!("unexpected error variant: {other:?}"),
    }
    let _ = c.shutdown().await;
}

#[tokio::test]
async fn timeout_on_hung_server() {
    let mut c = Client::launch(cfg("hang")).unwrap();
    let err = c
        .initialize_with_timeout(client_info(), Duration::from_millis(100))
        .await
        .unwrap_err();
    assert!(matches!(err, McpError::Timeout { .. }));
    let _ = c.shutdown().await;
}

#[tokio::test]
async fn shutdown_is_idempotent_with_drop() {
    // Drop without calling shutdown — should not panic and should free
    // the child process via kill_on_drop.
    {
        let mut c = Client::launch(cfg("normal")).unwrap();
        c.initialize(client_info()).await.unwrap();
        // Drop here.
    }
    // No assertion; the success criterion is "no zombies, no panics".
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn spawn_failure_returns_typed_error() {
    let cfg = ServerConfig {
        name: "missing".into(),
        command: "/this/does/not/exist".into(),
        args: vec![],
        env: HashMap::new(),
        cwd: None,
    };
    let err = match Client::launch(cfg) {
        Ok(_) => panic!("expected spawn failure for nonexistent command"),
        Err(e) => e,
    };
    assert!(matches!(err, McpError::Spawn { .. }));
}

#[tokio::test]
async fn progress_notifications_are_routed_to_subscriber() {
    // Server emits 3 progress frames before responding; the subscriber
    // must receive all three.
    let mut c = Client::launch(cfg("progress_then_reply")).unwrap();
    c.initialize(client_info()).await.unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    let result = c
        .call_tool_with_progress("echo", serde_json::json!({"text": "hi"}), tx, None)
        .await
        .unwrap();
    assert!(!result.is_error);

    // Drain until the channel closes. `call_tool_with_progress` only returns
    // after it has waited for the full frame burst (keyed on the handler's
    // delivered>=total completion signal) and deregistered the subscriber
    // (dropping the sender), so the close is a deterministic end-of-stream
    // signal and all three frames are guaranteed to be queued by the time we
    // get here. The 10s cap only guards against a genuine hang.
    let mut events = Vec::new();
    let drain = async {
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
    };
    tokio::time::timeout(Duration::from_secs(10), drain)
        .await
        .expect("progress channel should close once the subscriber is deregistered");
    assert_eq!(
        events.len(),
        3,
        "expected 3 progress events, got {}: {events:?}",
        events.len()
    );
    // rmcp 1.8 dispatches each progress frame on an independent spawned task,
    // so delivery order into the channel is not guaranteed. Assert the frame
    // *contents* order-independently by sorting on the progress value.
    events.sort_by(|a, b| a.progress.total_cmp(&b.progress));
    assert_eq!(events[0].progress, 1.0);
    assert_eq!(events[1].progress, 2.0);
    assert_eq!(events[2].progress, 3.0);
    assert!(events.iter().all(|e| e.total == Some(3.0)));
    assert_eq!(events[1].message.as_deref(), Some("step 2 of 3"));
    let _ = c.shutdown().await;
}

#[tokio::test]
async fn cancellation_aborts_in_flight_tools_call() {
    // Server replies to initialize/tools-list normally but hangs on
    // tools/call. Without cancellation we'd hit DEFAULT_REQUEST_TIMEOUT
    // (30 minutes); with cancellation we get McpError::Cancelled fast.
    use tokio_util::sync::CancellationToken;
    let mut c = Client::launch(cfg("hang_on_call")).unwrap();
    c.initialize(client_info()).await.unwrap();

    let token = CancellationToken::new();
    let trigger = token.clone();
    // Fire the cancel after a brief delay so the request is genuinely
    // in-flight when the cancel hits.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.cancel();
    });

    let started = std::time::Instant::now();
    let err = c
        .call_tool_with_cancel("echo", serde_json::json!({"text": "hi"}), &token)
        .await
        .unwrap_err();
    let elapsed = started.elapsed();
    assert!(
        matches!(err, McpError::Cancelled { .. }),
        "expected Cancelled, got {err:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "cancellation should be fast; took {elapsed:?}"
    );
    let _ = c.shutdown().await;
}
