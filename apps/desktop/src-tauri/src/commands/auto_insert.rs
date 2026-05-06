//! Auto-insert the first imported asset onto an empty timeline.
//!
//! Standard editor convention is "import → drag onto timeline." For
//! awidat's first-asset-on-fresh-project case that's a step too
//! many — the user already has nothing on the timeline, they just
//! imported the only thing, of course they want to see it. We
//! detect that case and append the clip programmatically; non-empty
//! timelines stay untouched (the user is explicitly arranging).

use std::path::Path;

use awidat_core::edl::{AnchorContext, EdlEnvelope, EdlOp, apply};
use awidat_proto::otio::{StackChild, TrackChild, TrackKind};
use awidat_proto::project::Project;

/// True iff the timeline contains at least one Clip on any video track.
/// Gaps and audio-only tracks don't count — a fresh project may
/// start with audio-tracking placeholders.
pub fn timeline_has_video_clips(project: &Project) -> bool {
    for stack_child in &project.timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        if !matches!(track.kind, TrackKind::Video) {
            continue;
        }
        for child in &track.children {
            if matches!(child, TrackChild::Clip(_)) {
                return true;
            }
        }
    }
    false
}

/// Append the asset at `asset_abs_path` to the project's video track
/// as a single clip spanning its full duration. No-op if the
/// timeline already has any video clip — non-empty timelines mean
/// the user is arranging manually and we don't barge in.
///
/// `asset_abs_path` must be inside `<project>/raw/`; we derive the
/// `raw/<filename>` asset id from the project-relative path.
///
/// `duration_s` is the asset's source duration (probed by the
/// caller, e.g. via `awidat_render::probe_duration_s` after the
/// proxy transcode finishes).
pub async fn auto_insert_if_empty(
    project_root: &Path,
    asset_abs_path: &Path,
    duration_s: f64,
) -> Result<bool, String> {
    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err(format!(
            "auto-insert: invalid duration {duration_s} for {}",
            asset_abs_path.display()
        ));
    }

    // Wrap the sync work in spawn_blocking — Project::read/write hit
    // disk and the EDL apply path is CPU-bound. We don't want to
    // hold the tokio runtime on either.
    let project_root = project_root.to_path_buf();
    let asset_abs_path = asset_abs_path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let project =
            Project::read(&project_root).map_err(|e| format!("auto-insert: read project: {e}"))?;

        if timeline_has_video_clips(&project) {
            // User has clips on the timeline already — don't disturb.
            return Ok(false);
        }

        // Derive the project-relative asset id.
        let asset_rel = match asset_abs_path.strip_prefix(&project_root) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => {
                return Err(format!(
                    "auto-insert: asset {} is not inside project {}",
                    asset_abs_path.display(),
                    project_root.display()
                ));
            }
        };

        let envelope = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: asset_rel,
                track: "Video 1".into(),
                at_position: None,
                start: Some(0.0),
                end: Some(duration_s),
                name: None,
            }],
        };

        let ctx = AnchorContext::with_project_root(&project_root);
        let (new_timeline, _outcome) = apply(&project.timeline, &envelope, &ctx)
            .map_err(|e| format!("auto-insert: edl apply: {e}"))?;

        // Persist. Project::write writes the OTIO + edit-plan +
        // manifest atomically; we only mutated the timeline.
        let mut updated = project;
        updated.timeline = new_timeline;
        updated
            .write(&project_root)
            .map_err(|e| format!("auto-insert: write project: {e}"))?;
        Ok(true)
    })
    .await
    .map_err(|e| format!("auto-insert: join: {e}"))?
}
