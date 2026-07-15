//! `find_dead_air` tool — surface silence ranges as editorial findings.
//!
//! Reads silence sidecars at `<project>/.montage/silences/<stem>-<hash>
//! .json` (written by Step 1.2's post-import chain), maps each source-
//! time silence range onto the project's timeline by walking the
//! current OTIO clips, and returns the ranges that survived
//! trimming + their surrounding transcript context.
//!
//! Each returned finding is shaped to become an `EditorialNote` of
//! kind `silence_trim` — the agent typically calls this tool, then
//! emits one Note per finding. In Phase 1.6 the dismissal-pattern
//! matcher consults its memory before re-surfacing repeats; v1
//! returns the raw findings and lets the caller filter.
//!
//! Sidecar discovery: tool reproduces the FNV-1a path-hash from
//! `apps/desktop/.../media.rs::stable_path_hash` so the core crate
//! doesn't need to depend on the desktop crate. Path computation is
//! `<project>/raw/<asset_id>` → absolute path → FNV-1a hash → join
//! with the project's silences dir under the asset's file stem.

use async_trait::async_trait;
use montage_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::podcast_cleanup_scan::scan_dead_air;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Default minimum silence duration the tool surfaces. Below this
/// (~breath beats), silences are part of natural speech rhythm; the
/// `find_dead_air` Note would be noise.
const DEFAULT_MIN_DURATION_S: f64 = 1.5;

/// Hard cap on returned findings. Long podcasts can have hundreds
/// of silences; we trim to keep the tool result manageable for the
/// model. The agent can re-call with a higher `max_results` if it
/// wants more.
const DEFAULT_MAX_RESULTS: usize = 20;
const HARD_MAX_RESULTS: usize = 100;

/// The `find_dead_air` tool.
pub struct FindDeadAirTool;

#[derive(Debug, Deserialize)]
struct FindDeadAirArgs {
    /// Min silence duration (seconds) to surface. Below this is
    /// treated as breath beat / natural rhythm. Default 1.5s.
    #[serde(default)]
    min_duration_s: Option<f64>,
    /// Max findings to return. Default 20, hard cap 100.
    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl ToolHandler for FindDeadAirTool {
    fn name(&self) -> &'static str {
        "find_dead_air"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_dead_air".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "min_duration_s": {
                        "type": "number",
                        "minimum": 0.6,
                        "description": "Minimum silence duration (s) to surface. Default 1.5."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 100,
                        "description": "Max findings to return. Default 20, hard cap 100."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: FindDeadAirArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_dead_air: invalid args ({e}). All fields optional."
            ))
        })?;
        let min_duration_s = args
            .min_duration_s
            .unwrap_or(DEFAULT_MIN_DURATION_S)
            .max(0.0);
        let max_results = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(HARD_MAX_RESULTS);

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("find_dead_air: failed to read project: {e}"))
        })?;

        let mut findings = scan_dead_air(
            &ctx.project_root,
            &project.timeline,
            min_duration_s,
            // Generate up to 2× the cap so dismissal-filtering still
            // returns up to `max_results` findings even when half
            // would have been filtered out.
            max_results.saturating_mul(2).max(max_results),
        );

        // Filter by per-project dismissal memory. The user dismissed
        // a duration bucket → all findings of that bucket get
        // dropped from this session's output.
        let dismissals = crate::dismissal::load_dismissals(&ctx.project_root);
        findings.retain(|f| {
            let bucket = crate::dismissal::DismissalBucket::for_silence(f.duration_s);
            !dismissals.is_dismissed(bucket)
        });
        findings.truncate(max_results);

        let body = serde_json::json!({
            "min_duration_s": min_duration_s,
            "findings": findings,
            "more_available": findings.len() == max_results,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
Surface silence ranges (\"dead air\") on the project timeline as \
editorial findings. Reads the per-asset silence sidecars produced \
on import, intersects each silence range with the clip's current \
source_range on the timeline (so trims that already removed a \
silence don't get re-surfaced), and returns the surviving silences \
plus surrounding transcript context.\
\n\nEach finding has: asset_id, source_start_s, source_end_s, \
timeline_start_s, timeline_end_s, duration_s, transcript_before, \
transcript_after. Use the timeline_* fields for click-to-seek; the \
source_* fields for `*** Trim Clip` envelopes.\
\n\nDefault min_duration_s=1.5 (below this is breath beat / \
natural rhythm — surfacing those would be noise). Default \
max_results=20, hard cap 100. Returns `more_available: true` when \
the cap was hit so you know to re-query with a higher limit.\
\n\nWhen no silence sidecars exist (assets weren't fully imported, \
or the post-import chain was interrupted), returns an empty \
findings list — NOT an error. Surface that as \"no dead air found \
yet — has indexing finished?\" rather than asserting the project is \
clean.\
";
