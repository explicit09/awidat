//! Montage parameter-animation diffing for vedit commits.
//!
//! `vedit-core` intentionally models only structural OTIO edit decisions.
//! Montage animation preferences live in opaque timeline metadata, so this
//! module computes a narrow side diff for `metadata.montage.parameter_animations`.

use std::collections::{BTreeMap, BTreeSet};

use montage_proto::professional::{AnimationTarget, Keyframe, ParameterAnimation};

use super::VcError;

/// One animation metadata change between two timeline refs.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AnimationChange {
    /// Animation exists only in `after`.
    AnimationAdded {
        /// Added animation summary.
        animation: AnimationSummary,
    },
    /// Animation exists only in `before`.
    AnimationRemoved {
        /// Removed animation summary.
        animation: AnimationSummary,
    },
    /// Animation id exists in both refs but target, keyframes, or review
    /// metadata changed.
    AnimationUpdated {
        /// Stable animation id.
        id: String,
        /// Stable human-readable target key.
        target: String,
        /// Field-level changes outside individual keyframes.
        field_changes: Vec<AnimationFieldChange>,
        /// Field-level changes within paired keyframes.
        keyframe_changes: Vec<AnimationKeyframeChange>,
        /// Derived changes between adjacent keyframes.
        segment_changes: Vec<AnimationSegmentChange>,
        /// Human-readable one-line summary.
        summary: String,
    },
}

/// Compact animation identity for diff output.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AnimationSummary {
    /// Stable animation id.
    pub id: String,
    /// Stable human-readable target key.
    pub target: String,
    /// Number of keyframes.
    pub keyframe_count: usize,
    /// Rationale attached to the animation, if any.
    pub rationale: Option<String>,
}

/// Field-level change outside individual keyframes.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AnimationFieldChange {
    /// Field path such as `target`, `metadata_only`, or `rationale`.
    pub field: String,
    /// Before value.
    pub before: serde_json::Value,
    /// After value.
    pub after: serde_json::Value,
}

/// One keyframe-level change aligned by rounded keyframe time.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AnimationKeyframeChange {
    /// Change kind: `added`, `removed`, or `modified`.
    pub op: &'static str,
    /// Rounded keyframe time in seconds.
    pub time_s: f64,
    /// Modified field, when `op=modified`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Before value, or the full removed keyframe for `op=removed`.
    pub before: serde_json::Value,
    /// After value, or the full added keyframe for `op=added`.
    pub after: serde_json::Value,
}

/// Segment-level change derived from adjacent keyframes.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AnimationSegmentChange {
    /// Segment index between keyframe `index` and `index + 1`.
    pub segment_index: usize,
    /// Segment field such as `duration_s`, `easing`, or `interpolation`.
    pub field: String,
    /// Before value.
    pub before: serde_json::Value,
    /// After value.
    pub after: serde_json::Value,
}

pub(crate) fn diff_parameter_animations(
    before: Option<&serde_json::Value>,
    after: Option<&serde_json::Value>,
) -> Result<Vec<AnimationChange>, VcError> {
    let before_by_id = animations_by_id(read_parameter_animations(before)?);
    let after_by_id = animations_by_id(read_parameter_animations(after)?);
    let ids = before_by_id
        .keys()
        .chain(after_by_id.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    let mut changes = Vec::new();
    for id in ids {
        match (before_by_id.get(&id), after_by_id.get(&id)) {
            (None, Some(after)) => changes.push(AnimationChange::AnimationAdded {
                animation: animation_summary(after),
            }),
            (Some(before), None) => changes.push(AnimationChange::AnimationRemoved {
                animation: animation_summary(before),
            }),
            (Some(before), Some(after)) if before != after => {
                changes.push(animation_updated_change(before, after));
            }
            _ => {}
        }
    }
    Ok(changes)
}

fn read_parameter_animations(
    timeline: Option<&serde_json::Value>,
) -> Result<Vec<ParameterAnimation>, VcError> {
    let Some(value) =
        timeline.and_then(|timeline| timeline.pointer("/metadata/montage/parameter_animations"))
    else {
        return Ok(Vec::new());
    };
    serde_json::from_value(value.clone()).map_err(|e| {
        VcError::Vedit(format!(
            "failed to parse montage parameter_animations metadata: {e}"
        ))
    })
}

fn animations_by_id(animations: Vec<ParameterAnimation>) -> BTreeMap<String, ParameterAnimation> {
    animations
        .into_iter()
        .map(|animation| (animation.id.clone(), animation))
        .collect()
}

fn animation_updated_change(
    before: &ParameterAnimation,
    after: &ParameterAnimation,
) -> AnimationChange {
    let field_changes = animation_field_changes(before, after);
    let keyframe_changes = keyframe_changes(before, after);
    let segment_changes = segment_changes(before, after);
    let summary = animation_update_summary(before, after, &segment_changes, &keyframe_changes);
    AnimationChange::AnimationUpdated {
        id: after.id.clone(),
        target: animation_target_key(&after.target),
        field_changes,
        keyframe_changes,
        segment_changes,
        summary,
    }
}

fn animation_field_changes(
    before: &ParameterAnimation,
    after: &ParameterAnimation,
) -> Vec<AnimationFieldChange> {
    let mut changes = Vec::new();
    push_field_change(
        &mut changes,
        "target",
        animation_target_key(&before.target),
        animation_target_key(&after.target),
    );
    push_field_change(
        &mut changes,
        "metadata_only",
        before.metadata_only,
        after.metadata_only,
    );
    push_field_change(
        &mut changes,
        "rationale",
        &before.rationale,
        &after.rationale,
    );
    push_field_change(
        &mut changes,
        "pre_extrapolation",
        before.pre_extrapolation,
        after.pre_extrapolation,
    );
    push_field_change(
        &mut changes,
        "post_extrapolation",
        before.post_extrapolation,
        after.post_extrapolation,
    );
    if before.keyframes.len() != after.keyframes.len() {
        push_field_change(
            &mut changes,
            "keyframe_count",
            before.keyframes.len(),
            after.keyframes.len(),
        );
    }
    changes
}

fn keyframe_changes(
    before: &ParameterAnimation,
    after: &ParameterAnimation,
) -> Vec<AnimationKeyframeChange> {
    let mut changes = Vec::new();
    let before_by_time = keyframes_by_time(&before.keyframes);
    let after_by_time = keyframes_by_time(&after.keyframes);
    let times = before_by_time
        .keys()
        .chain(after_by_time.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    for time_key in times {
        let time_s = time_key_to_seconds(time_key);
        match (before_by_time.get(&time_key), after_by_time.get(&time_key)) {
            (None, Some(after_keyframe)) => changes.push(AnimationKeyframeChange {
                op: "added",
                time_s,
                field: None,
                before: serde_json::Value::Null,
                after: serde_json::to_value(after_keyframe).unwrap_or(serde_json::Value::Null),
            }),
            (Some(before_keyframe), None) => changes.push(AnimationKeyframeChange {
                op: "removed",
                time_s,
                field: None,
                before: serde_json::to_value(before_keyframe).unwrap_or(serde_json::Value::Null),
                after: serde_json::Value::Null,
            }),
            (Some(before_keyframe), Some(after_keyframe)) => {
                push_keyframe_change(
                    &mut changes,
                    time_s,
                    "value",
                    before_keyframe.value,
                    after_keyframe.value,
                );
                push_keyframe_change(
                    &mut changes,
                    time_s,
                    "interpolation",
                    before_keyframe.interpolation,
                    after_keyframe.interpolation,
                );
                push_keyframe_change(
                    &mut changes,
                    time_s,
                    "easing",
                    before_keyframe.easing,
                    after_keyframe.easing,
                );
                push_keyframe_change(
                    &mut changes,
                    time_s,
                    "spring",
                    before_keyframe.spring,
                    after_keyframe.spring,
                );
            }
            (None, None) => {}
        }
    }
    changes
}

fn segment_changes(
    before: &ParameterAnimation,
    after: &ParameterAnimation,
) -> Vec<AnimationSegmentChange> {
    let mut changes = Vec::new();
    let paired = before
        .keyframes
        .len()
        .saturating_sub(1)
        .min(after.keyframes.len().saturating_sub(1));
    for index in 0..paired {
        let before_start = &before.keyframes[index];
        let before_end = &before.keyframes[index + 1];
        let after_start = &after.keyframes[index];
        let after_end = &after.keyframes[index + 1];
        push_segment_change(
            &mut changes,
            index,
            "duration_s",
            round3(before_end.time_s - before_start.time_s),
            round3(after_end.time_s - after_start.time_s),
        );
        push_segment_change(
            &mut changes,
            index,
            "interpolation",
            before_start.interpolation,
            after_start.interpolation,
        );
        push_segment_change(
            &mut changes,
            index,
            "easing",
            before_start.easing,
            after_start.easing,
        );
        push_segment_change(
            &mut changes,
            index,
            "spring_params",
            before_start.spring,
            after_start.spring,
        );
    }
    changes
}

fn push_field_change<T: serde::Serialize + PartialEq>(
    changes: &mut Vec<AnimationFieldChange>,
    field: &str,
    before: T,
    after: T,
) {
    if before == after {
        return;
    }
    changes.push(AnimationFieldChange {
        field: field.to_string(),
        before: serde_json::to_value(before).unwrap_or(serde_json::Value::Null),
        after: serde_json::to_value(after).unwrap_or(serde_json::Value::Null),
    });
}

fn push_keyframe_change<T: serde::Serialize + PartialEq>(
    changes: &mut Vec<AnimationKeyframeChange>,
    time_s: f64,
    field: &str,
    before: T,
    after: T,
) {
    if before == after {
        return;
    }
    changes.push(AnimationKeyframeChange {
        op: "modified",
        time_s,
        field: Some(field.to_string()),
        before: serde_json::to_value(before).unwrap_or(serde_json::Value::Null),
        after: serde_json::to_value(after).unwrap_or(serde_json::Value::Null),
    });
}

fn push_segment_change<T: serde::Serialize + PartialEq>(
    changes: &mut Vec<AnimationSegmentChange>,
    segment_index: usize,
    field: &str,
    before: T,
    after: T,
) {
    if before == after {
        return;
    }
    changes.push(AnimationSegmentChange {
        segment_index,
        field: field.to_string(),
        before: serde_json::to_value(before).unwrap_or(serde_json::Value::Null),
        after: serde_json::to_value(after).unwrap_or(serde_json::Value::Null),
    });
}

fn animation_summary(animation: &ParameterAnimation) -> AnimationSummary {
    AnimationSummary {
        id: animation.id.clone(),
        target: animation_target_key(&animation.target),
        keyframe_count: animation.keyframes.len(),
        rationale: animation.rationale.clone(),
    }
}

fn animation_target_key(target: &AnimationTarget) -> String {
    match target {
        AnimationTarget::Unset => "unset".to_string(),
        AnimationTarget::ClipParameter { clip_id, parameter } => {
            format!("clip:{clip_id}/{parameter}")
        }
        AnimationTarget::CompositionNodeParameter {
            composition_id,
            node_id,
            parameter,
        } => format!("composition:{composition_id}/{node_id}/{parameter}"),
        AnimationTarget::TrackParameter { track, parameter } => {
            format!("track:{track}/{parameter}")
        }
    }
}

fn animation_update_summary(
    before: &ParameterAnimation,
    after: &ParameterAnimation,
    segment_changes: &[AnimationSegmentChange],
    keyframe_changes: &[AnimationKeyframeChange],
) -> String {
    let target = animation_target_key(&after.target);
    let mut parts = Vec::new();
    let easing_segments = changed_segment_indices(segment_changes, "easing");
    if !easing_segments.is_empty() {
        parts.push(format!(
            "changed easing on segments {}",
            join_indices(&easing_segments)
        ));
    }
    let duration_segments = changed_segment_indices(segment_changes, "duration_s");
    if !duration_segments.is_empty() {
        parts.push(format!(
            "changed duration on segments {}",
            join_indices(&duration_segments)
        ));
    }
    if parts.is_empty() && !keyframe_changes.is_empty() {
        parts.push(format!(
            "changed {} keyframe fields",
            keyframe_changes.len()
        ));
    }
    if parts.is_empty() {
        parts.push("changed animation metadata".to_string());
    }
    format!("{} on {}: {}", before.id, target, parts.join("; "))
}

fn keyframes_by_time(keyframes: &[Keyframe]) -> BTreeMap<i64, &Keyframe> {
    keyframes
        .iter()
        .map(|keyframe| (time_key(keyframe.time_s), keyframe))
        .collect()
}

fn time_key(time_s: f64) -> i64 {
    (time_s * 1000.0).round() as i64
}

fn time_key_to_seconds(key: i64) -> f64 {
    key as f64 / 1000.0
}

fn changed_segment_indices(changes: &[AnimationSegmentChange], field: &str) -> Vec<usize> {
    changes
        .iter()
        .filter(|change| change.field == field)
        .map(|change| change.segment_index)
        .collect()
}

fn join_indices(indices: &[usize]) -> String {
    indices
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}
