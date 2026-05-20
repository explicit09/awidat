//! Boundary visual signals loaded from per-asset shot sidecars.
//!
//! The shot indexer writes one record per shot under
//! `index/shot/<asset>.json` with fields like `motion_magnitude`,
//! `dominant_direction`, `whip_pan_score`, `occlusion_score`, and
//! `action_score`. `assess_edit_quality` already reads these for one
//! side of a candidate cut; the transition planner needs both sides
//! and the relationship between them.
//!
//! This module exposes a focused `BoundaryVisualSignals` builder that
//! returns motion- and motion-direction-aware signals at the cut
//! boundary. Color/luminance delta signals are not yet sourced from
//! any sidecar and are intentionally omitted until the indexer
//! produces them.
//!
//! The data shape is deliberately minimal — only what the agent
//! consumes when picking a transition. The richer per-shot composition
//! data stays inside `assess_edit_quality.rs`.

use std::path::Path;

use awidat_index::read_sidecar;
use awidat_proto::index::AssetId;
use awidat_proto::transitions::MotionAlignment;

/// Visual signals at one side of a cut.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SideSignals {
    /// Average motion magnitude for the shot covering this side.
    pub motion_magnitude: Option<f64>,
    /// Inferred screen direction for the shot when the indexer can label one.
    pub motion_direction: Option<MotionAlignment>,
    /// Fast-pan confidence in `[0.0, 1.0]`.
    pub whip_pan_score: Option<f64>,
    /// Occlusion/mask confidence in `[0.0, 1.0]`.
    pub occlusion_score: Option<f64>,
    /// Cut-on-action confidence in `[0.0, 1.0]`.
    pub action_score: Option<f64>,
}

/// Motion-direction relationship between two sides of a cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionMatch {
    /// Both sides share the same direction.
    Aligned,
    /// The sides point in opposite directions on the same axis.
    Opposed,
    /// One side is on a perpendicular axis to the other.
    Orthogonal,
    /// At least one side lacks a direction label.
    Unknown,
}

impl MotionMatch {
    /// Stable lowercase identifier for JSON output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Aligned => "aligned",
            Self::Opposed => "opposed",
            Self::Orthogonal => "orthogonal",
            Self::Unknown => "unknown",
        }
    }
}

/// Motion- and direction-aware signals at a candidate cut boundary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoundaryVisualSignals {
    /// Signals for the outgoing clip just before the cut.
    pub outgoing: SideSignals,
    /// Signals for the incoming clip just after the cut.
    pub incoming: SideSignals,
}

impl BoundaryVisualSignals {
    /// Classify the motion-direction relationship between sides.
    pub fn motion_match(&self) -> MotionMatch {
        match (self.outgoing.motion_direction, self.incoming.motion_direction) {
            (Some(out_dir), Some(in_dir)) if out_dir.agrees_with(in_dir) => MotionMatch::Aligned,
            (Some(out_dir), Some(in_dir)) if out_dir.opposes(in_dir) => MotionMatch::Opposed,
            (Some(_), Some(_)) => MotionMatch::Orthogonal,
            _ => MotionMatch::Unknown,
        }
    }

    /// Confidence in the motion-match classification.
    ///
    /// Built from the lower of the two sides' motion magnitudes when both
    /// are known. Returns `None` when either side lacks a magnitude — a
    /// labelled direction without a magnitude isn't worth weighting.
    pub fn motion_match_confidence(&self) -> Option<f64> {
        let outgoing = self.outgoing.motion_magnitude?;
        let incoming = self.incoming.motion_magnitude?;
        Some(outgoing.min(incoming).clamp(0.0, 1.0))
    }

    /// True when at least one side has zero or near-zero motion.
    pub fn either_side_static(&self) -> bool {
        const STATIC_THRESHOLD: f64 = 0.08;
        let outgoing_static = self
            .outgoing
            .motion_magnitude
            .is_some_and(|m| m < STATIC_THRESHOLD);
        let incoming_static = self
            .incoming
            .motion_magnitude
            .is_some_and(|m| m < STATIC_THRESHOLD);
        outgoing_static || incoming_static
    }
}

/// Load the boundary visual signals for one candidate cut.
///
/// `from_source_end_s` is the source-time of the outgoing clip's last
/// rendered frame; `to_source_start_s` is the source-time of the
/// incoming clip's first rendered frame.
pub fn load_boundary_signals(
    project_root: &Path,
    from_asset: &str,
    from_source_end_s: f64,
    to_asset: &str,
    to_source_start_s: f64,
) -> BoundaryVisualSignals {
    BoundaryVisualSignals {
        outgoing: load_side_signals(project_root, from_asset, from_source_end_s),
        incoming: load_side_signals(project_root, to_asset, to_source_start_s),
    }
}

fn load_side_signals(project_root: &Path, asset_id: &str, source_at_s: f64) -> SideSignals {
    let Ok(sidecar) = read_sidecar(project_root, "shot", &AssetId::new(asset_id.to_string()))
    else {
        return SideSignals::default();
    };
    let Some(shots) = sidecar
        .pointer("/data/shots")
        .or_else(|| sidecar.get("shots"))
        .and_then(|value| value.as_array())
    else {
        return SideSignals::default();
    };
    let Some(shot) = shots.iter().find(|shot| {
        let start = shot
            .get("start_s")
            .or_else(|| shot.get("start"))
            .and_then(|value| value.as_f64())
            .unwrap_or(f64::NEG_INFINITY);
        let end = shot
            .get("end_s")
            .or_else(|| shot.get("end"))
            .and_then(|value| value.as_f64())
            .unwrap_or(f64::INFINITY);
        start <= source_at_s && source_at_s <= end
    }) else {
        return SideSignals::default();
    };

    SideSignals {
        motion_magnitude: shot
            .get("motion_magnitude")
            .and_then(|value| value.as_f64()),
        motion_direction: shot
            .get("dominant_direction")
            .and_then(|value| value.as_str())
            .and_then(MotionAlignment::from_hint),
        whip_pan_score: shot.get("whip_pan_score").and_then(|value| value.as_f64()),
        occlusion_score: shot.get("occlusion_score").and_then(|value| value.as_f64()),
        action_score: shot.get("action_score").and_then(|value| value.as_f64()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_shot_sidecar(project_root: &Path, asset_id: &str, shots: serde_json::Value) {
        let path = project_root
            .join("index")
            .join("shot")
            .join(format!("{asset_id}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let payload = serde_json::json!({ "data": { "shots": shots } });
        std::fs::write(path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
    }

    #[test]
    fn loads_motion_direction_from_shot_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        write_shot_sidecar(
            dir.path(),
            "raw_a",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.6, "dominant_direction": "left"}
            ]),
        );
        write_shot_sidecar(
            dir.path(),
            "raw_b",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.4, "dominant_direction": "left"}
            ]),
        );
        let signals = load_boundary_signals(dir.path(), "raw_a", 2.0, "raw_b", 1.0);
        assert_eq!(signals.outgoing.motion_magnitude, Some(0.6));
        assert_eq!(signals.outgoing.motion_direction, Some(MotionAlignment::Left));
        assert_eq!(signals.incoming.motion_magnitude, Some(0.4));
        assert_eq!(signals.motion_match(), MotionMatch::Aligned);
        assert_eq!(signals.motion_match_confidence(), Some(0.4));
    }

    #[test]
    fn classifies_opposed_motion() {
        let dir = tempfile::tempdir().unwrap();
        write_shot_sidecar(
            dir.path(),
            "raw_a",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.7, "dominant_direction": "left"}
            ]),
        );
        write_shot_sidecar(
            dir.path(),
            "raw_b",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.6, "dominant_direction": "right"}
            ]),
        );
        let signals = load_boundary_signals(dir.path(), "raw_a", 3.0, "raw_b", 0.5);
        assert_eq!(signals.motion_match(), MotionMatch::Opposed);
    }

    #[test]
    fn returns_unknown_when_direction_label_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_shot_sidecar(
            dir.path(),
            "raw_a",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.6}
            ]),
        );
        let signals = load_boundary_signals(dir.path(), "raw_a", 2.0, "raw_missing", 1.0);
        assert_eq!(signals.motion_match(), MotionMatch::Unknown);
        assert_eq!(signals.incoming, SideSignals::default());
    }

    #[test]
    fn detects_static_side() {
        let dir = tempfile::tempdir().unwrap();
        write_shot_sidecar(
            dir.path(),
            "raw_a",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.02, "dominant_direction": "left"}
            ]),
        );
        write_shot_sidecar(
            dir.path(),
            "raw_b",
            serde_json::json!([
                {"start_s": 0.0, "end_s": 5.0, "motion_magnitude": 0.6, "dominant_direction": "left"}
            ]),
        );
        let signals = load_boundary_signals(dir.path(), "raw_a", 2.0, "raw_b", 0.5);
        assert!(signals.either_side_static());
    }
}
