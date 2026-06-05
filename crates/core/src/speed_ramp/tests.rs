//! Unit tests for the pure speed-ramp math core. Split out of
//! `mod.rs` to keep the implementation file within the LOC budget.

#![cfg(test)]

use super::*;

fn req(shape: RampShape, duration: f64) -> RampRequest {
    RampRequest::with_defaults(shape, duration)
}

// Step 1: origin point.
#[test]
fn curve_starts_at_origin() {
    let curve = plan_ramp_curve(&req(RampShape::SlowMoReveal, 4.0)).unwrap();
    assert_eq!(
        curve[0],
        RampPoint {
            source_time_s: 0.0,
            timeline_time_s: 0.0
        }
    );
}

// Step 2: strictly source-increasing, timeline non-decreasing for every
// shape.
#[test]
fn curve_is_monotonic_for_every_shape() {
    for shape in [
        RampShape::SlowMoReveal,
        RampShape::RampToBeat,
        RampShape::PunchThen,
        RampShape::HoldFreeze,
    ] {
        let mut r = req(shape, 5.0);
        r.beat_at_s = Some(3.0);
        let curve = plan_ramp_curve(&r).unwrap();
        for w in curve.windows(2) {
            assert!(
                w[1].source_time_s > w[0].source_time_s,
                "{shape:?} source not strictly increasing: {w:?}"
            );
            assert!(
                w[1].timeline_time_s >= w[0].timeline_time_s,
                "{shape:?} timeline decreased: {w:?}"
            );
        }
    }
}

// Step 3: slow-mo segment slows the timeline — a small source span in the
// bell center maps to a LARGER timeline span.
#[test]
fn slow_mo_hold_stretches_timeline() {
    let r = req(RampShape::SlowMoReveal, 4.0);
    let curve = plan_ramp_curve(&r).unwrap();
    // Find the window straddling the center (u ~ 0.5).
    let mid_source = r.source_duration_s * 0.5;
    let win = curve
        .windows(2)
        .find(|w| w[0].source_time_s <= mid_source && w[1].source_time_s >= mid_source)
        .expect("a window crosses the center");
    let d_source = win[1].source_time_s - win[0].source_time_s;
    let d_timeline = win[1].timeline_time_s - win[0].timeline_time_s;
    assert!(
        d_timeline > d_source,
        "slow-mo must stretch: d_source={d_source}, d_timeline={d_timeline}"
    );
}

// Step 4: fast segment speeds it up — PunchThen center has source
// advancing faster than timeline.
#[test]
fn punch_then_center_compresses_timeline() {
    let r = req(RampShape::PunchThen, 4.0);
    let curve = plan_ramp_curve(&r).unwrap();
    let mid_source = r.source_duration_s * 0.5;
    let win = curve
        .windows(2)
        .find(|w| w[0].source_time_s <= mid_source && w[1].source_time_s >= mid_source)
        .expect("a window crosses the center");
    let d_source = win[1].source_time_s - win[0].source_time_s;
    let d_timeline = win[1].timeline_time_s - win[0].timeline_time_s;
    assert!(
        d_source > d_timeline,
        "fast burst must compress: d_source={d_source}, d_timeline={d_timeline}"
    );
}

// Step 5: HoldFreeze emits a flat timeline plateau — at least one window
// holds timeline constant while source advances.
#[test]
fn hold_freeze_emits_flat_plateau() {
    let r = req(RampShape::HoldFreeze, 4.0);
    let curve = plan_ramp_curve(&r).unwrap();
    let flat = curve.windows(2).any(|w| {
        w[1].source_time_s > w[0].source_time_s
            && (w[1].timeline_time_s - w[0].timeline_time_s).abs() < 1e-6
    });
    assert!(
        flat,
        "HoldFreeze must produce a flat freeze plateau: {curve:?}"
    );
}

// Finding 3364684048: near-freeze eased samples just outside the frozen
// plateau have tiny-but-nonzero factors; dividing Δsource by them would
// produce an extreme timeline spike. The integrator must treat any
// near-freeze factor as a freeze so no single timeline step blows up,
// while the plateau stays flat.
#[test]
fn hold_freeze_has_no_timeline_spike() {
    let r = req(RampShape::HoldFreeze, 4.0);
    let curve = plan_ramp_curve(&r).unwrap();

    // Total timeline never balloons past the real-time mapping plus a
    // sane allowance — a near-freeze spike would push one step into the
    // hundreds of seconds for a 4 s clip.
    let span_source = r.source_duration_s;
    let max_reasonable_step = span_source; // no single step exceeds the whole clip
    for w in curve.windows(2) {
        let d_timeline = w[1].timeline_time_s - w[0].timeline_time_s;
        assert!(
            d_timeline <= max_reasonable_step,
            "near-freeze timeline spike: Δtimeline={d_timeline} over Δsource={} (window {w:?})",
            w[1].source_time_s - w[0].source_time_s
        );
    }

    // The freeze plateau is still genuinely flat (timeline held constant
    // while source advances).
    let flat = curve.windows(2).any(|w| {
        w[1].source_time_s > w[0].source_time_s
            && (w[1].timeline_time_s - w[0].timeline_time_s).abs() < 1e-6
    });
    assert!(flat, "HoldFreeze lost its flat plateau: {curve:?}");
}

// Finding 3364684048 (no regression): the slow-mo shapes' real slow
// motion (e.g. 0.30×) must still stretch the timeline — the freeze guard
// must not clamp legitimate slow-mo factors.
#[test]
fn slow_mo_factor_not_clamped_by_freeze_guard() {
    let r = req(RampShape::SlowMoReveal, 4.0); // min_factor 0.30
    let curve = plan_ramp_curve(&r).unwrap();
    // The bell center (0.30×) must stretch source→timeline by ~1/0.30.
    let mid_source = r.source_duration_s * 0.5;
    let win = curve
        .windows(2)
        .find(|w| w[0].source_time_s <= mid_source && w[1].source_time_s >= mid_source)
        .expect("a window crosses the center");
    let d_source = win[1].source_time_s - win[0].source_time_s;
    let d_timeline = win[1].timeline_time_s - win[0].timeline_time_s;
    // 0.30× → ~3.3× stretch; require at least a clear >2× stretch so a
    // too-aggressive freeze floor (which would shrink it toward 1×) fails.
    assert!(
        d_timeline > d_source * 2.0,
        "slow-mo stretch was clamped: d_source={d_source}, d_timeline={d_timeline}"
    );
}

// Step 6: eased, not linear — the factor profile's slope near a
// transition is non-constant.
#[test]
fn factor_profile_is_eased_not_linear() {
    let r = req(RampShape::SlowMoReveal, 4.0);
    // Sample three points inside the entry transition (before the
    // plateau). Their first differences must differ if eased.
    let us = [0.05, 0.10, 0.15, 0.20];
    let fs: Vec<f64> = us.iter().map(|&u| factor_at(r.shape, u, &r)).collect();
    let d1 = fs[1] - fs[0];
    let d2 = fs[2] - fs[1];
    let d3 = fs[3] - fs[2];
    assert!(
        (d1 - d2).abs() > 1e-6 || (d2 - d3).abs() > 1e-6,
        "factor profile is linear (constant slope): d1={d1}, d2={d2}, d3={d3}"
    );
}

// Step 7: validate_curve rejects broken curves with a rule-naming Err.
#[test]
fn validate_curve_rejects_broken_curves() {
    // Decreasing source.
    let decreasing = vec![
        RampPoint {
            source_time_s: 0.0,
            timeline_time_s: 0.0,
        },
        RampPoint {
            source_time_s: 2.0,
            timeline_time_s: 1.0,
        },
        RampPoint {
            source_time_s: 1.0,
            timeline_time_s: 2.0,
        },
    ];
    let err = validate_curve(&decreasing).unwrap_err();
    assert!(err.contains("strictly increasing"), "got: {err}");

    // Not starting at origin.
    let off_origin = vec![
        RampPoint {
            source_time_s: 0.5,
            timeline_time_s: 0.0,
        },
        RampPoint {
            source_time_s: 1.0,
            timeline_time_s: 1.0,
        },
    ];
    let err = validate_curve(&off_origin).unwrap_err();
    assert!(err.contains("start at"), "got: {err}");

    // Too short.
    let short = vec![RampPoint {
        source_time_s: 0.0,
        timeline_time_s: 0.0,
    }];
    assert!(validate_curve(&short).is_err());

    // Decreasing timeline.
    let backwards = vec![
        RampPoint {
            source_time_s: 0.0,
            timeline_time_s: 0.0,
        },
        RampPoint {
            source_time_s: 1.0,
            timeline_time_s: 2.0,
        },
        RampPoint {
            source_time_s: 2.0,
            timeline_time_s: 1.0,
        },
    ];
    let err = validate_curve(&backwards).unwrap_err();
    assert!(err.contains("non-decreasing"), "got: {err}");
}

// Step 8: determinism.
#[test]
fn plan_is_deterministic() {
    let mut r = req(RampShape::RampToBeat, 6.0);
    r.beat_at_s = Some(2.0);
    assert_eq!(plan_ramp_curve(&r).unwrap(), plan_ramp_curve(&r).unwrap());
}

// Step 9: bounded samples — a pathological sample count is capped and the
// output stays under the hard point cap.
#[test]
fn samples_are_bounded() {
    let mut r = req(RampShape::SlowMoReveal, 4.0);
    r.samples = 9999;
    let curve = plan_ramp_curve(&r).unwrap();
    assert!(
        curve.len() <= MAX_POINTS,
        "curve exceeded MAX_POINTS: {}",
        curve.len()
    );
    assert!(curve.len() >= 2);
}

// Extra: a valid curve from each shape passes validate_curve (the
// self-verify belt-and-braces).
#[test]
fn every_shape_self_validates() {
    for shape in [
        RampShape::SlowMoReveal,
        RampShape::RampToBeat,
        RampShape::PunchThen,
        RampShape::HoldFreeze,
    ] {
        let curve = plan_ramp_curve(&req(shape, 5.0)).unwrap();
        validate_curve(&curve).unwrap_or_else(|e| panic!("{shape:?} self-validate failed: {e}"));
    }
}

// Extra: zero / non-finite duration fails loud.
#[test]
fn zero_duration_fails_loud() {
    assert!(plan_ramp_curve(&req(RampShape::SlowMoReveal, 0.0)).is_err());
    assert!(plan_ramp_curve(&req(RampShape::SlowMoReveal, f64::NAN)).is_err());
}
