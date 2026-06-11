//! `render_preflight` — inspect render backend selection without
//! starting a job. Ported from `crates/core/src/tools/render_preflight.rs`
//! to the in-process MCP server.

use std::collections::BTreeMap;

use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `render_preflight`.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct RenderPreflightArgs {
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default)]
    pub include_preview_cache: bool,
    #[serde(default)]
    pub preview_cache_max_tasks: Option<usize>,
}

impl Default for RenderPreflightArgs {
    fn default() -> Self {
        Self {
            scope: default_scope(),
            include_preview_cache: false,
            preview_cache_max_tasks: None,
        }
    }
}

fn default_scope() -> String {
    "timeline".into()
}

/// Run `render_preflight` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; argument
/// validation or backend analysis errors return `Err(String)`.
pub fn run(args: RenderPreflightArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.scope != "timeline" {
        return Err(format!(
            "render_preflight: scope must be 'timeline', got {:?}",
            args.scope
        ));
    }
    let preflight = montage_render::analyze_timeline_render_preflight(&ctx.project_root)
        .map_err(|error| format!("render_preflight: {error}"))?;
    let project = Project::read(&ctx.project_root)
        .map_err(|error| format!("render_preflight: project read failed: {error}"))?;
    let caption_summary = crate::captions::summarize_captions(&project);
    let motion_scenes =
        crate::tools::render_preflight::motion_scene_preflight_json(&project.timeline);
    let motion_scene_count = motion_scenes["count"].as_u64().unwrap_or(0);
    let backend = render_backend_json_value(&preflight.backend);
    let mut backend_evidence = preflight.metadata.clone();
    enrich_render_metadata_with_backend_capability(&mut backend_evidence, &preflight.backend);
    let limitations = preflight
        .limitations
        .iter()
        .map(|limitation| {
            serde_json::json!({
                "kind": limitation.kind,
                "animation_id": limitation.animation_id,
                "clip_id": limitation.clip_id,
                "parameter": limitation.parameter,
                "message": limitation.message,
            })
        })
        .collect::<Vec<_>>();
    let body = serde_json::json!({
        "scope": args.scope,
        "backend": backend,
        "backend_evidence": backend_evidence,
        "caption_summary": caption_summary,
        "caption_layout_readiness": caption_layout_readiness_json(&caption_summary),
        "preview_cache": preview_cache_preflight_json(&ctx.project_root, &args)?,
        "artifact_policy": "no_render_job_started",
        "total_duration_s": preflight.total_duration_s,
        "counts": {
            "segments": preflight.segment_count,
            "transitions": preflight.transition_count,
            "video_overlays": preflight.video_overlay_count,
            "titles": preflight.title_count,
            "editable_subtitle_tracks": preflight.editable_subtitle_track_count,
            "annotations": preflight.annotation_count,
            "audio_tracks": preflight.audio_track_count,
            "motion_scenes": motion_scene_count,
            "limitations": limitations.len(),
        },
        "motion_scenes": motion_scenes,
        "limitations": limitations,
    });
    Ok(body.to_string())
}

fn caption_layout_readiness_json(summary: &crate::captions::CaptionSummary) -> serde_json::Value {
    let safe_area_ready = summary.caption_overlay_count == summary.safe_area_caption_overlay_count;
    let word_timing_ready =
        summary.caption_overlay_count == summary.word_timed_caption_overlay_count;
    let status = if summary.caption_overlay_count == 0 {
        "no_caption_overlays"
    } else if !safe_area_ready {
        "needs_safe_area_metadata"
    } else if !word_timing_ready {
        "needs_word_timing_metadata"
    } else {
        "ready_for_libass_layout"
    };
    serde_json::json!({
        "status": status,
        "safe_area_ready": safe_area_ready,
        "word_timing_ready": word_timing_ready,
        "warning_count": summary.warnings.len(),
        "warnings": summary.warnings,
    })
}

fn preview_cache_preflight_json(
    project_root: &std::path::Path,
    args: &RenderPreflightArgs,
) -> Result<serde_json::Value, String> {
    if !args.include_preview_cache {
        return Ok(serde_json::Value::Null);
    }
    let summary = crate::preview_cache::build_preview_cache_summary(project_root)
        .map_err(|e| format!("render_preflight: unable to scan preview cache: {e}"))?;
    let selection_options = crate::preview_cache::PreviewCacheSelectionOptions {
        max_tasks: args.preview_cache_max_tasks,
        ..Default::default()
    };
    let selection =
        crate::preview_cache::select_preview_cache_refresh_tasks(&summary, &selection_options);
    let refresh_execution =
        crate::preview_cache::preview_cache_refresh_execution_contract(&selection);
    Ok(serde_json::json!({
        "asset_count": summary.asset_count,
        "ready_asset_count": summary.ready_asset_count,
        "refresh_task_count": summary.refresh_tasks.len(),
        "refresh_work": summary.refresh_work,
        "selected_refresh_tasks": selection.selected_refresh_tasks,
        "selected_refresh_work": selection.selected_refresh_work,
        "selected_task_count": selection.selected_task_count,
        "skipped_task_count": selection.skipped_task_count,
        "refresh_execution": refresh_execution,
    }))
}

fn render_backend_json_value(backend: &montage_render::RenderBackendKind) -> String {
    serde_json::to_value(backend)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{backend:?}"))
}

fn enrich_render_metadata_with_backend_capability(
    metadata: &mut BTreeMap<String, String>,
    backend: &montage_render::RenderBackendKind,
) {
    metadata.extend(crate::capabilities::render_feature_metadata_for_backend(
        backend,
    ));
}

pub const DESCRIPTION: &str = "\
Inspect render backend selection, capability metadata, and limitations without \
starting a render job.";
