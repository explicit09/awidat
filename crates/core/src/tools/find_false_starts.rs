//! `find_false_starts` tool — surface places where the speaker
//! began a thought, abandoned it, and restarted.
//!
//! Heuristic in v1 (gated by Phase 1 user reaction; the hard
//! version using a continuity model is Phase 4):
//!
//! 1. **Abrupt cut**: a word whose end is followed by a silence
//!    range ≥ 0.4s starting within 1s, AND the next word is not
//!    a continuation of the same thought (different sentence per
//!    whisper segment boundaries, or contains a restart marker).
//!
//! 2. **Restart marker**: the speaker says one of {"wait", "let
//!    me", "actually"} mid-utterance. We surface the *prior*
//!    fragment (from the segment start to the restart marker) as
//!    the false start; the trim removes everything from segment-
//!    start to restart-marker-start.
//!
//! Each finding shapes for an `EditorialNote` of kind `false_start`.
//! Trimmed-away ranges still surface if the visible portion contains
//! a restart marker — the user can decide whether to also delete
//! more upstream context.

use async_trait::async_trait;
use montage_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::podcast_cleanup_scan::scan_false_starts;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Default cap on findings.
const DEFAULT_MAX_RESULTS: usize = 20;
const HARD_MAX_RESULTS: usize = 100;

/// Tool that finds false starts and restart phrases in the active timeline.
pub struct FindFalseStartsTool;

#[derive(Debug, Deserialize)]
struct FindFalseStartsArgs {
    /// Max findings to return. Default 20, hard cap 100.
    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl ToolHandler for FindFalseStartsTool {
    fn name(&self) -> &'static str {
        "find_false_starts"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_false_starts".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
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
        let args: FindFalseStartsArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_false_starts: invalid args ({e}). All fields optional."
            ))
        })?;
        let max_results = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(HARD_MAX_RESULTS);

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_false_starts: failed to read project: {e}"
            ))
        })?;

        // Honor per-project dismissal: if the user dismissed
        // false-start findings in a prior session, return empty.
        let dismissals = crate::dismissal::load_dismissals(&ctx.project_root);
        if dismissals.is_dismissed(crate::dismissal::DismissalBucket::FalseStart) {
            let body = serde_json::json!({
                "findings": Vec::<crate::podcast_cleanup_scan::FalseStartFinding>::new(),
                "more_available": false,
                "dismissed": true,
            });
            return Ok(ToolOutput::text(body.to_string()));
        }

        let findings = scan_false_starts(&ctx.project_root, &project.timeline, max_results);
        let body = serde_json::json!({
            "findings": findings,
            "more_available": findings.len() == max_results,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

const DESCRIPTION: &str = "\
Detect places where the speaker began a thought, abandoned it, and \
restarted (\"actually, what I meant was…\", \"wait, let me back up\", \
etc), plus production/coaching asides such as \"cut\", \"one more time\", \
or \"you can just say\". v1 heuristic: scans the whisper transcript \
for restart markers and production-aside language, then surfaces the \
visible source fragment as the candidate false-start.\
\n\nEach finding: { asset_id, marker, source_start_s, source_end_s, \
timeline_start_s, timeline_end_s, snippet }. The marker tells the \
agent what triggered the detection so it can describe the Note. The \
source range covers the fragment that precedes the restart — what \
the user might trim.\
\n\nLimitations: this is a heuristic, not a continuity model. False \
positives are likely (\"actually\" used as an emphatic, not a \
restart). The agent should treat findings as suggestions and present \
them as Notes the user reviews — never auto-trim without approval.\
\n\nDefault max_results=20, hard cap 100. Returns empty when no \
whisper sidecars exist.\
";
