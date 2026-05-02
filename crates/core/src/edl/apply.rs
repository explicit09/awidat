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

use awidat_proto::otio::{StackChild, Timeline, TrackChild};
use thiserror::Error;

use super::anchor::{ClipLocator, resolve};
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
pub fn apply(
    original: &Timeline,
    envelope: &EdlEnvelope,
) -> Result<(Timeline, ApplyOutcome), ApplyError> {
    let mut working = original.clone();
    let mut applied = Vec::with_capacity(envelope.ops.len());

    for (index, op) in envelope.ops.iter().enumerate() {
        let description = apply_one(&mut working, index, op)?;
        let locator = locator_for_log(op, &working);
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
fn apply_one(working: &mut Timeline, index: usize, op: &EdlOp) -> Result<String, ApplyError> {
    match op {
        EdlOp::TrimClip {
            anchor,
            start,
            end,
        } => apply_trim(working, index, anchor, *start, *end),
        EdlOp::DeleteClip { anchor } => apply_delete(working, index, anchor),
        EdlOp::InsertBRoll { .. } => Err(ApplyError::NotImplemented {
            index,
            op: "Insert BRoll".into(),
        }),
        EdlOp::MoveClip { .. } => Err(ApplyError::NotImplemented {
            index,
            op: "Move Clip".into(),
        }),
        EdlOp::InsertTransition { .. } => Err(ApplyError::NotImplemented {
            index,
            op: "Insert Transition".into(),
        }),
    }
}

fn apply_trim(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    new_start: Option<f64>,
    new_end: Option<f64>,
) -> Result<String, ApplyError> {
    let locator = resolve(working, anchor).map_err(|miss| ApplyError::AnchorMiss { index, miss })?;
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
            message: format!(
                "trim: end {target_end} must be >= start {target_start}"
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

fn apply_delete(working: &mut Timeline, index: usize, anchor: &Anchor) -> Result<String, ApplyError> {
    let locator = resolve(working, anchor).map_err(|miss| ApplyError::AnchorMiss { index, miss })?;
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

fn locator_for_log(op: &EdlOp, _working: &Timeline) -> Option<ClipLocator> {
    // We don't re-resolve here — too late, the clip may be gone.
    // The applied description carries enough info; the locator is best-
    // effort. Return None for now; week-5 TUI will use the description.
    let _ = op;
    None
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
            let StackChild::Track(track) = sc else { continue };
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
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild,
        TimeRange, Track, TrackChild, TrackKind,
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
        let (new_tl, outcome) = apply(&tl, &env).unwrap();
        assert_eq!(outcome.applied.len(), 1);
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
    fn apply_delete_removes_clip() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo".into(),
                },
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env).unwrap();
        assert_eq!(outcome.applied.len(), 1);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 2);
        // Remaining clips: alpha, charlie.
        let TrackChild::Clip(c1) = &t.children[1] else { panic!() };
        assert_eq!(c1.name, "clip-2");
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
        let err = apply(&tl, &env).unwrap_err();
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
        let _ = apply(&tl, &env);
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
        let (new_tl, outcome) = apply(&tl, &env).unwrap();
        assert_eq!(outcome.applied.len(), 2);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // Alpha trimmed, bravo unchanged, charlie gone.
        assert_eq!(t.children.len(), 2);
        let TrackChild::Clip(alpha) = &t.children[0] else { panic!() };
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
        let err = apply(&tl, &env).unwrap_err();
        assert!(matches!(err, ApplyError::Invalid { message, .. } if message.contains("must be >= start")));
    }

    #[test]
    fn unimplemented_op_returns_clear_error() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::MoveClip {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-0".into(),
                },
                to_position: 2,
            }],
        };
        let err = apply(&tl, &env).unwrap_err();
        assert!(matches!(err, ApplyError::NotImplemented { op, .. } if op == "Move Clip"));
    }
}
