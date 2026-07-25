//! Taste gate: agreement with a professional editor's keep/cut/speed
//! decisions (docs/taste-gate-plan-2026-07-25.md, Phase B).
//!
//! Consumes decision lists produced by `tools/taste-corpus/align.py`
//! (contract: `tools/taste-corpus/decision_list_schema.json`) and scores
//! a PROPOSED list against a GROUND-TRUTH list with deterministic
//! metrics — no LLM anywhere:
//!
//! - **keep/cut agreement**: fraction of 10s windows where the proposed
//!   action matches the professional's.
//! - **cut-boundary F1** at ±0.5s and ±2s tolerances.
//! - **speed agreement**: on windows both sides keep, do they agree on
//!   sped-vs-not and land in the same factor bucket.
//! - **kept-mass Jaccard**: overlap of kept seconds — the structural-
//!   curation measure (pros keep the best 21% and rebuild; time-
//!   compression alone scores poorly here by design).
//!
//! House discipline (study Finding 13: taste polarity does not transfer
//! across houses): scoring two lists from different houses is an ERROR,
//! not a low score.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Scoring window, matching the plan of record.
const WINDOW_S: f64 = 10.0;

/// Boundary-match tolerances (seconds), tight and loose.
const BOUNDARY_TOL_TIGHT_S: f64 = 0.5;
const BOUNDARY_TOL_LOOSE_S: f64 = 2.0;

/// One recovered/proposed editorial decision list. Field-for-field the
/// JSON contract in `tools/taste-corpus/decision_list_schema.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionList {
    /// Schema version; only 1 exists.
    pub version: u32,
    /// Stable pair identifier.
    pub pair_id: String,
    /// Editorial house the decisions belong to.
    pub house: String,
    /// Raw-side reference (filename / id).
    pub raw_ref: String,
    /// Published-side reference (filename / id).
    pub published_ref: String,
    /// Alignment window used during recovery, seconds.
    pub window_s: f64,
    /// Alignment quality rollup.
    pub alignment: Alignment,
    /// The decisions, sorted by raw_span start.
    pub segments: Vec<Segment>,
}

/// Alignment quality rollup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alignment {
    /// Non-empty published windows.
    pub published_windows: u32,
    /// Windows that matched a raw window.
    pub matched_windows: u32,
    /// matched / published.
    pub coverage: f64,
}

/// One keep or cut span over the raw recording.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    /// `keep` or `cut`.
    pub action: Action,
    /// `[start_s, end_s]` in the raw recording.
    pub raw_span: [f64; 2],
    /// Where the kept span landed in the published edit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_span: Option<[f64; 2]>,
    /// Playback factor for keeps (1.0 = realtime).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    /// Cut scope relative to the show's aligned span.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<CutScope>,
    /// Mean alignment confidence for the span.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// Editorial action for a span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// The span survives into the published edit.
    Keep,
    /// The span is removed.
    Cut,
}

/// Where a cut falls relative to the published show's aligned raw span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutScope {
    /// Before the first kept second.
    PreShow,
    /// Between kept spans.
    InShow,
    /// After the last kept second.
    PostShow,
}

/// Taste-gate score for one (ground truth, proposed) pair.
#[derive(Debug, Clone, Serialize)]
pub struct TasteScore {
    /// Pair id of the ground truth.
    pub pair_id: String,
    /// House both lists belong to.
    pub house: String,
    /// Fraction of 10s windows with matching keep/cut action.
    pub keep_cut_agreement: f64,
    /// Cut-boundary F1 at ±0.5s.
    pub boundary_f1_tight: f64,
    /// Cut-boundary F1 at ±2s.
    pub boundary_f1_loose: f64,
    /// On co-kept windows: agreement on sped-vs-not + factor bucket.
    /// `None` when no windows are co-kept.
    pub speed_agreement: Option<f64>,
    /// Jaccard overlap of kept seconds (structural curation).
    pub kept_mass_jaccard: f64,
    /// Windows scored.
    pub windows: u32,
}

/// Errors loading or scoring decision lists.
#[derive(Debug, Error)]
pub enum TasteError {
    /// Reading a decision-list file failed.
    #[error("reading decision list {path}: {source}")]
    Read {
        /// File path.
        path: std::path::PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// The JSON didn't match the contract.
    #[error("invalid decision list: {0}")]
    Json(#[from] serde_json::Error),
    /// Unsupported schema version.
    #[error("unsupported decision-list version {0} (expected 1)")]
    Version(u32),
    /// Ground truth and proposal are from different houses.
    #[error(
        "house mismatch: ground truth '{ground_truth}' vs proposed '{proposed}' — \
         taste polarity does not transfer across houses (study Finding 13); \
         refusing to produce a number that looks comparable but is not"
    )]
    HouseMismatch {
        /// Ground-truth house.
        ground_truth: String,
        /// Proposed-list house.
        proposed: String,
    },
    /// A decision list carries no segments to score.
    #[error("decision list '{0}' has no segments")]
    Empty(String),
}

impl DecisionList {
    /// Load and validate a decision list from JSON.
    pub fn from_json_file(path: impl AsRef<Path>) -> Result<Self, TasteError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| TasteError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let list: Self = serde_json::from_str(&text)?;
        if list.version != 1 {
            return Err(TasteError::Version(list.version));
        }
        if list.segments.is_empty() {
            return Err(TasteError::Empty(list.pair_id));
        }
        Ok(list)
    }

    /// End of the last span — the scored horizon.
    fn raw_end(&self) -> f64 {
        self.segments
            .iter()
            .map(|s| s.raw_span[1])
            .fold(0.0, f64::max)
    }

    /// Action at a given raw time. Any time not inside a keep span is
    /// CUT — including times outside every segment, since the aligner
    /// only emits keep where the published edit demonstrably used the
    /// material. Keep wins overlaps.
    fn action_at(&self, t: f64) -> Action {
        let kept = self.segments.iter().any(|segment| {
            segment.action == Action::Keep && t >= segment.raw_span[0] && t < segment.raw_span[1]
        });
        if kept { Action::Keep } else { Action::Cut }
    }

    /// Speed at a raw time (only meaningful where action is Keep).
    fn speed_at(&self, t: f64) -> f64 {
        for segment in &self.segments {
            if segment.action == Action::Keep && t >= segment.raw_span[0] && t < segment.raw_span[1]
            {
                return segment.speed.unwrap_or(1.0);
            }
        }
        1.0
    }

    /// Sorted cut-boundary timestamps: every keep<->cut transition edge,
    /// i.e. the start and end of each keep span (excluding the scored
    /// horizon's outer edges, which every list trivially shares).
    fn boundaries(&self) -> Vec<f64> {
        let end = self.raw_end();
        let mut edges: Vec<f64> = self
            .segments
            .iter()
            .filter(|s| s.action == Action::Keep)
            .flat_map(|s| [s.raw_span[0], s.raw_span[1]])
            .filter(|&t| t > 0.0 && t < end)
            .collect();
        edges.sort_by(f64::total_cmp);
        edges.dedup_by(|a, b| (*a - *b).abs() < f64::EPSILON);
        edges
    }
}

/// Factor bucket for speed agreement: 1.0 / (1.0,1.5] / (1.5,2.0] / >2.0.
fn speed_bucket(factor: f64) -> u8 {
    if factor <= 1.0 {
        0
    } else if factor <= 1.5 {
        1
    } else if factor <= 2.0 {
        2
    } else {
        3
    }
}

/// Greedy one-to-one boundary matching within `tol` seconds -> F1.
fn boundary_f1(ground: &[f64], proposed: &[f64], tol: f64) -> f64 {
    if ground.is_empty() && proposed.is_empty() {
        return 1.0;
    }
    if ground.is_empty() || proposed.is_empty() {
        return 0.0;
    }
    let mut used = vec![false; proposed.len()];
    let mut matched = 0usize;
    for &g in ground {
        let mut best: Option<(usize, f64)> = None;
        for (i, &p) in proposed.iter().enumerate() {
            if used[i] {
                continue;
            }
            let d = (g - p).abs();
            if d <= tol && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        if let Some((i, _)) = best {
            used[i] = true;
            matched += 1;
        }
    }
    let precision = matched as f64 / proposed.len() as f64;
    let recall = matched as f64 / ground.len() as f64;
    if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    }
}

/// Score a proposed decision list against ground truth.
pub fn score(ground: &DecisionList, proposed: &DecisionList) -> Result<TasteScore, TasteError> {
    if ground.house != proposed.house {
        return Err(TasteError::HouseMismatch {
            ground_truth: ground.house.clone(),
            proposed: proposed.house.clone(),
        });
    }

    let horizon = ground.raw_end().max(proposed.raw_end());
    let windows = (horizon / WINDOW_S).ceil() as u32;

    let mut action_matches = 0u32;
    let mut co_kept = 0u32;
    let mut speed_matches = 0u32;
    let mut kept_ground_s = 0.0f64;
    let mut kept_proposed_s = 0.0f64;
    let mut kept_both_s = 0.0f64;

    for w in 0..windows {
        let t = (w as f64 + 0.5) * WINDOW_S;
        let ga = ground.action_at(t);
        let pa = proposed.action_at(t);
        if ga == pa {
            action_matches += 1;
        }
        if ga == Action::Keep {
            kept_ground_s += WINDOW_S;
        }
        if pa == Action::Keep {
            kept_proposed_s += WINDOW_S;
        }
        if ga == Action::Keep && pa == Action::Keep {
            kept_both_s += WINDOW_S;
            co_kept += 1;
            if speed_bucket(ground.speed_at(t)) == speed_bucket(proposed.speed_at(t)) {
                speed_matches += 1;
            }
        }
    }

    let union = kept_ground_s + kept_proposed_s - kept_both_s;
    let ground_bounds = ground.boundaries();
    let proposed_bounds = proposed.boundaries();

    Ok(TasteScore {
        pair_id: ground.pair_id.clone(),
        house: ground.house.clone(),
        keep_cut_agreement: if windows == 0 {
            0.0
        } else {
            f64::from(action_matches) / f64::from(windows)
        },
        boundary_f1_tight: boundary_f1(&ground_bounds, &proposed_bounds, BOUNDARY_TOL_TIGHT_S),
        boundary_f1_loose: boundary_f1(&ground_bounds, &proposed_bounds, BOUNDARY_TOL_LOOSE_S),
        speed_agreement: (co_kept > 0).then(|| f64::from(speed_matches) / f64::from(co_kept)),
        kept_mass_jaccard: if union > 0.0 {
            kept_both_s / union
        } else {
            1.0
        },
        windows,
    })
}

/// Lower an EDITED project timeline into a proposed [`DecisionList`]
/// over one raw asset — the bridge that lets the agent's actual output
/// be taste-scored against professional ground truth.
///
/// Every video-track clip whose external media reference contains
/// `raw_ref` contributes a keep span (its `source_range` in media
/// seconds) at the factor of its `montage.speed` effect (default 1.0).
/// The complement over `[0, raw_duration_s]` becomes cut segments,
/// scoped pre/in/post show. Overlapping keeps are left as-is: the
/// scorer's rasterization treats keep-wins-overlap.
pub fn decision_list_from_timeline(
    timeline: &montage_proto::otio::Timeline,
    raw_ref: &str,
    raw_duration_s: f64,
    house: &str,
    pair_id: &str,
) -> Result<DecisionList, TasteError> {
    use montage_proto::otio::{MediaReference, StackChild, TrackChild, TrackKind};

    let mut keeps: Vec<Segment> = Vec::new();
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        if track.kind != TrackKind::Video {
            continue;
        }
        for track_child in &track.children {
            let TrackChild::Clip(clip) = track_child else {
                continue;
            };
            let MediaReference::External(reference) = &clip.media_reference else {
                continue;
            };
            if !reference.target_url.contains(raw_ref) {
                continue;
            }
            let Some(range) = clip.source_range else {
                continue;
            };
            let start = range.start_time.to_seconds();
            let end = start + range.duration.to_seconds();
            if end <= start {
                continue;
            }
            let speed = clip
                .effects
                .iter()
                .find(|effect| effect.effect_name == montage_effects::SPEED)
                .and_then(|effect| effect.metadata.get("factor"))
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(1.0);
            keeps.push(Segment {
                action: Action::Keep,
                raw_span: [start, end],
                published_span: None,
                speed: Some(speed),
                scope: None,
                confidence: None,
            });
        }
    }

    if keeps.is_empty() {
        return Err(TasteError::Empty(format!(
            "{pair_id} (no video clips referencing '{raw_ref}')"
        )));
    }
    keeps.sort_by(|a, b| a.raw_span[0].total_cmp(&b.raw_span[0]));

    // Complement -> cuts, against the merged keep coverage.
    let mut merged: Vec<[f64; 2]> = Vec::new();
    for keep in &keeps {
        match merged.last_mut() {
            Some(last) if keep.raw_span[0] <= last[1] => {
                last[1] = last[1].max(keep.raw_span[1]);
            }
            _ => merged.push(keep.raw_span),
        }
    }
    let first_kept = merged[0][0];
    let last_kept = merged[merged.len() - 1][1];
    let mut segments = keeps;
    let mut cursor = 0.0;
    for span in &merged {
        if span[0] > cursor {
            let scope = if span[0] <= first_kept {
                CutScope::PreShow
            } else {
                CutScope::InShow
            };
            segments.push(Segment {
                action: Action::Cut,
                raw_span: [cursor, span[0]],
                published_span: None,
                speed: None,
                scope: Some(scope),
                confidence: None,
            });
        }
        cursor = cursor.max(span[1]);
    }
    if raw_duration_s > cursor {
        let scope = if cursor >= last_kept {
            CutScope::PostShow
        } else {
            CutScope::InShow
        };
        segments.push(Segment {
            action: Action::Cut,
            raw_span: [cursor, raw_duration_s],
            published_span: None,
            speed: None,
            scope: Some(scope),
            confidence: None,
        });
    }
    segments.sort_by(|a, b| a.raw_span[0].total_cmp(&b.raw_span[0]));

    Ok(DecisionList {
        version: 1,
        pair_id: pair_id.to_string(),
        house: house.to_string(),
        raw_ref: raw_ref.to_string(),
        published_ref: "proposed:timeline".to_string(),
        // Not an alignment product; carry the scorer window so the
        // document stays schema-valid (window_s must be > 0).
        window_s: WINDOW_S,
        alignment: Alignment {
            published_windows: 0,
            matched_windows: 0,
            coverage: 0.0,
        },
        segments,
    })
}

/// The keep-everything baseline: one realtime keep spanning the ground
/// truth's horizon. The floor any real editor proposal must beat — it
/// maximizes keep recall and fails structural curation by construction.
pub fn keep_everything_baseline(ground: &DecisionList) -> DecisionList {
    DecisionList {
        version: 1,
        pair_id: format!("{}-baseline-keep-all", ground.pair_id),
        house: ground.house.clone(),
        raw_ref: ground.raw_ref.clone(),
        published_ref: "baseline:keep-everything".to_string(),
        window_s: ground.window_s,
        alignment: Alignment {
            published_windows: 0,
            matched_windows: 0,
            coverage: 0.0,
        },
        segments: vec![Segment {
            action: Action::Keep,
            raw_span: [0.0, ground.raw_end()],
            published_span: Some([0.0, ground.raw_end()]),
            speed: Some(1.0),
            scope: None,
            confidence: None,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keep(a: f64, b: f64, speed: f64) -> Segment {
        Segment {
            action: Action::Keep,
            raw_span: [a, b],
            published_span: None,
            speed: Some(speed),
            scope: None,
            confidence: None,
        }
    }

    fn cut(a: f64, b: f64, scope: CutScope) -> Segment {
        Segment {
            action: Action::Cut,
            raw_span: [a, b],
            published_span: None,
            speed: None,
            scope: Some(scope),
            confidence: None,
        }
    }

    fn list(pair_id: &str, house: &str, segments: Vec<Segment>) -> DecisionList {
        DecisionList {
            version: 1,
            pair_id: pair_id.into(),
            house: house.into(),
            raw_ref: "raw".into(),
            published_ref: "pub".into(),
            window_s: 15.0,
            alignment: Alignment {
                published_windows: 10,
                matched_windows: 10,
                coverage: 1.0,
            },
            segments,
        }
    }

    /// Ground truth used across tests: keep 0-300 @1.0, cut 300-600,
    /// keep 600-900 @1.5, cut 900-1200 (post).
    fn ground() -> DecisionList {
        list(
            "gt",
            "testhouse",
            vec![
                keep(0.0, 300.0, 1.0),
                cut(300.0, 600.0, CutScope::InShow),
                keep(600.0, 900.0, 1.5),
                cut(900.0, 1200.0, CutScope::PostShow),
            ],
        )
    }

    #[test]
    fn identical_lists_score_perfect() {
        let g = ground();
        let s = score(&g, &g).unwrap_or_else(|e| panic!("same-house scoring succeeds: {e}"));
        assert_eq!(s.keep_cut_agreement, 1.0);
        assert_eq!(s.boundary_f1_tight, 1.0);
        assert_eq!(s.boundary_f1_loose, 1.0);
        assert_eq!(s.speed_agreement, Some(1.0));
        assert_eq!(s.kept_mass_jaccard, 1.0);
    }

    #[test]
    fn keep_everything_baseline_scores_partial() {
        let g = ground();
        let b = keep_everything_baseline(&g);
        let s = score(&g, &b).unwrap_or_else(|e| panic!("baseline scores: {e}"));
        // Baseline keeps all 1200s; ground keeps 600s -> half the windows
        // agree on action, and kept-mass Jaccard is 600/1200.
        assert!((s.keep_cut_agreement - 0.5).abs() < 1e-9);
        assert!((s.kept_mass_jaccard - 0.5).abs() < 1e-9);
        // Baseline has no interior boundaries; ground has several.
        assert_eq!(s.boundary_f1_loose, 0.0);
        // Co-kept windows: ground's sped 600-900 run disagrees with the
        // baseline's 1.0 (bucket mismatch on 300 of 600 co-kept seconds).
        assert_eq!(s.speed_agreement, Some(0.5));
    }

    #[test]
    fn boundary_f1_tolerances_are_ordered() {
        let g = ground();
        // Same structure, boundaries off by 1s: misses tight (0.5s),
        // hits loose (2s).
        let p = list(
            "prop",
            "testhouse",
            vec![
                keep(0.0, 301.0, 1.0),
                cut(301.0, 599.0, CutScope::InShow),
                keep(599.0, 901.0, 1.5),
                cut(901.0, 1200.0, CutScope::PostShow),
            ],
        );
        let s = score(&g, &p).unwrap_or_else(|e| panic!("scores: {e}"));
        assert_eq!(s.boundary_f1_tight, 0.0);
        assert_eq!(s.boundary_f1_loose, 1.0);
    }

    #[test]
    fn house_mismatch_is_an_error_not_a_number() {
        let g = ground();
        let mut p = ground();
        p.house = "otherhouse".into();
        let Err(err) = score(&g, &p) else {
            panic!("cross-house must refuse");
        };
        assert!(matches!(err, TasteError::HouseMismatch { .. }));
    }

    #[test]
    fn speed_bucket_edges() {
        assert_eq!(speed_bucket(1.0), 0);
        assert_eq!(speed_bucket(1.2), 1);
        assert_eq!(speed_bucket(1.5), 1);
        assert_eq!(speed_bucket(1.51), 2);
        assert_eq!(speed_bucket(2.0), 2);
        assert_eq!(speed_bucket(2.45), 3);
    }

    #[test]
    fn round_trips_real_aligner_output_shape() {
        // Field-for-field parse of the aligner's JSON shape, including
        // optional fields present/absent per action.
        let json = r#"{
          "version": 1,
          "pair_id": "brink",
          "house": "technologia",
          "raw_ref": "raw_brink.vtt",
          "published_ref": "Iv5_Udbilso",
          "window_s": 15.0,
          "alignment": {"published_windows": 100, "matched_windows": 95, "coverage": 0.95},
          "segments": [
            {"action": "cut", "raw_span": [0.0, 1455.0], "scope": "pre_show"},
            {"action": "keep", "raw_span": [1455.0, 1800.0],
             "published_span": [0.0, 300.0], "speed": 1.15, "confidence": 0.9}
          ]
        }"#;
        let list: DecisionList =
            serde_json::from_str(json).unwrap_or_else(|e| panic!("parses: {e}"));
        assert_eq!(list.segments.len(), 2);
        assert_eq!(list.segments[0].scope, Some(CutScope::PreShow));
        assert_eq!(list.segments[1].speed, Some(1.15));
    }

    // ---- timeline -> decision-list lowering ----

    fn timeline_with_clips(clips: Vec<montage_proto::otio::Clip>) -> montage_proto::otio::Timeline {
        use montage_proto::otio::{Stack, StackChild, Timeline, Track, TrackChild, TrackKind};
        let mut track = Track::empty("V1", TrackKind::Video);
        for clip in clips {
            track.children.push(TrackChild::Clip(clip));
        }
        let mut timeline = Timeline::empty("t");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        timeline.tracks = stack;
        timeline
    }

    fn raw_clip(name: &str, url: &str, start_s: f64, dur_s: f64) -> montage_proto::otio::Clip {
        use montage_proto::otio::{
            Clip, ExternalReference, MediaReference, RationalTime, TimeRange,
        };
        let mut clip = Clip::empty(name.to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new(url));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(start_s * 30.0, 30.0),
            RationalTime::new(dur_s * 30.0, 30.0),
        ));
        clip
    }

    #[test]
    fn timeline_lowers_to_keeps_speeds_and_scoped_cuts() {
        // Two keeps over the raw (10-100 @1.0, 200-300 @1.5x), a b-roll
        // clip on another asset (ignored), horizon 400s.
        let mut sped = raw_clip("clip-1", "raw/x.mp4", 200.0, 100.0);
        let mut effect = montage_proto::otio::Effect::new(montage_effects::SPEED);
        effect
            .metadata
            .insert("factor".to_string(), serde_json::json!(1.5));
        sped.effects.push(effect);
        let timeline = timeline_with_clips(vec![
            raw_clip("clip-0", "raw/x.mp4", 10.0, 90.0),
            sped,
            raw_clip("broll-0", "raw/other.mp4", 0.0, 50.0),
        ]);

        let list =
            decision_list_from_timeline(&timeline, "raw/x.mp4", 400.0, "testhouse", "pair-1")
                .unwrap_or_else(|e| panic!("lowering succeeds: {e}"));

        let keeps: Vec<&Segment> = list
            .segments
            .iter()
            .filter(|s| s.action == Action::Keep)
            .collect();
        assert_eq!(keeps.len(), 2, "b-roll asset must not contribute keeps");
        assert_eq!(keeps[0].raw_span, [10.0, 100.0]);
        assert_eq!(keeps[0].speed, Some(1.0));
        assert_eq!(keeps[1].raw_span, [200.0, 300.0]);
        assert_eq!(keeps[1].speed, Some(1.5));

        let cuts: Vec<&Segment> = list
            .segments
            .iter()
            .filter(|s| s.action == Action::Cut)
            .collect();
        assert_eq!(cuts.len(), 3);
        assert_eq!(cuts[0].raw_span, [0.0, 10.0]);
        assert_eq!(cuts[0].scope, Some(CutScope::PreShow));
        assert_eq!(cuts[1].raw_span, [100.0, 200.0]);
        assert_eq!(cuts[1].scope, Some(CutScope::InShow));
        assert_eq!(cuts[2].raw_span, [300.0, 400.0]);
        assert_eq!(cuts[2].scope, Some(CutScope::PostShow));
    }

    #[test]
    fn lowered_timeline_scores_against_ground_truth() {
        // End-to-end: a timeline that reproduces the `ground()` fixture's
        // decisions exactly must score perfect agreement.
        let mut sped = raw_clip("clip-1", "raw/x.mp4", 600.0, 300.0);
        let mut effect = montage_proto::otio::Effect::new(montage_effects::SPEED);
        effect
            .metadata
            .insert("factor".to_string(), serde_json::json!(1.5));
        sped.effects.push(effect);
        let timeline = timeline_with_clips(vec![raw_clip("clip-0", "raw/x.mp4", 0.0, 300.0), sped]);

        let proposed =
            decision_list_from_timeline(&timeline, "raw/x.mp4", 1200.0, "testhouse", "gt")
                .unwrap_or_else(|e| panic!("lowering succeeds: {e}"));
        let s = score(&ground(), &proposed).unwrap_or_else(|e| panic!("scores: {e}"));
        assert_eq!(s.keep_cut_agreement, 1.0);
        assert_eq!(s.boundary_f1_tight, 1.0);
        assert_eq!(s.speed_agreement, Some(1.0));
        assert_eq!(s.kept_mass_jaccard, 1.0);
    }

    #[test]
    fn timeline_without_matching_clips_is_an_error() {
        let timeline = timeline_with_clips(vec![raw_clip("c", "raw/other.mp4", 0.0, 10.0)]);
        let result = decision_list_from_timeline(&timeline, "raw/x.mp4", 100.0, "h", "p");
        assert!(matches!(result, Err(TasteError::Empty(_))));
    }
}
