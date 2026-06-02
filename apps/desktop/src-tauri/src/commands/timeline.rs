//! Timeline-pane Tauri commands. Reads `project.otio.json` and
//! returns a flattened, frontend-friendly view via the protocol
//! crate's [`TimelineSnapshot`].
//!
//! The shape we return is intentionally small — the timeline canvas
//! draws rounded rectangles + a ruler, not the full OTIO graph.
//! Transitions and gaps surface as variants so the canvas can color
//! them differently.

use std::path::Path;

use awidat_desktop_protocol::{
    TimelineCutBoundary, TimelineItem, TimelinePreviewLimitation, TimelineSnapshot, TimelineTrack,
};
use awidat_proto::awidat_meta;
use awidat_proto::otio::{MediaReference, StackChild, TrackChild, TrackKind};
use awidat_proto::professional::{
    AnimationTarget, Easing, ExtrapolationMode, KeyframeInterpolation, MotionScene,
    MotionSceneLayer, MotionSceneLayerKind, MotionSceneTransform, ParameterAnimation,
    is_runtime_clip_parameter,
};
use awidat_proto::project::Project;
use awidat_proto::transitions::{
    SemanticTransitionSpec, TransitionAudioPolicy, resolve_audio_policy,
};
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
        cut_boundaries: Vec::new(),
        preview_limitations: Vec::new(),
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
    let parameter_animations: &[ParameterAnimation] = timeline
        .metadata
        .awidat
        .as_ref()
        .map(|meta| meta.parameter_animations.as_slice())
        .unwrap_or(&[]);

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
        // Largest *clip* end on this track. We use this — not the raw
        // cursor — to compute timeline duration so trailing gaps left
        // over from earlier edits don't inflate the displayed length
        // past the last real content.
        let mut track_last_clip_end_s = 0.0_f64;

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
                    // Prefer proxies when they exist, but keep original source
                    // preview available while a proxy is still generating.
                    // Pro editors do the same: high-quality/original playback
                    // when decode is viable, proxies/cache as acceleration.
                    let (playable_path, playable_kind) = match &proxy_path {
                        Some(path) => (
                            Some(path.clone()),
                            awidat_desktop_protocol::PlayableKind::Proxy,
                        ),
                        None if asset_id
                            .as_deref()
                            .is_some_and(|aid| project_root.join(aid).is_file()) =>
                        {
                            (
                                asset_id.as_deref().map(|aid| {
                                    project_root.join(aid).to_string_lossy().into_owned()
                                }),
                                awidat_desktop_protocol::PlayableKind::Source,
                            )
                        }
                        None => (None, awidat_desktop_protocol::PlayableKind::Missing),
                    };
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
                    let animations = parameter_animations
                        .iter()
                        .filter_map(|animation| timeline_animation_for_clip(animation, &clip_uuid))
                        .collect();
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
                    let split_edit = clip
                        .metadata
                        .awidat
                        .as_ref()
                        .and_then(|m| m.split_edit.as_ref());
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
                            reveal: e
                                .metadata
                                .get("reveal")
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
                        playable_path,
                        playable_kind,
                        thumbnail_dir,
                        waveform_path,
                        volume,
                        speed,
                        fade_in_s,
                        fade_out_s,
                        audio_lead_s: split_edit.and_then(|s| s.audio_lead_s),
                        audio_trail_s: split_edit.and_then(|s| s.audio_trail_s),
                        split_edit_reason: split_edit.and_then(|s| s.reason.clone()),
                        split_edit_confidence: split_edit.and_then(|s| s.confidence),
                        link_group_id,
                        has_video: Some(matches!(track.kind, TrackKind::Video)),
                        has_audio: Some(matches!(track.kind, TrackKind::Audio)),
                        color_correction,
                        lut_path,
                        title,
                        video_overlay,
                        motion_shape: None,
                        motion_image: None,
                        animations,
                    });
                    if !is_titles_track {
                        track_cursor_s += duration_s;
                        if track_cursor_s > track_last_clip_end_s {
                            track_last_clip_end_s = track_cursor_s;
                        }
                    } else {
                        let title_end = item_track_start_s + duration_s;
                        if title_end > track_last_clip_end_s {
                            track_last_clip_end_s = title_end;
                        }
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
                    let transition_spec = t.metadata.get("awidat_transition").and_then(|value| {
                        serde_json::from_value::<SemanticTransitionSpec>(value.clone()).ok()
                    });
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
                        transition_id: transition_spec.as_ref().map(|spec| spec.id.clone()),
                        transition_family: transition_spec
                            .as_ref()
                            .and_then(|spec| spec.family.clone()),
                        transition_intent: transition_spec
                            .as_ref()
                            .and_then(|spec| spec.intent.clone()),
                        transition_energy: transition_spec.as_ref().and_then(|spec| spec.energy),
                        transition_direction: transition_spec.and_then(|spec| spec.direction),
                        audio_policy: transition_audio_policy_label(&t.transition_type),
                    });
                }
                TrackChild::Stack(_) => {
                    // v1 doesn't draw nested stacks; cursor unchanged.
                }
            }
        }

        if track_last_clip_end_s > max_end_s {
            max_end_s = track_last_clip_end_s;
        }
        tracks.push(TimelineTrack {
            name: track.name.clone(),
            kind: kind_str.into(),
            role: role.clone(),
            audio: audio_controls_for_track(track),
            items,
        });
    }
    if let Some(metadata) = timeline.metadata.awidat.as_ref() {
        if let Some(track) = motion_scene_preview_track(metadata) {
            let track_end_s = track
                .items
                .iter()
                .filter_map(|item| match item {
                    TimelineItem::Clip {
                        track_start_s,
                        duration_s,
                        ..
                    } => Some(track_start_s + duration_s),
                    _ => None,
                })
                .fold(0.0_f64, f64::max);
            max_end_s = max_end_s.max(track_end_s);
            tracks.push(track);
        }
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
        cut_boundaries: cut_boundaries_for_protocol(timeline),
        preview_limitations: {
            let mut limitations = preview_limitations_for_protocol(&tracks);
            limitations.extend(animation_preview_limitations_for_protocol(
                parameter_animations,
            ));
            if let Some(metadata) = timeline.metadata.awidat.as_ref() {
                limitations.extend(motion_scene_preview_limitations_for_protocol(metadata));
            }
            limitations
        },
        tracks,
    }
}

fn motion_scene_preview_track(
    metadata: &awidat_meta::AwidatTimelineMetadata,
) -> Option<TimelineTrack> {
    let mut items = Vec::new();
    for scene in &metadata.motion_scenes {
        for layer in sorted_motion_scene_layers(scene) {
            let clip_uuid = format!("{}:{}", scene.id, layer.id);
            let (title, motion_shape, motion_image) = match layer.kind {
                MotionSceneLayerKind::Text => {
                    let Some(text) =
                        layer_string_param(layer, "text").filter(|text| !text.trim().is_empty())
                    else {
                        continue;
                    };
                    (
                        Some(awidat_desktop_protocol::TitleStyling {
                            text,
                            position: layer_string_param(layer, "position")
                                .unwrap_or_else(|| "center".into()),
                            font_size: layer_u32_param(layer, "font_size").unwrap_or(64),
                            color: layer_string_param(layer, "color")
                                .unwrap_or_else(|| "#FFFFFF".into()),
                            font_weight: layer_string_param(layer, "font_weight")
                                .unwrap_or_else(|| "normal".into()),
                            animation: motion_scene_animation_name(layer),
                            reveal: layer_string_param(layer, "reveal")
                                .unwrap_or_else(|| "none".into()),
                        }),
                        None,
                        None,
                    )
                }
                MotionSceneLayerKind::Image => {
                    let Some(image) = motion_scene_image_for_protocol(layer) else {
                        continue;
                    };
                    (None, None, Some(image))
                }
                _ => {
                    let Some(shape) = motion_scene_shape_for_protocol(layer) else {
                        continue;
                    };
                    (None, Some(shape), None)
                }
            };
            items.push(TimelineItem::Clip {
                index: items.len(),
                name: format!("MotionScene {}", layer.id),
                clip_uuid: clip_uuid.clone(),
                track_start_s: layer.from_s,
                duration_s: layer.duration_s,
                asset_id: None,
                source_start_s: Some(layer.from_s),
                proxy_path: None,
                playable_path: None,
                playable_kind: awidat_desktop_protocol::PlayableKind::Missing,
                thumbnail_dir: None,
                waveform_path: None,
                volume: None,
                speed: None,
                fade_in_s: None,
                fade_out_s: None,
                audio_lead_s: None,
                audio_trail_s: None,
                split_edit_reason: None,
                split_edit_confidence: None,
                link_group_id: None,
                has_video: Some(false),
                has_audio: Some(false),
                color_correction: None,
                lut_path: None,
                title,
                video_overlay: None,
                motion_shape,
                motion_image,
                animations: motion_scene_layer_animations_for_protocol(layer, &clip_uuid),
            });
        }
    }
    if items.is_empty() {
        None
    } else {
        Some(TimelineTrack {
            name: "Motion Scenes".into(),
            kind: "video".into(),
            role: Some("titles".into()),
            audio: None,
            items,
        })
    }
}

fn motion_scene_preview_limitations_for_protocol(
    metadata: &awidat_meta::AwidatTimelineMetadata,
) -> Vec<TimelinePreviewLimitation> {
    metadata
        .motion_scenes
        .iter()
        .flat_map(|scene| {
            scene.layers.iter().filter_map(move |layer| match layer.kind {
                MotionSceneLayerKind::Text if layer_text_present(layer) => None,
                MotionSceneLayerKind::Text => Some(TimelinePreviewLimitation {
                    kind: "motion_scene_layer_unsupported".into(),
                    message: format!(
                        "MotionScene {} layer {} has no text parameter for live preview.",
                        scene.id, layer.id
                    ),
                }),
                _ if motion_scene_shape_for_protocol(layer).is_some() => None,
                MotionSceneLayerKind::Image if motion_scene_image_for_protocol(layer).is_some() => {
                    None
                }
                _ => Some(TimelinePreviewLimitation {
                    kind: "motion_scene_layer_unsupported".into(),
                    message: format!(
                        "MotionScene {} layer {} is stored but not previewed by the native MotionScene preview yet.",
                        scene.id, layer.id
                    ),
                }),
            })
        })
        .collect()
}

fn sorted_motion_scene_layers(scene: &MotionScene) -> Vec<&MotionSceneLayer> {
    let mut layers = scene.layers.iter().collect::<Vec<_>>();
    layers.sort_by_key(|layer| layer.z_index);
    layers
}

fn layer_text_present(layer: &MotionSceneLayer) -> bool {
    layer_string_param(layer, "text").is_some_and(|text| !text.trim().is_empty())
}

fn layer_string_param(layer: &MotionSceneLayer, key: &str) -> Option<String> {
    layer
        .params
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn layer_u32_param(layer: &MotionSceneLayer, key: &str) -> Option<u32> {
    layer
        .params
        .get(key)
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
}

fn motion_scene_shape_for_protocol(
    layer: &MotionSceneLayer,
) -> Option<awidat_desktop_protocol::MotionShapeStyling> {
    let is_rect = match layer.kind {
        MotionSceneLayerKind::Shape => matches!(
            layer_string_param(layer, "shape").as_deref(),
            Some("rect" | "rectangle")
        ),
        MotionSceneLayerKind::Solid => true,
        _ => false,
    };
    if !is_rect {
        return None;
    }
    let transform = MotionSceneTransform::from_layer_params(&layer.params);
    Some(awidat_desktop_protocol::MotionShapeStyling {
        shape: "rect".into(),
        x: transform.x,
        y: transform.y,
        width: transform.width,
        height: transform.height,
        color: layer_string_param(layer, "color").unwrap_or_else(|| "#FFFFFF".into()),
        opacity: transform.opacity,
        scale: transform.scale,
        anchor_x: transform.anchor_x,
        anchor_y: transform.anchor_y,
        rotation_deg: transform.rotation_deg,
    })
}

fn motion_scene_image_for_protocol(
    layer: &MotionSceneLayer,
) -> Option<awidat_desktop_protocol::MotionImageStyling> {
    if layer.kind != MotionSceneLayerKind::Image {
        return None;
    }
    let asset_id = layer_string_param(layer, "asset")
        .or_else(|| layer_string_param(layer, "asset_id"))
        .filter(|asset| !asset.trim().is_empty())?;
    let transform = MotionSceneTransform::from_layer_params(&layer.params);
    Some(awidat_desktop_protocol::MotionImageStyling {
        asset_id,
        x: transform.x,
        y: transform.y,
        width: transform.width,
        height: transform.height,
        opacity: transform.opacity,
        fit: transform.fit.as_str().into(),
        scale: transform.scale,
        anchor_x: transform.anchor_x,
        anchor_y: transform.anchor_y,
        rotation_deg: transform.rotation_deg,
    })
}

fn motion_scene_layer_animations_for_protocol(
    layer: &MotionSceneLayer,
    clip_uuid: &str,
) -> Vec<awidat_desktop_protocol::TimelineParameterAnimation> {
    layer
        .motion_animations()
        .into_iter()
        .filter(|animation| is_phase_3a_parameter(&animation.parameter))
        .map(
            |animation| awidat_desktop_protocol::TimelineParameterAnimation {
                id: format!("{clip_uuid}:{}", animation.parameter),
                target: awidat_desktop_protocol::TimelineAnimationTarget {
                    clip_id: clip_uuid.to_string(),
                    parameter: animation.parameter,
                },
                keyframes: animation
                    .keyframes
                    .into_iter()
                    .map(|keyframe| awidat_desktop_protocol::TimelineKeyframe {
                        time_s: keyframe.time_s,
                        value: keyframe.value,
                        interpolation: interpolation_name(keyframe.interpolation).to_string(),
                        easing: easing_name(keyframe.easing).to_string(),
                        bezier: keyframe.bezier.map(|handles| {
                            awidat_desktop_protocol::TimelineBezierHandles {
                                out_x: handles.out_x,
                                out_y: handles.out_y,
                                in_x: handles.in_x,
                                in_y: handles.in_y,
                            }
                        }),
                        tangent_mode: tangent_mode_name(keyframe.tangent_mode).to_string(),
                        spring: keyframe.spring.map(|spring| {
                            awidat_desktop_protocol::TimelineSpringParameters {
                                mass: spring.mass,
                                stiffness: spring.stiffness,
                                damping: spring.damping,
                            }
                        }),
                    })
                    .collect(),
                pre_extrapolation: extrapolation_name(animation.pre_extrapolation).to_string(),
                post_extrapolation: extrapolation_name(animation.post_extrapolation).to_string(),
                motion_path: animation.motion_path.as_ref().map(|path| {
                    awidat_desktop_protocol::TimelineMotionPath {
                        points: path
                            .points
                            .iter()
                            .map(|point| awidat_desktop_protocol::TimelineMotionPathPoint {
                                time_s: point.time_s,
                                x: point.x,
                                y: point.y,
                                outgoing_control: point.outgoing_control.map(|control| {
                                    awidat_desktop_protocol::TimelineMotionPathControlPoint {
                                        x: control.x,
                                        y: control.y,
                                    }
                                }),
                                incoming_control: point.incoming_control.map(|control| {
                                    awidat_desktop_protocol::TimelineMotionPathControlPoint {
                                        x: control.x,
                                        y: control.y,
                                    }
                                }),
                            })
                            .collect(),
                    }
                }),
                rationale: None,
            },
        )
        .collect()
}

fn motion_scene_animation_name(layer: &MotionSceneLayer) -> String {
    match layer_string_param(layer, "animation").as_deref() {
        Some("fade_slide_in") => "fade_in".into(),
        Some(value) => value.to_string(),
        None => "none".into(),
    }
}

fn is_phase_3a_parameter(parameter: &str) -> bool {
    is_runtime_clip_parameter(parameter)
}

fn interpolation_name(value: KeyframeInterpolation) -> &'static str {
    match value {
        KeyframeInterpolation::Hold => "hold",
        KeyframeInterpolation::Step => "step",
        KeyframeInterpolation::Linear => "linear",
        KeyframeInterpolation::Bezier => "bezier",
        KeyframeInterpolation::Spring => "spring",
    }
}

fn tangent_mode_name(value: awidat_proto::professional::TangentMode) -> &'static str {
    match value {
        awidat_proto::professional::TangentMode::Auto => "auto",
        awidat_proto::professional::TangentMode::Aligned => "aligned",
        awidat_proto::professional::TangentMode::Broken => "broken",
        awidat_proto::professional::TangentMode::Flat => "flat",
    }
}

fn extrapolation_name(value: ExtrapolationMode) -> &'static str {
    match value {
        ExtrapolationMode::Hold => "hold",
        ExtrapolationMode::Linear => "linear",
    }
}

fn easing_name(value: Easing) -> &'static str {
    match value {
        Easing::Linear => "linear",
        Easing::EaseInSine => "ease_in_sine",
        Easing::EaseOutSine => "ease_out_sine",
        Easing::EaseInOutSine => "ease_in_out_sine",
        Easing::EaseIn => "ease_in",
        Easing::EaseOut => "ease_out",
        Easing::EaseInOut => "ease_in_out",
        Easing::EaseInCubic => "ease_in_cubic",
        Easing::EaseOutCubic => "ease_out_cubic",
        Easing::EaseInOutCubic => "ease_in_out_cubic",
        Easing::EaseInQuart => "ease_in_quart",
        Easing::EaseOutQuart => "ease_out_quart",
        Easing::EaseInOutQuart => "ease_in_out_quart",
        Easing::EaseInQuint => "ease_in_quint",
        Easing::EaseOutQuint => "ease_out_quint",
        Easing::EaseInOutQuint => "ease_in_out_quint",
        Easing::EaseInExpo => "ease_in_expo",
        Easing::EaseOutExpo => "ease_out_expo",
        Easing::EaseInOutExpo => "ease_in_out_expo",
        Easing::EaseInCirc => "ease_in_circ",
        Easing::EaseOutCirc => "ease_out_circ",
        Easing::EaseInOutCirc => "ease_in_out_circ",
        Easing::EaseInBack => "ease_in_back",
        Easing::EaseOutBack => "ease_out_back",
        Easing::EaseInOutBack => "ease_in_out_back",
        Easing::EaseInElastic => "ease_in_elastic",
        Easing::EaseOutElastic => "ease_out_elastic",
        Easing::EaseInOutElastic => "ease_in_out_elastic",
        Easing::EaseInBounce => "ease_in_bounce",
        Easing::EaseOutBounce => "ease_out_bounce",
        Easing::EaseInOutBounce => "ease_in_out_bounce",
    }
}

fn timeline_animation_for_clip(
    animation: &ParameterAnimation,
    clip_id: &str,
) -> Option<awidat_desktop_protocol::TimelineParameterAnimation> {
    let AnimationTarget::ClipParameter {
        clip_id: target_clip_id,
        parameter,
    } = &animation.target
    else {
        return None;
    };
    if target_clip_id != clip_id || !is_phase_3a_parameter(parameter) {
        return None;
    }
    Some(awidat_desktop_protocol::TimelineParameterAnimation {
        id: animation.id.clone(),
        target: awidat_desktop_protocol::TimelineAnimationTarget {
            clip_id: target_clip_id.clone(),
            parameter: parameter.clone(),
        },
        keyframes: animation
            .keyframes
            .iter()
            .map(|keyframe| awidat_desktop_protocol::TimelineKeyframe {
                time_s: keyframe.time_s,
                value: keyframe.value,
                interpolation: interpolation_name(keyframe.interpolation).to_string(),
                easing: easing_name(keyframe.easing).to_string(),
                bezier: keyframe.bezier.map(|handles| {
                    awidat_desktop_protocol::TimelineBezierHandles {
                        out_x: handles.out_x,
                        out_y: handles.out_y,
                        in_x: handles.in_x,
                        in_y: handles.in_y,
                    }
                }),
                tangent_mode: tangent_mode_name(keyframe.tangent_mode).to_string(),
                spring: keyframe.spring.map(|spring| {
                    awidat_desktop_protocol::TimelineSpringParameters {
                        mass: spring.mass,
                        stiffness: spring.stiffness,
                        damping: spring.damping,
                    }
                }),
            })
            .collect(),
        pre_extrapolation: extrapolation_name(animation.pre_extrapolation).to_string(),
        post_extrapolation: extrapolation_name(animation.post_extrapolation).to_string(),
        motion_path: animation.motion_path.as_ref().map(|path| {
            awidat_desktop_protocol::TimelineMotionPath {
                points: path
                    .points
                    .iter()
                    .map(|point| awidat_desktop_protocol::TimelineMotionPathPoint {
                        time_s: point.time_s,
                        x: point.x,
                        y: point.y,
                        outgoing_control: point.outgoing_control.map(|control| {
                            awidat_desktop_protocol::TimelineMotionPathControlPoint {
                                x: control.x,
                                y: control.y,
                            }
                        }),
                        incoming_control: point.incoming_control.map(|control| {
                            awidat_desktop_protocol::TimelineMotionPathControlPoint {
                                x: control.x,
                                y: control.y,
                            }
                        }),
                    })
                    .collect(),
            }
        }),
        rationale: animation.rationale.clone(),
    })
}

fn animation_preview_limitations_for_protocol(
    animations: &[ParameterAnimation],
) -> Vec<TimelinePreviewLimitation> {
    animations
        .iter()
        .filter_map(|animation| match &animation.target {
            AnimationTarget::ClipParameter { parameter, .. } if is_phase_3a_parameter(parameter) => None,
            _ => Some(TimelinePreviewLimitation {
                kind: "animation_target_unsupported".to_string(),
                message: format!(
                    "Animation {} is stored but not previewed/rendered by the Phase 3A motion runtime.",
                    animation.id
                ),
            }),
        })
        .collect()
}

fn preview_limitations_for_protocol(tracks: &[TimelineTrack]) -> Vec<TimelinePreviewLimitation> {
    let has_split_edit_audio = tracks.iter().flat_map(|track| &track.items).any(|item| {
        matches!(
            item,
            TimelineItem::Clip {
                audio_lead_s: Some(lead_s),
                ..
            } if *lead_s > 0.0
        ) || matches!(
            item,
            TimelineItem::Clip {
                audio_trail_s: Some(trail_s),
                ..
            } if *trail_s > 0.0
        )
    });
    let has_transition_audio_policy = tracks.iter().flat_map(|track| &track.items).any(|item| {
        matches!(
            item,
            TimelineItem::Transition {
                audio_policy: Some(_),
                ..
            }
        )
    });

    let mut limitations = Vec::new();
    if has_split_edit_audio {
        limitations.push(TimelinePreviewLimitation {
            kind: "split_edit_audio".into(),
            message: "Live preview shows J/L split-edit intent, but final export is the source of truth for overlapped audio.".into(),
        });
    }
    if has_transition_audio_policy {
        limitations.push(TimelinePreviewLimitation {
            kind: "transition_audio_policy".into(),
            message: "Live preview approximates transition visuals; final export applies the registry audio policy.".into(),
        });
    }
    limitations
}

fn transition_audio_policy_label(kind: &str) -> Option<String> {
    match resolve_audio_policy(kind).ok()? {
        TransitionAudioPolicy::Crossfade => Some("crossfade".into()),
        TransitionAudioPolicy::Cut => Some("cut".into()),
    }
}

fn cut_boundaries_for_protocol(
    timeline: &awidat_proto::otio::Timeline,
) -> Vec<TimelineCutBoundary> {
    let mut boundaries = timeline
        .metadata
        .awidat
        .as_ref()
        .map(|meta| {
            meta.cut_boundaries
                .iter()
                .map(|(key, spec)| {
                    let (from_clip_id, to_clip_id) = key
                        .split_once("::")
                        .map(|(from, to)| (from.to_string(), to.to_string()))
                        .unwrap_or_else(|| (String::new(), String::new()));
                    TimelineCutBoundary {
                        key: key.clone(),
                        from_clip_id,
                        to_clip_id,
                        cut_type: cut_type_for_protocol(spec.cut_type).to_string(),
                        intent: spec.intent.clone(),
                        energy: spec.energy,
                        audio_relation: audio_relation_for_protocol(spec.audio_relation)
                            .to_string(),
                        confidence: spec.confidence,
                        reason: spec.reason.clone(),
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    boundaries.sort_by(|a, b| a.key.cmp(&b.key));
    boundaries
}

fn cut_type_for_protocol(cut_type: awidat_meta::CutType) -> &'static str {
    match cut_type {
        awidat_meta::CutType::HardCut => "hard_cut",
        awidat_meta::CutType::CutOnAction => "cut_on_action",
        awidat_meta::CutType::Cutaway => "cutaway",
        awidat_meta::CutType::Insert => "insert",
        awidat_meta::CutType::EyelineMatch => "eyeline_match",
        awidat_meta::CutType::ShotReverseShot => "shot_reverse_shot",
        awidat_meta::CutType::MatchCut => "match_cut",
        awidat_meta::CutType::SmashCut => "smash_cut",
        awidat_meta::CutType::JumpCut => "jump_cut",
        awidat_meta::CutType::CrossCut => "cross_cut",
        awidat_meta::CutType::JCut => "j_cut",
        awidat_meta::CutType::LCut => "l_cut",
    }
}

fn audio_relation_for_protocol(audio_relation: awidat_meta::AudioRelation) -> &'static str {
    match audio_relation {
        awidat_meta::AudioRelation::Sync => "sync",
        awidat_meta::AudioRelation::AudioLeads => "audio_leads",
        awidat_meta::AudioRelation::AudioTrails => "audio_trails",
        awidat_meta::AudioRelation::Overlap => "overlap",
        awidat_meta::AudioRelation::AudioCut => "audio_cut",
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

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_core::edl::anchor::AnchorContext;
    use awidat_core::edl::apply::apply;
    use awidat_core::edl::parser::parse;
    use awidat_proto::awidat_meta::{AwidatClipMetadata, SplitEditSpec};
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Timeline,
        Track, TrackChild, TrackKind, Transition,
    };
    use awidat_proto::professional::{
        AnimationTarget, BezierHandles, Easing, Keyframe, KeyframeInterpolation, MotionScene,
        MotionSceneLayer, MotionSceneLayerKind, ParameterAnimation,
    };

    #[test]
    fn preview_limitations_surface_split_edits_and_transition_audio_policy() {
        let mut timeline = Timeline::empty("preview-limitations");
        let mut track = Track::empty("V1", TrackKind::Video);
        track
            .children
            .push(TrackChild::Clip(clip_with_split_edit("a", Some(0.4), None)));
        track
            .children
            .push(TrackChild::Transition(Transition::symmetric(
                "awidat.whip_pan_right",
                0.18,
                24.0,
            )));
        track
            .children
            .push(TrackChild::Clip(clip_with_split_edit("b", None, Some(0.5))));
        timeline.tracks.children.push(StackChild::Track(track));

        let snapshot = flatten_timeline_public(&timeline, Path::new("/tmp/project"));
        let limitation_kinds = snapshot
            .preview_limitations
            .iter()
            .map(|limitation| limitation.kind.as_str())
            .collect::<Vec<_>>();

        assert!(limitation_kinds.contains(&"split_edit_audio"));
        assert!(limitation_kinds.contains(&"transition_audio_policy"));
    }

    #[test]
    fn supported_clip_animation_converts_for_protocol() {
        let animation = ParameterAnimation {
            id: "anim-a".to_string(),
            target: AnimationTarget::ClipParameter {
                clip_id: "clip-a".to_string(),
                parameter: "title.opacity".to_string(),
            },
            keyframes: vec![Keyframe {
                time_s: 0.0,
                value: 0.0,
                interpolation: KeyframeInterpolation::Linear,
                easing: Easing::EaseOut,
                bezier: None,
                tangent_mode: Default::default(),
                spring: None,
            }],
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: Some("Fade in".to_string()),
        };

        let Some(converted) = timeline_animation_for_clip(&animation, "clip-a") else {
            panic!("supported title opacity animation should convert");
        };
        assert_eq!(converted.target.parameter, "title.opacity");
        assert_eq!(converted.keyframes[0].easing, "ease_out");
    }

    #[test]
    fn bezier_clip_animation_converts_for_preview() {
        let animation = ParameterAnimation {
            id: "anim-bezier".to_string(),
            target: AnimationTarget::ClipParameter {
                clip_id: "clip-a".to_string(),
                parameter: "title.opacity".to_string(),
            },
            keyframes: vec![Keyframe {
                time_s: 0.0,
                value: 0.0,
                interpolation: KeyframeInterpolation::Bezier,
                easing: Easing::EaseOut,
                bezier: Some(BezierHandles {
                    out_x: 0.25,
                    out_y: 0.1,
                    in_x: 0.25,
                    in_y: 1.0,
                }),
                tangent_mode: Default::default(),
                spring: None,
            }],
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: None,
        };

        let Some(converted) = timeline_animation_for_clip(&animation, "clip-a") else {
            panic!("bezier animation should convert");
        };
        assert_eq!(converted.keyframes[0].interpolation, "bezier");
        assert_eq!(
            converted.keyframes[0].bezier.map(|handles| handles.in_y),
            Some(1.0)
        );
    }

    #[test]
    fn unsupported_clip_animation_is_not_converted() {
        let animation = ParameterAnimation {
            id: "anim-a".to_string(),
            target: AnimationTarget::ClipParameter {
                clip_id: "clip-a".to_string(),
                parameter: "volume".to_string(),
            },
            keyframes: vec![Keyframe::linear(0.0, 1.0)],
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: None,
        };

        assert!(timeline_animation_for_clip(&animation, "clip-a").is_none());
    }

    #[test]
    fn flatten_timeline_attaches_supported_parameter_animations() {
        let mut timeline = Timeline::empty("animation-snapshot");
        timeline.metadata.awidat = Some(awidat_meta::AwidatTimelineMetadata {
            parameter_animations: vec![ParameterAnimation {
                id: "anim-title-opacity".to_string(),
                target: AnimationTarget::ClipParameter {
                    clip_id: "title-1".to_string(),
                    parameter: "title.opacity".to_string(),
                },
                keyframes: vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 1.0)],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
                metadata_only: false,
                rationale: Some("Fade title in".to_string()),
            }],
            ..awidat_meta::AwidatTimelineMetadata::default()
        });
        let mut track = Track::empty("Titles", TrackKind::Video);
        track
            .metadata
            .insert("awidat_track_role".to_string(), serde_json::json!("titles"));
        track
            .children
            .push(TrackChild::Clip(clip_with_uuid("Title", "title-1")));
        timeline.tracks.children.push(StackChild::Track(track));

        let snapshot = flatten_timeline_public(&timeline, Path::new("/tmp/project"));
        let Some(title_track) = snapshot
            .tracks
            .iter()
            .find(|track| track.role.as_deref() == Some("titles"))
        else {
            panic!("titles track should be present");
        };
        let TimelineItem::Clip { animations, .. } = &title_track.items[0] else {
            panic!("expected title clip");
        };

        assert_eq!(animations.len(), 1);
        assert_eq!(animations[0].target.parameter, "title.opacity");
    }

    #[test]
    fn flatten_timeline_surfaces_text_motion_scene_as_preview_title() {
        let mut timeline = Timeline::empty("motion-scene-preview");
        timeline.metadata.awidat = Some(awidat_meta::AwidatTimelineMetadata {
            motion_scenes: vec![MotionScene {
                id: "scene-a".to_string(),
                start_s: 0.0,
                duration_s: 3.0,
                fps: 24.0,
                width: 1920,
                height: 1080,
                layers: vec![MotionSceneLayer {
                    id: "headline".to_string(),
                    kind: MotionSceneLayerKind::Text,
                    from_s: 0.5,
                    duration_s: 2.0,
                    z_index: 10,
                    params: [("text".to_string(), serde_json::json!("Motion Scene"))]
                        .into_iter()
                        .collect(),
                }],
                rationale: None,
            }],
            ..awidat_meta::AwidatTimelineMetadata::default()
        });

        let snapshot = flatten_timeline_public(&timeline, Path::new("/tmp/project"));
        let Some(title_track) = snapshot
            .tracks
            .iter()
            .find(|track| track.role.as_deref() == Some("titles"))
        else {
            panic!("motion scene should surface through a preview title track");
        };
        let TimelineItem::Clip {
            clip_uuid,
            track_start_s,
            duration_s,
            title: Some(title),
            ..
        } = &title_track.items[0]
        else {
            panic!("expected motion scene title clip");
        };

        assert_eq!(clip_uuid, "scene-a:headline");
        assert_eq!(*track_start_s, 0.5);
        assert_eq!(*duration_s, 2.0);
        assert_eq!(title.text, "Motion Scene");
    }

    #[test]
    fn unsupported_motion_scene_preview_layer_surfaces_limitation() {
        let mut timeline = Timeline::empty("motion-scene-preview-limitation");
        timeline.metadata.awidat = Some(awidat_meta::AwidatTimelineMetadata {
            motion_scenes: vec![MotionScene {
                id: "scene-a".to_string(),
                start_s: 0.0,
                duration_s: 3.0,
                fps: 24.0,
                width: 1920,
                height: 1080,
                layers: vec![MotionSceneLayer {
                    id: "shape".to_string(),
                    kind: MotionSceneLayerKind::Shape,
                    from_s: 0.0,
                    duration_s: 3.0,
                    z_index: 1,
                    params: [("shape".to_string(), serde_json::json!("circle"))]
                        .into_iter()
                        .collect(),
                }],
                rationale: None,
            }],
            ..awidat_meta::AwidatTimelineMetadata::default()
        });

        let snapshot = flatten_timeline_public(&timeline, Path::new("/tmp/project"));

        assert!(snapshot.tracks.is_empty());
        assert!(snapshot.preview_limitations.iter().any(|limitation| {
            limitation.kind == "motion_scene_layer_unsupported"
                && limitation.message.contains("scene-a layer shape")
        }));
    }

    #[test]
    fn flatten_timeline_surfaces_rectangle_motion_scene_as_preview_shape() {
        let mut timeline = Timeline::empty("motion-scene-shape-preview");
        timeline.metadata.awidat = Some(awidat_meta::AwidatTimelineMetadata {
            motion_scenes: vec![MotionScene {
                id: "scene-a".to_string(),
                start_s: 0.0,
                duration_s: 3.0,
                fps: 24.0,
                width: 1920,
                height: 1080,
                layers: vec![MotionSceneLayer {
                    id: "panel".to_string(),
                    kind: MotionSceneLayerKind::Shape,
                    from_s: 0.25,
                    duration_s: 1.5,
                    z_index: 5,
                    params: [
                        ("shape".to_string(), serde_json::json!("rect")),
                        ("x".to_string(), serde_json::json!(0.1)),
                        ("y".to_string(), serde_json::json!(0.2)),
                        ("width".to_string(), serde_json::json!(0.3)),
                        ("height".to_string(), serde_json::json!(0.4)),
                        ("color".to_string(), serde_json::json!("#224466")),
                        ("opacity".to_string(), serde_json::json!(0.75)),
                    ]
                    .into_iter()
                    .collect(),
                }],
                rationale: None,
            }],
            ..awidat_meta::AwidatTimelineMetadata::default()
        });

        let snapshot = flatten_timeline_public(&timeline, Path::new("/tmp/project"));
        let Some(shape_track) = snapshot.tracks.iter().find(|track| {
            track.items.iter().any(|item| {
                matches!(
                    item,
                    TimelineItem::Clip {
                        motion_shape: Some(_),
                        ..
                    }
                )
            })
        }) else {
            panic!("rectangle motion scene should surface through a preview track");
        };
        let TimelineItem::Clip {
            clip_uuid,
            motion_shape: Some(shape),
            title,
            ..
        } = &shape_track.items[0]
        else {
            panic!("expected motion scene shape clip");
        };

        assert_eq!(clip_uuid, "scene-a:panel");
        assert!(title.is_none());
        assert_eq!(shape.shape, "rect");
        assert_eq!(shape.x, 0.1);
        assert_eq!(shape.y, 0.2);
        assert_eq!(shape.width, 0.3);
        assert_eq!(shape.height, 0.4);
        assert_eq!(shape.color, "#224466");
        assert_eq!(shape.opacity, 0.75);
        assert!(
            snapshot
                .preview_limitations
                .iter()
                .all(|limitation| { limitation.kind != "motion_scene_layer_unsupported" })
        );
    }

    #[test]
    fn flatten_timeline_surfaces_image_motion_scene_as_preview_image() {
        let mut timeline = Timeline::empty("motion-scene-image-preview");
        timeline.metadata.awidat = Some(awidat_meta::AwidatTimelineMetadata {
            motion_scenes: vec![MotionScene {
                id: "scene-a".to_string(),
                start_s: 0.0,
                duration_s: 3.0,
                fps: 24.0,
                width: 1920,
                height: 1080,
                layers: vec![MotionSceneLayer {
                    id: "logo".to_string(),
                    kind: MotionSceneLayerKind::Image,
                    from_s: 0.25,
                    duration_s: 1.5,
                    z_index: 8,
                    params: [
                        ("asset".to_string(), serde_json::json!("raw/logo.png")),
                        ("x".to_string(), serde_json::json!(0.1)),
                        ("y".to_string(), serde_json::json!(0.2)),
                        ("width".to_string(), serde_json::json!(0.3)),
                        ("height".to_string(), serde_json::json!(0.4)),
                        ("opacity".to_string(), serde_json::json!(0.75)),
                        ("fit".to_string(), serde_json::json!("contain")),
                        ("scale".to_string(), serde_json::json!(1.15)),
                        ("anchor_x".to_string(), serde_json::json!(0.5)),
                        ("anchor_y".to_string(), serde_json::json!(0.5)),
                        ("rotation_deg".to_string(), serde_json::json!(-6.0)),
                        (
                            "animations".to_string(),
                            serde_json::json!([
                                {
                                    "parameter": "overlay.x",
                                    "keyframes": [
                                        { "time_s": 0.0, "value": 0.1 },
                                        { "time_s": 1.5, "value": 0.2 }
                                    ]
                                }
                            ]),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                }],
                rationale: None,
            }],
            ..awidat_meta::AwidatTimelineMetadata::default()
        });

        let snapshot = flatten_timeline_public(&timeline, Path::new("/tmp/project"));
        let Some(image_track) = snapshot.tracks.iter().find(|track| {
            track.items.iter().any(|item| {
                matches!(
                    item,
                    TimelineItem::Clip {
                        motion_image: Some(_),
                        ..
                    }
                )
            })
        }) else {
            panic!("image motion scene should surface through a preview track");
        };
        let TimelineItem::Clip {
            clip_uuid,
            motion_image: Some(image),
            title,
            motion_shape,
            animations,
            ..
        } = &image_track.items[0]
        else {
            panic!("expected motion scene image clip");
        };

        assert_eq!(clip_uuid, "scene-a:logo");
        assert!(title.is_none());
        assert!(motion_shape.is_none());
        assert_eq!(image.asset_id, "raw/logo.png");
        assert_eq!(image.x, 0.1);
        assert_eq!(image.y, 0.2);
        assert_eq!(image.width, 0.3);
        assert_eq!(image.height, 0.4);
        assert_eq!(image.opacity, 0.75);
        assert_eq!(image.fit, "contain");
        assert_eq!(image.scale, 1.15);
        assert_eq!(image.anchor_x, 0.5);
        assert_eq!(image.anchor_y, 0.5);
        assert_eq!(image.rotation_deg, -6.0);
        assert_eq!(animations.len(), 1);
        assert_eq!(animations[0].target.parameter, "overlay.x");
        assert!(
            snapshot
                .preview_limitations
                .iter()
                .all(|limitation| { limitation.kind != "motion_scene_layer_unsupported" })
        );
    }

    #[test]
    fn applied_set_motion_scene_edl_surfaces_in_preview_snapshot() {
        let edl = r##"
*** Begin EDL
*** Set Motion Scene
+ scene_json: {"id":"scene-edl","duration_s":3.0,"fps":24.0,"width":1920,"height":1080,"layers":[{"id":"panel","kind":"solid","from_s":0.0,"duration_s":3.0,"z_index":0,"params":{"x":0.08,"y":0.12,"width":0.84,"height":0.76,"color":"#111827","opacity":0.86}},{"id":"logo","kind":"image","from_s":0.2,"duration_s":2.4,"z_index":5,"params":{"asset":"raw/logo.png","x":0.12,"y":0.18,"width":0.28,"height":0.28,"fit":"contain","opacity":0.9}},{"id":"accent","kind":"shape","from_s":0.3,"duration_s":2.0,"z_index":6,"params":{"shape":"rect","x":0.12,"y":0.52,"width":0.28,"height":0.04,"color":"#22C55E","opacity":0.8}},{"id":"headline","kind":"text","from_s":0.5,"duration_s":2.0,"z_index":10,"params":{"text":"From EDL"}}]}
*** End EDL
"##;
        let envelope =
            parse(edl).unwrap_or_else(|err| panic!("motion scene EDL should parse: {err}"));
        let timeline = Timeline::empty("motion-scene-edl-preview");
        let (timeline, outcome) = apply(&timeline, &envelope, &AnchorContext::empty())
            .unwrap_or_else(|err| panic!("EDL should apply: {err}"));

        assert_eq!(outcome.applied.len(), 1);
        let snapshot = flatten_timeline_public(&timeline, Path::new("/tmp/project"));
        // Preview uses the same protocol shape for authored and EDL-applied
        // scenes, so this protects the real desktop path rather than only storage.
        let Some(title_track) = snapshot
            .tracks
            .iter()
            .find(|track| track.role.as_deref() == Some("titles"))
        else {
            panic!("applied motion scene should surface through a preview title track");
        };
        assert_eq!(title_track.items.len(), 4);
        let has_panel = title_track.items.iter().any(|item| {
            matches!(
                item,
                TimelineItem::Clip {
                    clip_uuid,
                    motion_shape: Some(shape),
                    ..
                } if clip_uuid == "scene-edl:panel" && shape.color == "#111827"
            )
        });
        let has_image = title_track.items.iter().any(|item| {
            matches!(
                item,
                TimelineItem::Clip {
                    clip_uuid,
                    motion_image: Some(image),
                    ..
                } if clip_uuid == "scene-edl:logo" && image.asset_id == "raw/logo.png"
            )
        });
        let has_accent = title_track.items.iter().any(|item| {
            matches!(
                item,
                TimelineItem::Clip {
                    clip_uuid,
                    motion_shape: Some(shape),
                    ..
                } if clip_uuid == "scene-edl:accent" && shape.color == "#22C55E"
            )
        });
        let has_headline = title_track.items.iter().any(|item| {
            matches!(
                item,
                TimelineItem::Clip {
                    clip_uuid,
                    title: Some(title),
                    ..
                } if clip_uuid == "scene-edl:headline" && title.text == "From EDL"
            )
        });

        assert!(has_panel);
        assert!(has_image);
        assert!(has_accent);
        assert!(has_headline);
        assert!(
            snapshot
                .preview_limitations
                .iter()
                .all(|limitation| limitation.kind != "motion_scene_layer_unsupported")
        );
    }

    #[test]
    fn unsupported_parameter_animation_surfaces_preview_limitation() {
        let mut timeline = Timeline::empty("animation-limitations");
        timeline.metadata.awidat = Some(awidat_meta::AwidatTimelineMetadata {
            parameter_animations: vec![ParameterAnimation {
                id: "anim-volume".to_string(),
                target: AnimationTarget::ClipParameter {
                    clip_id: "clip-a".to_string(),
                    parameter: "volume".to_string(),
                },
                keyframes: vec![Keyframe::linear(0.0, 1.0)],
                pre_extrapolation: ExtrapolationMode::Hold,
                post_extrapolation: ExtrapolationMode::Hold,
                motion_path: None,
                metadata_only: true,
                rationale: None,
            }],
            ..awidat_meta::AwidatTimelineMetadata::default()
        });

        let snapshot = flatten_timeline_public(&timeline, Path::new("/tmp/project"));

        assert!(snapshot.preview_limitations.iter().any(|limitation| {
            limitation.kind == "animation_target_unsupported"
                && limitation.message.contains("anim-volume")
        }));
    }

    fn clip_with_split_edit(name: &str, lead_s: Option<f64>, trail_s: Option<f64>) -> Clip {
        let mut clip = Clip::empty(name.to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/a.mp4"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));
        clip.metadata.awidat = Some(AwidatClipMetadata {
            split_edit: Some(SplitEditSpec {
                audio_lead_s: lead_s,
                audio_lead_from_clip_id: None,
                audio_trail_s: trail_s,
                audio_trail_to_clip_id: None,
                reason: Some("test split edit".into()),
                confidence: Some(0.9),
            }),
            ..AwidatClipMetadata::default()
        });
        clip
    }

    fn clip_with_uuid(name: &str, uuid: &str) -> Clip {
        let mut clip = Clip::empty(name.to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/a.mp4"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));
        let mut metadata = AwidatClipMetadata::default();
        metadata
            .extra
            .insert("clip_uuid".to_string(), serde_json::json!(uuid));
        clip.metadata.awidat = Some(metadata);
        clip
    }

    #[test]
    fn flatten_marks_clip_source_when_proxy_missing_but_source_exists() {
        use awidat_desktop_protocol::PlayableKind;

        let tmp = tempfile::tempdir().unwrap();
        let raw_dir = tmp.path().join("raw");
        std::fs::create_dir_all(&raw_dir).unwrap();
        std::fs::write(raw_dir.join("test.mov"), b"fake").unwrap();

        let mut timeline = Timeline::empty("playable-test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("test-clip".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/test.mov"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(clip));
        timeline.tracks.children.push(StackChild::Track(track));

        let snapshot = flatten_timeline_public(&timeline, tmp.path());

        let TimelineItem::Clip {
            playable_path,
            playable_kind,
            ..
        } = &snapshot.tracks[0].items[0]
        else {
            panic!("expected a Clip item");
        };

        assert_eq!(*playable_kind, PlayableKind::Source);
        assert!(
            playable_path
                .as_deref()
                .is_some_and(|path| path.ends_with("raw/test.mov"))
        );
    }

    #[test]
    fn flatten_marks_clip_missing_when_source_missing() {
        use awidat_desktop_protocol::PlayableKind;

        let tmp = tempfile::tempdir().unwrap();
        let mut timeline = Timeline::empty("missing-source-test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("test-clip".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/test.mov"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(clip));
        timeline.tracks.children.push(StackChild::Track(track));

        let snapshot = flatten_timeline_public(&timeline, tmp.path());

        let TimelineItem::Clip {
            playable_path,
            playable_kind,
            ..
        } = &snapshot.tracks[0].items[0]
        else {
            panic!("expected a Clip item");
        };

        assert_eq!(*playable_kind, PlayableKind::Missing);
        assert!(playable_path.is_none());
    }
}
