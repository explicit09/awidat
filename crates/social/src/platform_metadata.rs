use crate::model::Provider;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlatformMetadataProfile {
    pub title_required: bool,
    pub title_max: Option<usize>,
    pub description_max: Option<usize>,
    pub tags_total_max: Option<usize>,
    pub caption_max: Option<usize>,
}

pub fn profile_for(provider: &Provider) -> PlatformMetadataProfile {
    match provider {
        Provider::YouTube => PlatformMetadataProfile {
            title_required: true,
            title_max: Some(100),
            description_max: Some(5_000),
            tags_total_max: Some(500),
            caption_max: None,
        },
        Provider::TikTok => PlatformMetadataProfile {
            title_required: true,
            title_max: Some(150),
            description_max: Some(4_000),
            tags_total_max: None,
            caption_max: None,
        },
        Provider::Instagram => PlatformMetadataProfile {
            title_required: false,
            title_max: None,
            description_max: None,
            tags_total_max: None,
            caption_max: Some(2_200),
        },
        Provider::TwitterX => PlatformMetadataProfile {
            title_required: true,
            title_max: Some(280),
            description_max: Some(280),
            tags_total_max: None,
            caption_max: None,
        },
    }
}

pub fn validate_platform_fields(provider: &Provider, fields: &serde_json::Value) -> Vec<String> {
    let profile = profile_for(provider);
    let mut reasons = Vec::new();
    let title = string_field(fields, "title").unwrap_or_default();
    let description = string_field(fields, "description").unwrap_or_default();

    if profile.title_required && title.trim().is_empty() {
        reasons.push("title.required".to_string());
    }
    if exceeds(title.chars().count(), profile.title_max) {
        reasons.push("title.too_long".to_string());
    }

    if provider == &Provider::Instagram {
        if exceeds(description.chars().count(), profile.caption_max) {
            reasons.push("caption.too_long".to_string());
        }
    } else if exceeds(description.chars().count(), profile.description_max) {
        reasons.push("description.too_long".to_string());
    }

    if let Some(max) = profile.tags_total_max {
        let total = tags_total_chars(fields);
        if total > max {
            reasons.push("tags.too_long".to_string());
        }
    }

    reasons
}

fn string_field(fields: &serde_json::Value, key: &str) -> Option<String> {
    fields
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn tags_total_chars(fields: &serde_json::Value) -> usize {
    match fields.get("tags") {
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(|tag| tag.chars().count())
            .sum(),
        Some(serde_json::Value::String(tags)) => tags
            .split(',')
            .map(str::trim)
            .map(|tag| tag.chars().count())
            .sum(),
        _ => 0,
    }
}

fn exceeds(actual: usize, max: Option<usize>) -> bool {
    max.is_some_and(|max| actual > max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_profile_requires_title_and_limits_tags() {
        let reasons = validate_platform_fields(
            &Provider::YouTube,
            &serde_json::json!({
                "title": "",
                "tags": ["x".repeat(501)]
            }),
        );
        assert_eq!(reasons, vec!["title.required", "tags.too_long"]);
    }

    #[test]
    fn instagram_caption_uses_description_field() {
        let reasons = validate_platform_fields(
            &Provider::Instagram,
            &serde_json::json!({
                "description": "i".repeat(2_201)
            }),
        );
        assert_eq!(reasons, vec!["caption.too_long"]);
    }

    #[test]
    fn twitter_x_profile_limits_title() {
        let reasons = validate_platform_fields(
            &Provider::TwitterX,
            &serde_json::json!({
                "title": "x".repeat(281)
            }),
        );
        assert_eq!(reasons, vec!["title.too_long"]);
    }
}
