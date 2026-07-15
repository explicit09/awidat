//! `find_filler_words` tool — scan whisper transcripts for verbal
//! fillers ("um", "uh", "ah", "er", etc) that the agent could
//! suggest cutting.
//!
//! Reads `index/whisper/<asset>.json` per asset (word-level
//! alignment when available; falls back to segment-level if no
//! word array), filters words whose lowercase form matches a small
//! configurable filler list, intersects each word's span with the
//! timeline's clip ranges, and returns the surviving fillers as
//! editorial findings.
//!
//! Coupling with continuity (Phase 2): a filler that lands mid-
//! sentence is a "clean cut" candidate; a filler that's standing
//! on its own with silence before/after is even cleaner. v1
//! returns the raw filler ranges; the agent (or Phase 2 rules) can
//! decide which to surface as Notes.

use async_trait::async_trait;
use montage_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::podcast_cleanup_scan::{FillerFinding, scan_filler_words};
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;
use crate::transcript_cleanup::{default_filler_tokens, normalize_transcript_token};

/// Default cap on returned findings. Same magnitude as
/// `find_dead_air`; podcasts can have hundreds of "um"s and the
/// agent shouldn't drown in tool output.
const DEFAULT_MAX_RESULTS: usize = 30;
const HARD_MAX_RESULTS: usize = 200;

/// Tool that finds filler words in transcript regions visible on the timeline.
pub struct FindFillerWordsTool;

#[derive(Debug, Deserialize)]
struct FindFillerWordsArgs {
    /// Override the filler list. Lowercase-matched.
    #[serde(default)]
    fillers: Option<Vec<String>>,
    /// Include aggressive discourse markers ("like", "you know",
    /// "i mean", "basically") in the match list. Default false.
    #[serde(default)]
    aggressive: bool,
    /// Max findings to return. Default 30, hard cap 200.
    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl ToolHandler for FindFillerWordsTool {
    fn name(&self) -> &'static str {
        "find_filler_words"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_filler_words".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "fillers": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Override the default filler list (case-insensitive)."
                    },
                    "aggressive": {
                        "type": "boolean",
                        "description": "Include discourse markers like / you know / i mean / basically. Default false."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 200,
                        "description": "Max findings to return. Default 30, hard cap 200."
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
        let args: FindFillerWordsArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_filler_words: invalid args ({e}). All fields optional."
            ))
        })?;
        let fillers = build_filler_set(args.fillers, args.aggressive);
        let max_results = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(HARD_MAX_RESULTS);

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_filler_words: failed to read project: {e}"
            ))
        })?;

        // Honor dismissal memory at the bucket level: dismissing
        // "filler basic" silences all default-list fillers;
        // dismissing "filler aggressive" silences just the
        // discourse-marker findings (the basic ones still surface).
        let dismissals = crate::dismissal::load_dismissals(&ctx.project_root);
        let bucket = crate::dismissal::DismissalBucket::for_filler(args.aggressive);
        if dismissals.is_dismissed(bucket) {
            let body = serde_json::json!({
                "fillers": fillers,
                "findings": Vec::<FillerFinding>::new(),
                "more_available": false,
                "dismissed": true,
            });
            return Ok(ToolOutput::text(body.to_string()));
        }

        let findings =
            scan_filler_words(&ctx.project_root, &project.timeline, &fillers, max_results);
        let body = serde_json::json!({
            "fillers": fillers,
            "findings": findings,
            "more_available": findings.len() == max_results,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

fn build_filler_set(override_list: Option<Vec<String>>, aggressive: bool) -> Vec<String> {
    if let Some(list) = override_list {
        return list
            .into_iter()
            .map(|token| normalize_transcript_token(&token))
            .filter(|token| !token.is_empty())
            .collect();
    }
    default_filler_tokens(aggressive)
}

const DESCRIPTION: &str = "\
Scan the project's whisper transcripts for filler words (\"um\", \
\"uh\", etc) that the agent could suggest cutting. Each finding is \
a single word's span: { asset_id, text, source_start_s, \
source_end_s, timeline_start_s, timeline_end_s }, ready to become \
an EditorialNote of kind filler_word or to drive a `*** Trim Clip` \
envelope.\
\n\nDefault filler list: um/uh/uhh/umm/ah/ahh/er/err. Pass \
`aggressive: true` to also include discourse markers (like / so / \
just / but / yeah / basically / you know / i mean) — these are \
taste-dependent so they're opt-in. Pass `fillers: [...]` to \
override the list entirely.\
\n\nFindings are intersected with the timeline's clip ranges, so \
fillers in trimmed-away regions don't get re-surfaced. Default \
max_results=30, hard cap 200; `more_available: true` when the cap \
was hit.\
\n\nReturns an empty findings list when whisper sidecars haven't \
landed yet — surface that as \"transcripts still indexing?\" \
rather than \"no fillers in this episode.\"\
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_filler_set_defaults_to_basic_list() {
        let fillers = build_filler_set(None, false);
        assert!(fillers.iter().any(|f| f == "um"));
        assert!(!fillers.iter().any(|f| f == "like"));
    }

    #[test]
    fn build_filler_set_aggressive_includes_discourse_markers() {
        let fillers = build_filler_set(None, true);
        assert!(fillers.iter().any(|f| f == "like"));
    }

    #[test]
    fn build_filler_set_override_normalizes_tokens() {
        let fillers = build_filler_set(Some(vec!["FOOBAR".into()]), false);
        assert_eq!(fillers, vec!["foobar".to_string()]);
    }
}
