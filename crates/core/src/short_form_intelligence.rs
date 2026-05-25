//! Short-form adapters over fused media intelligence packages.

use std::collections::BTreeSet;
use std::path::Path;

use awidat_index::read_sidecar;
use awidat_proto::index::AssetId;
use awidat_proto::professional::{
    BrollRecommendationAsset, ClipCandidateAsset, TranscriptAlignmentPackage, UnderstandingAsset,
};
use serde_json::Value;

use crate::broll_recommendations::build_broll_recommendations_for_asset;
use crate::clip_candidates::build_clip_candidates_for_asset;
use crate::scene_aware_short_form::SceneAwareShortFormInput;
use crate::short_form_review::ShortFormReviewInput;
use crate::transcript_alignment::{PhraseGroupingOptions, normalize_whisper_alignment};
use crate::understanding::build_understanding_for_asset;

/// Fused short-form intelligence for one source asset.
#[derive(Debug, Clone, Default)]
pub struct ShortFormIntelligence {
    /// Fused scene/moment understanding.
    pub understanding: UnderstandingAsset,
    /// Ranked clip candidates derived from fused moments.
    pub clip_candidates: ClipCandidateAsset,
    /// Ranked B-roll recommendations derived from fused moments.
    pub broll_recommendations: BrollRecommendationAsset,
    /// Stable transcript alignment derived from whisper words when available.
    pub transcript_alignment: Option<TranscriptAlignmentPackage>,
}

/// Build the fused intelligence spine for one source asset.
pub fn build_short_form_intelligence(project_root: &Path, asset_id: &str) -> ShortFormIntelligence {
    let understanding = build_understanding_for_asset(project_root, asset_id);
    let clip_candidates = build_clip_candidates_for_asset(&understanding);
    let broll_recommendations = build_broll_recommendations_for_asset(&understanding);
    let transcript_alignment = read_sidecar(project_root, "whisper", &AssetId::new(asset_id))
        .ok()
        .and_then(|sidecar| {
            normalize_whisper_alignment(asset_id, &sidecar, PhraseGroupingOptions::default()).ok()
        });

    ShortFormIntelligence {
        understanding,
        clip_candidates,
        broll_recommendations,
        transcript_alignment,
    }
}

/// Enrich short-form review input so the engine sees fused intelligence first.
pub fn apply_to_short_form_review_input(
    input: &mut ShortFormReviewInput,
    intelligence: &ShortFormIntelligence,
) {
    apply_transcript_alignment(
        &mut input.transcript,
        intelligence.transcript_alignment.as_ref(),
    );
    if !intelligence.understanding.moments.is_empty() {
        input.editorial_moments = fused_editorial_moments(intelligence);
    }
    if !intelligence.understanding.scenes.is_empty() {
        input.scenes = fused_scenes(&intelligence.understanding);
    }
    if has_topics(&intelligence.understanding) {
        input.topics = fused_topics(&intelligence.understanding);
    }
    if !intelligence
        .broll_recommendations
        .recommendations
        .is_empty()
    {
        input.broll_assets = fused_broll_assets(&input.broll_assets, intelligence);
    }
}

/// Enrich scene-aware input so captions, holds, and support visuals use fused evidence.
pub fn apply_to_scene_aware_input(
    input: &mut SceneAwareShortFormInput,
    intelligence: &ShortFormIntelligence,
) {
    apply_transcript_alignment(
        &mut input.transcript,
        intelligence.transcript_alignment.as_ref(),
    );
    if input
        .transcript
        .pointer("/segments")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
    {
        input.transcript = transcript_from_understanding(&intelligence.understanding);
    }
    if !intelligence.understanding.moments.is_empty() {
        input.editorial_moments = fused_editorial_moments(intelligence);
    }
    if !intelligence.understanding.scenes.is_empty() {
        input.scenes = fused_scenes(&intelligence.understanding);
    }
    if has_topics(&intelligence.understanding) {
        input.topics = fused_topics(&intelligence.understanding);
    }
    if !intelligence
        .broll_recommendations
        .recommendations
        .is_empty()
    {
        input.clip = fused_scene_clip_metadata(&input.clip, intelligence);
    }
}

fn apply_transcript_alignment(
    transcript: &mut Value,
    alignment: Option<&TranscriptAlignmentPackage>,
) {
    let Some(alignment) = alignment else {
        return;
    };
    if alignment.words.is_empty() {
        return;
    }
    let has_segments = transcript
        .pointer("/segments")
        .and_then(Value::as_array)
        .is_some_and(|segments| !segments.is_empty());
    if !has_segments {
        *transcript = transcript_from_alignment(alignment);
        return;
    }
    let Some(segments) = transcript.get_mut("segments").and_then(Value::as_array_mut) else {
        return;
    };
    for segment in segments {
        let start_s = number_field(segment, &["start_s", "start"]).unwrap_or(f64::NAN);
        let end_s = number_field(segment, &["end_s", "end"]).unwrap_or(f64::NAN);
        if !start_s.is_finite() || !end_s.is_finite() || end_s <= start_s {
            continue;
        }
        let has_words = segment
            .get("words")
            .and_then(Value::as_array)
            .is_some_and(|words| !words.is_empty());
        if has_words {
            continue;
        }
        let words = alignment
            .words
            .iter()
            .filter(|word| word.range.start_s < end_s && word.range.end_s > start_s)
            .map(|word| {
                serde_json::json!({
                    "id": word.id,
                    "text": word.corrected_text.as_deref().unwrap_or(&word.original_text),
                    "start_s": word.range.start_s,
                    "end_s": word.range.end_s,
                    "speaker_id": word.speaker_id,
                    "confidence": word.confidence
                })
            })
            .collect::<Vec<_>>();
        if let Some(object) = segment.as_object_mut() {
            object.insert("words".into(), Value::Array(words));
        }
    }
}

fn transcript_from_alignment(alignment: &TranscriptAlignmentPackage) -> Value {
    serde_json::json!({
        "segments": alignment.phrases.iter().map(|phrase| {
            serde_json::json!({
                "id": phrase.id,
                "start_s": phrase.range.start_s,
                "end_s": phrase.range.end_s,
                "text": phrase.text,
                "words": alignment.words.iter()
                    .filter(|word| word.range.start_s < phrase.range.end_s && word.range.end_s > phrase.range.start_s)
                    .map(|word| serde_json::json!({
                        "id": word.id,
                        "text": word.corrected_text.as_deref().unwrap_or(&word.original_text),
                        "start_s": word.range.start_s,
                        "end_s": word.range.end_s,
                        "speaker_id": word.speaker_id,
                        "confidence": word.confidence
                    }))
                    .collect::<Vec<_>>()
            })
        }).collect::<Vec<_>>()
    })
}

fn transcript_from_understanding(asset: &UnderstandingAsset) -> Value {
    serde_json::json!({
        "segments": asset.moments.iter()
            .filter_map(|moment| {
                let text = moment.transcript_excerpt.as_deref().unwrap_or(&moment.label).trim();
                (!text.is_empty()).then(|| serde_json::json!({
                    "id": moment.id,
                    "start_s": moment.range.start_s,
                    "end_s": moment.range.end_s,
                    "speaker_id": moment.speaker_id,
                    "text": text
                }))
            })
            .collect::<Vec<_>>()
    })
}

fn fused_editorial_moments(intelligence: &ShortFormIntelligence) -> Value {
    let moments = intelligence
        .understanding
        .moments
        .iter()
        .map(|moment| {
            serde_json::json!({
                "id": moment.id,
                "kind": moment.category,
                "start_s": moment.range.start_s,
                "end_s": moment.range.end_s,
                "score": moment.confidence,
                "text": moment.transcript_excerpt.as_deref().unwrap_or(&moment.label),
                "reason": format!("fused understanding: {}", moment.label),
                "speaker_id": moment.speaker_id,
                "topics": moment.topics,
                "evidence_ids": moment.evidence.iter().map(|evidence| {
                    format!("{}:{}", evidence.producer, evidence.ref_id)
                }).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    let clip_candidates = intelligence
        .clip_candidates
        .candidates
        .iter()
        .map(|candidate| {
            serde_json::json!({
                "id": candidate.id,
                "kind": "clip_candidate",
                "start_s": candidate.range.start_s,
                "end_s": candidate.range.end_s,
                "score": candidate.score,
                "text": candidate.assembly.hook_text,
                "reason": candidate.explanation,
                "evidence_ids": candidate.evidence_ids
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "source": "fused_understanding",
        "moments": moments,
        "clip_candidates": clip_candidates
    })
}

fn fused_scenes(asset: &UnderstandingAsset) -> Value {
    serde_json::json!({
        "source": "fused_understanding",
        "shots": asset.scenes.iter().map(|scene| {
            serde_json::json!({
                "id": scene.id,
                "start_s": scene.range.start_s,
                "end_s": scene.range.end_s,
                "label": scene.label
            })
        }).collect::<Vec<_>>()
    })
}

fn fused_topics(asset: &UnderstandingAsset) -> Value {
    let mut seen = BTreeSet::new();
    let mut topics = Vec::new();
    for moment in &asset.moments {
        for topic in &moment.topics {
            let key = format!(
                "{}:{:.3}:{:.3}",
                topic, moment.range.start_s, moment.range.end_s
            );
            if seen.insert(key) {
                topics.push(serde_json::json!({
                    "label": topic,
                    "start_s": moment.range.start_s,
                    "end_s": moment.range.end_s,
                    "source_moment_id": moment.id
                }));
            }
        }
    }
    serde_json::json!({
        "source": "fused_understanding",
        "topics": topics
    })
}

fn fused_broll_assets(existing: &Value, intelligence: &ShortFormIntelligence) -> Value {
    let recommendations = intelligence
        .broll_recommendations
        .recommendations
        .iter()
        .map(|rec| {
            serde_json::json!({
                "id": rec.id,
                "moment_id": rec.moment_id,
                "start_s": rec.source_range.start_s,
                "end_s": rec.source_range.end_s,
                "score": rec.score,
                "category": rec.category,
                "strategy": rec.strategy,
                "rationale": rec.rationale,
                "placement": rec.insertion.placement,
                "evidence_ids": rec.evidence_ids
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "source": "fused_understanding",
        "recommendations": recommendations,
        "candidates": existing.pointer("/candidates").cloned().unwrap_or_else(|| Value::Array(Vec::new()))
    })
}

fn fused_scene_clip_metadata(existing: &Value, intelligence: &ShortFormIntelligence) -> Value {
    let mut object = existing.as_object().cloned().unwrap_or_default();
    object.insert(
        "broll_recommendations".into(),
        fused_broll_assets(&Value::Null, intelligence)["recommendations"].clone(),
    );
    Value::Object(object)
}

fn has_topics(asset: &UnderstandingAsset) -> bool {
    asset.moments.iter().any(|moment| !moment.topics.is_empty())
}

fn number_field(value: &Value, fields: &[&str]) -> Option<f64> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_f64))
}
