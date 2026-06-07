//! B-roll recommendation builder tests.
#![allow(clippy::too_many_arguments)]

use montage_core::broll_recommendations::build_broll_recommendation_package;
use montage_proto::professional::{
    BrollAssetStrategy, BrollRecommendationCategory, FusedMoment, SourceRange, UnderstandingAsset,
    UnderstandingEvidenceKind, UnderstandingEvidenceRef,
};

#[test]
fn builds_deterministic_scored_recommendations() {
    let package = build_broll_recommendation_package(&[asset_with_moments(vec![
        moment(
            "m-stat",
            "explanation",
            "Show the retention metric",
            "Retention improved by 38 percent after the new transcript pipeline.",
            vec!["AI editing".into()],
            10.0,
            20.0,
            0.88,
        ),
        moment(
            "m-product",
            "explanation",
            "Show the dashboard workflow",
            "The app dashboard lets editors review recommendations before assembly.",
            vec!["Montage".into()],
            30.0,
            38.0,
            0.82,
        ),
    ])]);
    let second = build_broll_recommendation_package(&[asset_with_moments(vec![
        moment(
            "m-stat",
            "explanation",
            "Show the retention metric",
            "Retention improved by 38 percent after the new transcript pipeline.",
            vec!["AI editing".into()],
            10.0,
            20.0,
            0.88,
        ),
        moment(
            "m-product",
            "explanation",
            "Show the dashboard workflow",
            "The app dashboard lets editors review recommendations before assembly.",
            vec!["Montage".into()],
            30.0,
            38.0,
            0.82,
        ),
    ])]);

    assert_eq!(package, second);
    assert_eq!(package.assets.len(), 1);
    assert_eq!(package.assets[0].recommendations.len(), 2);
    assert!(package.validate().is_empty());

    let top = &package.assets[0].recommendations[0];
    assert!(top.id.starts_with("broll-rec:"));
    assert_eq!(top.category, BrollRecommendationCategory::Statistic);
    assert_eq!(top.strategy, BrollAssetStrategy::MotionGraphic);
    assert_eq!(top.insertion.placement, "motion_graphic_overlay");
    assert!(
        top.score_breakdown
            .iter()
            .any(|part| part.name == "visual_specificity")
    );
    assert!(top.evidence_ids.iter().any(|id| id == "m-stat"));
}

#[test]
fn maps_moment_categories_to_visual_strategies() {
    let package = build_broll_recommendation_package(&[asset_with_moments(vec![
        moment(
            "m-story",
            "story",
            "Customer story",
            "She remembers the launch day and why it mattered.",
            vec![],
            1.0,
            7.0,
            0.78,
        ),
        moment(
            "m-technical",
            "explanation",
            "Explain embeddings",
            "The embedding pipeline stores transcript meaning for search.",
            vec!["semantic indexing".into()],
            12.0,
            18.0,
            0.84,
        ),
        moment(
            "m-emotion",
            "emotional_peak",
            "Emotional beat",
            "That was the moment the whole team understood the risk.",
            vec![],
            20.0,
            25.0,
            0.8,
        ),
    ])]);

    let recommendations = &package.assets[0].recommendations;
    assert!(recommendations.iter().any(|rec| {
        rec.category == BrollRecommendationCategory::Storytelling
            && rec.strategy == BrollAssetStrategy::Stock
    }));
    assert!(recommendations.iter().any(|rec| {
        rec.category == BrollRecommendationCategory::TechnicalConcept
            && rec.strategy == BrollAssetStrategy::MotionGraphic
    }));
    assert!(recommendations.iter().any(|rec| {
        rec.category == BrollRecommendationCategory::EmotionalMoment
            && rec.strategy == BrollAssetStrategy::Stock
    }));
}

#[test]
fn skips_invalid_ranges_and_low_confidence_noise() {
    let package = build_broll_recommendation_package(&[asset_with_moments(vec![
        moment(
            "m-invalid",
            "explanation",
            "Invalid",
            "This cannot produce a valid insertion.",
            vec![],
            9.0,
            3.0,
            0.9,
        ),
        moment(
            "m-noise",
            "dead_air",
            "Dead air",
            "Long pause.",
            vec![],
            11.0,
            13.0,
            0.2,
        ),
    ])]);

    assert!(package.assets[0].recommendations.is_empty());
}

fn asset_with_moments(moments: Vec<FusedMoment>) -> UnderstandingAsset {
    UnderstandingAsset {
        asset_id: "raw/interview.mov".into(),
        moments,
        ..UnderstandingAsset::default()
    }
}

fn moment(
    id: &str,
    category: &str,
    label: &str,
    excerpt: &str,
    topics: Vec<String>,
    start_s: f64,
    end_s: f64,
    confidence: f64,
) -> FusedMoment {
    FusedMoment {
        id: id.into(),
        asset_id: "raw/interview.mov".into(),
        range: SourceRange { start_s, end_s },
        category: category.into(),
        label: label.into(),
        transcript_excerpt: Some(excerpt.into()),
        topics,
        confidence,
        evidence: vec![UnderstandingEvidenceRef {
            kind: UnderstandingEvidenceKind::EditorialMoment,
            producer: "editorial-moments".into(),
            ref_id: id.into(),
        }],
        ..FusedMoment::default()
    }
}
