//! Deterministic tool and skill capability manifest.
//!
//! This is intentionally metadata-only: it exposes the callable surface,
//! approval defaults, schemas, and L1 skill metadata without skill bodies or
//! local filesystem paths.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use crate::capability_metadata::{CapabilityMetadata, SupportLevel};
use crate::skills::SkillRegistry;
use crate::tool::{ToolInvocation, ToolRegistry};

/// Complete capability manifest for agent/UI discoverability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityManifest {
    /// Manifest schema version.
    pub version: u32,
    /// Registered tool capabilities, sorted by name.
    pub tools: Vec<ToolCapability>,
    /// Discovered skill capabilities, sorted by name.
    pub skills: Vec<SkillCapability>,
    /// Graph-native effect capabilities, sorted by id.
    pub effects: Vec<EffectCapability>,
    /// Render pipeline feature capabilities, in dependency order.
    pub render_features: Vec<RenderFeatureCapability>,
}

/// One callable tool capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCapability {
    /// Tool name.
    pub name: String,
    /// Model-facing tool description.
    pub description: String,
    /// JSON schema for tool input.
    pub input_schema: serde_json::Value,
    /// Whether a representative empty invocation is mutating.
    pub mutating_default: bool,
    /// Whether approval is required for the representative invocation.
    pub approval_required_default: bool,
    /// Typed support metadata for this tool.
    pub metadata: CapabilityMetadata,
}

/// One loadable skill capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillCapability {
    /// Skill name.
    pub name: String,
    /// L1 skill description.
    pub description: String,
    /// Skill version.
    pub version: String,
    /// Optional tool allowlist declared by the skill.
    pub tools_allowlist: Vec<String>,
    /// Optional grouping tier.
    pub tier: Option<String>,
}

/// One graph-native effect capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectCapability {
    /// Stable effect id.
    pub id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Scope where the effect can be attached.
    pub scope: String,
    /// Media streams the effect influences.
    pub media_kind: String,
    /// Render/evaluation phase.
    pub phase: String,
    /// Current effect-registry support status.
    pub support: String,
    /// Current backend family.
    pub backend: String,
    /// Typed support metadata.
    pub metadata: CapabilityMetadata,
}

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

/// Build a deterministic capability manifest.
pub fn build_capability_manifest(
    registry: &ToolRegistry,
    skills: Option<&SkillRegistry>,
) -> CapabilityManifest {
    let mut tool_names: Vec<&str> = registry.names().copied().collect();
    tool_names.sort_unstable();

    let tools = tool_names
        .into_iter()
        .filter_map(|name| registry.get(name))
        .map(|handler| {
            let schema = handler.schema();
            let invocation = ToolInvocation {
                call_id: "capability_manifest".into(),
                name: schema.name.clone(),
                args: serde_json::json!({}),
            };
            let mutating_default = handler.is_mutating(&invocation);
            let metadata = handler.capability_metadata(&invocation);
            ToolCapability {
                name: schema.name,
                description: schema.description,
                input_schema: schema.input_schema,
                mutating_default,
                approval_required_default: mutating_default,
                metadata,
            }
        })
        .collect();

    let skill_iter = skills.into_iter().flat_map(SkillRegistry::all);
    let mut skill_capabilities: Vec<SkillCapability> = skill_iter
        .map(|skill| SkillCapability {
            name: skill.meta.name.clone(),
            description: skill.meta.description.clone(),
            version: skill.meta.version.clone(),
            tools_allowlist: skill.meta.tools_allowlist.clone(),
            tier: skill.meta.tier.clone(),
        })
        .collect();
    skill_capabilities.sort_by(|left, right| left.name.cmp(&right.name));

    CapabilityManifest {
        version: 1,
        tools,
        skills: skill_capabilities,
        effects: effect_capabilities(),
        render_features: render_feature_capabilities(),
    }
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

fn effect_capabilities() -> Vec<EffectCapability> {
    let mut effects: Vec<EffectCapability> = montage_effects::EFFECTS
        .iter()
        .map(|effect| EffectCapability {
            id: effect.id.into(),
            display_name: effect.display_name.into(),
            scope: effect_scope(effect.scope).into(),
            media_kind: media_kind(effect.media_kind).into(),
            phase: effect_phase(effect.phase).into(),
            support: effect_support(effect.support).into(),
            backend: effect_backend(effect.backend).into(),
            metadata: CapabilityMetadata::for_effect(effect.support, effect.backend),
        })
        .collect();
    effects.sort_by(|left, right| left.id.cmp(&right.id));
    effects
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

fn effect_scope(scope: montage_effects::EffectScope) -> &'static str {
    match scope {
        montage_effects::EffectScope::Clip => "clip",
        montage_effects::EffectScope::Track => "track",
        montage_effects::EffectScope::Timeline => "timeline",
    }
}

fn media_kind(kind: montage_effects::MediaKind) -> &'static str {
    match kind {
        montage_effects::MediaKind::Video => "video",
        montage_effects::MediaKind::Audio => "audio",
        montage_effects::MediaKind::Both => "both",
    }
}

fn effect_phase(phase: montage_effects::EffectPhase) -> &'static str {
    match phase {
        montage_effects::EffectPhase::Source => "source",
        montage_effects::EffectPhase::Clip => "clip",
        montage_effects::EffectPhase::Transform => "transform",
        montage_effects::EffectPhase::Transition => "transition",
        montage_effects::EffectPhase::TimelineOverlay => "timeline_overlay",
        montage_effects::EffectPhase::Output => "output",
    }
}

fn effect_support(support: montage_effects::SupportStatus) -> &'static str {
    match support {
        montage_effects::SupportStatus::Stable => "stable",
        montage_effects::SupportStatus::Experimental => "experimental",
        montage_effects::SupportStatus::Unavailable => "unavailable",
    }
}

fn effect_backend(backend: montage_effects::BackendKind) -> &'static str {
    match backend {
        montage_effects::BackendKind::FfmpegNative => "ffmpeg_native",
        montage_effects::BackendKind::SemanticOnly => "semantic_only",
    }
}
