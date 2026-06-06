//! Pure speed-ramp / time-remap math for `plan_speed_ramp`.
//!
//! This module owns all curve math with NO I/O and NO async. It turns a
//! ramp intent ([`RampShape`]) plus bounded controls ([`RampRequest`])
//! into a `Vec<`[`RampPoint`]`>` that satisfies every rule the renderer's
//! `read_time_remap` enforces (`crates/render/src/timeline.rs`), or returns
//! an `Err` naming the violated rule.
//!
//! THE CORE MATH (the #1 bug risk): playback speed across a segment is
//! `Δsource / Δtimeline`. So `timeline_time_s` is the OUTPUT clock and
//! `source_time_s` is how far into the media we have consumed. Consuming
//! 2s of source over 1s of timeline plays 2× (fast); consuming 0.5s of
//! source over 1s of timeline plays 0.5× (slow-mo). [`integrate_curve`]
//! walks the source axis and accumulates `timeline += Δsource / factor`,
//! so a SLOWER factor (e.g. 0.3) produces MORE timeline time — slow motion.

use serde::Serialize;

/// Smallest legitimate sustained slow-mo factor (5% = 20× slow). Any
/// non-freeze factor is clamped to at least this when sanitizing controls,
/// so it doubles as the near-freeze division cutoff: anything slower is a
/// freeze, not real slow motion.
pub const MIN_LEGAL_FACTOR: f64 = 0.05;
/// Near-freeze cutoff used by [`integrate_curve`]: a per-segment factor
/// at/below this is treated as a freeze (zero timeline advance) instead of
/// dividing, so a `HoldFreeze` plateau's eased edges can't spike the
/// timeline. Set to [`MIN_LEGAL_FACTOR`] so legitimate slow-mo is untouched.
const FREEZE_CUTOFF: f64 = MIN_LEGAL_FACTOR;
/// Default slowest sustained factor in the bell (30% = slow-mo).
pub const DEFAULT_MIN_FACTOR: f64 = 0.3;
/// Default fastest sustained factor (200% = fast).
pub const DEFAULT_MAX_FACTOR: f64 = 2.0;
/// Default eased samples per transition.
pub const DEFAULT_SAMPLES: usize = 24;
/// Hard cap on the number of eased samples per transition.
pub const MAX_SAMPLES: usize = 64;
/// Hard cap on the total number of emitted curve points (YAGNI guard).
pub const MAX_POINTS: usize = 200;
/// Smallest positive source step we permit (keeps the axis strictly
/// increasing in the face of float drift).
const EPS: f64 = 1e-6;

/// A point on the emitted source→timeline curve. Field names mirror the
/// render-side `TimeRemapPoint` exactly so serialization is 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct RampPoint {
    /// How far into the media (clip-local source seconds) this point sits.
    pub source_time_s: f64,
    /// Output timeline seconds at which that source position is reached.
    pub timeline_time_s: f64,
}

/// LLM-friendly ramp intents. Maps a craft word to a speed profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RampShape {
    /// fast → slow-hold → fast (the 200→65→200 bell).
    SlowMoReveal,
    /// accelerate into a beat boundary, then resume unity.
    RampToBeat,
    /// brief speed-up burst, return to unity.
    PunchThen,
    /// unity → freeze plateau → unity.
    HoldFreeze,
}

/// Inputs already parsed/clamped by the tool layer. The pure core never
/// reads tool args directly.
#[derive(Debug, Clone)]
pub struct RampRequest {
    /// Which speed profile to emit.
    pub shape: RampShape,
    /// Clip source duration in seconds (from ffprobe; `> 0`, finite).
    pub source_duration_s: f64,
    /// Slowest sustained factor in the bell (e.g. 0.3 = 30%). In `(0, 1]`.
    pub min_factor: f64,
    /// Fastest sustained factor (e.g. 2.0 = 200%). `>= 1.0`.
    pub max_factor: f64,
    /// Source seconds the held/slow (or freeze) section spans. `>= 0`.
    pub hold_s: f64,
    /// For `RampToBeat`: source-local seconds to align the ramp end to.
    pub beat_at_s: Option<f64>,
    /// Eased sampling resolution (samples per transition). Bounded.
    pub samples: usize,
}

impl RampRequest {
    /// Build a request with corpus-derived defaults for a duration + shape.
    pub fn with_defaults(shape: RampShape, source_duration_s: f64) -> Self {
        Self {
            shape,
            source_duration_s,
            min_factor: DEFAULT_MIN_FACTOR,
            max_factor: DEFAULT_MAX_FACTOR,
            hold_s: source_duration_s * 0.25,
            beat_at_s: None,
            samples: DEFAULT_SAMPLES,
        }
    }

    /// Sanitized copy: clamp every field to its legal range so the math
    /// core never sees a pathological value. Public so the tool layer can
    /// surface the EFFECTIVE clamped controls (min/max factor, hold,
    /// samples) it actually planned with, instead of the raw request.
    pub fn sanitized(&self) -> Self {
        // Duration is validated (finite, > 0) by `plan_ramp_curve` before
        // it is used; here we only clamp the controls that depend on it.
        let duration = self.source_duration_s;
        let min_factor = clamp_finite(self.min_factor, DEFAULT_MIN_FACTOR, MIN_LEGAL_FACTOR, 1.0);
        let max_factor = clamp_finite(self.max_factor, DEFAULT_MAX_FACTOR, 1.0, 8.0);
        let max_hold = (duration * 0.9).max(0.0);
        let hold_s = clamp_finite(self.hold_s, duration * 0.25, 0.0, max_hold);
        let samples = self.samples.clamp(2, MAX_SAMPLES);
        Self {
            shape: self.shape,
            source_duration_s: duration,
            min_factor,
            max_factor,
            hold_s,
            beat_at_s: self.beat_at_s,
            samples,
        }
    }
}

/// Clamp a possibly-non-finite value into `[lo, hi]`, substituting
/// `fallback` when the input is not finite.
fn clamp_finite(value: f64, fallback: f64, lo: f64, hi: f64) -> f64 {
    let v = if value.is_finite() { value } else { fallback };
    v.clamp(lo, hi)
}

/// THE pure entry point. Same input → same output. Returns a curve that
/// satisfies every `read_time_remap` rule, or an `Err` naming the
/// violation.
pub fn plan_ramp_curve(req: &RampRequest) -> Result<Vec<RampPoint>, String> {
    let req = req.sanitized();
    if !(req.source_duration_s.is_finite() && req.source_duration_s > 0.0) {
        return Err(format!(
            "speed_ramp: source_duration_s must be finite and > 0, got {}",
            req.source_duration_s
        ));
    }
    let mut points = integrate_curve(&req);
    enforce_monotonic(&mut points);
    validate_curve(&points)?;
    Ok(points)
}

/// The eased speed profile. `u` in `0..=1` walks the source axis; the
/// returned value is the instantaneous playback factor at that point.
///
/// Easing uses the smoothstep cubic `3u² − 2u³` so transitions ramp in and
/// out smoothly (the corpus's non-negotiable "ease everything"); a flat
/// linear gear-shift is explicitly the wrong shape. A central plateau holds
/// the extreme factor over the `hold` fraction of the source.
pub fn factor_at(shape: RampShape, u: f64, req: &RampRequest) -> f64 {
    let u = u.clamp(0.0, 1.0);
    let hold_frac = if req.source_duration_s > 0.0 {
        (req.hold_s / req.source_duration_s).clamp(0.0, 0.9)
    } else {
        0.0
    };
    match shape {
        RampShape::SlowMoReveal => {
            // unity-ish fast at the edges, eased down to min_factor over a
            // centered plateau, eased back up. A symmetric speed *bell*.
            bell(u, hold_frac, req.max_factor, req.min_factor)
        }
        RampShape::PunchThen => {
            // brief fast burst centered, returning to unity. The inverse
            // direction of SlowMoReveal: edges at unity, peak at max_factor.
            bell(u, hold_frac, 1.0, req.max_factor)
        }
        RampShape::HoldFreeze => {
            // unity → freeze plateau → unity. The plateau factor is a tiny
            // epsilon so Δsource/factor explodes; integrate_curve treats the
            // plateau as a true freeze (zero timeline advance).
            bell(u, hold_frac, 1.0, FREEZE_FACTOR)
        }
        RampShape::RampToBeat => {
            // accelerate from unity up to max_factor by the beat point,
            // then resume unity. Eased ramp-up, then eased ramp-down.
            let beat_u = req
                .beat_at_s
                .filter(|b| b.is_finite() && *b > 0.0)
                .map(|b| (b / req.source_duration_s).clamp(0.05, 0.95))
                .unwrap_or(0.5);
            if u <= beat_u {
                let t = smoothstep(u / beat_u.max(EPS));
                lerp(1.0, req.max_factor, t)
            } else {
                let denom = (1.0 - beat_u).max(EPS);
                let t = smoothstep((u - beat_u) / denom);
                lerp(req.max_factor, 1.0, t)
            }
        }
    }
}

/// The plateau factor used by `HoldFreeze` — small enough that the integrator
/// adds negligible timeline time, producing a flat freeze plateau.
const FREEZE_FACTOR: f64 = 1.0e-4;

/// A symmetric eased "bell": `edge` factor at u=0 and u=1, eased into a
/// centered plateau holding `peak` over the middle `hold_frac` of the axis.
fn bell(u: f64, hold_frac: f64, edge: f64, peak: f64) -> f64 {
    let half_hold = hold_frac / 2.0;
    let lo = 0.5 - half_hold; // plateau start
    let hi = 0.5 + half_hold; // plateau end
    if u <= lo {
        let t = smoothstep(u / lo.max(EPS));
        lerp(edge, peak, t)
    } else if u >= hi {
        let denom = (1.0 - hi).max(EPS);
        let t = smoothstep((u - hi) / denom);
        lerp(peak, edge, t)
    } else {
        peak
    }
}

/// Smoothstep cubic `3t² − 2t³` on a clamped `t`. Eased, NOT linear.
fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear blend between `a` and `b`.
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Convert the instantaneous speed profile into the cumulative
/// source→timeline position curve the renderer consumes.
///
/// Walks the source axis in `samples` steps. For each step the timeline
/// advances by `Δsource / factor`: a slower factor advances the timeline
/// MORE (slow motion), a faster factor advances it less (sped up). Point 0
/// is seeded at `(0, 0)` to satisfy the renderer's origin rule.
pub fn integrate_curve(req: &RampRequest) -> Vec<RampPoint> {
    let steps = req.samples.clamp(2, MAX_SAMPLES);
    let mut points = Vec::with_capacity(steps + 1);
    points.push(RampPoint {
        source_time_s: 0.0,
        timeline_time_s: 0.0,
    });
    let duration = req.source_duration_s;
    let mut timeline = 0.0f64;
    let mut prev_source = 0.0f64;
    for i in 1..=steps {
        let u = i as f64 / steps as f64;
        let source = u * duration;
        let d_source = source - prev_source;
        // Use the factor at the midpoint of the segment for a closer
        // approximation of the eased integral than an endpoint sample.
        let u_mid = (u + (i - 1) as f64 / steps as f64) / 2.0;
        let factor = factor_at(req.shape, u_mid, req).max(FREEZE_FACTOR);
        // Near-freeze guard (finding 3364684048): a `HoldFreeze` plateau
        // eases toward `FREEZE_FACTOR`, so samples just outside the frozen
        // section have tiny-but-nonzero factors. Dividing Δsource by them
        // would balloon the timeline (e.g. /0.002 → a ~hundred-second
        // spike). Any factor at/below `FREEZE_CUTOFF` is therefore treated
        // as a true freeze (zero timeline advance). `FREEZE_CUTOFF` sits at
        // the legitimate slow-mo floor (`MIN_LEGAL_FACTOR`, the
        // `min_factor` clamp lower bound), so real slow motion — e.g. the
        // 0.30× bell — divides normally and is never clamped.
        let d_timeline = if factor < FREEZE_CUTOFF {
            0.0
        } else {
            d_source / factor
        };
        timeline += d_timeline;
        points.push(RampPoint {
            source_time_s: source,
            timeline_time_s: timeline,
        });
        prev_source = source;
    }
    cap_points(points)
}

/// Cap the curve to `MAX_POINTS` by uniform decimation, always keeping the
/// first and last point.
fn cap_points(points: Vec<RampPoint>) -> Vec<RampPoint> {
    if points.len() <= MAX_POINTS {
        return points;
    }
    let n = points.len();
    let last = n - 1;
    let mut out = Vec::with_capacity(MAX_POINTS);
    for i in 0..MAX_POINTS {
        // Map the i-th output slot back onto the source index range.
        let idx = (i * last) / (MAX_POINTS - 1);
        out.push(points[idx]);
    }
    out
}

/// Clamp float drift so `source_time_s` strictly increases and
/// `timeline_time_s` is non-decreasing (the renderer's rules 4–5).
pub fn enforce_monotonic(points: &mut [RampPoint]) {
    if points.is_empty() {
        return;
    }
    // Pin the origin exactly.
    points[0].source_time_s = 0.0;
    points[0].timeline_time_s = 0.0;
    for i in 1..points.len() {
        let prev_source = points[i - 1].source_time_s;
        let prev_timeline = points[i - 1].timeline_time_s;
        let mut s = points[i].source_time_s;
        let mut t = points[i].timeline_time_s;
        if !s.is_finite() || s <= prev_source {
            s = prev_source + EPS;
        }
        if !t.is_finite() || t < prev_timeline {
            t = prev_timeline;
        }
        points[i].source_time_s = s;
        points[i].timeline_time_s = t;
    }
}

/// In-crate port of `read_time_remap`'s 5 contract rules. Returns `Err`
/// naming the first violated rule so the core self-verifies without the
/// render crate.
pub fn validate_curve(points: &[RampPoint]) -> Result<(), String> {
    // Rule 1: at least two points.
    if points.len() < 2 {
        return Err("speed_ramp: curve must contain at least two points".into());
    }
    // Rule 2: every field finite and >= 0.
    for (idx, p) in points.iter().enumerate() {
        if !(p.source_time_s.is_finite() && p.source_time_s >= 0.0) {
            return Err(format!(
                "speed_ramp: curve point #{idx} needs finite non-negative source_time_s, got {}",
                p.source_time_s
            ));
        }
        if !(p.timeline_time_s.is_finite() && p.timeline_time_s >= 0.0) {
            return Err(format!(
                "speed_ramp: curve point #{idx} needs finite non-negative timeline_time_s, got {}",
                p.timeline_time_s
            ));
        }
    }
    // Rule 3: first point is exactly the origin.
    let first = points[0];
    if first.source_time_s.abs() > 1e-9 || first.timeline_time_s.abs() > 1e-9 {
        return Err("speed_ramp: curve must start at source_time_s=0 and timeline_time_s=0".into());
    }
    // Rules 4–5: source strictly increasing, timeline non-decreasing.
    for pair in points.windows(2) {
        if pair[1].source_time_s <= pair[0].source_time_s {
            return Err(
                "speed_ramp: curve source_time_s values must be strictly increasing".into(),
            );
        }
        if pair[1].timeline_time_s < pair[0].timeline_time_s {
            return Err("speed_ramp: curve timeline_time_s values must be non-decreasing".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
