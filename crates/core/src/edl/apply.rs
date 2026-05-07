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

use awidat_proto::awidat_meta::AwidatClipMetadata;
use awidat_proto::otio::{Clip, StackChild, Timeline, TrackChild};
use thiserror::Error;

use super::anchor::{AnchorContext, ClipLocator, resolve};
use super::op::{Anchor, EdlEnvelope, EdlOp};

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
            at_position,
            start,
            end,
            name,
        } => apply_insert_clip(
            working,
            index,
            asset,
            track,
            *at_position,
            *start,
            *end,
            name.as_deref(),
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
        EdlOp::MoveClip {
            anchor,
            to_position,
        } => apply_move_clip(working, index, anchor, *to_position, ctx, locator),
        EdlOp::InsertTransition {
            between,
            kind,
            duration_s,
        } => apply_insert_transition(working, index, between, kind, *duration_s, ctx),
        EdlOp::SetVolume { .. } => Err(ApplyError::NotImplemented {
            index,
            op: "Set Volume".into(),
        }),
        EdlOp::SetSpeed { .. } => Err(ApplyError::NotImplemented {
            index,
            op: "Set Speed".into(),
        }),
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
        | EdlOp::SetVolume { anchor, .. }
        | EdlOp::SetSpeed { anchor, .. } => anchor,
        EdlOp::InsertClip { .. } | EdlOp::InsertTransition { .. } => return Ok(None),
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
    at_position: Option<usize>,
    start_s: Option<f64>,
    end_s: Option<f64>,
    name_override: Option<&str>,
) -> Result<String, ApplyError> {
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, TimeRange, Track, TrackKind,
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
            let track = Track::empty(track_name.to_string(), TrackKind::Video);
            working.tracks.children.push(StackChild::Track(track));
            working.tracks.children.len() - 1
        }
    };

    let StackChild::Track(track) = &mut working.tracks.children[track_idx] else {
        return Err(ApplyError::Invalid {
            index,
            message: "track index resolved to a non-track stack child".into(),
        });
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

    track.children.insert(position, TrackChild::Clip(clip));

    Ok(format!(
        "inserted clip {chosen_name:?} on track {track_name:?} at \
         position {position}: asset={asset:?} source=[{start:.3}s..{end:.3}s] \
         ({:.3}s)",
        end - start
    ))
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
    let awidat = clip.metadata.awidat.get_or_insert_with(AwidatClipMetadata::default);
    awidat
        .extra
        .insert("clip_uuid".into(), serde_json::Value::String(uuid));
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

    Ok(format!(
        "split clip {original_name:?} at {at_s:.3}s → {original_name:?} \
         [{start_s:.3}s..{at_s:.3}s] + {original_name:?}-b \
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
    Ok(format!("deleted clip {name:?}"))
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
    ctx: &AnchorContext,
) -> Result<String, ApplyError> {
    use awidat_proto::otio::Transition;

    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("transition duration {duration_s} must be > 0"),
        });
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

    let transition = Transition::symmetric(kind, duration_s, rate);
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
        let range = clip.source_range.as_ref().ok_or_else(|| ApplyError::Invalid {
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
    broll.media_reference =
        MediaReference::External(ExternalReference::new(asset.to_string()));
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
            let StackChild::Track(track) = &mut working.tracks.children[locator.track_index]
            else {
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
            let target_idx = working
                .tracks
                .children
                .iter()
                .enumerate()
                .find_map(|(i, sc)| match sc {
                    StackChild::Track(t)
                        if matches!(t.kind, TrackKind::Video)
                            && i != locator.track_index =>
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
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
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
                at_position: None,
                start: Some(0.0),
                end: Some(3.0),
                name: None,
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
                at_position: None,
                start: Some(0.0),
                end: Some(56.47),
                name: None,
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
                at_position: None,
                start: Some(0.0),
                end: Some(2.0),
                name: Some("intro".into()),
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
                at_position: Some(1),
                start: Some(0.0),
                end: Some(1.0),
                name: Some("inserted".into()),
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
                at_position: Some(1),
                start: Some(0.0),
                end: Some(1.0),
                name: None,
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
    fn apply_insert_clip_rejects_zero_duration() {
        use awidat_proto::otio::Timeline as Tl;
        let tl = Tl::empty("test");
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/x.mp4".into(),
                track: "V1".into(),
                at_position: None,
                start: Some(5.0),
                end: Some(5.0),
                name: None,
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
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0].description.contains("already at position"),
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
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::AnchorMiss { .. }),
            "expected anchor miss, got something else",
        );
    }
}
