# App Hosted MCP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a localhost MCP endpoint hosted by the Montage desktop app so external agents can inspect the live editor state and capture screenshots.

**Architecture:** Add a focused `app_mcp` Tauri backend module that owns the loopback JSON-RPC HTTP listener. Keep MCP request routing pure and testable, and keep desktop state collection behind a small snapshot function that reads `MontageState` plus `project.otio.json`.

**Tech Stack:** Rust, Tauri 2, Tokio `TcpListener`, serde JSON-RPC, existing `montage_proto::project::Project` project reader.

---

### Task 1: Pure MCP Routing

**Files:**
- Create: `apps/desktop/src-tauri/src/app_mcp.rs`

- [ ] **Step 1: Write tests for initialize, tools/list, resources/list, and unknown tools**

```rust
#[test]
fn routes_initialize_and_lists_tools() {
    let response = handle_json_rpc_value(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list"
    }), AppMcpSnapshot::default(), ScreenshotHandler::disabled());

    assert_eq!(response["jsonrpc"], "2.0");
    let tools = response["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|tool| tool["name"] == "get_editor_state"));
    assert!(tools.iter().any(|tool| tool["name"] == "take_screenshot"));
}
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run: `cargo test -p montage-desktop app_mcp::tests::routes_initialize_and_lists_tools`

Expected: FAIL because `app_mcp` does not exist yet.

- [ ] **Step 3: Implement the minimal JSON-RPC router and tool schemas**

Add MCP methods `initialize`, `notifications/initialized`, `tools/list`, `tools/call`, `resources/list`, and `resources/read`. Add tools `get_editor_state` and `take_screenshot`.

- [ ] **Step 4: Run the focused test and verify it passes**

Run: `cargo test -p montage-desktop app_mcp::tests::routes_initialize_and_lists_tools`

Expected: PASS.

### Task 2: Desktop State Snapshot

**Files:**
- Modify: `apps/desktop/src-tauri/src/app_mcp.rs`

- [ ] **Step 1: Write tests for project and timeline summarization**

Create a temporary project with `montage_proto::project::Project::init`, collect a snapshot from that path, and assert the project root, timeline name, track count, clip count, and duration fields are present.

- [ ] **Step 2: Run the focused test and verify it fails**

Run: `cargo test -p montage-desktop app_mcp::tests::summarizes_project_timeline`

Expected: FAIL until the snapshot reader is implemented.

- [ ] **Step 3: Implement snapshot collection**

Read `MontageState.project_root`, `MontageState.view_state`, and `Project::read(project_root)`. Return a clear `timeline_error` field instead of hiding malformed project state.

- [ ] **Step 4: Run the focused test and verify it passes**

Run: `cargo test -p montage-desktop app_mcp::tests::summarizes_project_timeline`

Expected: PASS.

### Task 3: Loopback HTTP Server

**Files:**
- Modify: `apps/desktop/src-tauri/src/app_mcp.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`

- [ ] **Step 1: Write tests for HTTP request parsing**

Use the pure request parser to verify POST `/mcp`, POST `/`, and CORS OPTIONS are accepted, while non-POST non-OPTIONS methods are rejected.

- [ ] **Step 2: Run the focused parser test and verify it fails**

Run: `cargo test -p montage-desktop app_mcp::tests::parses_http_json_rpc_request`

Expected: FAIL until the parser exists.

- [ ] **Step 3: Implement the localhost listener**

Bind `127.0.0.1:8420`, reject non-loopback peers, accept JSON-RPC POSTs, and return JSON responses with loopback-only CORS headers.

- [ ] **Step 4: Start the server in Tauri setup**

Call `app_mcp::start(app.handle().clone())` from `lib.rs` setup after state initialization.

- [ ] **Step 5: Run targeted desktop tests**

Run: `cargo test -p montage-desktop app_mcp`

Expected: PASS.

### Task 4: Verification

**Files:**
- Modify: formatting only where touched.

- [ ] **Step 1: Format check**

Run: `cargo fmt --all -- --check`

Expected: PASS.

- [ ] **Step 2: Compile the desktop crate**

Run: `cargo check -p montage-desktop`

Expected: PASS.
