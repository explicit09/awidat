//! Progressive media intelligence state builder.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use montage_index::media_files::{MediaScanOptions, collect_project_media_files};
use montage_index::sidecar_path;
use montage_proto::index::AssetId;
use montage_proto::professional::{
    MediaIntelligenceAggregateState, MediaIntelligenceAsset, MediaIntelligenceLayer,
    MediaIntelligenceLayerKind, MediaIntelligenceLayerStatus, MediaIntelligencePackage,
};
use serde_json::Value;

use crate::preview_cache::waveform_path_for;
use crate::proxy::{ProxyStatus, proxy_path_for, proxy_status_for};

/// Build progressive intelligence state for every raw media asset in a project.
pub fn build_media_intelligence_package(
    project_root: &Path,
) -> std::io::Result<MediaIntelligencePackage> {
    let files = collect_project_media_files(
        project_root,
        MediaScanOptions {
            include_raw: true,
            include_renders: false,
            max_files: None,
        },
    )?;
    let assets = files
        .into_iter()
        .map(|file| {
            build_media_intelligence_for_asset(
                project_root,
                &file.project_relative_path,
                &file.path,
            )
        })
        .collect();
    Ok(MediaIntelligencePackage { assets })
}

/// Build progressive intelligence state for one asset id and source path.
pub fn build_media_intelligence_for_asset(
    project_root: &Path,
    asset_id: &str,
    asset_path: &Path,
) -> MediaIntelligenceAsset {
    let source_exists = asset_path.is_file();
    let mut layers = Vec::new();
    layers.push(source_layer(project_root, asset_path));
    layers.push(proxy_layer(
        project_root,
        asset_id,
        asset_path,
        source_exists,
    ));
    layers.push(waveform_layer(project_root, asset_path, source_exists));
    layers.push(sidecar_layer(
        project_root,
        asset_id,
        asset_path,
        source_exists,
        MediaIntelligenceLayerKind::Transcript,
        "whisper",
        transcript_ready,
    ));
    layers.push(sidecar_layer(
        project_root,
        asset_id,
        asset_path,
        source_exists,
        MediaIntelligenceLayerKind::Speakers,
        "whisper",
        speakers_ready,
    ));
    layers.push(sidecar_layer(
        project_root,
        asset_id,
        asset_path,
        source_exists,
        MediaIntelligenceLayerKind::Scenes,
        "scenedetect",
        |value| any_array(value, &["shots", "scenes"]),
    ));
    layers.push(sidecar_layer(
        project_root,
        asset_id,
        asset_path,
        source_exists,
        MediaIntelligenceLayerKind::Topics,
        "topic",
        |value| any_array(value, &["topics"]),
    ));
    layers.push(sidecar_layer(
        project_root,
        asset_id,
        asset_path,
        source_exists,
        MediaIntelligenceLayerKind::Moments,
        "editorial-moments",
        |value| any_array(value, &["moments"]),
    ));
    layers.push(sidecar_layer(
        project_root,
        asset_id,
        asset_path,
        source_exists,
        MediaIntelligenceLayerKind::ClipCandidates,
        "clip",
        |value| {
            has_any(
                value,
                &["embeddings_b64", "frame_count", "clips", "candidates"],
            )
        },
    ));
    layers.push(broll_layer(
        project_root,
        asset_id,
        asset_path,
        source_exists,
    ));

    let aggregate_state = aggregate_state(&layers);
    let recommended_actions = recommended_actions(&layers);
    MediaIntelligenceAsset {
        asset_id: asset_id.to_string(),
        layers,
        aggregate_state,
        recommended_actions,
    }
}

fn source_layer(project_root: &Path, asset_path: &Path) -> MediaIntelligenceLayer {
    let exists = asset_path.is_file();
    MediaIntelligenceLayer {
        kind: MediaIntelligenceLayerKind::Source,
        status: if exists {
            MediaIntelligenceLayerStatus::Ready
        } else {
            MediaIntelligenceLayerStatus::Blocked
        },
        artifact_refs: vec![artifact_ref(project_root, asset_path)],
        producer: Some("raw_media".into()),
        stale: false,
        blocking_reason: (!exists).then(|| "source_missing".into()),
        updated_at: modified_at(asset_path),
    }
}

fn proxy_layer(
    project_root: &Path,
    asset_id: &str,
    asset_path: &Path,
    source_exists: bool,
) -> MediaIntelligenceLayer {
    let proxy_path = proxy_path_for(project_root, asset_path);
    if !source_exists {
        return blocked_layer(
            MediaIntelligenceLayerKind::Proxy,
            "proxy",
            project_root,
            proxy_path,
            "source_missing",
        );
    }
    let entry = proxy_status_for(project_root, asset_id, asset_path);
    let status = match entry.status {
        ProxyStatus::Fresh => MediaIntelligenceLayerStatus::Ready,
        ProxyStatus::Pending => MediaIntelligenceLayerStatus::Processing,
        ProxyStatus::Missing | ProxyStatus::Stale => MediaIntelligenceLayerStatus::Pending,
    };
    MediaIntelligenceLayer {
        kind: MediaIntelligenceLayerKind::Proxy,
        status,
        artifact_refs: vec![artifact_ref(project_root, &proxy_path)],
        producer: Some("proxy_cache".into()),
        stale: entry.status == ProxyStatus::Stale,
        blocking_reason: None,
        updated_at: modified_at(&proxy_path),
    }
}

fn waveform_layer(
    project_root: &Path,
    asset_path: &Path,
    source_exists: bool,
) -> MediaIntelligenceLayer {
    let waveform_path = waveform_path_for(project_root, asset_path);
    if !source_exists {
        return blocked_layer(
            MediaIntelligenceLayerKind::Waveform,
            "preview_cache",
            project_root,
            waveform_path,
            "source_missing",
        );
    }
    let exists = waveform_path.is_file();
    let stale = exists && is_stale(asset_path, &waveform_path);
    MediaIntelligenceLayer {
        kind: MediaIntelligenceLayerKind::Waveform,
        status: if exists && !stale {
            MediaIntelligenceLayerStatus::Ready
        } else {
            MediaIntelligenceLayerStatus::Pending
        },
        artifact_refs: vec![artifact_ref(project_root, &waveform_path)],
        producer: Some("preview_cache".into()),
        stale,
        blocking_reason: None,
        updated_at: modified_at(&waveform_path),
    }
}

fn sidecar_layer(
    project_root: &Path,
    asset_id: &str,
    asset_path: &Path,
    source_exists: bool,
    kind: MediaIntelligenceLayerKind,
    indexer: &'static str,
    is_ready: fn(&Value) -> bool,
) -> MediaIntelligenceLayer {
    let path = expected_sidecar_path(project_root, indexer, asset_id);
    if !source_exists {
        return blocked_layer(kind, indexer, project_root, path, "source_missing");
    }
    let (status, stale, blocking_reason) = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) if is_ready(&value) => {
                let stale = is_stale(asset_path, &path);
                (
                    if stale {
                        MediaIntelligenceLayerStatus::Pending
                    } else {
                        MediaIntelligenceLayerStatus::Ready
                    },
                    stale,
                    None,
                )
            }
            Ok(_) => (MediaIntelligenceLayerStatus::Pending, false, None),
            Err(e) => (
                MediaIntelligenceLayerStatus::Blocked,
                false,
                Some(format!("invalid_sidecar_json: {e}")),
            ),
        },
        Err(_) => (MediaIntelligenceLayerStatus::Pending, false, None),
    };
    MediaIntelligenceLayer {
        kind,
        status,
        artifact_refs: vec![artifact_ref(project_root, &path)],
        producer: Some(indexer.into()),
        stale,
        blocking_reason,
        updated_at: modified_at(&path),
    }
}

fn broll_layer(
    project_root: &Path,
    asset_id: &str,
    asset_path: &Path,
    source_exists: bool,
) -> MediaIntelligenceLayer {
    let shot_path = expected_sidecar_path(project_root, "shot", asset_id);
    let quality_path = expected_sidecar_path(project_root, "frame-quality", asset_id);
    if !source_exists {
        return MediaIntelligenceLayer {
            kind: MediaIntelligenceLayerKind::Broll,
            status: MediaIntelligenceLayerStatus::Blocked,
            artifact_refs: vec![
                artifact_ref(project_root, &shot_path),
                artifact_ref(project_root, &quality_path),
            ],
            producer: Some("broll_candidates".into()),
            stale: false,
            blocking_reason: Some("source_missing".into()),
            updated_at: None,
        };
    }

    let shot_ready = sidecar_ready(asset_path, &shot_path, |value| {
        has_any(value, &["frame_count", "summary"]) || any_array(value, &["shots"])
    });
    let quality_ready = sidecar_ready(asset_path, &quality_path, |value| {
        has_any(value, &["frame_count", "summary"]) || any_array(value, &["frames"])
    });
    let stale = is_stale(asset_path, &shot_path) || is_stale(asset_path, &quality_path);
    MediaIntelligenceLayer {
        kind: MediaIntelligenceLayerKind::Broll,
        status: if shot_ready && quality_ready && !stale {
            MediaIntelligenceLayerStatus::Ready
        } else {
            MediaIntelligenceLayerStatus::Pending
        },
        artifact_refs: vec![
            artifact_ref(project_root, &shot_path),
            artifact_ref(project_root, &quality_path),
        ],
        producer: Some("broll_candidates".into()),
        stale,
        blocking_reason: None,
        updated_at: modified_at(&shot_path).or_else(|| modified_at(&quality_path)),
    }
}

fn blocked_layer(
    kind: MediaIntelligenceLayerKind,
    producer: &str,
    project_root: &Path,
    path: PathBuf,
    reason: &str,
) -> MediaIntelligenceLayer {
    MediaIntelligenceLayer {
        kind,
        status: MediaIntelligenceLayerStatus::Blocked,
        artifact_refs: vec![artifact_ref(project_root, &path)],
        producer: Some(producer.into()),
        stale: false,
        blocking_reason: Some(reason.into()),
        updated_at: None,
    }
}

fn expected_sidecar_path(project_root: &Path, indexer: &str, asset_id: &str) -> PathBuf {
    sidecar_path(project_root, indexer, &AssetId::new(asset_id.to_string())).unwrap_or_else(|_| {
        project_root
            .join("index")
            .join(indexer)
            .join(format!("{asset_id}.json"))
    })
}

fn transcript_ready(value: &Value) -> bool {
    any_array(value, &["words", "segments"])
}

fn speakers_ready(value: &Value) -> bool {
    any_array(value, &["speakers"]) || array_has_key(value, "words", "speaker_id")
}

fn sidecar_ready(asset_path: &Path, path: &Path, ready: fn(&Value) -> bool) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    ready(&value) && !is_stale(asset_path, path)
}

fn any_array(value: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|key| {
        data(value)
            .get(*key)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    })
}

fn array_has_key(value: &Value, array_key: &str, item_key: &str) -> bool {
    data(value)
        .get(array_key)
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get(item_key)
                    .or_else(|| item.get("speaker"))
                    .or_else(|| item.get("speaker_label"))
                    .is_some()
            })
        })
}

fn has_any(value: &Value, keys: &[&str]) -> bool {
    keys.iter()
        .any(|key| !data(value).get(*key).unwrap_or(&Value::Null).is_null())
}

fn data(value: &Value) -> &Value {
    value.get("data").unwrap_or(value)
}

fn aggregate_state(layers: &[MediaIntelligenceLayer]) -> MediaIntelligenceAggregateState {
    if layers.iter().any(|layer| {
        layer.kind == MediaIntelligenceLayerKind::Source
            && layer.status == MediaIntelligenceLayerStatus::Blocked
    }) {
        return MediaIntelligenceAggregateState::Offline;
    }
    if layers
        .iter()
        .any(|layer| layer.status == MediaIntelligenceLayerStatus::Processing)
    {
        return MediaIntelligenceAggregateState::Processing;
    }
    if layers
        .iter()
        .any(|layer| layer.status == MediaIntelligenceLayerStatus::Blocked)
    {
        return MediaIntelligenceAggregateState::Blocked;
    }
    if layers
        .iter()
        .all(|layer| layer.status == MediaIntelligenceLayerStatus::Ready)
    {
        MediaIntelligenceAggregateState::Ready
    } else {
        MediaIntelligenceAggregateState::Partial
    }
}

fn recommended_actions(layers: &[MediaIntelligenceLayer]) -> Vec<String> {
    let mut actions = Vec::new();
    for layer in layers {
        let action = match (layer.kind, layer.status) {
            (MediaIntelligenceLayerKind::Source, MediaIntelligenceLayerStatus::Blocked) => {
                Some("restore_or_relink_source_media")
            }
            (MediaIntelligenceLayerKind::Proxy, MediaIntelligenceLayerStatus::Pending) => {
                Some("generate_proxy")
            }
            (MediaIntelligenceLayerKind::Waveform, MediaIntelligenceLayerStatus::Pending) => {
                Some("generate_waveform")
            }
            (MediaIntelligenceLayerKind::Transcript, MediaIntelligenceLayerStatus::Pending) => {
                Some("run_whisper_indexer")
            }
            (MediaIntelligenceLayerKind::Speakers, MediaIntelligenceLayerStatus::Pending) => {
                Some("run_speaker_diarization")
            }
            (MediaIntelligenceLayerKind::Scenes, MediaIntelligenceLayerStatus::Pending) => {
                Some("run_scenedetect_indexer")
            }
            (MediaIntelligenceLayerKind::Topics, MediaIntelligenceLayerStatus::Pending) => {
                Some("run_topic_indexer")
            }
            (MediaIntelligenceLayerKind::Moments, MediaIntelligenceLayerStatus::Pending) => {
                Some("run_editorial_moments_indexer")
            }
            (MediaIntelligenceLayerKind::ClipCandidates, MediaIntelligenceLayerStatus::Pending) => {
                Some("run_clip_indexer")
            }
            (MediaIntelligenceLayerKind::Broll, MediaIntelligenceLayerStatus::Pending) => {
                Some("run_shot_and_frame_quality_indexers")
            }
            _ => None,
        };
        if let Some(action) = action
            && !actions.iter().any(|existing| existing == action)
        {
            actions.push(action.to_string());
        }
    }
    actions
}

fn artifact_ref(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_stale(source_path: &Path, artifact_path: &Path) -> bool {
    let Ok(source_modified) = source_path.metadata().and_then(|meta| meta.modified()) else {
        return false;
    };
    let Ok(artifact_modified) = artifact_path.metadata().and_then(|meta| meta.modified()) else {
        return false;
    };
    artifact_modified < source_modified
}

fn modified_at(path: &Path) -> Option<String> {
    path.metadata()
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(system_time_to_rfc3339)
}

fn system_time_to_rfc3339(time: SystemTime) -> Option<String> {
    let millis = time.duration_since(UNIX_EPOCH).ok()?.as_millis();
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis as i64)
        .map(|time| time.to_rfc3339())
}
