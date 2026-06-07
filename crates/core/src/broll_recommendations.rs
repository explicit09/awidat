//! B-roll recommendation derivation from fused understanding.

use montage_proto::professional::{
    BrollAssetStrategy, BrollInsertionPlan, BrollRecommendation, BrollRecommendationAsset,
    BrollRecommendationCategory, BrollRecommendationPackage, BrollRecommendationScoreComponent,
    FusedMoment, SourceRange, UnderstandingAsset,
};
use sha2::{Digest, Sha256};

/// Build reviewable B-roll recommendations from fused understanding.
pub fn build_broll_recommendation_package(
    understanding_assets: &[UnderstandingAsset],
) -> BrollRecommendationPackage {
    BrollRecommendationPackage {
        assets: understanding_assets
            .iter()
            .map(build_broll_recommendations_for_asset)
            .collect(),
    }
}

/// Build ranked recommendations for one asset.
pub fn build_broll_recommendations_for_asset(
    asset: &UnderstandingAsset,
) -> BrollRecommendationAsset {
    let mut recommendations = asset
        .moments
        .iter()
        .filter_map(recommendation_from_moment)
        .collect::<Vec<_>>();
    recommendations.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    BrollRecommendationAsset {
        asset_id: asset.asset_id.clone(),
        recommendations,
    }
}

fn recommendation_from_moment(moment: &FusedMoment) -> Option<BrollRecommendation> {
    if !moment.range.is_valid() {
        return None;
    }
    if matches!(moment.category.as_str(), "dead_air" | "tangent") {
        return None;
    }

    let classification = classify_moment(moment);
    let score_breakdown = score_breakdown(moment, classification.category);
    let score = score_breakdown
        .iter()
        .map(|component| component.value)
        .sum::<f64>()
        / score_breakdown.len() as f64;
    if score < 0.45 {
        return None;
    }

    Some(BrollRecommendation {
        id: format!(
            "broll-rec:{}:{}:{}",
            short_hash(&moment.asset_id),
            short_hash(&moment.id),
            millis(moment.range.start_s)
        ),
        asset_id: moment.asset_id.clone(),
        moment_id: moment.id.clone(),
        source_range: moment.range.clone(),
        category: classification.category,
        strategy: classification.strategy,
        score,
        rationale: rationale(moment, classification.category, classification.strategy),
        score_breakdown,
        evidence_ids: evidence_ids(moment),
        insertion: insertion_plan(moment, classification.category, classification.strategy),
    })
}

#[derive(Debug, Clone, Copy)]
struct Classification {
    category: BrollRecommendationCategory,
    strategy: BrollAssetStrategy,
}

fn classify_moment(moment: &FusedMoment) -> Classification {
    let haystack = searchable_text(moment);
    let category = if contains_any(
        &haystack,
        &[
            "percent", "%", "million", "billion", "stat", "data", "metric",
        ],
    ) || haystack.chars().any(|ch| ch.is_ascii_digit())
    {
        BrollRecommendationCategory::Statistic
    } else if contains_any(&haystack, &["like ", "as if", "analogy", "metaphor"]) {
        BrollRecommendationCategory::Analogy
    } else if contains_any(
        &haystack,
        &["history", "historical", "years ago", "archive", "before"],
    ) {
        BrollRecommendationCategory::HistoricalReference
    } else if contains_any(
        &haystack,
        &[
            "app",
            "dashboard",
            "interface",
            "product",
            "workflow",
            "screen",
            "tool",
        ],
    ) {
        BrollRecommendationCategory::ProductMention
    } else if contains_any(
        &haystack,
        &[
            "api",
            "model",
            "code",
            "server",
            "database",
            "embedding",
            "transcript",
            "pipeline",
        ],
    ) {
        BrollRecommendationCategory::TechnicalConcept
    } else if matches!(moment.category.as_str(), "story" | "setup") {
        BrollRecommendationCategory::Storytelling
    } else if matches!(moment.category.as_str(), "emotional_peak" | "punchline") {
        BrollRecommendationCategory::EmotionalMoment
    } else if looks_like_entity_mention(moment) {
        BrollRecommendationCategory::EntityMention
    } else {
        BrollRecommendationCategory::Explanation
    };

    Classification {
        category,
        strategy: strategy_for(category),
    }
}

fn strategy_for(category: BrollRecommendationCategory) -> BrollAssetStrategy {
    match category {
        BrollRecommendationCategory::Statistic | BrollRecommendationCategory::TechnicalConcept => {
            BrollAssetStrategy::MotionGraphic
        }
        BrollRecommendationCategory::HistoricalReference
        | BrollRecommendationCategory::EntityMention => BrollAssetStrategy::ArchivalReference,
        BrollRecommendationCategory::ProductMention => BrollAssetStrategy::GeneratedVisual,
        BrollRecommendationCategory::Storytelling
        | BrollRecommendationCategory::EmotionalMoment => BrollAssetStrategy::Stock,
        BrollRecommendationCategory::Analogy | BrollRecommendationCategory::Explanation => {
            BrollAssetStrategy::GeneratedVisual
        }
    }
}

fn score_breakdown(
    moment: &FusedMoment,
    category: BrollRecommendationCategory,
) -> Vec<BrollRecommendationScoreComponent> {
    vec![
        BrollRecommendationScoreComponent {
            name: "visual_specificity".into(),
            value: visual_specificity(moment, category),
            reason: "uses transcript, topics, and category to estimate how concrete the visual prompt is".into(),
        },
        BrollRecommendationScoreComponent {
            name: "broll_need".into(),
            value: broll_need(moment, category),
            reason: "estimates whether the moment benefits from visual reinforcement".into(),
        },
        BrollRecommendationScoreComponent {
            name: "producer_confidence".into(),
            value: moment.confidence.clamp(0.0, 1.0),
            reason: "uses fused understanding confidence".into(),
        },
        BrollRecommendationScoreComponent {
            name: "evidence_quality".into(),
            value: evidence_quality(moment),
            reason: "scores transcript and sidecar evidence available for review".into(),
        },
    ]
}

fn visual_specificity(moment: &FusedMoment, category: BrollRecommendationCategory) -> f64 {
    let mut value: f64 = match category {
        BrollRecommendationCategory::Statistic => 0.95,
        BrollRecommendationCategory::ProductMention
        | BrollRecommendationCategory::TechnicalConcept => 0.84,
        BrollRecommendationCategory::HistoricalReference
        | BrollRecommendationCategory::EntityMention => 0.8,
        BrollRecommendationCategory::Analogy => 0.76,
        BrollRecommendationCategory::Storytelling
        | BrollRecommendationCategory::EmotionalMoment => 0.68,
        BrollRecommendationCategory::Explanation => 0.64,
    };
    if moment.transcript_excerpt.is_some() {
        value += 0.08;
    }
    if !moment.topics.is_empty() {
        value += 0.06;
    }
    value.clamp(0.0, 1.0)
}

fn broll_need(moment: &FusedMoment, category: BrollRecommendationCategory) -> f64 {
    let mut value: f64 = match category {
        BrollRecommendationCategory::Statistic
        | BrollRecommendationCategory::ProductMention
        | BrollRecommendationCategory::TechnicalConcept => 0.9,
        BrollRecommendationCategory::Explanation | BrollRecommendationCategory::Analogy => 0.82,
        BrollRecommendationCategory::HistoricalReference
        | BrollRecommendationCategory::EntityMention => 0.78,
        BrollRecommendationCategory::Storytelling
        | BrollRecommendationCategory::EmotionalMoment => 0.68,
    };
    if matches!(moment.energy.as_deref(), Some("high")) {
        value += 0.06;
    }
    if matches!(moment.category.as_str(), "dead_air" | "tangent") {
        value -= 0.45;
    }
    value.clamp(0.0, 1.0)
}

fn evidence_quality(moment: &FusedMoment) -> f64 {
    let mut value: f64 = if moment.evidence.is_empty() {
        0.35
    } else {
        0.62
    };
    if moment.transcript_excerpt.is_some() {
        value += 0.2;
    }
    if moment.evidence.len() >= 2 {
        value += 0.12;
    }
    if !moment.topics.is_empty() {
        value += 0.06;
    }
    value.clamp(0.0, 1.0)
}

fn insertion_plan(
    moment: &FusedMoment,
    category: BrollRecommendationCategory,
    strategy: BrollAssetStrategy,
) -> BrollInsertionPlan {
    let range = insertion_range(&moment.range);
    BrollInsertionPlan {
        anchor_id: moment.id.clone(),
        duration_s: range.end_s - range.start_s,
        range,
        placement: placement_for(category, strategy).into(),
        rationale: format!(
            "Anchor to {} so the visual lands on the supported idea without hiding surrounding context.",
            moment.label
        ),
    }
}

fn insertion_range(range: &SourceRange) -> SourceRange {
    let duration = range.end_s - range.start_s;
    if duration <= 6.0 {
        return range.clone();
    }
    let start_s = range.start_s + (duration * 0.2).min(2.0);
    let end_s = (start_s + 5.0).min(range.end_s);
    SourceRange { start_s, end_s }
}

fn placement_for(
    category: BrollRecommendationCategory,
    strategy: BrollAssetStrategy,
) -> &'static str {
    match (category, strategy) {
        (BrollRecommendationCategory::Statistic, _) => "motion_graphic_overlay",
        (BrollRecommendationCategory::TechnicalConcept, _) => "explainer_overlay",
        (BrollRecommendationCategory::ProductMention, _) => "screen_cutaway",
        (_, BrollAssetStrategy::ArchivalReference) => "archival_cutaway",
        (_, BrollAssetStrategy::Stock | BrollAssetStrategy::GeneratedVisual) => "cover_cutaway",
        _ => "supporting_visual",
    }
}

fn rationale(
    moment: &FusedMoment,
    category: BrollRecommendationCategory,
    strategy: BrollAssetStrategy,
) -> String {
    let excerpt = moment
        .transcript_excerpt
        .as_deref()
        .unwrap_or(moment.label.as_str());
    format!(
        "{category:?} opportunity using {strategy:?}; support \"{}\".",
        first_sentence(excerpt)
    )
}

fn evidence_ids(moment: &FusedMoment) -> Vec<String> {
    let mut ids = vec![moment.id.clone()];
    ids.extend(
        moment
            .evidence
            .iter()
            .map(|evidence| format!("{}:{}", evidence.producer, evidence.ref_id)),
    );
    ids
}

fn searchable_text(moment: &FusedMoment) -> String {
    let mut parts = vec![moment.category.as_str(), moment.label.as_str()];
    if let Some(excerpt) = moment.transcript_excerpt.as_deref() {
        parts.push(excerpt);
    }
    parts.extend(moment.topics.iter().map(String::as_str));
    parts.join(" ").to_lowercase()
}

fn looks_like_entity_mention(moment: &FusedMoment) -> bool {
    moment.topics.iter().any(|topic| {
        topic.split_whitespace().any(|word| {
            word.chars()
                .next()
                .is_some_and(|first| first.is_ascii_uppercase())
        })
    })
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn first_sentence(text: &str) -> String {
    text.split_terminator(['.', '!', '?'])
        .next()
        .unwrap_or(text)
        .trim()
        .to_string()
}

fn millis(seconds: f64) -> i64 {
    (seconds * 1000.0).round() as i64
}

fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("{digest:x}").chars().take(12).collect()
}
