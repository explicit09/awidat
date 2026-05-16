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

/// Convert scalar keyframes into a deterministic FFmpeg expression.
pub fn keyframes_to_ffmpeg_expr(keyframes: &[Keyframe], time_var: &str) -> String {
    let Some(last) = keyframes.last() else {
        return "0".to_string();
    };
    if keyframes.len() == 1 {
        return last.value.to_string();
    }

    let mut fallback = last.value.to_string();
    for pair in keyframes.windows(2).rev() {
        let current = &pair[0];
        let next = &pair[1];
        let value = if current.interpolation == KeyframeInterpolation::Hold
            || next.time_s <= current.time_s
        {
            current.value.to_string()
        } else {
            let raw = format!(
                "(({time_var}-{start})/({end}-{start}))",
                start = current.time_s,
                end = next.time_s,
            );
            let eased = ffmpeg_eased_progress(&raw, current.easing);
            format!(
                "{start_value}+({end_value}-{start_value})*({eased})",
                start_value = current.value,
                end_value = next.value,
            )
        };
        fallback = format!(
            "if(lt({time_var}\\,{end})\\,{value}\\,{fallback})",
            end = next.time_s,
        );
    }
    let first = &keyframes[0];
    format!(
        "if(lt({time_var}\\,{first_time})\\,{first_value}\\,{fallback})",
        first_time = first.time_s,
        first_value = first.value,
    )
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

fn ffmpeg_eased_progress(raw: &str, easing: Easing) -> String {
    match easing {
        Easing::Linear => raw.to_string(),
        Easing::EaseIn => format!("({raw})*({raw})"),
        Easing::EaseOut => format!("1-(1-({raw}))*(1-({raw}))"),
        Easing::EaseInOut => {
            format!("if(lt({raw}\\,0.5)\\,2*({raw})*({raw})\\,1-pow(-2*({raw})+2\\,2)/2)")
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

    #[test]
    fn ffmpeg_expression_holds_first_value_before_first_keyframe() {
        let keyframes = vec![Keyframe::linear(1.0, 2.0), Keyframe::linear(2.0, 4.0)];
        let expression = keyframes_to_ffmpeg_expr(&keyframes, "t");

        assert!(
            expression.starts_with("if(lt(t\\,1)\\,2\\,"),
            "expression should hold the first value before the first keyframe: {expression}"
        );
    }
}
