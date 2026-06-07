//! Fused scene/moment understanding builders.

use std::path::{Path, PathBuf};

use montage_index::sidecar_path;
use montage_proto::index::AssetId;
use montage_proto::professional::{
    FusedMoment, FusedScene, SourceRange, UnderstandingAsset, UnderstandingEvidenceKind,
    UnderstandingEvidenceRef, UnderstandingPackage,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Build fused understanding for one asset from existing sidecars.
pub fn build_understanding_for_asset(project_root: &Path, asset_id: &str) -> UnderstandingAsset {
    let sidecars = Sidecars::load(project_root, asset_id);
    let scenes = fused_scenes(asset_id, &sidecars);
    let moments = fused_moments(asset_id, &scenes, &sidecars);
    UnderstandingAsset {
        asset_id: asset_id.to_string(),
        scenes,
        moments,
        missing_evidence: sidecars.missing_evidence(),
    }
}

/// Build fused understanding for known asset ids.
pub fn build_understanding_package(
    project_root: &Path,
    asset_ids: &[String],
) -> UnderstandingPackage {
    UnderstandingPackage {
        assets: asset_ids
            .iter()
            .map(|asset_id| build_understanding_for_asset(project_root, asset_id))
            .collect(),
    }
}

fn fused_scenes(asset_id: &str, sidecars: &Sidecars) -> Vec<FusedScene> {
    let Some(scenes) = sidecars
        .scenedetect
        .as_ref()
        .and_then(|value| {
            data(value)
                .get("shots")
                .or_else(|| data(value).get("scenes"))
        })
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    scenes
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let range = range_from_value(value)?;
            Some(FusedScene {
                id: format!("scene:{}:{index}", short_hash(asset_id)),
                range,
                label: value
                    .get("label")
                    .or_else(|| value.get("shot_type"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                evidence: vec![evidence_ref(
                    UnderstandingEvidenceKind::Scene,
                    "scenedetect",
                    format!("shot:{index}"),
                )],
            })
        })
        .collect()
}

fn fused_moments(asset_id: &str, scenes: &[FusedScene], sidecars: &Sidecars) -> Vec<FusedMoment> {
    let Some(moments) = sidecars
        .editorial_moments
        .as_ref()
        .and_then(|value| data(value).get("moments"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    moments
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let range = range_from_value(value)?;
            let category = value
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("moment")
                .to_string();
            let scene_id = scenes
                .iter()
                .find(|scene| ranges_overlap(&scene.range, &range))
                .map(|scene| scene.id.clone());
            let topics = topics_for_range(sidecars.topic.as_ref(), &range);
            let transcript_excerpt = transcript_excerpt(sidecars.whisper.as_ref(), &range);
            let speaker_id = value
                .get("speaker")
                .or_else(|| value.get("speaker_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| speaker_for_range(sidecars.whisper.as_ref(), &range));
            let energy = value
                .get("energy")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| audio_energy_label(sidecars.audio_energy.as_ref(), &range));
            let score = value.get("score").and_then(Value::as_f64).unwrap_or(0.5);
            let moment_id = value
                .get("moment_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{category}-{index}"));
            let mut evidence = vec![evidence_ref(
                UnderstandingEvidenceKind::EditorialMoment,
                "editorial-moments",
                moment_id.clone(),
            )];
            if transcript_excerpt.is_some() {
                evidence.push(evidence_ref(
                    UnderstandingEvidenceKind::Transcript,
                    "whisper",
                    format!("range:{}-{}", millis(range.start_s), millis(range.end_s)),
                ));
            }
            if scene_id.is_some() {
                evidence.push(evidence_ref(
                    UnderstandingEvidenceKind::Scene,
                    "scenedetect",
                    scene_id.clone().unwrap_or_default(),
                ));
            }
            if !topics.is_empty() {
                evidence.push(evidence_ref(
                    UnderstandingEvidenceKind::Topic,
                    "topic",
                    topics.join(","),
                ));
            }
            if energy.is_some() {
                evidence.push(evidence_ref(
                    UnderstandingEvidenceKind::Audio,
                    "audio-energy",
                    format!("range:{}-{}", millis(range.start_s), millis(range.end_s)),
                ));
            }

            Some(FusedMoment {
                id: format!(
                    "understanding:{}:{}:{}",
                    short_hash(asset_id),
                    short_hash(&moment_id),
                    millis(range.start_s)
                ),
                asset_id: asset_id.to_string(),
                range,
                category,
                label: value
                    .get("note")
                    .or_else(|| value.get("label"))
                    .and_then(Value::as_str)
                    .unwrap_or("indexed editorial moment")
                    .to_string(),
                scene_id,
                speaker_id,
                transcript_excerpt,
                topics,
                energy,
                confidence: score.clamp(0.0, 1.0),
                evidence,
                notes: value
                    .get("note")
                    .and_then(Value::as_str)
                    .map(|note| vec![note.to_string()])
                    .unwrap_or_default(),
            })
        })
        .collect()
}

struct Sidecars {
    whisper: Option<Value>,
    scenedetect: Option<Value>,
    topic: Option<Value>,
    audio_energy: Option<Value>,
    editorial_moments: Option<Value>,
}

impl Sidecars {
    fn load(project_root: &Path, asset_id: &str) -> Self {
        Self {
            whisper: read_sidecar(project_root, "whisper", asset_id),
            scenedetect: read_sidecar(project_root, "scenedetect", asset_id),
            topic: read_sidecar(project_root, "topic", asset_id),
            audio_energy: read_sidecar(project_root, "audio-energy", asset_id),
            editorial_moments: read_sidecar(project_root, "editorial-moments", asset_id),
        }
    }

    fn missing_evidence(&self) -> Vec<UnderstandingEvidenceKind> {
        let mut missing = Vec::new();
        if self.whisper.is_none() {
            missing.push(UnderstandingEvidenceKind::Transcript);
            missing.push(UnderstandingEvidenceKind::Speaker);
        }
        if self.scenedetect.is_none() {
            missing.push(UnderstandingEvidenceKind::Scene);
        }
        if self.topic.is_none() {
            missing.push(UnderstandingEvidenceKind::Topic);
        }
        if self.audio_energy.is_none() {
            missing.push(UnderstandingEvidenceKind::Audio);
        }
        if self.editorial_moments.is_none() {
            missing.push(UnderstandingEvidenceKind::EditorialMoment);
        }
        missing
    }
}

fn read_sidecar(project_root: &Path, indexer: &str, asset_id: &str) -> Option<Value> {
    let path = sidecar_path(project_root, indexer, &AssetId::new(asset_id.to_string()))
        .unwrap_or_else(|_| fallback_sidecar_path(project_root, indexer, asset_id));
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn fallback_sidecar_path(project_root: &Path, indexer: &str, asset_id: &str) -> PathBuf {
    project_root
        .join("index")
        .join(indexer)
        .join(format!("{asset_id}.json"))
}

fn range_from_value(value: &Value) -> Option<SourceRange> {
    let start_s = value
        .get("start_s")
        .or_else(|| value.get("start"))
        .and_then(Value::as_f64)?;
    let end_s = value
        .get("end_s")
        .or_else(|| value.get("end"))
        .and_then(Value::as_f64)?;
    let range = SourceRange { start_s, end_s };
    range.is_valid().then_some(range)
}

fn transcript_excerpt(sidecar: Option<&Value>, range: &SourceRange) -> Option<String> {
    let segments = sidecar?
        .pointer("/data/segments")
        .and_then(Value::as_array)?;
    let text = segments
        .iter()
        .filter_map(|segment| {
            let segment_range = range_from_value(segment)?;
            ranges_overlap(&segment_range, range)
                .then(|| segment.get("text").and_then(Value::as_str).unwrap_or(""))
        })
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!text.trim().is_empty()).then_some(text)
}

fn speaker_for_range(sidecar: Option<&Value>, range: &SourceRange) -> Option<String> {
    let words = sidecar?.pointer("/data/words").and_then(Value::as_array)?;
    let mut speakers = words
        .iter()
        .filter_map(|word| {
            let word_range = range_from_value(word)?;
            ranges_overlap(&word_range, range).then(|| {
                word.get("speaker_id")
                    .or_else(|| word.get("speaker"))
                    .and_then(Value::as_str)
            })?
        })
        .collect::<Vec<_>>();
    speakers.sort_unstable();
    speakers.dedup();
    (speakers.len() == 1).then(|| speakers[0].to_string())
}

fn topics_for_range(sidecar: Option<&Value>, range: &SourceRange) -> Vec<String> {
    let Some(topics) = sidecar
        .and_then(|value| data(value).get("topics"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    topics
        .iter()
        .filter_map(|topic| {
            let topic_range = range_from_value(topic).unwrap_or(SourceRange {
                start_s: range.start_s,
                end_s: range.end_s,
            });
            ranges_overlap(&topic_range, range).then(|| {
                topic
                    .get("label")
                    .or_else(|| topic.get("topic"))
                    .or_else(|| topic.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
            })
        })
        .filter(|topic| !topic.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn audio_energy_label(sidecar: Option<&Value>, range: &SourceRange) -> Option<String> {
    let data = data(sidecar?);
    let windows = data
        .get("windows")
        .or_else(|| data.get("frames"))
        .and_then(Value::as_array)?;
    let values = windows
        .iter()
        .filter_map(|window| {
            let window_range = range_from_value(window)?;
            ranges_overlap(&window_range, range).then(|| {
                window
                    .get("rms")
                    .or_else(|| window.get("energy"))
                    .and_then(Value::as_f64)
            })?
        })
        .collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    let avg = values.iter().sum::<f64>() / values.len() as f64;
    Some(
        if avg >= 0.65 {
            "high"
        } else if avg >= 0.35 {
            "medium"
        } else {
            "low"
        }
        .to_string(),
    )
}

fn evidence_ref(
    kind: UnderstandingEvidenceKind,
    producer: &str,
    ref_id: String,
) -> UnderstandingEvidenceRef {
    UnderstandingEvidenceRef {
        kind,
        producer: producer.to_string(),
        ref_id,
    }
}

fn ranges_overlap(a: &SourceRange, b: &SourceRange) -> bool {
    a.end_s > b.start_s && a.start_s < b.end_s
}

fn data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

fn millis(seconds: f64) -> i64 {
    (seconds * 1000.0).round() as i64
}

fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("{digest:x}").chars().take(12).collect()
}
