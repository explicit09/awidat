//! In-process Awidat MCP server.
//!
//! Codex consumes this server like any other stdio MCP server (see the
//! `[mcp_servers.awidat]` entry passed via `-c` in
//! `crates/cli/src/chat_codex_cmd.rs`). Codex spawns our binary, talks
//! JSON-RPC over stdin/stdout, lists our tools, and routes model tool
//! calls into them.
//!
//! Why MCP instead of a deeper codex fork (step 2 design loop in
//! `docs/superpowers/specs/...`): codex's "native" tool surface is
//! JSON descriptors fed to the model, not a Rust trait we can
//! implement. The actual Rust execution code for a tool has to live
//! somewhere off the model's call path; MCP is the well-trodden hook
//! and codex already handles connection lifecycle, schema negotiation,
//! and approval routing for MCP servers. Putting our tools here keeps
//! all Awidat changes in `crates/`, not `vendor/codex-rs/`, so future
//! fork refreshes don't touch our tool surface.
//!
//! This module is the **skeleton** that step 3 of the codex-harness
//! migration delivers: one read-only tool (`view_timeline`) returning
//! stub data, just enough to prove codex talks to us end-to-end.
//! Steps 4 and 5 add the approval pre-gate fork hook and bulk-port
//! the ~100 real video tools.

use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_handler;
use rmcp::tool_router;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// Arguments for `view_timeline`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ViewTimelineArgs {
    /// Optional project root. Ignored by the skeleton; real
    /// implementation in step 5 will read `project.otio.json` from
    /// here.
    #[serde(default)]
    pub project_root: Option<String>,
}

/// The Awidat MCP server. One short-lived struct per child-process
/// invocation. Holds a `ToolRouter` populated by the `#[tool_router]`
/// macro on `impl AwidatMcpServer`.
#[derive(Debug, Clone)]
pub struct AwidatMcpServer {
    tool_router: ToolRouter<Self>,
}

impl AwidatMcpServer {
    /// Build a fresh server with all registered tools.
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

impl Default for AwidatMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl AwidatMcpServer {
    /// Stub `view_timeline`. Step 5 replaces this body with an OTIO
    /// read; for now it returns a canned summary so we can prove
    /// codex calls into us end-to-end.
    #[tool(
        description = "View a summary of the current Awidat project's edited timeline. \
            Skeleton implementation: returns a canned response while step 3 of the codex-harness \
            migration proves the in-process MCP server wiring."
    )]
    pub async fn view_timeline(&self, _args: Parameters<ViewTimelineArgs>) -> String {
        // Step 5 replaces this with the real OTIO loader. Until then,
        // returning a recognizable string lets the agent (and us)
        // confirm that the round trip works.
        "Awidat MCP skeleton: view_timeline returned stub data. \
         No project loaded; this is the hello-world wiring for step 3 \
         of the codex-harness migration."
            .to_string()
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AwidatMcpServer {}
