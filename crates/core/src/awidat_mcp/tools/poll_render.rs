//! `poll_render` — read the latest status of a render job started by
//! `start_render`. Ported from `crates/core/src/tools/poll_render.rs`
//! to the in-process MCP server.
//!
//! In the old harness the job table was an in-memory `JobManager` on
//! the per-`Session` `ToolContext`. The MCP server has no enclosing
//! `Session` — each tool call is a fresh `McpToolCtx::resolve()` — and
//! the ported `start_render` runs ffmpeg inline (awaiting completion
//! before it returns), so there is no in-flight job to poll afterwards.
//!
//! Rather than fake a status from the on-disk manifest, this port is
//! intentionally a stub that returns an honest unsupported-error to
//! the model. When out-of-process job state lands (a desktop-side
//! manager or a sidecar status file written by start_render) this can
//! become a manifest-driven read.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `poll_render`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PollRenderArgs {
    pub job_id: String,
}

pub fn run(_args: PollRenderArgs, _ctx: McpToolCtx) -> Result<String, String> {
    Err(
        "poll_render: not yet supported via MCP — needs out-of-process job state. \
         The in-process MCP server's start_render runs ffmpeg inline and returns \
         only after the render terminates, so there is no in-flight job to poll. \
         Inspect the output_path returned by start_render directly, or use \
         verify_render on the finished output."
            .into(),
    )
}

pub const DESCRIPTION: &str = "\
Read the latest status of a render job started by `start_render`. \
NOTE: in the in-process MCP server, start_render runs ffmpeg \
synchronously and returns only when the render terminates, so there is \
no in-flight job to poll. This tool returns an unsupported-error and \
asks the caller to inspect the output_path / call verify_render \
instead. Will become functional when out-of-process job state lands.";
