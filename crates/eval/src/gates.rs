//! Picture-pass gates: deterministic pass/fail checks over pacing stats,
//! scored against a house style profile (docs/post-house-pipeline.md §§1–2).
//! Every threshold traces to a named profile document, not a constant here.

use serde::Serialize;

use crate::{HouseProfile, PacingStats};

/// Body pacing is measured after the cold-open window (fixed structural
/// boundary; the requirement itself lives in the profile).
const BODY_START_SECS: f64 = 90.0;

/// Outcome of one gate check.
#[derive(Debug, Clone, Serialize)]
pub struct GateReport {
    /// Stable gate identifier (e.g. "picture.floor").
    pub gate: &'static str,
    /// Whether the program clears the gate.
    pub passed: bool,
    /// The measured value the verdict is based on.
    pub measured: f64,
    /// Human-readable explanation with the target.
    pub detail: String,
}

fn unknown_archetype(gate: &'static str, profile: &HouseProfile, archetype: &str) -> GateReport {
    GateReport {
        gate,
        passed: false,
        measured: f64::NAN,
        detail: format!(
            "archetype {archetype:?} not defined by profile {:?} — failing closed",
            profile.name
        ),
    }
}

/// Editorial-dead-zone floor: no profile-specified window may run under the
/// archetype's minimum rate. Catches shipping a raw composite (or any dead
/// zone) before publish. Archetypes without a floor pass as skipped.
pub fn floor(stats: &PacingStats, profile: &HouseProfile, archetype: &str) -> GateReport {
    let Some(targets) = profile.archetype(archetype) else {
        return unknown_archetype("picture.floor", profile, archetype);
    };
    let Some(spec) = &targets.floor else {
        return GateReport {
            gate: "picture.floor",
            passed: true,
            measured: f64::NAN,
            detail: format!("archetype {archetype:?} has no floor — skipped"),
        };
    };
    let measured = stats.sparsest_window_rate(spec.window_secs);
    GateReport {
        gate: "picture.floor",
        passed: measured >= spec.min_rate,
        measured,
        detail: format!(
            "sparsest {:.0}s window runs {measured:.2} cuts/min (floor {})",
            spec.window_secs, spec.min_rate
        ),
    }
}

/// The program must open the way the profile demands: peak-density minute
/// early and a blitz opening window. Profiles without a cold-open
/// requirement pass as skipped.
pub fn cold_open(stats: &PacingStats, profile: &HouseProfile) -> GateReport {
    let Some(spec) = &profile.cold_open else {
        return GateReport {
            gate: "picture.cold_open",
            passed: true,
            measured: f64::NAN,
            detail: format!(
                "profile {:?} has no cold-open requirement — skipped",
                profile.name
            ),
        };
    };
    let opening_rate = stats.rate_in_window(0.0, spec.window_secs);
    let peak = stats.peak_minute();
    let peak_early = peak.is_some_and(|m| m <= spec.last_peak_minute);
    GateReport {
        gate: "picture.cold_open",
        passed: peak_early && opening_rate >= spec.min_rate,
        measured: opening_rate,
        detail: format!(
            "first {:.0}s runs {opening_rate:.1} cuts/min (target ≥ {}), peak minute {peak:?} \
             (target ≤ {})",
            spec.window_secs, spec.min_rate, spec.last_peak_minute
        ),
    }
}

/// Body pacing (after the cold open) must sit inside the archetype band.
pub fn body_pacing(stats: &PacingStats, profile: &HouseProfile, archetype: &str) -> GateReport {
    let Some(targets) = profile.archetype(archetype) else {
        return unknown_archetype("picture.body_pacing", profile, archetype);
    };
    let (band_min, band_max) = targets.body_band;
    let measured = stats.rate_in_window(BODY_START_SECS, stats.duration_secs());
    GateReport {
        gate: "picture.body_pacing",
        passed: measured >= band_min && measured <= band_max,
        measured,
        detail: format!("body runs {measured:.1} cuts/min (archetype band {band_min}–{band_max})"),
    }
}
