//! `podcast_editorial_review_pack` — compact AI editorial evidence
//! packets. Ported from
//! `crates/core/src/tools/podcast_editorial_review_pack.rs` to the
//! in-process MCP server.

use std::path::{Path, PathBuf};

use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

const DEFAULT_MAX_RESULTS: usize = 30;
const HARD_MAX_RESULTS: usize = 100;
const DEFAULT_WINDOW_PADDING_S: f64 = 5.0;
const MAX_WINDOW_PADDING_S: f64 = 20.0;

/// Arguments to `podcast_editorial_review_pack`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PodcastEditorialReviewPackArgs {
    /// Maximum review packets to return. Default 30, hard cap 100.
    #[serde(default)]
    pub max_results: Option<usize>,
    /// Transcript context (seconds) to include before and after each
    /// evidence range. Default 5s, clamped to [1, 20].
    #[serde(default)]
    pub window_padding_s: Option<f64>,
    /// Include timeline-visible silence ranges as review evidence.
    /// Default true.
    #[serde(default)]
    pub include_dead_air: Option<bool>,
}

#[derive(Debug, Serialize)]
struct EditorialReviewPack {
    status: &'static str,
    summary_for_agent: String,
    classification_schema: serde_json::Value,
    agent_instructions: Vec<&'static str>,
    packets: Vec<EditorialReviewPacket>,
    missing_evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct EditorialReviewPacket {
    id: String,
    asset_id: String,
    source_start_s: f64,
    source_end_s: f64,
    timeline_start_s: f64,
    timeline_end_s: f64,
    signals: Vec<String>,
    transcript_before: String,
    transcript_during: String,
    transcript_after: String,
    review_question: String,
}

/// Run `podcast_editorial_review_pack` against the project resolved
/// from [`McpToolCtx`]. Returns the JSON body as `Ok(String)`;
/// project-read errors return `Err(String)`.
pub fn run(
    args: PodcastEditorialReviewPackArgs,
    ctx: McpToolCtx,
) -> Result<String, String> {
    let max_results = args
        .max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, HARD_MAX_RESULTS);
    let window_padding_s = args
        .window_padding_s
        .unwrap_or(DEFAULT_WINDOW_PADDING_S)
        .clamp(1.0, MAX_WINDOW_PADDING_S);
    let include_dead_air = args.include_dead_air.unwrap_or(true);

    let project = Project::read(&ctx.project_root).map_err(|e| {
        format!("podcast_editorial_review_pack: failed to read project: {e}")
    })?;

    let report = build_review_pack(
        &ctx.project_root,
        &project.timeline,
        max_results,
        window_padding_s,
        include_dead_air,
    );
    serde_json::to_string(&report)
        .map_err(|e| format!("serialize editorial review pack: {e}"))
}

fn build_review_pack(
    project_root: &Path,
    timeline: &awidat_proto::otio::Timeline,
    max_results: usize,
    window_padding_s: f64,
    include_dead_air: bool,
) -> EditorialReviewPack {
    let mut packets = Vec::new();
    let mut missing_evidence = Vec::new();

    let false_starts = crate::awidat_mcp::tools::find_false_starts::scan_false_starts(
        project_root,
        timeline,
        max_results,
    );
    if false_starts.is_empty() {
        missing_evidence.push("no false-start or production-aside recall signals".into());
    }
    for finding in false_starts {
        let signals = vec![format!(
            "false_start_or_production_aside:{}",
            finding.marker
        )];
        packets.push(packet_from_range(
            project_root,
            packets.len(),
            finding.asset_id,
            finding.source_start_s,
            finding.source_end_s,
            finding.timeline_start_s,
            finding.timeline_end_s,
            signals,
            window_padding_s,
            "Does this range represent a false start, production/coaching aside, natural speech, or useful content?",
        ));
        if packets.len() >= max_results {
            break;
        }
    }

    if include_dead_air && packets.len() < max_results {
        let dead_air = crate::awidat_mcp::tools::find_dead_air::scan_dead_air(
            project_root,
            timeline,
            0.8,
            max_results - packets.len(),
        );
        if dead_air.is_empty() {
            missing_evidence.push("no timeline-visible silence recall signals".into());
        }
        for finding in dead_air {
            let signals = vec![format!("silence:{:.2}s", finding.duration_s)];
            packets.push(packet_from_range(
                project_root,
                packets.len(),
                finding.asset_id,
                finding.source_start_s,
                finding.source_end_s,
                finding.timeline_start_s,
                finding.timeline_end_s,
                signals,
                window_padding_s,
                "Is this silence dead air to tighten, intentional pacing, a thought boundary, or a risky cut?",
            ));
            if packets.len() >= max_results {
                break;
            }
        }
    }

    let status = if packets.is_empty() {
        "no_packets"
    } else if missing_evidence.is_empty() {
        "ready"
    } else {
        "partial"
    };
    EditorialReviewPack {
        status,
        summary_for_agent: format!(
            "Editorial review pack: {status}. Classify {} packet(s) before proposing cleanup or episode-boundary edits.",
            packets.len()
        ),
        classification_schema: serde_json::json!({
            "decision": ["cut", "keep", "review"],
            "editorial_label": [
                "false_start",
                "production_aside",
                "coaching",
                "setup_or_not_recording",
                "dead_air",
                "natural_pause",
                "publishable_content",
                "episode_boundary",
                "cold_open_candidate",
                "needs_human_review"
            ],
            "confidence": "0.0 to 1.0",
            "reason": "Brief editorial explanation grounded in transcript_before/during/after.",
            "proposed_source_range_s": "Only include when decision is cut or review."
        }),
        agent_instructions: vec![
            "Treat signals as recall evidence, not truth.",
            "Use transcript_before, transcript_during, and transcript_after to decide editorial meaning.",
            "Never apply a cut directly from this pack; first state the classification and why.",
            "Silence alone is not a cut. Classify whether the pause is dead air, pacing, a thought boundary, or risky.",
            "For podcast starts, prefer publishable conversational intent over literal phrases like welcome.",
        ],
        packets,
        missing_evidence,
    }
}

#[allow(clippy::too_many_arguments)]
fn packet_from_range(
    project_root: &Path,
    index: usize,
    asset_id: String,
    source_start_s: f64,
    source_end_s: f64,
    timeline_start_s: f64,
    timeline_end_s: f64,
    signals: Vec<String>,
    window_padding_s: f64,
    review_question: &'static str,
) -> EditorialReviewPacket {
    let transcript = read_transcript_window(
        project_root,
        &asset_id,
        source_start_s,
        source_end_s,
        window_padding_s,
    );
    EditorialReviewPacket {
        id: format!("editorial-review-{index:03}"),
        asset_id,
        source_start_s,
        source_end_s,
        timeline_start_s,
        timeline_end_s,
        signals,
        transcript_before: transcript.before,
        transcript_during: transcript.during,
        transcript_after: transcript.after,
        review_question: review_question.into(),
    }
}

#[derive(Debug, Default)]
struct TranscriptWindow {
    before: String,
    during: String,
    after: String,
}

fn read_transcript_window(
    project_root: &Path,
    asset_id: &str,
    start_s: f64,
    end_s: f64,
    padding_s: f64,
) -> TranscriptWindow {
    let path = whisper_sidecar_path(project_root, asset_id);
    let Ok(bytes) = std::fs::read(&path) else {
        return TranscriptWindow::default();
    };
    let Ok(value): Result<serde_json::Value, _> = serde_json::from_slice(&bytes) else {
        return TranscriptWindow::default();
    };
    let Some(items) = value
        .pointer("/data/words")
        .or_else(|| value.pointer("/data/segments"))
        .and_then(|v| v.as_array())
    else {
        return TranscriptWindow::default();
    };

    let before_start = start_s - padding_s;
    let after_end = end_s + padding_s;
    let mut before = Vec::new();
    let mut during = Vec::new();
    let mut after = Vec::new();

    for item in items {
        let text = item
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            continue;
        }
        let item_start = item_start_s(item);
        let item_end = item_end_s(item);
        if !item_start.is_finite() || !item_end.is_finite() {
            continue;
        }
        if item_end < before_start || item_start > after_end {
            continue;
        }
        if item_end <= start_s {
            before.push(text);
        } else if item_start >= end_s {
            after.push(text);
        } else {
            during.push(text);
        }
    }

    TranscriptWindow {
        before: before.join(" "),
        during: during.join(" "),
        after: after.join(" "),
    }
}

fn whisper_sidecar_path(project_root: &Path, asset_id: &str) -> PathBuf {
    project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset_id}.json"))
}

fn item_start_s(v: &serde_json::Value) -> f64 {
    v.get("start_s")
        .or_else(|| v.get("start"))
        .and_then(|s| s.as_f64())
        .unwrap_or(f64::INFINITY)
}

fn item_end_s(v: &serde_json::Value) -> f64 {
    v.get("end_s")
        .or_else(|| v.get("end"))
        .and_then(|s| s.as_f64())
        .unwrap_or(f64::NEG_INFINITY)
}

pub const DESCRIPTION: &str = "\
Build a compact AI editorial review pack for podcast cleanup and \
episode-shape decisions. The tool collects timeline-visible recall \
signals such as false starts, production/coaching asides, and optional \
silence ranges, then adds before/during/after transcript context and \
an explicit classification schema. It is read-only and does not label \
anything as a final cut. The active agent must classify each packet \
as cut/keep/review before calling proposal or mutation tools.\
";
