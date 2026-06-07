//! `montage-mcp-server` — stdio MCP server hosting Montage's video
//! tools. Spawned by codex (or any MCP-aware client) as a child
//! process. See `montage_core::montage_mcp` for the server definition
//! and the design rationale.
//!
//! Step 3 of the codex-harness migration: hello-world wiring. One
//! stub tool registered, just enough to prove codex can call into
//! Montage code over the standard MCP transport.

use anyhow::Result;
use montage_core::montage_mcp::MontageMcpServer;
use rmcp::ServiceExt;
use rmcp::transport::io::stdio;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // MCP servers MUST keep stdout clean for JSON-RPC. Route any
    // logging to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let (stdin, stdout) = stdio();
    let service = MontageMcpServer::new().serve((stdin, stdout)).await?;
    service.waiting().await?;
    Ok(())
}
