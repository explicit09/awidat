//! Timeline-pane Tauri commands. Reads `project.otio.json` and
//! returns a flattened, frontend-friendly view via the protocol
//! crate's [`TimelineSnapshot`].
//!
//! The shape we return is intentionally small — the timeline canvas
//! draws rounded rectangles + a ruler, not the full OTIO graph.
//! Transitions and gaps surface as variants so the canvas can color
//! them differently.

use std::path::Path;

use awidat_desktop_protocol::{TimelineItem, TimelineSnapshot, TimelineTrack};
use awidat_proto::awidat_meta;
use awidat_proto::otio::{MediaReference, StackChild, TrackChild, TrackKind};
use awidat_proto::project::Project;
use tauri::State;

use crate::state::AwidatState;

/// Read `<project>/project.otio.json` and return the flattened
/// timeline view. Empty snapshot when no project loaded or OTIO
/// has no clips.
#[tauri::command]
pub async fn read_timeline(state: State<'_, AwidatState>) -> Result<TimelineSnapshot, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(empty_snapshot()),
    };
    // Project::read is sync; spawn_blocking keeps disk I/O off the
    // runtime threads.
    let snapshot = tokio::task::spawn_blocking(move || -> Result<TimelineSnapshot, String> {
        let project = Project::read(&project_root).map_err(|e| format!("read project: {e}"))?;
        Ok(flatten_timeline_public(&project.timeline, &project_root))
    })
    .await
    .map_err(|e| format!("join: {e}"))??;
    Ok(snapshot)
}

fn empty_snapshot() -> TimelineSnapshot {
    TimelineSnapshot {
        duration_s: 0.0,
        broadcast_overlay: None,
        tracks: Vec::new(),
    }
}

/// Public flattener: walk an OTIO `Timeline` and produce the
/// frontend-friendly `TimelineSnapshot`. Used by `read_timeline`
/// (which loads the timeline from disk) and by the proposal
/// pipeline (which already has the timeline in memory after
/// `apply()`).
pub fn flatten_timeline_public(
    timeline: &awidat_proto::otio::Timeline,
    project_root: &Path,
) -> TimelineSnapshot {
    let mut tracks: Vec<TimelineTrack> = Vec::new();
    let mut max_end_s = 0.0_f64;

    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            // Nested stacks aren't drawn in v1 — they're a multi-cam
            // primitive we'll wire up later.
            continue;
        };
        let kind_str = match track.kind {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
        };
        // Pull the awidat track-role tag if present. Today's only
        // value is "titles" (set by InsertTitle's auto-create).
        let role = track
            .metadata
            .get("awidat_track_role")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let is_titles_track = role.as_deref() == Some("titles");
        let mut items = Vec::with_capacity(track.children.len());
        let mut track_cursor_s = 0.0_f64;

        for (i, child) in track.children.iter().enumerate() {
            match child {
                TrackChild::Clip(clip) => {
                    let duration_s = clip
                        .source_range
                        .as_ref()
                        .map(|r| r.duration.to_seconds())
                        .unwrap_or(0.0);
                    let source_start_s = clip
                        .source_range
                        .as_ref()
                        .map(|r| r.start_time.to_seconds());
                    let asset_id = match &clip.media_reference {
                        MediaReference::External(ext) => {
                            Some(project_root_relative(project_root, &ext.target_url))
                        }
                        MediaReference::Missing(_) => None,
                    };
                    // Proxy path: `None` means "still transcoding" — the
                    // frontend draws its empty-state placeholder.
                    let proxy_path = asset_id.as_deref().and_then(|aid| {
                        crate::commands::media::proxy_path_for_asset_id(project_root, aid)
                    });
                    // Thumbnails dir: `None` while the post-import
                    // [`JobKind::Thumbnails`] job is still pending.
                    let thumbnail_dir = asset_id.as_deref().and_then(|aid| {
                        crate::commands::media::thumbnails_dir_for_asset_id(project_root, aid)
                    });
                    // Waveform sidecar: `None` while the post-import
                    // [`JobKind::Waveform`] job is pending and when the
                    // asset has no audio stream (sidecar exists with
                    // an empty buckets array).
                    let waveform_path = asset_id.as_deref().and_then(|aid| {
                        crate::commands::media::waveform_path_for_asset_id(project_root, aid)
                    });
                    // Prefer clip.metadata.awidat.extra["clip_uuid"];
                    // fall back to display name (the EDL resolver
                    // matches on either, so the fallback round-trips).
                    let clip_uuid = clip
                        .metadata
                        .awidat
                        .as_ref()
                        .and_then(|m| m.extra.get("clip_uuid"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| clip.name.clone());
                    // Per-clip volume/speed live on OTIO Effect nodes
                    // the apply layer stamps. PropertiesPane sliders
                    // and the canvas badge both read these.
                    let volume = clip
                        .effects
                        .iter()
                        .find(|e| e.effect_name == "awidat.volume")
                        .and_then(|e| e.metadata.get("value"))
                        .and_then(|v| v.as_f64());
                    let speed = clip
                        .effects
                        .iter()
                        .find(|e| e.effect_name == "awidat.speed")
                        .and_then(|e| e.metadata.get("factor"))
                        .and_then(|v| v.as_f64());
                    let (fade_in_s, fade_out_s) = clip
                        .effects
                        .iter()
                        .find(|e| e.effect_name == "awidat.audio_fade")
                        .map(|e| {
                            (
                                e.metadata.get("fade_in_s").and_then(|v| v.as_f64()),
                                e.metadata.get("fade_out_s").and_then(|v| v.as_f64()),
                            )
                        })
                        .unwrap_or((None, None));
                    let link_group_id = clip
                        .metadata
                        .awidat
                        .as_ref()
                        .and_then(|m| m.extra.get("link_group_id"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    let color_correction = clip
                        .effects
                        .iter()
                        .find(|e| e.effect_name == "awidat.color_correction")
                        .map(|e| awidat_desktop_protocol::ColorCorrectionStyling {
                            exposure_ev: e.metadata.get("exposure_ev").and_then(|v| v.as_f64()),
                            contrast: e.metadata.get("contrast").and_then(|v| v.as_f64()),
                            saturation: e.metadata.get("saturation").and_then(|v| v.as_f64()),
                            temperature: e.metadata.get("temperature").and_then(|v| v.as_f64()),
                            tint: e.metadata.get("tint").and_then(|v| v.as_f64()),
                            shadows: e.metadata.get("shadows").and_then(|v| v.as_f64()),
                            highlights: e.metadata.get("highlights").and_then(|v| v.as_f64()),
                        });
                    let lut_path = clip
                        .effects
                        .iter()
                        .find(|e| e.effect_name == "awidat.lut")
                        .and_then(|e| e.metadata.get("lut_path"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    // Titles track special handling: clips use their
                    // source_range.start_time as the timeline-time
                    // anchor (rather than the cumulative cursor), and
                    // don't advance the cursor — titles are sparse
                    // overlays, not a contiguous strip.
                    let title = clip
                        .effects
                        .iter()
                        .find(|e| e.effect_name == "awidat.title")
                        .map(|e| awidat_desktop_protocol::TitleStyling {
                            text: e
                                .metadata
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            position: e
                                .metadata
                                .get("position")
                                .and_then(|v| v.as_str())
                                .unwrap_or("center")
                                .to_string(),
                            font_size: e
                                .metadata
                                .get("font_size")
                                .and_then(|v| v.as_u64())
                                .map(|n| n as u32)
                                .unwrap_or(64),
                            color: e
                                .metadata
                                .get("color")
                                .and_then(|v| v.as_str())
                                .unwrap_or("#FFFFFF")
                                .to_string(),
                            font_weight: e
                                .metadata
                                .get("font_weight")
                                .and_then(|v| v.as_str())
                                .unwrap_or("normal")
                                .to_string(),
                            animation: e
                                .metadata
                                .get("animation")
                                .and_then(|v| v.as_str())
                                .unwrap_or("none")
                                .to_string(),
                        });
                    let video_overlay = clip
                        .effects
                        .iter()
                        .find(|e| e.effect_name == "awidat.video_overlay")
                        .map(|e| awidat_desktop_protocol::VideoOverlayStyling {
                            mode: e
                                .metadata
                                .get("mode")
                                .and_then(|v| v.as_str())
                                .unwrap_or("full_frame")
                                .to_string(),
                            corner: e
                                .metadata
                                .get("corner")
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                            scale: e.metadata.get("scale").and_then(|v| v.as_f64()),
                            margin_pct: e.metadata.get("margin_pct").and_then(|v| v.as_f64()),
                        });
                    let item_track_start_s = if is_titles_track {
                        source_start_s.unwrap_or(track_cursor_s)
                    } else {
                        track_cursor_s
                    };
                    items.push(TimelineItem::Clip {
                        index: i,
                        name: clip.name.clone(),
                        clip_uuid,
                        track_start_s: item_track_start_s,
                        duration_s,
                        asset_id,
                        source_start_s,
                        proxy_path,
                        thumbnail_dir,
                        waveform_path,
                        volume,
                        speed,
                        fade_in_s,
                        fade_out_s,
                        link_group_id,
                        has_video: Some(matches!(track.kind, TrackKind::Video)),
                        has_audio: Some(matches!(track.kind, TrackKind::Audio)),
                        color_correction,
                        lut_path,
                        title,
                        video_overlay,
                    });
                    if !is_titles_track {
                        track_cursor_s += duration_s;
                    }
                }
                TrackChild::Gap(gap) => {
                    let duration_s = gap.source_range.duration.to_seconds();
                    items.push(TimelineItem::Gap {
                        index: i,
                        track_start_s: track_cursor_s,
                        duration_s,
                    });
                    track_cursor_s += duration_s;
                }
                TrackChild::Transition(t) => {
                    let duration_s = t.in_offset.to_seconds() + t.out_offset.to_seconds();
                    let in_offset_s = t.in_offset.to_seconds();
                    let out_offset_s = t.out_offset.to_seconds();
                    items.push(TimelineItem::Transition {
                        index: i,
                        // Transition straddles the cut between
                        // surrounding clips. It does NOT advance the
                        // cursor because its time overlaps the
                        // neighboring clips.
                        track_start_s: (track_cursor_s - in_offset_s).max(0.0),
                        duration_s,
                        in_offset_s,
                        out_offset_s,
                        effect_name: t.transition_type.clone(),
                    });
                }
                TrackChild::Stack(_) => {
                    // v1 doesn't draw nested stacks; cursor unchanged.
                }
            }
        }

        if track_cursor_s > max_end_s {
            max_end_s = track_cursor_s;
        }
        tracks.push(TimelineTrack {
            name: track.name.clone(),
            kind: kind_str.into(),
            role: role.clone(),
            audio: audio_controls_for_track(track),
            items,
        });
    }

    // Video tracks render above audio; preserve order within each kind.
    tracks.sort_by_key(|t| if t.kind == "video" { 0 } else { 1 });

    TimelineSnapshot {
        duration_s: max_end_s,
        broadcast_overlay: timeline
            .metadata
            .awidat
            .as_ref()
            .and_then(|m| m.broadcast_overlay.as_ref())
            .map(broadcast_overlay_for_protocol),
        tracks,
    }
}

fn broadcast_overlay_for_protocol(
    config: &awidat_meta::BroadcastOverlayConfig,
) -> awidat_desktop_protocol::BroadcastOverlayConfig {
    awidat_desktop_protocol::BroadcastOverlayConfig {
        enabled: config.enabled,
        template_name: config.template_name.clone(),
        episode_title: config.episode_title.clone(),
        episode_subtitle: config.episode_subtitle.clone(),
        show_name: config.show_name.clone(),
        host_a: broadcast_host_for_protocol(&config.host_a),
        host_b: broadcast_host_for_protocol(&config.host_b),
        sponsors: config.sponsors.clone(),
        topics: config
            .topics
            .iter()
            .map(broadcast_timed_entry_for_protocol)
            .collect(),
        chapters: config
            .chapters
            .iter()
            .map(broadcast_timed_entry_for_protocol)
            .collect(),
        brand_logo_path: config.brand_logo_path.clone(),
        short_form_mode: config.short_form_mode,
        style: broadcast_style_for_protocol(&config.style),
    }
}

fn audio_controls_for_track(
    track: &awidat_proto::otio::Track,
) -> Option<awidat_desktop_protocol::TrackAudioControls> {
    if !matches!(track.kind, TrackKind::Audio) {
        return None;
    }
    let value = track.metadata.get("awidat_audio");
    let role = value
        .and_then(|v| v.get("role"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| default_audio_role(&track.name));
    let volume = value
        .and_then(|v| v.get("volume"))
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    let muted = value
        .and_then(|v| v.get("muted"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let solo = value
        .and_then(|v| v.get("solo"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let ducking =
        value
            .and_then(|v| v.get("ducking"))
            .map(|d| awidat_desktop_protocol::DuckingControls {
                enabled: d.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                amount_db: d.get("amount_db").and_then(|v| v.as_f64()).unwrap_or(-12.0),
                attack_ms: d.get("attack_ms").and_then(|v| v.as_f64()).unwrap_or(80.0),
                release_ms: d
                    .get("release_ms")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(300.0),
            });
    Some(awidat_desktop_protocol::TrackAudioControls {
        role,
        volume,
        muted,
        solo,
        ducking,
    })
}

fn default_audio_role(track_name: &str) -> String {
    let name = track_name.trim().to_ascii_lowercase();
    if name == "a1" || name == "audio 1" {
        "dialogue".into()
    } else if name.contains("music") {
        "music".into()
    } else {
        "sfx".into()
    }
}

fn broadcast_host_for_protocol(
    host: &awidat_meta::BroadcastHost,
) -> awidat_desktop_protocol::BroadcastHost {
    awidat_desktop_protocol::BroadcastHost {
        name: host.name.clone(),
        title: host.title.clone(),
        photo_path: host.photo_path.clone(),
    }
}

fn broadcast_timed_entry_for_protocol(
    entry: &awidat_meta::BroadcastTimedEntry,
) -> awidat_desktop_protocol::BroadcastTimedEntry {
    awidat_desktop_protocol::BroadcastTimedEntry {
        time_seconds: entry.time_seconds,
        text: entry.text.clone(),
    }
}

fn broadcast_style_for_protocol(
    style: &awidat_meta::BroadcastOverlayStyle,
) -> awidat_desktop_protocol::BroadcastOverlayStyle {
    awidat_desktop_protocol::BroadcastOverlayStyle {
        gold_hex: style.gold_hex.clone(),
        gold_light_hex: style.gold_light_hex.clone(),
        cyan_hex: style.cyan_hex.clone(),
        dark_navy_hex: style.dark_navy_hex.clone(),
        title_fade_in_end: style.title_fade_in_end,
        title_fade_out_start: style.title_fade_out_start,
        title_visible_end: style.title_visible_end,
        host_intro_start: style.host_intro_start,
        host_intro_end: style.host_intro_end,
        ticker_sponsor_duration: style.ticker_sponsor_duration,
        ticker_fade_duration: style.ticker_fade_duration,
        ticker_topic_duration: style.ticker_topic_duration,
        chapter_display_duration: style.chapter_display_duration,
        name_bar_height: style.name_bar_height,
        ticker_height: style.ticker_height,
        host_strip_height: style.host_strip_height,
    }
}

/// OTIO `target_url` may be project-relative or absolute. Normalize
/// to project-relative for the frontend (consistent with
/// `list_assets`'s output).
fn project_root_relative(project_root: &Path, target_url: &str) -> String {
    let p = std::path::PathBuf::from(target_url);
    if p.is_absolute() {
        match p.strip_prefix(project_root) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => target_url.to_string(),
        }
    } else {
        target_url.to_string()
    }
}
