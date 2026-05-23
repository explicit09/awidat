//! Kalman-based smoothing for subject-aware reframe paths.
//!
//! The lowering path stores raw evidence keyframes in
//! [`awidat_proto::professional::ReframePath`]. Different smoothing levels
//! (see [`ReframeSmoothing`]) are realized at render time so the authored
//! path stays untouched and can be re-smoothed simply by re-rendering with
//! a different policy.
//!
//! The smoother runs a 2-state constant-velocity Kalman filter on each
//! channel (`center_x`, `center_y`, `scale`) followed by an RTS backward
//! smoother. The result has lower jerk than the raw signal with no lag in
//! the middle of the path (the backward pass cancels the filter's group
//! delay).

use awidat_proto::professional::{ReframeKeyframe, ReframeSmoothing};

/// Process / measurement noise variances driving the Kalman update.
#[derive(Debug, Clone, Copy)]
struct NoiseParams {
    /// Spectral density of the constant-velocity model.
    process: f64,
    /// Variance of the per-sample measurement.
    measurement: f64,
}

impl NoiseParams {
    /// Translate a smoothing level into noise variances.
    ///
    /// Returns `None` when no smoothing should be applied — the caller
    /// short-circuits and returns the raw keyframes unchanged.
    fn for_smoothing(level: ReframeSmoothing) -> Option<Self> {
        match level {
            ReframeSmoothing::None => None,
            // Gentle: trust measurements more; lighter lag.
            ReframeSmoothing::Gentle => Some(Self {
                process: 1e-2,
                measurement: 1e-3,
            }),
            ReframeSmoothing::Moderate => Some(Self {
                process: 1e-3,
                measurement: 1e-2,
            }),
            // Strong: trust the process model; heavy damping of noisy detections.
            ReframeSmoothing::Strong => Some(Self {
                process: 1e-4,
                measurement: 1e-1,
            }),
        }
    }
}

/// Smooth a sequence of values using a constant-velocity Kalman filter
/// applied in a forward pass, then an RTS backward smoother for low-lag,
/// low-jerk output.
///
/// Returns a vector of the same length. The input must already be in
/// ascending-time order; per-sample dt is taken from the supplied `times`.
fn rts_smooth_1d(values: &[f64], times: &[f64], params: NoiseParams) -> Vec<f64> {
    debug_assert_eq!(values.len(), times.len());
    if values.len() < 2 {
        return values.to_vec();
    }
    // 2-state [pos, vel] constant-velocity model.
    let n = values.len();
    let mut x_pred = vec![[0.0f64; 2]; n];
    let mut p_pred = vec![[[0.0f64; 2]; 2]; n];
    let mut x_filt = vec![[0.0f64; 2]; n];
    let mut p_filt = vec![[[0.0f64; 2]; 2]; n];

    x_filt[0] = [values[0], 0.0];
    p_filt[0] = [[1.0, 0.0], [0.0, 1.0]];

    for k in 1..n {
        let dt = (times[k] - times[k - 1]).max(1e-6);
        let q = params.process;
        let r = params.measurement;

        // Predict: x = F x; F = [[1, dt],[0,1]]
        let xp_pos = x_filt[k - 1][0] + dt * x_filt[k - 1][1];
        let xp_vel = x_filt[k - 1][1];
        x_pred[k] = [xp_pos, xp_vel];

        // Predict covariance: P = F P F^T + Q
        // (Q = q * [[dt^4/4, dt^3/2],[dt^3/2, dt^2]])
        let p = p_filt[k - 1];
        let f00 = p[0][0] + dt * (p[1][0] + p[0][1]) + dt * dt * p[1][1];
        let f01 = p[0][1] + dt * p[1][1];
        let f10 = p[1][0] + dt * p[1][1];
        let f11 = p[1][1];
        let dt2 = dt * dt;
        let dt3 = dt2 * dt;
        let dt4 = dt3 * dt;
        let qmat = [[q * dt4 / 4.0, q * dt3 / 2.0], [q * dt3 / 2.0, q * dt2]];
        p_pred[k] = [
            [f00 + qmat[0][0], f01 + qmat[0][1]],
            [f10 + qmat[1][0], f11 + qmat[1][1]],
        ];

        // Update: H = [1, 0]
        let y = values[k] - xp_pos;
        let s = p_pred[k][0][0] + r;
        if s <= 0.0 {
            x_filt[k] = x_pred[k];
            p_filt[k] = p_pred[k];
            continue;
        }
        let k0 = p_pred[k][0][0] / s;
        let k1 = p_pred[k][1][0] / s;
        x_filt[k] = [xp_pos + k0 * y, xp_vel + k1 * y];
        let p00 = (1.0 - k0) * p_pred[k][0][0];
        let p01 = (1.0 - k0) * p_pred[k][0][1];
        let p10 = p_pred[k][1][0] - k1 * p_pred[k][0][0];
        let p11 = p_pred[k][1][1] - k1 * p_pred[k][0][1];
        p_filt[k] = [[p00, p01], [p10, p11]];
    }

    // RTS backward pass.
    let mut x_smooth = x_filt.clone();
    for k in (0..n - 1).rev() {
        let dt = (times[k + 1] - times[k]).max(1e-6);
        // Need P_{k+1|k}^{-1}; 2x2 inverse.
        let m = p_pred[k + 1];
        let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
        if det.abs() < 1e-12 {
            continue;
        }
        let inv = [
            [m[1][1] / det, -m[0][1] / det],
            [-m[1][0] / det, m[0][0] / det],
        ];
        // C = P_filt[k] F^T P_pred[k+1]^{-1}
        // F^T = [[1,0],[dt,1]]
        let pft = [
            [p_filt[k][0][0] + dt * p_filt[k][0][1], p_filt[k][0][1]],
            [p_filt[k][1][0] + dt * p_filt[k][1][1], p_filt[k][1][1]],
        ];
        let c = [
            [
                pft[0][0] * inv[0][0] + pft[0][1] * inv[1][0],
                pft[0][0] * inv[0][1] + pft[0][1] * inv[1][1],
            ],
            [
                pft[1][0] * inv[0][0] + pft[1][1] * inv[1][0],
                pft[1][0] * inv[0][1] + pft[1][1] * inv[1][1],
            ],
        ];
        let dx0 = x_smooth[k + 1][0] - x_pred[k + 1][0];
        let dx1 = x_smooth[k + 1][1] - x_pred[k + 1][1];
        x_smooth[k][0] = x_filt[k][0] + c[0][0] * dx0 + c[0][1] * dx1;
        x_smooth[k][1] = x_filt[k][1] + c[1][0] * dx0 + c[1][1] * dx1;
    }

    x_smooth.iter().map(|s| s[0]).collect()
}

/// Apply smoothing to keyframes per the given level, returning a new vec.
///
/// The output preserves the per-keyframe `time_s` and `confidence` fields
/// and clamps smoothed coordinates back to the valid range
/// (`center` in `[0, 1]`, `scale >= 1`).
pub fn smooth_keyframes(
    keyframes: &[ReframeKeyframe],
    level: ReframeSmoothing,
) -> Vec<ReframeKeyframe> {
    let Some(params) = NoiseParams::for_smoothing(level) else {
        return keyframes.to_vec();
    };
    if keyframes.len() < 2 {
        return keyframes.to_vec();
    }
    let times: Vec<f64> = keyframes.iter().map(|k| k.time_s).collect();
    let cx: Vec<f64> = keyframes.iter().map(|k| k.center[0]).collect();
    let cy: Vec<f64> = keyframes.iter().map(|k| k.center[1]).collect();
    let scale: Vec<f64> = keyframes.iter().map(|k| k.scale).collect();
    let sx = rts_smooth_1d(&cx, &times, params);
    let sy = rts_smooth_1d(&cy, &times, params);
    let ss = rts_smooth_1d(&scale, &times, params);
    keyframes
        .iter()
        .enumerate()
        .map(|(i, k)| ReframeKeyframe {
            time_s: k.time_s,
            center: [sx[i].clamp(0.0, 1.0), sy[i].clamp(0.0, 1.0)],
            scale: ss[i].max(1.0),
            confidence: k.confidence,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kf(time_s: f64, cx: f64, cy: f64, scale: f64) -> ReframeKeyframe {
        ReframeKeyframe {
            time_s,
            center: [cx, cy],
            scale,
            confidence: Some(1.0),
        }
    }

    fn total_variation(values: &[f64]) -> f64 {
        values.windows(2).map(|w| (w[1] - w[0]).abs()).sum()
    }

    fn mean(values: &[f64]) -> f64 {
        if values.is_empty() {
            0.0
        } else {
            values.iter().sum::<f64>() / values.len() as f64
        }
    }

    fn jittery_alternating_keyframes() -> Vec<ReframeKeyframe> {
        (0..20)
            .map(|i| {
                let t = f64::from(i) / 30.0;
                let cx = if i % 2 == 0 { 0.4 } else { 0.6 };
                kf(t, cx, 0.5, 1.5)
            })
            .collect()
    }

    #[test]
    fn smoothing_none_returns_unchanged() {
        let raw = vec![
            kf(0.0, 0.30, 0.35, 1.5),
            kf(0.05, 0.55, 0.40, 1.6),
            kf(0.10, 0.30, 0.35, 1.5),
            kf(0.15, 0.55, 0.40, 1.6),
            kf(0.20, 0.30, 0.35, 1.5),
        ];
        let out = smooth_keyframes(&raw, ReframeSmoothing::None);
        assert_eq!(out, raw);
    }

    #[test]
    fn smoothing_reduces_total_variation() {
        let raw = jittery_alternating_keyframes();
        let raw_cx: Vec<f64> = raw.iter().map(|k| k.center[0]).collect();
        let raw_tv = total_variation(&raw_cx);

        for level in [ReframeSmoothing::Moderate, ReframeSmoothing::Strong] {
            let out = smooth_keyframes(&raw, level);
            let out_cx: Vec<f64> = out.iter().map(|k| k.center[0]).collect();
            let out_tv = total_variation(&out_cx);
            assert!(
                out_tv < raw_tv,
                "smoothing {level:?} did not strictly reduce total variation: raw={raw_tv} smoothed={out_tv}"
            );
        }
    }

    #[test]
    fn smoothing_preserves_mean_within_tolerance() {
        let raw = jittery_alternating_keyframes();
        for level in [
            ReframeSmoothing::Gentle,
            ReframeSmoothing::Moderate,
            ReframeSmoothing::Strong,
        ] {
            let out = smooth_keyframes(&raw, level);
            let out_cx: Vec<f64> = out.iter().map(|k| k.center[0]).collect();
            let m = mean(&out_cx);
            assert!(
                (m - 0.5).abs() < 0.05,
                "smoothing {level:?} mean {m} drifted from 0.5"
            );
        }
    }

    #[test]
    fn smoothing_handles_short_paths() {
        let empty: Vec<ReframeKeyframe> = Vec::new();
        let single = vec![kf(0.0, 0.4, 0.6, 2.0)];
        for level in [
            ReframeSmoothing::None,
            ReframeSmoothing::Gentle,
            ReframeSmoothing::Moderate,
            ReframeSmoothing::Strong,
        ] {
            assert!(smooth_keyframes(&empty, level).is_empty());
            let out = smooth_keyframes(&single, level);
            assert_eq!(out.len(), 1);
            assert_eq!(out[0], single[0]);
        }
    }

    #[test]
    fn smoothing_clamps_to_valid_range() {
        // Keyframes near the edges; with aggressive overshoot the smoother
        // could exceed 0/1, but the final output must remain in range.
        let raw = vec![
            kf(0.00, 0.02, 0.95, 1.0),
            kf(0.05, 0.98, 0.05, 1.0),
            kf(0.10, 0.02, 0.95, 1.0),
            kf(0.15, 0.98, 0.05, 1.0),
            kf(0.20, 0.02, 0.95, 1.0),
            kf(0.25, 0.98, 0.05, 1.0),
        ];
        for level in [
            ReframeSmoothing::Gentle,
            ReframeSmoothing::Moderate,
            ReframeSmoothing::Strong,
        ] {
            let out = smooth_keyframes(&raw, level);
            assert_eq!(out.len(), raw.len());
            for k in &out {
                assert!(
                    (0.0..=1.0).contains(&k.center[0]),
                    "{level:?} cx out of range: {}",
                    k.center[0]
                );
                assert!(
                    (0.0..=1.0).contains(&k.center[1]),
                    "{level:?} cy out of range: {}",
                    k.center[1]
                );
                assert!(
                    k.scale >= 1.0,
                    "{level:?} scale dropped below 1.0: {}",
                    k.scale
                );
            }
        }
    }
}
