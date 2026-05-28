//! `podcast_cleanup_candidates` — aggregate existing cleanup evidence.
//! Ported from `crates/core/src/tools/podcast_cleanup_candidates.rs`
//! to the in-process MCP server.
//!
//! This does not invent a new audio analyzer. It packages the evidence
//! Awidat already has — dead air, filler words, and false starts —
//! into safe/review/risky candidate buckets for the podcast workflow.

use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `podcast_cleanup_candidates`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PodcastCleanupCandidatesArgs {
    /// Maximum candidates per evidence family. Default 40.
    #[serde(default)]
    pub max_results: Option<usize>,
    /// Minimum silence duration to consider. Default 1.2s.
    #[serde(default)]
    pub dead_air_min_duration_s: Option<f64>,
    /// Include discourse markers like like / you know / i mean.
    /// Default false.
    #[serde(default)]
    pub aggressive_fillers: bool,
}

#[derive(Debug, Serialize)]
struct CleanupReport {
    status: &'static str,
    summary_for_agent: String,
    safe_cuts: Vec<CleanupCandidate>,
    review_cuts: Vec<CleanupCandidate>,
    risky_cuts: Vec<CleanupCandidate>,
    missing_evidence: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CleanupCandidate {
    kind: &'static str,
    asset_id: String,
    source_start_s: f64,
    source_end_s: f64,
    timeline_start_s: f64,
    timeline_end_s: f64,
    confidence: f64,
    risk: &'static str,
    suggested_action: &'static str,
    evidence: String,
    requires_approval: bool,
}

/// Run `podcast_cleanup_candidates` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`;
/// project-read errors return `Err(String)`.
pub fn run(args: PodcastCleanupCandidatesArgs, ctx: McpToolCtx) -> Result<String, String> {
    let max_results = args.max_results.unwrap_or(40).clamp(1, 200);
    let dead_air_min_duration_s = args.dead_air_min_duration_s.unwrap_or(1.2).max(0.6);

    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("podcast_cleanup_candidates: failed to read project: {e}"))?;

    let mut safe_cuts = Vec::new();
    let mut review_cuts = Vec::new();
    let risky_cuts = Vec::new();
    let mut missing_evidence = Vec::new();

    let dead_air = crate::tools::find_dead_air::scan_dead_air(
        &ctx.project_root,
        &project.timeline,
        dead_air_min_duration_s,
        max_results,
    );
    if dead_air.is_empty() {
        missing_evidence.push("no timeline-visible dead-air findings from silence sidecars".into());
    }
    for finding in dead_air {
        let candidate = CleanupCandidate {
            kind: "dead_air",
            asset_id: finding.asset_id,
            source_start_s: finding.source_start_s,
            source_end_s: finding.source_end_s,
            timeline_start_s: finding.timeline_start_s,
            timeline_end_s: finding.timeline_end_s,
            confidence: if finding.duration_s >= 2.0 { 0.9 } else { 0.72 },
            risk: if finding.duration_s >= 2.0 {
                "low"
            } else {
                "medium"
            },
            suggested_action: if finding.duration_s >= 2.0 {
                "safe_cut_candidate"
            } else {
                "review_cut_candidate"
            },
            evidence: format!(
                "{:.2}s silence; before={:?}; after={:?}",
                finding.duration_s, finding.transcript_before, finding.transcript_after
            ),
            requires_approval: finding.duration_s < 2.0,
        };
        if candidate.requires_approval {
            review_cuts.push(candidate);
        } else {
            safe_cuts.push(candidate);
        }
    }

    let fillers = crate::transcript_cleanup::default_filler_tokens(args.aggressive_fillers);
    let filler_words = crate::tools::find_filler_words::scan_filler_words(
        &ctx.project_root,
        &project.timeline,
        &fillers,
        max_results,
    );
    if filler_words.is_empty() {
        missing_evidence.push("no timeline-visible filler findings from whisper words".into());
    }
    for finding in filler_words {
        review_cuts.push(CleanupCandidate {
            kind: "filler_word",
            asset_id: finding.asset_id,
            source_start_s: finding.source_start_s,
            source_end_s: finding.source_end_s,
            timeline_start_s: finding.timeline_start_s,
            timeline_end_s: finding.timeline_end_s,
            confidence: if args.aggressive_fillers { 0.55 } else { 0.78 },
            risk: "medium",
            suggested_action: "review_cut_candidate",
            evidence: format!("matched filler token {:?}", finding.text),
            requires_approval: true,
        });
    }

    let false_starts = crate::tools::find_false_starts::scan_false_starts(
        &ctx.project_root,
        &project.timeline,
        max_results,
    );
    if false_starts.is_empty() {
        missing_evidence.push("no timeline-visible false-start findings from whisper words".into());
    }
    for finding in false_starts {
        review_cuts.push(CleanupCandidate {
            kind: "false_start",
            asset_id: finding.asset_id,
            source_start_s: finding.source_start_s,
            source_end_s: finding.source_end_s,
            timeline_start_s: finding.timeline_start_s,
            timeline_end_s: finding.timeline_end_s,
            confidence: 0.75,
            risk: "medium",
            suggested_action: "review_cut_candidate",
            evidence: format!(
                "restart marker {:?}; snippet={:?}",
                finding.marker, finding.snippet
            ),
            requires_approval: true,
        });
    }

    let status = if safe_cuts.is_empty() && review_cuts.is_empty() {
        "no_candidates"
    } else if missing_evidence.is_empty() {
        "ready"
    } else {
        "partial"
    };
    let summary_for_agent = format!(
        "Cleanup status: {status}. Found {} safe cut(s), {} review cut(s), and {} risky cut(s).",
        safe_cuts.len(),
        review_cuts.len(),
        risky_cuts.len()
    );
    let report = CleanupReport {
        status,
        summary_for_agent,
        safe_cuts,
        review_cuts,
        risky_cuts,
        missing_evidence,
    };
    serde_json::to_string(&report).map_err(|e| format!("podcast_cleanup_candidates serialize: {e}"))
}

pub const DESCRIPTION: &str = "\
Aggregate existing podcast cleanup evidence into safe/review/risky \
candidate buckets. Uses current dead-air, filler-word, and false-start \
scanners; it does not mutate the timeline and does not require a new \
audio-analysis indexer.\
";
