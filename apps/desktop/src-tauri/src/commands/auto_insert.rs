//! Insert explicitly imported media onto the timeline, appending or placing at a chosen time.

use std::path::Path;

use montage_core::edl::{AnchorContext, EdlEnvelope, EdlOp, InsertTrackKind, apply};
use montage_proto::project::Project;

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
    Append,
    At(f64),
}

fn project_relative_asset_id(project_root: &Path, asset_abs_path: &Path) -> Result<String, String> {
    let canonical_root = project_root.canonicalize().map_err(|e| {
        format!(
            "auto-insert: resolve project {}: {e}",
            project_root.display()
        )
    })?;
    let canonical_asset = asset_abs_path.canonicalize().map_err(|e| {
        format!(
            "auto-insert: resolve asset {}: {e}",
            asset_abs_path.display()
        )
    })?;
    canonical_asset
        .strip_prefix(&canonical_root)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .map_err(|_| {
            format!(
                "auto-insert: asset {} is not inside project {}",
                asset_abs_path.display(),
                project_root.display()
            )
        })
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
        let _mutation = montage_core::vc::lock_timeline_mutation(&project_root)
            .map_err(|e| format!("auto-insert: lock timeline mutation: {e}"))?;
        let project =
            Project::read(&project_root).map_err(|e| format!("auto-insert: read project: {e}"))?;
        let at_s = match mode {
            InsertMode::At(at_s) => {
                if !at_s.is_finite() || at_s < 0.0 {
                    return Err(format!(
                        "auto-insert: at_s must be a finite non-negative timeline time, got {at_s}"
                    ));
                }
                Some(at_s)
            }
            InsertMode::Append => None,
        };
        let asset_rel = project_relative_asset_id(&project_root, &asset_abs_path)?;

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

    fn video_probe(duration_s: f64) -> montage_render::MediaProbe {
        montage_render::MediaProbe {
            duration_s: Some(duration_s),
            has_video: true,
            has_audio: false,
            stream_types: vec!["video".into()],
            video_width: Some(16),
            video_height: Some(16),
        }
    }

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
    async fn append_media_adds_second_import_to_existing_timeline() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        Project::init(root).unwrap();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        let first = root.join("raw/first.mp4");
        let second = root.join("raw/second.mp4");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();

        assert!(
            append_media(root, &first, &video_probe(10.0))
                .await
                .unwrap()
        );
        assert!(
            append_media(root, &second, &video_probe(12.0))
                .await
                .unwrap()
        );

        let project = Project::read(root).unwrap();
        assert_eq!(
            clip_assets(&project),
            vec!["raw/first.mp4".to_string(), "raw/second.mp4".to_string()]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn auto_insert_accepts_canonical_asset_under_symlinked_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        let root_alias = dir.path().join("project-alias");
        Project::init(&root).unwrap();
        std::os::unix::fs::symlink(&root, &root_alias).unwrap();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        let asset = root.join("raw/clip.mp4");
        std::fs::write(&asset, b"clip").unwrap();

        assert!(
            append_media(&root_alias, &asset, &video_probe(3.0))
                .await
                .unwrap()
        );

        let project = Project::read(&root).unwrap();
        assert_eq!(clip_assets(&project), vec!["raw/clip.mp4".to_string()]);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn append_media_accepts_private_tmp_asset_for_tmp_project() {
        let dir = tempfile::Builder::new()
            .prefix("montage-auto-insert-")
            .tempdir_in("/tmp")
            .unwrap();
        let root = dir.path().join("project");
        Project::init(&root).unwrap();
        std::fs::create_dir_all(root.join("raw")).unwrap();
        let asset = root.join("raw/clip.mp4");
        std::fs::write(&asset, b"clip").unwrap();
        let canonical_asset = asset.canonicalize().unwrap();
        let probe = montage_render::MediaProbe {
            duration_s: Some(3.0),
            has_video: true,
            has_audio: false,
            stream_types: vec!["video".into()],
            video_width: Some(16),
            video_height: Some(16),
        };

        assert!(append_media(&root, &canonical_asset, &probe).await.unwrap());

        let project = Project::read(&root).unwrap();
        assert_eq!(clip_assets(&project), vec!["raw/clip.mp4".to_string()]);
    }
}
