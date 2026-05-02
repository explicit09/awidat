//! Typed errors for the awidat MCP client.
//!
//! Variants are coarse but actionable. The CLI layer attaches `anyhow::Context`
//! at call sites (which indexer? which tool? which asset?). Tools that bubble
//! these to a model use `RespondToModel(err.to_string())` per `PLAN.md` §7.2.
//!
//! Most variants wrap `rmcp::service::ServiceError` — see `rmcp` for the
//! underlying transport / protocol error model. We keep our own enum at the
//! boundary so callers don't depend directly on the rmcp surface.

use thiserror::Error;

/// Anything that can go wrong talking to an MCP server.
#[derive(Debug, Error)]
pub enum McpError {
    /// Failed to spawn the child process or attach to its stdio pipes.
    /// The included message identifies which step failed.
    #[error("failed to spawn MCP server '{server}': {message}")]
    Spawn {
        /// Server name (from `ServerConfig.name`).
        server: String,
        /// Detail.
        message: String,
    },

    /// Underlying transport error — pipe I/O, send queue full, etc. Maps from
    /// `rmcp::service::ServiceError` for non-protocol errors.
    #[error("transport error talking to MCP server '{server}': {message}")]
    Transport {
        /// Server name.
        server: String,
        /// Diagnostic.
        message: String,
    },

    /// The server sent something that doesn't conform to MCP / JSON-RPC.
    /// The included message is a short diagnostic, not the raw payload.
    #[error("MCP server '{server}' violated the protocol: {message}")]
    ProtocolViolation {
        /// Server name.
        server: String,
        /// Diagnostic.
        message: String,
    },

    /// The server exited (or its stdout closed) while we were waiting for a
    /// response. The CLI surfaces this as "indexer X failed; other indexers
    /// continued" without taking the run down.
    #[error("MCP server '{server}' crashed: {message}")]
    ServerCrashed {
        /// Server name.
        server: String,
        /// Diagnostic.
        message: String,
    },

    /// We timed out waiting for a response.
    #[error("timed out waiting for response from MCP server '{server}' after {seconds}s")]
    Timeout {
        /// Server name.
        server: String,
        /// Configured timeout in seconds.
        seconds: u64,
    },

    /// The server reported a tool error (either a JSON-RPC error response or
    /// a `tools/call` result with `isError: true`).
    #[error("MCP server '{server}' tool error: {message}")]
    ToolError {
        /// Server name.
        server: String,
        /// Diagnostic from the server.
        message: String,
    },

    /// A request was abandoned by the caller (cancellation token fired) or
    /// because the client is shutting down.
    #[error("MCP request to '{server}' was cancelled")]
    Cancelled {
        /// Server name.
        server: String,
    },
}
