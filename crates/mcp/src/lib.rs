//! Awidat MCP client.
//!
//! Thin wrapper over [`rmcp`] (the official Rust MCP SDK) that exposes the
//! small surface our agent and CLI need: launch, initialize, list_tools,
//! call_tool (with optional cancellation and progress subscription),
//! capabilities, shutdown.
//!
//! # Surface
//!
//! - [`Client`] launches and supervises an MCP server subprocess and exposes
//!   [`Client::initialize`], [`Client::list_tools`], [`Client::call_tool`],
//!   [`Client::call_tool_with_cancel`], [`Client::call_tool_with_progress`],
//!   [`Client::capabilities`], [`Client::shutdown`].
//! - [`ServerConfig`] describes how to spawn one server.
//! - [`McpError`] is the typed error every public method returns.
//!
//! Background and design rationale: see `crates/mcp/NOTES.md`.

mod client;
mod error;
mod handler;

pub use client::{
    Client, ClientInfo, ContentBlock, MCP_PROTOCOL_VERSION, ServerCapabilities, ServerConfig,
    ServerInfo, ToolDescriptor, ToolResult, mcp_protocol_version,
};
pub use error::McpError;
pub use handler::ProgressEvent;
