//! Shared caption and subtitle inventory helpers.

use std::collections::BTreeMap;

use awidat_proto::otio::{Clip, Stack, StackChild, Track, TrackChild};
use awidat_proto::project::Project;
use serde::Serialize;

/// Caption source Awidat should treat as authoritative for delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionAuthority {
    /// Editable timeline subtitle tracks from `metadata.awidat.subtitle_tracks`.
    EditableSubtitleTracks,
    /// Burned-in caption overlay clips on the timeline.
    CaptionOverlays,
    /// Sidecar cues derived from transcript index sidecars.
    TranscriptFallback,
    /// No caption-like source is present.
    None,
}

impl CaptionAuthority {
    /// Stable string used in render/package metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EditableSubtitleTracks => "editable_subtitle_tracks",
            Self::CaptionOverlays => "caption_overlays",
            Self::TranscriptFallback => "transcript_fallback",
            Self::None => "none",
        }
    }
}

/// Caption and subtitle inventory for a loaded project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaptionSummary {
    /// Source selected as the project caption authority.
    pub selected_authority: CaptionAuthority,
    /// Number of editable subtitle tracks.
    pub editable_track_count: usize,
    /// Number of cues across editable subtitle tracks.
    pub editable_cue_count: usize,
    /// Number of caption-shaped overlay clips.
    pub caption_overlay_count: usize,
    /// Number of caption overlay clips carrying word timing metadata.
    pub word_timed_caption_overlay_count: usize,
    /// Number of caption overlay clips carrying an explicit safe-area profile.
    pub safe_area_caption_overlay_count: usize,
    /// Number of caption overlay clips using the mobile/vertical safe-area profile.
    pub mobile_safe_area_caption_overlay_count: usize,
    /// Number of caption overlay clips missing safe-area profile metadata.
    pub missing_safe_area_caption_overlay_count: usize,
    /// Number of sidecar cues that package export will write.
    pub sidecar_cue_count: usize,
    /// Whether any caption-like source exists.
    pub has_exportable_captions: bool,
    /// Human-readable routing warnings for package/export metadata.
    pub warnings: Vec<String>,
}

/// Summarize caption/subtitle state without transcript-derived sidecar cues.
pub fn summarize_captions(project: &Project) -> CaptionSummary {
    summarize_captions_with_sidecar_cue_count(project, 0)
}

/// Summarize caption/subtitle state with the cue count selected for sidecars.
pub fn summarize_captions_with_sidecar_cue_count(
    project: &Project,
    sidecar_cue_count: usize,
) -> CaptionSummary {
    let editable_track_count = project
        .timeline
        .metadata
        .awidat
        .as_ref()
        .map(|metadata| metadata.subtitle_tracks.len())
        .unwrap_or(0);
    let editable_cue_count = project
        .timeline
        .metadata
        .awidat
        .as_ref()
        .map(|metadata| {
            metadata
                .subtitle_tracks
                .iter()
                .map(|track| track.cues.len())
                .sum()
        })
        .unwrap_or(0);
    let overlay_counts = count_caption_overlays_in_stack(&project.timeline.tracks);
    let selected_authority = select_caption_authority(
        editable_cue_count,
        overlay_counts.caption_overlays,
        sidecar_cue_count,
    );
    let has_exportable_captions = !matches!(selected_authority, CaptionAuthority::None);
    let warnings = build_caption_warnings(
        selected_authority,
        editable_cue_count,
        overlay_counts.caption_overlays,
        overlay_counts.missing_safe_area_caption_overlays,
        sidecar_cue_count,
    );

    CaptionSummary {
        selected_authority,
        editable_track_count,
        editable_cue_count,
        caption_overlay_count: overlay_counts.caption_overlays,
        word_timed_caption_overlay_count: overlay_counts.word_timed_caption_overlays,
        safe_area_caption_overlay_count: overlay_counts.safe_area_caption_overlays,
        mobile_safe_area_caption_overlay_count: overlay_counts.mobile_safe_area_caption_overlays,
        missing_safe_area_caption_overlay_count: overlay_counts.missing_safe_area_caption_overlays,
        sidecar_cue_count,
        has_exportable_captions,
        warnings,
    }
}

/// Stable render/package metadata fields derived from a caption summary.
pub fn caption_summary_metadata(summary: &CaptionSummary) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "caption_authority".into(),
            summary.selected_authority.as_str().into(),
        ),
        (
            "caption_overlay_count".into(),
            summary.caption_overlay_count.to_string(),
        ),
        (
            "word_timed_caption_overlay_count".into(),
            summary.word_timed_caption_overlay_count.to_string(),
        ),
        (
            "safe_area_caption_overlay_count".into(),
            summary.safe_area_caption_overlay_count.to_string(),
        ),
        (
            "mobile_safe_area_caption_overlay_count".into(),
            summary.mobile_safe_area_caption_overlay_count.to_string(),
        ),
        (
            "missing_safe_area_caption_overlay_count".into(),
            summary.missing_safe_area_caption_overlay_count.to_string(),
        ),
        (
            "subtitle_sidecar_cue_count".into(),
            summary.sidecar_cue_count.to_string(),
        ),
        (
            "caption_warning_count".into(),
            summary.warnings.len().to_string(),
        ),
    ])
}

#[derive(Debug, Default)]
struct CaptionOverlayCounts {
    caption_overlays: usize,
    word_timed_caption_overlays: usize,
    safe_area_caption_overlays: usize,
    mobile_safe_area_caption_overlays: usize,
    missing_safe_area_caption_overlays: usize,
}

fn count_caption_overlays_in_stack(stack: &Stack) -> CaptionOverlayCounts {
    let mut counts = CaptionOverlayCounts::default();
    for child in &stack.children {
        match child {
            StackChild::Track(track) => count_caption_overlays_in_track(track, &mut counts),
            StackChild::Stack(stack) => {
                let nested = count_caption_overlays_in_stack(stack);
                counts.merge(nested);
            }
            StackChild::Clip(clip) => count_caption_overlay_clip(clip, &mut counts),
            StackChild::Gap(_) => {}
        }
    }
    counts
}

fn count_caption_overlays_in_track(track: &Track, counts: &mut CaptionOverlayCounts) {
    for child in &track.children {
        match child {
            TrackChild::Clip(clip) => count_caption_overlay_clip(clip, counts),
            TrackChild::Stack(stack) => {
                let nested = count_caption_overlays_in_stack(stack);
                counts.merge(nested);
            }
            TrackChild::Gap(_) | TrackChild::Transition(_) => {}
        }
    }
}

fn count_caption_overlay_clip(clip: &Clip, counts: &mut CaptionOverlayCounts) {
    let Some(caption_effect) = clip.effects.iter().find(|effect| {
        matches!(
            effect
                .metadata
                .get("role")
                .and_then(serde_json::Value::as_str),
            Some("caption" | "captions" | "subtitle" | "subtitles")
        )
    }) else {
        return;
    };
    counts.caption_overlays += 1;
    match caption_effect
        .metadata
        .get("safe_area")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some("mobile" | "9:16" | "vertical") => {
            counts.safe_area_caption_overlays += 1;
            counts.mobile_safe_area_caption_overlays += 1;
        }
        Some(_) => counts.safe_area_caption_overlays += 1,
        None => counts.missing_safe_area_caption_overlays += 1,
    }
    if caption_effect
        .metadata
        .get("word_timings")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|timings| !timings.is_empty())
    {
        counts.word_timed_caption_overlays += 1;
    }
}

impl CaptionOverlayCounts {
    fn merge(&mut self, other: Self) {
        self.caption_overlays += other.caption_overlays;
        self.word_timed_caption_overlays += other.word_timed_caption_overlays;
        self.safe_area_caption_overlays += other.safe_area_caption_overlays;
        self.mobile_safe_area_caption_overlays += other.mobile_safe_area_caption_overlays;
        self.missing_safe_area_caption_overlays += other.missing_safe_area_caption_overlays;
    }
}

fn select_caption_authority(
    editable_cue_count: usize,
    caption_overlay_count: usize,
    sidecar_cue_count: usize,
) -> CaptionAuthority {
    if editable_cue_count > 0 {
        CaptionAuthority::EditableSubtitleTracks
    } else if caption_overlay_count > 0 {
        CaptionAuthority::CaptionOverlays
    } else if sidecar_cue_count > 0 {
        CaptionAuthority::TranscriptFallback
    } else {
        CaptionAuthority::None
    }
}

fn build_caption_warnings(
    selected_authority: CaptionAuthority,
    editable_cue_count: usize,
    caption_overlay_count: usize,
    missing_safe_area_caption_overlay_count: usize,
    sidecar_cue_count: usize,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if selected_authority == CaptionAuthority::EditableSubtitleTracks && caption_overlay_count > 0 {
        warnings.push(
            "caption overlays exist but editable subtitle tracks are the export authority".into(),
        );
    }
    if selected_authority == CaptionAuthority::CaptionOverlays && sidecar_cue_count > 0 {
        warnings.push(
            "caption overlays exist but package sidecars were derived from transcript fallback cues"
                .into(),
        );
    }
    if selected_authority == CaptionAuthority::CaptionOverlays && editable_cue_count == 0 {
        warnings.push(
            "caption overlays exist without editable subtitle cues; sidecar subtitle export may be empty unless transcript fallback cues are available"
                .into(),
        );
    }
    if missing_safe_area_caption_overlay_count > 0 {
        warnings.push(format!(
            "{missing_safe_area_caption_overlay_count} caption overlays missing safe_area metadata; caption layout may ignore platform safe-area policy"
        ));
    }
    warnings
}
