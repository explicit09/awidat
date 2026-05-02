//! Typed errors for the MCP client.
//!
//! Variants are coarse but actionable. The CLI layer attaches `anyhow::Context`
//! at call sites (which indexer? which tool? which asset?). Tools that bubble
//! these to a model use `RespondToModel(err.to_string())` per `PLAN.md` §7.2.

use thiserror::Error;

/// Anything that can go wrong talking to an MCP server.
#[derive(Debug, Error)]
pub enum McpError {
    /// Failed to spawn the child process.
    #[error("failed to spawn MCP server '{server}': {source}")]
    Spawn {
        /// Server name (from `ServerConfig.name`).
        server: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The server's stdout/stdin couldn't be opened or piped. Implementation
    /// bug, not user error.
    #[error("failed to attach to stdio for MCP server '{server}': {message}")]
    StdioAttach {
        /// Server name.
        server: String,
        /// Detail.
        message: String,
    },

    /// I/O error reading from / writing to the child's pipes after launch.
    #[error("I/O error talking to MCP server '{server}': {source}")]
    Io {
        /// Server name.
        server: String,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },

    /// The server sent something that doesn't conform to MCP / JSON-RPC.
    /// The included message is a short diagnostic, not the raw payload.
    #[error("MCP server '{server}' violated the protocol: {message}")]
    ProtocolViolation {
        /// Server name.
        server: String,
        /// Diagnostic, e.g. `"unexpected jsonrpc version: 1.0"`.
        message: String,
    },

    /// The server exited (or its stdout closed) while we were waiting for a
    /// response. The CLI surfaces this as "indexer X failed; other indexers
    /// continued" without taking the run down.
    #[error("MCP server '{server}' crashed (exit status: {exit_status}); stderr tail: {stderr}")]
    ServerCrashed {
        /// Server name.
        server: String,
        /// String form of the exit status (or `"unknown"` if not waitable).
        exit_status: String,
        /// Last bytes of the server's stderr — capped at 4 KiB to keep
        /// diagnostics terse.
        stderr: String,
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

    /// A request was abandoned because the client is shutting down.
    #[error("MCP client cancelled while waiting on '{server}'")]
    Cancelled {
        /// Server name.
        server: String,
    },
}
