#![allow(clippy::expect_used, clippy::unwrap_used)]
//! Progressive media intelligence state tests.

use std::path::Path;

use awidat_core::media_intelligence::{
    build_media_intelligence_for_asset, build_media_intelligence_package,
};
use awidat_core::preview_cache::waveform_path_for;
use awidat_index::sidecar_path;
use awidat_proto::index::AssetId;
use awidat_proto::professional::{
    MediaIntelligenceAggregateState, MediaIntelligenceLayerKind, MediaIntelligenceLayerStatus,
};

#[test]
fn raw_asset_starts_as_partial_with_source_ready() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("raw/a.mov"), b"media");

    let package = build_media_intelligence_package(dir.path()).expect("package");

    assert_eq!(package.assets.len(), 1);
    let asset = &package.assets[0];
    assert_eq!(asset.asset_id, "raw/a.mov");
    assert_eq!(
        asset.aggregate_state,
        MediaIntelligenceAggregateState::Partial
    );
    assert_eq!(
        layer_status(asset, MediaIntelligenceLayerKind::Source),
        MediaIntelligenceLayerStatus::Ready
    );
    assert_eq!(
        layer_status(asset, MediaIntelligenceLayerKind::Transcript),
        MediaIntelligenceLayerStatus::Pending
    );
}

#[test]
fn sidecars_advance_independent_layers() {
    let dir = tempfile::tempdir().unwrap();
    let asset_path = dir.path().join("raw/a.mov");
    write(&asset_path, b"media");
    write(
        &waveform_path_for(dir.path(), &asset_path),
        br#"{"buckets":[0,1]}"#,
    );
    write_sidecar(
        dir.path(),
        "whisper",
        "raw/a.mov",
        serde_json::json!({
            "data": {
                "words": [
                    {"text": "hello", "start_s": 0.0, "end_s": 0.2, "speaker_id": "S1"}
                ],
                "speakers": [{"id": "S1"}]
            }
        }),
    );
    write_sidecar(
        dir.path(),
        "scenedetect",
        "raw/a.mov",
        serde_json::json!({ "data": { "shots": [{"start_s": 0.0, "end_s": 1.0}] } }),
    );
    write_sidecar(
        dir.path(),
        "topic",
        "raw/a.mov",
        serde_json::json!({ "data": { "topics": [{"label": "AI"}] } }),
    );
    write_sidecar(
        dir.path(),
        "editorial-moments",
        "raw/a.mov",
        serde_json::json!({ "data": { "moments": [{"start_s": 0.0, "end_s": 1.0}] } }),
    );

    let asset = build_media_intelligence_for_asset(dir.path(), "raw/a.mov", &asset_path);

    assert_eq!(
        layer_status(&asset, MediaIntelligenceLayerKind::Waveform),
        MediaIntelligenceLayerStatus::Ready
    );
    assert_eq!(
        layer_status(&asset, MediaIntelligenceLayerKind::Transcript),
        MediaIntelligenceLayerStatus::Ready
    );
    assert_eq!(
        layer_status(&asset, MediaIntelligenceLayerKind::Speakers),
        MediaIntelligenceLayerStatus::Ready
    );
    assert_eq!(
        layer_status(&asset, MediaIntelligenceLayerKind::Scenes),
        MediaIntelligenceLayerStatus::Ready
    );
    assert_eq!(
        layer_status(&asset, MediaIntelligenceLayerKind::Topics),
        MediaIntelligenceLayerStatus::Ready
    );
    assert_eq!(
        layer_status(&asset, MediaIntelligenceLayerKind::Moments),
        MediaIntelligenceLayerStatus::Ready
    );
}

#[test]
fn missing_source_blocks_dependent_layers_and_preserves_refs() {
    let dir = tempfile::tempdir().unwrap();
    let asset_path = dir.path().join("raw/missing.mov");

    let asset = build_media_intelligence_for_asset(dir.path(), "raw/missing.mov", &asset_path);

    assert_eq!(
        asset.aggregate_state,
        MediaIntelligenceAggregateState::Offline
    );
    for layer in &asset.layers {
        assert_eq!(layer.status, MediaIntelligenceLayerStatus::Blocked);
        assert_eq!(layer.blocking_reason.as_deref(), Some("source_missing"));
        assert!(
            !layer.artifact_refs.is_empty(),
            "layer {:?} should preserve expected artifact refs",
            layer.kind
        );
    }
}

fn layer_status(
    asset: &awidat_proto::professional::MediaIntelligenceAsset,
    kind: MediaIntelligenceLayerKind,
) -> MediaIntelligenceLayerStatus {
    asset
        .layers
        .iter()
        .find(|layer| layer.kind == kind)
        .map(|layer| layer.status)
        .unwrap()
}

fn write(path: &Path, body: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn write_sidecar(project_root: &Path, indexer: &str, asset_id: &str, value: serde_json::Value) {
    let path = sidecar_path(project_root, indexer, &AssetId::new(asset_id.to_string())).unwrap();
    let body = serde_json::to_vec(&value).unwrap();
    write(&path, &body);
}
