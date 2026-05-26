//! `view_episode` — compact textual episode map. Ported in step 5
//! from `crates/core/src/tools/view_episode.rs`. No arguments;
//! reads the project at `AWIDAT_PROJECT_ROOT` and renders the
//! shared `episode_map::render` view.

use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// `view_episode` takes no arguments. The empty struct keeps the
/// rmcp tool macro happy and surfaces the empty schema to the model.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ViewEpisodeArgs {}

/// Run `view_episode`. Returns the rendered map as `Ok(String)`;
/// project-read failures are surfaced as `Err(String)`.
pub fn run(_args: ViewEpisodeArgs, ctx: McpToolCtx) -> Result<String, String> {
    let project = Project::read(&ctx.project_root).map_err(|e| {
        format!(
            "view_episode: failed to read project at {}: {e}",
            ctx.project_root.display()
        )
    })?;
    Ok(crate::episode_map::render(&project, &ctx.project_root))
}

pub const DESCRIPTION: &str = "\
Return a compact textual map of the episode: title, speaker count, \
topic list with timestamps, and the current editorial state (clip \
count, total duration, trimmed/untrimmed). The same map was injected \
into your system prompt at session start; call this tool to refresh \
it after edits or after a context compaction. No arguments. Cheap \
and read-only.";
