//! Capability manifest contract tests.

use std::sync::Arc;

use async_trait::async_trait;
use awidat_core::{
    FunctionCallError, ToolContext, ToolHandler, ToolInvocation, ToolOutput, ToolRegistry,
    capabilities::{CapabilityManifest, build_capability_manifest},
};

struct ReadOnlyTool;

#[async_trait]
impl ToolHandler for ReadOnlyTool {
    fn name(&self) -> &'static str {
        "inspect_media"
    }

    fn schema(&self) -> awidat_core::anthropic::Tool {
        awidat_core::anthropic::Tool {
            name: self.name().into(),
            description: "Inspect media without changing project files.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {"type": "string"}
                },
                "required": ["asset_id"]
            }),
            cache_control: None,
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        _invocation: ToolInvocation,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        Ok(ToolOutput::text("ok"))
    }
}

struct MutatingTool;

#[async_trait]
impl ToolHandler for MutatingTool {
    fn name(&self) -> &'static str {
        "write_edit"
    }

    fn schema(&self) -> awidat_core::anthropic::Tool {
        awidat_core::anthropic::Tool {
            name: self.name().into(),
            description: "Write an edit decision to the project.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "operation": {"type": "string"}
                },
                "required": ["operation"]
            }),
            cache_control: None,
        }
    }

    async fn handle(
        &self,
        _invocation: ToolInvocation,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        Ok(ToolOutput::text("ok"))
    }
}

#[test]
fn capability_manifest_lists_tools_with_stable_order_and_approval_defaults() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(MutatingTool));
    registry.register(Arc::new(ReadOnlyTool));

    let manifest = build_capability_manifest(&registry, None);

    let names: Vec<&str> = manifest
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert_eq!(names, vec!["inspect_media", "write_edit"]);

    let inspect = &manifest.tools[0];
    assert!(!inspect.mutating_default);
    assert!(!inspect.approval_required_default);
    assert_eq!(
        inspect.input_schema["required"],
        serde_json::json!(["asset_id"])
    );

    let write = &manifest.tools[1];
    assert!(write.mutating_default);
    assert!(write.approval_required_default);
    assert_eq!(write.description, "Write an edit decision to the project.");
}

#[test]
fn capability_manifest_lists_skills_without_bodies_or_local_paths() {
    let dir = match tempfile::tempdir() {
        Ok(dir) => dir,
        Err(error) => panic!("create tempdir: {error}"),
    };
    let skills_root = dir.path().join("skills");
    if let Err(error) = std::fs::create_dir(&skills_root) {
        panic!("create skills root: {error}");
    }
    let skill_dir = skills_root.join("caption-pass");
    if let Err(error) = std::fs::create_dir(&skill_dir) {
        panic!("create skill dir: {error}");
    }
    if let Err(error) = std::fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: caption-pass
description: Improve captions.
version: 1.2.3
tools_allowlist:
  - inspect_media
tier: editorial
---

Private body text and local paths should not be part of the manifest.
"#,
    ) {
        panic!("write skill file: {error}");
    }

    let (skills, errors) = awidat_core::skills::SkillRegistry::discover(Some(&skills_root), None);
    assert!(errors.is_empty());

    let registry = ToolRegistry::new();
    let manifest = build_capability_manifest(&registry, Some(&skills));
    assert_eq!(manifest.skills.len(), 1);

    let skill = &manifest.skills[0];
    assert_eq!(skill.name, "caption-pass");
    assert_eq!(skill.description, "Improve captions.");
    assert_eq!(skill.version, "1.2.3");
    assert_eq!(skill.tools_allowlist, vec!["inspect_media"]);
    assert_eq!(skill.tier.as_deref(), Some("editorial"));

    let encoded = match serde_json::to_string(&manifest) {
        Ok(encoded) => encoded,
        Err(error) => panic!("serialize manifest: {error}"),
    };
    assert!(!encoded.contains("Private body text"));
    assert!(!encoded.contains(skill_dir.to_string_lossy().as_ref()));

    let decoded: CapabilityManifest = match serde_json::from_str(&encoded) {
        Ok(decoded) => decoded,
        Err(error) => panic!("deserialize manifest: {error}"),
    };
    assert_eq!(decoded.skills[0].name, "caption-pass");
}
