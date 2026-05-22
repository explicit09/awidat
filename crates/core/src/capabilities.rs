//! Deterministic tool and skill capability manifest.
//!
//! This is intentionally metadata-only: it exposes the callable surface,
//! approval defaults, schemas, and L1 skill metadata without skill bodies or
//! local filesystem paths.

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

fn effect_capabilities() -> Vec<EffectCapability> {
    let mut effects: Vec<EffectCapability> = awidat_effects::EFFECTS
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
            id: "ffmpeg_timeline_export".into(),
            display_name: "FFmpeg timeline export".into(),
            metadata: CapabilityMetadata {
                graph_mutates: false,
                preview_supported: SupportLevel::NotSupported,
                export_supported: SupportLevel::Supported,
                required_indexes: Vec::new(),
                approval_required: true,
                side_effects: vec!["writes render output files".into()],
                known_limitations: Vec::new(),
            },
        },
        RenderFeatureCapability {
            id: "section_render_export".into(),
            display_name: "Section render export".into(),
            metadata: CapabilityMetadata {
                graph_mutates: false,
                preview_supported: SupportLevel::NotSupported,
                export_supported: SupportLevel::Supported,
                required_indexes: Vec::new(),
                approval_required: true,
                side_effects: vec!["writes render output files".into()],
                known_limitations: Vec::new(),
            },
        },
        RenderFeatureCapability {
            id: "gpu_transition_raw_stream".into(),
            display_name: "GPU transition raw-stream export".into(),
            metadata: CapabilityMetadata {
                graph_mutates: false,
                preview_supported: SupportLevel::NotSupported,
                export_supported: SupportLevel::Supported,
                required_indexes: Vec::new(),
                approval_required: true,
                side_effects: vec!["writes intermediate and final render files".into()],
                known_limitations: vec![
                    "mixed xfade/GPU transition renders are not supported".into(),
                ],
            },
        },
        RenderFeatureCapability {
            id: "desktop_proxy_preview".into(),
            display_name: "Desktop proxy preview".into(),
            metadata: CapabilityMetadata {
                graph_mutates: false,
                preview_supported: SupportLevel::Supported,
                export_supported: SupportLevel::NotSupported,
                required_indexes: Vec::new(),
                approval_required: false,
                side_effects: vec!["writes proxy media files".into()],
                known_limitations: Vec::new(),
            },
        },
    ]
}

fn effect_scope(scope: awidat_effects::EffectScope) -> &'static str {
    match scope {
        awidat_effects::EffectScope::Clip => "clip",
        awidat_effects::EffectScope::Track => "track",
        awidat_effects::EffectScope::Timeline => "timeline",
    }
}

fn media_kind(kind: awidat_effects::MediaKind) -> &'static str {
    match kind {
        awidat_effects::MediaKind::Video => "video",
        awidat_effects::MediaKind::Audio => "audio",
        awidat_effects::MediaKind::Both => "both",
    }
}

fn effect_phase(phase: awidat_effects::EffectPhase) -> &'static str {
    match phase {
        awidat_effects::EffectPhase::Source => "source",
        awidat_effects::EffectPhase::Clip => "clip",
        awidat_effects::EffectPhase::Transform => "transform",
        awidat_effects::EffectPhase::Transition => "transition",
        awidat_effects::EffectPhase::TimelineOverlay => "timeline_overlay",
        awidat_effects::EffectPhase::Output => "output",
    }
}

fn effect_support(support: awidat_effects::SupportStatus) -> &'static str {
    match support {
        awidat_effects::SupportStatus::Stable => "stable",
        awidat_effects::SupportStatus::Experimental => "experimental",
        awidat_effects::SupportStatus::Unavailable => "unavailable",
    }
}

fn effect_backend(backend: awidat_effects::BackendKind) -> &'static str {
    match backend {
        awidat_effects::BackendKind::FfmpegNative => "ffmpeg_native",
        awidat_effects::BackendKind::SemanticOnly => "semantic_only",
    }
}
