//! Integration tests that exercise [`awidat_mcp::Client`] against the
//! tiny `awidat-mcp-test-server` binary in the same crate.
//!
//! Cargo builds the binary as a side effect of `cargo test`. We locate it
//! by walking up from the test binary's path to `target/<profile>/`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use awidat_mcp::{Client, ClientInfo, McpError, ServerConfig};

fn test_server_path() -> PathBuf {
    // Cargo runs integration tests from the per-crate target dir; the bins
    // built for the same crate live in $CARGO_BIN_EXE_<name> when declared
    // via `[[bin]]` at the crate root. We use that env (set by Cargo) for
    // robustness.
    let s = env!("CARGO_BIN_EXE_awidat-mcp-test-server");
    PathBuf::from(s)
}

fn cfg(mode: &str) -> ServerConfig {
    let mut env = HashMap::new();
    env.insert("AWIDAT_MCP_TEST_MODE".into(), mode.into());
    ServerConfig {
        name: format!("test-{mode}"),
        command: test_server_path().to_string_lossy().into_owned(),
        args: vec![],
        env,
        cwd: None,
    }
}

fn client_info() -> ClientInfo {
    ClientInfo {
        name: "awidat-test".into(),
        version: "0.0.1".into(),
    }
}

#[tokio::test]
async fn happy_path_initialize_list_call() {
    let mut c = Client::launch(cfg("normal")).await.unwrap();
    let info = c.initialize(client_info()).await.unwrap();
    assert_eq!(info.name, "awidat-test");
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
    let mut c = Client::launch(cfg("normal")).await.unwrap();
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
    let mut c = Client::launch(cfg("normal")).await.unwrap();
    let err = c.list_tools().await.unwrap_err();
    assert!(matches!(err, McpError::ProtocolViolation { .. }));
    c.shutdown().await.unwrap();
}

#[tokio::test]
async fn tool_error_surfaces_as_typed_error() {
    let mut c = Client::launch(cfg("tool_error")).await.unwrap();
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
    let mut c = Client::launch(cfg("normal")).await.unwrap();
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
    let mut c = Client::launch(cfg("crash_on_call")).await.unwrap();
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
    let mut c = Client::launch(cfg("malformed_init")).await.unwrap();
    let err = c.initialize(client_info()).await.unwrap_err();
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
    let mut c = Client::launch(cfg("hang")).await.unwrap();
    // Force a short timeout so the test runs fast.
    let res = tokio::time::timeout(Duration::from_secs(25), async {
        c.initialize(client_info()).await
    })
    .await;
    // The default initialize timeout is 20s. We give the outer harness 25s
    // as a safety margin. The expected outcome is McpError::Timeout.
    let err = res
        .expect("test harness timeout exceeded waiting for client-side timeout")
        .unwrap_err();
    assert!(matches!(err, McpError::Timeout { .. }));
    let _ = c.shutdown().await;
}

#[tokio::test]
async fn shutdown_is_idempotent_with_drop() {
    // Drop without calling shutdown — should not panic and should free
    // the child process via kill_on_drop.
    {
        let mut c = Client::launch(cfg("normal")).await.unwrap();
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
    let err = match Client::launch(cfg).await {
        Ok(_) => panic!("expected spawn failure for nonexistent command"),
        Err(e) => e,
    };
    assert!(matches!(err, McpError::Spawn { .. }));
}
