//! Deterministic tool and skill capability manifest.
//!
//! This is intentionally metadata-only: it exposes the callable surface,
//! approval defaults, schemas, and L1 skill metadata without skill bodies or
//! local filesystem paths.

use serde::{Deserialize, Serialize};

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
            ToolCapability {
                name: schema.name,
                description: schema.description,
                input_schema: schema.input_schema,
                mutating_default,
                approval_required_default: mutating_default,
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
    }
}
