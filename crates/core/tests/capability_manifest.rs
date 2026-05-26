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

    fn schema(&self) -> awidat_core::tool_schema::Tool {
        awidat_core::tool_schema::Tool {
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

    fn schema(&self) -> awidat_core::tool_schema::Tool {
        awidat_core::tool_schema::Tool {
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

struct FindMomentLikeTool;

#[async_trait]
impl ToolHandler for FindMomentLikeTool {
    fn name(&self) -> &'static str {
        "find_moment"
    }

    fn schema(&self) -> awidat_core::tool_schema::Tool {
        awidat_core::tool_schema::Tool {
            name: self.name().into(),
            description: "Search indexed editorial moments.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
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

struct StartRenderLikeTool;

#[async_trait]
impl ToolHandler for StartRenderLikeTool {
    fn name(&self) -> &'static str {
        "start_render"
    }

    fn schema(&self) -> awidat_core::tool_schema::Tool {
        awidat_core::tool_schema::Tool {
            name: self.name().into(),
            description: "Start a render job.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "output": {"type": "string"}
                },
                "required": ["output"]
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

struct RenderPreflightLikeTool;

#[async_trait]
impl ToolHandler for RenderPreflightLikeTool {
    fn name(&self) -> &'static str {
        "render_preflight"
    }

    fn schema(&self) -> awidat_core::tool_schema::Tool {
        awidat_core::tool_schema::Tool {
            name: self.name().into(),
            description: "Inspect render backend selection.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "scope": {"type": "string"}
                }
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

struct StreamRemuxLikeTool;

#[async_trait]
impl ToolHandler for StreamRemuxLikeTool {
    fn name(&self) -> &'static str {
        "stream_remux"
    }

    fn schema(&self) -> awidat_core::tool_schema::Tool {
        awidat_core::tool_schema::Tool {
            name: self.name().into(),
            description: "Start a stream-copy remux job.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "sources": {"type": "array"}
                },
                "required": ["sources"]
            }),
            cache_control: None,
        }
    }

    async fn handle(
        &self,
        _invocation: ToolInvocation,
        _ctx: awidat_core::ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        Ok(ToolOutput::text("ok"))
    }
}

struct ProxyStatusLikeTool;

#[async_trait]
impl ToolHandler for ProxyStatusLikeTool {
    fn name(&self) -> &'static str {
        "proxy_status"
    }

    fn schema(&self) -> awidat_core::tool_schema::Tool {
        awidat_core::tool_schema::Tool {
            name: self.name().into(),
            description: "Report proxy cache status.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {"type": "string"}
                }
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
        _ctx: awidat_core::ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        Ok(ToolOutput::text("ok"))
    }
}

struct PreviewCacheStatusLikeTool;

#[async_trait]
impl ToolHandler for PreviewCacheStatusLikeTool {
    fn name(&self) -> &'static str {
        "preview_cache_status"
    }

    fn schema(&self) -> awidat_core::tool_schema::Tool {
        awidat_core::tool_schema::Tool {
            name: self.name().into(),
            description: "Report preview cache status.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {"type": "string"},
                    "max_tasks": {"type": "integer"}
                }
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
        _ctx: awidat_core::ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        Ok(ToolOutput::text("ok"))
    }
}

struct VerifyRenderLikeTool;

#[async_trait]
impl ToolHandler for VerifyRenderLikeTool {
    fn name(&self) -> &'static str {
        "verify_render"
    }

    fn schema(&self) -> awidat_core::tool_schema::Tool {
        awidat_core::tool_schema::Tool {
            name: self.name().into(),
            description: "Verify a rendered output.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "output_path": {"type": "string"}
                }
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
        _ctx: awidat_core::ToolContext,
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
    assert!(!inspect.metadata.graph_mutates);
    assert_eq!(
        inspect.metadata.preview_supported,
        awidat_core::capabilities::SupportLevel::Unknown
    );
    assert_eq!(
        inspect.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::Unknown
    );
    assert!(inspect.metadata.required_indexes.is_empty());
    assert!(!inspect.metadata.approval_required);
    assert!(inspect.metadata.side_effects.is_empty());
    assert!(inspect.metadata.known_limitations.is_empty());
    assert_eq!(
        inspect.input_schema["required"],
        serde_json::json!(["asset_id"])
    );

    let write = &manifest.tools[1];
    assert!(write.mutating_default);
    assert!(write.approval_required_default);
    assert!(write.metadata.graph_mutates);
    assert!(write.metadata.approval_required);
    assert_eq!(
        write.metadata.side_effects,
        vec!["may mutate the project graph or filesystem"]
    );
    assert_eq!(write.description, "Write an edit decision to the project.");
}

#[test]
fn capability_manifest_adds_explicit_known_tool_metadata() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FindMomentLikeTool));
    registry.register(Arc::new(PreviewCacheStatusLikeTool));
    registry.register(Arc::new(ProxyStatusLikeTool));
    registry.register(Arc::new(RenderPreflightLikeTool));
    registry.register(Arc::new(StartRenderLikeTool));
    registry.register(Arc::new(StreamRemuxLikeTool));
    registry.register(Arc::new(VerifyRenderLikeTool));

    let manifest = build_capability_manifest(&registry, None);

    let Some(find_moment) = manifest
        .tools
        .iter()
        .find(|tool| tool.name == "find_moment")
    else {
        panic!("find_moment capability");
    };
    assert_eq!(
        find_moment.metadata.required_indexes,
        vec!["editorial_moments"]
    );
    assert!(!find_moment.metadata.graph_mutates);
    assert_eq!(
        find_moment.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::NotSupported
    );

    let Some(start_render) = manifest
        .tools
        .iter()
        .find(|tool| tool.name == "start_render")
    else {
        panic!("start_render capability");
    };
    assert!(start_render.metadata.approval_required);
    assert_eq!(
        start_render.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert_eq!(
        start_render.metadata.side_effects,
        vec!["starts an ffmpeg render job", "writes render output files"]
    );

    let Some(render_preflight) = manifest
        .tools
        .iter()
        .find(|tool| tool.name == "render_preflight")
    else {
        panic!("render_preflight capability");
    };
    assert!(!render_preflight.metadata.graph_mutates);
    assert!(!render_preflight.metadata.approval_required);
    assert_eq!(
        render_preflight.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert!(
        render_preflight
            .metadata
            .side_effects
            .iter()
            .any(|effect| effect.contains("no render job"))
    );
    assert!(
        render_preflight
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("preview-cache planning"))
    );

    let Some(verify_render) = manifest
        .tools
        .iter()
        .find(|tool| tool.name == "verify_render")
    else {
        panic!("verify_render capability");
    };
    assert!(verify_render.metadata.approval_required);
    assert!(
        verify_render
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("caption rendered-output evidence"))
    );

    let Some(stream_remux) = manifest
        .tools
        .iter()
        .find(|tool| tool.name == "stream_remux")
    else {
        panic!("stream_remux capability");
    };
    assert!(stream_remux.metadata.approval_required);
    assert!(!stream_remux.metadata.graph_mutates);
    assert_eq!(
        stream_remux.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert!(
        stream_remux
            .metadata
            .side_effects
            .iter()
            .any(|effect| effect.contains("render manifest"))
    );
    assert!(
        stream_remux
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("frame-domain effects"))
    );

    let Some(proxy_status) = manifest
        .tools
        .iter()
        .find(|tool| tool.name == "proxy_status")
    else {
        panic!("proxy_status capability");
    };
    assert_eq!(
        proxy_status.metadata.preview_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert_eq!(
        proxy_status.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::NotSupported
    );
    assert!(!proxy_status.metadata.graph_mutates);
    assert!(!proxy_status.metadata.approval_required);
    assert_eq!(
        proxy_status.metadata.side_effects,
        vec!["reads proxy cache artifact metadata"]
    );

    let Some(preview_cache_status) = manifest
        .tools
        .iter()
        .find(|tool| tool.name == "preview_cache_status")
    else {
        panic!("preview_cache_status capability");
    };
    assert_eq!(
        preview_cache_status.metadata.preview_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert_eq!(
        preview_cache_status.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::NotSupported
    );
    assert!(!preview_cache_status.metadata.graph_mutates);
    assert!(!preview_cache_status.metadata.approval_required);
    assert!(
        preview_cache_status
            .metadata
            .side_effects
            .iter()
            .any(|effect| effect.contains("reads proxy, thumbnail, and waveform"))
    );
    assert!(
        preview_cache_status
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("PreviewRefreshExecutor"))
    );
}

#[test]
fn capability_manifest_lists_effect_and_render_feature_metadata() {
    let manifest = build_capability_manifest(&ToolRegistry::new(), None);

    let effect_ids: Vec<&str> = manifest
        .effects
        .iter()
        .map(|effect| effect.id.as_str())
        .collect();
    assert!(effect_ids.contains(&"awidat.speed"));
    assert!(effect_ids.contains(&"awidat.color_pipeline"));

    let Some(speed) = manifest
        .effects
        .iter()
        .find(|effect| effect.id == "awidat.speed")
    else {
        panic!("speed effect");
    };
    assert_eq!(speed.display_name, "Speed");
    assert_eq!(
        speed.metadata.preview_supported,
        awidat_core::capabilities::SupportLevel::Unknown
    );
    assert_eq!(
        speed.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );

    let Some(color_pipeline) = manifest
        .effects
        .iter()
        .find(|effect| effect.id == "awidat.color_pipeline")
    else {
        panic!("color pipeline effect");
    };
    assert_eq!(
        color_pipeline.metadata.known_limitations,
        vec![
            "experimental effect support; some parameter combinations may emit render limitations"
        ]
    );

    let render_features: Vec<&str> = manifest
        .render_features
        .iter()
        .map(|feature| feature.id.as_str())
        .collect();
    assert_eq!(
        render_features,
        vec![
            "render_execution_manifest",
            "asset_preview_render",
            "stream_copy_remux",
            "asset_full_reencode",
            "ffmpeg_timeline_export",
            "ass_caption_burn_in",
            "section_render_export",
            "gpu_transition_raw_stream",
            "delivery_package_export",
            "render_manifest_verification",
            "render_backend_evidence_verification",
            "master_loudnorm_final_pass_verification",
            "libass_sidecar_evidence_verification",
            "caption_safe_area_verification",
            "cut_boundary_self_eval",
            "desktop_proxy_preview",
            "desktop_preview_cache_summary",
            "agent_preview_cache_status",
            "desktop_preview_cache_refresh"
        ]
    );

    let Some(timeline_feature) = awidat_core::capabilities::render_feature_for_backend(
        &awidat_render::RenderBackendKind::TimelineFfmpegReencode,
    ) else {
        panic!("timeline backend should map to a render feature");
    };
    assert_eq!(timeline_feature.id, "ffmpeg_timeline_export");
    assert_eq!(
        timeline_feature.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );

    let Some(gpu) = manifest
        .render_features
        .iter()
        .find(|feature| feature.id == "gpu_transition_raw_stream")
    else {
        panic!("gpu transition feature");
    };
    assert_eq!(
        gpu.metadata.known_limitations,
        vec!["mixed xfade/GPU transition renders are not supported"]
    );

    let Some(remux) = manifest
        .render_features
        .iter()
        .find(|feature| feature.id == "stream_copy_remux")
    else {
        panic!("stream copy remux feature");
    };
    assert_eq!(
        remux.metadata.preview_supported,
        awidat_core::capabilities::SupportLevel::NotSupported
    );
    assert_eq!(
        remux.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert!(
        remux
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("falls back"))
    );

    let Some(preview_cache) = manifest
        .render_features
        .iter()
        .find(|feature| feature.id == "desktop_preview_cache_summary")
    else {
        panic!("desktop preview cache summary feature");
    };
    assert_eq!(
        preview_cache.metadata.preview_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert_eq!(
        preview_cache.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::NotSupported
    );
    assert!(
        preview_cache
            .metadata
            .side_effects
            .iter()
            .any(|effect| effect.contains("reads preview cache"))
    );
    assert!(
        preview_cache
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("aggregate refresh_work counts"))
    );
    assert!(
        preview_cache
            .metadata
            .known_limitations
            .iter()
            .any(
                |limitation| limitation.contains("per-artifact refresh_tasks")
                    && limitation.contains("task_id")
                    && limitation.contains("estimated_weight")
            )
    );

    let Some(agent_preview_cache) = manifest
        .render_features
        .iter()
        .find(|feature| feature.id == "agent_preview_cache_status")
    else {
        panic!("agent preview cache status feature");
    };
    assert_eq!(
        agent_preview_cache.metadata.preview_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert!(
        agent_preview_cache
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("PreviewRefreshExecutor"))
    );

    let Some(preview_cache_refresh) = manifest
        .render_features
        .iter()
        .find(|feature| feature.id == "desktop_preview_cache_refresh")
    else {
        panic!("desktop preview cache refresh feature");
    };
    assert_eq!(
        preview_cache_refresh.metadata.preview_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert_eq!(
        preview_cache_refresh.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::NotSupported
    );
    assert!(preview_cache_refresh.metadata.approval_required);
    assert!(
        preview_cache_refresh
            .metadata
            .side_effects
            .iter()
            .any(|effect| effect.contains("writes preview cache artifacts"))
    );
    assert!(
        preview_cache_refresh
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("runs proxy, thumbnail, and waveform"))
    );

    let Some(backend_evidence) = manifest
        .render_features
        .iter()
        .find(|feature| feature.id == "render_backend_evidence_verification")
    else {
        panic!("render backend evidence verification feature");
    };
    assert_eq!(
        backend_evidence.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert!(
        backend_evidence
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("timeline render manifests"))
    );

    let Some(master_loudnorm) = manifest
        .render_features
        .iter()
        .find(|feature| feature.id == "master_loudnorm_final_pass_verification")
    else {
        panic!("master loudnorm final-pass verification feature");
    };
    assert_eq!(
        master_loudnorm.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert!(
        master_loudnorm
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("apply pass"))
    );

    let Some(libass_sidecar_evidence) = manifest
        .render_features
        .iter()
        .find(|feature| feature.id == "libass_sidecar_evidence_verification")
    else {
        panic!("libass sidecar evidence verification feature");
    };
    assert_eq!(
        libass_sidecar_evidence.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert!(
        libass_sidecar_evidence
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("required ASS sidecar fingerprints"))
    );
    assert!(
        libass_sidecar_evidence
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("layout/readability evidence"))
    );

    let Some(caption_safe_area) = manifest
        .render_features
        .iter()
        .find(|feature| feature.id == "caption_safe_area_verification")
    else {
        panic!("caption safe-area verification feature");
    };
    assert_eq!(
        caption_safe_area.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert!(
        caption_safe_area
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("frame-pixel scorer"))
    );

    let Some(cut_boundary_self_eval) = manifest
        .render_features
        .iter()
        .find(|feature| feature.id == "cut_boundary_self_eval")
    else {
        panic!("cut-boundary self-eval feature");
    };
    assert_eq!(
        cut_boundary_self_eval.metadata.export_supported,
        awidat_core::capabilities::SupportLevel::Supported
    );
    assert!(
        cut_boundary_self_eval
            .metadata
            .side_effects
            .iter()
            .any(|effect| effect.contains("render verification reports"))
    );

    let Some(ass_captions) = manifest
        .render_features
        .iter()
        .find(|feature| feature.id == "ass_caption_burn_in")
    else {
        panic!("ass caption feature");
    };
    assert!(
        ass_captions
            .metadata
            .side_effects
            .iter()
            .any(|effect| effect.contains("ASS subtitle sidecars"))
    );
    assert!(
        ass_captions
            .metadata
            .side_effects
            .iter()
            .any(|effect| effect.contains("editable subtitle tracks"))
    );
    assert!(
        ass_captions
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("caption overlays"))
    );
    assert!(
        ass_captions
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("editable subtitle tracks"))
    );
    assert!(
        ass_captions
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("mobile/default safe-area layout profiles"))
    );

    let Some(manifest_verification) = manifest
        .render_features
        .iter()
        .find(|feature| feature.id == "render_manifest_verification")
    else {
        panic!("render manifest verification feature");
    };
    assert!(
        manifest_verification
            .metadata
            .known_limitations
            .iter()
            .any(|limitation| limitation.contains("required inputs and sidecars"))
    );
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
