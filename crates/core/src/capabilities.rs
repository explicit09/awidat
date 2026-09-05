//! Render backend metadata recorded in preflight results and render manifests.

use std::collections::BTreeMap;

use montage_render::RenderBackendKind;

/// Stable metadata fields for the render feature selected by a backend.
pub fn render_feature_metadata_for_backend(
    backend: &RenderBackendKind,
) -> BTreeMap<String, String> {
    let (id, preview, export, limitation_count) = match backend {
        RenderBackendKind::AssetPreview => {
            ("asset_preview_render", "supported", "not_supported", "0")
        }
        RenderBackendKind::AssetSegmentStreamCopy | RenderBackendKind::StreamExportRemux => {
            ("stream_copy_remux", "not_supported", "supported", "1")
        }
        RenderBackendKind::AssetFullReencode => {
            ("asset_full_reencode", "not_supported", "supported", "0")
        }
        RenderBackendKind::TimelineFfmpegReencode => {
            ("ffmpeg_timeline_export", "not_supported", "supported", "0")
        }
        RenderBackendKind::TimelineRawStreamGpu => (
            "gpu_transition_raw_stream",
            "not_supported",
            "supported",
            "1",
        ),
        RenderBackendKind::PackageExport => {
            ("delivery_package_export", "not_supported", "supported", "0")
        }
    };
    BTreeMap::from([
        ("render_feature_id".into(), id.into()),
        ("render_feature_preview_supported".into(), preview.into()),
        ("render_feature_export_supported".into(), export.into()),
        ("render_feature_approval_required".into(), "true".into()),
        (
            "render_feature_limitation_count".into(),
            limitation_count.into(),
        ),
    ])
}
