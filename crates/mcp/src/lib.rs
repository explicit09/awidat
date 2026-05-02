//! Awidat MCP client.
//!
//! This crate is the **canonical tool protocol layer** for Awidat. Per
//! `PLAN.md` §6 every external tool — every footage indexer (Week 2), every
//! ffmpeg / search / read tool the agent will call (Week 4+) — is reached
//! through a Model Context Protocol server, never through a Python-specific
//! shim. Building the right MCP client once is therefore Concern 2 of Week 2:
//! the API shape here is the API every downstream caller sees forever.
//!
//! # Surface
//!
//! - [`Client`] launches and supervises an MCP server subprocess and exposes
//!   [`Client::initialize`], [`Client::list_tools`], [`Client::call_tool`].
//! - [`ServerConfig`] describes how to spawn one server.
//! - [`McpError`] is the typed error every public method returns.
//!
//! # What's implemented
//!
//! - JSON-RPC 2.0 over stdio with **newline-delimited JSON** framing per
//!   the MCP `2025-06-18` / `2025-11-25` stdio transport spec. (Note: this
//!   is *not* LSP-style `Content-Length:` framing — MCP stdio diverges from
//!   LSP on this point. Implementers must read the stdio transport doc, not
//!   assume LSP carries over.)
//! - The `initialize` → `notifications/initialized` → `tools/list` → `tools/call`
//!   request flow.
//! - Process supervision: dropping a [`Client`] kills the child; the child's
//!   stderr is captured for diagnostics; a server crash mid-call surfaces as
//!   a typed [`McpError::ServerCrashed`].
//!
//! # What's deferred
//!
//! Resources, prompts, sampling, elicitation, progress notifications,
//! bidirectional notifications, and HTTP transport. They land when an
//! agent feature needs them (Week 4+ for resources/sampling). The wire
//! types are forward-compatible: we serialize an `id` on every request and
//! never assume the server omits unknown fields.
//!
//! # Example
//!
//! ```no_run
//! use std::collections::HashMap;
//! use awidat_mcp::{Client, ClientInfo, ServerConfig};
//!
//! # async fn run() -> Result<(), awidat_mcp::McpError> {
//! let mut client = Client::launch(ServerConfig {
//!     name: "audio-energy".into(),
//!     command: "python".into(),
//!     args: vec!["-m".into(), "awidat_audio_energy".into()],
//!     env: HashMap::new(),
//!     cwd: None,
//! })?;
//!
//! let server_info = client.initialize(ClientInfo {
//!     name: "awidat".into(),
//!     version: env!("CARGO_PKG_VERSION").into(),
//! }).await?;
//! println!("Talking to {} {}", server_info.name, server_info.version);
//!
//! let tools = client.list_tools().await?;
//! for t in &tools { println!("- {}: {}", t.name, t.description.as_deref().unwrap_or("")); }
//!
//! let result = client.call_tool("index_asset", serde_json::json!({
//!     "asset_path": "/tmp/sample.wav",
//!     "asset_id": "raw/sample.wav",
//!     "asset_sha256": "deadbeef",
//! })).await?;
//! println!("Tool result content blocks: {}", result.content.len());
//! client.shutdown().await?;
//! # Ok(()) }
//! ```

mod client;
mod error;
mod protocol;
mod transport;

pub use client::{
    Client, ClientInfo, ServerCapabilities, ServerConfig, ServerInfo, ToolDescriptor, ToolResult,
};
pub use error::McpError;
pub use protocol::{ContentBlock, MCP_PROTOCOL_VERSION};
pub use transport::ProgressEvent;
