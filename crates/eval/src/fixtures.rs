//! Deterministic eval fixtures.
//!
//! The eval crate intentionally builds tiny projects on demand instead
//! of relying on local media. Tool behavior only needs OTIO structure,
//! synthetic sidecars, and placeholder asset files for the fast tiers.

use anyhow::Result;
use awidat_proto::otio::{
    Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange, Timeline,
    Track, TrackChild, TrackKind,
};
use awidat_proto::project::Project;
use std::path::{Path, PathBuf};

/// One synthetic clip for a fixture timeline.
#[derive(Debug, Clone)]
pub struct ClipSpec {
    /// Display name.
    pub name: String,
    /// Stable clip uuid stored in Awidat metadata.
    pub uuid: String,
    /// Project-relative media path.
    pub asset: String,
    /// Source start in seconds.
    pub start_s: f64,
    /// Source duration in seconds.
    pub duration_s: f64,
    /// Optional transcript anchor snippet.
    pub transcript: Option<String>,
}

impl ClipSpec {
    /// Convenience constructor for a single source-backed video clip.
    pub fn video(name: &str, uuid: &str, asset: &str, start_s: f64, duration_s: f64) -> Self {
        Self {
            name: name.into(),
            uuid: uuid.into(),
            asset: asset.into(),
            start_s,
            duration_s,
            transcript: None,
        }
    }

    /// Attach a transcript snippet anchor to the clip.
    pub fn with_transcript(mut self, snippet: &str) -> Self {
        self.transcript = Some(snippet.into());
        self
    }
}

/// Init an Awidat project and write a one-track timeline.
pub fn write_project_with_clips(root: &Path, name: &str, clips: &[ClipSpec]) -> Result<()> {
    Project::init(root)?;
    for clip in clips {
        write_asset(root, &clip.asset)?;
    }

    let mut project = Project::read(root)?;
    project.timeline = timeline_with_clips(name, clips);
    project.write(root)?;
    Ok(())
}

/// Build a timeline without touching disk.
pub fn timeline_with_clips(name: &str, clips: &[ClipSpec]) -> Timeline {
    let mut track = Track::empty("V1", TrackKind::Video);
    for spec in clips {
        let mut clip = Clip::empty(spec.name.clone());
        clip.media_reference = MediaReference::External(ExternalReference::new(&spec.asset));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(spec.start_s * 24.0, 24.0),
            RationalTime::new(spec.duration_s * 24.0, 24.0),
        ));
        let meta = clip.metadata.awidat.get_or_insert_with(Default::default);
        meta.extra.insert(
            "clip_uuid".into(),
            serde_json::Value::String(spec.uuid.clone()),
        );
        if let Some(snippet) = &spec.transcript {
            meta.anchor = Some(awidat_proto::awidat_meta::Anchor {
                transcript_snippet: Some(snippet.clone()),
                ..Default::default()
            });
        }
        track.children.push(TrackChild::Clip(clip));
    }

    let mut timeline = Timeline::empty(name);
    let mut stack = Stack::empty("root");
    stack.children.push(StackChild::Track(track));
    timeline.tracks = stack;
    timeline
}

/// Write a placeholder media asset.
pub fn write_asset(root: &Path, asset: &str) -> Result<PathBuf> {
    let path = root.join(asset);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, b"awidat eval placeholder media")?;
    Ok(path)
}

/// Write a whisper sidecar with word-level timing.
pub fn write_whisper_words(root: &Path, asset: &str, words: &[(&str, f64, f64)]) -> Result<()> {
    let path = root
        .join("index")
        .join("whisper")
        .join(format!("{asset}.json"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = serde_json::json!({
        "indexer": "whisper",
        "asset_id": asset,
        "data": {
            "words": words
                .iter()
                .map(|(text, start_s, end_s)| {
                    serde_json::json!({"text": text, "start_s": start_s, "end_s": end_s})
                })
                .collect::<Vec<_>>(),
            "segments": words
                .iter()
                .map(|(text, start_s, end_s)| {
                    serde_json::json!({"text": text, "start_s": start_s, "end_s": end_s})
                })
                .collect::<Vec<_>>()
        }
    });
    std::fs::write(path, serde_json::to_vec_pretty(&payload)?)?;
    Ok(())
}

/// Write the desktop-compatible silence sidecar for an asset.
pub fn write_silence_ranges(root: &Path, asset: &str, ranges: &[(f64, f64)]) -> Result<()> {
    let abs = write_asset(root, asset)?;
    let path = silence_sidecar_path(root, &abs);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = serde_json::json!({
        "ranges": ranges
            .iter()
            .map(|(start_s, end_s)| {
                serde_json::json!({"start_s": start_s, "end_s": end_s, "db_floor": -48.0})
            })
            .collect::<Vec<_>>(),
        "threshold_db": -40.0,
        "min_duration_s": 0.6,
    });
    std::fs::write(path, serde_json::to_vec_pretty(&payload)?)?;
    Ok(())
}

fn silence_sidecar_path(project_root: &Path, asset_abs_path: &Path) -> PathBuf {
    let stem = asset_abs_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset");
    project_root.join(".awidat").join("silences").join(format!(
        "{stem}-{:08x}.json",
        stable_path_hash(asset_abs_path)
    ))
}

fn stable_path_hash(path: &Path) -> u32 {
    let mut hash = 0x811c9dc5_u32;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

/// Count clips across all timeline tracks.
pub fn clip_count(timeline: &Timeline) -> usize {
    timeline
        .tracks
        .children
        .iter()
        .filter_map(|child| match child {
            StackChild::Track(track) => Some(track),
            _ => None,
        })
        .map(|track| {
            track
                .children
                .iter()
                .filter(|child| matches!(child, TrackChild::Clip(_)))
                .count()
        })
        .sum()
}

/// Return source range `(start_s, duration_s)` for a clip uuid.
pub fn clip_range_by_uuid(timeline: &Timeline, uuid: &str) -> Option<(f64, f64)> {
    for child in &timeline.tracks.children {
        let StackChild::Track(track) = child else {
            continue;
        };
        for track_child in &track.children {
            let TrackChild::Clip(clip) = track_child else {
                continue;
            };
            let clip_uuid = clip
                .metadata
                .awidat
                .as_ref()
                .and_then(|m| m.extra.get("clip_uuid"))
                .and_then(|v| v.as_str());
            if clip_uuid == Some(uuid)
                && let Some(range) = clip.source_range.as_ref()
            {
                return Some((range.start_time.to_seconds(), range.duration.to_seconds()));
            }
        }
    }
    None
}
