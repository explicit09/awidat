//! Tiny MCP-over-stdio test server used by `client_against_test_server.rs`.
//!
//! We hand-roll the JSON-RPC framing (newline-delimited) instead of pulling
//! in an MCP server library because (a) we don't want a dev-dep, and (b)
//! exercising the wire format from a deliberately-minimal implementation
//! catches client bugs the official server's politeness might paper over.
//!
//! Behavior is tweakable through env vars:
//!
//! - `MONTAGE_MCP_TEST_MODE=normal` (default) — implements `initialize`,
//!   `tools/list`, and a `tools/call` for `echo` and `index_asset`.
//! - `MONTAGE_MCP_TEST_MODE=crash_on_call` — exits with code 7 the first
//!   time `tools/call` is invoked.
//! - `MONTAGE_MCP_TEST_MODE=malformed_init` — replies to `initialize` with
//!   text that isn't a JSON-RPC response.
//! - `MONTAGE_MCP_TEST_MODE=tool_error` — `tools/call` returns a JSON-RPC
//!   error object.
//! - `MONTAGE_MCP_TEST_MODE=hang` — receives `initialize` then never
//!   replies (used to test client-side timeouts).
//! - `MONTAGE_MCP_TEST_MODE=hang_on_call` — replies to `initialize` and
//!   `tools/list` normally, but hangs on `tools/call` until stdin EOFs
//!   (used to test mid-call cancellation).
//! - `MONTAGE_MCP_TEST_MODE=progress_then_reply` — on `tools/call`, emits
//!   three `notifications/progress` frames keyed to the request's
//!   `_meta.progressToken`, then replies normally (used to test progress
//!   subscription).

use std::io::{BufRead, BufReader, Write};
use std::process::ExitCode;

enum ServerAction {
    Continue,
    Hang,
    Exit(ExitCode),
}

fn mode() -> String {
    std::env::var("MONTAGE_MCP_TEST_MODE").unwrap_or_else(|_| "normal".into())
}

fn main() -> ExitCode {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut reader = BufReader::new(stdin.lock());
    let mut line = String::new();
    let mode = mode();

    loop {
        line.clear();
        let n = match reader.read_line(&mut line) {
            Ok(0) => return ExitCode::SUCCESS,
            Ok(n) => n,
            Err(_) => return ExitCode::from(2),
        };
        let trimmed = line[..n].trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let Some(id) = msg.get("id") else {
            // Notifications carry no id; ignore them silently.
            continue;
        };

        match handle_message(&mut out, &msg, method, id, &mode) {
            ServerAction::Continue => {}
            ServerAction::Hang => {
                // Read further requests forever but never reply.
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return ExitCode::SUCCESS;
                    }
                }
            }
            ServerAction::Exit(code) => return code,
        }
    }
}

fn handle_message<W: Write>(
    out: &mut W,
    msg: &serde_json::Value,
    method: &str,
    id: &serde_json::Value,
    mode: &str,
) -> ServerAction {
    match (method, mode) {
        ("initialize", "malformed_init") => {
            let _ = writeln!(out, "this is not json");
            let _ = out.flush();
            ServerAction::Continue
        }
        ("initialize", "hang") => ServerAction::Hang,
        ("initialize", _) => {
            write_msg(out, &initialize_response(id));
            ServerAction::Continue
        }
        ("tools/list", _) => {
            write_msg(out, &tools_list_response(id));
            ServerAction::Continue
        }
        ("tools/call", "crash_on_call") => ServerAction::Exit(ExitCode::from(7)),
        ("tools/call", "hang_on_call") => ServerAction::Hang,
        ("tools/call", "tool_error") => {
            write_msg(out, &tool_error_response(id));
            ServerAction::Continue
        }
        ("tools/call", "progress_then_reply") => {
            // Look up the progress token the client sent, emit three
            // progress frames, then the regular tool reply.
            let token = msg
                .pointer("/params/_meta/progressToken")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            for (i, total) in [(1.0_f64, 3.0_f64), (2.0, 3.0), (3.0, 3.0)] {
                let note = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/progress",
                    "params": {
                        "progressToken": token,
                        "progress": i,
                        "total": total,
                        "message": format!("step {} of {}", i as u64, total as u64),
                    }
                });
                write_msg(out, &note);
            }
            write_msg(out, &tool_call_response(id, msg));
            ServerAction::Continue
        }
        ("tools/call", _) => {
            write_msg(out, &tool_call_response(id, msg));
            ServerAction::Continue
        }
        _ => {
            write_msg(out, &method_not_found_response(id, method));
            ServerAction::Continue
        }
    }
}

fn initialize_response(id: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": {
                "name": "montage-test",
                "version": "0.0.1"
            }
        }
    })
}

fn tools_list_response(id: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "tools": [
                {
                    "name": "echo",
                    "title": "Echo",
                    "description": "Echo back what you send.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string" }
                        },
                        "required": ["text"]
                    }
                },
                {
                    "name": "index_asset",
                    "description": "Stub indexer.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "asset_path": {"type": "string"},
                            "asset_id": {"type": "string"},
                            "asset_sha256": {"type": "string"}
                        },
                        "required": ["asset_path", "asset_id", "asset_sha256"]
                    }
                }
            ]
        }
    })
}

fn tool_error_response(id: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32000,
            "message": "synthetic tool failure"
        }
    })
}

fn tool_call_response(id: &serde_json::Value, msg: &serde_json::Value) -> serde_json::Value {
    let params = msg
        .get("params")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    match tool_name {
        "echo" => echo_response(id, &arguments),
        "index_asset" => index_asset_response(id, &arguments),
        other => unknown_tool_response(id, other),
    }
}

fn echo_response(id: &serde_json::Value, arguments: &serde_json::Value) -> serde_json::Value {
    let text = arguments
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [
                { "type": "text", "text": text }
            ],
            "isError": false
        }
    })
}

fn index_asset_response(
    id: &serde_json::Value,
    arguments: &serde_json::Value,
) -> serde_json::Value {
    let asset_id = arguments
        .get("asset_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let asset_sha = arguments
        .get("asset_sha256")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let sidecar = serde_json::json!({
        "indexer": "test-indexer",
        "indexer_version": "0.0.1",
        "schema_version": "1",
        "asset_id": asset_id,
        "asset_sha256": asset_sha,
        "produced_at": "2026-05-02T12:00:00Z",
        "data": {
            "echo": "ok"
        }
    });
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [],
            "structuredContent": sidecar,
            "isError": false
        }
    })
}

fn unknown_tool_response(id: &serde_json::Value, tool: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32601,
            "message": format!("unknown tool: {tool}")
        }
    })
}

fn method_not_found_response(id: &serde_json::Value, method: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32601,
            "message": format!("method not found: {method}")
        }
    })
}

fn write_msg<W: Write>(out: &mut W, v: &serde_json::Value) {
    let s = v.to_string();
    let _ = writeln!(out, "{s}");
    let _ = out.flush();
}
