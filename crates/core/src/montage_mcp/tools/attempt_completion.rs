//! `attempt_completion` — signal end-of-turn back to the parent agent.
//! Ported from `crates/core/src/tools/attempt_completion.rs` to the
//! in-process MCP server.
//!
//! In the old Montage harness this tool was the sub-agent's only path
//! back to the parent: it wrote a `result` string into the sub-session's
//! return slot and the `delegate` tool read that slot once the sub-agent
//! finished. Under codex + MCP there is no Montage-managed sub-agent
//! plumbing; codex's own `agent_tool` primitives handle sub-agent
//! lifecycle, and the model's final text comes back through its normal
//! response channel. So this tool becomes a thin "echo back the summary
//! as the tool result" wrapper — the agent calls it to mark its summary
//! visible in the transcript, but it doesn't gate the turn.
//!
//! Annotated `read_only_hint = true`: no filesystem writes, no external
//! side-effects.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `attempt_completion`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct AttemptCompletionArgs {
    /// The final answer/summary. Echoed verbatim as the tool result.
    pub result: String,
}

pub fn run(args: AttemptCompletionArgs, _ctx: McpToolCtx) -> Result<String, String> {
    if args.result.trim().is_empty() {
        return Err("attempt_completion: result must not be empty.".into());
    }
    Ok(args.result)
}

pub const DESCRIPTION: &str = "\
Record your final answer/summary for the current turn. The `result` \
string is echoed back verbatim as the tool result so it lands in the \
conversation transcript. NOTE: under MCP this tool does NOT gate or end \
the turn — the model's final assistant message is what closes the turn. \
Use this when you want the summary to appear as a structured tool result \
in addition to your free-form reply, or when porting prompts that \
expected the legacy sub-agent contract.";
