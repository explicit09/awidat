//! Timeline-pane Tauri commands. Reads `project.otio.json` and
//! returns a flattened, frontend-friendly view via the protocol
//! crate's [`TimelineSnapshot`].
//!
//! The shape we return is intentionally small — the timeline canvas
//! draws rounded rectangles + a ruler, not the full OTIO graph.
//! Transitions and gaps surface as variants so the canvas can color
//! them differently.

use std::path::Path;

use awidat_desktop_protocol::{TimelineItem, TimelineSnapshot, TimelineTrack};
use awidat_proto::otio::{MediaReference, StackChild, TrackChild, TrackKind};
use awidat_proto::project::Project;
use tauri::State;

use crate::state::AwidatState;

/// Read `<project>/project.otio.json` and return the flattened
/// timeline view. Empty snapshot when no project loaded or OTIO
/// has no clips.
#[tauri::command]
pub async fn read_timeline(state: State<'_, AwidatState>) -> Result<TimelineSnapshot, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(empty_snapshot()),
    };
    // Project::read is sync; spawn_blocking so we don't hold the
    // tokio runtime on disk I/O.
    let snapshot = tokio::task::spawn_blocking(move || -> Result<TimelineSnapshot, String> {
        let project = Project::read(&project_root).map_err(|e| format!("read project: {e}"))?;
        Ok(flatten_timeline_public(&project.timeline, &project_root))
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

/// Public flattener: walk an OTIO `Timeline` and produce the
/// frontend-friendly `TimelineSnapshot`. Used by `read_timeline`
/// (which loads the timeline from disk) and by the proposal
/// pipeline (which already has the timeline in memory after
/// `apply()`).
pub fn flatten_timeline_public(
    timeline: &awidat_proto::otio::Timeline,
    project_root: &Path,
) -> TimelineSnapshot {
    let mut tracks: Vec<TimelineTrack> = Vec::new();
    let mut max_end_s = 0.0_f64;

    for stack_child in &timeline.tracks.children {
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
                        MediaReference::External(ext) => {
                            Some(project_root_relative(project_root, &ext.target_url))
                        }
                        MediaReference::Missing(_) => None,
                    };
                    // Resolve the proxy path once per clip so the
                    // frontend can play this segment without doing
                    // its own filesystem lookups. `None` is fine —
                    // the frontend treats it as "still transcoding"
                    // and falls back to its empty-state placeholder
                    // for that clip's window.
                    let proxy_path = asset_id
                        .as_deref()
                        .and_then(|aid| {
                            crate::commands::media::proxy_path_for_asset_id(project_root, aid)
                        });
                    // Resolve the thumbnails dir once per clip so the
                    // canvas can tile filmstrip frames inside the
                    // clip body. `None` while the post-import
                    // [`JobKind::Thumbnails`] job hasn't completed —
                    // the canvas falls back to the same coloured-rect
                    // it drew before Step 10.
                    let thumbnail_dir = asset_id
                        .as_deref()
                        .and_then(|aid| {
                            crate::commands::media::thumbnails_dir_for_asset_id(project_root, aid)
                        });
                    // Resolve the waveform sidecar path once per clip
                    // so audio tracks can draw the amplitude line.
                    // `None` while the post-import [`JobKind::Waveform`]
                    // job hasn't completed, AND when the asset has no
                    // audio stream (sidecar exists but its buckets
                    // array is empty).
                    let waveform_path = asset_id
                        .as_deref()
                        .and_then(|aid| {
                            crate::commands::media::waveform_path_for_asset_id(project_root, aid)
                        });
                    // Anchor uuid: prefer clip.metadata.awidat.extra["clip_uuid"];
                    // fall back to display name (the EDL resolver also
                    // matches names, so the fallback round-trips).
                    let clip_uuid = clip
                        .metadata
                        .awidat
                        .as_ref()
                        .and_then(|m| m.extra.get("clip_uuid"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| clip.name.clone());
                    items.push(TimelineItem::Clip {
                        index: i,
                        name: clip.name.clone(),
                        clip_uuid,
                        track_start_s: track_cursor_s,
                        duration_s,
                        asset_id,
                        source_start_s,
                        proxy_path,
                        thumbnail_dir,
                        waveform_path,
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
