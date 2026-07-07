//! Picture-pass gates: deterministic pass/fail checks over pacing stats,
//! scored against house-style targets (docs/post-house-pipeline.md §2).

use serde::Serialize;

use crate::PacingStats;

/// SaaS-floor rule: no 5-minute window may fall under this rate.
/// Named for the shipped-raw-composite failure it exists to catch.
const SAAS_FLOOR_WINDOW_SECS: f64 = 300.0;
const SAAS_FLOOR_MIN_RATE: f64 = 0.5;

/// Cold-open targets from the study: every DOAC episode peaks in minute
/// 0–1 and opens ≥ 20 cuts/min over the first 90s (measured 26–55).
const COLD_OPEN_WINDOW_SECS: f64 = 90.0;
const COLD_OPEN_MIN_RATE: f64 = 20.0;
const COLD_OPEN_LAST_PEAK_MINUTE: usize = 1;

/// Body pacing is measured after the cold open.
const BODY_START_SECS: f64 = 90.0;

/// Outcome of one gate check.
#[derive(Debug, Clone, Serialize)]
pub struct GateReport {
    /// Stable gate identifier (e.g. "picture.saas_floor").
    pub gate: &'static str,
    /// Whether the program clears the gate.
    pub passed: bool,
    /// The measured value the verdict is based on.
    pub measured: f64,
    /// Human-readable explanation with the target.
    pub detail: String,
}

/// No 5-minute stretch may run under 0.5 cuts/min: catches shipping a raw
/// composite (or any dead editorial zone) before publish.
pub fn saas_floor(stats: &PacingStats) -> GateReport {
    let measured = stats.sparsest_window_rate(SAAS_FLOOR_WINDOW_SECS);
    GateReport {
        gate: "picture.saas_floor",
        passed: measured >= SAAS_FLOOR_MIN_RATE,
        measured,
        detail: format!(
            "sparsest 5-min window runs {measured:.2} cuts/min (floor {SAAS_FLOOR_MIN_RATE})"
        ),
    }
}

/// The program must open with a blitz: peak-density minute at 0–1 and the
/// first 90s at cold-open rate.
pub fn cold_open(stats: &PacingStats) -> GateReport {
    let opening_rate = stats.rate_in_window(0.0, COLD_OPEN_WINDOW_SECS);
    let peak = stats.peak_minute();
    let peak_early = peak.is_some_and(|m| m <= COLD_OPEN_LAST_PEAK_MINUTE);
    let passed = peak_early && opening_rate >= COLD_OPEN_MIN_RATE;
    GateReport {
        gate: "picture.cold_open",
        passed,
        measured: opening_rate,
        detail: format!(
            "first 90s runs {opening_rate:.1} cuts/min (target ≥ {COLD_OPEN_MIN_RATE}), \
             peak minute {peak:?} (target ≤ {COLD_OPEN_LAST_PEAK_MINUTE})"
        ),
    }
}

/// Body pacing (after the cold open) must sit inside the archetype band.
pub fn body_pacing(stats: &PacingStats, band_min: f64, band_max: f64) -> GateReport {
    let measured = stats.rate_in_window(BODY_START_SECS, stats.duration_secs());
    GateReport {
        gate: "picture.body_pacing",
        passed: measured >= band_min && measured <= band_max,
        measured,
        detail: format!("body runs {measured:.1} cuts/min (archetype band {band_min}–{band_max})"),
    }
}
