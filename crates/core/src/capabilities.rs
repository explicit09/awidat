//! Render pipeline capability metadata.
//!
//! This is intentionally metadata-only: it exposes render feature support
//! levels and known limitations without skill bodies or local filesystem
//! paths. The tool/skill/effect capability manifest (`CapabilityManifest`,
//! `build_capability_manifest`) was deleted along with `capability_manifest.rs`
//! — the render-feature half below is the surviving, still-consumed half
//! (see `render_feature_metadata_for_backend`, used by `render_cmd.rs` and
//! the montage_mcp render tools).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use crate::capability_metadata::{CapabilityMetadata, SupportLevel};

/// One render pipeline capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderFeatureCapability {
    /// Stable render feature id.
    pub id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Typed support metadata.
    pub metadata: CapabilityMetadata,
}

/// Return the render feature capability that corresponds to a selected
/// execution backend.
pub fn render_feature_for_backend(
    backend: &montage_render::RenderBackendKind,
) -> Option<RenderFeatureCapability> {
    let feature_id = match backend {
        montage_render::RenderBackendKind::AssetPreview => "asset_preview_render",
        montage_render::RenderBackendKind::AssetSegmentStreamCopy => "stream_copy_remux",
        montage_render::RenderBackendKind::AssetFullReencode => "asset_full_reencode",
        montage_render::RenderBackendKind::TimelineFfmpegReencode => "ffmpeg_timeline_export",
        montage_render::RenderBackendKind::TimelineRawStreamGpu => "gpu_transition_raw_stream",
        montage_render::RenderBackendKind::PackageExport => "delivery_package_export",
        montage_render::RenderBackendKind::StreamExportRemux => "stream_copy_remux",
    };
    render_feature_capabilities()
        .into_iter()
        .find(|feature| feature.id == feature_id)
}

/// Stable metadata fields for the render feature selected by a backend.
pub fn render_feature_metadata_for_backend(
    backend: &montage_render::RenderBackendKind,
) -> BTreeMap<String, String> {
    let Some(feature) = render_feature_for_backend(backend) else {
        return BTreeMap::new();
    };
    BTreeMap::from([
        ("render_feature_id".into(), feature.id),
        (
            "render_feature_preview_supported".into(),
            support_level_name(feature.metadata.preview_supported).into(),
        ),
        (
            "render_feature_export_supported".into(),
            support_level_name(feature.metadata.export_supported).into(),
        ),
        (
            "render_feature_approval_required".into(),
            feature.metadata.approval_required.to_string(),
        ),
        (
            "render_feature_limitation_count".into(),
            feature.metadata.known_limitations.len().to_string(),
        ),
    ])
}

fn support_level_name(level: SupportLevel) -> &'static str {
    match level {
        SupportLevel::Supported => "supported",
        SupportLevel::NotSupported => "not_supported",
        SupportLevel::Unknown => "unknown",
    }
}

fn render_feature_capabilities() -> Vec<RenderFeatureCapability> {
    vec![
        RenderFeatureCapability {
            id: "render_execution_manifest".into(),
            display_name: "Render execution manifest".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                false,
                vec!["writes deterministic render manifest sidecars".into()],
                Vec::new(),
            ),
        },
        RenderFeatureCapability {
            id: "asset_preview_render".into(),
            display_name: "Asset preview render".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::Supported,
                SupportLevel::NotSupported,
                true,
                vec!["writes diagnostic preview render files".into()],
                Vec::new(),
            ),
        },
        RenderFeatureCapability {
            id: "stream_copy_remux".into(),
            display_name: "Stream-copy remux export".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                true,
                vec!["writes remuxed media outputs without frame-domain effects".into()],
                vec![
                    "falls back to re-encode when timeline edits require frame-domain rendering"
                        .into(),
                ],
            ),
        },
        RenderFeatureCapability {
            id: "asset_full_reencode".into(),
            display_name: "Asset full re-encode".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                true,
                vec!["writes full-quality asset render files".into()],
                Vec::new(),
            ),
        },
        RenderFeatureCapability {
            id: "ffmpeg_timeline_export".into(),
            display_name: "FFmpeg timeline export".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                true,
                vec!["writes render output files".into()],
                Vec::new(),
            ),
        },
        RenderFeatureCapability {
            id: "ass_caption_burn_in".into(),
            display_name: "ASS caption burn-in".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::Unknown,
                SupportLevel::Supported,
                true,
                vec![
                    "writes temporary ASS subtitle sidecars for word-timed captions and editable subtitle tracks"
                        .into(),
                ],
                vec![
                    "eligible caption overlays require caption role metadata and non-empty word timings"
                        .into(),
                    "editable subtitle tracks are burned in through ASS sidecars when the timeline renderer selects FFmpeg re-encode"
                        .into(),
                    "currently supports mobile/default safe-area layout profiles; custom caption layout profiles are not yet exposed"
                        .into(),
                ],
            ),
        },
        RenderFeatureCapability {
            id: "section_render_export".into(),
            display_name: "Section render export".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                true,
                vec!["writes render output files".into()],
                Vec::new(),
            ),
        },
        RenderFeatureCapability {
            id: "gpu_transition_raw_stream".into(),
            display_name: "GPU transition raw-stream export".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                true,
                vec!["writes intermediate and final render files".into()],
                vec!["mixed xfade/GPU transition renders are not supported".into()],
            ),
        },
        RenderFeatureCapability {
            id: "delivery_package_export".into(),
            display_name: "Delivery package export".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                true,
                vec!["writes delivery package files and render manifests".into()],
                Vec::new(),
            ),
        },
        RenderFeatureCapability {
            id: "render_manifest_verification".into(),
            display_name: "Render manifest verification".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                true,
                vec![
                    "writes render verification reports".into(),
                    "updates render manifest verification summaries".into(),
                ],
                vec![
                    "validates required inputs and sidecars only for persisted render manifests"
                        .into(),
                ],
            ),
        },
        RenderFeatureCapability {
            id: "render_backend_evidence_verification".into(),
            display_name: "Render backend evidence verification".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                true,
                vec!["writes render verification reports".into()],
                vec![
                    "currently validates backend-selection evidence on timeline render manifests"
                        .into(),
                ],
            ),
        },
        RenderFeatureCapability {
            id: "master_loudnorm_final_pass_verification".into(),
            display_name: "Master loudnorm final-pass verification".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                true,
                vec!["writes render verification reports".into()],
                vec![
                    "requires two-pass master loudnorm manifests to identify the final encoded apply pass"
                        .into(),
                ],
            ),
        },
        RenderFeatureCapability {
            id: "libass_sidecar_evidence_verification".into(),
            display_name: "Libass sidecar evidence verification".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                true,
                vec!["writes render verification reports".into()],
                vec![
                    "requires libass caption manifests to include required ASS sidecar fingerprints"
                        .into(),
                    "requires libass caption manifests to include layout/readability evidence from ASS sidecars"
                        .into(),
                ],
            ),
        },
        RenderFeatureCapability {
            id: "caption_safe_area_verification".into(),
            display_name: "Caption safe-area verification".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                true,
                vec!["writes render verification reports".into()],
                vec![
                    "safe-area and occlusion are now measured per caption event from rendered output via the frame-pixel scorer when ffmpeg and a render output are available; libass-layout sidecar derivation remains a named fallback path"
                        .into(),
                ],
            ),
        },
        RenderFeatureCapability {
            id: "cut_boundary_self_eval".into(),
            display_name: "Cut-boundary self-eval".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::NotSupported,
                SupportLevel::Supported,
                true,
                vec!["writes render verification reports".into()],
                vec![
                    "currently checks clip cut boundaries against duration, long silence, and black-frame detector ranges"
                        .into(),
                ],
            ),
        },
        RenderFeatureCapability {
            id: "desktop_proxy_preview".into(),
            display_name: "Desktop proxy preview".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::Supported,
                SupportLevel::NotSupported,
                false,
                vec!["writes proxy media files".into()],
                Vec::new(),
            ),
        },
        RenderFeatureCapability {
            id: "desktop_preview_cache_summary".into(),
            display_name: "Desktop preview cache summary".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::Supported,
                SupportLevel::NotSupported,
                false,
                vec!["reads preview cache artifact metadata".into()],
                vec![
                    "reports aggregate refresh_work counts for proxy, thumbnail, and waveform scheduling"
                        .into(),
                    "reports per-artifact refresh_tasks with task_id, estimated_weight, artifact paths, and missing/stale reasons"
                        .into(),
                ],
            ),
        },
        RenderFeatureCapability {
            id: "agent_preview_cache_status".into(),
            display_name: "Agent preview cache status".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::Supported,
                SupportLevel::NotSupported,
                false,
                vec!["reads proxy, thumbnail, and waveform cache metadata".into()],
                vec![
                    "reports per-artifact refresh_tasks with task_id, estimated_weight, artifact paths, and missing/stale reasons"
                        .into(),
                    "refresh execution now runs against the `PreviewRefreshExecutor` trait via `run_preview_cache_refresh`, persists per-task lifecycle state to `.montage/preview-cache/refresh-plan.json`, and resumes from prior pending tasks; cross-process file locking is not yet implemented"
                        .into(),
                    "does not generate artifacts; it is the agent-facing readiness/preflight view"
                        .into(),
                ],
            ),
        },
        RenderFeatureCapability {
            id: "desktop_preview_cache_refresh".into(),
            display_name: "Desktop preview cache refresh".into(),
            metadata: CapabilityMetadata::render_feature(
                SupportLevel::Supported,
                SupportLevel::NotSupported,
                true,
                vec!["writes preview cache artifacts".into()],
                vec![
                    "runs proxy, thumbnail, and waveform generation sequentially from missing/stale refresh tasks"
                        .into(),
                    "does not yet expose a shared worker pool or persisted refresh queue".into(),
                ],
            ),
        },
    ]
}
