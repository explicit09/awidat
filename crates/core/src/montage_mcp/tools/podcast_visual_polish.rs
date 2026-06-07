//! `podcast_visual_polish` — forced visual/multicam planning pass.
//! Ported from `crates/core/src/tools/podcast_visual_polish.rs` to the
//! in-process MCP server.

use std::path::Path;

use montage_index::walk_indexer;
use montage_proto::montage_meta::TimelineMarkerCategory;
use montage_proto::otio::{MediaReference, StackChild, Track, TrackChild, TrackKind};
use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::editorial_skills::{EditorialSkillRegistry, opportunity_json};
use crate::montage_mcp::context::McpToolCtx;

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
        .montage
        .as_ref()
        .and_then(|meta| meta.broadcast_overlay.as_ref())
        .is_some();
    let broll_recommendation_count = project
        .timeline
        .metadata
        .montage
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
        "Call plan_visual_support_proposals for transcript/topic moments that need quote highlights, retention lists, chapter cards, search bars, counters, maps, or source-backed B-roll.",
        "Review returned editorial_skill, evidence, export_intent, missing_information, and verification before applying any visual-support EDL.",
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
    let editorial_skill_opportunities = visual_polish_opportunities(
        &project,
        &ctx.project_root,
        !has_broadcast_overlay,
        broll_recommendation_count == 0,
        timeline_health.hard_cut_broll_overlay_count > 0,
        caption_summary.caption_overlay_count > 0,
    );
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
        "editorial_skill_opportunities": editorial_skill_opportunities,
        "visual_support_router": {
            "tool": "plan_visual_support",
            "proposal_tool": "plan_visual_support_proposals",
            "gate": "after story map and cleanup, before final render",
            "editorial_skills": [
                "retention-list-opener",
                "quote-highlight",
                "search-bar-sequence",
                "source-backed-broll",
                "route-map",
                "statistic-counter",
                "podcast-hook",
                "chapter-intro",
                "short-form-reframing"
            ],
            "rule": "route each visual need through editorial skills before choosing b-roll, MotionScene, title/annotation, timeline edit, or effects"
        },
        "required_before_render": true,
    });
    serde_json::to_string(&body).map_err(|e| format!("podcast_visual_polish serialize: {e}"))
}

fn visual_polish_opportunities(
    project: &Project,
    project_root: &Path,
    needs_chapter_cards: bool,
    needs_broll_review: bool,
    has_hard_cut_broll: bool,
    has_captions: bool,
) -> Vec<serde_json::Value> {
    let mut signals = Vec::<(&'static str, String, f64, f64)>::new();
    if needs_chapter_cards {
        signals.push((
            "topic_shift",
            "Podcast visual polish needs chapter title cards or lower thirds before final render"
                .into(),
            0.0,
            4.0,
        ));
    }
    if needs_broll_review {
        signals.push((
            "weak_visual",
            "Podcast visual polish found no stored B-roll recommendation package".into(),
            0.0,
            4.0,
        ));
    }
    if has_hard_cut_broll {
        signals.push((
            "weak_visual",
            "Existing B-roll overlays have hard in/out edges and need motivated visual support"
                .into(),
            0.0,
            4.0,
        ));
    }
    if has_captions {
        signals.push((
            "topic_shift",
            "Captioned podcast sections may need chapter or topic-support overlays".into(),
            0.0,
            4.0,
        ));
    }
    let placements = asset_timeline_placements(project);
    signals.extend(timeline_marker_story_signals(project));
    signals.extend(topic_index_story_signals(project_root, &placements));
    signals.extend(editorial_moment_story_signals(project_root, &placements));
    signals.extend(whisper_transcript_story_signals(project_root, &placements));
    signals.extend(weak_shot_story_signals(project_root, &placements));
    EditorialSkillRegistry::bundled()
        .story_opportunities(deduplicated_story_signals(signals))
        .iter()
        .map(opportunity_json)
        .collect()
}

/// Where one clip of an asset sits on the timeline: the clip's source-media
/// slice `[source_start_s, source_end_s)` and the timeline second at which
/// that slice begins.
#[derive(Clone, Copy)]
struct ClipPlacement {
    source_start_s: f64,
    source_end_s: f64,
    timeline_offset_s: f64,
}

/// Map every external asset id to the clip placements that reference it.
/// Sidecars index source media, so their times are asset-local; this lets us
/// translate them into timeline times. One asset can appear in several clips
/// (different trims / positions), so each maps to a list.
fn asset_timeline_placements(
    project: &Project,
) -> std::collections::HashMap<String, Vec<ClipPlacement>> {
    let mut placements: std::collections::HashMap<String, Vec<ClipPlacement>> =
        std::collections::HashMap::new();
    for stack_child in &project.timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let mut track_cursor_s = 0.0_f64;
        for tc in &track.children {
            match tc {
                TrackChild::Clip(clip) => {
                    let Some(range) = clip.source_range.as_ref() else {
                        continue;
                    };
                    let duration_s = range.duration.to_seconds();
                    if let MediaReference::External(ext) = &clip.media_reference {
                        let source_start_s = range.start_time.to_seconds();
                        placements
                            .entry(ext.target_url.clone())
                            .or_default()
                            .push(ClipPlacement {
                                source_start_s,
                                source_end_s: source_start_s + duration_s,
                                timeline_offset_s: track_cursor_s,
                            });
                    }
                    track_cursor_s += duration_s;
                }
                TrackChild::Gap(gap) => {
                    track_cursor_s += gap.source_range.duration.to_seconds();
                }
                TrackChild::Transition(_) | TrackChild::Stack(_) => {}
            }
        }
    }
    placements
}

/// Translate an asset-local `[source_start_s, source_end_s]` range into
/// timeline seconds using the clip placements for `asset`.
///
/// Returns one `(timeline_start_s, timeline_end_s)` per clip whose source
/// slice overlaps the range (the same asset can be cut into the timeline more
/// than once). When the asset is not on the timeline at all, falls back to the
/// raw source times so opportunities are still surfaced — they just can't be
/// timeline-anchored without a placement.
fn translate_to_timeline(
    placements: &std::collections::HashMap<String, Vec<ClipPlacement>>,
    asset: &str,
    source_start_s: f64,
    source_end_s: f64,
) -> Vec<(f64, f64)> {
    let Some(clips) = placements.get(asset) else {
        return vec![(source_start_s, source_end_s)];
    };
    let mut out = Vec::new();
    for clip in clips {
        if source_end_s <= clip.source_start_s || source_start_s >= clip.source_end_s {
            continue;
        }
        let start = clip.timeline_offset_s + (source_start_s - clip.source_start_s);
        let end = clip.timeline_offset_s + (source_end_s - clip.source_start_s);
        out.push((start, end));
    }
    out
}

fn deduplicated_story_signals(
    signals: Vec<(&'static str, String, f64, f64)>,
) -> Vec<(&'static str, String, f64, f64)> {
    let mut deduped = Vec::new();
    for signal in signals {
        if deduped
            .iter()
            .any(|existing| story_signals_overlap(existing, &signal))
        {
            continue;
        }
        deduped.push(signal);
    }
    deduped
}

fn story_signals_overlap(
    a: &(&'static str, String, f64, f64),
    b: &(&'static str, String, f64, f64),
) -> bool {
    a.0 == b.0
        && normalize_story_label(&a.1) == normalize_story_label(&b.1)
        && time_ranges_overlap(a.2, a.3, b.2, b.3)
}

fn normalize_story_label(label: &str) -> String {
    label
        .split_whitespace()
        .map(|part| {
            part.trim_matches(|c: char| !c.is_alphanumeric())
                .to_ascii_lowercase()
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn time_ranges_overlap(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> bool {
    let start = a_start.max(b_start);
    let end = a_end.min(b_end);
    end > start
}

fn editorial_moment_story_signals(
    project_root: &Path,
    placements: &std::collections::HashMap<String, Vec<ClipPlacement>>,
) -> Vec<(&'static str, String, f64, f64)> {
    let Ok(sidecars) = walk_indexer(project_root, "editorial-moments") else {
        return Vec::new();
    };
    let mut signals = Vec::new();
    for (asset, sidecar) in sidecars {
        let Some(moments) = sidecar_array(&sidecar, "moments") else {
            continue;
        };
        for moment in moments {
            let Some(start_s) = json_f64(moment, &["start_s", "start"]) else {
                continue;
            };
            if !start_s.is_finite() || start_s < 0.0 {
                continue;
            }
            let end_s = json_f64(moment, &["end_s", "end"])
                .filter(|end_s| end_s.is_finite() && *end_s > start_s)
                .unwrap_or(start_s + 4.0);
            let label = json_string(
                moment,
                &[
                    "note",
                    "transcript_excerpt",
                    "text",
                    "label",
                    "summary",
                    "kind",
                ],
            );
            let Some(label) = label
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
            else {
                continue;
            };
            let kind = moment
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .map(moment_story_signal_kind)
                .unwrap_or_else(|| transcript_story_signal_kind(label).unwrap_or("beat"));
            for (timeline_start_s, timeline_end_s) in
                translate_to_timeline(placements, &asset, start_s, end_s)
            {
                signals.push((kind, label.to_string(), timeline_start_s, timeline_end_s));
            }
        }
    }
    signals
}

fn whisper_transcript_story_signals(
    project_root: &Path,
    placements: &std::collections::HashMap<String, Vec<ClipPlacement>>,
) -> Vec<(&'static str, String, f64, f64)> {
    let Ok(sidecars) = walk_indexer(project_root, "whisper") else {
        return Vec::new();
    };
    let mut signals = Vec::new();
    for (asset, sidecar) in sidecars {
        let Some(segments) = sidecar_array(&sidecar, "segments") else {
            continue;
        };
        for segment in segments {
            let Some(text) = json_string(segment, &["text", "transcript", "caption"]) else {
                continue;
            };
            let text = text.trim();
            let Some(kind) = transcript_story_signal_kind(text) else {
                continue;
            };
            let Some(start_s) = json_f64(segment, &["start_s", "start"]) else {
                continue;
            };
            if !start_s.is_finite() || start_s < 0.0 {
                continue;
            }
            let end_s = json_f64(segment, &["end_s", "end"])
                .filter(|end_s| end_s.is_finite() && *end_s > start_s)
                .unwrap_or(start_s + 4.0);
            for (timeline_start_s, timeline_end_s) in
                translate_to_timeline(placements, &asset, start_s, end_s)
            {
                signals.push((kind, text.to_string(), timeline_start_s, timeline_end_s));
            }
        }
    }
    signals
}

fn weak_shot_story_signals(
    project_root: &Path,
    placements: &std::collections::HashMap<String, Vec<ClipPlacement>>,
) -> Vec<(&'static str, String, f64, f64)> {
    let Ok(sidecars) = walk_indexer(project_root, "shot") else {
        return Vec::new();
    };
    let mut signals = Vec::new();
    for (asset, sidecar) in sidecars {
        let Some(shots) = sidecar_array(&sidecar, "shots") else {
            continue;
        };
        for shot in shots {
            if !is_weak_visual_shot(shot) {
                continue;
            }
            let Some(start_s) = json_f64(shot, &["start_s", "start"]) else {
                continue;
            };
            if !start_s.is_finite() || start_s < 0.0 {
                continue;
            }
            let end_s = json_f64(shot, &["end_s", "end"])
                .filter(|end_s| end_s.is_finite() && *end_s > start_s)
                .unwrap_or(start_s + 4.0);
            let label = json_string(shot, &["label", "description", "type"])
                .unwrap_or_else(|| "low-value shot".to_string());
            for (timeline_start_s, timeline_end_s) in
                translate_to_timeline(placements, &asset, start_s, end_s)
            {
                signals.push((
                    "weak_visual",
                    format!("Weak visual shot: {label}"),
                    timeline_start_s,
                    timeline_end_s,
                ));
            }
        }
    }
    signals
}

fn topic_index_story_signals(
    project_root: &Path,
    placements: &std::collections::HashMap<String, Vec<ClipPlacement>>,
) -> Vec<(&'static str, String, f64, f64)> {
    let Ok(sidecars) = walk_indexer(project_root, "topic") else {
        return Vec::new();
    };
    let mut signals = Vec::new();
    for (asset, sidecar) in sidecars {
        let Some(topics) = sidecar
            .get("data")
            .and_then(|data| data.get("topics"))
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for topic in topics {
            let Some(label) = topic
                .get("label")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|label| !label.is_empty())
            else {
                continue;
            };
            let Some(start_s) = topic.get("start_s").and_then(serde_json::Value::as_f64) else {
                continue;
            };
            if !start_s.is_finite() || start_s < 0.0 {
                continue;
            }
            let end_s = topic
                .get("end_s")
                .and_then(serde_json::Value::as_f64)
                .filter(|end_s| end_s.is_finite() && *end_s > start_s)
                .unwrap_or(start_s + 4.0);
            for (timeline_start_s, timeline_end_s) in
                translate_to_timeline(placements, &asset, start_s, end_s)
            {
                signals.push((
                    "topic_shift",
                    label.to_string(),
                    timeline_start_s,
                    timeline_end_s,
                ));
            }
        }
    }
    signals
}

fn timeline_marker_story_signals(project: &Project) -> Vec<(&'static str, String, f64, f64)> {
    let Some(metadata) = project.timeline.metadata.montage.as_ref() else {
        return Vec::new();
    };
    metadata
        .timeline_markers
        .iter()
        .filter_map(|marker| {
            let label = marker.label.trim();
            if label.is_empty() || !marker.time_s.is_finite() || marker.time_s < 0.0 {
                return None;
            }
            let kind = marker_story_signal_kind(marker.category, label);
            let duration_s = marker.duration_s.unwrap_or(4.0).max(0.5);
            Some((
                kind,
                label.to_string(),
                marker.time_s,
                marker.time_s + duration_s,
            ))
        })
        .collect()
}

fn marker_story_signal_kind(category: Option<TimelineMarkerCategory>, label: &str) -> &'static str {
    let lower = label.to_ascii_lowercase();
    if lower.contains("hook") || lower.contains("cold open") {
        "hook"
    } else if lower.contains("b-roll") || lower.contains("broll") || lower.contains("visual") {
        "weak_visual"
    } else if lower.contains("stat")
        || lower.contains('%')
        || lower.chars().any(|c| c.is_ascii_digit())
    {
        "stat"
    } else if lower.contains("map") || lower.contains("route") || lower.contains("location") {
        "map"
    } else if matches!(category, Some(TimelineMarkerCategory::Section)) {
        "topic_shift"
    } else {
        "beat"
    }
}

fn moment_story_signal_kind(kind: &str) -> &'static str {
    let lower = kind.to_ascii_lowercase();
    if lower.contains("hook") || lower.contains("cold_open") {
        "hook"
    } else if lower.contains("quote") {
        "quote"
    } else if lower.contains("stat") || lower.contains("claim") {
        "stat"
    } else if lower.contains("broll") || lower.contains("b-roll") || lower.contains("visual") {
        "weak_visual"
    } else if lower.contains("topic") || lower.contains("chapter") {
        "topic_shift"
    } else {
        "beat"
    }
}

fn transcript_story_signal_kind(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("percent")
        || lower.contains('%')
        || lower.contains('$')
        || lower.contains("growth")
        || lower.chars().any(|c| c.is_ascii_digit())
    {
        Some("stat")
    } else if lower.contains("we will cover")
        || lower.contains("three things")
        || lower.contains("steps")
        || lower.contains("framework")
        || lower.contains("list")
    {
        Some("list")
    } else if lower.contains("search") || lower.contains("query") || lower.contains("google") {
        Some("search")
    } else if lower.contains("route") || lower.contains("map") || lower.contains("location") {
        Some("map")
    } else if lower.contains("quote") {
        Some("quote")
    } else {
        None
    }
}

fn is_weak_visual_shot(shot: &serde_json::Value) -> bool {
    let framing = json_string(shot, &["framing_quality", "quality"]).unwrap_or_default();
    let label = json_string(shot, &["label", "description", "type"]).unwrap_or_default();
    let motion = json_string(shot, &["motion", "motion_label"]).unwrap_or_default();
    let combined = format!("{framing} {label} {motion}").to_ascii_lowercase();
    combined.contains("weak")
        || combined.contains("no visual")
        || combined.contains("low-value")
        || combined.contains("static")
}

fn sidecar_array<'a>(
    sidecar: &'a serde_json::Value,
    key: &str,
) -> Option<&'a Vec<serde_json::Value>> {
    sidecar
        .get("data")
        .and_then(|data| data.get(key))
        .or_else(|| sidecar.get(key))
        .and_then(serde_json::Value::as_array)
}

fn json_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn json_f64(value: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(serde_json::Value::as_f64))
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

fn is_broll_clip(clip: &montage_proto::otio::Clip) -> bool {
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

fn clip_asset_id(clip: &montage_proto::otio::Clip) -> Option<&str> {
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

#[cfg(test)]
mod tests {
    use montage_proto::montage_meta::{TimelineMarker, TimelineMarkerCategory};
    use montage_proto::project::Project;

    use super::{PodcastVisualPolishArgs, run};
    use crate::montage_mcp::context::McpToolCtx;

    #[test]
    fn visual_polish_routes_to_editorial_skill_proposal_gate() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::init(dir.path()).unwrap();
        project.write(dir.path()).unwrap();

        let body = run(
            PodcastVisualPolishArgs {},
            McpToolCtx {
                project_root: dir.path().to_path_buf(),
            },
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            body["visual_support_router"]["proposal_tool"],
            "plan_visual_support_proposals"
        );
        assert_eq!(
            body["visual_support_router"]["gate"],
            "after story map and cleanup, before final render"
        );
        assert!(
            body["visual_support_router"]["editorial_skills"]
                .as_array()
                .unwrap()
                .iter()
                .any(|skill| skill == "source-backed-broll")
        );
        assert!(
            body["editorial_skill_opportunities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|opportunity| {
                    opportunity["primary_skill_id"] == "source-backed-broll"
                        && opportunity["next_tool"] == "plan_visual_support_proposals"
                })
        );
    }

    #[test]
    fn visual_polish_turns_timeline_markers_into_editorial_skill_opportunities() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let metadata = project.timeline.metadata.montage.as_mut().unwrap();
        metadata.timeline_markers.push(TimelineMarker {
            id: "chapter-ai-search".into(),
            label: "Chapter: AI search changes podcast discovery".into(),
            time_s: 42.0,
            duration_s: Some(6.0),
            category: Some(TimelineMarkerCategory::Section),
            source: Some("story_map".into()),
            ..TimelineMarker::default()
        });
        project.write(dir.path()).unwrap();

        let body = run(
            PodcastVisualPolishArgs {},
            McpToolCtx {
                project_root: dir.path().to_path_buf(),
            },
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(
            body["editorial_skill_opportunities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|opportunity| {
                    opportunity["trigger_kind"] == "topic_shift"
                        && opportunity["selection_text"]
                            == "Chapter: AI search changes podcast discovery"
                        && opportunity["timeline_start_s"] == 42.0
                        && opportunity["timeline_end_s"] == 48.0
                        && opportunity["primary_skill_id"] == "chapter-intro"
                })
        );
    }

    #[test]
    fn visual_polish_turns_topic_index_sidecars_into_editorial_skill_opportunities() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::init(dir.path()).unwrap();
        project.write(dir.path()).unwrap();
        let topic_dir = dir.path().join("index/topic/raw");
        std::fs::create_dir_all(&topic_dir).unwrap();
        std::fs::write(
            topic_dir.join("episode.mov.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "data": {
                    "topics": [
                        {"start_s": 12.0, "end_s": 20.0, "label": "AI search changes podcast discovery"}
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let body = run(
            PodcastVisualPolishArgs {},
            McpToolCtx {
                project_root: dir.path().to_path_buf(),
            },
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(
            body["editorial_skill_opportunities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|opportunity| {
                    opportunity["trigger_kind"] == "topic_shift"
                        && opportunity["selection_text"] == "AI search changes podcast discovery"
                        && opportunity["timeline_start_s"] == 12.0
                        && opportunity["timeline_end_s"] == 20.0
                        && opportunity["primary_skill_id"] == "chapter-intro"
                })
        );
    }

    #[test]
    fn visual_polish_deduplicates_overlapping_story_signal_opportunities() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let metadata = project.timeline.metadata.montage.as_mut().unwrap();
        metadata.timeline_markers.push(TimelineMarker {
            id: "chapter-ai-search".into(),
            label: "AI search changes podcast discovery".into(),
            time_s: 12.0,
            duration_s: Some(8.0),
            category: Some(TimelineMarkerCategory::Section),
            source: Some("story_map".into()),
            ..TimelineMarker::default()
        });
        project.write(dir.path()).unwrap();
        let topic_dir = dir.path().join("index/topic/raw");
        std::fs::create_dir_all(&topic_dir).unwrap();
        std::fs::write(
            topic_dir.join("episode.mov.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "data": {
                    "topics": [
                        {"start_s": 13.0, "end_s": 19.0, "label": "AI search changes podcast discovery"}
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let body = run(
            PodcastVisualPolishArgs {},
            McpToolCtx {
                project_root: dir.path().to_path_buf(),
            },
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();

        let matches = body["editorial_skill_opportunities"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|opportunity| {
                opportunity["trigger_kind"] == "topic_shift"
                    && opportunity["selection_text"] == "AI search changes podcast discovery"
                    && opportunity["primary_skill_id"] == "chapter-intro"
            })
            .count();
        assert_eq!(matches, 1);
    }

    #[test]
    fn visual_polish_turns_editorial_moment_sidecars_into_skill_opportunities() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::init(dir.path()).unwrap();
        project.write(dir.path()).unwrap();
        let moment_dir = dir.path().join("index/editorial-moments/raw");
        std::fs::create_dir_all(&moment_dir).unwrap();
        std::fs::write(
            moment_dir.join("episode.mov.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "data": {
                    "moments": [
                        {
                            "kind": "hook",
                            "start_s": 3.0,
                            "end_s": 9.0,
                            "note": "Cold open: the edit almost failed on launch day"
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let body = run(
            PodcastVisualPolishArgs {},
            McpToolCtx {
                project_root: dir.path().to_path_buf(),
            },
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(
            body["editorial_skill_opportunities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|opportunity| {
                    opportunity["trigger_kind"] == "hook"
                        && opportunity["selection_text"]
                            == "Cold open: the edit almost failed on launch day"
                        && opportunity["timeline_start_s"] == 3.0
                        && opportunity["timeline_end_s"] == 9.0
                        && opportunity["primary_skill_id"] == "podcast-hook"
                })
        );
    }

    #[test]
    fn visual_polish_turns_whisper_claims_into_skill_opportunities() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::init(dir.path()).unwrap();
        project.write(dir.path()).unwrap();
        let whisper_dir = dir.path().join("index/whisper/raw");
        std::fs::create_dir_all(&whisper_dir).unwrap();
        std::fs::write(
            whisper_dir.join("episode.mov.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "data": {
                    "segments": [
                        {
                            "text": "Retention improved by 42 percent after the transcript pipeline became stable.",
                            "start_s": 18.0,
                            "end_s": 24.0
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let body = run(
            PodcastVisualPolishArgs {},
            McpToolCtx {
                project_root: dir.path().to_path_buf(),
            },
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(
            body["editorial_skill_opportunities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|opportunity| {
                    opportunity["trigger_kind"] == "stat"
                        && opportunity["selection_text"]
                            == "Retention improved by 42 percent after the transcript pipeline became stable."
                        && opportunity["timeline_start_s"] == 18.0
                        && opportunity["timeline_end_s"] == 24.0
                        && opportunity["primary_skill_id"] == "statistic-counter"
                })
        );
    }

    #[test]
    fn whisper_signal_times_are_translated_to_timeline_for_trimmed_clips() {
        use montage_proto::otio::{
            Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Track,
            TrackChild, TrackKind,
        };

        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();

        // Place raw/episode.mov on the timeline starting 10s into the source
        // media (a trim) at timeline offset 0. A whisper segment at source
        // time 18s therefore sits at timeline 8s.
        let mut clip = Clip::empty("clip-0".to_string());
        clip.media_reference =
            MediaReference::External(ExternalReference::new("raw/episode.mov".to_string()));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(10.0 * 30.0, 30.0),
            RationalTime::new(30.0 * 30.0, 30.0),
        ));
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(track));
        project.write(dir.path()).unwrap();

        let whisper_dir = dir.path().join("index/whisper/raw");
        std::fs::create_dir_all(&whisper_dir).unwrap();
        std::fs::write(
            whisper_dir.join("episode.mov.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "data": {
                    "segments": [
                        {
                            "text": "Retention improved by 42 percent after the transcript pipeline became stable.",
                            "start_s": 18.0,
                            "end_s": 24.0
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let body = run(
            PodcastVisualPolishArgs {},
            McpToolCtx {
                project_root: dir.path().to_path_buf(),
            },
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(
            body["editorial_skill_opportunities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|opportunity| {
                    opportunity["trigger_kind"] == "stat"
                        && opportunity["timeline_start_s"] == 8.0
                        && opportunity["timeline_end_s"] == 14.0
                }),
            "whisper sidecar source times must be translated to timeline times: {body:#}"
        );
    }

    #[test]
    fn visual_polish_turns_weak_shot_sidecars_into_broll_opportunities() {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::init(dir.path()).unwrap();
        project.write(dir.path()).unwrap();
        let shot_dir = dir.path().join("index/shot/raw");
        std::fs::create_dir_all(&shot_dir).unwrap();
        std::fs::write(
            shot_dir.join("episode.mov.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "data": {
                    "shots": [
                        {
                            "start_s": 35.0,
                            "end_s": 43.0,
                            "label": "static wide shot with no visual change",
                            "framing_quality": "weak"
                        }
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let body = run(
            PodcastVisualPolishArgs {},
            McpToolCtx {
                project_root: dir.path().to_path_buf(),
            },
        )
        .unwrap();
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(
            body["editorial_skill_opportunities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|opportunity| {
                    opportunity["trigger_kind"] == "weak_visual"
                        && opportunity["selection_text"]
                            == "Weak visual shot: static wide shot with no visual change"
                        && opportunity["timeline_start_s"] == 35.0
                        && opportunity["timeline_end_s"] == 43.0
                        && opportunity["primary_skill_id"] == "source-backed-broll"
                })
        );
    }
}
