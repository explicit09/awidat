//! Apply a parsed [`EdlEnvelope`] to an OTIO [`Timeline`] (clone).
//!
//! Per `PLAN.md` §6.2:
//! 1. Lark parse → already done by `parser.rs`.
//! 2. Anchor resolution → `anchor::resolve`.
//! 3. Schema validation → frame ranges in bounds, asset paths, etc.
//! 4. **OTIO round-trip** — apply to a clone, validate against the OTIO
//!    schema. Reject if invalid.
//! 5. Hooks — deferred (skills phase).
//! 6. Commit to disk + emit `TimelineDiff` event — done at the tool
//!    handler level, not here.
//!
//! Per-track cursor: the survey (and codex's `seek_sequence.rs:12,16`)
//! flags this as load-bearing — when a single envelope makes multiple
//! changes to the same track, later anchors must be resolved against the
//! *post-prior-op* timeline state, not the original. We re-resolve every
//! op against the working clone, which has the same effect as the
//! cursor pattern but reuses one resolver path.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};

use awidat_effects::StackPolicy;
use awidat_proto::awidat_meta::{
    AwidatClipMetadata, AwidatTimelineMetadata, BroadcastOverlayConfig, BroadcastOverlayStyle,
    BroadcastTimedEntry,
};
use awidat_proto::otio::{Clip, StackChild, Timeline, TrackChild, TrackKind};
use thiserror::Error;

use super::anchor::{AnchorContext, ClipLocator, resolve};
use super::op::{Anchor, AudioFxConfig, EdlEnvelope, EdlOp, InsertTrackKind};

/// One record of what was applied. Surfaced back to the model + the TUI.
#[derive(Debug, Clone)]
pub struct AppliedOp {
    /// Op index in the envelope.
    pub index: usize,
    /// One-line human description of what landed.
    pub description: String,
    /// The clip locator that resolved (when applicable).
    pub locator: Option<ClipLocator>,
}

/// Outcome of applying one envelope.
#[derive(Debug, Clone)]
pub struct ApplyOutcome {
    /// What landed, in source order.
    pub applied: Vec<AppliedOp>,
}

/// Apply errors. All are `RespondToModel`-shaped at the handler layer.
#[derive(Debug, Error)]
pub enum ApplyError {
    /// Anchor resolution failed for op at `index`.
    #[error("op #{index}: {miss}")]
    AnchorMiss {
        /// Op index in the envelope.
        index: usize,
        /// Underlying miss with candidates.
        miss: super::anchor::AnchorMiss,
    },
    /// Op asked for a field outside the legal range.
    #[error("op #{index}: invalid field: {message}")]
    Invalid {
        /// Op index.
        index: usize,
        /// Diagnostic.
        message: String,
    },
    /// OTIO round-trip validation failed after applying. The model gets
    /// the OTIO error verbatim — usually a duration/range issue.
    #[error("op #{index}: OTIO validation failed after apply: {message}")]
    OtioInvalid {
        /// Op index that triggered the rejection.
        index: usize,
        /// Diagnostic from `Project::read_otio_timeline` / validate.
        message: String,
    },
    /// An op type isn't yet implemented at the apply layer (parser
    /// accepted it; apply hasn't caught up). Surfaced as a clear
    /// "deferred" error rather than silent skip.
    #[error("op #{index}: '{op}' not yet implemented (deferred to a later batch)")]
    NotImplemented {
        /// Op index.
        index: usize,
        /// Human-readable op kind.
        op: String,
    },
}

/// Apply `envelope` to a clone of `original`. Returns `(new_timeline,
/// outcome)` on success; `(original_unchanged, error)` is implicit — the
/// original timeline is never mutated by this function.
///
/// `ctx` carries side data the resolver needs (project root for
/// loading whisper sidecars). Pass [`AnchorContext::empty()`] when
/// you don't have a project root — the resolver falls back to clip
/// metadata only.
pub fn apply(
    original: &Timeline,
    envelope: &EdlEnvelope,
    ctx: &AnchorContext,
) -> Result<(Timeline, ApplyOutcome), ApplyError> {
    let mut working = original.clone();
    let mut applied = Vec::with_capacity(envelope.ops.len());

    for (index, op) in envelope.ops.iter().enumerate() {
        let locator = resolve_locator_for_op(&working, index, op, ctx)?;
        let description = apply_one(&mut working, index, op, ctx, locator)?;
        applied.push(AppliedOp {
            index,
            description,
            locator,
        });
    }

    // OTIO round-trip: validate the working timeline's invariants. Per
    // PLAN §6.2 step 4, this is the "linter on edit" — we reject
    // structurally-bad edits before committing.
    if let Err(e) = working.validate_for_test() {
        return Err(ApplyError::OtioInvalid {
            index: applied.last().map(|a| a.index).unwrap_or(0),
            message: e,
        });
    }

    Ok((working, ApplyOutcome { applied }))
}

/// Apply one op in place. Returns a one-line description of what landed.
fn apply_one(
    working: &mut Timeline,
    index: usize,
    op: &EdlOp,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    match op {
        EdlOp::TrimClip { anchor, start, end } => {
            apply_trim(working, index, anchor, *start, *end, ctx, locator)
        }
        EdlOp::DeleteClip { anchor } => apply_delete(working, index, anchor, ctx, locator),
        EdlOp::SplitClip { anchor, at_s } => {
            apply_split(working, index, anchor, *at_s, ctx, locator)
        }
        EdlOp::UntrimClip { anchor, start, end } => {
            apply_untrim(working, index, anchor, *start, *end, ctx, locator)
        }
        EdlOp::InsertClip {
            asset,
            track,
            track_kind,
            at_position,
            start,
            end,
            name,
            link_group_id,
        } => apply_insert_clip(
            working,
            index,
            asset,
            track,
            *track_kind,
            *at_position,
            *start,
            *end,
            name.as_deref(),
            link_group_id.as_deref(),
        ),
        EdlOp::InsertBRoll {
            anchor,
            asset,
            duration_s,
            position,
        } => apply_insert_broll(
            working,
            index,
            anchor,
            asset,
            *duration_s,
            *position,
            ctx,
            locator,
        ),
        EdlOp::InsertPiP {
            anchor,
            asset,
            duration_s,
            source_start_s,
            corner,
            scale,
            margin_pct,
        } => apply_insert_pip(
            working,
            index,
            anchor,
            asset,
            *duration_s,
            *source_start_s,
            *corner,
            *scale,
            *margin_pct,
            ctx,
            locator,
        ),
        EdlOp::MoveClip {
            anchor,
            to_position,
            at_s,
        } => apply_move_clip(working, index, anchor, *to_position, *at_s, ctx, locator),
        EdlOp::InsertTransition {
            between,
            kind,
            duration_s,
            spec,
        } => apply_insert_transition(
            working,
            index,
            between,
            kind,
            *duration_s,
            spec.as_ref(),
            ctx,
        ),
        EdlOp::SetVolume { anchor, value } => {
            apply_set_volume(working, index, anchor, *value, ctx, locator)
        }
        EdlOp::SetAudioFade {
            anchor,
            fade_in_s,
            fade_out_s,
        } => apply_set_audio_fade(
            working,
            index,
            anchor,
            *fade_in_s,
            *fade_out_s,
            ctx,
            locator,
        ),
        EdlOp::SetTrackAudio {
            track,
            role,
            volume,
            muted,
            solo,
        } => apply_set_track_audio(
            working,
            index,
            track,
            role.as_deref(),
            *volume,
            *muted,
            *solo,
        ),
        EdlOp::SetDucking {
            track,
            enabled,
            amount_db,
            attack_ms,
            release_ms,
        } => apply_set_ducking(
            working,
            index,
            track,
            *enabled,
            *amount_db,
            *attack_ms,
            *release_ms,
        ),
        EdlOp::SetSyncGroup {
            anchor,
            sync_group_id,
            offset_s,
            speed_factor,
            confidence,
        } => apply_set_sync_group(
            working,
            index,
            anchor,
            sync_group_id,
            *offset_s,
            *speed_factor,
            *confidence,
            ctx,
            locator,
        ),
        EdlOp::SetClipAudioFx { anchor, fx } => {
            apply_set_clip_audio_fx(working, index, anchor, fx, ctx, locator)
        }
        EdlOp::SetTrackAudioFx { track, fx } => apply_set_track_audio_fx(working, index, track, fx),
        EdlOp::SetEffect {
            anchor,
            effect,
            params,
            rationale,
        } => apply_set_effect(
            working,
            index,
            anchor,
            effect,
            params,
            rationale.as_deref(),
            ctx,
            locator,
        ),
        EdlOp::SetSpeed { anchor, factor } => {
            apply_set_speed(working, index, anchor, *factor, ctx, locator)
        }
        EdlOp::SetColorCorrection {
            anchor,
            exposure_ev,
            contrast,
            saturation,
            temperature,
            tint,
            shadows,
            highlights,
        } => apply_set_color_correction(
            working,
            index,
            anchor,
            *exposure_ev,
            *contrast,
            *saturation,
            *temperature,
            *tint,
            *shadows,
            *highlights,
            ctx,
            locator,
        ),
        EdlOp::ApplyLut { anchor, lut_path } => {
            apply_lut(working, index, anchor, lut_path, ctx, locator)
        }
        EdlOp::InsertTitle {
            start_s,
            end_s,
            text,
            position,
            font_size,
            color,
            font_weight,
            animation,
        } => apply_insert_title(
            working,
            index,
            *start_s,
            *end_s,
            text,
            *position,
            *font_size,
            color,
            *font_weight,
            *animation,
        ),
        EdlOp::SetTitle {
            anchor,
            start_s,
            end_s,
            text,
            position,
            font_size,
            color,
            font_weight,
            animation,
        } => apply_set_title(
            working,
            index,
            anchor,
            *start_s,
            *end_s,
            text.as_deref(),
            *position,
            *font_size,
            color.as_deref(),
            *font_weight,
            *animation,
            ctx,
            locator,
        ),
        EdlOp::InsertCaption {
            start_s,
            end_s,
            text,
            position,
            font_size,
            color,
            safe_area,
        } => apply_insert_caption(
            working, index, *start_s, *end_s, text, *position, *font_size, color, safe_area,
        ),
        EdlOp::SetOutputFormat {
            aspect_ratio,
            platform,
            safe_area,
        } => apply_set_output_format(
            working,
            index,
            aspect_ratio,
            platform.as_deref(),
            safe_area.as_deref(),
        ),
        EdlOp::SetLoudnessTarget {
            integrated_lufs,
            true_peak_db,
        } => apply_set_loudness_target(working, index, *integrated_lufs, *true_peak_db),
        EdlOp::SetPackageMetadata {
            platform,
            title,
            description,
            tags,
        } => apply_set_package_metadata(
            working,
            index,
            platform.as_deref(),
            title.as_deref(),
            description.as_deref(),
            tags.as_deref(),
        ),
        EdlOp::SetBroadcastOverlay { config } => {
            apply_set_broadcast_overlay(working, index, config)
        }
    }
}

fn resolve_locator_for_op(
    working: &Timeline,
    index: usize,
    op: &EdlOp,
    ctx: &AnchorContext,
) -> Result<Option<ClipLocator>, ApplyError> {
    let anchor = match op {
        EdlOp::TrimClip { anchor, .. }
        | EdlOp::DeleteClip { anchor }
        | EdlOp::SplitClip { anchor, .. }
        | EdlOp::UntrimClip { anchor, .. }
        | EdlOp::MoveClip { anchor, .. }
        | EdlOp::InsertBRoll { anchor, .. }
        | EdlOp::InsertPiP { anchor, .. }
        | EdlOp::SetVolume { anchor, .. }
        | EdlOp::SetAudioFade { anchor, .. }
        | EdlOp::SetSyncGroup { anchor, .. }
        | EdlOp::SetClipAudioFx { anchor, .. }
        | EdlOp::SetEffect { anchor, .. }
        | EdlOp::SetSpeed { anchor, .. }
        | EdlOp::SetColorCorrection { anchor, .. }
        | EdlOp::ApplyLut { anchor, .. }
        | EdlOp::SetTitle { anchor, .. } => anchor,
        EdlOp::InsertClip { .. }
        | EdlOp::InsertTransition { .. }
        | EdlOp::SetTrackAudio { .. }
        | EdlOp::SetDucking { .. }
        | EdlOp::SetTrackAudioFx { .. }
        | EdlOp::InsertTitle { .. }
        | EdlOp::InsertCaption { .. }
        | EdlOp::SetOutputFormat { .. }
        | EdlOp::SetLoudnessTarget { .. }
        | EdlOp::SetPackageMetadata { .. }
        | EdlOp::SetBroadcastOverlay { .. } => return Ok(None),
    };
    resolve(working, anchor, ctx)
        .map(Some)
        .map_err(|miss| ApplyError::AnchorMiss { index, miss })
}

fn required_locator(index: usize, locator: Option<ClipLocator>) -> Result<ClipLocator, ApplyError> {
    locator.ok_or_else(|| ApplyError::Invalid {
        index,
        message: "internal error: anchored op applied without a resolved locator".into(),
    })
}

fn apply_trim(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    new_start: Option<f64>,
    new_end: Option<f64>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-clip track child".into(),
        });
    };
    let Some(range) = clip.source_range.as_ref() else {
        return Err(ApplyError::Invalid {
            index,
            message: "clip has no source_range; cannot trim a clip with implicit range".into(),
        });
    };
    let rate = range.start_time.rate;
    let original_start_s = range.start_time.to_seconds();
    let original_end_s = range.start_time.to_seconds() + range.duration.to_seconds();
    let target_start = new_start.unwrap_or(original_start_s);
    let target_end = new_end.unwrap_or(original_end_s);
    if target_end < target_start {
        return Err(ApplyError::Invalid {
            index,
            message: format!("trim: end {target_end} must be >= start {target_start}"),
        });
    }
    // If the agent asks for a trim that *expands* past the current
    // source range, reject with a clear hint. This happened in real
    // video runs: agent passed timeline-position seconds that
    // exceeded the post-trim source range, expecting an extension
    // of the clip; got a silently-different OTIO that round-tripped
    // wrong. Better to fail loud and tell them how to recover.
    if target_end > original_end_s + 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "trim: end {target_end:.3}s is past the current source range \
                 end {original_end_s:.3}s. Trim can only narrow a clip's source \
                 range, not extend it. If you want a longer clip, you may need \
                 to inspect_clip first to see the current source bounds, then \
                 trim within them."
            ),
        });
    }
    if target_start < original_start_s - 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "trim: start {target_start:.3}s is before the current source \
                 range start {original_start_s:.3}s. Trim can only narrow a \
                 clip's source range, not extend it backward. inspect_clip \
                 to see the current bounds before re-trimming."
            ),
        });
    }
    if (target_start - original_start_s).abs() < 1e-6 && (target_end - original_end_s).abs() < 1e-6
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "trim: requested range [{target_start:.3}s..{target_end:.3}s] is a no-op; \
                 the clip is already at that source range. view_timeline shows each clip's \
                 current source=[start..end]. To trim the first N seconds from the current \
                 visible clip, set start to current source start + N. To trim the last N \
                 seconds, set end to current source end - N."
            ),
        });
    }
    let new_dur = target_end - target_start;
    if new_dur < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("trim: computed negative duration {new_dur}"),
        });
    }
    clip.source_range = Some(awidat_proto::otio::TimeRange::new(
        awidat_proto::otio::RationalTime::new(target_start * rate, rate),
        awidat_proto::otio::RationalTime::new(new_dur * rate, rate),
    ));
    let name = clip.name.clone();
    Ok(format!(
        "trimmed clip {name:?} to [{target_start:.3}s..{target_end:.3}s] ({new_dur:.3}s)"
    ))
}

/// Reset / extend a previously-trimmed clip's source range outward.
/// Inverse-direction of `apply_trim`: Trim narrows; Untrim widens.
///
/// `new_start` and `new_end` default to the clip's *original* media
/// bounds when the media reference declares an `available_range`.
/// Without that bound we trust the agent's values (OTIO round-trip
/// validation catches structurally-bad widening) — defaulting to
/// `0.0` / `current_end` is the conservative choice in that case.
///
/// Errors when:
/// - The clip has no `source_range` (no current state to widen from).
/// - The new range would be narrower than the current one (use
///   `Trim Clip` for that — keeps the two ops semantically distinct
///   so the model can't accidentally narrow when it meant to widen).
/// - The new range is invalid (`end < start`).
fn apply_untrim(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    new_start: Option<f64>,
    new_end: Option<f64>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    use awidat_proto::otio::{ExternalReference, MediaReference};
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-clip track child".into(),
        });
    };
    let Some(range) = clip.source_range.as_ref() else {
        return Err(ApplyError::Invalid {
            index,
            message: "clip has no source_range; nothing to untrim. Set source_range via a \
                      previous Trim Clip first."
                .into(),
        });
    };
    let rate = range.start_time.rate;
    let cur_start_s = range.start_time.to_seconds();
    let cur_end_s = cur_start_s + range.duration.to_seconds();

    // Discover the media's outer bounds from `available_range` if
    // declared on the external reference. Falls back to trusting the
    // agent — the OTIO validator will reject anything truly broken.
    let available = match &clip.media_reference {
        MediaReference::External(ExternalReference {
            available_range, ..
        }) => available_range.as_ref(),
        _ => None,
    };
    let (avail_start_s, avail_end_s) = match available {
        Some(r) => (
            r.start_time.to_seconds(),
            r.start_time.to_seconds() + r.duration.to_seconds(),
        ),
        None => (0.0, f64::INFINITY),
    };

    // Defaults: omitted fields keep the *current* source_range value,
    // matching apply_trim's behavior. A previous version of this op
    // defaulted to the available_range bounds — which meant an Untrim
    // that only widens `end` would also reset `start` to 0, surprising
    // the agent (real-video run trace: "the untrim wiped the start
    // trim too"). Preserve-on-omit is the right contract.
    let target_start = new_start.unwrap_or(cur_start_s);
    let target_end = new_end.unwrap_or(cur_end_s);

    if target_end < target_start {
        return Err(ApplyError::Invalid {
            index,
            message: format!("untrim: end {target_end} must be >= start {target_start}"),
        });
    }

    // Refuse to NARROW via Untrim — if the agent wants to narrow,
    // they should use Trim Clip. Keeps the two ops distinct.
    if target_start > cur_start_s + 1e-6 || target_end < cur_end_s - 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "untrim: requested range [{target_start:.3}s..{target_end:.3}s] is \
                 narrower than current [{cur_start_s:.3}s..{cur_end_s:.3}s]. Untrim only \
                 widens; use Trim Clip to narrow."
            ),
        });
    }

    // Cap to available range when we know it.
    let final_start = target_start.max(avail_start_s);
    let final_end = if avail_end_s.is_finite() {
        target_end.min(avail_end_s)
    } else {
        target_end
    };

    let new_dur = final_end - final_start;
    if new_dur <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "untrim: computed non-positive duration {new_dur} after capping to \
                 available range"
            ),
        });
    }

    clip.source_range = Some(awidat_proto::otio::TimeRange::new(
        awidat_proto::otio::RationalTime::new(final_start * rate, rate),
        awidat_proto::otio::RationalTime::new(new_dur * rate, rate),
    ));
    let name = clip.name.clone();
    Ok(format!(
        "untrimmed clip {name:?} to [{final_start:.3}s..{final_end:.3}s] \
         ({new_dur:.3}s) — was [{cur_start_s:.3}s..{cur_end_s:.3}s]"
    ))
}

/// Split one clip into two at `at_s` seconds into the source media.
///
/// Insert a new clip on a (possibly fresh) track from an asset on
/// disk. This is the load-bearing op for *building* a timeline —
/// every other op mutates an existing one.
///
/// Behavior:
/// - If `track` doesn't exist, create it (Video kind by default) and
///   append it to the timeline's tracks.
/// - Auto-name the clip `clip-N` where N is the new child's index in
///   the track, unless `name` is supplied.
/// - Build an ExternalReference to `asset`.
/// - Default `start = 0.0`, `end = 1.0` if neither supplied (the
///   agent is expected to pass at least one — the OTIO validator
///   will reject zero-duration clips, so the model gets a clear
///   error if it forgot).
/// - Insert at `at_position` (clamped to `[0, len]`); default
///   `at_position = len` (append).
#[allow(clippy::too_many_arguments)]
fn apply_insert_clip(
    working: &mut Timeline,
    index: usize,
    asset: &str,
    track_name: &str,
    track_kind_hint: Option<InsertTrackKind>,
    at_position: Option<usize>,
    start_s: Option<f64>,
    end_s: Option<f64>,
    name_override: Option<&str>,
    link_group_id: Option<&str>,
) -> Result<String, ApplyError> {
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, TimeRange, Track,
    };

    // Find or create the named track.
    let track_idx = working
        .tracks
        .children
        .iter()
        .enumerate()
        .find_map(|(i, sc)| match sc {
            StackChild::Track(t) if t.name == track_name => Some(i),
            _ => None,
        });
    let track_idx = match track_idx {
        Some(i) => i,
        None => {
            let kind = infer_insert_track_kind(track_name, track_kind_hint);
            let track = Track::empty(track_name.to_string(), kind);
            working.tracks.children.push(StackChild::Track(track));
            working.tracks.children.len() - 1
        }
    };

    // Determine the source range.
    let start = start_s.unwrap_or(0.0);
    let end = end_s.unwrap_or(start + 1.0);
    if end <= start {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "insert: end {end:.3}s must be > start {start:.3}s. \
                 Pass `+ start: <s>` and `+ end: <s>` (in source-media \
                 seconds). Use inspect_clip on the asset first to see \
                 its duration."
            ),
        });
    }
    let rate = 24.0_f64;
    let source_range = TimeRange::new(
        RationalTime::new(start * rate, rate),
        RationalTime::new((end - start) * rate, rate),
    );

    let linked_video_start_s = link_group_id.and_then(|id| {
        let StackChild::Track(track) = &working.tracks.children[track_idx] else {
            return None;
        };
        if at_position.is_none() && matches!(track.kind, TrackKind::Audio) {
            linked_clip_track_time(working, id, TrackKind::Video)
        } else {
            None
        }
    });

    let StackChild::Track(track) = &mut working.tracks.children[track_idx] else {
        return Err(ApplyError::Invalid {
            index,
            message: "track index resolved to a non-track stack child".into(),
        });
    };

    if let Some(target_time_s) = linked_video_start_s {
        let cursor_s = track_cursor(track);
        if cursor_s + 0.001 < target_time_s {
            track
                .children
                .push(TrackChild::Gap(awidat_proto::otio::Gap::of_duration(
                    target_time_s - cursor_s,
                    rate,
                )));
        }
    }

    // Build the clip.
    let position = at_position
        .unwrap_or(track.children.len())
        .min(track.children.len());
    let chosen_name = name_override
        .map(str::to_string)
        .unwrap_or_else(|| next_clip_name_in_track(track));
    let mut clip = Clip::empty(chosen_name.clone());
    clip.media_reference = MediaReference::External(ExternalReference::new(asset.to_string()));
    clip.source_range = Some(source_range);
    // Stamp a fresh anchor uuid so Step 8's drag-to-trim can build
    // `Anchor::ClipUuid { uuid }` against this clip without falling
    // back to name-based matching (which works but is brittle when
    // two inserts pick the same default name).
    stamp_fresh_clip_uuid(&mut clip);
    if let Some(link_group_id) = link_group_id {
        stamp_link_group_id(&mut clip, link_group_id);
    }

    track.children.insert(position, TrackChild::Clip(clip));

    Ok(format!(
        "inserted clip {chosen_name:?} on track {track_name:?} at \
         position {position}: asset={asset:?} source=[{start:.3}s..{end:.3}s] \
         ({:.3}s)",
        end - start
    ))
}

fn infer_insert_track_kind(track_name: &str, hint: Option<InsertTrackKind>) -> TrackKind {
    match hint {
        Some(InsertTrackKind::Video) => TrackKind::Video,
        Some(InsertTrackKind::Audio) => TrackKind::Audio,
        Some(InsertTrackKind::Auto) | None => {
            let name = track_name.trim().to_ascii_lowercase();
            if name == "a1"
                || name.starts_with("a ")
                || name.starts_with("a-")
                || name.starts_with("audio")
                || name.starts_with("music")
            {
                TrackKind::Audio
            } else {
                TrackKind::Video
            }
        }
    }
}

/// Stamp a fresh, process-unique clip uuid into `clip.metadata.awidat
/// .extra["clip_uuid"]`. Used by InsertClip (new clip created) and
/// Split (right piece needs its own uuid — clip.clone() inherited
/// the parent's, which breaks Anchor::ClipUuid resolution because
/// two clips would share the same uuid).
///
/// Uniqueness is "monotonic counter + nanosecond timestamp formatted
/// as a 16-char hex string" — short, stable across cargo build IDs,
/// and uniqueness-by-construction across rapid back-to-back calls.
/// Not cryptographically random; we don't need that here.
fn stamp_fresh_clip_uuid(clip: &mut Clip) {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64;
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let uuid = format!("c-{:013x}{:03x}", nanos & 0xFFFF_FFFF_FFFF_F, seq & 0xFFF);
    let awidat = clip
        .metadata
        .awidat
        .get_or_insert_with(AwidatClipMetadata::default);
    awidat
        .extra
        .insert("clip_uuid".into(), serde_json::Value::String(uuid));
}

fn stamp_link_group_id(clip: &mut Clip, link_group_id: &str) {
    let awidat = clip
        .metadata
        .awidat
        .get_or_insert_with(AwidatClipMetadata::default);
    awidat.extra.insert(
        "link_group_id".into(),
        serde_json::Value::String(link_group_id.to_string()),
    );
}

fn linked_clip_track_time(
    timeline: &Timeline,
    link_group_id: &str,
    kind: TrackKind,
) -> Option<f64> {
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        if track.kind != kind {
            continue;
        }
        let mut cursor_s = 0.0;
        for child in &track.children {
            if let TrackChild::Clip(clip) = child
                && clip_link_group_id(clip).as_deref() == Some(link_group_id)
            {
                return Some(cursor_s);
            }
            cursor_s += child_duration(child);
        }
    }
    None
}

fn clip_link_group_id(clip: &Clip) -> Option<String> {
    clip.metadata
        .awidat
        .as_ref()
        .and_then(|m| m.extra.get("link_group_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn clip_uuid(clip: &Clip) -> Option<&str> {
    clip.metadata
        .awidat
        .as_ref()
        .and_then(|m| m.extra.get("clip_uuid"))
        .and_then(|v| v.as_str())
}

fn next_clip_name_in_track(track: &awidat_proto::otio::Track) -> String {
    let used = track
        .children
        .iter()
        .filter_map(|tc| match tc {
            TrackChild::Clip(clip) => Some(clip.name.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    for i in 0usize.. {
        let candidate = format!("clip-{i}");
        if !used.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!("usize iterator is unbounded")
}

/// Both pieces share the original media reference; the left piece
/// keeps the original name and metadata, the right piece gets a
/// `<name>-b` suffix so the agent can anchor each independently
/// afterward (e.g. `Delete Clip` on the new b-piece).
///
/// Errors when:
/// - The clip has no `source_range` (we can't compute a partition).
/// - `at_s` is outside `[start, end)` of the source range.
fn apply_split(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    at_s: f64,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-clip track child".into(),
        });
    };
    let Some(range) = clip.source_range.clone() else {
        return Err(ApplyError::Invalid {
            index,
            message: "clip has no source_range; cannot split a clip with implicit range".into(),
        });
    };
    let rate = range.start_time.rate;
    let start_s = range.start_time.to_seconds();
    let end_s = start_s + range.duration.to_seconds();
    if at_s <= start_s || at_s >= end_s {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "split: at_s {at_s} must lie strictly inside source range [{start_s}, {end_s})"
            ),
        });
    }

    // Build the right piece (clone before we mutate the left in
    // place, so we still have access to the original media ref +
    // metadata).
    let original_name = clip.name.clone();
    let mut right = clip.clone();
    right.name = format!("{original_name}-b");
    right.source_range = Some(awidat_proto::otio::TimeRange::new(
        awidat_proto::otio::RationalTime::new(at_s * rate, rate),
        awidat_proto::otio::RationalTime::new((end_s - at_s) * rate, rate),
    ));
    // The clip.clone() above also cloned the parent's clip_uuid —
    // which would mean two clips share the same anchor uuid after
    // this op. Stamp a fresh one so Anchor::ClipUuid resolves
    // unambiguously to whichever piece the agent (or user) names.
    stamp_fresh_clip_uuid(&mut right);
    let right_uuid = clip_uuid(&right).map(str::to_string);

    // Trim the left piece in place.
    let TrackChild::Clip(left) = &mut track.children[locator.child_index] else {
        // Already type-checked above; this branch can't actually fire.
        return Err(ApplyError::Invalid {
            index,
            message: "split: left piece type-check vanished mid-op".into(),
        });
    };
    left.source_range = Some(awidat_proto::otio::TimeRange::new(
        awidat_proto::otio::RationalTime::new(start_s * rate, rate),
        awidat_proto::otio::RationalTime::new((at_s - start_s) * rate, rate),
    ));

    // Insert the right piece directly after the left.
    track
        .children
        .insert(locator.child_index + 1, TrackChild::Clip(right));

    let right_name = format!("{original_name}-b");
    let right_anchor = right_uuid
        .as_deref()
        .map(|uuid| format!(" anchor=clip_uuid={uuid}"))
        .unwrap_or_default();
    Ok(format!(
        "split clip {original_name:?} at {at_s:.3}s → {original_name:?} \
         [{start_s:.3}s..{at_s:.3}s] + {right_name:?}{right_anchor} \
         [{at_s:.3}s..{end_s:.3}s]"
    ))
}

fn apply_delete(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let overlay_shift = broadcast_overlay_shift_for_delete(working, &locator);
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-track stack child".into(),
        });
    };
    let removed = track.children.remove(locator.child_index);
    let name = match &removed {
        TrackChild::Clip(c) => c.name.clone(),
        _ => "<non-clip>".to_string(),
    };
    let removed_transitions = remove_transitions_around_deleted_child(track, locator.child_index);
    if let Some((cut_point, duration)) = overlay_shift {
        shift_broadcast_overlay_timestamps(working, cut_point, duration);
    }
    if removed_transitions == 0 {
        Ok(format!("deleted clip {name:?}"))
    } else {
        Ok(format!(
            "deleted clip {name:?} and removed {removed_transitions} adjacent transition(s)"
        ))
    }
}

fn remove_transitions_around_deleted_child(
    track: &mut awidat_proto::otio::Track,
    index: usize,
) -> usize {
    let mut removed = 0;
    if matches!(track.children.get(index), Some(TrackChild::Transition(_))) {
        track.children.remove(index);
        removed += 1;
    }
    if index > 0
        && matches!(
            track.children.get(index - 1),
            Some(TrackChild::Transition(_))
        )
    {
        track.children.remove(index - 1);
        removed += 1;
    }
    removed
}

/// Insert a `Transition` node between two adjacent clips on the same
/// track. The transition straddles the cut at `from`'s end /
/// `to`'s start; the render pipeline interprets this as an xfade-style
/// overlap (Step 14.5).
///
/// Validation:
/// - both anchors must resolve
/// - both must be on the *same* track
/// - they must be at *adjacent* indices (`to.child_index ==
///   from.child_index + 1`); we don't support transitions that cross
///   a gap or another transition in v1
/// - `duration_s > 0`
fn apply_insert_transition(
    working: &mut Timeline,
    index: usize,
    between: &super::op::TransitionBetween,
    kind: &str,
    duration_s: f64,
    spec: Option<&awidat_proto::transitions::SemanticTransitionSpec>,
    ctx: &AnchorContext,
) -> Result<String, ApplyError> {
    use awidat_proto::otio::Transition;

    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("transition duration {duration_s} must be > 0"),
        });
    }
    awidat_proto::transitions::resolve_ffmpeg_xfade(kind)
        .map_err(|e| ApplyError::Invalid {
            index,
            message: format!("transition {kind:?} is not supported by the phase-one renderer: {e}"),
        })?
        .ok_or_else(|| ApplyError::Invalid {
            index,
            message: format!("transition {kind:?} is semantic-only and cannot be inserted"),
        })?;
    if let Some(spec) = spec {
        awidat_proto::transitions::validate_semantic_transition_spec(spec).map_err(|e| {
            ApplyError::Invalid {
                index,
                message: format!("transition spec for {:?} is invalid: {e}", spec.id),
            }
        })?;
    }

    let from_loc = resolve(working, &between.from, ctx)
        .map_err(|miss| ApplyError::AnchorMiss { index, miss })?;
    let to_loc = resolve(working, &between.to, ctx)
        .map_err(|miss| ApplyError::AnchorMiss { index, miss })?;

    if from_loc.track_index != to_loc.track_index {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "transition: anchors resolve to different tracks \
                 ({} vs {}). Both clips must live on the same track.",
                from_loc.track_index, to_loc.track_index,
            ),
        });
    }
    if to_loc.child_index != from_loc.child_index + 1 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "transition: anchors are not adjacent (from at \
                 index {}, to at index {}). Transitions only insert \
                 between consecutive clips in v1.",
                from_loc.child_index, to_loc.child_index,
            ),
        });
    }

    // Pull the rate off the from-clip's source_range so the transition's
    // RationalTime offsets share a denominator with the surrounding
    // clips (avoids round-trip drift on save).
    let StackChild::Track(track) = &mut working.tracks.children[from_loc.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "transition: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(from_clip) = &track.children[from_loc.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "transition: from anchor resolved to a non-clip child".into(),
        });
    };
    let rate = from_clip
        .source_range
        .as_ref()
        .map(|r| r.start_time.rate)
        .unwrap_or(24.0);

    let mut transition = Transition::symmetric(kind, duration_s, rate);
    if let Some(spec) = spec {
        transition.metadata.insert(
            "awidat_transition".into(),
            serde_json::to_value(spec).map_err(|e| ApplyError::Invalid {
                index,
                message: format!("transition spec could not be serialized: {e}"),
            })?,
        );
    }
    track
        .children
        .insert(from_loc.child_index + 1, TrackChild::Transition(transition));

    Ok(format!(
        "inserted transition {kind:?} ({duration_s:.3}s) between \
         clips at indices {} and {} on track {}",
        from_loc.child_index,
        from_loc.child_index + 2, // shifted by the insert
        from_loc.track_index,
    ))
}

/// Move an anchored clip to a different position within its track.
/// Cross-track moves aren't supported in v1 — `to_position` indexes
/// into the *anchor's* track, clamped to the track's child count.
///
/// The vec-extract / vec-insert pattern means the move is conceptually
/// a swap when `to_position == child_index + 1` (inserting just past
/// where we extracted lands the clip back where it started); the
/// helper below normalizes that case to a no-op rather than confusing
/// the user with an op that "succeeded but did nothing."
fn apply_move_clip(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    to_position: usize,
    at_s: Option<f64>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "move: anchor resolved to a non-track stack child".into(),
        });
    };

    if locator.child_index >= track.children.len() {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "move: source index {} out of bounds for track of length {}",
                locator.child_index,
                track.children.len()
            ),
        });
    }

    if let Some(target_start_s) = at_s {
        return apply_move_clip_to_time(index, track, locator.child_index, target_start_s);
    }

    // The user-facing target is "the clip's index in the post-move
    // track", clamped to a legal slot (0..=len). After extraction the
    // track is one shorter, so when the user-target is past the
    // extraction point we shift the insert index down by 1 to
    // compensate for the gap that just collapsed.
    let len_before = track.children.len();
    let target_user = to_position.min(len_before.saturating_sub(1));
    if target_user == locator.child_index {
        // Same-position move — describe as a no-op rather than
        // mutating + producing an effective-no-change OTIO.
        let TrackChild::Clip(c) = &track.children[locator.child_index] else {
            return Ok("move: source is not a clip; left timeline unchanged".into());
        };
        return Ok(format!(
            "move: clip {:?} already at position {}; left timeline unchanged",
            c.name, target_user,
        ));
    }

    let removed = track.children.remove(locator.child_index);
    // Translate user-facing target into a post-extract insertion
    // index. When moving forward (target_user > source), the
    // extraction shifted everyone after the source left by 1, so the
    // user's target index now means "insert just after element
    // target_user - 1" → insert at target_user.
    let insert_at = if target_user >= locator.child_index {
        target_user.min(track.children.len())
    } else {
        target_user
    };
    track.children.insert(insert_at, removed);

    let TrackChild::Clip(c) = &track.children[insert_at] else {
        return Ok(format!(
            "move: moved a non-clip child from {} to {} on track {}",
            locator.child_index, insert_at, locator.track_index,
        ));
    };
    Ok(format!(
        "moved clip {:?} from position {} to position {} on track {}",
        c.name, locator.child_index, target_user, locator.track_index,
    ))
}

fn apply_move_clip_to_time(
    index: usize,
    track: &mut awidat_proto::otio::Track,
    child_index: usize,
    target_start_s: f64,
) -> Result<String, ApplyError> {
    if !target_start_s.is_finite() || target_start_s < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("move: at_s must be a finite non-negative time, got {target_start_s}"),
        });
    }

    let original_start_s: f64 = track.children[..child_index]
        .iter()
        .map(child_duration)
        .sum();
    let duration_s = child_duration(&track.children[child_index]);
    if duration_s <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: "move: source clip has zero duration".into(),
        });
    }
    if (target_start_s - original_start_s).abs() < 0.01 {
        let TrackChild::Clip(c) = &track.children[child_index] else {
            return Ok("move: source is not a clip; left timeline unchanged".into());
        };
        return Ok(format!(
            "move: clip {:?} already starts at {target_start_s:.3}s; left timeline unchanged",
            c.name
        ));
    }

    let rate = track.children[child_index].as_clip_rate().unwrap_or(24.0);
    let removed = std::mem::replace(
        &mut track.children[child_index],
        TrackChild::Gap(awidat_proto::otio::Gap::of_duration(duration_s, rate)),
    );
    let name = match &removed {
        TrackChild::Clip(c) => c.name.clone(),
        _ => "<non-clip>".to_string(),
    };
    insert_child_at_time(track, removed, target_start_s, duration_s, rate);
    merge_adjacent_gaps(&mut track.children, rate);

    Ok(format!(
        "moved clip {name:?} from {original_start_s:.3}s to {target_start_s:.3}s on track {:?}",
        track.name
    ))
}

trait TrackChildClipRate {
    fn as_clip_rate(&self) -> Option<f64>;
}

impl TrackChildClipRate for TrackChild {
    fn as_clip_rate(&self) -> Option<f64> {
        match self {
            TrackChild::Clip(c) => c
                .source_range
                .as_ref()
                .map(|r| r.start_time.rate)
                .filter(|r| *r > 0.0),
            _ => None,
        }
    }
}

fn insert_child_at_time(
    track: &mut awidat_proto::otio::Track,
    child: TrackChild,
    target_start_s: f64,
    child_duration_s: f64,
    rate: f64,
) {
    const EPS: f64 = 0.001;
    let mut cursor_s = 0.0;
    for i in 0..track.children.len() {
        let dur_s = child_duration(&track.children[i]);
        let end_s = cursor_s + dur_s;
        if target_start_s <= cursor_s + EPS {
            track.children.insert(i, child);
            return;
        }
        if target_start_s < end_s - EPS {
            if matches!(track.children[i], TrackChild::Gap(_)) {
                let before_s = (target_start_s - cursor_s).max(0.0);
                let after_s = (end_s - target_start_s - child_duration_s).max(0.0);
                let mut replacement = Vec::new();
                if before_s > EPS {
                    replacement.push(TrackChild::Gap(awidat_proto::otio::Gap::of_duration(
                        before_s, rate,
                    )));
                }
                replacement.push(child);
                if after_s > EPS {
                    replacement.push(TrackChild::Gap(awidat_proto::otio::Gap::of_duration(
                        after_s, rate,
                    )));
                }
                track.children.splice(i..=i, replacement);
                return;
            }
            let insert_at = if target_start_s < cursor_s + dur_s / 2.0 {
                i
            } else {
                i + 1
            };
            track.children.insert(insert_at, child);
            return;
        }
        cursor_s = end_s;
    }
    if target_start_s > cursor_s + EPS {
        track
            .children
            .push(TrackChild::Gap(awidat_proto::otio::Gap::of_duration(
                target_start_s - cursor_s,
                rate,
            )));
    }
    track.children.push(child);
}

fn merge_adjacent_gaps(children: &mut Vec<TrackChild>, rate: f64) {
    let mut merged = Vec::with_capacity(children.len());
    for child in std::mem::take(children) {
        if let TrackChild::Gap(gap) = child {
            let dur_s = gap.source_range.duration.to_seconds();
            if dur_s <= 0.001 {
                continue;
            }
            if let Some(TrackChild::Gap(prev)) = merged.last_mut() {
                let total_s = prev.source_range.duration.to_seconds() + dur_s;
                *prev = awidat_proto::otio::Gap::of_duration(total_s, rate);
            } else {
                merged.push(TrackChild::Gap(gap));
            }
        } else {
            merged.push(child);
        }
    }
    *children = merged;
}

/// Insert a b-roll clip near an anchor, either replacing footage in
/// place (`Replace`) or layering on a higher video track (`Overlay`).
///
/// In v1 the broll always lands at the *anchor clip's start* position
/// (no in-clip offset field). The op carries `duration_s`, the broll's
/// length on the timeline.
///
/// `Replace`: the anchor clip's leading `duration_s` window is swapped
/// for the broll. The anchor clip's residual tail (if any) stays on
/// the same track immediately after the broll. If `duration_s` is
/// >= the anchor's full duration, the broll fully consumes the anchor
/// — no tail piece is reinserted.
///
/// `Overlay`: a new video track (named `V<N>` for the next free
/// number) is created if no video track besides the anchor's exists,
/// and the broll lands on it. v1 places the broll at the anchor
/// clip's track-time start; nothing on the lower track is mutated.
fn apply_insert_broll(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    asset: &str,
    duration_s: f64,
    position: super::op::BRollPosition,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, TimeRange, Track, TrackKind,
    };
    let _ = (anchor, ctx);

    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("broll duration {duration_s} must be > 0"),
        });
    }

    let locator = required_locator(index, locator)?;

    // Snapshot the anchor clip's source range + name + rate before
    // mutating anything — we'll need them in both branches and
    // referencing back into `working` via locator after a structural
    // change is fragile.
    let (anchor_name, rate, anchor_source_start, anchor_source_dur) = {
        let StackChild::Track(track) = &working.tracks.children[locator.track_index] else {
            return Err(ApplyError::Invalid {
                index,
                message: "broll: anchor resolved to a non-track stack child".into(),
            });
        };
        let TrackChild::Clip(clip) = &track.children[locator.child_index] else {
            return Err(ApplyError::Invalid {
                index,
                message: "broll: anchor resolved to a non-clip track child".into(),
            });
        };
        let range = clip
            .source_range
            .as_ref()
            .ok_or_else(|| ApplyError::Invalid {
                index,
                message: "broll: anchor clip has no source_range".into(),
            })?;
        (
            clip.name.clone(),
            range.start_time.rate,
            range.start_time.to_seconds(),
            range.duration.to_seconds(),
        )
    };

    // Build the broll clip — same shape as apply_insert_clip's clip
    // construction.
    let mut broll = Clip::empty(format!("broll-from-{anchor_name}"));
    broll.media_reference = MediaReference::External(ExternalReference::new(asset.to_string()));
    broll.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, rate),
        RationalTime::new(duration_s * rate, rate),
    ));
    stamp_fresh_clip_uuid(&mut broll);

    match position {
        super::op::BRollPosition::Replace => {
            // Compute the residual tail BEFORE we touch the track —
            // we'll only insert it if the broll doesn't fully consume
            // the anchor clip.
            let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
                return Err(ApplyError::Invalid {
                    index,
                    message: "broll: anchor resolved to a non-track stack child".into(),
                });
            };

            // Pull the anchor clip out as the template for the
            // residual tail.
            let TrackChild::Clip(orig) = track.children.remove(locator.child_index) else {
                return Err(ApplyError::Invalid {
                    index,
                    message: "broll: anchor resolved to a non-clip child".into(),
                });
            };
            track
                .children
                .insert(locator.child_index, TrackChild::Clip(broll));

            if duration_s < anchor_source_dur {
                let residual_start = anchor_source_start + duration_s;
                let residual_dur = anchor_source_dur - duration_s;
                let mut tail = orig;
                tail.name = format!("{anchor_name}-tail");
                tail.source_range = Some(TimeRange::new(
                    RationalTime::new(residual_start * rate, rate),
                    RationalTime::new(residual_dur * rate, rate),
                ));
                stamp_fresh_clip_uuid(&mut tail);
                track
                    .children
                    .insert(locator.child_index + 1, TrackChild::Clip(tail));
            }

            Ok(format!(
                "inserted b-roll over {anchor_name:?} on track {}: \
                 asset={asset:?} duration={duration_s:.3}s (replace)",
                locator.track_index,
            ))
        }
        super::op::BRollPosition::Overlay => {
            // Find an overlay-target video track. v1: any video track
            // that isn't the anchor's track. If none, create a fresh
            // one named V<N+1> where N is the highest existing
            // V-prefixed track number (or fall back to "V2").
            let target_idx =
                working
                    .tracks
                    .children
                    .iter()
                    .enumerate()
                    .find_map(|(i, sc)| match sc {
                        StackChild::Track(t)
                            if matches!(t.kind, TrackKind::Video) && i != locator.track_index =>
                        {
                            Some(i)
                        }
                        _ => None,
                    });

            let target_idx = match target_idx {
                Some(i) => i,
                None => {
                    let next_name = next_video_track_name(working);
                    let track = Track::empty(next_name, TrackKind::Video);
                    working.tracks.children.push(StackChild::Track(track));
                    working.tracks.children.len() - 1
                }
            };

            // Anchor's track-time = sum of preceding child durations.
            // We need this so the agent can place the overlay
            // correctly even when the lower track has lots of
            // earlier clips. v1 implementation: append at the end of
            // the overlay track if its current duration is at-or-past
            // the anchor's track-time; otherwise pad with a Gap.
            let anchor_track_time =
                track_time_at(working, locator.track_index, locator.child_index);
            let StackChild::Track(target) = &mut working.tracks.children[target_idx] else {
                return Err(ApplyError::Invalid {
                    index,
                    message: "broll: overlay target resolved to a non-track stack child".into(),
                });
            };
            let target_cursor = track_cursor(target);
            if target_cursor < anchor_track_time {
                // Pad with a Gap so the broll lines up under the
                // anchor on the lower track.
                let gap_dur = anchor_track_time - target_cursor;
                target
                    .children
                    .push(TrackChild::Gap(awidat_proto::otio::Gap::of_duration(
                        gap_dur, rate,
                    )));
            }
            target.children.push(TrackChild::Clip(broll));
            Ok(format!(
                "inserted b-roll over {anchor_name:?} on overlay track {}: \
                 asset={asset:?} duration={duration_s:.3}s (overlay)",
                target.name,
            ))
        }
    }
}

/// Insert a picture-in-picture clip on an upper video track.
fn apply_insert_pip(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    asset: &str,
    duration_s: f64,
    source_start_s: f64,
    corner: super::op::PiPCorner,
    scale: f64,
    margin_pct: f64,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, TimeRange, Track, TrackChild,
        TrackKind,
    };
    let _ = (anchor, ctx);

    validate_project_relative_asset(index, "insert_pip", asset)?;
    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("insert_pip: duration_s {duration_s} must be > 0"),
        });
    }
    if !source_start_s.is_finite() || source_start_s < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("insert_pip: source_start_s {source_start_s} must be >= 0"),
        });
    }
    if !scale.is_finite() || !(0.10..=0.60).contains(&scale) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("insert_pip: scale {scale} must be in range 0.10..=0.60"),
        });
    }
    if !margin_pct.is_finite() || !(0.0..=0.15).contains(&margin_pct) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("insert_pip: margin_pct {margin_pct} must be in range 0.0..=0.15"),
        });
    }

    let locator = required_locator(index, locator)?;
    let (anchor_name, rate) = {
        let StackChild::Track(track) = &working.tracks.children[locator.track_index] else {
            return Err(ApplyError::Invalid {
                index,
                message: "insert_pip: anchor resolved to a non-track stack child".into(),
            });
        };
        let TrackChild::Clip(clip) = &track.children[locator.child_index] else {
            return Err(ApplyError::Invalid {
                index,
                message: "insert_pip: anchor resolved to a non-clip track child".into(),
            });
        };
        let range = clip
            .source_range
            .as_ref()
            .ok_or_else(|| ApplyError::Invalid {
                index,
                message: "insert_pip: anchor clip has no source_range".into(),
            })?;
        (clip.name.clone(), range.start_time.rate)
    };

    let mut pip = Clip::empty(format!("pip-from-{anchor_name}"));
    pip.media_reference = MediaReference::External(ExternalReference::new(asset.to_string()));
    pip.source_range = Some(TimeRange::new(
        RationalTime::new(source_start_s * rate, rate),
        RationalTime::new(duration_s * rate, rate),
    ));
    stamp_fresh_clip_uuid(&mut pip);
    stamp_video_overlay_effect(&mut pip, "pip", Some(corner), Some(scale), Some(margin_pct));

    let target_idx = working
        .tracks
        .children
        .iter()
        .enumerate()
        .find_map(|(i, sc)| match sc {
            StackChild::Track(t)
                if matches!(t.kind, TrackKind::Video) && i != locator.track_index =>
            {
                Some(i)
            }
            _ => None,
        });
    let target_idx = match target_idx {
        Some(i) => i,
        None => {
            let next_name = next_video_track_name(working);
            let track = Track::empty(next_name, TrackKind::Video);
            working.tracks.children.push(StackChild::Track(track));
            working.tracks.children.len() - 1
        }
    };

    let anchor_track_time = track_time_at(working, locator.track_index, locator.child_index);
    let StackChild::Track(target) = &mut working.tracks.children[target_idx] else {
        return Err(ApplyError::Invalid {
            index,
            message: "insert_pip: overlay target resolved to a non-track stack child".into(),
        });
    };
    let target_cursor = track_cursor(target);
    if target_cursor < anchor_track_time {
        target
            .children
            .push(TrackChild::Gap(awidat_proto::otio::Gap::of_duration(
                anchor_track_time - target_cursor,
                rate,
            )));
    }
    target.children.push(TrackChild::Clip(pip));
    Ok(format!(
        "inserted PiP over {anchor_name:?} on overlay track {}: asset={asset:?} \
         source_start={source_start_s:.3}s duration={duration_s:.3}s corner={}",
        target.name,
        pip_corner_str(corner),
    ))
}

/// Effect name used for per-clip volume changes. Render reads this
/// in [`crates/render/src/timeline.rs`]'s FilterPlanner and emits
/// `volume=<value>` on the segment's audio stream.
const VOLUME_EFFECT_NAME: &str = awidat_effects::VOLUME;

/// Effect name used for media overlays and picture-in-picture clips.
const VIDEO_OVERLAY_EFFECT_NAME: &str = awidat_effects::VIDEO_OVERLAY;

/// Effect name used for per-clip audio fades.
const AUDIO_FADE_EFFECT_NAME: &str = awidat_effects::AUDIO_FADE;

/// Effect name used for per-clip speed changes. Render reads this
/// and emits `setpts=<1/factor>*PTS` on video + `atempo=<factor>`
/// on audio (chained when factor is outside `[0.5, 2.0]`).
const SPEED_EFFECT_NAME: &str = awidat_effects::SPEED;

/// Effect name used for clip-level color correction. Render reads the
/// optional numeric fields and emits FFmpeg color filters before speed.
const COLOR_CORRECTION_EFFECT_NAME: &str = awidat_effects::COLOR_CORRECTION;

/// Effect name used for clip-level LUT application. Render maps the
/// project-relative `lut_path` to FFmpeg's `lut3d` filter.
const LUT_EFFECT_NAME: &str = awidat_effects::LUT;

/// Effect name stamped on title-clip synthesized clips. The metadata
/// holds text/position/font_size/color/font_weight/animation; render
/// walks the Titles track and emits drawtext filters per title.
const TITLE_EFFECT_NAME: &str = awidat_effects::TITLE;

/// Track name used for the auto-created Titles track. Render walks
/// any video track whose `metadata["awidat_track_role"]` is "titles"
/// (not just by name) so the user can rename it.
const TITLES_TRACK_NAME: &str = "Titles";

/// Track-metadata key flagging this track as the project's title
/// overlay track. `apply_insert_title` stamps it on track creation;
/// the render pipeline pattern-matches on it.
const TITLES_TRACK_ROLE_KEY: &str = "awidat_track_role";

/// Track-metadata value for the titles track.
const TITLES_TRACK_ROLE_VALUE: &str = "titles";

/// Track metadata key holding first-class audio controls.
const AUDIO_TRACK_METADATA_KEY: &str = "awidat_audio";

/// Clip effect storing waveform/timecode sync metadata.
const SYNC_GROUP_EFFECT_NAME: &str = "awidat.sync_group";

/// Clip effect storing FFmpeg-native audio repair settings.
const AUDIO_FX_EFFECT_NAME: &str = "awidat.audio_fx";

/// Stamp an awidat.volume Effect on the anchored clip with `value`
/// (linear gain multiplier). Idempotent: any existing
/// awidat.volume effect on the clip is removed first, so two
/// SetVolume ops in one envelope leave only the second's value.
fn apply_set_volume(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    value: f64,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    if !value.is_finite() || value < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_volume: value {value} must be finite and >= 0.0"),
        });
    }
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_volume: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_volume: anchor resolved to a non-clip track child".into(),
        });
    };
    clip.effects.retain(|e| e.effect_name != VOLUME_EFFECT_NAME);
    let mut effect = awidat_proto::otio::Effect::new(VOLUME_EFFECT_NAME);
    effect
        .metadata
        .insert("value".to_string(), serde_json::json!(value));
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!(
        "set volume on clip {clip_name:?} to {value:.3} (linear gain)"
    ))
}

fn apply_set_audio_fade(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    fade_in_s: Option<f64>,
    fade_out_s: Option<f64>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let fade_in = fade_in_s.unwrap_or(0.0);
    let fade_out = fade_out_s.unwrap_or(0.0);
    if !fade_in.is_finite() || !fade_out.is_finite() || fade_in < 0.0 || fade_out < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "set_audio_fade: fade_in_s {fade_in} and fade_out_s {fade_out} must be finite and >= 0.0"
            ),
        });
    }
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_audio_fade: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_audio_fade: anchor resolved to a non-clip track child".into(),
        });
    };
    clip.effects
        .retain(|e| e.effect_name != AUDIO_FADE_EFFECT_NAME);
    let mut effect = awidat_proto::otio::Effect::new(AUDIO_FADE_EFFECT_NAME);
    effect
        .metadata
        .insert("fade_in_s".to_string(), serde_json::json!(fade_in));
    effect
        .metadata
        .insert("fade_out_s".to_string(), serde_json::json!(fade_out));
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!(
        "set audio fade on clip {clip_name:?}: in={fade_in:.3}s out={fade_out:.3}s"
    ))
}

fn apply_set_track_audio(
    working: &mut Timeline,
    index: usize,
    track_name: &str,
    role: Option<&str>,
    volume: Option<f64>,
    muted: Option<bool>,
    solo: Option<bool>,
) -> Result<String, ApplyError> {
    if let Some(volume) = volume
        && (!volume.is_finite() || volume < 0.0)
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_track_audio: volume {volume} must be finite and >= 0.0"),
        });
    }
    let track = find_track_mut(working, track_name).ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!("set_track_audio: track {track_name:?} not found"),
    })?;
    if !matches!(track.kind, TrackKind::Audio) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_track_audio: track {track_name:?} is not an audio track"),
        });
    }
    let mut value = track
        .metadata
        .get(AUDIO_TRACK_METADATA_KEY)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let map = value.as_object_mut().ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!(
            "set_track_audio: existing {AUDIO_TRACK_METADATA_KEY} metadata is not an object"
        ),
    })?;
    if let Some(role) = role {
        map.insert("role".into(), serde_json::Value::String(role.to_string()));
    }
    if let Some(volume) = volume {
        map.insert("volume".into(), serde_json::json!(volume));
    }
    if let Some(muted) = muted {
        map.insert("muted".into(), serde_json::json!(muted));
    }
    if let Some(solo) = solo {
        map.insert("solo".into(), serde_json::json!(solo));
    }
    track
        .metadata
        .insert(AUDIO_TRACK_METADATA_KEY.to_string(), value);
    Ok(format!("set audio controls on track {track_name:?}"))
}

fn apply_set_ducking(
    working: &mut Timeline,
    index: usize,
    track_name: &str,
    enabled: Option<bool>,
    amount_db: Option<f64>,
    attack_ms: Option<f64>,
    release_ms: Option<f64>,
) -> Result<String, ApplyError> {
    let enabled = enabled.unwrap_or(true);
    let amount_db = amount_db.unwrap_or(-12.0);
    let attack_ms = attack_ms.unwrap_or(80.0);
    let release_ms = release_ms.unwrap_or(300.0);
    if !amount_db.is_finite()
        || !attack_ms.is_finite()
        || !release_ms.is_finite()
        || attack_ms < 0.0
        || release_ms < 0.0
    {
        return Err(ApplyError::Invalid {
            index,
            message: "set_ducking: amount_db, attack_ms, and release_ms must be finite; timings must be >= 0.0".into(),
        });
    }
    let track = find_track_mut(working, track_name).ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!("set_ducking: track {track_name:?} not found"),
    })?;
    if !matches!(track.kind, TrackKind::Audio) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_ducking: track {track_name:?} is not an audio track"),
        });
    }
    let mut value = track
        .metadata
        .get(AUDIO_TRACK_METADATA_KEY)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let map = value.as_object_mut().ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!(
            "set_ducking: existing {AUDIO_TRACK_METADATA_KEY} metadata is not an object"
        ),
    })?;
    map.insert(
        "ducking".into(),
        serde_json::json!({
            "enabled": enabled,
            "amount_db": amount_db,
            "attack_ms": attack_ms,
            "release_ms": release_ms,
        }),
    );
    track
        .metadata
        .insert(AUDIO_TRACK_METADATA_KEY.to_string(), value);
    Ok(format!(
        "set ducking on track {track_name:?}: enabled={enabled} amount={amount_db:.1}dB"
    ))
}

#[allow(clippy::too_many_arguments)]
fn apply_set_sync_group(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    sync_group_id: &str,
    offset_s: f64,
    speed_factor: Option<f64>,
    confidence: Option<f64>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    if sync_group_id.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "set_sync_group: sync_group_id must not be empty".into(),
        });
    }
    if !offset_s.is_finite() {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_sync_group: offset_s {offset_s} must be finite"),
        });
    }
    if let Some(factor) = speed_factor
        && (!factor.is_finite() || factor <= 0.0)
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_sync_group: speed_factor {factor} must be finite and > 0.0"),
        });
    }
    if let Some(confidence) = confidence
        && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_sync_group: confidence {confidence} must be in [0, 1]"),
        });
    }

    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_sync_group: anchor resolved to a non-track stack child".into(),
        });
    };

    let clip_name = {
        let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
            return Err(ApplyError::Invalid {
                index,
                message: "set_sync_group: anchor resolved to a non-clip track child".into(),
            });
        };
        clip.effects
            .retain(|e| e.effect_name != SYNC_GROUP_EFFECT_NAME);
        let mut effect = awidat_proto::otio::Effect::new(SYNC_GROUP_EFFECT_NAME);
        effect
            .metadata
            .insert("sync_group_id".into(), serde_json::json!(sync_group_id));
        effect
            .metadata
            .insert("offset_s".into(), serde_json::json!(offset_s));
        if let Some(speed_factor) = speed_factor {
            effect
                .metadata
                .insert("speed_factor".into(), serde_json::json!(speed_factor));
            if (speed_factor - 1.0).abs() > 1e-9 {
                clip.effects.retain(|e| e.effect_name != SPEED_EFFECT_NAME);
                let mut speed = awidat_proto::otio::Effect::new(SPEED_EFFECT_NAME);
                speed
                    .metadata
                    .insert("factor".into(), serde_json::json!(speed_factor));
                clip.effects.push(speed);
            }
        }
        if let Some(confidence) = confidence {
            effect
                .metadata
                .insert("confidence".into(), serde_json::json!(confidence));
        }
        clip.effects.push(effect);
        clip.name.clone()
    };

    let aligned_to_s = offset_s.max(0.0);
    if aligned_to_s > 0.0 {
        apply_move_clip_to_time(index, track, locator.child_index, aligned_to_s)?;
    }
    Ok(format!(
        "set sync group {sync_group_id:?} on clip {clip_name:?}: offset={offset_s:.3}s{}{}",
        speed_factor
            .map(|v| format!(" speed_factor={v:.6}"))
            .unwrap_or_default(),
        confidence
            .map(|v| format!(" confidence={v:.2}"))
            .unwrap_or_default()
    ))
}

fn apply_set_clip_audio_fx(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    fx: &AudioFxConfig,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    validate_audio_fx(index, "set_clip_audio_fx", fx)?;
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_clip_audio_fx: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_clip_audio_fx: anchor resolved to a non-clip track child".into(),
        });
    };
    clip.effects
        .retain(|e| e.effect_name != AUDIO_FX_EFFECT_NAME);
    let mut effect = awidat_proto::otio::Effect::new(AUDIO_FX_EFFECT_NAME);
    effect.metadata = audio_fx_metadata(index, fx)?;
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!("set audio FX on clip {clip_name:?}"))
}

fn apply_set_track_audio_fx(
    working: &mut Timeline,
    index: usize,
    track_name: &str,
    fx: &AudioFxConfig,
) -> Result<String, ApplyError> {
    validate_audio_fx(index, "set_track_audio_fx", fx)?;
    let track = find_track_mut(working, track_name).ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!("set_track_audio_fx: track {track_name:?} not found"),
    })?;
    if !matches!(track.kind, TrackKind::Audio) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_track_audio_fx: track {track_name:?} is not an audio track"),
        });
    }
    let mut value = track
        .metadata
        .get(AUDIO_TRACK_METADATA_KEY)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let map = value.as_object_mut().ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!(
            "set_track_audio_fx: existing {AUDIO_TRACK_METADATA_KEY} metadata is not an object"
        ),
    })?;
    map.insert(
        "fx".into(),
        serde_json::Value::Object(audio_fx_metadata(index, fx)?),
    );
    track
        .metadata
        .insert(AUDIO_TRACK_METADATA_KEY.to_string(), value);
    Ok(format!("set audio FX on track {track_name:?}"))
}

fn validate_audio_fx(index: usize, label: &str, fx: &AudioFxConfig) -> Result<(), ApplyError> {
    if fx.is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: format!("{label}: at least one audio FX parameter is required"),
        });
    }
    let nums = [
        ("high_pass_hz", fx.high_pass_hz),
        ("low_pass_hz", fx.low_pass_hz),
        ("compressor_threshold_db", fx.compressor_threshold_db),
        ("compressor_ratio", fx.compressor_ratio),
        ("limiter_limit_db", fx.limiter_limit_db),
        ("noise_gate_threshold_db", fx.noise_gate_threshold_db),
        ("hum_notch_hz", fx.hum_notch_hz),
        ("de_ess_hz", fx.de_ess_hz),
        ("de_ess_reduction_db", fx.de_ess_reduction_db),
        ("loudnorm_i", fx.loudnorm_i),
        ("loudnorm_tp", fx.loudnorm_tp),
    ];
    for (name, value) in nums {
        if let Some(value) = value
            && !value.is_finite()
        {
            return Err(ApplyError::Invalid {
                index,
                message: format!("{label}: {name} must be finite"),
            });
        }
    }
    for band in &fx.eq_bands {
        if !band.freq_hz.is_finite()
            || !band.gain_db.is_finite()
            || band.width_hz.is_some_and(|v| !v.is_finite())
        {
            return Err(ApplyError::Invalid {
                index,
                message: format!("{label}: EQ band values must be finite"),
            });
        }
    }
    Ok(())
}

fn audio_fx_metadata(
    index: usize,
    fx: &AudioFxConfig,
) -> Result<serde_json::Map<String, serde_json::Value>, ApplyError> {
    serde_json::to_value(fx)
        .map_err(|e| ApplyError::Invalid {
            index,
            message: format!("audio_fx: serialize config failed: {e}"),
        })?
        .as_object()
        .cloned()
        .ok_or_else(|| ApplyError::Invalid {
            index,
            message: "audio_fx: serialized config was not an object".into(),
        })
}

fn apply_set_effect(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    effect_id: &str,
    params: &serde_json::Map<String, serde_json::Value>,
    rationale: Option<&str>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let (definition, mut metadata) =
        awidat_effects::normalize_params(effect_id, params).map_err(|e| ApplyError::Invalid {
            index,
            message: format!("set_effect: {e}"),
        })?;
    if !matches!(definition.scope, awidat_effects::EffectScope::Clip) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_effect: effect {effect_id:?} is not a clip-scoped effect"),
        });
    }
    if let Some(rationale) = rationale
        && !rationale.trim().is_empty()
    {
        metadata.insert(
            "rationale".to_string(),
            serde_json::Value::String(rationale.to_string()),
        );
    }

    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_effect: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_effect: anchor resolved to a non-clip track child".into(),
        });
    };

    if matches!(definition.stack_policy, StackPolicy::ReplaceSameId) {
        clip.effects.retain(|e| e.effect_name != definition.id);
    }
    let mut effect = awidat_proto::otio::Effect::new(definition.id);
    effect.name = definition.display_name.to_string();
    effect.metadata = metadata;
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!(
        "set effect {} on clip {clip_name:?}",
        definition.id
    ))
}

/// Stamp an awidat.speed Effect on the anchored clip with `factor`
/// (playback rate multiplier). Idempotent: any existing
/// awidat.speed effect on the clip is removed first.
fn apply_set_speed(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    factor: f64,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    if !factor.is_finite() || factor <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_speed: factor {factor} must be finite and > 0.0"),
        });
    }
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_speed: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_speed: anchor resolved to a non-clip track child".into(),
        });
    };
    clip.effects.retain(|e| e.effect_name != SPEED_EFFECT_NAME);
    let mut effect = awidat_proto::otio::Effect::new(SPEED_EFFECT_NAME);
    effect
        .metadata
        .insert("factor".to_string(), serde_json::json!(factor));
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!(
        "set speed on clip {clip_name:?} to {factor:.3}× (timeline duration scales by 1/factor)"
    ))
}

#[allow(clippy::too_many_arguments)]
fn apply_set_color_correction(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    exposure_ev: Option<f64>,
    contrast: Option<f64>,
    saturation: Option<f64>,
    temperature: Option<f64>,
    tint: Option<f64>,
    shadows: Option<f64>,
    highlights: Option<f64>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let fields = [
        ("exposure_ev", exposure_ev, -4.0, 4.0),
        ("contrast", contrast, 0.0, 3.0),
        ("saturation", saturation, 0.0, 3.0),
        ("temperature", temperature, -1.0, 1.0),
        ("tint", tint, -1.0, 1.0),
        ("shadows", shadows, -1.0, 1.0),
        ("highlights", highlights, -1.0, 1.0),
    ];
    if fields.iter().all(|(_, value, _, _)| value.is_none()) {
        return Err(ApplyError::Invalid {
            index,
            message: "set_color_correction: provide at least one correction field".into(),
        });
    }
    for (name, value, min, max) in fields {
        if let Some(value) = value {
            if !value.is_finite() || value < min || value > max {
                return Err(ApplyError::Invalid {
                    index,
                    message: format!(
                        "set_color_correction: {name} {value} must be finite and within [{min}, {max}]"
                    ),
                });
            }
        }
    }

    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_color_correction: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_color_correction: anchor resolved to a non-clip track child".into(),
        });
    };

    clip.effects
        .retain(|e| e.effect_name != COLOR_CORRECTION_EFFECT_NAME);
    let mut effect = awidat_proto::otio::Effect::new(COLOR_CORRECTION_EFFECT_NAME);
    for (name, value, _, _) in [
        ("exposure_ev", exposure_ev, -4.0, 4.0),
        ("contrast", contrast, 0.0, 3.0),
        ("saturation", saturation, 0.0, 3.0),
        ("temperature", temperature, -1.0, 1.0),
        ("tint", tint, -1.0, 1.0),
        ("shadows", shadows, -1.0, 1.0),
        ("highlights", highlights, -1.0, 1.0),
    ] {
        if let Some(value) = value {
            effect
                .metadata
                .insert(name.to_string(), serde_json::json!(value));
        }
    }
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!("set color correction on clip {clip_name:?}"))
}

fn apply_lut(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    lut_path: &str,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    validate_lut_path(index, lut_path)?;
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "apply_lut: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "apply_lut: anchor resolved to a non-clip track child".into(),
        });
    };
    clip.effects.retain(|e| e.effect_name != LUT_EFFECT_NAME);
    let mut effect = awidat_proto::otio::Effect::new(LUT_EFFECT_NAME);
    effect
        .metadata
        .insert("lut_path".to_string(), serde_json::json!(lut_path));
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!("applied LUT {lut_path:?} to clip {clip_name:?}"))
}

fn validate_lut_path(index: usize, lut_path: &str) -> Result<(), ApplyError> {
    let path = std::path::Path::new(lut_path);
    if lut_path.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(ApplyError::Invalid {
            index,
            message: "apply_lut: lut_path must be a non-empty project-relative path without '..'"
                .into(),
        });
    }
    Ok(())
}

fn validate_project_relative_asset(
    index: usize,
    op_name: &str,
    asset: &str,
) -> Result<(), ApplyError> {
    let path = std::path::Path::new(asset);
    if asset.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "{op_name}: asset must be a non-empty project-relative path without '..'"
            ),
        });
    }
    Ok(())
}

fn stamp_video_overlay_effect(
    clip: &mut awidat_proto::otio::Clip,
    mode: &str,
    corner: Option<super::op::PiPCorner>,
    scale: Option<f64>,
    margin_pct: Option<f64>,
) {
    clip.effects
        .retain(|e| e.effect_name != VIDEO_OVERLAY_EFFECT_NAME);
    let mut effect = awidat_proto::otio::Effect::new(VIDEO_OVERLAY_EFFECT_NAME);
    effect
        .metadata
        .insert("mode".to_string(), serde_json::json!(mode));
    if let Some(corner) = corner {
        effect.metadata.insert(
            "corner".to_string(),
            serde_json::json!(pip_corner_str(corner)),
        );
    }
    if let Some(scale) = scale {
        effect
            .metadata
            .insert("scale".to_string(), serde_json::json!(scale));
    }
    if let Some(margin_pct) = margin_pct {
        effect
            .metadata
            .insert("margin_pct".to_string(), serde_json::json!(margin_pct));
    }
    clip.effects.push(effect);
}

fn pip_corner_str(corner: super::op::PiPCorner) -> &'static str {
    match corner {
        super::op::PiPCorner::TopLeft => "top_left",
        super::op::PiPCorner::TopRight => "top_right",
        super::op::PiPCorner::BottomLeft => "bottom_left",
        super::op::PiPCorner::BottomRight => "bottom_right",
    }
}

/// Insert a title overlay onto the project's Titles track. The
/// titles track auto-creates on first call (Video kind, flagged via
/// track metadata). The overlay is structurally a Clip with a
/// MissingReference media_reference (no real media — drawtext
/// renders the title from the awidat.title Effect's metadata) and
/// a source_range whose duration matches `end_s - start_s`.
///
/// Stamps a fresh clip_uuid on the synthesized clip so subsequent
/// `Set Title` ops can resolve via `Anchor::ClipUuid`.
#[allow(clippy::too_many_arguments)]
fn apply_insert_title(
    working: &mut Timeline,
    index: usize,
    start_s: f64,
    end_s: f64,
    text: &str,
    position: super::op::TitlePosition,
    font_size: u32,
    color: &str,
    font_weight: super::op::TitleWeight,
    animation: super::op::TitleAnimation,
) -> Result<String, ApplyError> {
    apply_insert_text_overlay(
        working,
        index,
        "title",
        None,
        start_s,
        end_s,
        text,
        position,
        font_size,
        color,
        font_weight,
        animation,
    )
}

/// Insert a caption overlay as a graph node. Captions intentionally
/// reuse the title render effect so the graph has one overlay track
/// while still preserving `role = "caption"` for downstream format
/// and audit tooling.
#[allow(clippy::too_many_arguments)]
fn apply_insert_caption(
    working: &mut Timeline,
    index: usize,
    start_s: f64,
    end_s: f64,
    text: &str,
    position: super::op::TitlePosition,
    font_size: u32,
    color: &str,
    safe_area: &str,
) -> Result<String, ApplyError> {
    if safe_area.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "insert_caption: safe_area must be non-empty".into(),
        });
    }
    apply_insert_text_overlay(
        working,
        index,
        "caption",
        Some(safe_area),
        start_s,
        end_s,
        text,
        position,
        font_size,
        color,
        super::op::TitleWeight::Bold,
        super::op::TitleAnimation::None,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_insert_text_overlay(
    working: &mut Timeline,
    index: usize,
    role: &str,
    safe_area: Option<&str>,
    start_s: f64,
    end_s: f64,
    text: &str,
    position: super::op::TitlePosition,
    font_size: u32,
    color: &str,
    font_weight: super::op::TitleWeight,
    animation: super::op::TitleAnimation,
) -> Result<String, ApplyError> {
    use awidat_proto::otio::{Clip, RationalTime, TimeRange};

    if !start_s.is_finite() || !end_s.is_finite() || end_s <= start_s {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "insert_{role}: invalid window [{start_s}..{end_s}]; end_s must be finite and > start_s"
            ),
        });
    }
    if text.is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: format!("insert_{role}: text must be non-empty"),
        });
    }

    // Find or create the Titles track. Match by metadata flag first
    // (the user may have renamed it), then by the canonical name as
    // a fallback.
    let titles_idx = find_or_create_titles_track(working);
    let StackChild::Track(track) = &mut working.tracks.children[titles_idx] else {
        return Err(ApplyError::Invalid {
            index,
            message: format!("insert_{role}: titles track resolved to a non-track stack child"),
        });
    };

    // The title's source_range is interpreted in *timeline-time*
    // here — the Titles track is treated as a top-level container,
    // not a media-bearing track. Render walks `track_role: titles`
    // tracks and reads start_s/end_s from each clip's source_range
    // for the drawtext `enable=between(t,...)` window.
    let rate = 24.0_f64;
    let duration_s = end_s - start_s;
    let mut clip = Clip::empty(format!("{role}-{:.3}-{:.3}", start_s, end_s));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(start_s * rate, rate),
        RationalTime::new(duration_s * rate, rate),
    ));
    stamp_fresh_clip_uuid(&mut clip);

    // Build the awidat.title effect with all the styling.
    let mut effect = awidat_proto::otio::Effect::new(TITLE_EFFECT_NAME);
    effect
        .metadata
        .insert("role".to_string(), serde_json::json!(role));
    effect
        .metadata
        .insert("text".to_string(), serde_json::json!(text));
    effect
        .metadata
        .insert("start_s".to_string(), serde_json::json!(start_s));
    effect
        .metadata
        .insert("end_s".to_string(), serde_json::json!(end_s));
    effect.metadata.insert(
        "position".to_string(),
        serde_json::json!(title_position_str(position)),
    );
    effect
        .metadata
        .insert("font_size".to_string(), serde_json::json!(font_size));
    effect
        .metadata
        .insert("color".to_string(), serde_json::json!(color));
    effect.metadata.insert(
        "font_weight".to_string(),
        serde_json::json!(title_weight_str(font_weight)),
    );
    effect.metadata.insert(
        "animation".to_string(),
        serde_json::json!(title_animation_str(animation)),
    );
    if let Some(profile) = safe_area {
        effect
            .metadata
            .insert("safe_area".to_string(), serde_json::json!(profile));
    }
    clip.effects.push(effect);

    // Insert the title in playback order so the Titles track stays
    // sorted by start_s. This makes render's left-to-right walk
    // produce drawtext filters in time order — a small nicety for
    // anyone reading the generated argv.
    let position_idx = title_insertion_index(track, start_s);
    track.children.insert(position_idx, TrackChild::Clip(clip));

    Ok(format!(
        "inserted {role} {text:?} on Titles track at [{start_s:.3}s..{end_s:.3}s] \
         (position={position:?}, animation={animation:?})"
    ))
}

fn timeline_awidat_metadata(working: &mut Timeline) -> &mut AwidatTimelineMetadata {
    let meta = working
        .metadata
        .awidat
        .get_or_insert_with(AwidatTimelineMetadata::default);
    if meta.version.is_empty() {
        meta.version = awidat_proto::AWIDAT_PROJECT_VERSION.to_string();
    }
    meta
}

fn apply_set_output_format(
    working: &mut Timeline,
    index: usize,
    aspect_ratio: &str,
    platform: Option<&str>,
    safe_area: Option<&str>,
) -> Result<String, ApplyError> {
    const SUPPORTED: &[&str] = &["16:9", "9:16", "1:1", "4:5"];
    if !SUPPORTED.contains(&aspect_ratio) {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "set_output_format: aspect_ratio {aspect_ratio:?} must be one of {SUPPORTED:?}"
            ),
        });
    }
    let meta = timeline_awidat_metadata(working);
    meta.extra.insert(
        "output_format".into(),
        serde_json::json!({
            "aspect_ratio": aspect_ratio,
            "platform": platform,
            "safe_area": safe_area,
        }),
    );
    Ok(format!(
        "set output format to aspect_ratio={aspect_ratio:?}, platform={:?}, safe_area={:?}",
        platform, safe_area
    ))
}

fn apply_set_loudness_target(
    working: &mut Timeline,
    index: usize,
    integrated_lufs: f64,
    true_peak_db: Option<f64>,
) -> Result<String, ApplyError> {
    if !integrated_lufs.is_finite() || integrated_lufs >= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "set_loudness_target: integrated_lufs {integrated_lufs} must be finite and below 0"
            ),
        });
    }
    if let Some(peak) = true_peak_db
        && (!peak.is_finite() || peak > 0.0)
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_loudness_target: true_peak_db {peak} must be finite and <= 0"),
        });
    }
    let meta = timeline_awidat_metadata(working);
    meta.extra.insert(
        "loudness_target".into(),
        serde_json::json!({
            "integrated_lufs": integrated_lufs,
            "true_peak_db": true_peak_db,
        }),
    );
    Ok(format!(
        "set loudness target to {integrated_lufs:.1} LUFS, true_peak_db={true_peak_db:?}"
    ))
}

fn apply_set_package_metadata(
    working: &mut Timeline,
    index: usize,
    platform: Option<&str>,
    title: Option<&str>,
    description: Option<&str>,
    tags: Option<&str>,
) -> Result<String, ApplyError> {
    if platform.is_none() && title.is_none() && description.is_none() && tags.is_none() {
        return Err(ApplyError::Invalid {
            index,
            message: "set_package_metadata: at least one field is required".into(),
        });
    }
    let meta = timeline_awidat_metadata(working);
    meta.extra.insert(
        "package_metadata".into(),
        serde_json::json!({
            "platform": platform,
            "title": title,
            "description": description,
            "tags": tags,
        }),
    );
    Ok(format!(
        "set package metadata platform={:?}, title={:?}",
        platform, title
    ))
}

fn apply_set_broadcast_overlay(
    working: &mut Timeline,
    index: usize,
    config: &BroadcastOverlayConfig,
) -> Result<String, ApplyError> {
    validate_broadcast_overlay_config(index, config)?;
    let meta = timeline_awidat_metadata(working);
    meta.broadcast_overlay = Some(config.clone());
    Ok(format!(
        "set broadcast overlay enabled={}, title={:?}, topics={}, chapters={}",
        config.enabled,
        config.episode_title,
        config.topics.len(),
        config.chapters.len(),
    ))
}

fn validate_broadcast_overlay_config(
    index: usize,
    config: &BroadcastOverlayConfig,
) -> Result<(), ApplyError> {
    validate_optional_project_path(index, "brand_logo_path", config.brand_logo_path.as_deref())?;
    validate_optional_project_path(
        index,
        "host_a.photo_path",
        config.host_a.photo_path.as_deref(),
    )?;
    validate_optional_project_path(
        index,
        "host_b.photo_path",
        config.host_b.photo_path.as_deref(),
    )?;
    validate_timed_entries(index, "topics", &config.topics)?;
    validate_timed_entries(index, "chapters", &config.chapters)?;
    validate_overlay_style(index, &config.style)?;
    Ok(())
}

fn validate_optional_project_path(
    index: usize,
    field: &str,
    value: Option<&str>,
) -> Result<(), ApplyError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_broadcast_overlay: {field} cannot be empty"),
        });
    }
    let path = std::path::Path::new(value);
    if path.is_absolute() || value.split('/').any(|part| part == "..") {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "set_broadcast_overlay: {field} must be project-relative and cannot contain '..'"
            ),
        });
    }
    Ok(())
}

fn validate_timed_entries(
    index: usize,
    field: &str,
    entries: &[BroadcastTimedEntry],
) -> Result<(), ApplyError> {
    for entry in entries {
        if !entry.time_seconds.is_finite() || entry.time_seconds < 0.0 {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "set_broadcast_overlay: {field} time_seconds must be finite and >= 0"
                ),
            });
        }
        if entry.text.trim().is_empty() {
            return Err(ApplyError::Invalid {
                index,
                message: format!("set_broadcast_overlay: {field} text cannot be empty"),
            });
        }
    }
    Ok(())
}

fn validate_overlay_style(index: usize, style: &BroadcastOverlayStyle) -> Result<(), ApplyError> {
    let finite_positive = [
        ("title_fade_in_end", style.title_fade_in_end),
        ("title_visible_end", style.title_visible_end),
        ("host_intro_end", style.host_intro_end),
        ("ticker_sponsor_duration", style.ticker_sponsor_duration),
        ("ticker_fade_duration", style.ticker_fade_duration),
        ("ticker_topic_duration", style.ticker_topic_duration),
        ("chapter_display_duration", style.chapter_display_duration),
        ("name_bar_height", style.name_bar_height),
        ("ticker_height", style.ticker_height),
        ("host_strip_height", style.host_strip_height),
    ];
    for (field, value) in finite_positive {
        if !value.is_finite() || value <= 0.0 {
            return Err(ApplyError::Invalid {
                index,
                message: format!("set_broadcast_overlay: style.{field} must be finite and > 0"),
            });
        }
    }
    if !style.title_fade_out_start.is_finite() || style.title_fade_out_start < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: "set_broadcast_overlay: style.title_fade_out_start must be finite and >= 0"
                .into(),
        });
    }
    if !style.host_intro_start.is_finite() || style.host_intro_start < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: "set_broadcast_overlay: style.host_intro_start must be finite and >= 0".into(),
        });
    }
    if style.title_visible_end <= style.title_fade_in_end
        || style.title_fade_out_start > style.title_visible_end
        || style.host_intro_end <= style.host_intro_start
    {
        return Err(ApplyError::Invalid {
            index,
            message: "set_broadcast_overlay: style timing windows are inconsistent".into(),
        });
    }
    Ok(())
}

/// Update the awidat.title effect on an anchored title clip.
/// All styling fields are optional — None leaves the existing value
/// alone. start_s / end_s adjust both the effect metadata AND the
/// underlying clip's source_range so the timeline window stays in
/// sync with the drawtext enable window.
#[allow(clippy::too_many_arguments)]
fn apply_set_title(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    start_s: Option<f64>,
    end_s: Option<f64>,
    text: Option<&str>,
    position: Option<super::op::TitlePosition>,
    font_size: Option<u32>,
    color: Option<&str>,
    font_weight: Option<super::op::TitleWeight>,
    animation: Option<super::op::TitleAnimation>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_title: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_title: anchor resolved to a non-clip child".into(),
        });
    };
    let effect = clip
        .effects
        .iter_mut()
        .find(|e| e.effect_name == TITLE_EFFECT_NAME)
        .ok_or_else(|| ApplyError::Invalid {
            index,
            message: format!(
                "set_title: anchored clip {:?} carries no awidat.title effect — \
                 SetTitle only updates existing titles, use InsertTitle to create one",
                clip.name,
            ),
        })?;

    if let Some(t) = text {
        if t.is_empty() {
            return Err(ApplyError::Invalid {
                index,
                message: "set_title: text must be non-empty".into(),
            });
        }
        effect
            .metadata
            .insert("text".to_string(), serde_json::json!(t));
    }
    if let Some(p) = position {
        effect.metadata.insert(
            "position".to_string(),
            serde_json::json!(title_position_str(p)),
        );
    }
    if let Some(f) = font_size {
        effect
            .metadata
            .insert("font_size".to_string(), serde_json::json!(f));
    }
    if let Some(c) = color {
        effect
            .metadata
            .insert("color".to_string(), serde_json::json!(c));
    }
    if let Some(w) = font_weight {
        effect.metadata.insert(
            "font_weight".to_string(),
            serde_json::json!(title_weight_str(w)),
        );
    }
    if let Some(a) = animation {
        effect.metadata.insert(
            "animation".to_string(),
            serde_json::json!(title_animation_str(a)),
        );
    }

    // start_s / end_s require both the effect metadata AND the
    // clip's source_range to update so the timeline view + the
    // render's enable window agree.
    if start_s.is_some() || end_s.is_some() {
        let prior = clip
            .source_range
            .as_ref()
            .ok_or_else(|| ApplyError::Invalid {
                index,
                message: "set_title: title clip has no source_range to mutate".into(),
            })?;
        let rate = prior.start_time.rate;
        let prior_start = prior.start_time.to_seconds();
        let prior_end = prior_start + prior.duration.to_seconds();
        let new_start = start_s.unwrap_or(prior_start);
        let new_end = end_s.unwrap_or(prior_end);
        if !new_start.is_finite() || !new_end.is_finite() || new_end <= new_start {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "set_title: invalid window [{new_start}..{new_end}]; end_s must be > start_s"
                ),
            });
        }
        clip.source_range = Some(awidat_proto::otio::TimeRange::new(
            awidat_proto::otio::RationalTime::new(new_start * rate, rate),
            awidat_proto::otio::RationalTime::new((new_end - new_start) * rate, rate),
        ));
        effect
            .metadata
            .insert("start_s".to_string(), serde_json::json!(new_start));
        effect
            .metadata
            .insert("end_s".to_string(), serde_json::json!(new_end));
    }

    Ok(format!("updated title clip {:?}", clip.name))
}

/// Find the existing Titles track or push a new one onto the
/// timeline. Returns the track's index in `tracks.children`. The
/// new track is flagged via `metadata["awidat_track_role"] = "titles"`
/// so the render pipeline can route its clips into drawtext filters.
fn find_or_create_titles_track(working: &mut Timeline) -> usize {
    if let Some(idx) = working
        .tracks
        .children
        .iter()
        .position(|sc| matches!(sc, StackChild::Track(t) if is_titles_track(t)))
    {
        return idx;
    }
    let mut track = awidat_proto::otio::Track::empty(
        TITLES_TRACK_NAME.to_string(),
        awidat_proto::otio::TrackKind::Video,
    );
    track.metadata.insert(
        TITLES_TRACK_ROLE_KEY.to_string(),
        serde_json::json!(TITLES_TRACK_ROLE_VALUE),
    );
    working.tracks.children.push(StackChild::Track(track));
    working.tracks.children.len() - 1
}

/// True iff `track` is the project's Titles track. Matches the
/// metadata flag first; falls back to the canonical name so a
/// hand-edited OTIO from before the metadata flag landed is still
/// recognized.
fn is_titles_track(track: &awidat_proto::otio::Track) -> bool {
    if track
        .metadata
        .get(TITLES_TRACK_ROLE_KEY)
        .and_then(|v| v.as_str())
        == Some(TITLES_TRACK_ROLE_VALUE)
    {
        return true;
    }
    track.name == TITLES_TRACK_NAME
}

/// Find the insertion index for a new title at `start_s` on the
/// Titles track. Walks children left-to-right and returns the first
/// position whose existing source_range start is >= start_s. Keeps
/// the track sorted by start_s for readable render output.
fn title_insertion_index(track: &awidat_proto::otio::Track, start_s: f64) -> usize {
    for (i, child) in track.children.iter().enumerate() {
        let TrackChild::Clip(c) = child else { continue };
        let existing_start = c
            .source_range
            .as_ref()
            .map(|r| r.start_time.to_seconds())
            .unwrap_or(0.0);
        if existing_start >= start_s {
            return i;
        }
    }
    track.children.len()
}

fn title_position_str(p: super::op::TitlePosition) -> &'static str {
    match p {
        super::op::TitlePosition::Top => "top",
        super::op::TitlePosition::Center => "center",
        super::op::TitlePosition::Bottom => "bottom",
    }
}

fn title_weight_str(w: super::op::TitleWeight) -> &'static str {
    match w {
        super::op::TitleWeight::Normal => "normal",
        super::op::TitleWeight::Bold => "bold",
    }
}

fn title_animation_str(a: super::op::TitleAnimation) -> &'static str {
    match a {
        super::op::TitleAnimation::None => "none",
        super::op::TitleAnimation::FadeIn => "fade_in",
        super::op::TitleAnimation::FadeOut => "fade_out",
        super::op::TitleAnimation::FadeInOut => "fade_in_out",
        super::op::TitleAnimation::SlideIn => "slide_in",
        super::op::TitleAnimation::SlideOut => "slide_out",
    }
}

fn primary_content_track_index(timeline: &Timeline) -> Option<usize> {
    timeline
        .tracks
        .children
        .iter()
        .enumerate()
        .find_map(|(i, sc)| match sc {
            StackChild::Track(track) if !is_titles_track(track) => Some(i),
            _ => None,
        })
}

fn broadcast_overlay_shift_for_delete(
    timeline: &Timeline,
    locator: &ClipLocator,
) -> Option<(f64, f64)> {
    if primary_content_track_index(timeline) != Some(locator.track_index) {
        return None;
    }
    let StackChild::Track(track) = &timeline.tracks.children[locator.track_index] else {
        return None;
    };
    if is_titles_track(track) {
        return None;
    }
    let child = track.children.get(locator.child_index)?;
    let duration = child_duration(child);
    if !duration.is_finite() || duration <= 0.0 {
        return None;
    }
    Some((
        track_time_at(timeline, locator.track_index, locator.child_index),
        duration,
    ))
}

fn shift_broadcast_overlay_timestamps(timeline: &mut Timeline, cut_point: f64, duration: f64) {
    if !cut_point.is_finite() || !duration.is_finite() || duration <= 0.0 {
        return;
    }
    let meta = timeline_awidat_metadata(timeline);
    let Some(overlay) = meta.broadcast_overlay.as_mut() else {
        return;
    };
    shift_broadcast_timed_entries(&mut overlay.topics, cut_point, duration);
    shift_broadcast_timed_entries(&mut overlay.chapters, cut_point, duration);
}

fn shift_broadcast_timed_entries(
    entries: &mut [BroadcastTimedEntry],
    cut_point: f64,
    duration: f64,
) {
    for entry in entries {
        if entry.time_seconds > cut_point {
            entry.time_seconds = (entry.time_seconds - duration).max(0.0);
        }
    }
}

fn find_track_mut<'a>(
    timeline: &'a mut Timeline,
    track_name: &str,
) -> Option<&'a mut awidat_proto::otio::Track> {
    timeline.tracks.children.iter_mut().find_map(|sc| match sc {
        StackChild::Track(track) if track.name == track_name => Some(track),
        _ => None,
    })
}

/// Sum the durations of children before `child_index` on the given
/// track. Used by the overlay broll path to compute the anchor's
/// track-time start so the broll on V2 lines up with the underlying
/// clip on V1.
fn track_time_at(timeline: &Timeline, track_index: usize, child_index: usize) -> f64 {
    let StackChild::Track(track) = &timeline.tracks.children[track_index] else {
        return 0.0;
    };
    let mut t = 0.0;
    for (i, child) in track.children.iter().enumerate() {
        if i >= child_index {
            break;
        }
        t += child_duration(child);
    }
    t
}

/// Total duration of a track's children. Used to know where to
/// append on the overlay track without recomputing per-element.
fn track_cursor(track: &awidat_proto::otio::Track) -> f64 {
    track.children.iter().map(child_duration).sum()
}

fn child_duration(child: &TrackChild) -> f64 {
    match child {
        TrackChild::Clip(c) => c
            .source_range
            .as_ref()
            .map(|r| r.duration.to_seconds())
            .unwrap_or(0.0),
        TrackChild::Gap(g) => g.source_range.duration.to_seconds(),
        // Transitions overlap their neighbors in source-time terms
        // (their `in_offset + out_offset` is the *visual* duration,
        // not extra timeline length). For the cursor math here we
        // treat them as zero — they don't push later clips later.
        TrackChild::Transition(_) => 0.0,
        // Nested stacks aren't produced anywhere in the awidat
        // pipeline today; treat as zero rather than panicking.
        TrackChild::Stack(_) => 0.0,
    }
}

/// Pick the next free `V<N>` name for a brand-new video track.
/// Walks existing track names that match `V<digits>` and returns the
/// max+1; falls back to `V2` if the parse misses (no `V1` in scope,
/// or all names are non-numeric).
fn next_video_track_name(timeline: &Timeline) -> String {
    let mut max_n: u32 = 1;
    for sc in &timeline.tracks.children {
        let StackChild::Track(t) = sc else { continue };
        if let Some(rest) = t.name.strip_prefix('V')
            && let Ok(n) = rest.parse::<u32>()
        {
            if n > max_n {
                max_n = n;
            }
        }
    }
    format!("V{}", max_n + 1)
}

/// Lightweight validate hook on Timeline. We reach into the proto crate's
/// validate machinery via [`awidat_proto::project::Project`] indirectly —
/// but here we just round-trip through serde to catch shape-level errors.
trait ValidateForApply {
    fn validate_for_test(&self) -> Result<(), String>;
}

impl ValidateForApply for Timeline {
    fn validate_for_test(&self) -> Result<(), String> {
        // The OTIO crate's typed validators ride on `Timeline::validate`;
        // it's pub(crate). The cheapest portable check: serialize +
        // deserialize, which exercises the shape-level invariants of
        // the entire tree.
        let s = serde_json::to_string(self).map_err(|e| e.to_string())?;
        let _: Timeline = serde_json::from_str(&s).map_err(|e| e.to_string())?;
        // Then walk: every clip must have a positive duration, every
        // track child non-negative duration. We do this manually since
        // Timeline::validate is pub(crate).
        let mut warnings = Vec::new();
        for sc in &self.tracks.children {
            let StackChild::Track(track) = sc else {
                continue;
            };
            for tc in &track.children {
                if let TrackChild::Clip(c) = tc
                    && let Some(r) = &c.source_range
                {
                    if r.duration.value < 0.0 {
                        warnings.push(format!("clip {:?} has negative duration", c.name));
                    }
                    if r.duration.rate <= 0.0 || r.start_time.rate <= 0.0 {
                        warnings.push(format!("clip {:?} has non-positive time rate", c.name));
                    }
                }
            }
        }
        let _ = HashSet::<()>::new(); // reserved for future dup-uuid detection
        if warnings.is_empty() {
            Ok(())
        } else {
            Err(warnings.join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
    use awidat_proto::otio::{
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
        Track, TrackChild, TrackKind,
    };

    fn timeline_with_three_clips() -> Timeline {
        let mut tl = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        for (i, snip) in ["alpha snippet", "bravo snippet", "charlie snippet"]
            .iter()
            .enumerate()
        {
            let mut c = Clip::empty(format!("clip-{i}"));
            c.media_reference =
                MediaReference::External(ExternalReference::new(format!("raw/{i}.mp4")));
            c.source_range = Some(TimeRange::new(
                RationalTime::new(0.0, 24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
            c.metadata = ClipMetadata {
                awidat: Some(AwidatClipMetadata {
                    anchor: Some(AwAnchor {
                        transcript_snippet: Some((*snip).to_string()),
                        ..AwAnchor::default()
                    }),
                    ..AwidatClipMetadata::default()
                }),
                ..ClipMetadata::default()
            };
            track.children.push(TrackChild::Clip(c));
        }
        tl.tracks.children.push(StackChild::Track(track));
        tl
    }

    #[test]
    fn apply_trim_shortens_clip() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                start: None,
                end: Some(3.0),
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 1);
        assert_eq!(
            outcome.applied[0].locator,
            Some(ClipLocator {
                track_index: 0,
                child_index: 1,
            }),
            "trim proposals need the resolved locator for diff hints and drag handles",
        );
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &t.children[1] else {
            panic!()
        };
        let r = c.source_range.as_ref().unwrap();
        assert!((r.duration.to_seconds() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn apply_trim_start_only_shortens_from_current_end() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-1".into(),
                },
                start: Some(1.0),
                end: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &t.children[1] else {
            panic!()
        };
        let r = c.source_range.as_ref().unwrap();
        assert!((r.start_time.to_seconds() - 1.0).abs() < 1e-9);
        assert!((r.duration.to_seconds() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn apply_trim_rejects_no_op_range() {
        let mut tl = timeline_with_three_clips();
        let StackChild::Track(t) = &mut tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &mut t.children[1] else {
            panic!()
        };
        c.source_range = Some(TimeRange::new(
            RationalTime::new(5.0 * 24.0, 24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));

        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-1".into(),
                },
                start: Some(5.0),
                end: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("no-op")),
            "got: {err:?}"
        );
    }

    #[test]
    fn apply_delete_removes_clip() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo".into(),
                },
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 1);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 2);
        // Remaining clips: alpha, charlie.
        let TrackChild::Clip(c1) = &t.children[1] else {
            panic!()
        };
        assert_eq!(c1.name, "clip-2");
    }

    #[test]
    fn apply_delete_removes_adjacent_transitions() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::InsertTransition {
                    between: super::super::op::TransitionBetween {
                        from: Anchor::TranscriptSnippet {
                            text: "alpha snippet".into(),
                        },
                        to: Anchor::TranscriptSnippet {
                            text: "bravo snippet".into(),
                        },
                    },
                    kind: "SMPTE_Dissolve".into(),
                    duration_s: 0.3,
                    spec: None,
                },
                EdlOp::InsertTransition {
                    between: super::super::op::TransitionBetween {
                        from: Anchor::TranscriptSnippet {
                            text: "bravo snippet".into(),
                        },
                        to: Anchor::TranscriptSnippet {
                            text: "charlie snippet".into(),
                        },
                    },
                    kind: "SMPTE_Dissolve".into(),
                    duration_s: 0.3,
                    spec: None,
                },
            ],
        };
        let (tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();

        let (new_tl, outcome) = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::DeleteClip {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                }],
            },
            &AnchorContext::empty(),
        )
        .unwrap();

        assert!(
            outcome.applied[0]
                .description
                .contains("removed 2 adjacent transition")
        );
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 2);
        assert!(
            t.children
                .iter()
                .all(|child| matches!(child, TrackChild::Clip(_)))
        );
    }

    #[test]
    fn apply_split_partitions_clip_at_timestamp() {
        let tl = timeline_with_three_clips();
        // bravo's source_range is [0, 5s]. Split at 2.0s into two
        // pieces: [0, 2s] and [2s, 5s].
        let env = EdlEnvelope {
            ops: vec![EdlOp::SplitClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo".into(),
                },
                at_s: 2.0,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 1);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // alpha, bravo (left), bravo-b (right), charlie. 4 clips.
        assert_eq!(t.children.len(), 4);
        let TrackChild::Clip(left) = &t.children[1] else {
            panic!()
        };
        let TrackChild::Clip(right) = &t.children[2] else {
            panic!()
        };
        assert_eq!(left.name, "clip-1");
        assert_eq!(right.name, "clip-1-b");
        let lr = left.source_range.as_ref().unwrap();
        let rr = right.source_range.as_ref().unwrap();
        assert!((lr.duration.to_seconds() - 2.0).abs() < 1e-9);
        assert!((rr.duration.to_seconds() - 3.0).abs() < 1e-9);
        assert!((rr.start_time.to_seconds() - 2.0).abs() < 1e-9);
    }

    fn extract_clip_uuid(clip: &Clip) -> Option<String> {
        clip.metadata
            .awidat
            .as_ref()
            .and_then(|m| m.extra.get("clip_uuid"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    #[test]
    fn apply_split_stamps_distinct_clip_uuids_on_each_piece() {
        // The right piece is built via clip.clone() — without
        // explicit re-stamping it would inherit the parent's
        // clip_uuid. That breaks Anchor::ClipUuid resolution because
        // two clips would share the same uuid. Verify the right
        // piece has a fresh uuid distinct from the left.
        let mut tl = timeline_with_three_clips();
        // Pre-stamp the parent so we can confirm the right piece
        // diverges from a known starting uuid.
        let StackChild::Track(t) = &mut tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &mut t.children[1] else {
            panic!()
        };
        c.metadata
            .awidat
            .get_or_insert_with(AwidatClipMetadata::default)
            .extra
            .insert(
                "clip_uuid".into(),
                serde_json::Value::String("parent-uuid".into()),
            );

        let env = EdlEnvelope {
            ops: vec![EdlOp::SplitClip {
                anchor: Anchor::ClipUuid {
                    uuid: "parent-uuid".into(),
                },
                at_s: 2.0,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(left) = &t.children[1] else {
            panic!()
        };
        let TrackChild::Clip(right) = &t.children[2] else {
            panic!()
        };
        let left_uuid = extract_clip_uuid(left).unwrap();
        let right_uuid = extract_clip_uuid(right).unwrap();
        assert_eq!(left_uuid, "parent-uuid");
        assert_ne!(left_uuid, right_uuid);
        assert!(right_uuid.starts_with("c-"));
        assert!(outcome.applied[0].description.contains(&right_uuid));
        assert!(outcome.applied[0].description.contains("anchor=clip_uuid="));
    }

    #[test]
    fn apply_insert_clip_stamps_fresh_clip_uuid() {
        let mut tl = Timeline::empty("test");
        let mut track =
            awidat_proto::otio::Track::empty("V1", awidat_proto::otio::TrackKind::Video);
        // No existing clip — the insert must generate its own anchor.
        track
            .children
            .push(TrackChild::Gap(awidat_proto::otio::Gap::of_duration(
                0.0, 24.0,
            )));
        tl.tracks.children.push(StackChild::Track(track));

        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/x.mp4".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: None,
                start: Some(0.0),
                end: Some(3.0),
                name: None,
                link_group_id: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(inserted) = &t.children[1] else {
            panic!()
        };
        let uuid = extract_clip_uuid(inserted).expect("insert should stamp a clip_uuid");
        assert!(uuid.starts_with("c-"), "got: {uuid}");
    }

    #[test]
    fn apply_split_rejects_at_s_outside_range() {
        let tl = timeline_with_three_clips();
        // bravo's range is [0, 5s]. Splitting at 7s is outside.
        let env = EdlEnvelope {
            ops: vec![EdlOp::SplitClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo".into(),
                },
                at_s: 7.0,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        match err {
            ApplyError::Invalid { index, message } => {
                assert_eq!(index, 0);
                assert!(
                    message.contains("must lie strictly inside"),
                    "got: {message}"
                );
            }
            other => panic!("want Invalid, got {other:?}"),
        }
    }

    #[test]
    fn split_then_delete_in_one_envelope_cuts_middle_out() {
        // The headline editorial flow: cut a phrase out of the middle
        // of a clip by splitting twice and deleting the middle piece.
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SplitClip {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo".into(),
                    },
                    at_s: 2.0,
                },
                // After the first split, bravo-b ([2s, 5s]) is the
                // right piece. Split it again at 4s to isolate the
                // middle [2s, 4s] chunk.
                EdlOp::SplitClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1-b".into(),
                    },
                    at_s: 4.0,
                },
                // Delete the now-isolated middle.
                EdlOp::DeleteClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1-b".into(),
                    },
                },
            ],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 3);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // alpha, bravo[0..2], bravo-b-b[4..5], charlie = 4 clips.
        assert_eq!(t.children.len(), 4);
    }

    #[test]
    fn apply_untrim_widens_after_a_trim() {
        // Trim bravo to [0, 2s], then Untrim back to its original [0, 5s].
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::TrimClip {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo".into(),
                    },
                    start: None,
                    end: Some(2.0),
                },
                EdlOp::UntrimClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    start: None,
                    end: Some(5.0),
                },
            ],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 2);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &t.children[1] else {
            panic!()
        };
        let r = c.source_range.as_ref().unwrap();
        assert!((r.duration.to_seconds() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn apply_untrim_refuses_to_narrow() {
        // bravo's source range starts at [0, 5s]. Try to "untrim" to
        // [0, 3s] — that's narrowing, which Untrim refuses.
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::UntrimClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo".into(),
                },
                start: None,
                end: Some(3.0),
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        match err {
            ApplyError::Invalid { message, .. } => {
                assert!(
                    message.contains("Untrim only widens"),
                    "expected 'Untrim only widens' hint; got: {message}"
                );
            }
            other => panic!("want Invalid, got {other:?}"),
        }
    }

    #[test]
    fn apply_untrim_caps_to_available_range_when_known() {
        // Build a single-clip timeline whose external reference declares
        // available_range = [0, 4s]. Trim it to [0, 2s], then Untrim
        // asking for [0, 999s]. Result should cap at 4s.
        use awidat_proto::otio::{
            Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild,
            TimeRange, Timeline as Tl, Track, TrackChild, TrackKind,
        };
        let mut tl = Tl::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-0".to_string());
        let mut ext = ExternalReference::new("raw/x.mp4");
        ext.available_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(4.0 * 24.0, 24.0),
        ));
        clip.media_reference = MediaReference::External(ext);
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));
        clip.metadata = ClipMetadata::default();
        track.children.push(TrackChild::Clip(clip));
        tl.tracks.children.push(StackChild::Track(track));

        let env = EdlEnvelope {
            ops: vec![EdlOp::UntrimClip {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-0".into(),
                },
                start: None,
                end: Some(999.0),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &t.children[0] else {
            panic!()
        };
        let r = c.source_range.as_ref().unwrap();
        // Capped to available_range end (4s).
        assert!(
            (r.duration.to_seconds() - 4.0).abs() < 1e-9,
            "expected duration capped to 4s; got {}",
            r.duration.to_seconds()
        );
    }

    /// Regression test for a real bug found in a live run: untrim with
    /// only `end` specified used to default `start` to 0.0, wiping
    /// the existing trimmed-in point. Now omitted fields preserve
    /// the current value (matching apply_trim's contract).
    #[test]
    fn apply_untrim_preserves_start_when_only_end_specified() {
        // Build a clip with source_range = [10s, 20s] (10s duration
        // starting at 10s into the source media). Untrim with end=30
        // and start=None should produce [10s, 30s] — NOT [0s, 30s].
        let mut tl = awidat_proto::otio::Timeline::empty("preserve");
        let mut track =
            awidat_proto::otio::Track::empty("V1", awidat_proto::otio::TrackKind::Video);
        let mut clip = awidat_proto::otio::Clip::empty("clip-0".to_string());
        clip.media_reference = awidat_proto::otio::MediaReference::External(
            awidat_proto::otio::ExternalReference::new("raw/x.mp4"),
        );
        clip.source_range = Some(awidat_proto::otio::TimeRange::new(
            awidat_proto::otio::RationalTime::new(10.0 * 24.0, 24.0),
            awidat_proto::otio::RationalTime::new(10.0 * 24.0, 24.0),
        ));
        track
            .children
            .push(awidat_proto::otio::TrackChild::Clip(clip));
        tl.tracks
            .children
            .push(awidat_proto::otio::StackChild::Track(track));

        let env = EdlEnvelope {
            ops: vec![EdlOp::UntrimClip {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-0".into(),
                },
                start: None,
                end: Some(30.0),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &t.children[0] else {
            panic!()
        };
        let r = c.source_range.as_ref().unwrap();
        assert!(
            (r.start_time.to_seconds() - 10.0).abs() < 1e-9,
            "start should be preserved at 10.0s, got {}",
            r.start_time.to_seconds()
        );
        assert!(
            (r.duration.to_seconds() - 20.0).abs() < 1e-9,
            "duration should be 30-10=20s, got {}",
            r.duration.to_seconds()
        );
    }

    #[test]
    fn apply_insert_clip_creates_track_when_missing() {
        // Empty timeline. Insert one clip — track gets created.
        use awidat_proto::otio::Timeline as Tl;
        let tl = Tl::empty("test");
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/clip-1.MOV".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: None,
                start: Some(0.0),
                end: Some(56.47),
                name: None,
                link_group_id: None,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 1);
        assert_eq!(new_tl.tracks.children.len(), 1, "track created");
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.name, "V1");
        assert_eq!(t.children.len(), 1);
        let TrackChild::Clip(c) = &t.children[0] else {
            panic!()
        };
        assert_eq!(c.name, "clip-0");
        let r = c.source_range.as_ref().unwrap();
        assert!((r.duration.to_seconds() - 56.47).abs() < 1e-6);
    }

    #[test]
    fn apply_insert_clip_appends_to_existing_track() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/extra.mp4".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: None,
                start: Some(0.0),
                end: Some(2.0),
                name: Some("intro".into()),
                link_group_id: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // 3 existing + 1 inserted at the end.
        assert_eq!(t.children.len(), 4);
        let TrackChild::Clip(c) = &t.children[3] else {
            panic!()
        };
        assert_eq!(c.name, "intro");
    }

    #[test]
    fn apply_insert_clip_at_position_inserts_in_middle() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/middle.mp4".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: Some(1),
                start: Some(0.0),
                end: Some(1.0),
                name: Some("inserted".into()),
                link_group_id: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 4);
        let TrackChild::Clip(c) = &t.children[1] else {
            panic!()
        };
        assert_eq!(c.name, "inserted", "new clip lands at index 1");
    }

    #[test]
    fn apply_insert_clip_default_name_avoids_duplicate_when_inserted_in_middle() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/middle.mp4".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: Some(1),
                start: Some(0.0),
                end: Some(1.0),
                name: None,
                link_group_id: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let names = t
            .children
            .iter()
            .filter_map(|tc| match tc {
                TrackChild::Clip(c) => Some(c.name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["clip-0", "clip-3", "clip-1", "clip-2"]);
    }

    #[test]
    fn apply_linked_audio_insert_pads_to_video_start() {
        use awidat_proto::otio::{
            Clip, ExternalReference, MediaReference, RationalTime, TimeRange,
        };

        let mut tl = awidat_proto::otio::Timeline::empty("test");
        let mut v1 = awidat_proto::otio::Track::empty("Video 1", TrackKind::Video);
        let mut first = Clip::empty("first");
        first.media_reference = MediaReference::External(ExternalReference::new("raw/first.mp4"));
        first.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(48.0, 24.0),
        ));
        v1.children.push(TrackChild::Clip(first));
        tl.tracks.children.push(StackChild::Track(v1));

        let env = EdlEnvelope {
            ops: vec![
                EdlOp::InsertClip {
                    asset: "raw/second.mp4".into(),
                    track: "Video 1".into(),
                    track_kind: Some(InsertTrackKind::Video),
                    at_position: None,
                    start: Some(0.0),
                    end: Some(5.0),
                    name: None,
                    link_group_id: Some("lg-1".into()),
                },
                EdlOp::InsertClip {
                    asset: "raw/second.mp4".into(),
                    track: "A1".into(),
                    track_kind: Some(InsertTrackKind::Audio),
                    at_position: None,
                    start: Some(0.0),
                    end: Some(5.0),
                    name: None,
                    link_group_id: Some("lg-1".into()),
                },
            ],
        };

        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(a1) = &new_tl.tracks.children[1] else {
            panic!()
        };
        assert!(matches!(a1.children[0], TrackChild::Gap(_)));
        assert!((child_duration(&a1.children[0]) - 2.0).abs() < 0.001);
        assert!(matches!(a1.children[1], TrackChild::Clip(_)));
    }

    #[test]
    fn apply_insert_clip_rejects_zero_duration() {
        use awidat_proto::otio::Timeline as Tl;
        let tl = Tl::empty("test");
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/x.mp4".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: None,
                start: Some(5.0),
                end: Some(5.0),
                name: None,
                link_group_id: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        match err {
            ApplyError::Invalid { message, .. } => {
                assert!(message.contains("must be > start"), "got: {message}");
            }
            other => panic!("want Invalid, got {other:?}"),
        }
    }

    #[test]
    fn apply_insert_clip_can_create_audio_track() {
        use awidat_proto::otio::Timeline as Tl;
        let tl = Tl::empty("test");
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/dialogue.wav".into(),
                track: "A1".into(),
                track_kind: Some(InsertTrackKind::Audio),
                at_position: None,
                start: Some(0.0),
                end: Some(10.0),
                name: None,
                link_group_id: Some("lg-test".into()),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert!(matches!(t.kind, TrackKind::Audio));
        let TrackChild::Clip(c) = &t.children[0] else {
            panic!()
        };
        let link = c
            .metadata
            .awidat
            .as_ref()
            .and_then(|m| m.extra.get("link_group_id"))
            .and_then(|v| v.as_str());
        assert_eq!(link, Some("lg-test"));
    }

    #[test]
    fn apply_audio_fade_and_track_audio_replace_metadata() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetAudioFade {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-0".into(),
                    },
                    fade_in_s: Some(0.25),
                    fade_out_s: Some(0.5),
                },
                EdlOp::SetTrackAudio {
                    track: "V1".into(),
                    role: Some("dialogue".into()),
                    volume: Some(0.8),
                    muted: Some(false),
                    solo: Some(true),
                },
            ],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            err.to_string().contains("is not an audio track"),
            "expected video-track rejection, got {err}"
        );

        let mut audio_tl = awidat_proto::otio::Timeline::empty("audio");
        let mut track = awidat_proto::otio::Track::empty("A1", TrackKind::Audio);
        let mut clip = awidat_proto::otio::Clip::empty("clip-0");
        clip.media_reference = awidat_proto::otio::MediaReference::External(
            awidat_proto::otio::ExternalReference::new("raw/a.wav"),
        );
        clip.source_range = Some(awidat_proto::otio::TimeRange::new(
            awidat_proto::otio::RationalTime::zero(24.0),
            awidat_proto::otio::RationalTime::new(10.0 * 24.0, 24.0),
        ));
        stamp_fresh_clip_uuid(&mut clip);
        track.children.push(TrackChild::Clip(clip));
        audio_tl.tracks.children.push(StackChild::Track(track));
        let uuid = match &audio_tl.tracks.children[0] {
            StackChild::Track(t) => match &t.children[0] {
                TrackChild::Clip(c) => extract_clip_uuid(c).unwrap(),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetAudioFade {
                    anchor: Anchor::ClipUuid { uuid },
                    fade_in_s: Some(0.25),
                    fade_out_s: Some(0.5),
                },
                EdlOp::SetTrackAudio {
                    track: "A1".into(),
                    role: Some("dialogue".into()),
                    volume: Some(0.8),
                    muted: Some(false),
                    solo: Some(true),
                },
            ],
        };
        let (new_tl, _) = apply(&audio_tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.metadata["awidat_audio"]["volume"].as_f64().unwrap(), 0.8);
        let TrackChild::Clip(c) = &t.children[0] else {
            panic!()
        };
        let fade = c
            .effects
            .iter()
            .find(|e| e.effect_name == AUDIO_FADE_EFFECT_NAME)
            .unwrap();
        assert_eq!(fade.metadata["fade_in_s"].as_f64(), Some(0.25));
        assert_eq!(fade.metadata["fade_out_s"].as_f64(), Some(0.5));
    }

    #[test]
    fn apply_anchor_miss_is_anchor_miss_error() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "no such snippet".into(),
                },
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        match err {
            ApplyError::AnchorMiss { index, .. } => assert_eq!(index, 0),
            other => panic!("want AnchorMiss, got {other:?}"),
        }
    }

    #[test]
    fn apply_does_not_mutate_input_on_error() {
        let tl = timeline_with_three_clips();
        let snapshot = serde_json::to_string(&tl).unwrap();
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "missing".into(),
                },
            }],
        };
        let _ = apply(&tl, &env, &AnchorContext::empty());
        let after = serde_json::to_string(&tl).unwrap();
        assert_eq!(snapshot, after);
    }

    #[test]
    fn apply_multi_op_envelope_in_order() {
        // Trim alpha, then delete charlie. Both anchors resolve against
        // the post-prior-op state implicitly (re-resolution per op).
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::TrimClip {
                    anchor: Anchor::TranscriptSnippet {
                        text: "alpha".into(),
                    },
                    start: None,
                    end: Some(2.0),
                },
                EdlOp::DeleteClip {
                    anchor: Anchor::TranscriptSnippet {
                        text: "charlie".into(),
                    },
                },
            ],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 2);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // Alpha trimmed, bravo unchanged, charlie gone.
        assert_eq!(t.children.len(), 2);
        let TrackChild::Clip(alpha) = &t.children[0] else {
            panic!()
        };
        assert!((alpha.source_range.as_ref().unwrap().duration.to_seconds() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn trim_with_end_less_than_start_is_invalid() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "alpha".into(),
                },
                start: Some(3.0),
                end: Some(1.0),
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { message, .. } if message.contains("must be >= start"))
        );
    }

    #[test]
    fn apply_insert_broll_replace_swaps_in_place_with_tail() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertBRoll {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                asset: "raw/broll.mp4".into(),
                duration_s: 2.0,
                position: super::super::op::BRollPosition::Replace,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // 3 originals → 4 children (broll inserted in place of bravo,
        // plus the bravo tail).
        assert_eq!(t.children.len(), 4);
        let TrackChild::Clip(broll) = &t.children[1] else {
            panic!("expected broll at idx 1, got {:?}", t.children[1])
        };
        assert!(broll.name.starts_with("broll-from-clip-1"));
        assert_eq!(
            broll
                .source_range
                .as_ref()
                .map(|r| r.duration.to_seconds())
                .unwrap_or(0.0),
            2.0,
        );
        let TrackChild::Clip(tail) = &t.children[2] else {
            panic!("expected tail at idx 2, got {:?}", t.children[2])
        };
        assert!(tail.name.starts_with("clip-1-tail"));
        // tail.source_range = [2.0, 5.0] → start 2s, duration 3s.
        let r = tail.source_range.as_ref().unwrap();
        assert!((r.start_time.to_seconds() - 2.0).abs() < 1e-9);
        assert!((r.duration.to_seconds() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn apply_insert_broll_replace_consumes_anchor_when_duration_meets_or_exceeds() {
        let tl = timeline_with_three_clips();
        // Anchor source duration is 5s; broll for 5s → no tail.
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertBRoll {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                asset: "raw/broll.mp4".into(),
                duration_s: 5.0,
                position: super::super::op::BRollPosition::Replace,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 3); // 3 originals, bravo replaced by broll.
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name.starts_with("broll-from")));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-2"));
    }

    #[test]
    fn apply_insert_broll_overlay_creates_v2_track_with_padding_gap() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertBRoll {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                asset: "raw/broll.mp4".into(),
                duration_s: 2.0,
                position: super::super::op::BRollPosition::Overlay,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        // V1 is unchanged.
        let StackChild::Track(v1) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(v1.children.len(), 3);
        assert_eq!(v1.name, "V1");
        // V2 was created with a 5s Gap + the broll clip (anchor's
        // track-time start = 5s).
        assert_eq!(new_tl.tracks.children.len(), 2);
        let StackChild::Track(v2) = &new_tl.tracks.children[1] else {
            panic!()
        };
        assert_eq!(v2.name, "V2");
        assert_eq!(v2.children.len(), 2);
        let TrackChild::Gap(g) = &v2.children[0] else {
            panic!("expected gap at v2 idx 0")
        };
        assert!((g.source_range.duration.to_seconds() - 5.0).abs() < 1e-9);
        let TrackChild::Clip(broll) = &v2.children[1] else {
            panic!("expected broll at v2 idx 1")
        };
        assert!(broll.name.starts_with("broll-from-clip-1"));
    }

    #[test]
    fn apply_insert_pip_creates_overlay_track_and_stamps_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertPiP {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                asset: "raw/pip.mp4".into(),
                duration_s: 2.0,
                source_start_s: 1.0,
                corner: super::super::op::PiPCorner::BottomRight,
                scale: 0.28,
                margin_pct: 0.035,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(v2) = &new_tl.tracks.children[1] else {
            panic!()
        };
        assert_eq!(v2.name, "V2");
        assert_eq!(v2.children.len(), 2);
        let TrackChild::Gap(g) = &v2.children[0] else {
            panic!("expected gap at v2 idx 0")
        };
        assert!((g.source_range.duration.to_seconds() - 5.0).abs() < 1e-9);
        let TrackChild::Clip(pip) = &v2.children[1] else {
            panic!("expected pip clip")
        };
        let range = pip.source_range.as_ref().unwrap();
        assert!((range.start_time.to_seconds() - 1.0).abs() < 1e-9);
        assert!((range.duration.to_seconds() - 2.0).abs() < 1e-9);
        let effect = pip
            .effects
            .iter()
            .find(|e| e.effect_name == VIDEO_OVERLAY_EFFECT_NAME)
            .expect("video overlay effect");
        assert_eq!(
            effect.metadata.get("mode").and_then(|v| v.as_str()),
            Some("pip")
        );
        assert_eq!(
            effect.metadata.get("corner").and_then(|v| v.as_str()),
            Some("bottom_right")
        );
    }

    #[test]
    fn apply_insert_broll_rejects_zero_duration() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertBRoll {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                asset: "raw/broll.mp4".into(),
                duration_s: 0.0,
                position: super::super::op::BRollPosition::Replace,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("must be > 0")),
            "expected duration error, got {err:?}",
        );
    }

    #[test]
    fn apply_insert_transition_lands_between_adjacent_clips() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 1.0,
                spec: None,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 1);

        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // 3 clips + 1 transition = 4 children; transition sits at index 1.
        assert_eq!(t.children.len(), 4);
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-0"));
        let TrackChild::Transition(tr) = &t.children[1] else {
            panic!("expected transition at index 1, got {:?}", t.children[1])
        };
        assert_eq!(tr.transition_type, "SMPTE_Dissolve");
        // symmetric: half the duration on each side.
        assert!((tr.in_offset.to_seconds() - 0.5).abs() < 1e-9);
        assert!((tr.out_offset.to_seconds() - 0.5).abs() < 1e-9);
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-1"));
    }

    #[test]
    fn apply_insert_transition_persists_semantic_metadata() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "awidat.slide_left".into(),
                duration_s: 0.28,
                spec: Some(awidat_proto::transitions::SemanticTransitionSpec {
                    id: "awidat.slide_left".into(),
                    family: Some("slide".into()),
                    intent: Some("hide_motion_jump".into()),
                    energy: Some(0.7),
                    direction: Some("left".into()),
                    params: serde_json::Map::new(),
                }),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Transition(tr) = &t.children[1] else {
            panic!("expected transition")
        };
        assert_eq!(tr.transition_type, "awidat.slide_left");
        let meta = tr
            .metadata
            .get("awidat_transition")
            .and_then(serde_json::Value::as_object)
            .unwrap();
        assert_eq!(
            meta.get("id").and_then(|v| v.as_str()),
            Some("awidat.slide_left")
        );
        assert_eq!(
            meta.get("intent").and_then(|v| v.as_str()),
            Some("hide_motion_jump")
        );
    }

    #[test]
    fn apply_insert_transition_rejects_non_adjacent_anchors() {
        let tl = timeline_with_three_clips();
        // "alpha" (idx 0) and "charlie" (idx 2) are 2 apart, not adjacent.
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "charlie snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 1.0,
                spec: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("not adjacent")),
            "expected adjacency error, got {err:?}",
        );
    }

    #[test]
    fn apply_insert_transition_rejects_anchors_on_different_tracks() {
        let mut tl = timeline_with_three_clips();
        // Add a second track with one clip whose snippet "delta snippet"
        // resolves on the new track only.
        let mut track2 = Track::empty("V2", TrackKind::Video);
        let mut c = Clip::empty("delta-clip".to_string());
        c.media_reference =
            MediaReference::External(ExternalReference::new("raw/delta.mp4".to_string()));
        c.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));
        c.metadata = ClipMetadata {
            awidat: Some(AwidatClipMetadata {
                anchor: Some(AwAnchor {
                    transcript_snippet: Some("delta snippet".to_string()),
                    ..AwAnchor::default()
                }),
                ..AwidatClipMetadata::default()
            }),
            ..ClipMetadata::default()
        };
        track2.children.push(TrackChild::Clip(c));
        tl.tracks.children.push(StackChild::Track(track2));

        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "delta snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 1.0,
                spec: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("different tracks")),
            "expected cross-track error, got {err:?}",
        );
    }

    #[test]
    fn apply_insert_transition_rejects_zero_duration() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 0.0,
                spec: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("must be > 0")),
            "expected duration error, got {err:?}",
        );
    }

    #[test]
    fn apply_move_clip_to_start_reorders_track() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::MoveClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "charlie snippet".into(),
                },
                to_position: 0,
                at_s: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-2"));
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-0"));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-1"));
    }

    #[test]
    fn apply_move_clip_to_end_clamps_to_last_position() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::MoveClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "alpha snippet".into(),
                },
                to_position: 99, // way past the end — clamp.
                at_s: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // alpha (was at 0) should now be at 2 (last position).
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-1"));
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-2"));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-0"));
    }

    #[test]
    fn apply_move_clip_same_position_is_a_no_op() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::MoveClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                to_position: 1, // already there.
                at_s: None,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0]
                .description
                .contains("already at position"),
            "expected no-op description, got {:?}",
            outcome.applied[0].description,
        );
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-0"));
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-1"));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-2"));
    }

    #[test]
    fn apply_move_clip_to_time_leaves_gap_and_places_at_target() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::MoveClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "alpha snippet".into(),
                },
                to_position: 0,
                at_s: Some(20.0),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert!(matches!(&t.children[0], TrackChild::Gap(_)));
        assert!((child_duration(&t.children[0]) - 5.0).abs() < 0.001);
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-1"));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-2"));
        assert!(matches!(&t.children[3], TrackChild::Gap(_)));
        assert!((child_duration(&t.children[3]) - 5.0).abs() < 0.001);
        assert!(matches!(&t.children[4], TrackChild::Clip(c) if c.name == "clip-0"));
    }

    #[test]
    fn apply_insert_transition_anchor_miss_surfaces_clearly() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "no such phrase here".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 1.0,
                spec: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::AnchorMiss { .. }),
            "expected anchor miss, got something else",
        );
    }

    #[test]
    fn apply_set_volume_stamps_effect_with_value() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetVolume {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                value: 0.5,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        assert_eq!(clip.effects[0].effect_name, "awidat.volume");
        let v = clip.effects[0]
            .metadata
            .get("value")
            .and_then(|v| v.as_f64())
            .unwrap();
        assert!((v - 0.5).abs() < 1e-9);
    }

    #[test]
    fn apply_set_volume_replaces_existing_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetVolume {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    value: 0.5,
                },
                EdlOp::SetVolume {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    value: 0.8,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        let v = clip.effects[0]
            .metadata
            .get("value")
            .and_then(|v| v.as_f64())
            .unwrap();
        assert!((v - 0.8).abs() < 1e-9);
    }

    #[test]
    fn apply_set_volume_rejects_negative_value() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetVolume {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                value: -0.1,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains(">= 0.0")),
            "expected validation error, got {err:?}",
        );
    }

    #[test]
    fn apply_set_effect_stamps_validated_metadata() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "awidat.color_correction".into(),
                params: serde_json::Map::from_iter([
                    ("contrast".into(), serde_json::json!(1.15)),
                    ("saturation".into(), serde_json::json!(0.9)),
                ]),
                rationale: Some("subtle correction for flat camera angle".into()),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        assert_eq!(clip.effects[0].effect_name, "awidat.color_correction");
        assert_eq!(clip.effects[0].name, "Color Correction");
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("contrast")
                .and_then(|v| v.as_f64()),
            Some(1.15)
        );
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("rationale")
                .and_then(|v| v.as_str()),
            Some("subtle correction for flat camera angle")
        );
    }

    #[test]
    fn apply_set_effect_replaces_same_id_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetEffect {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    effect: "awidat.color_correction".into(),
                    params: serde_json::Map::from_iter([(
                        "contrast".into(),
                        serde_json::json!(1.15),
                    )]),
                    rationale: None,
                },
                EdlOp::SetEffect {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    effect: "awidat.color_correction".into(),
                    params: serde_json::Map::from_iter([(
                        "saturation".into(),
                        serde_json::json!(0.8),
                    )]),
                    rationale: None,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        let color_effects: Vec<_> = clip
            .effects
            .iter()
            .filter(|effect| effect.effect_name == "awidat.color_correction")
            .collect();
        assert_eq!(color_effects.len(), 1);
        assert!(!color_effects[0].metadata.contains_key("contrast"));
        assert_eq!(
            color_effects[0]
                .metadata
                .get("saturation")
                .and_then(|v| v.as_f64()),
            Some(0.8)
        );
    }

    #[test]
    fn apply_set_effect_preserves_different_effect_ids() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetEffect {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    effect: "awidat.volume".into(),
                    params: serde_json::Map::from_iter([("value".into(), serde_json::json!(0.75))]),
                    rationale: None,
                },
                EdlOp::SetEffect {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    effect: "awidat.color_correction".into(),
                    params: serde_json::Map::from_iter([(
                        "contrast".into(),
                        serde_json::json!(1.1),
                    )]),
                    rationale: None,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert!(
            clip.effects
                .iter()
                .any(|e| e.effect_name == "awidat.volume")
        );
        assert!(
            clip.effects
                .iter()
                .any(|e| e.effect_name == "awidat.color_correction")
        );
    }

    #[test]
    fn apply_set_effect_rejects_unknown_effect_before_writing() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "awidat.nope".into(),
                params: serde_json::Map::new(),
                rationale: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("unknown effect id")),
            "expected unknown effect validation error, got {err:?}"
        );
        let StackChild::Track(t) = &tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert!(clip.effects.is_empty());
    }

    #[test]
    fn apply_set_effect_rejects_out_of_range_params_before_writing() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "awidat.color_correction".into(),
                params: serde_json::Map::from_iter([("contrast".into(), serde_json::json!(4.0))]),
                rationale: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("contrast")),
            "expected range validation error, got {err:?}"
        );
        let StackChild::Track(t) = &tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert!(clip.effects.is_empty());
    }

    #[test]
    fn apply_set_speed_stamps_effect_with_factor() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetSpeed {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                factor: 2.0,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        assert_eq!(clip.effects[0].effect_name, "awidat.speed");
        let f = clip.effects[0]
            .metadata
            .get("factor")
            .and_then(|v| v.as_f64())
            .unwrap();
        assert!((f - 2.0).abs() < 1e-9);
    }

    #[test]
    fn apply_set_speed_rejects_zero_factor() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetSpeed {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                factor: 0.0,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("> 0.0")),
            "expected validation error, got {err:?}",
        );
    }

    #[test]
    fn apply_set_color_correction_stamps_effect_with_fields() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetColorCorrection {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                exposure_ev: Some(0.25),
                contrast: Some(1.1),
                saturation: None,
                temperature: Some(-0.2),
                tint: None,
                shadows: None,
                highlights: Some(-0.3),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        assert_eq!(clip.effects[0].effect_name, "awidat.color_correction");
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("exposure_ev")
                .and_then(|v| v.as_f64()),
            Some(0.25)
        );
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("temperature")
                .and_then(|v| v.as_f64()),
            Some(-0.2)
        );
        assert!(!clip.effects[0].metadata.contains_key("saturation"));
    }

    #[test]
    fn apply_set_color_correction_replaces_existing_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetColorCorrection {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    exposure_ev: Some(0.25),
                    contrast: None,
                    saturation: None,
                    temperature: None,
                    tint: None,
                    shadows: None,
                    highlights: None,
                },
                EdlOp::SetColorCorrection {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    exposure_ev: None,
                    contrast: Some(0.9),
                    saturation: Some(1.2),
                    temperature: None,
                    tint: None,
                    shadows: None,
                    highlights: None,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        let color_effects: Vec<_> = clip
            .effects
            .iter()
            .filter(|effect| effect.effect_name == "awidat.color_correction")
            .collect();
        assert_eq!(color_effects.len(), 1);
        assert!(!color_effects[0].metadata.contains_key("exposure_ev"));
        assert_eq!(
            color_effects[0]
                .metadata
                .get("saturation")
                .and_then(|v| v.as_f64()),
            Some(1.2)
        );
    }

    #[test]
    fn apply_set_color_correction_rejects_invalid_field_value() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetColorCorrection {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                exposure_ev: Some(8.0),
                contrast: None,
                saturation: None,
                temperature: None,
                tint: None,
                shadows: None,
                highlights: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("exposure_ev")),
            "expected validation error, got {err:?}",
        );
    }

    #[test]
    fn apply_lut_stamps_effect_and_rejects_unsafe_paths() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::ApplyLut {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                lut_path: "luts/show-look.cube".into(),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        assert_eq!(clip.effects[0].effect_name, "awidat.lut");
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("lut_path")
                .and_then(|v| v.as_str()),
            Some("luts/show-look.cube")
        );

        let err = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::ApplyLut {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    lut_path: "../secret.cube".into(),
                }],
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("project-relative")),
            "expected unsafe path validation error, got {err:?}",
        );
    }

    #[test]
    fn apply_insert_title_creates_titles_track_and_stamps_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTitle {
                start_s: 0.0,
                end_s: 3.0,
                text: "Welcome".into(),
                position: super::super::op::TitlePosition::Top,
                font_size: 72,
                color: "#FFAA00".into(),
                font_weight: super::super::op::TitleWeight::Bold,
                animation: super::super::op::TitleAnimation::FadeInOut,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        // V1 (3 clips) + Titles track.
        assert_eq!(new_tl.tracks.children.len(), 2);
        let StackChild::Track(titles) = &new_tl.tracks.children[1] else {
            panic!("expected titles track at index 1")
        };
        assert_eq!(titles.name, "Titles");
        assert_eq!(
            titles
                .metadata
                .get("awidat_track_role")
                .and_then(|v| v.as_str()),
            Some("titles"),
        );
        assert_eq!(titles.children.len(), 1);
        let TrackChild::Clip(title_clip) = &titles.children[0] else {
            panic!("expected title clip on titles track")
        };
        assert_eq!(title_clip.effects.len(), 1);
        let effect = &title_clip.effects[0];
        assert_eq!(effect.effect_name, "awidat.title");
        assert_eq!(
            effect.metadata.get("text").and_then(|v| v.as_str()),
            Some("Welcome"),
        );
        assert_eq!(
            effect.metadata.get("position").and_then(|v| v.as_str()),
            Some("top"),
        );
        assert_eq!(
            effect.metadata.get("font_weight").and_then(|v| v.as_str()),
            Some("bold"),
        );
        assert_eq!(
            effect.metadata.get("animation").and_then(|v| v.as_str()),
            Some("fade_in_out"),
        );
        assert_eq!(
            effect.metadata.get("font_size").and_then(|v| v.as_u64()),
            Some(72),
        );
    }

    #[test]
    fn apply_insert_caption_creates_caption_overlay_node() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertCaption {
                start_s: 1.0,
                end_s: 2.5,
                text: "This changed everything".into(),
                position: super::super::op::TitlePosition::Bottom,
                font_size: 52,
                color: "#FFFFFF".into(),
                safe_area: "mobile".into(),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(titles) = &new_tl.tracks.children[1] else {
            panic!("expected titles track")
        };
        let TrackChild::Clip(caption_clip) = &titles.children[0] else {
            panic!("expected caption clip")
        };
        assert!(caption_clip.name.starts_with("caption-"));
        let effect = &caption_clip.effects[0];
        assert_eq!(effect.effect_name, "awidat.title");
        assert_eq!(
            effect.metadata.get("role").and_then(|v| v.as_str()),
            Some("caption"),
        );
        assert_eq!(
            effect.metadata.get("safe_area").and_then(|v| v.as_str()),
            Some("mobile"),
        );
        assert_eq!(
            effect.metadata.get("position").and_then(|v| v.as_str()),
            Some("bottom"),
        );
    }

    #[test]
    fn apply_output_loudness_and_package_metadata_live_on_timeline_graph() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetOutputFormat {
                    aspect_ratio: "9:16".into(),
                    platform: Some("youtube_shorts".into()),
                    safe_area: Some("mobile".into()),
                },
                EdlOp::SetLoudnessTarget {
                    integrated_lufs: -16.0,
                    true_peak_db: Some(-1.0),
                },
                EdlOp::SetPackageMetadata {
                    platform: Some("youtube_shorts".into()),
                    title: Some("Launch Risk".into()),
                    description: Some("A short clip about launch risk".into()),
                    tags: Some("launch,risk,clip".into()),
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let extra = &new_tl
            .metadata
            .awidat
            .as_ref()
            .expect("awidat metadata")
            .extra;
        assert_eq!(
            extra
                .get("output_format")
                .and_then(|v| v.get("aspect_ratio"))
                .and_then(|v| v.as_str()),
            Some("9:16"),
        );
        assert_eq!(
            extra
                .get("loudness_target")
                .and_then(|v| v.get("integrated_lufs"))
                .and_then(|v| v.as_f64()),
            Some(-16.0),
        );
        assert_eq!(
            extra
                .get("package_metadata")
                .and_then(|v| v.get("title"))
                .and_then(|v| v.as_str()),
            Some("Launch Risk"),
        );
    }

    #[test]
    fn apply_insert_title_reuses_existing_titles_track() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::InsertTitle {
                    start_s: 0.0,
                    end_s: 3.0,
                    text: "First".into(),
                    position: super::super::op::TitlePosition::Top,
                    font_size: 64,
                    color: "#FFFFFF".into(),
                    font_weight: super::super::op::TitleWeight::Normal,
                    animation: super::super::op::TitleAnimation::None,
                },
                EdlOp::InsertTitle {
                    start_s: 5.0,
                    end_s: 8.0,
                    text: "Second".into(),
                    position: super::super::op::TitlePosition::Bottom,
                    font_size: 48,
                    color: "#FFFFFF".into(),
                    font_weight: super::super::op::TitleWeight::Normal,
                    animation: super::super::op::TitleAnimation::None,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        // Still only 2 tracks (V1 + Titles), with two titles on the
        // Titles track in start_s order.
        assert_eq!(new_tl.tracks.children.len(), 2);
        let StackChild::Track(titles) = &new_tl.tracks.children[1] else {
            panic!()
        };
        assert_eq!(titles.children.len(), 2);
        let TrackChild::Clip(first) = &titles.children[0] else {
            panic!()
        };
        let TrackChild::Clip(second) = &titles.children[1] else {
            panic!()
        };
        assert_eq!(
            first.effects[0]
                .metadata
                .get("text")
                .and_then(|v| v.as_str()),
            Some("First"),
        );
        assert_eq!(
            second.effects[0]
                .metadata
                .get("text")
                .and_then(|v| v.as_str()),
            Some("Second"),
        );
    }

    #[test]
    fn apply_insert_title_rejects_invalid_window() {
        let tl = timeline_with_three_clips();
        // end_s <= start_s.
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTitle {
                start_s: 5.0,
                end_s: 3.0,
                text: "x".into(),
                position: super::super::op::TitlePosition::Center,
                font_size: 64,
                color: "#FFFFFF".into(),
                font_weight: super::super::op::TitleWeight::Normal,
                animation: super::super::op::TitleAnimation::None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("end_s must be"))
        );
    }

    #[test]
    fn apply_insert_title_rejects_empty_text() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTitle {
                start_s: 0.0,
                end_s: 3.0,
                text: "".into(),
                position: super::super::op::TitlePosition::Center,
                font_size: 64,
                color: "#FFFFFF".into(),
                font_weight: super::super::op::TitleWeight::Normal,
                animation: super::super::op::TitleAnimation::None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("non-empty"))
        );
    }

    #[test]
    fn apply_set_title_updates_only_specified_fields() {
        // Create a title, capture its uuid, then mutate its text.
        let tl = timeline_with_three_clips();
        let insert_env = EdlEnvelope {
            ops: vec![EdlOp::InsertTitle {
                start_s: 0.0,
                end_s: 3.0,
                text: "Original".into(),
                position: super::super::op::TitlePosition::Center,
                font_size: 64,
                color: "#FFFFFF".into(),
                font_weight: super::super::op::TitleWeight::Normal,
                animation: super::super::op::TitleAnimation::None,
            }],
        };
        let (after_insert, _) = apply(&tl, &insert_env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(titles) = &after_insert.tracks.children[1] else {
            panic!()
        };
        let TrackChild::Clip(title_clip) = &titles.children[0] else {
            panic!()
        };
        let title_uuid = title_clip
            .metadata
            .awidat
            .as_ref()
            .and_then(|m| m.extra.get("clip_uuid"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .expect("title clip should have a stamped clip_uuid");

        let set_env = EdlEnvelope {
            ops: vec![EdlOp::SetTitle {
                anchor: Anchor::ClipUuid {
                    uuid: title_uuid.clone(),
                },
                start_s: None,
                end_s: None,
                text: Some("Updated".into()),
                position: None,
                font_size: None,
                color: None,
                font_weight: Some(super::super::op::TitleWeight::Bold),
                animation: None,
            }],
        };
        let (after_set, _) = apply(&after_insert, &set_env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(titles2) = &after_set.tracks.children[1] else {
            panic!()
        };
        let TrackChild::Clip(updated) = &titles2.children[0] else {
            panic!()
        };
        let effect = &updated.effects[0];
        // Updated fields.
        assert_eq!(
            effect.metadata.get("text").and_then(|v| v.as_str()),
            Some("Updated"),
        );
        assert_eq!(
            effect.metadata.get("font_weight").and_then(|v| v.as_str()),
            Some("bold"),
        );
        // Untouched fields preserve their original values.
        assert_eq!(
            effect.metadata.get("position").and_then(|v| v.as_str()),
            Some("center"),
        );
        assert_eq!(
            effect.metadata.get("font_size").and_then(|v| v.as_u64()),
            Some(64),
        );
    }

    #[test]
    fn apply_set_title_rejects_clip_without_title_effect() {
        // Anchor a clip-on-V1 (no awidat.title effect) → error.
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetTitle {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                start_s: None,
                end_s: None,
                text: Some("nope".into()),
                position: None,
                font_size: None,
                color: None,
                font_weight: None,
                animation: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("awidat.title"))
        );
    }

    #[test]
    fn apply_set_broadcast_overlay_stores_timeline_metadata() {
        let tl = timeline_with_three_clips();
        let mut config = BroadcastOverlayConfig {
            episode_title: "Ben Adams".into(),
            show_name: "Technologia Talks".into(),
            sponsors: vec!["LEARN-X".into(), "Throwly".into()],
            ..BroadcastOverlayConfig::default()
        };
        config.host_a.name = "Tadiwa Mbuwayesango".into();
        config.host_a.title = "Co-Host".into();
        config.host_a.photo_path = Some("branding/tadiwa.jpg".into());
        config.topics.push(BroadcastTimedEntry {
            time_seconds: 42.0,
            text: "Custom drones".into(),
        });

        let env = EdlEnvelope {
            ops: vec![EdlOp::SetBroadcastOverlay {
                config: config.clone(),
            }],
        };
        let (new_tl, applied) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(applied.applied.len(), 1);
        let stored = new_tl
            .metadata
            .awidat
            .as_ref()
            .and_then(|m| m.broadcast_overlay.as_ref())
            .expect("overlay config should be stored");
        assert_eq!(stored.episode_title, "Ben Adams");
        assert_eq!(
            stored.host_a.photo_path.as_deref(),
            Some("branding/tadiwa.jpg")
        );
        assert_eq!(stored.topics[0].text, "Custom drones");
    }

    #[test]
    fn apply_set_broadcast_overlay_replaces_existing_config() {
        let tl = timeline_with_three_clips();
        let first = BroadcastOverlayConfig {
            episode_title: "First".into(),
            ..BroadcastOverlayConfig::default()
        };
        let second = BroadcastOverlayConfig {
            episode_title: "Second".into(),
            topics: vec![BroadcastTimedEntry {
                time_seconds: 5.0,
                text: "Replacement".into(),
            }],
            ..BroadcastOverlayConfig::default()
        };
        let (after_first, _) = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::SetBroadcastOverlay { config: first }],
            },
            &AnchorContext::empty(),
        )
        .unwrap();
        let (after_second, _) = apply(
            &after_first,
            &EdlEnvelope {
                ops: vec![EdlOp::SetBroadcastOverlay {
                    config: second.clone(),
                }],
            },
            &AnchorContext::empty(),
        )
        .unwrap();
        let stored = after_second
            .metadata
            .awidat
            .as_ref()
            .and_then(|m| m.broadcast_overlay.as_ref())
            .unwrap();
        assert_eq!(stored.episode_title, "Second");
        assert_eq!(stored.topics.len(), 1);
    }

    #[test]
    fn apply_set_broadcast_overlay_rejects_unsafe_asset_paths() {
        let tl = timeline_with_three_clips();
        let mut config = BroadcastOverlayConfig::default();
        config.host_a.photo_path = Some("../secret.jpg".into());
        let err = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::SetBroadcastOverlay { config }],
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("project-relative"))
        );
    }

    #[test]
    fn delete_primary_clip_shifts_broadcast_overlay_topics_and_chapters() {
        let mut tl = timeline_with_three_clips();
        let mut config = BroadcastOverlayConfig::default();
        config.topics = vec![
            BroadcastTimedEntry {
                time_seconds: 4.0,
                text: "Before cut".into(),
            },
            BroadcastTimedEntry {
                time_seconds: 12.0,
                text: "After cut".into(),
            },
        ];
        config.chapters = vec![BroadcastTimedEntry {
            time_seconds: 14.0,
            text: "Shifted chapter".into(),
        }];
        tl.metadata.awidat = Some(AwidatTimelineMetadata {
            broadcast_overlay: Some(config),
            ..AwidatTimelineMetadata::default()
        });

        let (new_tl, _) = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::DeleteClip {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                }],
            },
            &AnchorContext::empty(),
        )
        .unwrap();

        let overlay = new_tl
            .metadata
            .awidat
            .as_ref()
            .and_then(|m| m.broadcast_overlay.as_ref())
            .unwrap();
        assert_eq!(overlay.topics[0].time_seconds, 4.0);
        assert_eq!(overlay.topics[1].time_seconds, 7.0);
        assert_eq!(overlay.chapters[0].time_seconds, 9.0);
    }

    #[test]
    fn apply_set_volume_then_set_speed_coexist_on_same_clip() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetVolume {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    value: 0.5,
                },
                EdlOp::SetSpeed {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    factor: 1.5,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        // Different effect names → both effects survive.
        assert_eq!(clip.effects.len(), 2);
        let names: Vec<&str> = clip
            .effects
            .iter()
            .map(|e| e.effect_name.as_str())
            .collect();
        assert!(names.contains(&"awidat.volume"));
        assert!(names.contains(&"awidat.speed"));
    }
}
