//! Timeline-pane Tauri commands. Reads `project.otio.json` and
//! returns a flattened, frontend-friendly view: per-track lists of
//! clips with `start_s` / `duration_s` and the asset they reference.
//!
//! The shape we return is intentionally small — the timeline canvas
//! draws rounded rectangles + a ruler, not the full OTIO graph.
//! Transitions, effects, and gaps are surfaced as their own variants
//! so the frontend can color them differently.

use std::path::Path;

use awidat_proto::otio::{MediaReference, StackChild, TrackChild, TrackKind};
use awidat_proto::project::Project;
use serde::Serialize;
use tauri::State;

use crate::state::AwidatState;

/// One row in the timeline pane — a track with a sequence of items.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineTrack {
    /// Track name from the OTIO file.
    pub name: String,
    /// Track kind: "video" or "audio".
    pub kind: String,
    /// Items in this track in playback order.
    pub items: Vec<TimelineItem>,
}

/// One drawable item on a track. Variant-tagged so the frontend can
/// render each kind differently.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimelineItem {
    /// A clip — references an asset, has a source range.
    Clip {
        /// Index of this item within its track. Stable across reads.
        index: usize,
        /// Display name (clip's OTIO `name` field).
        name: String,
        /// Start of this clip on the track timeline, in seconds.
        track_start_s: f64,
        /// Duration on the track, in seconds.
        duration_s: f64,
        /// Asset id (project-relative path), if the clip references
        /// one. `None` for clips with missing or non-external refs.
        asset_id: Option<String>,
        /// Source-asset start offset, in seconds. Useful when the
        /// frontend wants to map "this clip plays source[12.5s..]".
        source_start_s: Option<f64>,
    },
    /// Empty time on the track (silence / black frames).
    Gap {
        index: usize,
        track_start_s: f64,
        duration_s: f64,
    },
    /// Transition (cross-dissolve, fade) between two clips. Sits at
    /// the cut point; doesn't have its own duration on the timeline.
    Transition {
        index: usize,
        track_start_s: f64,
        /// Cumulative effect duration (in_offset + out_offset).
        duration_s: f64,
        /// Effect name from the OTIO transition (e.g.
        /// "SMPTE_Dissolve").
        effect_name: String,
    },
}

/// Snapshot of the project's timeline. Empty `tracks` is a normal
/// "fresh project, no clips yet" state — not an error.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineSnapshot {
    /// Total project duration in seconds (max track end across all
    /// tracks). Zero when timeline is empty.
    pub duration_s: f64,
    /// Tracks in order: video first, then audio. Empty when project
    /// has no clips.
    pub tracks: Vec<TimelineTrack>,
}

/// Read `<project>/project.otio.json` and return the flattened
/// timeline view. Empty snapshot when no project loaded or OTIO
/// has no clips.
#[tauri::command]
pub async fn read_timeline(
    state: State<'_, AwidatState>,
) -> Result<TimelineSnapshot, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(empty_snapshot()),
    };
    // Project::read is sync; spawn_blocking so we don't hold the
    // tokio runtime on disk I/O.
    let snapshot = tokio::task::spawn_blocking(move || -> Result<TimelineSnapshot, String> {
        let project = Project::read(&project_root)
            .map_err(|e| format!("read project: {e}"))?;
        Ok(flatten_timeline(&project, &project_root))
    })
    .await
    .map_err(|e| format!("join: {e}"))??;
    Ok(snapshot)
}

fn empty_snapshot() -> TimelineSnapshot {
    TimelineSnapshot {
        duration_s: 0.0,
        tracks: Vec::new(),
    }
}

fn flatten_timeline(project: &Project, project_root: &Path) -> TimelineSnapshot {
    let mut tracks: Vec<TimelineTrack> = Vec::new();
    let mut max_end_s = 0.0_f64;

    for stack_child in &project.timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            // Nested stacks aren't drawn in v1 — they're a multi-cam
            // primitive we'll wire up later.
            continue;
        };
        let kind_str = match track.kind {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
        };
        let mut items = Vec::with_capacity(track.children.len());
        let mut track_cursor_s = 0.0_f64;

        for (i, child) in track.children.iter().enumerate() {
            match child {
                TrackChild::Clip(clip) => {
                    let duration_s = clip
                        .source_range
                        .as_ref()
                        .map(|r| r.duration.to_seconds())
                        .unwrap_or(0.0);
                    let source_start_s = clip
                        .source_range
                        .as_ref()
                        .map(|r| r.start_time.to_seconds());
                    let asset_id = match &clip.media_reference {
                        MediaReference::External(ext) => Some(
                            project_root_relative(project_root, &ext.target_url),
                        ),
                        MediaReference::Missing(_) => None,
                    };
                    items.push(TimelineItem::Clip {
                        index: i,
                        name: clip.name.clone(),
                        track_start_s: track_cursor_s,
                        duration_s,
                        asset_id,
                        source_start_s,
                    });
                    track_cursor_s += duration_s;
                }
                TrackChild::Gap(gap) => {
                    let duration_s = gap.source_range.duration.to_seconds();
                    items.push(TimelineItem::Gap {
                        index: i,
                        track_start_s: track_cursor_s,
                        duration_s,
                    });
                    track_cursor_s += duration_s;
                }
                TrackChild::Transition(t) => {
                    let duration_s = t.in_offset.to_seconds() + t.out_offset.to_seconds();
                    items.push(TimelineItem::Transition {
                        index: i,
                        // Transition straddles the cut between
                        // surrounding clips; we anchor it at the
                        // current cursor for drawing purposes. It
                        // does NOT advance the cursor — its time
                        // overlaps the neighboring clips.
                        track_start_s: track_cursor_s,
                        duration_s,
                        effect_name: t.transition_type.clone(),
                    });
                }
                TrackChild::Stack(_) => {
                    // Nested stacks: skipped in v1. The cursor doesn't
                    // advance — they don't draw on this row.
                }
            }
        }

        if track_cursor_s > max_end_s {
            max_end_s = track_cursor_s;
        }
        tracks.push(TimelineTrack {
            name: track.name.clone(),
            kind: kind_str.into(),
            items,
        });
    }

    // Sort: video tracks first (shown above audio in the pane),
    // preserve original order within each kind.
    tracks.sort_by_key(|t| if t.kind == "video" { 0 } else { 1 });

    TimelineSnapshot {
        duration_s: max_end_s,
        tracks,
    }
}

/// OTIO `target_url` may be project-relative or absolute. Normalize
/// to project-relative for the frontend (consistent with
/// `list_assets`'s output).
fn project_root_relative(project_root: &Path, target_url: &str) -> String {
    let p = std::path::PathBuf::from(target_url);
    if p.is_absolute() {
        match p.strip_prefix(project_root) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => target_url.to_string(),
        }
    } else {
        target_url.to_string()
    }
}
