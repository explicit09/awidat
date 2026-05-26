//! `podcast_visual_polish` — forced visual/multicam planning pass.
//! Ported from `crates/core/src/tools/podcast_visual_polish.rs` to the
//! in-process MCP server.

use awidat_index::walk_indexer;
use awidat_proto::otio::{MediaReference, StackChild, Track, TrackChild, TrackKind};
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

/// Arguments to `podcast_visual_polish`. The tool takes no arguments;
/// the empty struct keeps the schema shape consistent.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PodcastVisualPolishArgs {}

/// Run `podcast_visual_polish` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`.
pub fn run(_args: PodcastVisualPolishArgs, ctx: McpToolCtx) -> Result<String, String> {
    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("podcast_visual_polish: failed to read project: {e}"))?;
    let face_assets = indexed_assets(&ctx.project_root, "face");
    let shot_assets = indexed_assets(&ctx.project_root, "shot");
    let whisper_assets = indexed_assets(&ctx.project_root, "whisper");
    let topic_assets = indexed_assets(&ctx.project_root, "topic");
    let caption_summary = crate::captions::summarize_captions(&project);
    let timeline_health = inspect_timeline_visual_health(&project);
    let has_broadcast_overlay = project
        .timeline
        .metadata
        .awidat
        .as_ref()
        .and_then(|meta| meta.broadcast_overlay.as_ref())
        .is_some();
    let broll_recommendation_count = project
        .timeline
        .metadata
        .awidat
        .as_ref()
        .and_then(|meta| meta.broll_recommendations.as_ref())
        .map(|package| {
            package
                .assets
                .iter()
                .map(|asset| asset.recommendations.len())
                .sum::<usize>()
        })
        .unwrap_or(0);

    let mut issues = Vec::new();
    let mut recommendations = Vec::new();
    if whisper_assets.is_empty() {
        issues.push(serde_json::json!({
            "kind": "missing_transcript",
            "severity": "error",
            "message": "Visual polish needs transcript/speaker evidence for angle and chapter planning."
        }));
    }
    if face_assets.is_empty() || shot_assets.is_empty() {
        issues.push(serde_json::json!({
            "kind": "missing_multicam_evidence",
            "severity": "warning",
            "face_asset_count": face_assets.len(),
            "shot_asset_count": shot_assets.len(),
            "message": "Run face and shot indexing before trusting speaker angle/reaction planning."
        }));
    }
    if topic_assets.is_empty() {
        issues.push(serde_json::json!({
            "kind": "missing_chapter_evidence",
            "severity": "warning",
            "message": "No topic index found; chapter/title-card planning should use transcript/story-map evidence."
        }));
    }
    if !has_broadcast_overlay {
        recommendations.push("Plan lower thirds and chapter/title cards before final render.");
    }
    if broll_recommendation_count == 0 {
        issues.push(serde_json::json!({
            "kind": "broll_review_missing",
            "severity": "warning",
            "message": "No stored B-roll recommendation package found; run read_broll_recommendations/find_broll_opportunities or explicitly report B-roll as skipped before calling the podcast done."
        }));
    }
    if timeline_health.video_audio_duration_delta_s > 2.0 {
        issues.push(serde_json::json!({
            "kind": "av_duration_mismatch",
            "severity": "warning",
            "base_video_duration_s": round3(timeline_health.base_video_duration_s),
            "primary_audio_duration_s": round3(timeline_health.primary_audio_duration_s),
            "delta_s": round3(timeline_health.video_audio_duration_delta_s),
            "message": "Primary video and audio track durations differ materially; verify linked A/V before render because preview and export may diverge."
        }));
    }
    if timeline_health.hard_cut_broll_overlay_count > 0 {
        issues.push(serde_json::json!({
            "kind": "broll_overlay_hard_cuts",
            "severity": "warning",
            "count": timeline_health.hard_cut_broll_overlay_count,
            "message": "One or more full-frame B-roll overlays have hard in/out edges. Add explicit fade/transition treatment or mark the hard cut as intentional before calling visual polish done."
        }));
    }
    if caption_summary.caption_overlay_count > 0
        && caption_summary.missing_safe_area_caption_overlay_count > 0
    {
        issues.push(serde_json::json!({
            "kind": "caption_safe_area_missing",
            "severity": "warning",
            "missing_count": caption_summary.missing_safe_area_caption_overlay_count,
            "message": "Caption overlays are missing safe-area metadata."
        }));
    }
    recommendations.extend([
        "Call plan_visual_support before choosing b-roll, titles, MotionScene, or direct effects for each visual need.",
        "When plan_visual_support returns motion_scene, call plan_motion_scene and apply its Set Motion Scene EDL before claiming native motion graphics are planned.",
        "Run plan_multicam or produce an angle plan with minimum hold duration; avoid switching on short backchannels.",
        "Use broll_candidates/find_broll_opportunities for jump-cut cover and visual examples.",
        "Plan reaction shots around emotional peaks, jokes, and strong claims.",
    ]);
    let status = if issues.iter().any(|issue| issue["severity"] == "error") {
        "needs_fix"
    } else if issues.is_empty() {
        "ready"
    } else {
        "needs_review"
    };
    let body = serde_json::json!({
        "status": status,
        "summary_for_agent": format!("Visual polish status: {status}. {} issue(s), {} recommendation(s).", issues.len(), recommendations.len()),
        "evidence": {
            "whisper_asset_count": whisper_assets.len(),
            "face_asset_count": face_assets.len(),
            "shot_asset_count": shot_assets.len(),
            "topic_asset_count": topic_assets.len(),
            "broll_recommendation_count": broll_recommendation_count,
            "broll_overlay_count": timeline_health.broll_overlay_count,
            "broll_overlay_windows": timeline_health.broll_overlay_windows,
            "base_video_duration_s": round3(timeline_health.base_video_duration_s),
            "primary_audio_duration_s": round3(timeline_health.primary_audio_duration_s),
            "video_audio_duration_delta_s": round3(timeline_health.video_audio_duration_delta_s),
            "caption_summary": caption_summary,
            "has_broadcast_overlay": has_broadcast_overlay
        },
        "issues": issues,
        "recommendations": recommendations,
        "visual_support_router": {
            "tool": "plan_visual_support",
            "motion_scene_followup": "plan_motion_scene",
            "rule": "route each visual need before choosing b-roll, MotionScene, title/annotation, timeline edit, or effects"
        },
        "required_before_render": true,
    });
    serde_json::to_string(&body)
        .map_err(|e| format!("podcast_visual_polish serialize: {e}"))
}

fn indexed_assets(project_root: &std::path::Path, indexer: &str) -> Vec<String> {
    walk_indexer(project_root, indexer)
        .map(|iter| iter.map(|(asset, _)| asset).collect())
        .unwrap_or_default()
}

#[derive(Default)]
struct TimelineVisualHealth {
    base_video_duration_s: f64,
    primary_audio_duration_s: f64,
    video_audio_duration_delta_s: f64,
    broll_overlay_count: usize,
    hard_cut_broll_overlay_count: usize,
    broll_overlay_windows: Vec<serde_json::Value>,
}

fn inspect_timeline_visual_health(project: &Project) -> TimelineVisualHealth {
    let mut health = TimelineVisualHealth::default();
    let mut seen_base_video = false;
    let mut seen_audio = false;
    for stack_child in &project.timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let duration_s = track_duration_s(track);
        match track.kind {
            TrackKind::Video if !seen_base_video => {
                health.base_video_duration_s = duration_s;
                seen_base_video = true;
            }
            TrackKind::Video => {
                collect_broll_overlay_windows(track, &mut health);
            }
            TrackKind::Audio if !seen_audio => {
                health.primary_audio_duration_s = duration_s;
                seen_audio = true;
            }
            TrackKind::Audio => {}
        }
    }
    if seen_base_video && seen_audio {
        health.video_audio_duration_delta_s =
            (health.base_video_duration_s - health.primary_audio_duration_s).abs();
    }
    health
}

fn collect_broll_overlay_windows(track: &Track, health: &mut TimelineVisualHealth) {
    let mut cursor_s = 0.0;
    for (index, child) in track.children.iter().enumerate() {
        let duration_s = child_duration_s(child);
        if let TrackChild::Clip(clip) = child
            && is_broll_clip(clip)
        {
            health.broll_overlay_count += 1;
            if clip.effects.is_empty() && !has_adjacent_transition(track, index) {
                health.hard_cut_broll_overlay_count += 1;
            }
            health.broll_overlay_windows.push(serde_json::json!({
                "track": track.name,
                "clip": clip.name,
                "asset": clip_asset_id(clip),
                "start_s": round3(cursor_s),
                "end_s": round3(cursor_s + duration_s),
                "duration_s": round3(duration_s),
                "has_effects": !clip.effects.is_empty(),
                "has_adjacent_transition": has_adjacent_transition(track, index),
            }));
        }
        cursor_s += duration_s;
    }
}

fn is_broll_clip(clip: &awidat_proto::otio::Clip) -> bool {
    let name = clip.name.to_ascii_lowercase();
    name.contains("broll")
        || name.contains("b-roll")
        || clip_asset_id(clip)
            .map(|asset| {
                let asset = asset.to_ascii_lowercase();
                asset.contains("/broll/")
                    || asset.contains("/generated/")
                    || asset.contains("broll")
                    || asset.contains("b-roll")
            })
            .unwrap_or(false)
}

fn has_adjacent_transition(track: &Track, index: usize) -> bool {
    index
        .checked_sub(1)
        .and_then(|prev| track.children.get(prev))
        .is_some_and(|child| matches!(child, TrackChild::Transition(_)))
        || track
            .children
            .get(index + 1)
            .is_some_and(|child| matches!(child, TrackChild::Transition(_)))
}

fn clip_asset_id(clip: &awidat_proto::otio::Clip) -> Option<&str> {
    match &clip.media_reference {
        MediaReference::External(reference) => Some(reference.target_url.as_str()),
        MediaReference::Missing(_) => None,
    }
}

fn track_duration_s(track: &Track) -> f64 {
    track.children.iter().map(child_duration_s).sum()
}

fn child_duration_s(child: &TrackChild) -> f64 {
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
            .filter_map(|child| match child {
                StackChild::Track(track) => Some(track_duration_s(track)),
                StackChild::Clip(clip) => clip
                    .source_range
                    .as_ref()
                    .map(|range| range.duration.to_seconds()),
                StackChild::Gap(gap) => Some(gap.source_range.duration.to_seconds()),
                StackChild::Stack(_) => None,
            })
            .fold(0.0, f64::max),
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

pub const DESCRIPTION: &str = "\
Check podcast visual polish readiness: multicam evidence, b-roll planning, \
chapters, lower thirds, and captions.";
