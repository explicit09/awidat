//! `podcast_qc_report` — pre-render podcast QC gate.
//! Thin MCP wrapper around the canonical tool implementation.

use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `podcast_qc_report`. None — read-only inspection over
/// the entire timeline.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PodcastQcReportArgs {}

/// Run `podcast_qc_report` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`;
/// project-read errors return `Err(String)`.
pub fn run(_args: PodcastQcReportArgs, ctx: McpToolCtx) -> Result<String, String> {
    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("podcast_qc_report: failed to read project: {e}"))?;
    let body =
        crate::tools::podcast_qc_report::build_podcast_qc_report(&ctx.project_root, &project);
    serde_json::to_string(&body).map_err(|e| format!("podcast_qc_report serialize: {e}"))
}

pub const DESCRIPTION: &str = "\
Run pre-render podcast QC for gaps, missing media, captions, audio readiness, \
workflow-section completion, and suspicious timeline structure. Returns a \
status (ready / needs_review / blocked), an `issues` list with severity-tagged \
findings, `workflow_sections` receipts, caption summary, and audio meter count. \
Read-only; timeline renders block when the report status is blocked.\
";
