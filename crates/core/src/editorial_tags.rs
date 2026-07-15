//! Stable editorial tags derived from an applied EDL envelope.
//!
//! Shared by the legacy `ToolHandler` path and the live MCP path so
//! preference learning does not depend on which dispatch surface ran
//! `apply_edl`.

use crate::edl::{BRollPosition, EdlEnvelope, EdlOp};

/// Extract sorted, deduped editorial tags from an envelope.
pub fn editorial_tags_for_envelope(envelope: &EdlEnvelope) -> Vec<String> {
    let mut tags = Vec::new();
    let mut transition_count = 0_usize;
    for op in &envelope.ops {
        match op {
            EdlOp::SetCutIntent { spec, .. } => {
                push_tag_owned(&mut tags, "cut_type", serde_tag(&spec.cut_type));
                push_tag(&mut tags, "cut_intent", Some(spec.intent.as_str()));
                push_tag_owned(&mut tags, "audio_relation", serde_tag(&spec.audio_relation));
                if spec.intent == "thematic_montage" {
                    tags.push("broll_mode:thematic_montage".into());
                }
            }
            EdlOp::InsertTransition { kind, spec, .. } => {
                transition_count += 1;
                tags.push(format!("transition_id:{}", normalize_tag_value(kind)));
                if let Some(family) = spec
                    .as_ref()
                    .and_then(|spec| spec.family.as_deref())
                    .or_else(|| {
                        montage_proto::transitions::lookup_builtin_transition(kind)
                            .map(|transition| transition.family)
                    })
                {
                    push_tag(&mut tags, "transition_family", Some(family));
                }
                if let Some(intent) = spec.as_ref().and_then(|spec| spec.intent.as_deref()) {
                    push_tag(&mut tags, "transition_intent", Some(intent));
                }
            }
            EdlOp::SetAudioLead { lead_s, .. } => {
                tags.push("split_edit:j_cut".into());
                tags.push(format!("audio_lead_range:{}", split_lead_bucket(*lead_s)));
            }
            EdlOp::SetAudioTrail { trail_s, .. } => {
                tags.push("split_edit:l_cut".into());
                tags.push(format!(
                    "audio_trail_range:{}",
                    split_trail_bucket(*trail_s)
                ));
            }
            EdlOp::InsertBRoll { position, .. } => {
                tags.push("broll_mode:literal_cover".into());
                tags.push(format!(
                    "broll_position:{}",
                    match position {
                        BRollPosition::Overlay => "overlay",
                        BRollPosition::Replace => "replace",
                    }
                ));
            }
            EdlOp::SetOutputFormat {
                aspect_ratio,
                platform,
                safe_area,
            } => {
                push_tag(&mut tags, "format_aspect", Some(aspect_ratio.as_str()));
                push_tag(&mut tags, "format_platform", platform.as_deref());
                push_tag(&mut tags, "format_safe_area", safe_area.as_deref());
            }
            _ => {}
        }
    }
    if transition_count > 0 {
        tags.push(format!(
            "transition_density:{}",
            transition_density_bucket(transition_count)
        ));
    }
    tags.sort();
    tags.dedup();
    if tags.contains(&"broll_mode:thematic_montage".to_string()) {
        tags.retain(|tag| tag != "broll_mode:literal_cover");
    }
    tags
}

fn transition_density_bucket(transition_count: usize) -> &'static str {
    match transition_count {
        0 => "none",
        1 => "single",
        2 => "moderate",
        _ => "high",
    }
}

fn push_tag(tags: &mut Vec<String>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        let value = normalize_tag_value(value);
        if !value.is_empty() {
            tags.push(format!("{key}:{value}"));
        }
    }
}

fn push_tag_owned(tags: &mut Vec<String>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        push_tag(tags, key, Some(&value));
    }
}

fn serde_tag<T: serde::Serialize>(value: &T) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
}

fn normalize_tag_value(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c == ':' { 'x' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .collect()
}

fn split_lead_bucket(seconds: f64) -> &'static str {
    if seconds < 0.25 {
        "under_0_25"
    } else if seconds <= 0.60 {
        "0_25_to_0_60"
    } else {
        "over_0_60"
    }
}

fn split_trail_bucket(seconds: f64) -> &'static str {
    if seconds < 0.25 {
        "under_0_25"
    } else if seconds <= 0.80 {
        "0_25_to_0_80"
    } else {
        "over_0_80"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edl::parse as edl_parse;

    #[test]
    fn tags_j_cut_and_l_cut() {
        let edl = "\
*** Begin EDL
*** Set Audio Lead
@@ anchor: clip_uuid=clip-0
+ lead_s: 0.4
*** Set Audio Trail
@@ anchor: clip_uuid=clip-1
+ trail_s: 0.5
*** End EDL
";
        let envelope = edl_parse(edl).expect("parse");
        let tags = editorial_tags_for_envelope(&envelope);
        assert!(tags.iter().any(|t| t == "split_edit:j_cut"), "{tags:?}");
        assert!(tags.iter().any(|t| t == "split_edit:l_cut"), "{tags:?}");
    }
}
