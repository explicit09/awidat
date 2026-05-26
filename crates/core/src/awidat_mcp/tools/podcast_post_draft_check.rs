//! `podcast_post_draft_check` — verify draft episode boundaries.
//! Ported from `crates/core/src/tools/podcast_post_draft_check.rs`
//! to the in-process MCP server.
//!
//! This is the second pass after a draft/extraction exists. It checks the
//! current timeline's first and last transcript-visible moments for leftover
//! pre-show, retake, or post-show chatter before render.

use std::path::{Path, PathBuf};

use awidat_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `podcast_post_draft_check`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PodcastPostDraftCheckArgs {
    /// Seconds to inspect at the start and end of the current draft
    /// timeline. Default 90.
    #[serde(default)]
    pub boundary_window_s: Option<f64>,
}

#[derive(Debug, Serialize)]
struct PostDraftCheckReport {
    status: &'static str,
    summary_for_agent: String,
    requires_tighten_span: bool,
    start_check: BoundaryCheck,
    ending_check: BoundaryCheck,
    missing_evidence: Vec<String>,
}

#[derive(Debug, Serialize)]
struct BoundaryCheck {
    boundary: &'static str,
    inspected_window_s: f64,
    transcript_preview: String,
    issues: Vec<BoundaryIssue>,
}

#[derive(Debug, Serialize)]
struct BoundaryIssue {
    kind: &'static str,
    asset_id: String,
    source_start_s: f64,
    source_end_s: f64,
    timeline_start_s: f64,
    timeline_end_s: f64,
    confidence: f64,
    suggested_action: &'static str,
    evidence: String,
}

#[derive(Debug, Clone)]
struct VisibleTranscript {
    asset_id: String,
    text: String,
    source_start_s: f64,
    source_end_s: f64,
    timeline_start_s: f64,
    timeline_end_s: f64,
}

/// Run `podcast_post_draft_check` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`;
/// project-read errors return `Err(String)`.
pub fn run(args: PodcastPostDraftCheckArgs, ctx: McpToolCtx) -> Result<String, String> {
    let boundary_window_s = args.boundary_window_s.unwrap_or(90.0).clamp(15.0, 240.0);
    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("podcast_post_draft_check: failed to read project: {e}"))?;

    let visible = collect_visible_transcript(&ctx.project_root, &project.timeline);
    let mut missing_evidence = Vec::new();
    if visible.is_empty() {
        missing_evidence.push("no timeline-visible whisper transcript evidence".into());
    }

    let timeline_end_s = visible
        .iter()
        .map(|item| item.timeline_end_s)
        .fold(0.0_f64, f64::max);
    let start_items: Vec<_> = visible
        .iter()
        .filter(|item| item.timeline_start_s <= boundary_window_s)
        .cloned()
        .collect();
    let end_window_start_s = (timeline_end_s - boundary_window_s).max(0.0);
    let end_items: Vec<_> = visible
        .iter()
        .filter(|item| item.timeline_end_s >= end_window_start_s)
        .cloned()
        .collect();

    let start_check = BoundaryCheck {
        boundary: "start",
        inspected_window_s: boundary_window_s,
        transcript_preview: preview_text(&start_items),
        issues: find_boundary_issues("start", &start_items),
    };
    let ending_check = BoundaryCheck {
        boundary: "ending",
        inspected_window_s: boundary_window_s,
        transcript_preview: preview_text(&end_items),
        issues: find_boundary_issues("ending", &end_items),
    };

    let issue_count = start_check.issues.len() + ending_check.issues.len();
    let status = if visible.is_empty() {
        "missing_evidence"
    } else if issue_count == 0 {
        "clean"
    } else {
        "needs_review"
    };
    let report = PostDraftCheckReport {
        status,
        summary_for_agent: format!(
            "Post-draft check status: {status}. Found {issue_count} boundary issue(s). Run this after extraction/cleanup and before final render."
        ),
        requires_tighten_span: issue_count > 0,
        start_check,
        ending_check,
        missing_evidence,
    };
    serde_json::to_string(&report)
        .map_err(|e| format!("podcast_post_draft_check serialize: {e}"))
}

fn collect_visible_transcript(project_root: &Path, timeline: &Timeline) -> Vec<VisibleTranscript> {
    let mut out = Vec::new();
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let mut track_cursor_s = 0.0_f64;
        for child in &track.children {
            match child {
                TrackChild::Clip(clip) => {
                    let MediaReference::External(ext) = &clip.media_reference else {
                        if let Some(range) = clip.source_range.as_ref() {
                            track_cursor_s += range.duration.to_seconds();
                        }
                        continue;
                    };
                    let Some(range) = clip.source_range.as_ref() else {
                        continue;
                    };
                    let asset_id = ext.target_url.clone();
                    let source_start_s = range.start_time.to_seconds();
                    let source_end_s = source_start_s + range.duration.to_seconds();
                    for segment in load_transcript_segments(project_root, &asset_id) {
                        if segment.end_s <= source_start_s || segment.start_s >= source_end_s {
                            continue;
                        }
                        let visible_start = segment.start_s.max(source_start_s);
                        let visible_end = segment.end_s.min(source_end_s);
                        out.push(VisibleTranscript {
                            asset_id: asset_id.clone(),
                            text: segment.text,
                            source_start_s: visible_start,
                            source_end_s: visible_end,
                            timeline_start_s: track_cursor_s + (visible_start - source_start_s),
                            timeline_end_s: track_cursor_s + (visible_end - source_start_s),
                        });
                    }
                    track_cursor_s += range.duration.to_seconds();
                }
                TrackChild::Gap(gap) => {
                    track_cursor_s += gap.source_range.duration.to_seconds();
                }
                TrackChild::Transition(_) | TrackChild::Stack(_) => {}
            }
        }
    }
    out.sort_by(|a, b| {
        a.timeline_start_s
            .partial_cmp(&b.timeline_start_s)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

#[derive(Debug)]
struct TranscriptSegment {
    text: String,
    start_s: f64,
    end_s: f64,
}

fn load_transcript_segments(project_root: &Path, asset_id: &str) -> Vec<TranscriptSegment> {
    let path = whisper_sidecar_path(project_root, asset_id);
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(value): Result<serde_json::Value, _> = serde_json::from_slice(&bytes) else {
        return Vec::new();
    };
    let Some(items) = value
        .pointer("/data/segments")
        .or_else(|| value.pointer("/data/words"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let text = item.get("text").and_then(|v| v.as_str())?.trim();
            if text.is_empty() {
                return None;
            }
            let start_s = item
                .get("start_s")
                .or_else(|| item.get("start"))
                .and_then(|v| v.as_f64())?;
            let end_s = item
                .get("end_s")
                .or_else(|| item.get("end"))
                .and_then(|v| v.as_f64())?;
            (end_s > start_s).then(|| TranscriptSegment {
                text: text.to_string(),
                start_s,
                end_s,
            })
        })
        .collect()
}

fn find_boundary_issues(boundary: &'static str, items: &[VisibleTranscript]) -> Vec<BoundaryIssue> {
    let needles: &[(&str, &str, f64)] = match boundary {
        "start" => &[
            ("are we recording", "pre_show_chatter", 0.9),
            ("mic check", "pre_show_chatter", 0.88),
            ("camera", "pre_show_chatter", 0.65),
            ("start again", "retake_chatter", 0.9),
            ("let me say that again", "retake_chatter", 0.95),
        ],
        _ => &[
            ("are we done", "tail_chatter", 0.95),
            ("that's a wrap", "tail_chatter", 0.9),
            ("that is a wrap", "tail_chatter", 0.9),
            ("stop recording", "tail_chatter", 0.95),
            ("cut", "tail_chatter", 0.72),
            ("off camera", "tail_chatter", 0.8),
        ],
    };
    let mut issues = Vec::new();
    for item in items {
        let lower = item.text.to_ascii_lowercase();
        for (needle, kind, confidence) in needles {
            if lower.contains(needle) {
                issues.push(BoundaryIssue {
                    kind,
                    asset_id: item.asset_id.clone(),
                    source_start_s: item.source_start_s,
                    source_end_s: item.source_end_s,
                    timeline_start_s: item.timeline_start_s,
                    timeline_end_s: item.timeline_end_s,
                    confidence: *confidence,
                    suggested_action: "review_boundary_trim",
                    evidence: item.text.clone(),
                });
                break;
            }
        }
    }
    issues
}

fn preview_text(items: &[VisibleTranscript]) -> String {
    let mut preview = items
        .iter()
        .map(|item| item.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if preview.len() > 500 {
        preview.truncate(500);
        preview.push_str("...");
    }
    preview
}

fn whisper_sidecar_path(project_root: &Path, asset_id: &str) -> PathBuf {
    project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset_id}.json"))
}

pub const DESCRIPTION: &str = "\
Check the current draft timeline's opening and ending transcript windows for \
leftover pre-show, retake, or post-show chatter before render. Run after the \
agent has made an extraction/cleanup draft and before final render. The tool \
does not mutate the timeline.\
";
