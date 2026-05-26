//! `podcast_edit_proposal` — turn cleanup evidence into a gated edit plan.
//! Ported from `crates/core/src/tools/podcast_edit_proposal.rs`.
//!
//! This is deliberately evidence-first. It does not mutate the timeline; it
//! packages existing dead-air, filler, and false-start findings into a reviewable
//! proposal with explicit approval and continuity requirements.

use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `podcast_edit_proposal`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PodcastEditProposalArgs {
    /// Maximum candidates per evidence family. Default 40.
    #[serde(default)]
    pub max_results: Option<usize>,
    /// Minimum silence duration to consider. Default 1.2s.
    #[serde(default)]
    pub dead_air_min_duration_s: Option<f64>,
    /// Include discourse markers like like / you know / i mean. Default
    /// false.
    #[serde(default)]
    pub aggressive_fillers: bool,
}

#[derive(Debug, Serialize)]
struct EditProposalReport {
    status: &'static str,
    summary_for_agent: String,
    proposal_policy: ProposalPolicy,
    safe_edits: Vec<ProposedEdit>,
    review_edits: Vec<ProposedEdit>,
    risky_edits: Vec<ProposedEdit>,
    missing_evidence: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ProposalPolicy {
    apply_mode: &'static str,
    safe_edits: &'static str,
    review_edits: &'static str,
    risky_edits: &'static str,
    approval_prompt: &'static str,
}

#[derive(Debug, Serialize)]
struct ProposedEdit {
    id: String,
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
    requires_user_approval: bool,
    continuity_gate: ContinuityGate,
}

#[derive(Debug, Serialize)]
struct ContinuityGate {
    required_before_apply: bool,
    required_tool: Option<&'static str>,
    tool_args: Option<serde_json::Value>,
    reason: &'static str,
}

pub fn run(args: PodcastEditProposalArgs, ctx: McpToolCtx) -> Result<String, String> {
    let max_results = args.max_results.unwrap_or(40).clamp(1, 200);
    let dead_air_min_duration_s = args.dead_air_min_duration_s.unwrap_or(1.2).max(0.6);

    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("podcast_edit_proposal: failed to read project: {e}"))?;

    let mut safe_edits = Vec::new();
    let mut review_edits = Vec::new();
    let risky_edits = Vec::new();
    let mut missing_evidence = Vec::new();

    let dead_air = crate::tools::find_dead_air::scan_dead_air(
        &ctx.project_root,
        &project.timeline,
        dead_air_min_duration_s,
        max_results,
    );
    if dead_air.is_empty() {
        missing_evidence
            .push("no timeline-visible dead-air findings from silence sidecars".into());
    }
    for (index, finding) in dead_air.into_iter().enumerate() {
        let edit = ProposedEdit {
            id: format!("dead-air-{}", index + 1),
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
            requires_user_approval: finding.duration_s < 2.0,
            continuity_gate: continuity_gate(
                finding.duration_s < 2.0,
                finding.timeline_start_s,
                "trim_out",
                "Medium-risk silence trims can change pacing or remove intentional beats.",
            ),
        };
        if edit.requires_user_approval {
            review_edits.push(edit);
        } else {
            safe_edits.push(edit);
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
    for (index, finding) in filler_words.into_iter().enumerate() {
        review_edits.push(ProposedEdit {
            id: format!("filler-{}", index + 1),
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
            requires_user_approval: true,
            continuity_gate: continuity_gate(
                true,
                finding.timeline_start_s,
                "cut",
                "Filler-word cuts often land inside a sentence and need continuity review.",
            ),
        });
    }

    let false_starts = crate::tools::find_false_starts::scan_false_starts(
        &ctx.project_root,
        &project.timeline,
        max_results,
    );
    if false_starts.is_empty() {
        missing_evidence
            .push("no timeline-visible false-start findings from whisper words".into());
    }
    for (index, finding) in false_starts.into_iter().enumerate() {
        review_edits.push(ProposedEdit {
            id: format!("false-start-{}", index + 1),
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
            requires_user_approval: true,
            continuity_gate: continuity_gate(
                true,
                finding.timeline_start_s,
                "trim_out",
                "False-start removals can remove setup for the clean retake.",
            ),
        });
    }

    let status = if safe_edits.is_empty() && review_edits.is_empty() {
        "no_candidates"
    } else if missing_evidence.is_empty() {
        "ready"
    } else {
        "partial"
    };
    let report = EditProposalReport {
        status,
        summary_for_agent: format!(
            "Edit proposal status: {status}. {} safe edit(s), {} review edit(s), {} risky edit(s). Ask for approval before applying review/risky items.",
            safe_edits.len(),
            review_edits.len(),
            risky_edits.len()
        ),
        proposal_policy: ProposalPolicy {
            apply_mode: "proposal_first",
            safe_edits: "May be batched after user agrees to the proposed cleanup direction.",
            review_edits: "Require explicit user approval and assess_edit_quality before apply_edl.",
            risky_edits: "Require explicit user approval, assess_edit_quality, and a short rationale before apply_edl.",
            approval_prompt: "Here are the proposed podcast edits grouped by risk. Approve all safe edits, choose review items, or ask for changes before I apply them.",
        },
        safe_edits,
        review_edits,
        risky_edits,
        missing_evidence,
    };
    serde_json::to_string(&report)
        .map_err(|e| format!("podcast_edit_proposal serialize: {e}"))
}

fn continuity_gate(
    required: bool,
    at_s: f64,
    kind: &'static str,
    reason: &'static str,
) -> ContinuityGate {
    ContinuityGate {
        required_before_apply: required,
        required_tool: required.then_some("assess_edit_quality"),
        tool_args: required.then(|| {
            serde_json::json!({
                "at_s": at_s,
                "kind": kind,
                "objective": "tighten"
            })
        }),
        reason,
    }
}

pub const DESCRIPTION: &str = "\
Build a reviewable podcast edit proposal from existing cleanup evidence. \
The tool groups timeline-visible dead air, filler words, and false starts \
into safe/review/risky edits and marks review/risky candidates with the \
continuity gate they must pass before mutation. It does not change the \
timeline.\
";
