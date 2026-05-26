//! `request_user_input` — stub. Ported from
//! `crates/core/src/tools/request_user_input.rs` to the in-process MCP
//! server as a stub returning an unsupported error.
//!
//! In the old harness this tool suspended the in-flight tool call until
//! the user replied, by pushing a [`UserInputRequest`] onto the
//! runtime's user-input channel and awaiting an embedded oneshot. The
//! MCP server has no enclosing `Session`, no `user_input_tx` channel,
//! and no per-call await context. The clean long-term mapping is MCP's
//! elicitation protocol (server-initiated prompts to the client) — once
//! the in-process server learns to issue those, this tool can route a
//! question through the client and return the response as the tool
//! result.
//!
//! Until then this is a stub: returns an `Err(String)` so the model
//! can route around it by phrasing the question in chat and waiting
//! for the user's next message. Annotated `read_only_hint = true`
//! because it never writes anything.
//!
//! See `crates/core/src/awidat_mcp/tools/poll_render.rs` for the same
//! "stub with explanatory Err" pattern.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `request_user_input`. Kept for schema-compat with the
/// legacy tool even though the stub ignores them.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct RequestUserInputArgs {
    /// Concrete one-sentence question.
    pub question: String,
    /// Optional list of choices.
    #[serde(default)]
    pub options: Option<Vec<String>>,
    /// Optional pre-filled default.
    #[serde(default)]
    pub default: Option<String>,
}

pub fn run(_args: RequestUserInputArgs, _ctx: McpToolCtx) -> Result<String, String> {
    Err(
        "request_user_input is not yet bridged to MCP elicitation; the agent \
         should phrase a question in chat instead and wait for the user's next message"
            .into(),
    )
}

pub const DESCRIPTION: &str = "\
Ask the human a question and wait for a reply. NOTE: in the in-process \
MCP server this is currently a stub — it returns an unsupported-error \
and asks the agent to phrase the question in its chat reply and wait \
for the user's next turn instead. Will become functional once the \
server bridges MCP's elicitation protocol to the client.";
