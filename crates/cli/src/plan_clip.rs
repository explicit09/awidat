//! Shared timeline clip selection for deterministic planner commands.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use montage_proto::otio::{Clip, MediaReference, Stack, StackChild, Track, TrackChild, TrackKind};

/// External video clip selected for a planner command.
#[derive(Debug, Clone)]
pub struct SelectedClip {
    /// Stable clip anchor, preferring `metadata.montage.extra.clip_uuid`.
    pub clip_uuid: String,
    /// Project-relative or absolute media asset path.
    pub asset: String,
    /// Owning track name.
    pub track_name: String,
    /// Original child index in the owning track.
    pub track_position: usize,
    /// Source range start in media seconds.
    pub source_start_s: f64,
    /// Source range end in media seconds.
    pub source_end_s: f64,
}

/// Select the first active external video clip, or the clip matching `selector`.
pub fn select_clip(root: &Stack, selector: Option<&str>) -> Result<SelectedClip> {
    let clips = select_clips(root, selector)?;
    clips
        .into_iter()
        .next()
        .context("no active external video clip found on the timeline")
}

/// Select all active external video clips, or all clips matching `selector`.
pub fn select_clips(root: &Stack, selector: Option<&str>) -> Result<Vec<SelectedClip>> {
    let mut candidates = Vec::new();
    for child in &root.children {
        let StackChild::Track(track) = child else {
            continue;
        };
        if track.kind != TrackKind::Video {
            continue;
        }
        for (track_position, track_child) in track.children.iter().enumerate() {
            let TrackChild::Clip(clip) = track_child else {
                continue;
            };
            let Some(candidate) = clip_candidate(track, track_position, clip) else {
                continue;
            };
            if selector.is_none_or(|wanted| candidate.matches(wanted)) {
                candidates.push(candidate);
            }
        }
    }

    if candidates.is_empty() {
        if let Some(selector) = selector {
            bail!("no active external video clip matched {selector:?}");
        }
        bail!("no active external video clip found on the timeline");
    }
    Ok(candidates)
}

/// Resolve a selected asset relative to a project root.
pub fn resolve_asset_path(project_root: &Path, asset: &str) -> PathBuf {
    let path = Path::new(asset);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

fn clip_candidate(track: &Track, track_position: usize, clip: &Clip) -> Option<SelectedClip> {
    if !clip.active {
        return None;
    }
    let MediaReference::External(reference) = &clip.media_reference else {
        return None;
    };
    let range = clip.source_range?;
    let source_start_s = range.start_time.to_seconds();
    let source_end_s = source_start_s + range.duration.to_seconds();
    if source_end_s <= source_start_s {
        return None;
    }
    Some(SelectedClip {
        clip_uuid: clip_uuid(clip).unwrap_or(&clip.name).to_string(),
        asset: reference.target_url.clone(),
        track_name: track.name.clone(),
        track_position,
        source_start_s,
        source_end_s,
    })
}

fn clip_uuid(clip: &Clip) -> Option<&str> {
    clip.metadata
        .montage
        .as_ref()
        .and_then(|metadata| metadata.extra.get("clip_uuid"))
        .and_then(|value| value.as_str())
}

impl SelectedClip {
    fn matches(&self, selector: &str) -> bool {
        self.asset == selector
            || self.clip_uuid == selector
            || asset_name(&self.asset).is_some_and(|name| name == selector)
            || asset_stem(&self.asset).is_some_and(|stem| stem == selector)
    }
}

fn asset_name(asset: &str) -> Option<&str> {
    Path::new(asset).file_name().and_then(|name| name.to_str())
}

fn asset_stem(asset: &str) -> Option<&str> {
    Path::new(asset).file_stem().and_then(|stem| stem.to_str())
}
