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
//! Step 3 (read-only view_timeline stub) and step 4 (mutating-tool
//! pre-execution approval gating) of the codex-harness migration
//! ship here. Step 5 will bulk-port the ~100 real video tools.
//!
//! ## Adding a tool
//!
//! Write an `async fn` inside `impl AwidatMcpServer` with
//! `#[tool(description = "...")]`. Annotate every tool with one of:
//!
//! - `annotations(read_only_hint = true)` for tools that only read
//!   project state. Codex won't fire an approval prompt for these.
//! - `annotations(destructive_hint = true)` for tools that mutate
//!   project state (EDL writes, render jobs, asset imports, bash).
//!   Codex's `requires_mcp_tool_approval` gate (see
//!   `vendor/codex-rs/core/src/mcp_tool_call.rs`) consults this and
//!   fires the pre-execution approval prompt when
//!   `approval_policy != "never"`.
//!
//! Tools missing either annotation are conservatively treated as
//! mutating by codex (see `mcp_tool_call.rs::requires_mcp_tool_approval`).
//! That fail-closed behavior is the right default; the annotations are
//! how a tool opts out.

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

/// Arguments for `apply_fake_edl`. Step 4 stub mutating tool used
/// purely to exercise codex's pre-execution approval prompt; step 5
/// replaces this with the real `apply_edl`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ApplyFakeEdlArgs {
    /// EDL text the model wants to apply. Ignored by the stub.
    pub edl: String,
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
            migration proves the in-process MCP server wiring.",
        annotations(read_only_hint = true)
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

    /// Stub `apply_fake_edl`. Annotated as destructive so codex
    /// fires a pre-execution approval prompt; if the user approves,
    /// we return a canned acknowledgement instead of actually
    /// mutating anything. Step 5 replaces this with the real
    /// `apply_edl` implementation.
    #[tool(
        description = "Apply an EDL envelope to the project's edited timeline. \
            Mutates project state. Step 4 stub: returns a canned acknowledgement \
            after the codex approval prompt fires.",
        annotations(destructive_hint = true, read_only_hint = false)
    )]
    pub async fn apply_fake_edl(&self, args: Parameters<ApplyFakeEdlArgs>) -> String {
        let preview: String = args.0.edl.chars().take(80).collect();
        format!(
            "Awidat MCP step-4 stub: apply_fake_edl accepted EDL prefix {preview:?}. \
             No real mutation performed."
        )
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AwidatMcpServer {}
