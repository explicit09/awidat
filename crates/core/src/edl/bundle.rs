//! Bundle a continuity-fixer onto a base EDL envelope. Phase 2.4.
//!
//! When `assess_continuity` returns `dirty` for a proposed cut, the
//! agent shouldn't ship the raw cut on its own — the user will see
//! a jarring edit. Instead the agent calls one of the helpers here
//! to build a single envelope that contains BOTH the original op
//! AND a 0.3s SMPTE_Dissolve between the affected clip and its
//! neighbor. The user gets one ghost overlay covering the whole fix.
//!
//! v1 shipped cross-dissolve bundling; Phase 3.7 adds the b-roll
//! cover variant ([`bundle_with_broll_cover`]) so the agent can mask
//! a dirty cut by laying a fetched Pexels clip over the affected
//! window instead of softening with a transition. The agent picks
//! between dissolve and b-roll based on the rule that fired:
//! mid-motion or speaker-switch mid-utterance → b-roll; everything
//! else → dissolve.
//!
//! Pure function — no I/O. Caller provides the envelope plus anchors
//! and the helper returns a new envelope with the appropriate
//! `*** Insert Transition` or `*** Insert BRoll` op appended.

use super::op::{Anchor, BRollPosition, EdlEnvelope, EdlOp, TransitionBetween};

/// Default dissolve length when bundling continuity-fixers.
/// Short enough to be unobtrusive, long enough to soften the cut.
pub const DEFAULT_DISSOLVE_S: f64 = 0.3;

/// Append a `*** Insert Transition` op to `envelope` placing a
/// SMPTE_Dissolve between `from` and `to`. Returns a fresh envelope
/// (caller's input is consumed). The added op runs AFTER any
/// existing ops in the envelope — the apply layer evaluates left-
/// to-right, so a Trim + Transition lands trim-first, then
/// transition.
///
/// The dissolve duration defaults to [`DEFAULT_DISSOLVE_S`]; pass
/// a custom value when the caller has reason to (a 0.5s dissolve
/// reads more deliberate, a 0.15s dissolve hides a sharp cut).
pub fn bundle_with_dissolve(
    envelope: EdlEnvelope,
    from: Anchor,
    to: Anchor,
    duration_s: Option<f64>,
) -> EdlEnvelope {
    let mut ops = envelope.ops;
    ops.push(EdlOp::InsertTransition {
        between: TransitionBetween { from, to },
        kind: "SMPTE_Dissolve".to_string(),
        duration_s: duration_s.unwrap_or(DEFAULT_DISSOLVE_S),
    });
    EdlEnvelope { ops }
}

/// Default cutaway length when bundling a b-roll cover for a dirty
/// cut. Long enough to actually hide the underlying jar, short
/// enough that the audience doesn't lose the speaker's flow.
pub const DEFAULT_BROLL_COVER_S: f64 = 2.0;

/// Append a `*** Insert BRoll` op to `envelope` covering `anchor`
/// with the asset at `asset_path` for `duration_s` seconds. Position
/// is always `Overlay` — replacing the underlying clip would defeat
/// the purpose (the audio under the cut still needs to play).
///
/// Returns a fresh envelope with the new op appended. Caller picks
/// the asset (typically a Pexels download from `use_broll`); this
/// helper is a pure ops-list manipulator and doesn't fetch.
///
/// Use when [`crate::continuity::Verdict::Dirty`] fires AND a
/// b-roll opportunity has surfaced at the same anchor — the cover
/// hides the visual jar without altering the audio. Fall back to
/// [`bundle_with_dissolve`] when no good b-roll is available.
pub fn bundle_with_broll_cover(
    envelope: EdlEnvelope,
    anchor: Anchor,
    asset_path: impl Into<String>,
    duration_s: Option<f64>,
) -> EdlEnvelope {
    let mut ops = envelope.ops;
    ops.push(EdlOp::InsertBRoll {
        anchor,
        asset: asset_path.into(),
        duration_s: duration_s.unwrap_or(DEFAULT_BROLL_COVER_S),
        position: BRollPosition::Overlay,
    });
    EdlEnvelope { ops }
}

/// True when the envelope contains a continuity-related cut op
/// (Trim, Split, or Delete). The agent reads this before deciding
/// whether to bundle — there's no point appending a transition to
/// an envelope that's just `Set Volume` or `Insert Title`.
pub fn envelope_needs_continuity_check(envelope: &EdlEnvelope) -> bool {
    envelope.ops.iter().any(|op| {
        matches!(
            op,
            EdlOp::TrimClip { .. } | EdlOp::SplitClip { .. } | EdlOp::DeleteClip { .. }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edl::op::{Anchor, EdlOp};

    fn anchor(name: &str) -> Anchor {
        Anchor::ClipUuid {
            uuid: name.to_string(),
        }
    }

    #[test]
    fn bundle_appends_insert_transition() {
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: anchor("clip-a"),
                start: None,
                end: Some(2.0),
            }],
        };
        let bundled = bundle_with_dissolve(env, anchor("clip-a"), anchor("clip-b"), None);
        assert_eq!(bundled.ops.len(), 2);
        match &bundled.ops[1] {
            EdlOp::InsertTransition {
                between,
                kind,
                duration_s,
            } => {
                assert_eq!(kind, "SMPTE_Dissolve");
                assert!((duration_s - DEFAULT_DISSOLVE_S).abs() < 1e-9);
                assert!(matches!(&between.from, Anchor::ClipUuid { uuid } if uuid == "clip-a"));
                assert!(matches!(&between.to, Anchor::ClipUuid { uuid } if uuid == "clip-b"));
            }
            _ => panic!("expected InsertTransition at end of bundle"),
        }
    }

    #[test]
    fn bundle_honors_custom_duration() {
        let env = EdlEnvelope {
            ops: vec![EdlOp::SplitClip {
                anchor: anchor("clip-a"),
                at_s: 5.0,
            }],
        };
        let bundled =
            bundle_with_dissolve(env, anchor("clip-a"), anchor("clip-b"), Some(0.5));
        match &bundled.ops[1] {
            EdlOp::InsertTransition { duration_s, .. } => {
                assert!((duration_s - 0.5).abs() < 1e-9);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn bundle_preserves_original_ops_in_order() {
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::TrimClip {
                    anchor: anchor("a"),
                    start: Some(1.0),
                    end: None,
                },
                EdlOp::DeleteClip {
                    anchor: anchor("b"),
                },
            ],
        };
        let bundled = bundle_with_dissolve(env, anchor("a"), anchor("c"), None);
        assert_eq!(bundled.ops.len(), 3);
        assert!(matches!(&bundled.ops[0], EdlOp::TrimClip { .. }));
        assert!(matches!(&bundled.ops[1], EdlOp::DeleteClip { .. }));
        assert!(matches!(&bundled.ops[2], EdlOp::InsertTransition { .. }));
    }

    #[test]
    fn envelope_needs_continuity_check_detects_trims() {
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: anchor("a"),
                start: None,
                end: Some(2.0),
            }],
        };
        assert!(envelope_needs_continuity_check(&env));
    }

    #[test]
    fn envelope_needs_continuity_check_skips_non_cut_ops() {
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetVolume {
                anchor: anchor("a"),
                value: 0.5,
            }],
        };
        assert!(!envelope_needs_continuity_check(&env));
    }

    #[test]
    fn bundle_with_broll_cover_appends_insert_broll() {
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: anchor("clip-a"),
                start: None,
                end: Some(2.0),
            }],
        };
        let bundled = bundle_with_broll_cover(
            env,
            anchor("clip-a"),
            "raw/broll/pexels-9.mp4",
            None,
        );
        assert_eq!(bundled.ops.len(), 2);
        match &bundled.ops[1] {
            EdlOp::InsertBRoll {
                asset,
                duration_s,
                position,
                anchor,
            } => {
                assert_eq!(asset, "raw/broll/pexels-9.mp4");
                assert!((duration_s - DEFAULT_BROLL_COVER_S).abs() < 1e-9);
                assert_eq!(*position, BRollPosition::Overlay);
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-a"));
            }
            _ => panic!("expected InsertBRoll"),
        }
    }

    #[test]
    fn bundle_with_broll_cover_honors_custom_duration() {
        let env = EdlEnvelope {
            ops: vec![EdlOp::SplitClip {
                anchor: anchor("clip-a"),
                at_s: 5.0,
            }],
        };
        let bundled = bundle_with_broll_cover(
            env,
            anchor("clip-a"),
            "raw/broll/pexels-1.mp4",
            Some(3.5),
        );
        match &bundled.ops[1] {
            EdlOp::InsertBRoll { duration_s, .. } => {
                assert!((duration_s - 3.5).abs() < 1e-9);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn envelope_needs_continuity_check_detects_splits_and_deletes() {
        let split_env = EdlEnvelope {
            ops: vec![EdlOp::SplitClip {
                anchor: anchor("a"),
                at_s: 2.0,
            }],
        };
        let delete_env = EdlEnvelope {
            ops: vec![EdlOp::DeleteClip {
                anchor: anchor("a"),
            }],
        };
        assert!(envelope_needs_continuity_check(&split_env));
        assert!(envelope_needs_continuity_check(&delete_env));
    }
}
