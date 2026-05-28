//! `podcast_qc_report` — pre-render podcast QC gate.
//! Ported from `crates/core/src/tools/podcast_qc_report.rs` to the
//! in-process MCP server.

use awidat_proto::otio::{MediaReference, Stack, StackChild, Track, TrackChild};
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

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
    let body = build_podcast_qc_report(&ctx.project_root, &project);
    serde_json::to_string(&body).map_err(|e| format!("podcast_qc_report serialize: {e}"))
}

fn build_podcast_qc_report(project_root: &std::path::Path, project: &Project) -> serde_json::Value {
    let mut issues = Vec::new();
    collect_timeline_issues(project_root, &project.timeline.tracks, &mut issues);
    let caption_summary = crate::captions::summarize_captions(project);
    for warning in &caption_summary.warnings {
        issues.push(serde_json::json!({
            "kind": "caption_warning",
            "severity": "warning",
            "message": warning,
        }));
    }
    let audio_finishing = crate::professional::derive_audio_finishing_state(&project.timeline);
    if audio_finishing.meters.is_empty() {
        issues.push(serde_json::json!({
            "kind": "audio_metering_missing",
            "severity": "warning",
            "message": "No audio meter readings found; run podcast_audio_polish before final render."
        }));
    }
    if !project
        .timeline
        .metadata
        .awidat
        .as_ref()
        .is_some_and(|meta| meta.cut_boundaries.len() > 0)
    {
        issues.push(serde_json::json!({
            "kind": "cut_intent_missing",
            "severity": "info",
            "message": "No cut-boundary intent metadata found; suspicious cuts may be harder to audit."
        }));
    }
    let status = if issues.iter().any(|issue| issue["severity"] == "error") {
        "blocked"
    } else if issues.iter().any(|issue| issue["severity"] == "warning") {
        "needs_review"
    } else {
        "ready"
    };
    serde_json::json!({
        "status": status,
        "summary_for_agent": format!("Podcast QC status: {status}. {} issue(s) before render.", issues.len()),
        "issues": issues,
        "caption_summary": caption_summary,
        "audio_meter_count": audio_finishing.meters.len(),
        "required_before_render": true,
    })
}

fn collect_timeline_issues(
    project_root: &std::path::Path,
    stack: &Stack,
    issues: &mut Vec<serde_json::Value>,
) {
    collect_primary_av_duration_issue(stack, issues);
    for child in &stack.children {
        match child {
            StackChild::Track(track) => collect_track_issues(project_root, track, issues),
            StackChild::Stack(stack) => collect_timeline_issues(project_root, stack, issues),
            StackChild::Clip(_) | StackChild::Gap(_) => {}
        }
    }
}

fn collect_primary_av_duration_issue(stack: &Stack, issues: &mut Vec<serde_json::Value>) {
    let video = first_track_duration(stack, awidat_proto::otio::TrackKind::Video);
    let audio = first_track_duration(stack, awidat_proto::otio::TrackKind::Audio);
    let (Some((video_name, video_duration_s)), Some((audio_name, audio_duration_s))) =
        (video, audio)
    else {
        return;
    };
    let drift_s = (video_duration_s - audio_duration_s).abs();
    if drift_s > 0.25 {
        issues.push(serde_json::json!({
            "kind": "primary_av_duration_mismatch",
            "severity": "error",
            "video_track": video_name,
            "audio_track": audio_name,
            "video_duration_s": video_duration_s,
            "audio_duration_s": audio_duration_s,
            "drift_s": drift_s,
            "message": "Primary video and audio track durations differ; linked cleanup likely changed only one side of A/V."
        }));
    }
}

fn first_track_duration(
    stack: &Stack,
    kind: awidat_proto::otio::TrackKind,
) -> Option<(String, f64)> {
    for child in &stack.children {
        match child {
            StackChild::Track(track) if track.kind == kind => {
                let duration_s = track.children.iter().map(track_child_duration).sum();
                return Some((track.name.clone(), duration_s));
            }
            StackChild::Stack(stack) => {
                if let Some(found) = first_track_duration(stack, kind) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

fn track_child_duration(child: &TrackChild) -> f64 {
    match child {
        TrackChild::Clip(clip) => clip
            .source_range
            .as_ref()
            .map(|range| range.duration.to_seconds())
            .unwrap_or(0.0),
        TrackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
        TrackChild::Transition(transition) => {
            transition.in_offset.to_seconds() + transition.out_offset.to_seconds()
        }
        TrackChild::Stack(stack) => stack
            .children
            .iter()
            .map(|child| match child {
                StackChild::Clip(clip) => clip
                    .source_range
                    .as_ref()
                    .map(|range| range.duration.to_seconds())
                    .unwrap_or(0.0),
                StackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
                StackChild::Track(track) => track.children.iter().map(track_child_duration).sum(),
                StackChild::Stack(stack) => {
                    first_track_duration(stack, awidat_proto::otio::TrackKind::Video)
                        .map(|(_, duration)| duration)
                        .unwrap_or(0.0)
                }
            })
            .sum(),
    }
}

fn collect_track_issues(
    project_root: &std::path::Path,
    track: &Track,
    issues: &mut Vec<serde_json::Value>,
) {
    let mut cursor_s = 0.0_f64;
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => {
                if let MediaReference::External(reference) = &clip.media_reference {
                    let asset_path = project_root.join(&reference.target_url);
                    if !asset_path.exists() {
                        issues.push(serde_json::json!({
                            "kind": "missing_media",
                            "severity": "error",
                            "track": track.name,
                            "clip": clip.name,
                            "asset": reference.target_url,
                            "message": "Timeline references media that does not exist on disk."
                        }));
                    }
                }
                if !clip.active {
                    issues.push(serde_json::json!({
                        "kind": "inactive_clip",
                        "severity": "warning",
                        "track": track.name,
                        "clip": clip.name,
                        "message": "Inactive clip remains on timeline before render."
                    }));
                }
                if let Some(range) = &clip.source_range {
                    cursor_s += range.duration.to_seconds();
                }
            }
            TrackChild::Gap(gap) => {
                let duration_s = gap.source_range.duration.to_seconds();
                if duration_s > 0.5 {
                    issues.push(serde_json::json!({
                        "kind": "timeline_gap",
                        "severity": "warning",
                        "track": track.name,
                        "timeline_start_s": cursor_s,
                        "duration_s": duration_s,
                        "message": "Timeline contains a visible gap longer than 0.5s."
                    }));
                }
                cursor_s += duration_s;
            }
            TrackChild::Transition(transition) => {
                cursor_s += transition.in_offset.to_seconds() + transition.out_offset.to_seconds();
            }
            TrackChild::Stack(stack) => collect_timeline_issues(project_root, stack, issues),
        }
    }
}

pub const DESCRIPTION: &str = "\
Run pre-render podcast QC for gaps, missing media, captions, audio readiness, \
and suspicious timeline structure. Returns a status (ready / needs_review / \
blocked), an `issues` list with severity-tagged findings (missing_media, \
inactive_clip, timeline_gap, primary_av_duration_mismatch, caption_warning, \
audio_metering_missing, cut_intent_missing), plus a caption summary and audio \
meter count. Read-only; the gate is informational and does not block the \
render by itself.\
";
