//! Short-form clip candidate derivation.

use montage_proto::professional::{
    ClipCandidate, ClipCandidateAssembly, ClipCandidateAsset, ClipCandidatePackage,
    ClipCandidateScoreComponent, FusedMoment, SourceRange, UnderstandingAsset,
};
use sha2::{Digest, Sha256};

/// Build reviewable clip candidates from fused understanding.
pub fn build_clip_candidate_package(
    understanding_assets: &[UnderstandingAsset],
) -> ClipCandidatePackage {
    ClipCandidatePackage {
        assets: understanding_assets
            .iter()
            .map(build_clip_candidates_for_asset)
            .collect(),
    }
}

/// Build ranked candidates for one asset.
pub fn build_clip_candidates_for_asset(asset: &UnderstandingAsset) -> ClipCandidateAsset {
    let mut candidates = asset
        .moments
        .iter()
        .filter_map(candidate_from_moment)
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (index, candidate) in candidates.iter_mut().enumerate() {
        candidate.rank = (index + 1) as u32;
    }
    ClipCandidateAsset {
        asset_id: asset.asset_id.clone(),
        candidates,
    }
}

fn candidate_from_moment(moment: &FusedMoment) -> Option<ClipCandidate> {
    let score_breakdown = score_breakdown(moment);
    let score = score_breakdown
        .iter()
        .map(|component| component.value)
        .sum::<f64>()
        / score_breakdown.len() as f64;
    if score < 0.45 {
        return None;
    }
    let range = expanded_short_range(&moment.range);
    Some(ClipCandidate {
        id: format!(
            "clip-candidate:{}:{}:{}",
            short_hash(&moment.asset_id),
            short_hash(&moment.id),
            millis(range.start_s)
        ),
        asset_id: moment.asset_id.clone(),
        range: range.clone(),
        rank: 0,
        score,
        explanation: candidate_explanation(moment, score),
        score_breakdown,
        evidence_ids: vec![moment.id.clone()],
        assembly: ClipCandidateAssembly {
            asset_id: moment.asset_id.clone(),
            source_range: range,
            aspect_ratio: "9:16".into(),
            caption_style: caption_style(moment),
            hook_text: hook_text(moment),
            required_setup_ids: required_setup_ids(moment),
        },
    })
}

fn score_breakdown(moment: &FusedMoment) -> Vec<ClipCandidateScoreComponent> {
    vec![
        ClipCandidateScoreComponent {
            name: "moment_strength".into(),
            value: category_score(&moment.category),
            reason: format!("moment category is {}", moment.category),
        },
        ClipCandidateScoreComponent {
            name: "producer_confidence".into(),
            value: moment.confidence.clamp(0.0, 1.0),
            reason: "uses fused editorial confidence".into(),
        },
        ClipCandidateScoreComponent {
            name: "reviewability".into(),
            value: if moment.transcript_excerpt.is_some() {
                0.9
            } else {
                0.55
            },
            reason: "candidate has transcript context for caption review".into(),
        },
        ClipCandidateScoreComponent {
            name: "duration_fit".into(),
            value: duration_score(&moment.range),
            reason: "source range can be expanded into a short-form clip".into(),
        },
    ]
}

fn category_score(category: &str) -> f64 {
    match category {
        "hook" => 1.0,
        "emotional_peak" | "punchline" => 0.92,
        "story" | "question" | "answer" => 0.84,
        "explanation" | "setup" | "cta" => 0.74,
        "tangent" | "dead_air" => 0.15,
        _ => 0.55,
    }
}

fn duration_score(range: &SourceRange) -> f64 {
    let duration = range.end_s - range.start_s;
    if (18.0..=55.0).contains(&duration) {
        1.0
    } else if (8.0..18.0).contains(&duration) || (55.0..=75.0).contains(&duration) {
        0.72
    } else {
        0.45
    }
}

fn expanded_short_range(range: &SourceRange) -> SourceRange {
    let start_s = (range.start_s - 2.0).max(0.0);
    let end_s = (range.end_s + 3.0).max(start_s + 1.0);
    SourceRange { start_s, end_s }
}

fn candidate_explanation(moment: &FusedMoment, score: f64) -> String {
    format!(
        "{} candidate from {} moment; score {:.2}; {}",
        if score >= 0.75 {
            "Strong"
        } else {
            "Reviewable"
        },
        moment.category,
        score,
        moment
            .transcript_excerpt
            .as_deref()
            .unwrap_or("no transcript excerpt available")
    )
}

fn caption_style(moment: &FusedMoment) -> String {
    match moment.category.as_str() {
        "hook" | "punchline" => "kinetic_emphasis",
        "story" | "emotional_peak" => "clean_story",
        _ => "readable_default",
    }
    .into()
}

fn hook_text(moment: &FusedMoment) -> String {
    moment
        .transcript_excerpt
        .as_deref()
        .map(first_sentence)
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| moment.label.clone())
}

fn required_setup_ids(moment: &FusedMoment) -> Vec<String> {
    moment
        .notes
        .iter()
        .filter_map(|note| note.strip_prefix("requires:"))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
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
