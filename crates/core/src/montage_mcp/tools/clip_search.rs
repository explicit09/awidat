//! `clip_search` — stub. Ported from
//! `crates/core/src/tools/clip_search.rs` to the in-process MCP server
//! as a stub returning an unsupported error.
//!
//! In the old harness this tool searched per-second CLIP image
//! embeddings against a free-text query by encoding the query through
//! the live `clip-mcp` server (a child MCP indexer process managed by
//! `ToolContext.mcp_host`'s lazy pool) and then computing cosine
//! similarity against each frame's float16 vector. The in-process MCP
//! server has no `mcp_host` — there is no lazy MCP indexer subprocess
//! pool wired in yet — so the encode-query path can't run.
//!
//! Until the pool is rebuilt the right fallback is the transcript-based
//! search via `find_moment` (BM25 over whisper segments). Annotated
//! `read_only_hint = true` because the live implementation is
//! side-effect-free.
//!
//! See `crates/core/src/montage_mcp/tools/poll_render.rs` for the same
//! "stub with explanatory Err" pattern.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `clip_search`. Kept for schema-compat with the legacy
/// tool even though the stub ignores them.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ClipSearchArgs {
    /// Free-text description of what to find.
    pub query: String,
    /// Restrict to one asset id; otherwise rank across all assets.
    #[serde(default)]
    pub asset_id: Option<String>,
    /// Top-K to return per asset. Default 5, hard cap 25.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Minimum cosine score floor.
    #[serde(default)]
    pub min_score: Option<f32>,
}

pub fn run(_args: ClipSearchArgs, _ctx: McpToolCtx) -> Result<String, String> {
    Err(
        "clip_search needs the MCP indexer pool which is not yet wired into the \
         in-process server; use find_moment as a transcript-based alternative \
         until this is rebuilt"
            .into(),
    )
}

pub const DESCRIPTION: &str = "\
Find frames matching a free-text query using per-second CLIP image \
embeddings. NOTE: in the in-process MCP server this is currently a \
stub — it returns an unsupported-error because the lazy clip-mcp \
indexer subprocess pool is not yet wired in. Use `find_moment` for a \
transcript-based search in the meantime; this tool will become \
functional once the MCP indexer pool lands.";
