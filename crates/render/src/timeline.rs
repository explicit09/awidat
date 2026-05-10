//! Timeline-render planning: walk a project's OTIO and produce the
//! [`RenderJobSpec`] that ffmpeg will execute.
//!
//! Extracted from `awidat-core::tools::start_render` so the desktop's
//! Export button can call into the same logic without going through
//! the agent tool. Both call sites depend on this module; both
//! produce identical specs.
//!
//! The non-obvious bit is the **single re-encode at concat
//! boundaries**. Stream-copy concat at non-keyframe-aligned cut
//! points produces audible clicks (DTS-seam scratch); we accept
//! the encoder cost to avoid that.
//!
//! Audio tracks aren't enumerated separately — most awidat
//! projects keep video and audio paired in the same source file,
//! so the concat filter pulls each input's audio stream alongside
//! its video stream.
//!
//! Gaps and Transitions are skipped in v1; they land when the EDL
//! grows transition / gap awareness on the render side.

use std::path::{Path, PathBuf};

use awidat_proto::awidat_meta::{BroadcastHost, BroadcastOverlayConfig, BroadcastOverlayStyle};
use awidat_proto::otio::{MediaReference, StackChild, TrackChild, TrackKind};
use awidat_proto::project::{files, read_otio_timeline};
use awidat_proto::transitions;
use chrono::Utc;
use serde::Deserialize;
use thiserror::Error;

use crate::job::RenderJobSpec;

/// Errors building a timeline-render spec.
#[derive(Debug, Error)]
pub enum RenderTimelineError {
    /// `<project_root>/project.otio.json` doesn't exist.
    #[error("no project.otio.json found at {0} — this isn't an awidat project root")]
    NoOtio(PathBuf),
    /// OTIO file present but parse / validation failed.
    #[error("timeline parse failed: {message}")]
    OtioParse {
        /// Diagnostic from the underlying parser.
        message: String,
    },
    /// A clip referenced an asset path that isn't on disk.
    #[error("timeline references missing asset {missing} (clip '{clip_name}')")]
    MissingAsset {
        /// Clip name from the OTIO.
        clip_name: String,
        /// Absolute path that didn't resolve.
        missing: PathBuf,
    },
    /// A clip lacked a `source_range`.
    #[error("clip '{clip_name}' has no source_range — can't extract a renderable segment")]
    ClipMissingRange {
        /// Clip name from the OTIO.
        clip_name: String,
    },
    /// A clip referenced a LUT path that isn't on disk.
    #[error("clip '{clip_name}' references missing LUT {missing}")]
    MissingLut {
        /// Clip name from the OTIO.
        clip_name: String,
        /// Absolute LUT path that didn't resolve.
        missing: PathBuf,
    },
    /// The timeline parsed but has no clips on any video track.
    #[error("timeline has no clips to render")]
    EmptyTimeline,
    /// A transition kind/id cannot be exported by the phase-one renderer.
    #[error("unsupported timeline transition {kind:?}: {message}")]
    UnsupportedTransition {
        /// Transition kind or Awidat id from OTIO.
        kind: String,
        /// Detailed registry/lookup message.
        message: String,
    },
}

/// Broadcast overlay config plus project root for resolving optional
/// project-relative assets.
#[derive(Debug, Clone)]
pub struct BroadcastOverlayPlan {
    /// Timeline-level overlay config read from OTIO metadata.
    pub config: BroadcastOverlayConfig,
    /// Project root used to resolve project-relative overlay assets.
    pub project_root: PathBuf,
}

/// One source-media segment to feed into the timeline-render concat.
/// Public so callers can sum durations or otherwise inspect the plan
/// before kicking off ffmpeg.
#[derive(Debug, Clone, Default)]
pub struct TimelineSegment {
    /// Absolute path to the source media.
    pub asset_path: PathBuf,
    /// Seconds into the source media where the cut starts.
    pub start_s: f64,
    /// Seconds of duration to take from the source.
    pub duration_s: f64,
    /// Linear gain multiplier for this segment's audio. `None` means
    /// no `awidat.volume` effect is on the underlying clip — the
    /// FilterPlanner skips emitting a `volume=` filter and the audio
    /// passes through unchanged. `Some(1.0)` is unity (functionally
    /// identical to `None` but the planner still emits the filter
    /// for explicitness).
    pub volume: Option<f64>,
    /// Playback rate multiplier. `None` means no `awidat.speed`
    /// effect — the segment plays at 1×. The segment's contribution
    /// to the master timeline duration is `duration_s / factor` when
    /// `factor` is `Some`. (Step 15.4 wires the setpts/atempo path;
    /// 15.3 only plumbs the field.)
    pub speed: Option<f64>,
    /// Optional clip-level color correction controls, read from the
    /// `awidat.color_correction` effect.
    pub color_correction: Option<ColorCorrectionPlan>,
    /// Optional absolute LUT path, read from the `awidat.lut` effect.
    pub lut_path: Option<PathBuf>,
    /// Optional FFmpeg-native audio FX chain.
    pub audio_fx: Option<AudioFxPlan>,
}

/// Render-time audio clip span extracted from an OTIO audio track.
#[derive(Debug, Clone, Default)]
pub struct AudioClipPlan {
    /// Absolute source media path.
    pub asset_path: PathBuf,
    /// Source start time in seconds.
    pub start_s: f64,
    /// Source duration in seconds.
    pub duration_s: f64,
    /// Optional gain multiplier for the clip.
    pub volume: Option<f64>,
    /// Optional playback speed multiplier.
    pub speed: Option<f64>,
    /// Optional fade-in duration in seconds.
    pub fade_in_s: Option<f64>,
    /// Optional fade-out duration in seconds.
    pub fade_out_s: Option<f64>,
    /// Optional FFmpeg-native audio FX chain.
    pub audio_fx: Option<AudioFxPlan>,
}

/// Render-time item on an audio track.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum AudioTrackItemPlan {
    /// Media-backed audio clip.
    Clip(AudioClipPlan),
    /// Silent gap with a duration in seconds.
    Gap {
        /// Gap duration in seconds.
        duration_s: f64,
    },
}

/// Render-time audio track plan including mix metadata and items.
#[derive(Debug, Clone)]
pub struct AudioTrackPlan {
    /// Track display name.
    pub name: String,
    /// Semantic role such as dialogue, music, effects, or ambience.
    pub role: String,
    /// Track gain multiplier.
    pub volume: f64,
    /// Whether the track is muted.
    pub muted: bool,
    /// Whether the track is soloed.
    pub solo: bool,
    /// Optional ducking behavior for this track.
    pub ducking: Option<DuckingPlan>,
    /// Optional FFmpeg-native audio FX chain for the full mixed track.
    pub audio_fx: Option<AudioFxPlan>,
    /// Ordered audio items on the track.
    pub items: Vec<AudioTrackItemPlan>,
}

/// Audio ducking parameters for music/effects under dialogue.
#[derive(Debug, Clone)]
pub struct DuckingPlan {
    /// Whether ducking is active.
    pub enabled: bool,
    /// Gain reduction applied while ducking.
    pub amount_db: f64,
    /// Ducking attack in milliseconds.
    pub attack_ms: f64,
    /// Ducking release in milliseconds.
    pub release_ms: f64,
}

/// FFmpeg-native audio cleanup/EQ/dynamics settings.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct AudioFxPlan {
    /// High-pass cutoff in Hz.
    pub high_pass_hz: Option<f64>,
    /// Low-pass cutoff in Hz.
    pub low_pass_hz: Option<f64>,
    /// Parametric EQ bands.
    #[serde(default)]
    pub eq_bands: Vec<EqBandPlan>,
    /// Compressor threshold in dB.
    pub compressor_threshold_db: Option<f64>,
    /// Compressor ratio.
    pub compressor_ratio: Option<f64>,
    /// Limiter ceiling in dBFS.
    pub limiter_limit_db: Option<f64>,
    /// Noise gate threshold in dBFS.
    pub noise_gate_threshold_db: Option<f64>,
    /// Hum notch frequency in Hz.
    pub hum_notch_hz: Option<f64>,
    /// Center frequency for the de-ess approximation.
    pub de_ess_hz: Option<f64>,
    /// Gain reduction for the de-ess approximation.
    pub de_ess_reduction_db: Option<f64>,
    /// Loudnorm integrated loudness target.
    pub loudnorm_i: Option<f64>,
    /// Loudnorm true-peak target.
    pub loudnorm_tp: Option<f64>,
}

/// One parametric EQ band.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EqBandPlan {
    /// Center frequency in Hz.
    pub freq_hz: f64,
    /// Gain in dB.
    pub gain_db: f64,
    /// Band width in Hz.
    pub width_hz: Option<f64>,
}

/// Clip-level color controls that render maps into FFmpeg filters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColorCorrectionPlan {
    /// Exposure offset in stops.
    pub exposure_ev: Option<f64>,
    /// Contrast multiplier.
    pub contrast: Option<f64>,
    /// Saturation multiplier.
    pub saturation: Option<f64>,
    /// Normalized warm/cool control.
    pub temperature: Option<f64>,
    /// Normalized green/magenta control.
    pub tint: Option<f64>,
    /// Normalized shadow control.
    pub shadows: Option<f64>,
    /// Normalized highlight control.
    pub highlights: Option<f64>,
}

/// Pull a numeric metadata field off the first effect on `clip` whose
/// `effect_name` matches. Returns `None` when no such effect exists
/// or the metadata field is missing / non-numeric. Used to surface
/// awidat.volume / awidat.speed values into the render pipeline.
fn read_effect_number(
    clip: &awidat_proto::otio::Clip,
    effect_name: &str,
    field: &str,
) -> Option<f64> {
    clip.effects
        .iter()
        .find(|e| e.effect_name == effect_name)
        .and_then(|e| e.metadata.get(field))
        .and_then(serde_json::Value::as_f64)
}

fn read_effect_string(
    clip: &awidat_proto::otio::Clip,
    effect_name: &str,
    field: &str,
) -> Option<String> {
    clip.effects
        .iter()
        .find(|e| e.effect_name == effect_name)
        .and_then(|e| e.metadata.get(field))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn read_color_correction(clip: &awidat_proto::otio::Clip) -> Option<ColorCorrectionPlan> {
    let effect = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.color_correction")?;
    let m = &effect.metadata;
    let plan = ColorCorrectionPlan {
        exposure_ev: m.get("exposure_ev").and_then(serde_json::Value::as_f64),
        contrast: m.get("contrast").and_then(serde_json::Value::as_f64),
        saturation: m.get("saturation").and_then(serde_json::Value::as_f64),
        temperature: m.get("temperature").and_then(serde_json::Value::as_f64),
        tint: m.get("tint").and_then(serde_json::Value::as_f64),
        shadows: m.get("shadows").and_then(serde_json::Value::as_f64),
        highlights: m.get("highlights").and_then(serde_json::Value::as_f64),
    };
    Some(plan)
}

fn read_audio_fade(clip: &awidat_proto::otio::Clip) -> (Option<f64>, Option<f64>) {
    let Some(effect) = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.audio_fade")
    else {
        return (None, None);
    };
    (
        effect
            .metadata
            .get("fade_in_s")
            .and_then(serde_json::Value::as_f64),
        effect
            .metadata
            .get("fade_out_s")
            .and_then(serde_json::Value::as_f64),
    )
}

fn read_clip_audio_fx(clip: &awidat_proto::otio::Clip) -> Option<AudioFxPlan> {
    let effect = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.audio_fx")?;
    serde_json::from_value(serde_json::Value::Object(effect.metadata.clone())).ok()
}

type TimelineFullPlan = (
    Vec<TimelineSegment>,
    Vec<TransitionPlan>,
    Vec<TitlePlan>,
    Option<BroadcastOverlayPlan>,
    Vec<AudioTrackPlan>,
);

/// Walk `<project_root>/project.otio.json` and collect every
/// video-track clip's `(asset, source_range)` in playback order.
/// Skips Gap, Transition, and nested Stack children for v1.
///
/// Wraps [`collect_timeline_plan`] and drops the transitions +
/// titles — preserved for callers that don't need either.
pub fn collect_timeline_segments(
    project_root: &Path,
) -> Result<Vec<TimelineSegment>, RenderTimelineError> {
    let (segs, _, _, _, _) = collect_timeline_full_plan(project_root)?;
    Ok(segs)
}

/// Walk `<project_root>/project.otio.json` and collect both the
/// renderable segments AND the transitions between adjacent
/// segments. Returned in playback order; `TransitionPlan` indices
/// reference the returned segments slice.
///
/// Step 14.5: the render pipeline uses this to splice xfade filters
/// between the segments that have a [`TrackChild::Transition`]
/// between them on the OTIO track.
///
/// Wraps [`collect_timeline_full_plan`] and drops the titles —
/// preserved for callers that don't need title awareness.
pub fn collect_timeline_plan(
    project_root: &Path,
) -> Result<(Vec<TimelineSegment>, Vec<TransitionPlan>), RenderTimelineError> {
    let (segs, transitions, _, _, _) = collect_timeline_full_plan(project_root)?;
    Ok((segs, transitions))
}

/// Walk `<project_root>/project.otio.json` and collect segments +
/// transitions + titles. The Titles track (flagged via
/// `track.metadata["awidat_track_role"] = "titles"` or matched by
/// name `"Titles"` for backwards-compat) is excluded from segment
/// production — its clips are virtual, not media-bearing.
pub fn collect_timeline_full_plan(
    project_root: &Path,
) -> Result<TimelineFullPlan, RenderTimelineError> {
    let otio_path = project_root.join(files::OTIO);
    if !otio_path.exists() {
        return Err(RenderTimelineError::NoOtio(otio_path));
    }
    let mut warnings = Vec::new();
    let timeline = read_otio_timeline(&otio_path, &mut warnings).map_err(|e| {
        RenderTimelineError::OtioParse {
            message: e.to_string(),
        }
    })?;

    let mut segs = Vec::new();
    let mut transitions = Vec::new();
    let mut titles = Vec::new();
    let mut audio_tracks = Vec::new();
    for child in &timeline.tracks.children {
        let StackChild::Track(track) = child else {
            continue;
        };
        if matches!(track.kind, TrackKind::Audio) {
            audio_tracks.push(collect_audio_track_plan(project_root, track)?);
            continue;
        }
        if !matches!(track.kind, TrackKind::Video) {
            continue;
        }
        if is_titles_track(track) {
            // Walk titles separately; don't try to read media off it.
            for tc in &track.children {
                let TrackChild::Clip(clip) = tc else { continue };
                let Some(plan) = parse_title_plan(clip) else {
                    continue;
                };
                titles.push(plan);
            }
            continue;
        }
        // Walk the track's children. Clips become segments; a
        // Transition immediately following a Clip queues a transition
        // pointing at the *next* clip we'll see (`pending_transition`).
        // Other children (Gap, Stack) reset the pending state — they
        // can't sit between clips that share a transition in v1.
        let mut pending_transition: Option<(String, f64)> = None;
        for tc in &track.children {
            match tc {
                TrackChild::Clip(clip) => {
                    let MediaReference::External(ext) = &clip.media_reference else {
                        pending_transition = None;
                        continue;
                    };
                    let Some(range) = clip.source_range.as_ref() else {
                        return Err(RenderTimelineError::ClipMissingRange {
                            clip_name: clip.name.clone(),
                        });
                    };
                    let asset_path = project_root.join(&ext.target_url);
                    if !asset_path.exists() {
                        return Err(RenderTimelineError::MissingAsset {
                            clip_name: clip.name.clone(),
                            missing: asset_path,
                        });
                    }
                    let volume = read_effect_number(clip, "awidat.volume", "value");
                    let speed = read_effect_number(clip, "awidat.speed", "factor");
                    let color_correction = read_color_correction(clip);
                    let audio_fx = read_clip_audio_fx(clip);
                    let lut_path = read_effect_string(clip, "awidat.lut", "lut_path")
                        .map(|lut_path| project_root.join(lut_path));
                    if let Some(lut_path) = lut_path.as_ref()
                        && !lut_path.exists()
                    {
                        return Err(RenderTimelineError::MissingLut {
                            clip_name: clip.name.clone(),
                            missing: lut_path.clone(),
                        });
                    }
                    let new_index = segs.len();
                    segs.push(TimelineSegment {
                        asset_path,
                        start_s: range.start_time.to_seconds(),
                        duration_s: range.duration.to_seconds(),
                        volume,
                        speed,
                        color_correction,
                        lut_path,
                        audio_fx,
                    });
                    if let Some((kind, duration_s)) = pending_transition.take()
                        && new_index > 0
                    {
                        transitions.push(TransitionPlan {
                            from_segment_index: new_index - 1,
                            to_segment_index: new_index,
                            kind,
                            duration_s,
                        });
                    }
                }
                TrackChild::Transition(t) => {
                    let xfade =
                        transitions::resolve_ffmpeg_xfade(&t.transition_type).map_err(|e| {
                            RenderTimelineError::UnsupportedTransition {
                                kind: t.transition_type.clone(),
                                message: e.to_string(),
                            }
                        })?;
                    if xfade.is_none() {
                        return Err(RenderTimelineError::UnsupportedTransition {
                            kind: t.transition_type.clone(),
                            message: "semantic-only transition cannot be exported".into(),
                        });
                    }
                    let total = t.in_offset.to_seconds() + t.out_offset.to_seconds();
                    pending_transition = Some((t.transition_type.clone(), total));
                }
                TrackChild::Gap(_) | TrackChild::Stack(_) => {
                    pending_transition = None;
                }
            }
        }
    }
    let broadcast_overlay = timeline
        .metadata
        .awidat
        .as_ref()
        .and_then(|m| m.broadcast_overlay.clone())
        .map(|config| BroadcastOverlayPlan {
            config,
            project_root: project_root.to_path_buf(),
        });
    Ok((segs, transitions, titles, broadcast_overlay, audio_tracks))
}

fn collect_audio_track_plan(
    project_root: &Path,
    track: &awidat_proto::otio::Track,
) -> Result<AudioTrackPlan, RenderTimelineError> {
    let settings = parse_audio_track_settings(track);
    let mut items = Vec::new();
    for tc in &track.children {
        match tc {
            TrackChild::Clip(clip) => {
                let MediaReference::External(ext) = &clip.media_reference else {
                    continue;
                };
                let Some(range) = clip.source_range.as_ref() else {
                    return Err(RenderTimelineError::ClipMissingRange {
                        clip_name: clip.name.clone(),
                    });
                };
                let asset_path = project_root.join(&ext.target_url);
                if !asset_path.exists() {
                    return Err(RenderTimelineError::MissingAsset {
                        clip_name: clip.name.clone(),
                        missing: asset_path,
                    });
                }
                let (fade_in_s, fade_out_s) = read_audio_fade(clip);
                let audio_fx = read_clip_audio_fx(clip);
                items.push(AudioTrackItemPlan::Clip(AudioClipPlan {
                    asset_path,
                    start_s: range.start_time.to_seconds(),
                    duration_s: range.duration.to_seconds(),
                    volume: read_effect_number(clip, "awidat.volume", "value"),
                    speed: read_effect_number(clip, "awidat.speed", "factor"),
                    fade_in_s,
                    fade_out_s,
                    audio_fx,
                }));
            }
            TrackChild::Gap(gap) => items.push(AudioTrackItemPlan::Gap {
                duration_s: gap.source_range.duration.to_seconds(),
            }),
            TrackChild::Transition(_) | TrackChild::Stack(_) => {}
        }
    }
    Ok(AudioTrackPlan {
        name: track.name.clone(),
        role: settings.role,
        volume: settings.volume,
        muted: settings.muted,
        solo: settings.solo,
        ducking: settings.ducking,
        audio_fx: settings.audio_fx,
        items,
    })
}

fn parse_audio_track_settings(track: &awidat_proto::otio::Track) -> AudioTrackPlan {
    let value = track.metadata.get("awidat_audio");
    let role = value
        .and_then(|v| v.get("role"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| default_audio_role(&track.name));
    let volume = value
        .and_then(|v| v.get("volume"))
        .and_then(serde_json::Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(1.0);
    let muted = value
        .and_then(|v| v.get("muted"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let solo = value
        .and_then(|v| v.get("solo"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let ducking = value.and_then(|v| v.get("ducking")).map(|d| DuckingPlan {
        enabled: d
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
        amount_db: d
            .get("amount_db")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(-12.0),
        attack_ms: d
            .get("attack_ms")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(80.0),
        release_ms: d
            .get("release_ms")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(300.0),
    });
    let audio_fx = value
        .and_then(|v| v.get("fx"))
        .and_then(|v| serde_json::from_value::<AudioFxPlan>(v.clone()).ok());
    AudioTrackPlan {
        name: track.name.clone(),
        role,
        volume,
        muted,
        solo,
        ducking,
        audio_fx,
        items: Vec::new(),
    }
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

/// True iff the track is the project's Titles track. Mirrors the
/// apply-side check in `crates/core/src/edl/apply.rs` —
/// `track.metadata["awidat_track_role"] = "titles"` flag with a
/// fallback to the canonical name.
fn is_titles_track(track: &awidat_proto::otio::Track) -> bool {
    if track
        .metadata
        .get("awidat_track_role")
        .and_then(|v| v.as_str())
        == Some("titles")
    {
        return true;
    }
    track.name == "Titles"
}

/// Parse one synthesized title-clip into a [`TitlePlan`]. Returns
/// `None` if the clip carries no awidat.title effect or required
/// metadata is missing — the render walk just skips invalid titles
/// rather than aborting.
fn parse_title_plan(clip: &awidat_proto::otio::Clip) -> Option<TitlePlan> {
    let effect = clip
        .effects
        .iter()
        .find(|e| e.effect_name == "awidat.title")?;
    let m = &effect.metadata;
    let text = m.get("text").and_then(|v| v.as_str())?.to_string();
    let start_s = m.get("start_s").and_then(serde_json::Value::as_f64)?;
    let end_s = m.get("end_s").and_then(serde_json::Value::as_f64)?;
    if end_s <= start_s {
        return None;
    }
    let position = match m
        .get("position")
        .and_then(|v| v.as_str())
        .unwrap_or("center")
    {
        "top" => TitlePosition::Top,
        "bottom" => TitlePosition::Bottom,
        _ => TitlePosition::Center,
    };
    let font_size = m
        .get("font_size")
        .and_then(serde_json::Value::as_u64)
        .map(|n| n as u32)
        .unwrap_or(64);
    let color = m
        .get("color")
        .and_then(|v| v.as_str())
        .unwrap_or("#FFFFFF")
        .to_string();
    let font_weight = match m
        .get("font_weight")
        .and_then(|v| v.as_str())
        .unwrap_or("normal")
    {
        "bold" => TitleWeight::Bold,
        _ => TitleWeight::Normal,
    };
    let animation = match m
        .get("animation")
        .and_then(|v| v.as_str())
        .unwrap_or("none")
    {
        "fade_in" => TitleAnimation::FadeIn,
        "fade_out" => TitleAnimation::FadeOut,
        "fade_in_out" => TitleAnimation::FadeInOut,
        "slide_in" => TitleAnimation::SlideIn,
        "slide_out" => TitleAnimation::SlideOut,
        _ => TitleAnimation::None,
    };
    let role = m
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("title")
        .to_string();
    let safe_area = m
        .get("safe_area")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Some(TitlePlan {
        text,
        start_s,
        end_s,
        position,
        font_size,
        color,
        font_weight,
        animation,
        role,
        safe_area,
    })
}

/// One title overlay parsed from the project's Titles track. The
/// FilterPlanner emits one `drawtext=` per title at the end of the
/// filter graph, chained off the master concat output.
#[derive(Debug, Clone)]
pub struct TitlePlan {
    /// Text to render.
    pub text: String,
    /// When the title appears, in master-timeline seconds.
    pub start_s: f64,
    /// When the title disappears, in master-timeline seconds.
    pub end_s: f64,
    /// Vertical band on the frame.
    pub position: TitlePosition,
    /// Font size in pixels (rendered against a 1080p reference frame;
    /// ffmpeg scales proportionally).
    pub font_size: u32,
    /// Hex colour string like `"#FFFFFF"`.
    pub color: String,
    /// Bold vs normal weight.
    pub font_weight: TitleWeight,
    /// Entry / exit animation.
    pub animation: TitleAnimation,
    /// Overlay role, usually `"title"` or `"caption"`.
    pub role: String,
    /// Optional safe-area profile carried by caption nodes.
    pub safe_area: Option<String>,
}

/// Mirrors `awidat_core::edl::op::TitlePosition` to avoid a render
/// → core dep. Render only needs the variants for emitting drawtext
/// y= expressions; the parsing happens in core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlePosition {
    /// Near the top edge.
    Top,
    /// Vertically centered.
    Center,
    /// Near the bottom edge.
    Bottom,
}

/// Mirrors `awidat_core::edl::op::TitleWeight`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleWeight {
    /// Regular weight.
    Normal,
    /// Bold weight.
    Bold,
}

/// Mirrors `awidat_core::edl::op::TitleAnimation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleAnimation {
    /// No animation.
    None,
    /// Fade in over the leading 500ms.
    FadeIn,
    /// Fade out over the trailing 500ms.
    FadeOut,
    /// Fade in at start_s, fade out at end_s.
    FadeInOut,
    /// Slide in from off-screen.
    SlideIn,
    /// Slide out off-screen.
    SlideOut,
}

/// One transition between two segments in the timeline. Step 14.4
/// introduces this type so the [`FilterPlanner`] has a slot for
/// future transition wiring; in 14.4 callers always pass an empty
/// `transitions` slice and the planner emits the same monolithic
/// concat filter as before.
///
/// `from_segment_index` and `to_segment_index` are indices into the
/// segments slice the planner is fed. They MUST be adjacent
/// (`to == from + 1`); the apply layer rejects non-adjacent
/// transitions before they reach here.
#[derive(Debug, Clone)]
pub struct TransitionPlan {
    /// Index of the outgoing segment.
    pub from_segment_index: usize,
    /// Index of the incoming segment. Must be `from_segment_index + 1`.
    pub to_segment_index: usize,
    /// Transition kind (`"SMPTE_Dissolve"`, `"awidat.fade_in"`, etc).
    /// Wired to ffmpeg's xfade transition names in 14.5.
    pub kind: String,
    /// Total transition duration on the timeline, in seconds.
    pub duration_s: f64,
}

/// Plans the `-filter_complex` argument + map labels for a render.
///
/// Step 14.4 extracts this from the prior monolithic
/// [`build_timeline_argv`] so future filter types (transitions in
/// 14.5; volume / speed in Step 15; drawtext in Step 16) can compose
/// without rewriting the same builder. Behaviour with empty
/// `transitions` AND empty `titles` is identical to the pre-extract
/// code.
///
/// The planner doesn't take ownership of the segments or care about
/// the input source — callers feed the slices by reference.
pub struct FilterPlanner<'a> {
    segments: &'a [TimelineSegment],
    transitions: &'a [TransitionPlan],
    titles: &'a [TitlePlan],
    broadcast_overlay: Option<&'a BroadcastOverlayPlan>,
}

/// Output of [`FilterPlanner::plan`]. Carries everything the caller
/// needs to splice into an ffmpeg argv: the filter graph string and
/// the `[outv]` / `[outa]` map labels (the planner picks these so
/// 14.5's xfade chain can rename them if it needs intermediate
/// stages without breaking the caller's `-map` args).
#[derive(Debug, Clone)]
pub struct FilterPlan {
    /// Value for `-filter_complex`.
    pub filter_complex: String,
    /// Label for `-map` on the video output (typically `[outv]`).
    pub video_out_label: String,
    /// Label for `-map` on the audio output (typically `[outa]`).
    pub audio_out_label: String,
}

impl<'a> FilterPlanner<'a> {
    /// Construct a planner over segments + transitions, with no
    /// titles. Equivalent to [`Self::with_titles`] passing `&[]`.
    pub fn new(segments: &'a [TimelineSegment], transitions: &'a [TransitionPlan]) -> Self {
        Self::with_titles(segments, transitions, &[])
    }

    /// Construct a planner over segments + transitions + titles.
    /// Title overlays land as `drawtext=` filters appended to the
    /// master video output of the segment + transition graph.
    pub fn with_titles(
        segments: &'a [TimelineSegment],
        transitions: &'a [TransitionPlan],
        titles: &'a [TitlePlan],
    ) -> Self {
        Self {
            segments,
            transitions,
            titles,
            broadcast_overlay: None,
        }
    }

    /// Construct a planner over media segments, transitions, sparse
    /// title overlays, and an optional timeline-level broadcast overlay.
    pub fn with_titles_and_broadcast_overlay(
        segments: &'a [TimelineSegment],
        transitions: &'a [TransitionPlan],
        titles: &'a [TitlePlan],
        broadcast_overlay: Option<&'a BroadcastOverlayPlan>,
    ) -> Self {
        Self {
            segments,
            transitions,
            titles,
            broadcast_overlay,
        }
    }

    /// Build the filter complex + output labels.
    ///
    /// With no transitions: emits the same monolithic
    /// `[0:v:0][0:a:0]…concat=n=N:v=1:a=1[outv][outa]` graph the
    /// pre-extract code produced.
    ///
    /// With transitions (Step 14.5): groups consecutive segments
    /// connected by a transition into "chunks." Each pair-chunk
    /// emits `xfade` + `acrossfade` filters into a `[xv<i>][xa<i>]`
    /// pair which then participates in the final concat in place
    /// of the two raw segment streams. Lone segments still feed in
    /// directly via `[i:v:0][i:a:0]`.
    ///
    /// v1 chunk policy: each segment can participate in at most one
    /// transition. If two transitions try to share the same middle
    /// segment (a chain of three), the second transition is dropped
    /// with a debug-trace log — the render still produces a valid
    /// output, just without that overlap. Multi-transition chains
    /// land in a future commit.
    pub fn plan(&self) -> FilterPlan {
        let base = if self.transitions.is_empty() {
            self.plan_no_transitions()
        } else {
            self.plan_with_transitions()
        };
        let base = if let Some(overlay) = self.broadcast_overlay {
            self.append_broadcast_overlay(base, overlay)
        } else {
            base
        };
        if self.titles.is_empty() {
            base
        } else {
            self.append_titles(base)
        }
    }

    /// Decorate a video-only graph with broadcast overlay and titles.
    /// Used by explicit-audio renders, where audio is mixed separately.
    pub fn decorate_video_filter(
        &self,
        filter_complex: String,
        video_out_label: String,
    ) -> FilterPlan {
        let mut base = FilterPlan {
            filter_complex,
            video_out_label,
            audio_out_label: String::new(),
        };
        if let Some(overlay) = self.broadcast_overlay {
            base = self.append_broadcast_overlay(base, overlay);
        }
        if !self.titles.is_empty() {
            base = self.append_titles(base);
        }
        base
    }

    /// Splice a `drawtext=` chain onto `base.video_out_label` and
    /// rename the master video output to `[outv]` afterwards. Audio
    /// passes through untouched. The `enable='between(t,start,end)'`
    /// expression on each drawtext bounds the title to its window so
    /// concurrent titles all live in the same chain without
    /// interfering.
    fn append_titles(&self, base: FilterPlan) -> FilterPlan {
        // Pick a stable intermediate label. The base already produced
        // [outv] / [outa]; we rename [outv] → [base_v] inside the
        // filter_complex by appending a drawtext chain that consumes
        // [base_v] and produces a fresh [titled_v]. We then expose
        // [titled_v] as the new video_out_label.
        //
        // Strategy: don't try to rename the existing label (we'd have
        // to rewrite the filter graph); instead, take the base's
        // video_out_label as our INPUT and produce a new output.
        let in_label = base.video_out_label.clone();
        let out_label = "[titled_v]".to_string();

        let mut filter = base.filter_complex.clone();
        filter.push(';');
        filter.push_str(&in_label);
        // Comma-separate the drawtext filters so they all run on the
        // same input → single output. drawtext's `enable=` keeps each
        // bounded to its window without cross-contamination.
        let parts: Vec<String> = self.titles.iter().map(format_drawtext_filter).collect();
        filter.push_str(&parts.join(","));
        filter.push_str(&out_label);

        FilterPlan {
            filter_complex: filter,
            video_out_label: out_label,
            audio_out_label: base.audio_out_label,
        }
    }

    fn append_broadcast_overlay(
        &self,
        base: FilterPlan,
        overlay: &BroadcastOverlayPlan,
    ) -> FilterPlan {
        if !overlay.config.enabled {
            return base;
        }
        let in_label = base.video_out_label.clone();
        let out_label = "[broadcast_v]".to_string();
        let parts = format_broadcast_overlay_filters(overlay);
        if parts.is_empty() {
            return base;
        }
        let mut filter = base.filter_complex.clone();
        filter.push(';');
        filter.push_str(&format_broadcast_overlay_graph(
            &in_label, &out_label, overlay, &parts,
        ));
        FilterPlan {
            filter_complex: filter,
            video_out_label: out_label,
            audio_out_label: base.audio_out_label,
        }
    }

    fn plan_no_transitions(&self) -> FilterPlan {
        let n = self.segments.len();
        let mut filter = String::new();
        // Pre-stage so per-segment effects (volume in 15.3, speed in
        // 15.4) prepend their filter chain before the concat. Each
        // call returns the (video, audio) labels to feed into concat.
        let inputs: Vec<(String, String)> = (0..n)
            .map(|i| stage_segment_inputs(&mut filter, i, &self.segments[i]))
            .collect();
        for (v, a) in &inputs {
            filter.push_str(v);
            filter.push_str(a);
        }
        filter.push_str(&format!("concat=n={n}:v=1:a=1[outv][outa]"));
        FilterPlan {
            filter_complex: filter,
            video_out_label: "[outv]".into(),
            audio_out_label: "[outa]".into(),
        }
    }

    fn plan_with_transitions(&self) -> FilterPlan {
        let n = self.segments.len();

        // Build a "next-segment-of-the-same-chunk" map. seg `i`'s
        // partner is `j` iff there's a transition between them. We
        // enforce v1 single-transition-per-segment by only
        // remembering the *first* transition for any given segment.
        let mut paired_with: Vec<Option<&TransitionPlan>> = vec![None; n];
        for t in self.transitions {
            if t.from_segment_index >= n || t.to_segment_index != t.from_segment_index + 1 {
                tracing::debug!(
                    transition = ?t,
                    "FilterPlanner: dropping transition with non-adjacent indices"
                );
                continue;
            }
            // Either segment already taken? Drop this one.
            if paired_with[t.from_segment_index].is_some()
                || paired_with[t.to_segment_index].is_some()
            {
                tracing::debug!(
                    transition = ?t,
                    "FilterPlanner: dropping transition (segment already part of a chunk)",
                );
                continue;
            }
            paired_with[t.from_segment_index] = Some(t);
        }

        let mut filter = String::new();

        // Pre-stage each segment's video / audio inputs. Step 15.3
        // adds the per-segment volume filter here: when a segment
        // carries an awidat.volume effect, its audio stream goes
        // through `volume=<v>` first, producing a [av<i>] label that
        // downstream graph nodes use in place of [i:a:0]. Speed
        // lands in 15.4 with a parallel pass on video + atempo on
        // audio.
        let inputs: Vec<(String, String)> = (0..n)
            .map(|i| stage_segment_inputs(&mut filter, i, &self.segments[i]))
            .collect();

        // Track the order of concat input pairs (each entry is a
        // pre-built (video_label, audio_label) ready to drop in).
        let mut concat_inputs: Vec<(String, String)> = Vec::with_capacity(n);

        let mut i = 0;
        let mut chunk_id: usize = 0;
        while i < n {
            if let Some(t) = paired_with[i] {
                let j = t.to_segment_index;
                let v_label = format!("[xv{chunk_id}]");
                let a_label = format!("[xa{chunk_id}]");
                let xfade_kind = map_transition_kind(&t.kind);
                // xfade offset = the from-segment's *post-speed*
                // duration minus the transition duration. Both inputs
                // must be at the cut point at offset
                // `effective_duration - transition.duration`. A
                // 4s clip at 2x speed has effective duration 2s.
                let from_dur = effective_duration(&self.segments[i]);
                let offset = (from_dur - t.duration_s).max(0.0);
                filter.push_str(&format!(
                    "{from_v}{to_v}xfade=transition={kind}:duration={dur}:offset={off}{out};",
                    from_v = inputs[i].0,
                    to_v = inputs[j].0,
                    kind = xfade_kind,
                    dur = t.duration_s,
                    off = offset,
                    out = v_label,
                ));
                filter.push_str(&format!(
                    "{from_a}{to_a}acrossfade=d={dur}{out};",
                    from_a = inputs[i].1,
                    to_a = inputs[j].1,
                    dur = t.duration_s,
                    out = a_label,
                ));
                concat_inputs.push((v_label, a_label));
                chunk_id += 1;
                i += 2;
            } else {
                concat_inputs.push((inputs[i].0.clone(), inputs[i].1.clone()));
                i += 1;
            }
        }

        // Tail: single-input concat would just rename, so when we
        // have one chunk it might be a paired-xfade output already.
        // ffmpeg's concat takes n>=1; we always wrap so the caller's
        // `-map [outv] -map [outa]` is uniform.
        for (v, a) in &concat_inputs {
            filter.push_str(v);
            filter.push_str(a);
        }
        filter.push_str(&format!(
            "concat=n={n}:v=1:a=1[outv][outa]",
            n = concat_inputs.len(),
        ));

        FilterPlan {
            filter_complex: filter,
            video_out_label: "[outv]".into(),
            audio_out_label: "[outa]".into(),
        }
    }
}

/// Build the per-segment video / audio entry labels into `filter`.
/// Threads clip-level graph effects in front of the raw stream so
/// downstream filter graph nodes (concat, xfade) read the post-effect
/// labels.
///
/// Order matters: color correction and LUTs run before speed so
/// visual transforms operate in source-frame space. For audio,
/// `atempo` runs before `volume` so the volume gain applies to the
/// time-stretched signal.
///
/// Returns the `(video_label, audio_label)` pair to feed into the
/// next filter graph node.
fn stage_segment_inputs(filter: &mut String, i: usize, seg: &TimelineSegment) -> (String, String) {
    let mut video_label = format!("[{i}:v:0]");
    let mut audio_label = format!("[{i}:a:0]");

    if let Some(color) = seg.color_correction.as_ref()
        && let Some(chain) = color_filter_chain(color)
    {
        let cv = format!("[cv{i}]");
        filter.push_str(&format!("{video_label}{chain}{cv};"));
        video_label = cv;
    }

    if let Some(lut_path) = seg.lut_path.as_ref() {
        let lv = format!("[lv{i}]");
        let path = filter_escape_single_quoted(&lut_path.to_string_lossy());
        filter.push_str(&format!("{video_label}lut3d=file='{path}'{lv};"));
        video_label = lv;
    }

    // Audio repair runs before speed/fades/gain/loudness on each clip:
    // cleanup -> EQ -> dynamics. Track-level FX are applied after
    // per-track concat.
    if let Some(fx) = seg.audio_fx.as_ref()
        && let Some(chain) = audio_fx_filter_chain(fx)
    {
        let fx_label = format!("[afx{i}]");
        filter.push_str(&format!("{audio_label}{chain}{fx_label};"));
        audio_label = fx_label;
    }

    // Speed after color/audio cleanup: setpts on video, atempo
    // (possibly chained) on audio.
    if let Some(factor) = seg.speed
        && (factor - 1.0).abs() > 1e-9
        && factor > 0.0
    {
        let sv = format!("[sv{i}]");
        filter.push_str(&format!(
            "{video_label}setpts={inv}*PTS{sv};",
            inv = 1.0 / factor,
        ));
        video_label = sv;

        let sa = format!("[sa{i}]");
        let chain = atempo_chain(factor);
        filter.push_str(&format!("{audio_label}{chain}{sa};"));
        audio_label = sa;
    }

    // Volume next: applies to whatever the audio_label currently
    // points at (raw input or speed-stretched stream).
    if let Some(v) = seg.volume
        && (v - 1.0).abs() > 1e-9
    {
        let av = format!("[av{i}]");
        filter.push_str(&format!("{audio_label}volume={v}{av};"));
        audio_label = av;
    }
    (video_label, audio_label)
}

fn audio_fx_filter_chain(plan: &AudioFxPlan) -> Option<String> {
    let mut filters = Vec::new();

    // cleanup
    if let Some(freq) = positive(plan.high_pass_hz) {
        filters.push(format!("highpass=f={}", fmt_filter_num(freq)));
    }
    if let Some(freq) = positive(plan.low_pass_hz) {
        filters.push(format!("lowpass=f={}", fmt_filter_num(freq)));
    }
    if let Some(threshold_db) = finite(plan.noise_gate_threshold_db) {
        filters.push(format!(
            "agate=threshold={}:ratio=2:attack=8:release=120",
            fmt_filter_num(db_to_linear(threshold_db).clamp(0.000001, 1.0))
        ));
    }
    if let Some(freq) = positive(plan.hum_notch_hz) {
        filters.push(format!(
            "bandstop=f={}:width_type=h:width=4",
            fmt_filter_num(freq)
        ));
        filters.push(format!(
            "bandstop=f={}:width_type=h:width=4",
            fmt_filter_num(freq * 2.0)
        ));
    }

    // EQ
    for band in &plan.eq_bands {
        if band.freq_hz.is_finite() && band.freq_hz > 0.0 && band.gain_db.is_finite() {
            let width = band.width_hz.filter(|v| v.is_finite() && *v > 0.0);
            filters.push(format!(
                "equalizer=f={}:width_type=h:width={}:g={}",
                fmt_filter_num(band.freq_hz),
                fmt_filter_num(width.unwrap_or(120.0)),
                fmt_filter_num(band.gain_db),
            ));
        }
    }
    if let Some(freq) = positive(plan.de_ess_hz) {
        let reduction = finite(plan.de_ess_reduction_db).unwrap_or(4.0).abs();
        filters.push(format!(
            "equalizer=f={}:width_type=h:width=2500:g=-{}",
            fmt_filter_num(freq),
            fmt_filter_num(reduction),
        ));
    }

    // dynamics and final loudness
    if let Some(threshold) = finite(plan.compressor_threshold_db) {
        let ratio = finite(plan.compressor_ratio).unwrap_or(3.0).max(1.0);
        filters.push(format!(
            "acompressor=threshold={}dB:ratio={}:attack=5:release=80",
            fmt_filter_num(threshold),
            fmt_filter_num(ratio),
        ));
    }
    if let Some(limit_db) = finite(plan.limiter_limit_db) {
        filters.push(format!(
            "alimiter=limit={}",
            fmt_filter_num(db_to_linear(limit_db).clamp(0.000001, 1.0))
        ));
    }
    if let Some(i) = finite(plan.loudnorm_i) {
        let tp = finite(plan.loudnorm_tp).unwrap_or(-1.5);
        filters.push(format!(
            "loudnorm=I={}:TP={}:LRA=11",
            fmt_filter_num(i),
            fmt_filter_num(tp),
        ));
    }

    if filters.is_empty() {
        None
    } else {
        Some(filters.join(","))
    }
}

fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite())
}

fn positive(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite() && *v > 0.0)
}

fn db_to_linear(db: f64) -> f64 {
    10_f64.powf(db / 20.0)
}

fn color_filter_chain(plan: &ColorCorrectionPlan) -> Option<String> {
    let mut filters = Vec::new();
    let exposure = plan.exposure_ev.unwrap_or(0.0);
    let contrast = plan.contrast.unwrap_or(1.0);
    let saturation = plan.saturation.unwrap_or(1.0);
    if exposure.abs() > 1e-9 || (contrast - 1.0).abs() > 1e-9 || (saturation - 1.0).abs() > 1e-9 {
        let shadows = plan.shadows.unwrap_or(0.0);
        let highlights = plan.highlights.unwrap_or(0.0);
        let brightness = (exposure * 0.14 + shadows * 0.06 + highlights * 0.03).clamp(-1.0, 1.0);
        filters.push(format!(
            "eq=brightness={}:contrast={}:saturation={}",
            fmt_filter_num(brightness),
            fmt_filter_num(contrast),
            fmt_filter_num(saturation),
        ));
    }

    let temperature = plan.temperature.unwrap_or(0.0);
    let tint = plan.tint.unwrap_or(0.0);
    let shadows = plan.shadows.unwrap_or(0.0);
    let highlights = plan.highlights.unwrap_or(0.0);
    if shadows.abs() > 1e-9 || highlights.abs() > 1e-9 {
        let shadow_mid = (0.25 + shadows * 0.16).clamp(0.08, 0.45);
        let highlight_mid = (0.75 + highlights * 0.16).clamp(0.55, 0.92);
        filters.push(format!(
            "curves=all='0/0 0.25/{} 0.75/{} 1/1'",
            fmt_filter_num(shadow_mid),
            fmt_filter_num(highlight_mid),
        ));
    }

    if temperature.abs() > 1e-9 || tint.abs() > 1e-9 {
        let rs = (temperature * 0.1 + tint * 0.045).clamp(-1.0, 1.0);
        let gs = (-tint * 0.09).clamp(-1.0, 1.0);
        let bs = (-temperature * 0.1 + tint * 0.045).clamp(-1.0, 1.0);
        let rh = (temperature * 0.07 + tint * 0.035).clamp(-1.0, 1.0);
        let gh = (-tint * 0.07).clamp(-1.0, 1.0);
        let bh = (-temperature * 0.07 + tint * 0.035).clamp(-1.0, 1.0);
        filters.push(format!(
            "colorbalance=rs={}:gs={}:bs={}:rh={}:gh={}:bh={}",
            fmt_filter_num(rs),
            fmt_filter_num(gs),
            fmt_filter_num(bs),
            fmt_filter_num(rh),
            fmt_filter_num(gh),
            fmt_filter_num(bh),
        ));
    }

    if filters.is_empty() {
        None
    } else {
        Some(filters.join(","))
    }
}

fn fmt_filter_num(value: f64) -> String {
    let mut s = format!("{value:.6}");
    while s.contains('.') && s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s == "-0" { "0".into() } else { s }
}

fn filter_escape_single_quoted(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '\'' => "\\'".chars().collect::<Vec<_>>(),
            ':' => "\\:".chars().collect::<Vec<_>>(),
            ',' => "\\,".chars().collect::<Vec<_>>(),
            other => vec![other],
        })
        .collect()
}

/// Length of the alpha / slide ramp for fade and slide animations,
/// in seconds. Half a second feels intentional without lingering.
const ANIMATION_RAMP_S: f64 = 0.5;

/// Build one ffmpeg `drawtext=...` filter from a title plan.
///
/// Position uses proportional y= expressions so titles survive
/// resolution changes:
///   - top    → `y=h*0.05`
///   - center → `y=(h-text_h)/2`
///   - bottom → `y=h*0.85`
///
/// Animations modulate alpha (fade) or x/y (slide) via piecewise
/// expressions; see [`apply_title_animation`] for the math. The
/// `enable=between(t,start,end)` window still bounds the title
/// regardless of animation — fade-out reaches alpha 0 right at
/// `end`, slide-out reaches off-screen right at `end`.
///
/// `text_h` and `text_w` are drawtext-evaluated dimensions of the
/// rendered text; `h` and `w` are the frame dimensions.
fn format_drawtext_filter(t: &TitlePlan) -> String {
    let escaped_text = drawtext_escape(&t.text);
    let resting_y = match t.position {
        TitlePosition::Top => "h*0.05".to_string(),
        TitlePosition::Center => "(h-text_h)/2".to_string(),
        TitlePosition::Bottom => "h*0.85".to_string(),
    };
    let resting_x = "(w-text_w)/2".to_string();
    let weight_attr = match t.font_weight {
        TitleWeight::Normal => "",
        // ffmpeg drawtext doesn't have a `font_weight=` flag — bold
        // is communicated via the fontfile itself. Without a custom
        // bold font bundle, we approximate bold by stroking the
        // text with the same color (`borderw` adds a thicker outline,
        // which visually thickens the strokes).
        TitleWeight::Bold => ":borderw=2",
    };
    let fontfile = pick_fontfile_attr();
    let anim = apply_title_animation(t, &resting_x, &resting_y);
    format!(
        "drawtext=text='{text}'{font}:fontsize={size}:fontcolor={color}{weight}\
         :x={x}:y={y}{alpha}:enable='between(t\\,{start}\\,{end})'",
        text = escaped_text,
        font = fontfile,
        size = t.font_size,
        color = t.color,
        weight = weight_attr,
        x = anim.x,
        y = anim.y,
        alpha = anim.alpha,
        start = t.start_s,
        end = t.end_s,
    )
}

fn format_broadcast_overlay_filters(overlay: &BroadcastOverlayPlan) -> Vec<String> {
    let c = &overlay.config;
    let st = &c.style;
    let mut parts = Vec::new();
    let navy = normalize_hex(&st.dark_navy_hex);
    let gold = normalize_hex(&st.gold_hex);
    let gold_light = normalize_hex(&st.gold_light_hex);
    let cyan = normalize_hex(&st.cyan_hex);
    let show = if c.show_name.trim().is_empty() {
        "BROADCAST".to_string()
    } else {
        c.show_name.to_uppercase()
    };

    if c.short_form_mode {
        return format_short_form_brand_bar(c, &show, &navy, &gold);
    }

    if !c.episode_title.trim().is_empty() {
        parts.extend(format_episode_title_card(c, st, &navy, &gold, &cyan));
    }
    if !c.host_a.name.trim().is_empty() || !c.host_b.name.trim().is_empty() {
        parts.extend(format_host_name_bar(c, st, &navy, &gold));
        parts.extend(format_host_intro_strip(c, st, &gold, &gold_light, &navy));
    }
    parts.extend(format_smart_ticker(c, st, &show, &navy, &gold, &cyan));
    parts.extend(format_chapter_cards(c, st, &navy, &gold));
    parts
}

fn format_short_form_brand_bar(
    c: &BroadcastOverlayConfig,
    show: &str,
    navy: &str,
    gold: &str,
) -> Vec<String> {
    let display = if !c.show_name.trim().is_empty() {
        c.show_name.to_uppercase()
    } else if !c.episode_title.trim().is_empty() {
        c.episode_title.to_uppercase()
    } else {
        show.to_string()
    };
    vec![
        format!("drawbox=x=0:y=ih*0.9583:w=iw:h=ih*0.0417:color={navy}@0.90:t=fill"),
        format!("drawbox=x=0:y=ih*0.9583:w=iw:h=3:color={gold}@1:t=fill"),
        broadcast_drawtext(
            &display,
            "(w-text_w)/2",
            "h*0.983-text_h/2",
            32,
            gold,
            None,
            None,
            true,
        )
        .replace(":enable='between(t\\,0\\,0)'", ""),
    ]
}

fn format_episode_title_card(
    c: &BroadcastOverlayConfig,
    st: &BroadcastOverlayStyle,
    navy: &str,
    gold: &str,
    cyan: &str,
) -> Vec<String> {
    let start = 0.0;
    let end = st.title_visible_end;
    let x = "iw*0.26";
    let y = "ih*0.46";
    let w = "iw*0.48";
    let h = "ih*0.19";
    let alpha = broadcast_title_alpha(st);
    let mut out = vec![
        format!(
            "drawbox=x={x}:y={y}:w={w}:h={h}:color={navy}@0.88:t=fill:enable='between(t\\,{start}\\,{end})'"
        ),
        format!(
            "drawbox=x={x}:y={y}:w={w}:h=4:color={gold}@1:t=fill:enable='between(t\\,{start}\\,{end})'"
        ),
        format!(
            "drawbox=x={x}:y=ih*0.65-2:w={w}:h=2:color={cyan}@0.27:t=fill:enable='between(t\\,{start}\\,{end})'"
        ),
        broadcast_drawtext(
            "EPISODE",
            "w*0.29",
            "h*0.50",
            18,
            gold,
            Some(&alpha),
            Some((start, end)),
            false,
        ),
        broadcast_drawtext(
            &c.episode_title.to_uppercase(),
            "w*0.29",
            "h*0.545",
            55,
            "#FFFFFF",
            Some(&alpha),
            Some((start, end)),
            true,
        ),
    ];
    if !c.episode_subtitle.trim().is_empty() {
        out.push(broadcast_drawtext(
            &c.episode_subtitle,
            "w*0.29",
            "h*0.60",
            23,
            "#CBD5E1",
            Some(&alpha),
            Some((start, end)),
            false,
        ));
    }
    out
}

fn format_host_name_bar(
    c: &BroadcastOverlayConfig,
    st: &BroadcastOverlayStyle,
    navy: &str,
    gold: &str,
) -> Vec<String> {
    let name_h = broadcast_ref_to_1080_px(st.name_bar_height);
    let ticker_h = broadcast_ref_to_1080_px(st.ticker_height);
    let y = format!("ih-{}", fmt_px(name_h + ticker_h));
    let divider_h = broadcast_ref_to_1080_px(64.0);
    let divider_y = format!("ih-{}", fmt_px(ticker_h + (name_h + divider_h) / 2.0));
    let text_y = format!("h-{}", fmt_px(ticker_h + name_h / 2.0 + 16.0));
    let enable = format!(
        "not(between(t\\,{s}\\,{e}))",
        s = st.host_intro_start,
        e = st.host_intro_end
    );
    let mut out = vec![
        format!(
            "drawbox=x=0:y={y}:w=iw:h={h}:color={navy}@0.92:t=fill:enable='{enable}'",
            h = fmt_px(name_h)
        ),
        format!("drawbox=x=0:y={y}:w=iw:h=2:color={gold}@0.55:t=fill:enable='{enable}'"),
        format!(
            "drawbox=x=iw/2:y={divider_y}:w=1:h={divider_h}:color={gold}@0.35:t=fill:enable='{enable}'",
            divider_h = fmt_px(divider_h)
        ),
    ];
    out.extend(format_host_inline_text(&c.host_a, "28", &text_y, &enable));
    out.extend(format_host_inline_text(
        &c.host_b,
        "w-text_w-28",
        &text_y,
        &enable,
    ));
    out
}

fn format_host_inline_text(host: &BroadcastHost, x: &str, y: &str, enable: &str) -> Vec<String> {
    if host.name.trim().is_empty() {
        return Vec::new();
    }
    let text = if host.title.trim().is_empty() {
        host.name.to_uppercase()
    } else {
        format!(
            "{}  {}",
            host.name.to_uppercase(),
            host.title.to_uppercase()
        )
    };
    vec![
        broadcast_drawtext(&text, x, y, 32, "#FFFFFF", None, None, true).replace(
            ":enable='between(t\\,0\\,0)'",
            &format!(":enable='{enable}'"),
        ),
    ]
}

fn format_smart_ticker(
    c: &BroadcastOverlayConfig,
    st: &BroadcastOverlayStyle,
    show: &str,
    navy: &str,
    gold: &str,
    cyan: &str,
) -> Vec<String> {
    let enable = format!(
        "not(between(t\\,{s}\\,{e}))",
        s = st.host_intro_start,
        e = st.host_intro_end
    );
    let ticker_h = broadcast_ref_to_1080_px(st.ticker_height);
    let ticker_y = format!("ih-{}", fmt_px(ticker_h));
    let label_w = broadcast_ref_to_1080_px(680.0);
    let content_x = label_w + broadcast_ref_to_1080_px(48.0);
    let topic_badge_x = content_x;
    let topic_badge_w = broadcast_ref_to_1080_px(278.0);
    let topic_text_x = topic_badge_x + topic_badge_w + broadcast_ref_to_1080_px(28.0);
    let topic_badge_y = format!("ih-{}", fmt_px(ticker_h / 2.0 + 31.0));
    let ticker_text_y = format!("h-{}", fmt_px(ticker_h / 2.0 + 20.0));
    let label_text_y = format!("h-{}", fmt_px(ticker_h / 2.0 + 16.0));
    let cycle = st.ticker_sponsor_duration
        + st.ticker_fade_duration
        + st.ticker_topic_duration
        + st.ticker_fade_duration;
    let sponsor_phase_enable = if let Some(first_topic) = c
        .topics
        .iter()
        .map(|t| t.time_seconds)
        .min_by(f64::total_cmp)
    {
        format!(
            "(lt(t\\,{first_topic})+lt(mod(t\\,{cycle})\\,{sponsor})+gte(mod(t\\,{cycle})\\,{topic_end}))",
            sponsor = st.ticker_sponsor_duration,
            topic_end = cycle - st.ticker_fade_duration
        )
    } else {
        "1".to_string()
    };
    let sponsor_enable = format!(
        "(not(between(t\\,{s}\\,{e}))*{sponsor_phase_enable})",
        s = st.host_intro_start,
        e = st.host_intro_end
    );
    let sponsors = if c.sponsors.is_empty() {
        show.to_string()
    } else {
        c.sponsors
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("   ◆   ")
    };
    let mut out = vec![
        format!(
            "drawbox=x=0:y={ticker_y}:w=iw:h={ticker_h}:color={navy}@1:t=fill:enable='{enable}'",
            ticker_h = fmt_px(ticker_h)
        ),
        format!(
            "drawbox=x=0:y={ticker_y}:w={label_w}:h={ticker_h}:color={gold}@1:t=fill:enable='{enable}'",
            label_w = fmt_px(label_w),
            ticker_h = fmt_px(ticker_h)
        ),
        format!("drawbox=x=0:y={ticker_y}:w=iw:h=3:color={gold}@1:t=fill:enable='{enable}'"),
        broadcast_drawtext(show, "32", &label_text_y, 26, navy, None, None, true).replace(
            ":enable='between(t\\,0\\,0)'",
            &format!(":enable='{enable}'"),
        ),
    ];
    if !c.topics.is_empty() {
        for topic in &c.topics {
            let start = topic.time_seconds;
            let end = start + st.ticker_topic_duration;
            let next_topic = c
                .topics
                .iter()
                .filter_map(|candidate| {
                    (candidate.time_seconds > start).then_some(candidate.time_seconds)
                })
                .min_by(f64::total_cmp)
                .unwrap_or(1.0e12);
            let topic_enable = format!(
                "(not(between(t\\,{host_s}\\,{host_e}))*gte(t\\,{start})*lt(t\\,{next_topic})*gte(mod(t\\,{cycle})\\,{topic_s})*lt(mod(t\\,{cycle})\\,{topic_e}))",
                host_s = st.host_intro_start,
                host_e = st.host_intro_end,
                topic_s = st.ticker_sponsor_duration,
                topic_e = cycle - st.ticker_fade_duration,
            );
            out.push(format!(
                "drawbox=x={x}:y={y}:w={w}:h=31:color={cyan}@1:t=fill:enable='between(t\\,{start}\\,{end})'",
                x = fmt_px(topic_badge_x),
                y = topic_badge_y,
                w = fmt_px(topic_badge_w),
            ).replace(&format!("between(t\\,{start}\\,{end})"), &topic_enable));
            out.push(
                broadcast_drawtext(
                    "NOW DISCUSSING",
                    &fmt_px(topic_badge_x + 12.0),
                    &format!("h-{}", fmt_px(ticker_h / 2.0 + 14.0)),
                    23,
                    navy,
                    None,
                    None,
                    true,
                )
                .replace(
                    ":enable='between(t\\,0\\,0)'",
                    &format!(":enable='{topic_enable}'"),
                ),
            );
            out.push(
                broadcast_drawtext(
                    &topic.text,
                    &fmt_px(topic_text_x),
                    &ticker_text_y,
                    33,
                    "#FFFFFF",
                    None,
                    None,
                    false,
                )
                .replace(
                    ":enable='between(t\\,0\\,0)'",
                    &format!(":enable='{topic_enable}'"),
                ),
            );
        }
    }
    if !sponsors.trim().is_empty() {
        out.push(
            broadcast_drawtext(
                &sponsors,
                &format!("{}-mod(t*70\\,900)", fmt_px(content_x)),
                &ticker_text_y,
                32,
                "#CBD5E1",
                None,
                None,
                true,
            )
            .replace(
                ":enable='between(t\\,0\\,0)'",
                &format!(":enable='{sponsor_enable}'"),
            ),
        );
    }
    out
}

fn format_host_intro_strip(
    c: &BroadcastOverlayConfig,
    st: &BroadcastOverlayStyle,
    gold: &str,
    gold_light: &str,
    navy: &str,
) -> Vec<String> {
    let start = st.host_intro_start;
    let end = st.host_intro_end;
    let enable = format!("between(t\\,{start}\\,{end})");
    let host_h = broadcast_ref_to_1080_px(st.host_strip_height);
    let host_y = format!("ih-{}", fmt_px(host_h));
    let divider_h = broadcast_ref_to_1080_px(110.0);
    let divider_y = format!("ih-{}", fmt_px((host_h + divider_h) / 2.0));
    let name_y = format!("h-{}", fmt_px(host_h / 2.0 - 10.0));
    let title_y = format!("h-{}", fmt_px(host_h / 2.0 + 50.0));
    let host_a_text_x = fmt_px(broadcast_ref_to_1080_px(60.0 + 260.0 + 28.0));
    let host_b_text_x = format!(
        "w-text_w-{}",
        fmt_px(broadcast_ref_to_1080_px(60.0 + 260.0 + 28.0))
    );
    let mut out = vec![
        format!(
            "drawbox=x=0:y={host_y}:w=iw/2:h={host_h}:color={gold}@1:t=fill:enable='{enable}'",
            host_h = fmt_px(host_h)
        ),
        format!(
            "drawbox=x=iw/2:y={host_y}:w=iw/2:h={host_h}:color={gold_light}@1:t=fill:enable='{enable}'",
            host_h = fmt_px(host_h)
        ),
        format!(
            "drawbox=x=iw/2:y={divider_y}:w=2:h={divider_h}:color={navy}@0.2:t=fill:enable='{enable}'",
            divider_h = fmt_px(divider_h)
        ),
    ];
    out.extend(format_host_intro_host(
        &c.host_a,
        &host_a_text_x,
        &name_y,
        &title_y,
        navy,
        start,
        end,
    ));
    out.extend(format_host_intro_host(
        &c.host_b,
        &host_b_text_x,
        &name_y,
        &title_y,
        navy,
        start,
        end,
    ));
    out
}

fn format_host_intro_host(
    host: &BroadcastHost,
    x: &str,
    name_y: &str,
    title_y: &str,
    navy: &str,
    start: f64,
    end: f64,
) -> Vec<String> {
    if host.name.trim().is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.push(broadcast_drawtext(
        &host.name.to_uppercase(),
        x,
        name_y,
        50,
        navy,
        None,
        Some((start, end)),
        true,
    ));
    if !host.title.trim().is_empty() {
        out.push(broadcast_drawtext(
            &host.title.to_uppercase(),
            x,
            title_y,
            29,
            navy,
            None,
            Some((start, end)),
            true,
        ));
    }
    out
}

fn format_chapter_cards(
    c: &BroadcastOverlayConfig,
    st: &BroadcastOverlayStyle,
    navy: &str,
    gold: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    for (idx, chapter) in c.chapters.iter().enumerate() {
        let start = chapter.time_seconds.max(st.title_visible_end);
        let end = start + st.chapter_display_duration;
        out.push(format!("drawbox=x=iw*0.30:y=ih*0.38:w=80:h=50:color={gold}@1:t=fill:enable='between(t\\,{start}\\,{end})'"));
        out.push(format!("drawbox=x=iw*0.30+80:y=ih*0.38:w=iw*0.40:h=50:color={navy}@0.88:t=fill:enable='between(t\\,{start}\\,{end})'"));
        out.push(broadcast_drawtext(
            &(idx + 1).to_string(),
            "w*0.30+32",
            "h*0.38+36",
            36,
            navy,
            None,
            Some((start, end)),
            true,
        ));
        out.push(broadcast_drawtext(
            &chapter.text.to_uppercase(),
            "w*0.30+96",
            "h*0.38+36",
            36,
            "#FFFFFF",
            None,
            Some((start, end)),
            true,
        ));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn broadcast_drawtext(
    text: &str,
    x: &str,
    y: &str,
    size: u32,
    color: &str,
    alpha: Option<&str>,
    window: Option<(f64, f64)>,
    bold: bool,
) -> String {
    let fontfile = pick_fontfile_attr();
    let border = if bold { ":borderw=1" } else { "" };
    let alpha = alpha.map(|a| format!(":alpha='{a}'")).unwrap_or_default();
    let enable = if let Some((start, end)) = window {
        format!(":enable='between(t\\,{start}\\,{end})'")
    } else {
        ":enable='between(t\\,0\\,0)'".into()
    };
    format!(
        "drawtext=text='{text}'{font}:fontsize={size}:fontcolor={color}{border}:x={x}:y={y}{alpha}{enable}",
        text = drawtext_escape(text),
        font = fontfile,
    )
}

fn broadcast_title_alpha(st: &BroadcastOverlayStyle) -> String {
    format!(
        "if(lt(t\\,{fade_in})\\,t/{fade_in}\\,if(lt(t\\,{fade_out})\\,1\\,({end}-t)/({end}-{fade_out})))",
        fade_in = st.title_fade_in_end,
        fade_out = st.title_fade_out_start,
        end = st.title_visible_end,
    )
}

fn normalize_hex(value: &str) -> String {
    if value.starts_with('#') {
        value.to_string()
    } else {
        format!("#{value}")
    }
}

fn broadcast_ref_to_1080_px(value: f64) -> f64 {
    value * 0.5
}

fn fmt_px(value: f64) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < 0.001 {
        format!("{rounded:.0}")
    } else {
        format!("{value:.3}")
    }
}

fn format_broadcast_overlay_graph(
    in_label: &str,
    out_label: &str,
    overlay: &BroadcastOverlayPlan,
    parts: &[String],
) -> String {
    let asset_overlays = broadcast_asset_overlays(overlay);
    if asset_overlays.is_empty() {
        let mut graph = String::new();
        graph.push_str(in_label);
        graph.push_str(&parts.join(","));
        graph.push_str(out_label);
        return graph;
    }

    let mut graph = String::new();
    let mut current = "[broadcast_base]".to_string();
    graph.push_str(in_label);
    graph.push_str(&parts.join(","));
    graph.push_str(&current);
    for (idx, asset) in asset_overlays.iter().enumerate() {
        let photo_label = format!("[broadcast_asset{idx}]");
        let next = if idx + 1 == asset_overlays.len() {
            out_label.to_string()
        } else {
            format!("[broadcast_asset_v{idx}]")
        };
        graph.push(';');
        graph.push_str(&format!(
            "movie='{path}',scale={size}:{size}:force_original_aspect_ratio=increase,crop={size}:{size},format=rgba{photo};{current}{photo}overlay=x={x}:y={y}:enable='between(t\\,{start}\\,{end})'{next}",
            path = filter_escape_single_quoted(&asset.path.to_string_lossy()),
            size = asset.size,
            photo = photo_label,
            current = current,
            x = asset.x,
            y = asset.y,
            start = asset.start_s,
            end = asset.end_s,
            next = next,
        ));
        current = next;
    }
    graph
}

struct BroadcastAssetOverlay {
    path: PathBuf,
    x: String,
    y: String,
    size: u32,
    start_s: f64,
    end_s: f64,
}

fn broadcast_asset_overlays(overlay: &BroadcastOverlayPlan) -> Vec<BroadcastAssetOverlay> {
    let c = &overlay.config;
    let st = &c.style;
    let mut out = Vec::new();
    if c.short_form_mode {
        if let Some(path) = resolve_project_overlay_asset(overlay, c.brand_logo_path.as_deref()) {
            out.push(BroadcastAssetOverlay {
                path,
                x: "32".into(),
                y: "main_h-70".into(),
                size: 52,
                start_s: 0.0,
                end_s: 1.0e9,
            });
        }
        return out;
    }
    let photo_size = broadcast_ref_to_1080_px(260.0).round() as u32;
    let host_h = broadcast_ref_to_1080_px(st.host_strip_height);
    let photo_y = format!("main_h-{}", fmt_px((host_h + f64::from(photo_size)) / 2.0));
    if let Some(path) = resolve_project_overlay_asset(overlay, c.host_a.photo_path.as_deref()) {
        out.push(BroadcastAssetOverlay {
            path,
            x: "30".into(),
            y: photo_y.clone(),
            size: photo_size,
            start_s: st.host_intro_start,
            end_s: st.host_intro_end,
        });
    }
    if let Some(path) = resolve_project_overlay_asset(overlay, c.host_b.photo_path.as_deref()) {
        out.push(BroadcastAssetOverlay {
            path,
            x: "main_w-160".into(),
            y: photo_y,
            size: photo_size,
            start_s: st.host_intro_start,
            end_s: st.host_intro_end,
        });
    }
    out
}

fn resolve_project_overlay_asset(
    overlay: &BroadcastOverlayPlan,
    rel_path: Option<&str>,
) -> Option<PathBuf> {
    let rel_path = rel_path?;
    if rel_path.is_empty() || rel_path.starts_with('/') || rel_path.split('/').any(|p| p == "..") {
        return None;
    }
    let path = overlay.project_root.join(rel_path);
    path.is_file().then_some(path)
}

/// Per-animation x / y / alpha expressions. `x` and `y` always have
/// the resting position as the default; `alpha` is empty when the
/// animation doesn't fade. Fade ramps live in `alpha`; slide ramps
/// live in `x` or `y` depending on the title's resting position.
struct AnimatedExpressions {
    x: String,
    y: String,
    /// Either empty or `:alpha='<expr>'` ready to splice in.
    alpha: String,
}

/// Build the animation expressions for a title. Pure function so it
/// can be unit-tested without spinning up ffmpeg.
fn apply_title_animation(t: &TitlePlan, resting_x: &str, resting_y: &str) -> AnimatedExpressions {
    let start = t.start_s;
    let end = t.end_s;
    let ramp = ANIMATION_RAMP_S;
    let fade_in_end = start + ramp;
    let fade_out_start = (end - ramp).max(start);

    match t.animation {
        TitleAnimation::None => AnimatedExpressions {
            x: resting_x.to_string(),
            y: resting_y.to_string(),
            alpha: String::new(),
        },
        TitleAnimation::FadeIn => AnimatedExpressions {
            x: resting_x.to_string(),
            y: resting_y.to_string(),
            // Linear ramp 0→1 over [start, start+ramp]; 1 thereafter.
            // `if(lt(t,A), B, C)` evaluates B when t<A, else C.
            alpha: format!(":alpha='if(lt(t\\,{fade_in_end})\\,(t-{start})/{ramp}\\,1)'"),
        },
        TitleAnimation::FadeOut => AnimatedExpressions {
            x: resting_x.to_string(),
            y: resting_y.to_string(),
            // 1 until [end-ramp, end], then ramp 1→0.
            alpha: format!(":alpha='if(lt(t\\,{fade_out_start})\\,1\\,({end}-t)/{ramp})'"),
        },
        TitleAnimation::FadeInOut => AnimatedExpressions {
            x: resting_x.to_string(),
            y: resting_y.to_string(),
            // Three pieces: ramp in, plateau, ramp out.
            alpha: format!(
                ":alpha='if(lt(t\\,{fade_in_end})\\,(t-{start})/{ramp}\\,if(lt(t\\,{fade_out_start})\\,1\\,({end}-t)/{ramp}))'"
            ),
        },
        TitleAnimation::SlideIn => slide_expressions(
            t.position,
            resting_x,
            resting_y,
            SlideDirection::In,
            start,
            end,
            ramp,
        ),
        TitleAnimation::SlideOut => slide_expressions(
            t.position,
            resting_x,
            resting_y,
            SlideDirection::Out,
            start,
            end,
            ramp,
        ),
    }
}

#[derive(Clone, Copy)]
enum SlideDirection {
    In,
    Out,
}

/// Build x / y expressions for a slide animation. The slide direction
/// follows the title's resting position:
///
///   - `Top` slides in from above (y goes from `-text_h` to resting).
///   - `Bottom` slides in from below (y from `h` to resting).
///   - `Center` slides in from the right (x from `w` to resting).
///
/// Slide-out reverses each interpolation.
fn slide_expressions(
    position: TitlePosition,
    resting_x: &str,
    resting_y: &str,
    dir: SlideDirection,
    start: f64,
    end: f64,
    ramp: f64,
) -> AnimatedExpressions {
    let ramp_in_end = start + ramp;
    let ramp_out_start = (end - ramp).max(start);
    // For Top/Bottom we slide along Y; for Center we slide along X.
    let slide_along_y = matches!(position, TitlePosition::Top | TitlePosition::Bottom);
    let off_y = match position {
        TitlePosition::Top => "-text_h".to_string(),
        TitlePosition::Bottom => "h".to_string(),
        TitlePosition::Center => resting_y.to_string(),
    };
    let off_x = match position {
        TitlePosition::Center => "w".to_string(),
        _ => resting_x.to_string(),
    };

    // Linear interp helper: write an `if(lt(t,A), <off>+<rest-off>*progress, <rest>)`-style
    // expression. We build it as ffmpeg-string.
    let (x_expr, y_expr) = match (dir, slide_along_y) {
        (SlideDirection::In, true) => (
            resting_x.to_string(),
            // y(t) = off_y + (resting - off_y) * progress, where
            // progress = (t-start)/ramp clamped to 1.
            format!(
                "if(lt(t\\,{ramp_in_end})\\,{off_y}+({resting_y}-({off_y}))*(t-{start})/{ramp}\\,{resting_y})"
            ),
        ),
        (SlideDirection::In, false) => (
            format!(
                "if(lt(t\\,{ramp_in_end})\\,{off_x}+({resting_x}-({off_x}))*(t-{start})/{ramp}\\,{resting_x})"
            ),
            resting_y.to_string(),
        ),
        (SlideDirection::Out, true) => (
            resting_x.to_string(),
            // y(t) = resting until ramp_out_start, then linear to off_y.
            format!(
                "if(lt(t\\,{ramp_out_start})\\,{resting_y}\\,{resting_y}+({off_y}-({resting_y}))*(t-{ramp_out_start})/{ramp})"
            ),
        ),
        (SlideDirection::Out, false) => (
            format!(
                "if(lt(t\\,{ramp_out_start})\\,{resting_x}\\,{resting_x}+({off_x}-({resting_x}))*(t-{ramp_out_start})/{ramp})"
            ),
            resting_y.to_string(),
        ),
    };

    AnimatedExpressions {
        x: x_expr,
        y: y_expr,
        alpha: String::new(),
    }
}

/// Escape characters drawtext treats as special inside `text='...'`.
/// drawtext uses `\` to escape `:`, `'`, `\`, and `,` — we don't
/// support newlines (single-line titles in v1).
fn drawtext_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\\' => "\\\\".to_string(),
            '\'' => "\\'".to_string(),
            ':' => "\\:".to_string(),
            ',' => "\\,".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// Best-effort font lookup. Returns either an empty string (let
/// ffmpeg's drawtext fall back to its default font search) or a
/// `:fontfile=<path>` segment ready to splice into the filter args.
///
/// We probe a small list of well-known system fonts in priority
/// order: macOS Helvetica, Linux DejaVu, Linux Liberation. If none
/// resolve, we omit `fontfile=` and rely on ffmpeg's default —
/// recent ffmpeg builds (5+) handle this gracefully on macOS; older
/// builds may fail at render time with a clear "no fontfile" error.
fn pick_fontfile_attr() -> String {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/HelveticaNeue.ttc",
        "/System/Library/Fonts/Helvetica.ttc",
        "/System/Library/Fonts/Supplemental/Arial Narrow Bold.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ];
    for path in CANDIDATES {
        if std::path::Path::new(path).is_file() {
            return format!(":fontfile={path}");
        }
    }
    String::new()
}

/// Effective on-timeline duration of a segment, accounting for any
/// awidat.speed effect: `duration_s / factor` when factor is set,
/// raw `duration_s` otherwise. A 4s clip at 2× plays in 2s; at 0.5×
/// it plays in 8s.
fn effective_duration(seg: &TimelineSegment) -> f64 {
    match seg.speed {
        Some(f) if f > 0.0 => seg.duration_s / f,
        _ => seg.duration_s,
    }
}

/// Decompose a speed factor into a chain of `atempo=` calls, each
/// in atempo's per-instance legal range `[0.5, 2.0]`. Returns a
/// string like `atempo=2.0,atempo=2.0` for factor=4, or `atempo=0.5,
/// atempo=0.6` for factor=0.3. Caller is responsible for prepending
/// the input label and appending the output label.
fn atempo_chain(factor: f64) -> String {
    // atempo's legal range is [0.5, 2.0] per filter instance.
    // - factor >= 0.5 && factor <= 2.0 → single atempo=<factor>.
    // - factor > 2.0 → chain atempo=2.0 stages until product >= factor,
    //   then a remainder.
    // - factor < 0.5 → chain atempo=0.5 stages until product <= factor,
    //   then a remainder.
    if (0.5..=2.0).contains(&factor) {
        return format!("atempo={factor}");
    }
    let mut stages = Vec::<f64>::new();
    let mut remaining = factor;
    if factor > 2.0 {
        while remaining > 2.0 {
            stages.push(2.0);
            remaining /= 2.0;
        }
    } else {
        while remaining < 0.5 {
            stages.push(0.5);
            remaining /= 0.5;
        }
    }
    stages.push(remaining);
    stages
        .into_iter()
        .map(|s| format!("atempo={s}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Map an OTIO transition kind or Awidat transition id to an ffmpeg
/// `xfade=transition=` name. Project-level render planning validates
/// this earlier; direct test/helper callers get an intentionally
/// invalid token for unsupported values instead of a wrong fallback.
fn map_transition_kind(kind: &str) -> String {
    transitions::resolve_ffmpeg_xfade(kind)
        .ok()
        .flatten()
        .unwrap_or("__unsupported_awidat_transition__")
        .to_string()
}

/// Build the ffmpeg argv that concats `segs` into `output_path` with
/// a single re-encode. The re-encode kills the DTS-seam scratch that
/// stream-copy concat produces at non-keyframe-aligned cut points.
/// libx264 medium preset / CRF 20, AAC 192k — universal compatibility.
///
/// Internally delegates the filter-graph construction to
/// [`FilterPlanner`] (Step 14.4 extraction); behaviour is byte-
/// identical to the prior monolithic builder. Callers wanting
/// transitions should use [`build_timeline_argv_with_transitions`]
/// (Step 14.5).
pub fn build_timeline_argv(segs: &[TimelineSegment], output_path: &Path) -> Vec<String> {
    build_timeline_argv_with_transitions(segs, &[], output_path)
}

/// Like [`build_timeline_argv`] but accepts a transitions slice that
/// gets composed into the filter graph. Wraps
/// [`build_timeline_argv_full`] with no titles — preserved for
/// callers that don't need title awareness.
pub fn build_timeline_argv_with_transitions(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    output_path: &Path,
) -> Vec<String> {
    build_timeline_argv_full(segs, transitions, &[], None, output_path)
}

/// Like [`build_timeline_argv_with_transitions`] but also takes a
/// titles slice. Each [`TitlePlan`] becomes a `drawtext=` filter
/// chained off the master video output of the segment + transition
/// graph. Step 16.3 ships the basic shape (no animation);
/// 16.4 wires alpha / x / y expressions for animations.
pub fn build_timeline_argv_full(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    titles: &[TitlePlan],
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
    output_path: &Path,
) -> Vec<String> {
    let mut argv = vec!["-y".to_string(), "-loglevel".into(), "info".into()];
    for s in segs {
        argv.extend([
            "-ss".into(),
            format!("{}", s.start_s),
            "-t".into(),
            format!("{}", s.duration_s),
            "-i".into(),
            s.asset_path.to_string_lossy().into_owned(),
        ]);
    }
    let plan = FilterPlanner::with_titles_and_broadcast_overlay(
        segs,
        transitions,
        titles,
        broadcast_overlay,
    )
    .plan();
    argv.extend([
        "-filter_complex".into(),
        plan.filter_complex,
        "-map".into(),
        plan.video_out_label,
        "-map".into(),
        plan.audio_out_label,
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "20".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        output_path.to_string_lossy().into_owned(),
    ]);
    argv
}

/// Build an ffmpeg argv for explicit audio-track mixing plus video timeline rendering.
/// Build an ffmpeg argv for timelines with first-class audio tracks.
/// Video streams are rendered video-only and final audio is mixed from
/// explicit audio-track plans.
pub fn build_timeline_argv_with_audio_tracks(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    titles: &[TitlePlan],
    broadcast_overlay: Option<&BroadcastOverlayPlan>,
    audio_tracks: &[AudioTrackPlan],
    output_path: &Path,
) -> Vec<String> {
    let mut argv = vec!["-y".to_string(), "-loglevel".into(), "info".into()];
    for s in segs {
        argv.extend([
            "-ss".into(),
            format!("{}", s.start_s),
            "-t".into(),
            format!("{}", s.duration_s),
            "-i".into(),
            s.asset_path.to_string_lossy().into_owned(),
        ]);
    }
    for track in audio_tracks {
        for item in &track.items {
            let AudioTrackItemPlan::Clip(c) = item else {
                continue;
            };
            argv.extend([
                "-ss".into(),
                format!("{}", c.start_s),
                "-t".into(),
                format!("{}", c.duration_s),
                "-i".into(),
                c.asset_path.to_string_lossy().into_owned(),
            ]);
        }
    }

    let fallback_video_duration = audio_tracks
        .iter()
        .map(audio_track_duration)
        .fold(0.1_f64, f64::max);
    let mut filter = plan_video_only_filter(segs, transitions, fallback_video_duration);
    let base_video_label = "[vonly]";
    let mut video_label = base_video_label.to_string();
    if !titles.is_empty() || broadcast_overlay.is_some() {
        let decorated =
            FilterPlanner::with_titles_and_broadcast_overlay(&[], &[], titles, broadcast_overlay)
                .decorate_video_filter(filter, video_label.clone());
        filter = decorated.filter_complex;
        video_label = decorated.video_out_label;
    }

    let mut next_input = segs.len();
    let audio_label = plan_audio_mix_filter(&mut filter, audio_tracks, &mut next_input);
    argv.extend([
        "-filter_complex".into(),
        filter,
        "-map".into(),
        video_label,
        "-map".into(),
        audio_label,
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "20".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        output_path.to_string_lossy().into_owned(),
    ]);
    argv
}

fn plan_video_only_filter(
    segs: &[TimelineSegment],
    transitions: &[TransitionPlan],
    fallback_duration_s: f64,
) -> String {
    let mut filter = String::new();
    if segs.is_empty() {
        filter.push_str(&format!(
            "color=c=black:s=1280x720:r=30:d={fallback_duration_s}[vonly];"
        ));
        return filter;
    }
    let video_inputs: Vec<String> = (0..segs.len())
        .map(|i| stage_segment_video_input(&mut filter, i, &segs[i]))
        .collect();
    if transitions.is_empty() {
        for v in &video_inputs {
            filter.push_str(v);
        }
        filter.push_str(&format!("concat=n={}:v=1:a=0[vonly];", video_inputs.len()));
        return filter;
    }

    // Explicit audio projects still preserve the common visual xfade
    // case; audio transitions are owned by the audio tracks.
    let mut used = vec![false; segs.len()];
    let mut concat_inputs = Vec::new();
    for t in transitions {
        if t.from_segment_index + 1 != t.to_segment_index
            || t.to_segment_index >= segs.len()
            || used[t.from_segment_index]
            || used[t.to_segment_index]
        {
            continue;
        }
        let offset = (effective_duration(&segs[t.from_segment_index]) - t.duration_s).max(0.0);
        let label = format!("[xv{}]", t.from_segment_index);
        filter.push_str(&format!(
            "{}{}xfade=transition={}:duration={}:offset={}{};",
            video_inputs[t.from_segment_index],
            video_inputs[t.to_segment_index],
            map_transition_kind(&t.kind),
            t.duration_s,
            offset,
            label
        ));
        concat_inputs.push(label);
        used[t.from_segment_index] = true;
        used[t.to_segment_index] = true;
    }
    for (i, v) in video_inputs.iter().enumerate() {
        if !used[i] {
            concat_inputs.push(v.clone());
        }
    }
    for v in &concat_inputs {
        filter.push_str(v);
    }
    filter.push_str(&format!("concat=n={}:v=1:a=0[vonly];", concat_inputs.len()));
    filter
}

fn audio_track_duration(track: &AudioTrackPlan) -> f64 {
    track
        .items
        .iter()
        .map(|item| match item {
            AudioTrackItemPlan::Clip(c) => {
                if let Some(factor) = c.speed
                    && factor > 0.0
                {
                    return c.duration_s / factor;
                }
                c.duration_s
            }
            AudioTrackItemPlan::Gap { duration_s } => *duration_s,
        })
        .sum()
}

fn stage_segment_video_input(filter: &mut String, i: usize, seg: &TimelineSegment) -> String {
    let mut video_label = format!("[{i}:v:0]");
    if let Some(color) = seg.color_correction.as_ref()
        && let Some(chain) = color_filter_chain(color)
    {
        let cv = format!("[cv{i}]");
        filter.push_str(&format!("{video_label}{chain}{cv};"));
        video_label = cv;
    }
    if let Some(lut_path) = seg.lut_path.as_ref() {
        let lv = format!("[lv{i}]");
        let path = filter_escape_single_quoted(&lut_path.to_string_lossy());
        filter.push_str(&format!("{video_label}lut3d=file='{path}'{lv};"));
        video_label = lv;
    }
    if let Some(factor) = seg.speed
        && (factor - 1.0).abs() > 1e-9
        && factor > 0.0
    {
        let sv = format!("[sv{i}]");
        filter.push_str(&format!(
            "{video_label}setpts={inv}*PTS{sv};",
            inv = 1.0 / factor,
        ));
        video_label = sv;
    }
    video_label
}

fn plan_audio_mix_filter(
    filter: &mut String,
    audio_tracks: &[AudioTrackPlan],
    next_input: &mut usize,
) -> String {
    let solo_active = audio_tracks.iter().any(|t| t.solo);
    let mut audible = Vec::<(&AudioTrackPlan, String)>::new();
    for (track_index, track) in audio_tracks.iter().enumerate() {
        if track.muted || (solo_active && !track.solo) || track.items.is_empty() {
            continue;
        }
        let label = plan_one_audio_track(filter, track_index, track, next_input);
        audible.push((track, label));
    }
    if audible.is_empty() {
        filter.push_str("anullsrc=r=48000:cl=stereo:d=0.1[finala];");
        return "[finala]".into();
    }

    let ducking_needed = audible.iter().any(|(t, _)| {
        t.role != "dialogue" && t.ducking.as_ref().map(|d| d.enabled).unwrap_or(false)
    });
    let mut audible_labels: Vec<String> = audible.iter().map(|(_, label)| label.clone()).collect();
    let mut dialogue_labels = Vec::new();
    if ducking_needed {
        for (idx, (track, label)) in audible.iter().enumerate() {
            if track.role == "dialogue" {
                let main = format!("[dlgmain{idx}]");
                let sc = format!("[dlgsc{idx}]");
                filter.push_str(&format!("{label}asplit=2{main}{sc};"));
                audible_labels[idx] = main;
                dialogue_labels.push(sc);
            }
        }
    }
    let sidechain_label = if dialogue_labels.is_empty() {
        None
    } else if dialogue_labels.len() == 1 {
        Some(dialogue_labels[0].clone())
    } else {
        for l in &dialogue_labels {
            filter.push_str(l);
        }
        filter.push_str(&format!(
            "amix=inputs={}:duration=longest[ducksc];",
            dialogue_labels.len()
        ));
        Some("[ducksc]".into())
    };

    let mut mix_labels = Vec::new();
    for (idx, (track, _)) in audible.iter().enumerate() {
        let label = &audible_labels[idx];
        if let (Some(duck), Some(sidechain)) = (track.ducking.as_ref(), sidechain_label.as_ref())
            && duck.enabled
            && track.role != "dialogue"
        {
            let out = format!("[ducked{idx}]");
            filter.push_str(&format!(
                "{label}{sidechain}sidechaincompress=threshold=0.05:ratio=8:attack={}:release={}{};",
                duck.attack_ms, duck.release_ms, out
            ));
            mix_labels.push(out);
            continue;
        }
        mix_labels.push(label.to_string());
    }
    for l in &mix_labels {
        filter.push_str(l);
    }
    filter.push_str(&format!(
        "amix=inputs={}:duration=longest:dropout_transition=0[finala];",
        mix_labels.len()
    ));
    "[finala]".into()
}

fn plan_one_audio_track(
    filter: &mut String,
    track_index: usize,
    track: &AudioTrackPlan,
    next_input: &mut usize,
) -> String {
    let mut item_labels = Vec::new();
    for (item_index, item) in track.items.iter().enumerate() {
        match item {
            AudioTrackItemPlan::Gap { duration_s } => {
                let label = format!("[agap{track_index}_{item_index}]");
                filter.push_str(&format!(
                    "anullsrc=r=48000:cl=stereo:d={duration_s}{label};"
                ));
                item_labels.push(label);
            }
            AudioTrackItemPlan::Clip(clip) => {
                let input = *next_input;
                *next_input += 1;
                let mut label = format!("[{input}:a:0]");
                let trimmed = format!("[atrim{track_index}_{item_index}]");
                let source_end = clip.start_s + clip.duration_s;
                filter.push_str(&format!(
                    "{label}atrim={}:{},asetpts=PTS-STARTPTS{};",
                    clip.start_s, source_end, trimmed
                ));
                label = trimmed;
                if let Some(fx) = clip.audio_fx.as_ref()
                    && let Some(chain) = audio_fx_filter_chain(fx)
                {
                    let fx_label = format!("[acfx{track_index}_{item_index}]");
                    filter.push_str(&format!("{label}{chain}{fx_label};"));
                    label = fx_label;
                }
                if let Some(factor) = clip.speed
                    && (factor - 1.0).abs() > 1e-9
                    && factor > 0.0
                {
                    let sped = format!("[aspeed{track_index}_{item_index}]");
                    filter.push_str(&format!("{label}{}{};", atempo_chain(factor), sped));
                    label = sped;
                }
                if let Some(v) = clip.volume
                    && (v - 1.0).abs() > 1e-9
                {
                    let vol = format!("[avol{track_index}_{item_index}]");
                    filter.push_str(&format!("{label}volume={v}{vol};"));
                    label = vol;
                }
                if let Some(fade_in) = clip.fade_in_s
                    && fade_in > 0.0
                {
                    let fade = format!("[afi{track_index}_{item_index}]");
                    filter.push_str(&format!("{label}afade=t=in:st=0:d={fade_in}{fade};"));
                    label = fade;
                }
                if let Some(fade_out) = clip.fade_out_s
                    && fade_out > 0.0
                {
                    let fade = format!("[afo{track_index}_{item_index}]");
                    let st = (clip.duration_s - fade_out).max(0.0);
                    filter.push_str(&format!("{label}afade=t=out:st={st}:d={fade_out}{fade};"));
                    label = fade;
                }
                item_labels.push(label);
            }
        }
    }
    for l in &item_labels {
        filter.push_str(l);
    }
    let track_label = format!("[atrack{track_index}]");
    filter.push_str(&format!(
        "concat=n={}:v=0:a=1{};",
        item_labels.len(),
        track_label
    ));
    let mut track_label = track_label;
    if let Some(fx) = track.audio_fx.as_ref()
        && let Some(chain) = audio_fx_filter_chain(fx)
    {
        let out = format!("[atrackfx{track_index}]");
        filter.push_str(&format!("{track_label}{chain}{out};"));
        track_label = out;
    }
    if (track.volume - 1.0).abs() > 1e-9 {
        let out = format!("[atrackv{track_index}]");
        filter.push_str(&format!("{track_label}volume={}{};", track.volume, out));
        out
    } else {
        track_label
    }
}

/// One-call helper: walk OTIO, build the spec, return it. The output
/// path is `<project_root>/renders/timeline-<HHMMSS>.mp4` — same
/// naming `start_render scope=timeline` uses, so the agent and the
/// desktop produce indistinguishable artifacts.
pub fn build_timeline_render_spec(
    project_root: &Path,
) -> Result<RenderJobSpec, RenderTimelineError> {
    let (segs, transitions, titles, broadcast_overlay, audio_tracks) =
        collect_timeline_full_plan(project_root)?;
    if segs.is_empty() && audio_tracks.is_empty() {
        return Err(RenderTimelineError::EmptyTimeline);
    }
    // Total duration is the sum of each segment's *effective*
    // duration (post-speed) minus the cumulative transition
    // overlap. A 4s clip at 2× contributes 2s; a 0.5s transition
    // overlaps two effective durations by 0.5s. Titles are
    // overlays — they don't add to the master timeline length.
    let raw_total: f64 = if segs.is_empty() {
        audio_tracks
            .iter()
            .map(audio_track_duration)
            .fold(0.0_f64, f64::max)
    } else {
        segs.iter().map(effective_duration).sum()
    };
    let trans_total: f64 = transitions.iter().map(|t| t.duration_s).sum();
    let total_duration_s = (raw_total - trans_total).max(0.0);
    let renders_dir = project_root.join("renders");
    let timestamp = Utc::now().format("%H%M%S");
    let output_path = renders_dir.join(format!("timeline-{timestamp}.mp4"));
    let argv = if audio_tracks.is_empty() {
        build_timeline_argv_full(
            &segs,
            &transitions,
            &titles,
            broadcast_overlay.as_ref(),
            &output_path,
        )
    } else {
        build_timeline_argv_with_audio_tracks(
            &segs,
            &transitions,
            &titles,
            broadcast_overlay.as_ref(),
            &audio_tracks,
            &output_path,
        )
    };
    Ok(RenderJobSpec {
        args: argv,
        total_duration_s: Some(total_duration_s),
        cwd: Some(project_root.to_path_buf()),
        output_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild,
        TimeRange as OtioRange, Timeline, Track, TrackChild, TrackKind,
    };
    use std::fs;

    fn write_fixture_project(dir: &Path) -> PathBuf {
        let asset_rel = "raw/x.mp4";
        fs::create_dir_all(dir.join("raw")).unwrap();
        fs::write(dir.join(asset_rel), b"stub").unwrap();
        let mut clip = Clip::empty("c1".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_rel));
        clip.source_range = Some(OtioRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        tl.tracks = stack;
        let otio_path = dir.join(files::OTIO);
        fs::write(&otio_path, serde_json::to_string_pretty(&tl).unwrap()).unwrap();
        otio_path
    }

    #[test]
    fn no_otio_returns_no_otio_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = build_timeline_render_spec(dir.path()).unwrap_err();
        assert!(matches!(err, RenderTimelineError::NoOtio(_)));
    }

    #[test]
    fn empty_otio_returns_empty_timeline() {
        let dir = tempfile::tempdir().unwrap();
        // Init an OTIO file with no tracks.
        awidat_proto::project::Project::init(dir.path()).unwrap();
        let err = build_timeline_render_spec(dir.path()).unwrap_err();
        assert!(matches!(err, RenderTimelineError::EmptyTimeline));
    }

    #[test]
    fn fixture_project_produces_concat_argv() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project(dir.path());
        let spec = build_timeline_render_spec(dir.path()).unwrap();
        assert!(spec.total_duration_s.unwrap() > 1.9);
        // Concat filter present, libx264 (re-encode, not stream-copy).
        let cmd = spec.args.join(" ");
        assert!(cmd.contains("concat=n=1:v=1:a=1"));
        assert!(cmd.contains("libx264"));
        assert!(!cmd.contains(" copy "));
        // Output under renders/ with timeline-<HHMMSS>.mp4 naming.
        assert!(spec.output_path.starts_with(dir.path().join("renders")));
        assert!(
            spec.output_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("timeline-")
        );
    }

    #[test]
    fn missing_asset_returns_missing_asset_error() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture_project(dir.path());
        // Delete the asset; spec build should fail with MissingAsset.
        fs::remove_file(dir.path().join("raw/x.mp4")).unwrap();
        let err = build_timeline_render_spec(dir.path()).unwrap_err();
        assert!(matches!(err, RenderTimelineError::MissingAsset { .. }));
    }

    /// Build a basic TimelineSegment for tests — no volume / speed
    /// effects. Saves repeating `..Default::default()` 12 times.
    fn seg(path: &str, start: f64, dur: f64) -> TimelineSegment {
        TimelineSegment {
            asset_path: PathBuf::from(path),
            start_s: start,
            duration_s: dur,
            ..Default::default()
        }
    }

    #[test]
    fn explicit_audio_tracks_use_video_only_concat_and_amix() {
        let segs = vec![seg("/tmp/a.mp4", 0.0, 2.0)];
        let audio_tracks = vec![AudioTrackPlan {
            name: "A1".into(),
            role: "dialogue".into(),
            volume: 0.8,
            muted: false,
            solo: false,
            ducking: None,
            audio_fx: None,
            items: vec![
                AudioTrackItemPlan::Clip(AudioClipPlan {
                    asset_path: PathBuf::from("/tmp/a.wav"),
                    start_s: 0.0,
                    duration_s: 2.0,
                    volume: Some(0.5),
                    speed: None,
                    fade_in_s: Some(0.1),
                    fade_out_s: Some(0.2),
                    audio_fx: None,
                }),
                AudioTrackItemPlan::Gap { duration_s: 1.0 },
            ],
        }];
        let argv = build_timeline_argv_with_audio_tracks(
            &segs,
            &[],
            &[],
            None,
            &audio_tracks,
            Path::new("/tmp/out.mp4"),
        );
        let filter = argv
            .windows(2)
            .find_map(|w| (w[0] == "-filter_complex").then(|| w[1].clone()))
            .unwrap();
        assert!(filter.contains("concat=n=1:v=1:a=0[vonly]"));
        assert!(filter.contains("atrim=0:2"));
        assert!(filter.contains("volume=0.5"));
        assert!(filter.contains("afade=t=in"));
        assert!(filter.contains("anullsrc"));
        assert!(filter.contains("amix=inputs=1"));
    }

    #[test]
    fn filter_planner_with_no_transitions_emits_legacy_concat_graph() {
        // Step 14.4 extracted FilterPlanner from build_timeline_argv;
        // this test pins the no-transition graph shape so future
        // commits can't drift it without noticing.
        let segs = vec![seg("/tmp/a.mp4", 0.0, 2.0), seg("/tmp/b.mp4", 1.0, 3.0)];
        let plan = FilterPlanner::new(&segs, &[]).plan();
        assert_eq!(
            plan.filter_complex,
            "[0:v:0][0:a:0][1:v:0][1:a:0]concat=n=2:v=1:a=1[outv][outa]",
        );
        assert_eq!(plan.video_out_label, "[outv]");
        assert_eq!(plan.audio_out_label, "[outa]");
    }

    #[test]
    fn filter_planner_with_one_transition_emits_xfade_pair() {
        let segs = vec![seg("/tmp/a.mp4", 0.0, 5.0), seg("/tmp/b.mp4", 0.0, 4.0)];
        let trans = vec![TransitionPlan {
            from_segment_index: 0,
            to_segment_index: 1,
            kind: "SMPTE_Dissolve".into(),
            duration_s: 1.0,
        }];
        let plan = FilterPlanner::new(&segs, &trans).plan();
        // xfade with kind=fade (mapped from SMPTE_Dissolve), offset =
        // from.duration - transition.duration = 4.0.
        assert!(
            plan.filter_complex
                .contains("xfade=transition=fade:duration=1:offset=4"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // acrossfade for audio.
        assert!(
            plan.filter_complex.contains("acrossfade=d=1"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // The chunk pair feeds into a 1-input concat (the merged xfade
        // counts as one input pair).
        assert!(
            plan.filter_complex
                .contains("concat=n=1:v=1:a=1[outv][outa]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_with_transition_in_middle_of_three_segments() {
        // [A, B, C] with a transition between A and B → concat n=2:
        // input 1 = xfade(A, B), input 2 = C alone.
        let segs = vec![
            seg("/tmp/a.mp4", 0.0, 3.0),
            seg("/tmp/b.mp4", 0.0, 4.0),
            seg("/tmp/c.mp4", 0.0, 2.0),
        ];
        let trans = vec![TransitionPlan {
            from_segment_index: 0,
            to_segment_index: 1,
            kind: "SMPTE_Dissolve".into(),
            duration_s: 0.5,
        }];
        let plan = FilterPlanner::new(&segs, &trans).plan();
        assert!(plan.filter_complex.contains("xfade="));
        // Concat takes 2 inputs: chunk(A,B) + raw C.
        assert!(
            plan.filter_complex
                .contains("concat=n=2:v=1:a=1[outv][outa]")
        );
        // C's raw streams must appear in the concat input list.
        assert!(plan.filter_complex.contains("[2:v:0][2:a:0]"));
    }

    #[test]
    fn filter_planner_drops_chained_transitions() {
        // [A, B, C] with transitions A-B AND B-C: B can only belong
        // to one chunk in v1. The first transition wins; the second
        // is dropped (with a debug-trace log we can't easily assert
        // here). Resulting concat: xfade(A,B) + raw C.
        let segs = vec![
            seg("/tmp/a.mp4", 0.0, 3.0),
            seg("/tmp/b.mp4", 0.0, 4.0),
            seg("/tmp/c.mp4", 0.0, 2.0),
        ];
        let trans = vec![
            TransitionPlan {
                from_segment_index: 0,
                to_segment_index: 1,
                kind: "SMPTE_Dissolve".into(),
                duration_s: 0.5,
            },
            TransitionPlan {
                from_segment_index: 1,
                to_segment_index: 2,
                kind: "SMPTE_Dissolve".into(),
                duration_s: 0.5,
            },
        ];
        let plan = FilterPlanner::new(&segs, &trans).plan();
        // Exactly one xfade (A-B).
        let xfade_count = plan.filter_complex.matches("xfade=").count();
        assert_eq!(xfade_count, 1, "filter graph: {}", plan.filter_complex);
        // C still in the concat as a raw input.
        assert!(plan.filter_complex.contains("[2:v:0][2:a:0]"));
    }

    #[test]
    fn filter_planner_emits_volume_filter_when_segment_carries_value() {
        // Step 15.3: a segment with volume=0.5 prepends a volume=
        // filter that produces [av0], then the concat consumes
        // [av0] in place of the raw [0:a:0].
        let mut s0 = seg("/tmp/a.mp4", 0.0, 2.0);
        s0.volume = Some(0.5);
        let s1 = seg("/tmp/b.mp4", 0.0, 3.0);
        let plan = FilterPlanner::new(&[s0, s1], &[]).plan();
        assert!(
            plan.filter_complex.contains("[0:a:0]volume=0.5[av0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // Concat input pair for seg 0 uses [av0] for audio, raw for video.
        assert!(
            plan.filter_complex.contains("[0:v:0][av0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // Seg 1 has no volume effect — raw labels.
        assert!(
            plan.filter_complex.contains("[1:v:0][1:a:0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_skips_volume_filter_at_unity() {
        // volume = 1.0 is the no-op default; no filter should land.
        let mut s0 = seg("/tmp/a.mp4", 0.0, 2.0);
        s0.volume = Some(1.0);
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            !plan.filter_complex.contains("volume="),
            "filter graph should skip unity volume: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_volume_threads_through_xfade_pair() {
        // Volume on the to-segment of an xfade pair must feed the
        // [av<i>] label into acrossfade, not the raw [i:a:0].
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let mut s1 = seg("/tmp/b.mp4", 0.0, 4.0);
        s1.volume = Some(0.3);
        let trans = vec![TransitionPlan {
            from_segment_index: 0,
            to_segment_index: 1,
            kind: "SMPTE_Dissolve".into(),
            duration_s: 1.0,
        }];
        let plan = FilterPlanner::new(&[s0, s1], &trans).plan();
        assert!(
            plan.filter_complex.contains("[1:a:0]volume=0.3[av1]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // acrossfade reads [0:a:0] (no volume on s0) and [av1] (volume on s1).
        assert!(
            plan.filter_complex.contains("[0:a:0][av1]acrossfade"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_audio_fx_runs_before_speed_and_volume() {
        let mut s = seg("/tmp/a.mp4", 0.0, 4.0);
        s.audio_fx = Some(AudioFxPlan {
            high_pass_hz: Some(80.0),
            hum_notch_hz: Some(60.0),
            compressor_threshold_db: Some(-18.0),
            compressor_ratio: Some(2.5),
            limiter_limit_db: Some(-1.0),
            loudnorm_i: Some(-16.0),
            loudnorm_tp: Some(-1.5),
            ..Default::default()
        });
        s.speed = Some(1.1);
        s.volume = Some(0.8);
        let plan = FilterPlanner::new(&[s], &[]).plan();
        let filter = plan.filter_complex;
        let fx_pos = filter.find("highpass=f=80").unwrap();
        let speed_pos = filter.find("atempo=1.1").unwrap();
        let volume_pos = filter.find("volume=0.8").unwrap();
        assert!(fx_pos < speed_pos, "filter graph: {filter}");
        assert!(speed_pos < volume_pos, "filter graph: {filter}");
        assert!(filter.contains("bandstop=f=60"));
        assert!(filter.contains("acompressor=threshold=-18dB:ratio=2.5"));
        assert!(filter.contains("loudnorm=I=-16:TP=-1.5:LRA=11"));
    }

    #[test]
    fn filter_planner_emits_setpts_and_atempo_for_speed_segment() {
        // Step 15.4: a segment with speed=2.0 prepends setpts on
        // video and atempo on audio, threads the [sv<i>]/[sa<i>]
        // labels into the concat.
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.speed = Some(2.0);
        let s1 = seg("/tmp/b.mp4", 0.0, 3.0);
        let plan = FilterPlanner::new(&[s0, s1], &[]).plan();
        // setpts on video with 1/factor.
        assert!(
            plan.filter_complex.contains("[0:v:0]setpts=0.5*PTS[sv0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // atempo single-stage (factor 2.0 sits inside [0.5, 2.0]).
        assert!(
            plan.filter_complex.contains("[0:a:0]atempo=2[sa0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // Concat for seg 0 reads [sv0][sa0].
        assert!(
            plan.filter_complex.contains("[sv0][sa0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_chains_atempo_for_extreme_speed() {
        // factor=4.0 → atempo=2.0 twice (2.0 × 2.0 = 4.0).
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.speed = Some(4.0);
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex.contains("atempo=2,atempo=2"),
            "expected chained atempo for factor=4, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_chains_atempo_for_slow_speed() {
        // factor=0.25 → atempo=0.5 twice (0.5 × 0.5 = 0.25).
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.speed = Some(0.25);
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex.contains("atempo=0.5,atempo=0.5"),
            "expected chained atempo for factor=0.25, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_speed_uses_effective_duration_for_xfade_offset() {
        // 4s @ 2× = 2s effective. xfade duration 0.5 → offset 1.5
        // (effective − transition.duration).
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.speed = Some(2.0);
        let s1 = seg("/tmp/b.mp4", 0.0, 3.0);
        let trans = vec![TransitionPlan {
            from_segment_index: 0,
            to_segment_index: 1,
            kind: "SMPTE_Dissolve".into(),
            duration_s: 0.5,
        }];
        let plan = FilterPlanner::new(&[s0, s1], &trans).plan();
        assert!(
            plan.filter_complex.contains("offset=1.5"),
            "expected offset=1.5 (post-speed), got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_speed_and_volume_compose_in_order() {
        // Both effects on the same segment: setpts/atempo run first,
        // then volume runs on the time-stretched audio.
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.speed = Some(2.0);
        s0.volume = Some(0.5);
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        // atempo runs first (input [0:a:0] → [sa0]).
        assert!(
            plan.filter_complex.contains("[0:a:0]atempo=2[sa0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // volume runs next on [sa0] → [av0].
        assert!(
            plan.filter_complex.contains("[sa0]volume=0.5[av0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_emits_color_correction_before_speed() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.color_correction = Some(ColorCorrectionPlan {
            exposure_ev: Some(0.5),
            contrast: Some(1.2),
            saturation: Some(0.9),
            temperature: Some(0.4),
            tint: None,
            shadows: None,
            highlights: Some(-0.3),
        });
        s0.speed = Some(2.0);
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex
                .contains("[0:v:0]eq=brightness=0.061:contrast=1.2:saturation=0.9,curves="),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains(",colorbalance="),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[cv0]setpts=0.5*PTS[sv0]"),
            "color correction should feed speed, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_emits_lut3d_before_speed_and_concat() {
        let mut s0 = seg("/tmp/a.mp4", 0.0, 4.0);
        s0.lut_path = Some(PathBuf::from("/tmp/luts/show-look.cube"));
        s0.speed = Some(2.0);
        let plan = FilterPlanner::new(&[s0], &[]).plan();
        assert!(
            plan.filter_complex
                .contains("[0:v:0]lut3d=file='/tmp/luts/show-look.cube'[lv0]"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[lv0]setpts=0.5*PTS[sv0]"),
            "LUT should feed speed, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("[sv0][sa0]concat"),
            "post-LUT/post-speed labels should feed concat, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_appends_drawtext_for_title_overlay() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let title = TitlePlan {
            text: "Hello".into(),
            start_s: 0.0,
            end_s: 3.0,
            position: TitlePosition::Top,
            font_size: 64,
            color: "#FFFFFF".into(),
            font_weight: TitleWeight::Normal,
            animation: TitleAnimation::None,
            role: "title".into(),
            safe_area: None,
        };
        let plan = FilterPlanner::with_titles(&[s0], &[], &[title]).plan();
        assert!(
            plan.filter_complex.contains("drawtext=text='Hello'"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("fontsize=64"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("fontcolor=#FFFFFF"),
            "filter graph: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("y=h*0.05"),
            "filter graph (top position): {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("enable='between(t\\,0\\,3)'"),
            "filter graph: {}",
            plan.filter_complex,
        );
        // The video output label is now [titled_v]; audio still
        // [outa].
        assert_eq!(plan.video_out_label, "[titled_v]");
        assert_eq!(plan.audio_out_label, "[outa]");
    }

    #[test]
    fn filter_planner_chains_multiple_titles_with_commas() {
        let s0 = seg("/tmp/a.mp4", 0.0, 10.0);
        let titles = vec![
            TitlePlan {
                text: "One".into(),
                start_s: 0.0,
                end_s: 3.0,
                position: TitlePosition::Top,
                font_size: 64,
                color: "#FFFFFF".into(),
                font_weight: TitleWeight::Normal,
                animation: TitleAnimation::None,
                role: "title".into(),
                safe_area: None,
            },
            TitlePlan {
                text: "Two".into(),
                start_s: 5.0,
                end_s: 8.0,
                position: TitlePosition::Bottom,
                font_size: 48,
                color: "#FFAA00".into(),
                font_weight: TitleWeight::Bold,
                animation: TitleAnimation::None,
                role: "caption".into(),
                safe_area: Some("mobile".into()),
            },
        ];
        let plan = FilterPlanner::with_titles(&[s0], &[], &titles).plan();
        // Both titles land in the chain.
        assert!(plan.filter_complex.contains("text='One'"));
        assert!(plan.filter_complex.contains("text='Two'"));
        // Bold position uses borderw fallback (no bold-fontfile bundle).
        assert!(plan.filter_complex.contains("borderw=2"));
        // Bottom position uses h*0.85.
        assert!(plan.filter_complex.contains("y=h*0.85"));
    }

    #[test]
    fn filter_planner_appends_broadcast_overlay_filters() {
        let s0 = seg("/tmp/a.mp4", 0.0, 120.0);
        let mut config = BroadcastOverlayConfig {
            episode_title: "Ben Adams".into(),
            episode_subtitle: "Drone Hardware Entrepreneur".into(),
            show_name: "TECHNOLOGIA TALKS".into(),
            sponsors: vec!["LEARN-X".into(), "Throwly".into()],
            ..BroadcastOverlayConfig::default()
        };
        config.host_a.name = "Tadiwa Mbuwayesango".into();
        config.host_a.title = "Co-Host".into();
        config.host_a.photo_path = Some("branding/tadiwa.jpg".into());
        config.host_b.name = "Elvis Kimara".into();
        config.host_b.title = "Co-Host".into();
        config.host_b.photo_path = Some("branding/elvis.jpg".into());
        config
            .topics
            .push(awidat_proto::awidat_meta::BroadcastTimedEntry {
                time_seconds: 45.0,
                text: "Custom drones".into(),
            });
        config
            .chapters
            .push(awidat_proto::awidat_meta::BroadcastTimedEntry {
                time_seconds: 60.0,
                text: "Hardware barriers".into(),
            });
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("branding")).unwrap();
        fs::write(dir.path().join("branding/tadiwa.jpg"), b"stub").unwrap();
        fs::write(dir.path().join("branding/elvis.jpg"), b"stub").unwrap();
        let overlay = BroadcastOverlayPlan {
            config,
            project_root: dir.path().to_path_buf(),
        };
        let plan =
            FilterPlanner::with_titles_and_broadcast_overlay(&[s0], &[], &[], Some(&overlay))
                .plan();
        assert_eq!(plan.video_out_label, "[broadcast_v]");
        assert!(
            plan.filter_complex.contains("drawbox=x=0:y=ih-100"),
            "expected ticker bar, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("BEN ADAMS"),
            "expected title card text, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("NOW DISCUSSING"),
            "expected topic badge, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("HARDWARE BARRIERS"),
            "expected chapter card, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("movie='"),
            "expected project-relative host photos to become movie filters, got: {}",
            plan.filter_complex,
        );
        assert!(
            plan.filter_complex.contains("overlay=x=30:y=main_h-145"),
            "expected left host photo overlay, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn filter_planner_short_form_broadcast_overlay_suppresses_long_form_layers() {
        let s0 = seg("/tmp/a.mp4", 0.0, 30.0);
        let config = BroadcastOverlayConfig {
            short_form_mode: true,
            episode_title: "Long Episode Title".into(),
            show_name: "TECHNOLOGIA TALKS".into(),
            sponsors: vec!["LEARN-X".into()],
            ..BroadcastOverlayConfig::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let overlay = BroadcastOverlayPlan {
            config,
            project_root: dir.path().to_path_buf(),
        };
        let plan =
            FilterPlanner::with_titles_and_broadcast_overlay(&[s0], &[], &[], Some(&overlay))
                .plan();
        assert_eq!(plan.video_out_label, "[broadcast_v]");
        assert!(
            plan.filter_complex.contains("TECHNOLOGIA TALKS"),
            "expected short-form show label, got: {}",
            plan.filter_complex,
        );
        assert!(
            !plan.filter_complex.contains("NOW DISCUSSING"),
            "short-form mode should suppress long-form ticker/topic layers: {}",
            plan.filter_complex,
        );
        assert!(
            !plan.filter_complex.contains("EPISODE"),
            "short-form mode should suppress title card layers: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn drawtext_escape_handles_special_chars() {
        let s = drawtext_escape("text: with 'quote', backslash\\");
        assert!(s.contains("\\:"));
        assert!(s.contains("\\'"));
        assert!(s.contains("\\\\"));
        assert!(s.contains("\\,"));
    }

    fn title(animation: TitleAnimation, position: TitlePosition) -> TitlePlan {
        TitlePlan {
            text: "Hi".into(),
            start_s: 1.0,
            end_s: 4.0,
            position,
            font_size: 48,
            color: "#FFFFFF".into(),
            font_weight: TitleWeight::Normal,
            animation,
            role: "title".into(),
            safe_area: None,
        }
    }

    #[test]
    fn fade_in_emits_alpha_ramp_at_start() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let plan = FilterPlanner::with_titles(
            &[s0],
            &[],
            &[title(TitleAnimation::FadeIn, TitlePosition::Center)],
        )
        .plan();
        assert!(
            plan.filter_complex.contains("alpha='if(lt(t\\,1.5)"),
            "expected fade-in ramp ending at start+0.5=1.5, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn fade_out_emits_alpha_ramp_at_end() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let plan = FilterPlanner::with_titles(
            &[s0],
            &[],
            &[title(TitleAnimation::FadeOut, TitlePosition::Center)],
        )
        .plan();
        // Fade-out plateau ends at end-0.5 = 3.5.
        assert!(
            plan.filter_complex.contains("alpha='if(lt(t\\,3.5)"),
            "expected fade-out plateau ending at 3.5, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn fade_in_out_emits_two_piece_alpha_expression() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let plan = FilterPlanner::with_titles(
            &[s0],
            &[],
            &[title(TitleAnimation::FadeInOut, TitlePosition::Center)],
        )
        .plan();
        // Both ramps appear (1.5 = fade-in end, 3.5 = fade-out start).
        assert!(plan.filter_complex.contains("if(lt(t\\,1.5)"));
        assert!(plan.filter_complex.contains("if(lt(t\\,3.5)"));
    }

    #[test]
    fn slide_in_for_top_position_animates_y_from_above() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let plan = FilterPlanner::with_titles(
            &[s0],
            &[],
            &[title(TitleAnimation::SlideIn, TitlePosition::Top)],
        )
        .plan();
        // Slide-in on top starts off-screen at y=-text_h.
        assert!(
            plan.filter_complex.contains("-text_h"),
            "expected slide-in y to start at -text_h, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn slide_in_for_center_position_animates_x_from_right() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let plan = FilterPlanner::with_titles(
            &[s0],
            &[],
            &[title(TitleAnimation::SlideIn, TitlePosition::Center)],
        )
        .plan();
        // Slide-in on center starts off-screen at x=w (right edge).
        // Confirm an x= expression that mentions `w` (off-screen) and
        // the ramp end time (start_s + ramp = 1.5).
        assert!(
            plan.filter_complex.contains("x=if(lt(t\\,1.5)"),
            "expected slide-in x ramp on center title, got: {}",
            plan.filter_complex,
        );
    }

    #[test]
    fn slide_out_for_bottom_position_animates_y_to_below() {
        let s0 = seg("/tmp/a.mp4", 0.0, 5.0);
        let plan = FilterPlanner::with_titles(
            &[s0],
            &[],
            &[title(TitleAnimation::SlideOut, TitlePosition::Bottom)],
        )
        .plan();
        // Slide-out on bottom starts the ramp at end-0.5 = 3.5.
        assert!(
            plan.filter_complex.contains("if(lt(t\\,3.5)"),
            "expected slide-out ramp starting at 3.5, got: {}",
            plan.filter_complex,
        );
        // Off-screen target for bottom is y=h.
        assert!(
            plan.filter_complex.contains("h-(h*0.85)")
                || plan
                    .filter_complex
                    .contains("(h-({y_rest}))".replace("{y_rest}", "h*0.85").as_str())
                || plan.filter_complex.matches("h*0.85").count() >= 2
        );
    }

    #[test]
    fn build_timeline_argv_unchanged_after_extraction() {
        // Behaviour-preservation guard for 14.4. The argv produced
        // for a multi-segment fixture must be exactly what the old
        // monolithic builder produced. If 14.5 changes the
        // no-transitions graph, this test is the canary.
        let segs = vec![seg("/tmp/a.mp4", 0.0, 2.0), seg("/tmp/b.mp4", 1.0, 3.0)];
        let argv = build_timeline_argv(&segs, Path::new("/tmp/out.mp4"));
        let cmd = argv.join(" ");
        // Two -ss / -t / -i triples preceded by `-y -loglevel info`.
        assert!(
            cmd.starts_with("-y -loglevel info -ss 0 -t 2 -i /tmp/a.mp4 -ss 1 -t 3 -i /tmp/b.mp4")
        );
        assert!(cmd.contains(
            "-filter_complex [0:v:0][0:a:0][1:v:0][1:a:0]concat=n=2:v=1:a=1[outv][outa] \
             -map [outv] -map [outa]",
        ));
        assert!(cmd.ends_with("/tmp/out.mp4"));
    }
}
