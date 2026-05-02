//! Tiny MCP-over-stdio test server used by `client_against_test_server.rs`.
//!
//! We hand-roll the JSON-RPC framing (newline-delimited) instead of pulling
//! in an MCP server library because (a) we don't want a dev-dep, and (b)
//! exercising the wire format from a deliberately-minimal implementation
//! catches client bugs the official server's politeness might paper over.
//!
//! Behavior is tweakable through env vars:
//!
//! - `AWIDAT_MCP_TEST_MODE=normal` (default) — implements `initialize`,
//!   `tools/list`, and a `tools/call` for `echo` and `index_asset`.
//! - `AWIDAT_MCP_TEST_MODE=crash_on_call` — exits with code 7 the first
//!   time `tools/call` is invoked.
//! - `AWIDAT_MCP_TEST_MODE=malformed_init` — replies to `initialize` with
//!   text that isn't a JSON-RPC response.
//! - `AWIDAT_MCP_TEST_MODE=tool_error` — `tools/call` returns a JSON-RPC
//!   error object.
//! - `AWIDAT_MCP_TEST_MODE=hang` — receives `initialize` then never
//!   replies (used to test client-side timeouts).

use std::io::{BufRead, BufReader, Write};
use std::process::ExitCode;

fn mode() -> String {
    std::env::var("AWIDAT_MCP_TEST_MODE").unwrap_or_else(|_| "normal".into())
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
        let id = msg.get("id").cloned();

        // Notifications carry no id; ignore them silently.
        if id.is_none() {
            continue;
        }

        match (method, mode.as_str()) {
            ("initialize", "malformed_init") => {
                let _ = writeln!(out, "this is not json");
                let _ = out.flush();
            }
            ("initialize", "hang") => {
                // Read further requests forever but never reply.
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return ExitCode::SUCCESS;
                    }
                }
            }
            ("initialize", _) => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2025-06-18",
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": "awidat-test",
                            "version": "0.0.1"
                        }
                    }
                });
                write_msg(&mut out, &resp);
            }
            ("tools/list", _) => {
                let resp = serde_json::json!({
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
                });
                write_msg(&mut out, &resp);
            }
            ("tools/call", "crash_on_call") => {
                return ExitCode::from(7);
            }
            ("tools/call", "tool_error") => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32000,
                        "message": "synthetic tool failure"
                    }
                });
                write_msg(&mut out, &resp);
            }
            ("tools/call", _) => {
                let params = msg.get("params").cloned().unwrap_or(serde_json::Value::Null);
                let tool_name = params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));
                let resp = match tool_name {
                    "echo" => {
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
                    "index_asset" => {
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
                    other => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32601,
                            "message": format!("unknown tool: {other}")
                        }
                    }),
                };
                write_msg(&mut out, &resp);
            }
            (_, _) => {
                let resp = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32601,
                        "message": format!("method not found: {method}")
                    }
                });
                write_msg(&mut out, &resp);
            }
        }
    }
}

fn write_msg<W: Write>(out: &mut W, v: &serde_json::Value) {
    let s = v.to_string();
    let _ = writeln!(out, "{s}");
    let _ = out.flush();
}
