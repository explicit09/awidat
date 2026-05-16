//! Phase 3A keyframe evaluation and render support classification.

use awidat_proto::professional::{Easing, Keyframe, KeyframeInterpolation};

/// Returns true for parameter paths supported by the Phase 3A render/preview runtime.
pub fn is_phase_3a_parameter(parameter: &str) -> bool {
    matches!(
        parameter,
        "title.opacity"
            | "title.x"
            | "title.y"
            | "overlay.opacity"
            | "overlay.x"
            | "overlay.y"
            | "overlay.scale"
    )
}

/// Evaluates sorted clip-local keyframes at `local_time_s`.
pub fn evaluate_keyframes(keyframes: &[Keyframe], local_time_s: f64) -> Option<f64> {
    let first = keyframes.first()?;
    if local_time_s <= first.time_s {
        return Some(first.value);
    }

    for pair in keyframes.windows(2) {
        let current = &pair[0];
        let next = &pair[1];
        if local_time_s > next.time_s {
            continue;
        }

        if current.interpolation == KeyframeInterpolation::Hold || next.time_s <= current.time_s {
            return Some(current.value);
        }

        let raw_progress = (local_time_s - current.time_s) / (next.time_s - current.time_s);
        let eased_progress = ease_progress(raw_progress, current.easing);
        return Some(current.value + (next.value - current.value) * eased_progress);
    }

    keyframes.last().map(|keyframe| keyframe.value)
}

fn ease_progress(progress: f64, easing: Easing) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => progress,
        Easing::EaseIn => progress * progress,
        Easing::EaseOut => 1.0 - (1.0 - progress) * (1.0 - progress),
        Easing::EaseInOut => {
            if progress < 0.5 {
                2.0 * progress * progress
            } else {
                1.0 - (-2.0 * progress + 2.0).powi(2) / 2.0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::professional::{Easing, KeyframeInterpolation};

    #[test]
    fn evaluates_linear_keyframes() {
        let keyframes = vec![Keyframe::linear(0.0, 0.0), Keyframe::linear(1.0, 1.0)];
        assert_eq!(evaluate_keyframes(&keyframes, 0.5), Some(0.5));
    }

    #[test]
    fn hold_interpolation_keeps_previous_value() {
        let keyframes = vec![
            Keyframe {
                time_s: 0.0,
                value: 2.0,
                interpolation: KeyframeInterpolation::Hold,
                easing: Easing::Linear,
            },
            Keyframe::linear(1.0, 4.0),
        ];
        assert_eq!(evaluate_keyframes(&keyframes, 0.5), Some(2.0));
    }
}
