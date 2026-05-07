//! Rule-based continuity engine — Phase 2.
//!
//! Evaluates whether a proposed cut/trim/split at timeline-time `t`
//! would jar the viewer. Five pure-function rules contribute:
//!
//!   1. **No mid-sentence cuts** — whisper sidecar shows speaker
//!      mid-word at `t`.
//!   2. **Respect breath beats** — cutting OUT a silence shorter
//!      than 0.4s makes speech feel rushed.
//!   3. **No mid-motion cuts** — motion sidecar shows high
//!      magnitude at `t` AND no scene change nearby.
//!   4. **Speaker turn boundaries** — diarization (when present)
//!      makes mid-utterance speaker swaps risky.
//!   5. **Rhythm preservation** — three or more cuts within 5s
//!      looks frantic in podcast format.
//!
//! Each rule returns its own [`RuleVerdict`]. The aggregator
//! combines them — any "dirty" wins; multiple "risky" with no
//! "dirty" stays risky; all "clean"/"abstain" stays clean.
//!
//! **Abstain** is a load-bearing third state: when a rule's input
//! is missing (no whisper sidecar, no motion sidecar, etc), it
//! says "I don't know" rather than "clean." This keeps a
//! half-indexed project from showing false-clean confidence.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Per-rule and aggregate verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Cut is fine; no rule flagged it.
    Clean,
    /// One or more rules flagged the cut as questionable. The
    /// agent should ask the user (Manual / Copilot) or bundle a
    /// crossfade automatically (Autopilot).
    Risky,
    /// At least one rule is confident the cut would jar. The
    /// agent should propose a transition or b-roll cover, NOT
    /// the raw cut.
    Dirty,
    /// At least one rule had no input data. The aggregate uses
    /// this when *every* rule abstained — otherwise "abstain"
    /// degrades to whatever non-abstain rules said.
    Abstain,
}

/// What kind of cut is being assessed. Different kinds weigh
/// rules differently — e.g. a `Split` doesn't care about rhythm
/// preservation the way a `Cut` does (split is conceptually
/// non-destructive; subsequent edits are what create rhythm).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutKind {
    /// A trim's leading edge — `*** Trim Clip + start:`.
    TrimIn,
    /// A trim's trailing edge — `*** Trim Clip + end:`.
    TrimOut,
    /// A split point — `*** Split Clip + at_s:`.
    Cut,
}

/// One rule's contribution to the verdict. The `id` is stable
/// (used by the dismissal-pattern matcher in 1.6) and the
/// `reason` is human-readable (rendered in NotesPanel).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleVerdict {
    /// Stable rule identifier, e.g. `"mid_sentence"`. String rather
    /// than `&'static str` so the type round-trips through serde
    /// (the agent tool serializes these to JSON for the model).
    pub id: String,
    /// Per-rule verdict.
    pub verdict: Verdict,
    /// Human-readable explanation. Empty when the rule abstained.
    pub reason: String,
}

/// Aggregate output of [`assess_continuity`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContinuityVerdict {
    /// Combined verdict across all rules.
    pub verdict: Verdict,
    /// Per-rule contributions, in the order they were evaluated.
    /// Rules that abstained are still listed so the agent can
    /// reason about which inputs are missing.
    pub rules: Vec<RuleVerdict>,
}

/// All inputs the continuity engine needs. Caller assembles this
/// once per session and reuses across calls (the asset sidecars
/// rarely change during a turn).
#[derive(Debug, Clone, Default)]
pub struct ContinuityInputs {
    /// Whisper word-level alignment for the asset under cut, if
    /// available. `None` → mid-sentence rule abstains.
    pub whisper_words: Option<Vec<WhisperWord>>,
    /// Whisper segment-level alignment with speaker labels, if
    /// diarization ran. `None` → speaker-turn rule abstains.
    pub whisper_segments: Option<Vec<WhisperSegment>>,
    /// Per-second motion magnitudes from the motion sidecar, if
    /// available. `None` → mid-motion rule abstains.
    pub motion_magnitudes: Option<Vec<f32>>,
    /// Source-time scene-change indices from the scenedetect
    /// sidecar, if indexed. Each entry is the source-time
    /// (seconds) of a hard cut between shots.
    pub scene_changes_s: Option<Vec<f64>>,
    /// Source-time silence ranges from the silence sidecar.
    /// Used by the breath-beat rule.
    pub silences: Option<Vec<SilenceRange>>,
    /// Existing cut points within ±5 seconds of `at_s`.
    /// Caller pre-filters from the timeline so the rule engine
    /// doesn't need to walk OTIO. Empty vec → no nearby cuts.
    pub nearby_cuts_s: Vec<f64>,
}

/// One whisper word with timing. Mirrors the shape the existing
/// `index/whisper/<asset>.json` files use.
#[derive(Debug, Clone)]
pub struct WhisperWord {
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
}

/// One whisper segment with optional speaker label.
#[derive(Debug, Clone)]
pub struct WhisperSegment {
    pub start_s: f64,
    pub end_s: f64,
    pub speaker_id: Option<String>,
}

/// One silence range. Mirrors the silence sidecar shape.
#[derive(Debug, Clone, Copy)]
pub struct SilenceRange {
    pub start_s: f64,
    pub end_s: f64,
}

/// Top-level entry point. Evaluates all five rules and returns the
/// aggregate. Pure function — no I/O.
///
/// **Two coordinate spaces matter here.** Whisper/motion/silence
/// sidecars are keyed by source-media seconds (the asset's own
/// timeline). The rhythm-preservation rule cares about cuts on the
/// master timeline (the project's playback time). When a cut sits
/// on clip C, source-time and track-time differ by
/// `clip.source_range.start - clip.track_start`. The agent tool
/// resolves both before calling.
///
/// `source_at_s`: source-media time for whisper/motion/silence/
/// speaker rules.
/// `track_at_s`: master-timeline time for the rhythm rule. The
/// caller pre-fills `inputs.nearby_cuts_s` in the same coord space.
pub fn assess_continuity(
    source_at_s: f64,
    track_at_s: f64,
    kind: CutKind,
    inputs: &ContinuityInputs,
) -> ContinuityVerdict {
    let mut rules = Vec::with_capacity(5);
    rules.push(rule_mid_sentence(source_at_s, inputs));
    rules.push(rule_breath_beat(source_at_s, kind, inputs));
    rules.push(rule_mid_motion(source_at_s, inputs));
    rules.push(rule_speaker_turn(source_at_s, inputs));
    rules.push(rule_rhythm(track_at_s, kind, inputs));

    let verdict = aggregate_verdicts(&rules);
    ContinuityVerdict { verdict, rules }
}

/// Combine per-rule verdicts into the aggregate.
///
/// Rules:
/// - Any `Dirty` → `Dirty`.
/// - One or more `Risky` (and no `Dirty`) → `Risky`.
/// - All `Abstain` → `Abstain`.
/// - Otherwise → `Clean`.
fn aggregate_verdicts(rules: &[RuleVerdict]) -> Verdict {
    let mut any_dirty = false;
    let mut any_risky = false;
    let mut any_concrete = false;
    for r in rules {
        match r.verdict {
            Verdict::Dirty => any_dirty = true,
            Verdict::Risky => any_risky = true,
            Verdict::Clean => any_concrete = true,
            Verdict::Abstain => {}
        }
    }
    if any_dirty {
        Verdict::Dirty
    } else if any_risky {
        Verdict::Risky
    } else if any_concrete {
        Verdict::Clean
    } else {
        Verdict::Abstain
    }
}

// --- rule 1: mid-sentence ---

/// Dirty if a whisper word's [start, end] strictly contains `at_s`.
/// (Cuts that land on a word boundary are clean.) Abstains when
/// no whisper sidecar is loaded.
pub fn rule_mid_sentence(at_s: f64, inputs: &ContinuityInputs) -> RuleVerdict {
    let Some(words) = &inputs.whisper_words else {
        return RuleVerdict {
            id: "mid_sentence".into(),
            verdict: Verdict::Abstain,
            reason: String::new(),
        };
    };
    for w in words {
        // Strict interior — equality at the boundaries is clean.
        if w.start_s < at_s && at_s < w.end_s {
            return RuleVerdict {
                id: "mid_sentence".into(),
                verdict: Verdict::Dirty,
                reason: format!(
                    "cut lands mid-word: speaker is saying \"{}\" at {at_s:.2}s",
                    w.text.trim()
                ),
            };
        }
    }
    RuleVerdict {
        id: "mid_sentence".into(),
        verdict: Verdict::Clean,
        reason: String::new(),
    }
}

// --- rule 2: breath-beat preservation ---

/// `TrimIn` / `TrimOut` whose edge falls within a silence range
/// shorter than 0.4s is suspicious — that silence is a breath
/// beat the speaker took, not dead air; cutting it makes the
/// preceding/following speech feel rushed.
///
/// `Cut` (split) doesn't trigger this rule — splits are
/// non-destructive at the cut point.
pub fn rule_breath_beat(
    at_s: f64,
    kind: CutKind,
    inputs: &ContinuityInputs,
) -> RuleVerdict {
    if matches!(kind, CutKind::Cut) {
        return RuleVerdict {
            id: "breath_beat".into(),
            verdict: Verdict::Clean,
            reason: String::new(),
        };
    }
    let Some(silences) = &inputs.silences else {
        return RuleVerdict {
            id: "breath_beat".into(),
            verdict: Verdict::Abstain,
            reason: String::new(),
        };
    };
    for s in silences {
        // Trim edge falls inside this silence (any amount; even at
        // boundary is concerning for a short breath).
        if s.start_s <= at_s && at_s <= s.end_s {
            let duration = s.end_s - s.start_s;
            if duration < 0.4 {
                return RuleVerdict {
                    id: "breath_beat".into(),
                    verdict: Verdict::Risky,
                    reason: format!(
                        "trim cuts into a {duration:.2}s breath beat — \
                         speech may feel rushed"
                    ),
                };
            }
        }
    }
    RuleVerdict {
        id: "breath_beat".into(),
        verdict: Verdict::Clean,
        reason: String::new(),
    }
}

// --- rule 3: mid-motion ---

/// Cuts within ±0.3s of a scene change are clean (the editor is
/// cutting on a hard cut). Otherwise, when motion magnitude at
/// `at_s` exceeds 0.4, flag as risky. Above 0.65 is dirty.
pub fn rule_mid_motion(at_s: f64, inputs: &ContinuityInputs) -> RuleVerdict {
    let Some(magnitudes) = &inputs.motion_magnitudes else {
        return RuleVerdict {
            id: "mid_motion".into(),
            verdict: Verdict::Abstain,
            reason: String::new(),
        };
    };

    // Scene-change proximity exempts the cut. Convert source-time
    // to magnitude index (1 sample per second).
    if let Some(scenes) = &inputs.scene_changes_s
        && scenes.iter().any(|s| (s - at_s).abs() <= 0.3)
    {
        return RuleVerdict {
            id: "mid_motion".into(),
            verdict: Verdict::Clean,
            reason: String::new(),
        };
    }

    let idx = at_s.floor() as usize;
    if idx >= magnitudes.len() {
        return RuleVerdict {
            id: "mid_motion".into(),
            verdict: Verdict::Abstain,
            reason: String::new(),
        };
    }
    let m = magnitudes[idx];
    if m >= 0.65 {
        RuleVerdict {
            id: "mid_motion".into(),
            verdict: Verdict::Dirty,
            reason: format!(
                "high motion at {at_s:.2}s (score {m:.2}) — cut would jar; \
                 prefer a transition or b-roll cover"
            ),
        }
    } else if m >= 0.4 {
        RuleVerdict {
            id: "mid_motion".into(),
            verdict: Verdict::Risky,
            reason: format!(
                "moderate motion at {at_s:.2}s (score {m:.2}) — consider a \
                 short crossfade"
            ),
        }
    } else {
        RuleVerdict {
            id: "mid_motion".into(),
            verdict: Verdict::Clean,
            reason: String::new(),
        }
    }
}

// --- rule 4: speaker turn ---

/// When diarization data is available, flag cuts that fall
/// strictly inside a single speaker's segment (i.e. NOT at a
/// natural speaker boundary) as risky for non-Cut kinds.
/// Abstains when no diarized segments are loaded.
pub fn rule_speaker_turn(at_s: f64, inputs: &ContinuityInputs) -> RuleVerdict {
    let Some(segments) = &inputs.whisper_segments else {
        return RuleVerdict {
            id: "speaker_turn".into(),
            verdict: Verdict::Abstain,
            reason: String::new(),
        };
    };
    let any_diarized = segments.iter().any(|s| s.speaker_id.is_some());
    if !any_diarized {
        return RuleVerdict {
            id: "speaker_turn".into(),
            verdict: Verdict::Abstain,
            reason: String::new(),
        };
    }
    // Find the segment containing `at_s`. If the cut sits well
    // inside (>0.5s from either end), flag risky — natural
    // speaker boundaries are at segment edges.
    for s in segments {
        if s.start_s < at_s && at_s < s.end_s {
            let from_start = at_s - s.start_s;
            let to_end = s.end_s - at_s;
            if from_start > 0.5 && to_end > 0.5 {
                return RuleVerdict {
                    id: "speaker_turn".into(),
                    verdict: Verdict::Risky,
                    reason: format!(
                        "cut lands mid-utterance (speaker {}); prefer edges \
                         of the segment",
                        s.speaker_id.as_deref().unwrap_or("?")
                    ),
                };
            }
            return RuleVerdict {
                id: "speaker_turn".into(),
                verdict: Verdict::Clean,
                reason: String::new(),
            };
        }
    }
    RuleVerdict {
        id: "speaker_turn".into(),
        verdict: Verdict::Clean,
        reason: String::new(),
    }
}

// --- rule 5: rhythm preservation ---

/// Risky when the proposed cut would land within 5 seconds of
/// two or more existing cuts (3+ cuts in a 5s window looks
/// frantic in podcast format). `Cut` (split) ignores this rule —
/// splits are structural, not visible cuts.
pub fn rule_rhythm(
    at_s: f64,
    kind: CutKind,
    inputs: &ContinuityInputs,
) -> RuleVerdict {
    if matches!(kind, CutKind::Cut) {
        return RuleVerdict {
            id: "rhythm".into(),
            verdict: Verdict::Clean,
            reason: String::new(),
        };
    }
    let nearby = inputs
        .nearby_cuts_s
        .iter()
        .filter(|t| (**t - at_s).abs() <= 2.5)
        .count();
    // `nearby` counts existing cuts within ±2.5s (a 5s window
    // centered on the proposed cut). With the proposed cut
    // included, 2+ existing cuts = 3+ total in the window.
    if nearby >= 2 {
        return RuleVerdict {
            id: "rhythm".into(),
            verdict: Verdict::Risky,
            reason: format!(
                "{nearby} existing cuts within 5s of {at_s:.2}s — adding \
                 another would feel frantic"
            ),
        };
    }
    RuleVerdict {
        id: "rhythm".into(),
        verdict: Verdict::Clean,
        reason: String::new(),
    }
}

// --- sidecar I/O helpers ---
//
// These let the `assess_continuity` tool build [`ContinuityInputs`]
// from on-disk sidecars without the engine knowing about paths.
// Pure-function rule code stays separate from I/O for testability.

/// FNV-1a hash matching `apps/desktop/.../media.rs::stable_path_hash`.
/// Required for sidecar path lookup. Identical algorithm copied so
/// `awidat-core` doesn't depend on the desktop crate.
fn stable_path_hash(path: &Path) -> u32 {
    let mut hash = 0x811c9dc5_u32;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

/// Path to the per-asset motion sidecar.
pub fn motion_sidecar_path(project_root: &Path, asset_id: &str) -> PathBuf {
    let abs = project_root.join(asset_id);
    let stem = abs.file_stem().and_then(|s| s.to_str()).unwrap_or("asset");
    project_root
        .join(".awidat")
        .join("motion")
        .join(format!("{stem}-{:08x}.json", stable_path_hash(&abs)))
}

/// Path to the per-asset silence sidecar.
pub fn silence_sidecar_path(project_root: &Path, asset_id: &str) -> PathBuf {
    let abs = project_root.join(asset_id);
    let stem = abs.file_stem().and_then(|s| s.to_str()).unwrap_or("asset");
    project_root
        .join(".awidat")
        .join("silences")
        .join(format!("{stem}-{:08x}.json", stable_path_hash(&abs)))
}

/// Path to the per-asset whisper sidecar produced by the indexer.
pub fn whisper_sidecar_path(project_root: &Path, asset_id: &str) -> PathBuf {
    project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset_id}.json"))
}

/// Path to the per-asset scenedetect sidecar.
pub fn scenedetect_sidecar_path(project_root: &Path, asset_id: &str) -> PathBuf {
    project_root
        .join("index")
        .join("scenedetect")
        .join(format!("{asset_id}.json"))
}

/// Load motion magnitudes from disk. Returns `None` on missing /
/// unparseable sidecar — the caller treats `None` as "rule abstains."
pub fn load_motion_magnitudes(project_root: &Path, asset_id: &str) -> Option<Vec<f32>> {
    let path = motion_sidecar_path(project_root, asset_id);
    let bytes = std::fs::read(&path).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("magnitudes")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect()
        })
}

/// Load silence ranges from the silence sidecar.
pub fn load_silences(project_root: &Path, asset_id: &str) -> Option<Vec<SilenceRange>> {
    let path = silence_sidecar_path(project_root, asset_id);
    let bytes = std::fs::read(&path).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("ranges").and_then(|m| m.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|r| {
                let s = r.get("start_s")?.as_f64()?;
                let e = r.get("end_s")?.as_f64()?;
                Some(SilenceRange {
                    start_s: s,
                    end_s: e,
                })
            })
            .collect()
    })
}

/// Load whisper words from disk.
pub fn load_whisper_words(project_root: &Path, asset_id: &str) -> Option<Vec<WhisperWord>> {
    let path = whisper_sidecar_path(project_root, asset_id);
    let bytes = std::fs::read(&path).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let arr = v.pointer("/data/words").and_then(|x| x.as_array())?;
    Some(
        arr.iter()
            .filter_map(|w| {
                let start = w
                    .get("start_s")
                    .or_else(|| w.get("start"))?
                    .as_f64()?;
                let end = w.get("end_s").or_else(|| w.get("end"))?.as_f64()?;
                let text = w.get("text")?.as_str()?.to_string();
                Some(WhisperWord {
                    start_s: start,
                    end_s: end,
                    text,
                })
            })
            .collect(),
    )
}

/// Load whisper segments (with optional speaker IDs).
pub fn load_whisper_segments(project_root: &Path, asset_id: &str) -> Option<Vec<WhisperSegment>> {
    let path = whisper_sidecar_path(project_root, asset_id);
    let bytes = std::fs::read(&path).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let arr = v.pointer("/data/segments").and_then(|x| x.as_array())?;
    Some(
        arr.iter()
            .filter_map(|s| {
                let start = s.get("start_s").or_else(|| s.get("start"))?.as_f64()?;
                let end = s.get("end_s").or_else(|| s.get("end"))?.as_f64()?;
                let speaker_id = s.get("speaker_id").and_then(|x| x.as_str()).map(String::from);
                Some(WhisperSegment {
                    start_s: start,
                    end_s: end,
                    speaker_id,
                })
            })
            .collect(),
    )
}

/// Load scene-change source-times from the scenedetect sidecar.
pub fn load_scene_changes(project_root: &Path, asset_id: &str) -> Option<Vec<f64>> {
    let path = scenedetect_sidecar_path(project_root, asset_id);
    let bytes = std::fs::read(&path).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    // scenedetect emits shots with start/end times; we want the
    // boundaries (each shot's start, except the first which is
    // always 0).
    let shots = v.pointer("/data/shots").and_then(|x| x.as_array())?;
    let mut out = Vec::with_capacity(shots.len());
    for (i, s) in shots.iter().enumerate() {
        if i == 0 {
            continue;
        }
        if let Some(t) = s.get("start_s").or_else(|| s.get("start")).and_then(|x| x.as_f64()) {
            out.push(t);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(words: &[(&str, f64, f64)]) -> Vec<WhisperWord> {
        words
            .iter()
            .map(|(t, s, e)| WhisperWord {
                start_s: *s,
                end_s: *e,
                text: (*t).to_string(),
            })
            .collect()
    }

    fn segs(items: &[(f64, f64, Option<&str>)]) -> Vec<WhisperSegment> {
        items
            .iter()
            .map(|(s, e, sp)| WhisperSegment {
                start_s: *s,
                end_s: *e,
                speaker_id: sp.map(String::from),
            })
            .collect()
    }

    fn silences(items: &[(f64, f64)]) -> Vec<SilenceRange> {
        items
            .iter()
            .map(|(s, e)| SilenceRange {
                start_s: *s,
                end_s: *e,
            })
            .collect()
    }

    // --- aggregate ---

    #[test]
    fn aggregate_dirty_wins() {
        let rules = vec![
            RuleVerdict { id: "a".into(), verdict: Verdict::Clean, reason: String::new() },
            RuleVerdict { id: "b".into(), verdict: Verdict::Risky, reason: String::new() },
            RuleVerdict { id: "c".into(), verdict: Verdict::Dirty, reason: String::new() },
        ];
        assert_eq!(aggregate_verdicts(&rules), Verdict::Dirty);
    }

    #[test]
    fn aggregate_risky_when_no_dirty() {
        let rules = vec![
            RuleVerdict { id: "a".into(), verdict: Verdict::Clean, reason: String::new() },
            RuleVerdict { id: "b".into(), verdict: Verdict::Risky, reason: String::new() },
            RuleVerdict { id: "c".into(), verdict: Verdict::Abstain, reason: String::new() },
        ];
        assert_eq!(aggregate_verdicts(&rules), Verdict::Risky);
    }

    #[test]
    fn aggregate_abstain_when_all_abstain() {
        let rules = vec![
            RuleVerdict { id: "a".into(), verdict: Verdict::Abstain, reason: String::new() },
            RuleVerdict { id: "b".into(), verdict: Verdict::Abstain, reason: String::new() },
        ];
        assert_eq!(aggregate_verdicts(&rules), Verdict::Abstain);
    }

    // --- rule 1: mid-sentence ---

    #[test]
    fn mid_sentence_dirty_when_at_word_interior() {
        let inputs = ContinuityInputs {
            whisper_words: Some(ws(&[("hello", 1.0, 1.5), ("world", 1.6, 2.1)])),
            ..Default::default()
        };
        let v = rule_mid_sentence(1.3, &inputs);
        assert_eq!(v.verdict, Verdict::Dirty);
        assert!(v.reason.contains("hello"));
    }

    #[test]
    fn mid_sentence_clean_at_word_boundary() {
        let inputs = ContinuityInputs {
            whisper_words: Some(ws(&[("hello", 1.0, 1.5)])),
            ..Default::default()
        };
        // Boundary equality is clean (cut at 1.0 or 1.5 is fine).
        assert_eq!(rule_mid_sentence(1.0, &inputs).verdict, Verdict::Clean);
        assert_eq!(rule_mid_sentence(1.5, &inputs).verdict, Verdict::Clean);
    }

    #[test]
    fn mid_sentence_clean_in_silence() {
        let inputs = ContinuityInputs {
            whisper_words: Some(ws(&[("hello", 1.0, 1.5), ("world", 2.0, 2.5)])),
            ..Default::default()
        };
        assert_eq!(rule_mid_sentence(1.7, &inputs).verdict, Verdict::Clean);
    }

    #[test]
    fn mid_sentence_abstains_without_whisper() {
        let inputs = ContinuityInputs::default();
        assert_eq!(rule_mid_sentence(1.0, &inputs).verdict, Verdict::Abstain);
    }

    // --- rule 2: breath-beat ---

    #[test]
    fn breath_beat_risky_when_trim_inside_short_silence() {
        let inputs = ContinuityInputs {
            silences: Some(silences(&[(1.0, 1.3)])), // 0.3s breath beat
            ..Default::default()
        };
        let v = rule_breath_beat(1.15, CutKind::TrimIn, &inputs);
        assert_eq!(v.verdict, Verdict::Risky);
        assert!(v.reason.contains("breath beat"));
    }

    #[test]
    fn breath_beat_clean_when_silence_long_enough() {
        let inputs = ContinuityInputs {
            silences: Some(silences(&[(1.0, 2.0)])), // 1s — real dead air
            ..Default::default()
        };
        assert_eq!(
            rule_breath_beat(1.5, CutKind::TrimOut, &inputs).verdict,
            Verdict::Clean
        );
    }

    #[test]
    fn breath_beat_clean_for_split_kind() {
        let inputs = ContinuityInputs {
            silences: Some(silences(&[(1.0, 1.3)])),
            ..Default::default()
        };
        // Splits don't trigger this rule.
        assert_eq!(
            rule_breath_beat(1.15, CutKind::Cut, &inputs).verdict,
            Verdict::Clean
        );
    }

    #[test]
    fn breath_beat_abstains_without_silences() {
        let inputs = ContinuityInputs::default();
        assert_eq!(
            rule_breath_beat(1.0, CutKind::TrimIn, &inputs).verdict,
            Verdict::Abstain
        );
    }

    // --- rule 3: mid-motion ---

    #[test]
    fn mid_motion_dirty_when_score_high_no_scene_change() {
        let inputs = ContinuityInputs {
            motion_magnitudes: Some(vec![0.1, 0.7, 0.2]),
            ..Default::default()
        };
        // at_s=1.5 → idx 1 → magnitude 0.7 → dirty.
        let v = rule_mid_motion(1.5, &inputs);
        assert_eq!(v.verdict, Verdict::Dirty);
    }

    #[test]
    fn mid_motion_risky_when_score_moderate() {
        let inputs = ContinuityInputs {
            motion_magnitudes: Some(vec![0.1, 0.5, 0.2]),
            ..Default::default()
        };
        let v = rule_mid_motion(1.0, &inputs);
        assert_eq!(v.verdict, Verdict::Risky);
    }

    #[test]
    fn mid_motion_clean_near_scene_change() {
        let inputs = ContinuityInputs {
            motion_magnitudes: Some(vec![0.1, 0.9, 0.2]),
            scene_changes_s: Some(vec![1.4]), // within 0.3 of at_s=1.5
            ..Default::default()
        };
        // High motion but a scene change is right there → editor
        // is cutting on a hard cut, so it's clean.
        let v = rule_mid_motion(1.5, &inputs);
        assert_eq!(v.verdict, Verdict::Clean);
    }

    #[test]
    fn mid_motion_clean_when_score_low() {
        let inputs = ContinuityInputs {
            motion_magnitudes: Some(vec![0.05, 0.1, 0.05]),
            ..Default::default()
        };
        assert_eq!(rule_mid_motion(1.0, &inputs).verdict, Verdict::Clean);
    }

    #[test]
    fn mid_motion_abstains_without_sidecar() {
        let inputs = ContinuityInputs::default();
        assert_eq!(rule_mid_motion(1.0, &inputs).verdict, Verdict::Abstain);
    }

    // --- rule 4: speaker-turn ---

    #[test]
    fn speaker_turn_risky_mid_utterance() {
        let inputs = ContinuityInputs {
            whisper_segments: Some(segs(&[(0.0, 5.0, Some("A"))])),
            ..Default::default()
        };
        let v = rule_speaker_turn(2.5, &inputs);
        assert_eq!(v.verdict, Verdict::Risky);
        assert!(v.reason.contains("A"));
    }

    #[test]
    fn speaker_turn_clean_near_segment_edge() {
        let inputs = ContinuityInputs {
            whisper_segments: Some(segs(&[(0.0, 5.0, Some("A"))])),
            ..Default::default()
        };
        // Within 0.5s of the end → clean.
        assert_eq!(rule_speaker_turn(4.7, &inputs).verdict, Verdict::Clean);
        assert_eq!(rule_speaker_turn(0.2, &inputs).verdict, Verdict::Clean);
    }

    #[test]
    fn speaker_turn_abstains_without_diarization() {
        // Segments present but no speaker_ids → abstain.
        let inputs = ContinuityInputs {
            whisper_segments: Some(segs(&[(0.0, 5.0, None)])),
            ..Default::default()
        };
        assert_eq!(rule_speaker_turn(2.5, &inputs).verdict, Verdict::Abstain);
    }

    #[test]
    fn speaker_turn_abstains_without_segments() {
        let inputs = ContinuityInputs::default();
        assert_eq!(rule_speaker_turn(1.0, &inputs).verdict, Verdict::Abstain);
    }

    // --- rule 5: rhythm ---

    #[test]
    fn rhythm_risky_with_two_nearby_cuts() {
        let inputs = ContinuityInputs {
            nearby_cuts_s: vec![10.5, 11.8],
            ..Default::default()
        };
        // at_s=12.0 → both cuts within ±2.5s → 2 nearby → risky.
        let v = rule_rhythm(12.0, CutKind::TrimIn, &inputs);
        assert_eq!(v.verdict, Verdict::Risky);
    }

    #[test]
    fn rhythm_clean_with_one_nearby_cut() {
        let inputs = ContinuityInputs {
            nearby_cuts_s: vec![10.5],
            ..Default::default()
        };
        assert_eq!(
            rule_rhythm(12.0, CutKind::TrimIn, &inputs).verdict,
            Verdict::Clean
        );
    }

    #[test]
    fn rhythm_clean_for_split_kind() {
        let inputs = ContinuityInputs {
            nearby_cuts_s: vec![10.0, 11.0, 11.5],
            ..Default::default()
        };
        assert_eq!(rule_rhythm(12.0, CutKind::Cut, &inputs).verdict, Verdict::Clean);
    }

    // --- multi-rule integration ---

    #[test]
    fn integration_dirty_overrides_clean() {
        // Mid-sentence cut at 1.3 (dirty) + scene change at 1.4
        // (motion clean) → aggregate is Dirty.
        let inputs = ContinuityInputs {
            whisper_words: Some(ws(&[("hello", 1.0, 1.5)])),
            motion_magnitudes: Some(vec![0.0, 0.0]),
            scene_changes_s: Some(vec![1.4]),
            ..Default::default()
        };
        let v = assess_continuity(1.3, 1.3, CutKind::TrimIn, &inputs);
        assert_eq!(v.verdict, Verdict::Dirty);
        assert_eq!(v.rules.len(), 5);
        assert_eq!(v.rules[0].id, "mid_sentence");
        assert_eq!(v.rules[0].verdict, Verdict::Dirty);
    }
}
