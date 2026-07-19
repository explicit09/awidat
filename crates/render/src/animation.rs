//! Phase 3A keyframe evaluation and render support classification.

use montage_proto::professional::{
    BezierHandles, Easing, ExtrapolationMode, Keyframe, KeyframeInterpolation, MotionPath,
    MotionPathControlPoint, MotionPathPoint, ParameterAnimation, SpringParameters, TangentMode,
    is_runtime_clip_parameter,
};

/// Returns true for parameter paths supported by the Phase 3A render/preview runtime.
pub fn is_phase_3a_parameter(parameter: &str) -> bool {
    is_runtime_clip_parameter(parameter)
}

/// Evaluates sorted clip-local keyframes at `local_time_s`.
pub fn evaluate_keyframes(keyframes: &[Keyframe], local_time_s: f64) -> Option<f64> {
    evaluate_keyframes_with_extrapolation(
        keyframes,
        local_time_s,
        ExtrapolationMode::Hold,
        ExtrapolationMode::Hold,
    )
}

/// Evaluates sorted keyframes with explicit pre/post extrapolation behavior.
pub fn evaluate_keyframes_with_modes(
    keyframes: &[Keyframe],
    local_time_s: f64,
    pre_extrapolation: ExtrapolationMode,
    post_extrapolation: ExtrapolationMode,
) -> Option<f64> {
    evaluate_keyframes_with_extrapolation(
        keyframes,
        local_time_s,
        pre_extrapolation,
        post_extrapolation,
    )
}

/// Evaluates a persisted animation at `local_time_s`.
pub fn evaluate_animation(animation: &ParameterAnimation, local_time_s: f64) -> Option<f64> {
    evaluate_keyframes_with_extrapolation(
        &animation.keyframes,
        local_time_s,
        animation.pre_extrapolation,
        animation.post_extrapolation,
    )
}

fn evaluate_keyframes_with_extrapolation(
    keyframes: &[Keyframe],
    local_time_s: f64,
    pre_extrapolation: ExtrapolationMode,
    post_extrapolation: ExtrapolationMode,
) -> Option<f64> {
    let first = keyframes.first()?;
    if local_time_s <= first.time_s {
        return Some(match pre_extrapolation {
            ExtrapolationMode::Hold => first.value,
            ExtrapolationMode::Linear => {
                linear_extrapolated_value(keyframes, local_time_s, true).unwrap_or(first.value)
            }
        });
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
        if current.interpolation == KeyframeInterpolation::Step {
            return Some(if raw_progress < 0.5 {
                current.value
            } else {
                next.value
            });
        }
        if current.interpolation == KeyframeInterpolation::Spring {
            let progress = current
                .spring
                .map(|spring| spring_progress(raw_progress, spring))
                .unwrap_or_else(|| spring_progress(raw_progress, SpringParameters::default()));
            return Some(current.value + (next.value - current.value) * progress);
        }
        let eased_progress = if current.interpolation == KeyframeInterpolation::Bezier {
            effective_bezier_handles(current, next)
                .map(|handles| bezier_progress(raw_progress, handles))
                .unwrap_or_else(|| ease_progress(raw_progress, current.easing))
        } else {
            ease_progress(raw_progress, current.easing)
        };
        return Some(current.value + (next.value - current.value) * eased_progress);
    }

    keyframes.last().map(|keyframe| match post_extrapolation {
        ExtrapolationMode::Hold => keyframe.value,
        ExtrapolationMode::Linear => {
            linear_extrapolated_value(keyframes, local_time_s, false).unwrap_or(keyframe.value)
        }
    })
}

fn linear_extrapolated_value(
    keyframes: &[Keyframe],
    local_time_s: f64,
    before_first: bool,
) -> Option<f64> {
    let (a, b) = if before_first {
        (keyframes.first()?, keyframes.get(1)?)
    } else {
        let b = keyframes.last()?;
        let a = keyframes.get(keyframes.len().checked_sub(2)?)?;
        (a, b)
    };
    let dt = b.time_s - a.time_s;
    if dt <= 0.0 {
        return None;
    }
    let slope = (b.value - a.value) / dt;
    Some(if before_first {
        a.value + slope * (local_time_s - a.time_s)
    } else {
        b.value + slope * (local_time_s - b.time_s)
    })
}

/// Convert scalar keyframes into a deterministic FFmpeg expression.
pub fn keyframes_to_ffmpeg_expr(keyframes: &[Keyframe], time_var: &str) -> String {
    keyframes_to_ffmpeg_expr_with_extrapolation(
        keyframes,
        time_var,
        ExtrapolationMode::Hold,
        ExtrapolationMode::Hold,
    )
}

/// Convert scalar keyframes into a deterministic FFmpeg expression with explicit extrapolation.
pub fn keyframes_to_ffmpeg_expr_with_extrapolation(
    keyframes: &[Keyframe],
    time_var: &str,
    pre_extrapolation: ExtrapolationMode,
    post_extrapolation: ExtrapolationMode,
) -> String {
    let Some(last) = keyframes.last() else {
        return "0".to_string();
    };
    if keyframes.len() == 1 {
        return last.value.to_string();
    }

    let mut fallback = ffmpeg_extrapolated_endpoint(keyframes, time_var, post_extrapolation, false)
        .unwrap_or_else(|| last.value.to_string());
    for pair in keyframes.windows(2).rev() {
        let current = &pair[0];
        let next = &pair[1];
        let value = if current.interpolation == KeyframeInterpolation::Hold
            || next.time_s <= current.time_s
        {
            current.value.to_string()
        } else if current.interpolation == KeyframeInterpolation::Step {
            let raw = format!(
                "(({time_var}-{start})/({end}-{start}))",
                start = current.time_s,
                end = next.time_s,
            );
            format!(
                "if(lt({raw}\\,0.5)\\,{start_value}\\,{end_value})",
                start_value = current.value,
                end_value = next.value,
            )
        } else if current.interpolation == KeyframeInterpolation::Bezier {
            let raw = format!(
                "(({time_var}-{start})/({end}-{start}))",
                start = current.time_s,
                end = next.time_s,
            );
            let progress_expr = effective_bezier_handles(current, next)
                .map(|handles| ffmpeg_sampled_bezier_progress(&raw, handles))
                .unwrap_or_else(|| ffmpeg_eased_progress(&raw, current.easing));
            format!(
                "{start_value}+({end_value}-{start_value})*({progress_expr})",
                start_value = current.value,
                end_value = next.value,
            )
        } else if current.interpolation == KeyframeInterpolation::Spring {
            let raw = format!(
                "(({time_var}-{start})/({end}-{start}))",
                start = current.time_s,
                end = next.time_s,
            );
            let spring = current.spring.unwrap_or_default();
            let progress_expr = ffmpeg_sampled_spring_progress(&raw, spring);
            format!(
                "{start_value}+({end_value}-{start_value})*({progress_expr})",
                start_value = current.value,
                end_value = next.value,
            )
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
    let first_value = ffmpeg_extrapolated_endpoint(keyframes, time_var, pre_extrapolation, true)
        .unwrap_or_else(|| first.value.to_string());
    format!(
        "if(lt({time_var}\\,{first_time})\\,{first_value}\\,{fallback})",
        first_time = first.time_s,
    )
}

/// Evaluates a 2D motion path at `local_time_s`.
pub fn evaluate_motion_path(path: &MotionPath, local_time_s: f64) -> Option<(f64, f64)> {
    let first = path.points.first()?;
    if local_time_s <= first.time_s {
        return Some((first.x, first.y));
    }
    for pair in path.points.windows(2) {
        let current = pair[0];
        let next = pair[1];
        if local_time_s > next.time_s {
            continue;
        }
        if next.time_s <= current.time_s {
            return Some((current.x, current.y));
        }
        let progress = (local_time_s - current.time_s) / (next.time_s - current.time_s);
        return Some(motion_path_segment_point(current, next, progress));
    }
    path.points.last().map(|point| (point.x, point.y))
}

/// Evaluates a persisted animation's motion path, using the animation value
/// as normalized path progress when keyframes are present.
pub fn evaluate_animation_motion_path(
    animation: &ParameterAnimation,
    local_time_s: f64,
) -> Option<(f64, f64)> {
    let path = animation.motion_path.as_ref()?;
    if animation.keyframes.is_empty() {
        return evaluate_motion_path(path, local_time_s);
    }
    let progress = evaluate_animation(animation, local_time_s)?;
    let path_time_s = motion_path_time_for_progress(path, progress)?;
    evaluate_motion_path(path, path_time_s)
}

/// Converts a motion path coordinate to an FFmpeg expression.
pub fn motion_path_to_ffmpeg_expr(
    path: &MotionPath,
    coordinate: MotionPathCoordinate,
    time_var: &str,
) -> String {
    let Some(last) = path.points.last() else {
        return "0".to_string();
    };
    if path.points.len() == 1 {
        return motion_path_coordinate_value(*last, coordinate).to_string();
    }
    let mut fallback = motion_path_coordinate_value(*last, coordinate).to_string();
    for pair in path.points.windows(2).rev() {
        let current = pair[0];
        let next = pair[1];
        let value = if next.time_s <= current.time_s {
            motion_path_coordinate_value(current, coordinate).to_string()
        } else {
            let start_value = motion_path_coordinate_value(current, coordinate);
            let end_value = motion_path_coordinate_value(next, coordinate);
            let progress = format!(
                "(({time_var}-{start})/({end}-{start}))",
                start = current.time_s,
                end = next.time_s,
            );
            if motion_path_segment_has_controls(current, next) {
                motion_path_cubic_expr(current, next, coordinate, &progress)
            } else {
                format!("{start_value}+({end_value}-{start_value})*{progress}")
            }
        };
        fallback = format!(
            "if(lt({time_var}\\,{end})\\,{value}\\,{fallback})",
            end = next.time_s
        );
    }
    let first = path.points[0];
    format!(
        "if(lt({time_var}\\,{first_time})\\,{first_value}\\,{fallback})",
        first_time = first.time_s,
        first_value = motion_path_coordinate_value(first, coordinate),
    )
}

/// Converts a motion path coordinate to an FFmpeg expression, using keyframes
/// as normalized path progress when present.
pub fn motion_path_to_ffmpeg_expr_with_keyframes(
    path: &MotionPath,
    keyframes: &[Keyframe],
    pre_extrapolation: ExtrapolationMode,
    post_extrapolation: ExtrapolationMode,
    coordinate: MotionPathCoordinate,
    time_var: &str,
) -> String {
    if keyframes.is_empty() {
        return motion_path_to_ffmpeg_expr(path, coordinate, time_var);
    }
    let Some(first) = path.points.first() else {
        return "0".to_string();
    };
    let Some(last) = path.points.last() else {
        return "0".to_string();
    };
    let progress_expr = keyframes_to_ffmpeg_expr_with_extrapolation(
        keyframes,
        time_var,
        pre_extrapolation,
        post_extrapolation,
    );
    let path_time_expr = format!(
        "{}+({}-{})*min(max({progress_expr}\\,0)\\,1)",
        first.time_s, last.time_s, first.time_s
    );
    motion_path_to_ffmpeg_expr(path, coordinate, &path_time_expr)
}

/// Axis selector for motion path expression lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionPathCoordinate {
    /// Horizontal coordinate.
    X,
    /// Vertical coordinate.
    Y,
}

fn motion_path_coordinate_value(point: MotionPathPoint, coordinate: MotionPathCoordinate) -> f64 {
    match coordinate {
        MotionPathCoordinate::X => point.x,
        MotionPathCoordinate::Y => point.y,
    }
}

fn motion_path_segment_has_controls(current: MotionPathPoint, next: MotionPathPoint) -> bool {
    current.outgoing_control.is_some() || next.incoming_control.is_some()
}

fn motion_path_segment_point(
    current: MotionPathPoint,
    next: MotionPathPoint,
    progress: f64,
) -> (f64, f64) {
    if !motion_path_segment_has_controls(current, next) {
        return (
            current.x + (next.x - current.x) * progress,
            current.y + (next.y - current.y) * progress,
        );
    }
    let outgoing = current.outgoing_control.unwrap_or(MotionPathControlPoint {
        x: current.x,
        y: current.y,
    });
    let incoming = next.incoming_control.unwrap_or(MotionPathControlPoint {
        x: next.x,
        y: next.y,
    });
    (
        cubic_bezier_value(current.x, outgoing.x, incoming.x, next.x, progress),
        cubic_bezier_value(current.y, outgoing.y, incoming.y, next.y, progress),
    )
}

fn cubic_bezier_value(start: f64, control_1: f64, control_2: f64, end: f64, progress: f64) -> f64 {
    let inverse = 1.0 - progress;
    inverse.powi(3) * start
        + 3.0 * inverse.powi(2) * progress * control_1
        + 3.0 * inverse * progress.powi(2) * control_2
        + progress.powi(3) * end
}

fn motion_path_cubic_expr(
    current: MotionPathPoint,
    next: MotionPathPoint,
    coordinate: MotionPathCoordinate,
    progress: &str,
) -> String {
    let start = motion_path_coordinate_value(current, coordinate);
    let outgoing = current
        .outgoing_control
        .map(|control| motion_path_control_value(control, coordinate))
        .unwrap_or(start);
    let end = motion_path_coordinate_value(next, coordinate);
    let incoming = next
        .incoming_control
        .map(|control| motion_path_control_value(control, coordinate))
        .unwrap_or(end);
    format!(
        "pow(1-{progress}\\,3)*{start}+3*pow(1-{progress}\\,2)*{progress}*{outgoing}+3*(1-{progress})*pow({progress}\\,2)*{incoming}+pow({progress}\\,3)*{end}"
    )
}

fn motion_path_control_value(
    control: MotionPathControlPoint,
    coordinate: MotionPathCoordinate,
) -> f64 {
    match coordinate {
        MotionPathCoordinate::X => control.x,
        MotionPathCoordinate::Y => control.y,
    }
}

fn motion_path_time_for_progress(path: &MotionPath, progress: f64) -> Option<f64> {
    if !progress.is_finite() {
        return None;
    }
    let first = path.points.first()?;
    let last = path.points.last()?;
    Some(first.time_s + (last.time_s - first.time_s) * progress.clamp(0.0, 1.0))
}

fn ffmpeg_extrapolated_endpoint(
    keyframes: &[Keyframe],
    time_var: &str,
    mode: ExtrapolationMode,
    before_first: bool,
) -> Option<String> {
    if mode == ExtrapolationMode::Hold {
        return None;
    }
    let (a, b) = if before_first {
        (keyframes.first()?, keyframes.get(1)?)
    } else {
        let b = keyframes.last()?;
        let a = keyframes.get(keyframes.len().checked_sub(2)?)?;
        (a, b)
    };
    let dt = b.time_s - a.time_s;
    if dt <= 0.0 {
        return None;
    }
    let slope = (b.value - a.value) / dt;
    Some(if before_first {
        format!(
            "{start_value}+({slope})*({time_var}-{start_time})",
            start_value = a.value,
            start_time = a.time_s,
        )
    } else {
        format!(
            "{end_value}+({slope})*({time_var}-{end_time})",
            end_value = b.value,
            end_time = b.time_s,
        )
    })
}

fn ease_progress(progress: f64, easing: Easing) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => progress,
        Easing::EaseInSine => 1.0 - (progress * std::f64::consts::FRAC_PI_2).cos(),
        Easing::EaseOutSine => (progress * std::f64::consts::FRAC_PI_2).sin(),
        Easing::EaseInOutSine => -((std::f64::consts::PI * progress).cos() - 1.0) / 2.0,
        Easing::EaseIn => progress * progress,
        Easing::EaseOut => 1.0 - (1.0 - progress) * (1.0 - progress),
        Easing::EaseInOut => {
            if progress < 0.5 {
                2.0 * progress * progress
            } else {
                1.0 - (-2.0 * progress + 2.0).powi(2) / 2.0
            }
        }
        Easing::EaseInCubic => progress.powi(3),
        Easing::EaseOutCubic => 1.0 - (1.0 - progress).powi(3),
        Easing::EaseInOutCubic => {
            if progress < 0.5 {
                4.0 * progress.powi(3)
            } else {
                1.0 - (-2.0 * progress + 2.0).powi(3) / 2.0
            }
        }
        Easing::EaseInQuart => progress.powi(4),
        Easing::EaseOutQuart => 1.0 - (1.0 - progress).powi(4),
        Easing::EaseInOutQuart => {
            if progress < 0.5 {
                8.0 * progress.powi(4)
            } else {
                1.0 - (-2.0 * progress + 2.0).powi(4) / 2.0
            }
        }
        Easing::EaseInQuint => progress.powi(5),
        Easing::EaseOutQuint => 1.0 - (1.0 - progress).powi(5),
        Easing::EaseInOutQuint => {
            if progress < 0.5 {
                16.0 * progress.powi(5)
            } else {
                1.0 - (-2.0 * progress + 2.0).powi(5) / 2.0
            }
        }
        Easing::EaseInExpo => {
            if progress <= 0.0 {
                0.0
            } else {
                2.0_f64.powf(10.0 * progress - 10.0)
            }
        }
        Easing::EaseOutExpo => {
            if progress >= 1.0 {
                1.0
            } else {
                1.0 - 2.0_f64.powf(-10.0 * progress)
            }
        }
        Easing::EaseInOutExpo => {
            if progress <= 0.0 {
                0.0
            } else if progress >= 1.0 {
                1.0
            } else if progress < 0.5 {
                2.0_f64.powf(20.0 * progress - 10.0) / 2.0
            } else {
                (2.0 - 2.0_f64.powf(-20.0 * progress + 10.0)) / 2.0
            }
        }
        Easing::EaseInCirc => 1.0 - (1.0 - progress.powi(2)).sqrt(),
        Easing::EaseOutCirc => (1.0 - (progress - 1.0).powi(2)).sqrt(),
        Easing::EaseInOutCirc => {
            if progress < 0.5 {
                (1.0 - (1.0 - (2.0 * progress).powi(2)).sqrt()) / 2.0
            } else {
                ((1.0 - (-2.0 * progress + 2.0).powi(2)).sqrt() + 1.0) / 2.0
            }
        }
        Easing::EaseInBack => {
            const C1: f64 = 1.70158;
            const C3: f64 = C1 + 1.0;
            C3 * progress.powi(3) - C1 * progress.powi(2)
        }
        Easing::EaseOutBack => {
            const C1: f64 = 1.70158;
            const C3: f64 = C1 + 1.0;
            1.0 + C3 * (progress - 1.0).powi(3) + C1 * (progress - 1.0).powi(2)
        }
        Easing::EaseInOutBack => {
            const C1: f64 = 1.70158;
            const C2: f64 = C1 * 1.525;
            if progress < 0.5 {
                ((2.0 * progress).powi(2) * ((C2 + 1.0) * 2.0 * progress - C2)) / 2.0
            } else {
                ((2.0 * progress - 2.0).powi(2) * ((C2 + 1.0) * (progress * 2.0 - 2.0) + C2) + 2.0)
                    / 2.0
            }
        }
        Easing::EaseInElastic => ease_in_elastic(progress),
        Easing::EaseOutElastic => ease_out_elastic(progress),
        Easing::EaseInOutElastic => {
            if progress <= 0.0 {
                0.0
            } else if progress >= 1.0 {
                1.0
            } else {
                let c5 = (2.0 * std::f64::consts::PI) / 4.5;
                if progress < 0.5 {
                    -(2.0_f64.powf(20.0 * progress - 10.0)
                        * ((20.0 * progress - 11.125) * c5).sin())
                        / 2.0
                } else {
                    (2.0_f64.powf(-20.0 * progress + 10.0)
                        * ((20.0 * progress - 11.125) * c5).sin())
                        / 2.0
                        + 1.0
                }
            }
        }
        Easing::EaseInBounce => 1.0 - ease_out_bounce(1.0 - progress),
        Easing::EaseOutBounce => ease_out_bounce(progress),
        Easing::EaseInOutBounce => {
            if progress < 0.5 {
                (1.0 - ease_out_bounce(1.0 - 2.0 * progress)) / 2.0
            } else {
                (1.0 + ease_out_bounce(2.0 * progress - 1.0)) / 2.0
            }
        }
    }
}

fn spring_progress(progress: f64, spring: SpringParameters) -> f64 {
    if progress <= 0.0 {
        return 0.0;
    }
    if progress >= 1.0 {
        return 1.0;
    }
    let mass = spring.mass.max(f64::EPSILON);
    let stiffness = spring.stiffness.max(f64::EPSILON);
    let damping = spring.damping.max(0.0);
    let p = progress.max(0.0);
    let omega_0 = (stiffness / mass).sqrt();
    let zeta = damping / (2.0 * (stiffness * mass).sqrt());
    if zeta < 1.0 - 1e-6 {
        let omega_d = omega_0 * (1.0 - zeta * zeta).sqrt();
        let envelope = (-zeta * omega_0 * p).exp();
        1.0 - envelope
            * ((omega_d * p).cos() + (zeta / (1.0 - zeta * zeta).sqrt()) * (omega_d * p).sin())
    } else {
        1.0 - (-omega_0 * p).exp() * (1.0 + omega_0 * p)
    }
}

fn ease_in_elastic(progress: f64) -> f64 {
    if progress <= 0.0 {
        0.0
    } else if progress >= 1.0 {
        1.0
    } else {
        let c4 = (2.0 * std::f64::consts::PI) / 3.0;
        -2.0_f64.powf(10.0 * progress - 10.0) * ((progress * 10.0 - 10.75) * c4).sin()
    }
}

fn ease_out_elastic(progress: f64) -> f64 {
    if progress <= 0.0 {
        0.0
    } else if progress >= 1.0 {
        1.0
    } else {
        let c4 = (2.0 * std::f64::consts::PI) / 3.0;
        2.0_f64.powf(-10.0 * progress) * ((progress * 10.0 - 0.75) * c4).sin() + 1.0
    }
}

fn ease_out_bounce(progress: f64) -> f64 {
    const N1: f64 = 7.5625;
    const D1: f64 = 2.75;
    if progress < 1.0 / D1 {
        N1 * progress.powi(2)
    } else if progress < 2.0 / D1 {
        let p = progress - 1.5 / D1;
        N1 * p.powi(2) + 0.75
    } else if progress < 2.5 / D1 {
        let p = progress - 2.25 / D1;
        N1 * p.powi(2) + 0.9375
    } else {
        let p = progress - 2.625 / D1;
        N1 * p.powi(2) + 0.984375
    }
}

fn bezier_progress(progress: f64, handles: BezierHandles) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    let mut t = progress;
    for _ in 0..8 {
        let x = cubic_bezier(0.0, handles.out_x, handles.in_x, 1.0, t) - progress;
        let slope = cubic_bezier_derivative(handles.out_x, handles.in_x, t);
        if x.abs() < 1e-12 || slope.abs() < 1e-12 {
            break;
        }
        t = (t - x / slope).clamp(0.0, 1.0);
    }
    cubic_bezier(0.0, handles.out_y, handles.in_y, 1.0, t)
}

fn effective_bezier_handles(current: &Keyframe, next: &Keyframe) -> Option<BezierHandles> {
    let mut handles = current.bezier?;
    if current.tangent_mode == TangentMode::Flat {
        handles.out_y = 0.0;
    }
    if next.tangent_mode == TangentMode::Flat {
        handles.in_y = 1.0;
    }
    Some(handles)
}

fn cubic_bezier(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    let inv = 1.0 - t;
    inv.powi(3) * p0 + 3.0 * inv.powi(2) * t * p1 + 3.0 * inv * t.powi(2) * p2 + t.powi(3) * p3
}

fn cubic_bezier_derivative(p1: f64, p2: f64, t: f64) -> f64 {
    let inv = 1.0 - t;
    3.0 * inv.powi(2) * p1 + 6.0 * inv * t * (p2 - p1) + 3.0 * t.powi(2) * (1.0 - p2)
}

fn ffmpeg_eased_progress(raw: &str, easing: Easing) -> String {
    match easing {
        Easing::Linear => raw.to_string(),
        Easing::EaseIn => format!("({raw})*({raw})"),
        Easing::EaseOut => format!("1-(1-({raw}))*(1-({raw}))"),
        Easing::EaseInOut => {
            format!("if(lt({raw}\\,0.5)\\,2*({raw})*({raw})\\,1-pow(-2*({raw})+2\\,2)/2)")
        }
        Easing::EaseInCubic => format!("pow({raw}\\,3)"),
        Easing::EaseOutCubic => format!("1-pow(1-({raw})\\,3)"),
        Easing::EaseInQuart => format!("pow({raw}\\,4)"),
        Easing::EaseOutQuart => format!("1-pow(1-({raw})\\,4)"),
        Easing::EaseInQuint => format!("pow({raw}\\,5)"),
        Easing::EaseOutQuint => format!("1-pow(1-({raw})\\,5)"),
        _ => ffmpeg_sampled_easing_progress(raw, easing),
    }
}

fn ffmpeg_sampled_easing_progress(raw: &str, easing: Easing) -> String {
    let samples = match easing {
        Easing::EaseInElastic
        | Easing::EaseOutElastic
        | Easing::EaseInOutElastic
        | Easing::EaseInBounce
        | Easing::EaseOutBounce
        | Easing::EaseInOutBounce
        | Easing::EaseInBack
        | Easing::EaseOutBack
        | Easing::EaseInOutBack => 64,
        _ => 16,
    };
    let mut fallback = ease_progress(1.0, easing).to_string();
    for index in (0..samples).rev() {
        let start = index as f64 / samples as f64;
        let end = (index + 1) as f64 / samples as f64;
        let start_value = ease_progress(start, easing);
        let end_value = ease_progress(end, easing);
        let segment_value =
            format!("{start_value}+({end_value}-{start_value})*(({raw}-{start})/({end}-{start}))");
        fallback = format!("if(lt({raw}\\,{end})\\,{segment_value}\\,{fallback})");
    }
    fallback
}

fn ffmpeg_sampled_bezier_progress(raw: &str, handles: BezierHandles) -> String {
    const SAMPLES: usize = 32;
    let mut fallback = bezier_progress(1.0, handles).to_string();
    for index in (0..SAMPLES).rev() {
        let start = index as f64 / SAMPLES as f64;
        let end = (index + 1) as f64 / SAMPLES as f64;
        let start_value = bezier_progress(start, handles);
        let end_value = bezier_progress(end, handles);
        let segment_value =
            format!("{start_value}+({end_value}-{start_value})*(({raw}-{start})/({end}-{start}))");
        fallback = format!("if(lt({raw}\\,{end})\\,{segment_value}\\,{fallback})");
    }
    fallback
}

fn ffmpeg_sampled_spring_progress(raw: &str, spring: SpringParameters) -> String {
    sampled_progress_expr(raw, |progress| spring_progress(progress, spring))
}

fn sampled_progress_expr(raw: &str, value_at: impl Fn(f64) -> f64) -> String {
    const SAMPLES: usize = 64;
    let mut fallback = value_at(1.0).to_string();
    for index in (0..SAMPLES).rev() {
        let start = index as f64 / SAMPLES as f64;
        let end = (index + 1) as f64 / SAMPLES as f64;
        let start_value = value_at(start);
        let end_value = value_at(end);
        let segment_value =
            format!("{start_value}+({end_value}-{start_value})*(({raw}-{start})/({end}-{start}))");
        fallback = format!("if(lt({raw}\\,{end})\\,{segment_value}\\,{fallback})");
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;
    use montage_proto::professional::{Easing, KeyframeInterpolation};
    use serde::Deserialize;
    use std::path::PathBuf;

    #[test]
    fn overlay_rotation_is_runtime_supported() {
        assert!(is_phase_3a_parameter("overlay.rotation_deg"));
    }

    #[derive(Deserialize)]
    struct AnimationParityFixture {
        cases: Vec<AnimationParityCase>,
    }

    #[derive(Deserialize)]
    struct AnimationParityCase {
        name: String,
        keyframes: Vec<Keyframe>,
        samples: Vec<AnimationParitySample>,
    }

    #[derive(Deserialize)]
    struct AnimationParitySample {
        time_s: f64,
        expected: f64,
    }

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
                bezier: None,
                tangent_mode: Default::default(),
                spring: None,
            },
            Keyframe::linear(1.0, 4.0),
        ];
        assert_eq!(evaluate_keyframes(&keyframes, 0.5), Some(2.0));
    }

    #[test]
    fn step_interpolation_switches_at_segment_midpoint() {
        let keyframes = vec![
            Keyframe {
                time_s: 0.0,
                value: 2.0,
                interpolation: KeyframeInterpolation::Step,
                easing: Easing::Linear,
                bezier: None,
                tangent_mode: Default::default(),
                spring: None,
            },
            Keyframe::linear(1.0, 4.0),
        ];
        assert_eq!(evaluate_keyframes(&keyframes, 0.49), Some(2.0));
        assert_eq!(evaluate_keyframes(&keyframes, 0.5), Some(4.0));
    }

    #[test]
    fn evaluates_named_back_and_bounce_easing() {
        let back = vec![
            Keyframe {
                time_s: 0.0,
                value: 0.0,
                interpolation: KeyframeInterpolation::Linear,
                easing: Easing::EaseOutBack,
                bezier: None,
                tangent_mode: Default::default(),
                spring: None,
            },
            Keyframe::linear(1.0, 1.0),
        ];
        let bounce = vec![
            Keyframe {
                time_s: 0.0,
                value: 0.0,
                interpolation: KeyframeInterpolation::Linear,
                easing: Easing::EaseInBounce,
                bezier: None,
                tangent_mode: Default::default(),
                spring: None,
            },
            Keyframe::linear(1.0, 1.0),
        ];

        assert!((evaluate_keyframes(&back, 0.5).unwrap() - 1.0876975).abs() < 1e-7);
        assert!((evaluate_keyframes(&bounce, 0.25).unwrap() - 0.02734375).abs() < 1e-9);
    }

    #[test]
    fn linear_extrapolation_extends_first_and_last_segment_velocity() {
        let animation = ParameterAnimation {
            id: "move".into(),
            target: montage_proto::professional::AnimationTarget::ClipParameter {
                clip_id: "clip-a".into(),
                parameter: "overlay.x".into(),
            },
            keyframes: vec![Keyframe::linear(1.0, 10.0), Keyframe::linear(3.0, 20.0)],
            metadata_only: false,
            pre_extrapolation: ExtrapolationMode::Linear,
            post_extrapolation: ExtrapolationMode::Linear,
            motion_path: None,
            rationale: None,
        };

        assert_eq!(evaluate_animation(&animation, 0.0), Some(5.0));
        assert_eq!(evaluate_animation(&animation, 4.0), Some(25.0));
    }

    #[test]
    fn spring_interpolation_uses_physical_parameters() {
        let keyframes = vec![
            Keyframe {
                time_s: 0.0,
                value: 0.0,
                interpolation: KeyframeInterpolation::Spring,
                easing: Easing::Linear,
                bezier: None,
                tangent_mode: Default::default(),
                spring: Some(Default::default()),
            },
            Keyframe::linear(1.0, 1.0),
        ];

        let value = evaluate_keyframes(&keyframes, 0.5).unwrap();

        assert!(value > 0.0);
        assert_ne!(value, 0.5);
    }

    #[test]
    fn spring_interpolation_handles_near_critical_damping() {
        let keyframes = vec![
            Keyframe {
                time_s: 0.0,
                value: 0.0,
                interpolation: KeyframeInterpolation::Spring,
                easing: Easing::Linear,
                bezier: None,
                tangent_mode: Default::default(),
                spring: Some(SpringParameters {
                    mass: 1.0,
                    stiffness: 100.0,
                    damping: 19.99999,
                }),
            },
            Keyframe::linear(1.0, 1.0),
        ];

        let value = evaluate_keyframes(&keyframes, 0.5).unwrap();

        assert!(value.is_finite());
        assert!((0.0..=1.0).contains(&value));
    }

    #[test]
    fn ffmpeg_spring_progress_uses_high_resolution_sampling() {
        let keyframes = vec![
            Keyframe {
                time_s: 0.0,
                value: 0.0,
                interpolation: KeyframeInterpolation::Spring,
                easing: Easing::Linear,
                bezier: None,
                tangent_mode: Default::default(),
                spring: Some(SpringParameters {
                    mass: 1.0,
                    stiffness: 200.0,
                    damping: 8.0,
                }),
            },
            Keyframe::linear(1.0, 1.0),
        ];

        let expr = keyframes_to_ffmpeg_expr(&keyframes, "t");

        assert!(
            expr.contains("0.984375") && expr.matches("if(lt(((t-0)/(1-0))\\,").count() >= 64,
            "spring FFmpeg lowering should sample at least 64 segments: {expr}"
        );
    }

    #[test]
    fn flat_tangent_mode_flattens_bezier_segment_endpoints() {
        let mut next = Keyframe::linear(1.0, 1.0);
        next.tangent_mode = TangentMode::Flat;
        let keyframes = vec![
            Keyframe {
                time_s: 0.0,
                value: 0.0,
                interpolation: KeyframeInterpolation::Bezier,
                easing: Easing::Linear,
                bezier: Some(BezierHandles {
                    out_x: 0.25,
                    out_y: 0.9,
                    in_x: 0.75,
                    in_y: 0.1,
                }),
                tangent_mode: TangentMode::Flat,
                spring: None,
            },
            next,
        ];

        let early = evaluate_keyframes(&keyframes, 0.01).unwrap();
        let late = evaluate_keyframes(&keyframes, 0.99).unwrap();

        assert!(
            early < 0.01,
            "flat outgoing tangent should start slowly: {early}"
        );
        assert!(
            late > 0.99,
            "flat incoming tangent should finish slowly: {late}"
        );
    }

    #[test]
    fn evaluates_motion_path_points() {
        let path = montage_proto::professional::MotionPath {
            points: vec![
                montage_proto::professional::MotionPathPoint {
                    time_s: 0.0,
                    x: -0.2,
                    y: 0.0,
                    outgoing_control: None,
                    incoming_control: None,
                },
                montage_proto::professional::MotionPathPoint {
                    time_s: 1.0,
                    x: 0.2,
                    y: 0.4,
                    outgoing_control: None,
                    incoming_control: None,
                },
            ],
        };

        assert_eq!(evaluate_motion_path(&path, 0.5), Some((0.0, 0.2)));
    }

    #[test]
    fn evaluates_motion_path_spatial_bezier_segment() {
        let path =
            montage_proto::professional::MotionPath {
                points: vec![
                    montage_proto::professional::MotionPathPoint {
                        time_s: 0.0,
                        x: 0.0,
                        y: 0.0,
                        outgoing_control: Some(
                            montage_proto::professional::MotionPathControlPoint { x: 0.0, y: 1.0 },
                        ),
                        incoming_control: None,
                    },
                    montage_proto::professional::MotionPathPoint {
                        time_s: 1.0,
                        x: 1.0,
                        y: 1.0,
                        outgoing_control: None,
                        incoming_control: Some(
                            montage_proto::professional::MotionPathControlPoint { x: 1.0, y: 0.0 },
                        ),
                    },
                ],
            };

        let midpoint = evaluate_motion_path(&path, 0.5).expect("path should evaluate");

        assert_eq!(midpoint, (0.5, 0.5));
        assert_eq!(evaluate_motion_path(&path, 0.25), Some((0.15625, 0.4375)));
    }

    #[test]
    fn motion_path_ffmpeg_expression_lowers_spatial_bezier_segment() {
        let path =
            montage_proto::professional::MotionPath {
                points: vec![
                    montage_proto::professional::MotionPathPoint {
                        time_s: 0.0,
                        x: 0.0,
                        y: 0.0,
                        outgoing_control: Some(
                            montage_proto::professional::MotionPathControlPoint { x: 0.0, y: 1.0 },
                        ),
                        incoming_control: None,
                    },
                    montage_proto::professional::MotionPathPoint {
                        time_s: 1.0,
                        x: 1.0,
                        y: 1.0,
                        outgoing_control: None,
                        incoming_control: Some(
                            montage_proto::professional::MotionPathControlPoint { x: 1.0, y: 0.0 },
                        ),
                    },
                ],
            };

        let expression = motion_path_to_ffmpeg_expr(&path, MotionPathCoordinate::X, "t");

        assert!(
            expression.contains("3*pow(1-((t-0)/(1-0))\\,2)*((t-0)/(1-0))*0"),
            "expected cubic Bezier term in FFmpeg expression: {expression}"
        );
        assert!(
            expression.contains("pow(((t-0)/(1-0))\\,3)*1"),
            "expected cubic endpoint in FFmpeg expression: {expression}"
        );
    }

    #[test]
    fn evaluates_motion_path_with_animation_progress_easing() {
        let animation = ParameterAnimation {
            id: "move-eased".into(),
            target: montage_proto::professional::AnimationTarget::ClipParameter {
                clip_id: "clip-a".into(),
                parameter: "title.position".into(),
            },
            keyframes: vec![
                Keyframe {
                    time_s: 0.0,
                    value: 0.0,
                    interpolation: KeyframeInterpolation::Bezier,
                    easing: Easing::Linear,
                    bezier: Some(BezierHandles {
                        out_x: 0.25,
                        out_y: 0.9,
                        in_x: 0.75,
                        in_y: 0.1,
                    }),
                    tangent_mode: TangentMode::Auto,
                    spring: None,
                },
                Keyframe::linear(1.0, 1.0),
            ],
            metadata_only: false,
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: Some(montage_proto::professional::MotionPath {
                points: vec![
                    montage_proto::professional::MotionPathPoint {
                        time_s: 0.0,
                        x: -0.2,
                        y: 0.0,
                        outgoing_control: None,
                        incoming_control: None,
                    },
                    montage_proto::professional::MotionPathPoint {
                        time_s: 1.0,
                        x: 0.2,
                        y: 0.4,
                        outgoing_control: None,
                        incoming_control: None,
                    },
                ],
            }),
            rationale: None,
        };

        let eased = evaluate_animation_motion_path(&animation, 0.25).unwrap();
        let linear_at_same_time =
            evaluate_motion_path(animation.motion_path.as_ref().unwrap(), 0.25)
                .expect("path should evaluate");

        assert_ne!(eased, linear_at_same_time);
        assert!(
            eased.0 > linear_at_same_time.0,
            "bezier progress should advance farther along x than raw linear time"
        );
        assert!(
            eased.1 > linear_at_same_time.1,
            "bezier progress should advance farther along y than raw linear time"
        );
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

    #[test]
    fn shared_animation_parity_fixture_matches_rust_evaluator() {
        let fixture = read_animation_parity_fixture();

        for case in fixture.cases {
            for sample in case.samples {
                let actual = evaluate_keyframes(&case.keyframes, sample.time_s)
                    .unwrap_or_else(|| panic!("case {} produced no value", case.name));
                assert!(
                    (actual - sample.expected).abs() < 1e-9,
                    "case {} at {}s expected {}, got {}",
                    case.name,
                    sample.time_s,
                    sample.expected,
                    actual
                );
            }
        }
    }

    fn read_animation_parity_fixture() -> AnimationParityFixture {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("fixtures")
            .join("motion")
            .join("animation-parity.json");
        let json = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        montage_proto::serde_robust::from_json_str(&json)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
    }
}
