//! Auto-insert imported assets onto the timeline.
//!
//! Standard editor convention is "import → drag onto timeline." For
//! montage's first-asset-on-fresh-project case that's a step too
//! many — the user already has nothing on the timeline, they just
//! imported the only thing, of course they want to see it. We also
//! support explicit multi-file imports by appending each selected
//! asset in order.

use std::path::Path;

use montage_core::edl::{AnchorContext, EdlEnvelope, EdlOp, InsertTrackKind, apply};
use montage_proto::otio::{StackChild, TrackChild, TrackKind};
use montage_proto::project::Project;

/// True iff the timeline contains at least one Clip on any video track.
/// Gaps and audio-only tracks don't count — a fresh project may
/// start with audio-tracking placeholders.
#[allow(dead_code)]
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

fn timeline_has_any_clips(project: &Project) -> bool {
    project.timeline.tracks.children.iter().any(|stack_child| {
        let StackChild::Track(track) = stack_child else {
            return false;
        };
        track
            .children
            .iter()
            .any(|child| matches!(child, TrackChild::Clip(_)))
    })
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
/// caller, e.g. via `montage_render::probe_duration_s` after the
/// proxy transcode finishes).
#[allow(dead_code)]
pub async fn auto_insert_if_empty(
    project_root: &Path,
    asset_abs_path: &Path,
    duration_s: f64,
) -> Result<bool, String> {
    insert_asset(
        project_root,
        asset_abs_path,
        duration_s,
        InsertMode::IfEmpty,
    )
    .await
}

/// Append the asset to the project's primary video track regardless
/// of whether other clips already exist. Used for explicit multi-file
/// imports where every selected source should become editable on the
/// timeline.
#[allow(dead_code)]
pub async fn append_asset(
    project_root: &Path,
    asset_abs_path: &Path,
    duration_s: f64,
) -> Result<bool, String> {
    insert_asset(project_root, asset_abs_path, duration_s, InsertMode::Append).await
}

pub async fn append_media(
    project_root: &Path,
    asset_abs_path: &Path,
    probe: &montage_render::MediaProbe,
) -> Result<bool, String> {
    insert_media(project_root, asset_abs_path, probe, InsertMode::Append).await
}

pub async fn insert_media_at(
    project_root: &Path,
    asset_abs_path: &Path,
    probe: &montage_render::MediaProbe,
    at_s: f64,
) -> Result<bool, String> {
    insert_media(project_root, asset_abs_path, probe, InsertMode::At(at_s)).await
}

#[derive(Debug, Clone, Copy)]
enum InsertMode {
    IfEmpty,
    Append,
    At(f64),
}

#[allow(dead_code)]
async fn insert_asset(
    project_root: &Path,
    asset_abs_path: &Path,
    duration_s: f64,
    mode: InsertMode,
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
    // Auto-insert runs as a user-initiated side effect of import, so
    // attribute the resulting vedit commit to the seat-holder rather
    // than the agent default.
    let seat_author = crate::commands::vedit::desktop_commit_author();
    tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let project =
            Project::read(&project_root).map_err(|e| format!("auto-insert: read project: {e}"))?;

        if matches!(mode, InsertMode::IfEmpty) && timeline_has_video_clips(&project) {
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
                track_kind: None,
                at_position: None,
                at_s: None,
                start: Some(0.0),
                end: Some(duration_s),
                name: None,
                link_group_id: None,
                snap: None,
            }],
        };

        let ctx = AnchorContext::with_project_root(&project_root);
        let (new_timeline, outcome) = apply(&project.timeline, &envelope, &ctx)
            .map_err(|e| format!("auto-insert: edl apply: {e}"))?;
        let applied_descriptions: Vec<String> = outcome
            .applied
            .iter()
            .map(|a| a.description.clone())
            .collect();
        let action_metadata = montage_core::vc::ActionMetadata {
            source: Some("agent".into()),
            operations: outcome.applied.iter().map(|a| a.metadata.clone()).collect(),
        };

        // Persist. Project::write writes the OTIO + edit-plan +
        // manifest atomically; we only mutated the timeline.
        let mut updated = project;
        updated.timeline = new_timeline;
        updated
            .write(&project_root)
            .map_err(|e| format!("auto-insert: write project: {e}"))?;

        // Keep the implicit import -> timeline graph mutation in the
        // same audit trail as explicit apply_edl/proposal writes.
        match montage_core::vc::open_or_init(&project_root) {
            Ok(repo) => {
                if let Err(e) = montage_core::vc::auto_commit_apply_as_with_metadata(
                    &repo,
                    &applied_descriptions,
                    Some("Auto-inserted the first imported asset onto the empty timeline."),
                    seat_author,
                    Some(&action_metadata),
                ) {
                    tracing::warn!(
                        error = %e,
                        "vedit auto-commit failed (auto-insert path)"
                    );
                }
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "vedit repo unavailable; skipping auto-insert commit"
                );
            }
        }
        Ok(true)
    })
    .await
    .map_err(|e| format!("auto-insert: join: {e}"))?
}

async fn insert_media(
    project_root: &Path,
    asset_abs_path: &Path,
    probe: &montage_render::MediaProbe,
    mode: InsertMode,
) -> Result<bool, String> {
    let duration_s = probe.duration_s.ok_or_else(|| {
        format!(
            "auto-insert: ffprobe couldn't determine duration for {}",
            asset_abs_path.display()
        )
    })?;
    if !probe.has_audio && !probe.has_video {
        return Err(format!(
            "auto-insert: {} has no audio or video streams",
            asset_abs_path.display()
        ));
    }
    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err(format!(
            "auto-insert: invalid duration {duration_s} for {}",
            asset_abs_path.display()
        ));
    }

    let project_root = project_root.to_path_buf();
    let asset_abs_path = asset_abs_path.to_path_buf();
    let has_video = probe.has_video;
    let has_audio = probe.has_audio;
    // Resolve seat-holder identity before crossing the spawn_blocking
    // boundary so the auto-commit is attributed to the user, not the
    // agent default.
    let seat_author = crate::commands::vedit::desktop_commit_author();
    tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let project =
            Project::read(&project_root).map_err(|e| format!("auto-insert: read project: {e}"))?;
        if matches!(mode, InsertMode::IfEmpty) && timeline_has_any_clips(&project) {
            return Ok(false);
        }
        let at_s = match mode {
            InsertMode::At(at_s) => {
                if !at_s.is_finite() || at_s < 0.0 {
                    return Err(format!(
                        "auto-insert: at_s must be a finite non-negative timeline time, got {at_s}"
                    ));
                }
                Some(at_s)
            }
            InsertMode::IfEmpty | InsertMode::Append => None,
        };
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

        let link_group_id = if has_video && has_audio {
            Some(format!(
                "lg-{}",
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            ))
        } else {
            None
        };
        let mut ops = Vec::new();
        if has_video {
            ops.push(EdlOp::InsertClip {
                asset: asset_rel.clone(),
                track: "Video 1".into(),
                track_kind: Some(InsertTrackKind::Video),
                at_position: None,
                at_s,
                start: Some(0.0),
                end: Some(duration_s),
                name: None,
                link_group_id: link_group_id.clone(),
                snap: None,
            });
        }
        if has_audio {
            ops.push(EdlOp::InsertClip {
                asset: asset_rel,
                track: "A1".into(),
                track_kind: Some(InsertTrackKind::Audio),
                at_position: None,
                at_s,
                start: Some(0.0),
                end: Some(duration_s),
                name: None,
                link_group_id,
                snap: None,
            });
        }

        let envelope = EdlEnvelope { ops };
        let ctx = AnchorContext::with_project_root(&project_root);
        let (new_timeline, outcome) = apply(&project.timeline, &envelope, &ctx)
            .map_err(|e| format!("auto-insert: edl apply: {e}"))?;
        let applied_descriptions: Vec<String> = outcome
            .applied
            .iter()
            .map(|a| a.description.clone())
            .collect();
        let action_metadata = montage_core::vc::ActionMetadata {
            source: Some("agent".into()),
            operations: outcome.applied.iter().map(|a| a.metadata.clone()).collect(),
        };

        let mut updated = project;
        updated.timeline = new_timeline;
        updated
            .write(&project_root)
            .map_err(|e| format!("auto-insert: write project: {e}"))?;

        if let Ok(repo) = montage_core::vc::open_or_init(&project_root)
            && let Err(e) = montage_core::vc::auto_commit_apply_as_with_metadata(
                &repo,
                &applied_descriptions,
                Some("Auto-inserted imported media onto the timeline."),
                seat_author,
                Some(&action_metadata),
            )
        {
            tracing::warn!(error = %e, "vedit auto-commit failed (auto-insert media path)");
        }
        Ok(true)
    })
    .await
    .map_err(|e| format!("auto-insert: join: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use montage_proto::otio::{MediaReference, StackChild, TrackChild};

    fn clip_assets(project: &Project) -> Vec<String> {
        let mut out = Vec::new();
        for stack_child in &project.timeline.tracks.children {
            let StackChild::Track(track) = stack_child else {
                continue;
            };
            for child in &track.children {
                let TrackChild::Clip(clip) = child else {
                    continue;
                };
                if let MediaReference::External(ext) = &clip.media_reference {
                    out.push(ext.target_url.clone());
                }
            }
        }
        out
    }

    #[tokio::test]
    async fn append_asset_adds_second_import_to_existing_timeline() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        Project::init(root).unwrap();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        let first = root.join("raw/first.mp4");
        let second = root.join("raw/second.mp4");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();

        assert!(auto_insert_if_empty(root, &first, 10.0).await.unwrap());
        assert!(append_asset(root, &second, 12.0).await.unwrap());

        let project = Project::read(root).unwrap();
        assert_eq!(
            clip_assets(&project),
            vec!["raw/first.mp4".to_string(), "raw/second.mp4".to_string()]
        );
    }

    #[tokio::test]
    async fn auto_insert_if_empty_keeps_non_empty_timeline_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        Project::init(root).unwrap();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        let first = root.join("raw/first.mp4");
        let second = root.join("raw/second.mp4");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();

        assert!(auto_insert_if_empty(root, &first, 10.0).await.unwrap());
        assert!(!auto_insert_if_empty(root, &second, 12.0).await.unwrap());

        let project = Project::read(root).unwrap();
        assert_eq!(clip_assets(&project), vec!["raw/first.mp4".to_string()]);
    }
}
