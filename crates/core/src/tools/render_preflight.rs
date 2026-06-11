//! `render_preflight` tool — inspect render backend selection without starting a job.

use std::collections::BTreeMap;

use async_trait::async_trait;
use montage_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Read-only render preflight tool.
pub struct RenderPreflightTool;

#[derive(Debug, Deserialize)]
struct RenderPreflightArgs {
    #[serde(default = "default_scope")]
    scope: String,
    #[serde(default)]
    include_preview_cache: bool,
    #[serde(default)]
    preview_cache_max_tasks: Option<usize>,
}

fn default_scope() -> String {
    "timeline".into()
}

#[async_trait]
impl ToolHandler for RenderPreflightTool {
    fn name(&self) -> &'static str {
        "render_preflight"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "render_preflight".into(),
            description: "Inspect render backend selection, capability metadata, and limitations without starting a render job.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "enum": ["timeline"],
                        "description": "Preflight target. Currently supports the edited timeline."
                    },
                    "include_preview_cache": {
                        "type": "boolean",
                        "default": false,
                        "description": "Include read-only preview-cache readiness and bounded refresh planning."
                    },
                    "preview_cache_max_tasks": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Optional maximum number of preview-cache refresh tasks to include when include_preview_cache is true."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: RenderPreflightArgs =
            serde_json::from_value(invocation.args).map_err(|error| {
                FunctionCallError::RespondToModel(format!(
                    "render_preflight: invalid args ({error}). All fields are optional."
                ))
            })?;
        if args.scope != "timeline" {
            return Err(FunctionCallError::RespondToModel(format!(
                "render_preflight: scope must be 'timeline', got {:?}",
                args.scope
            )));
        }
        let preflight = montage_render::analyze_timeline_render_preflight(&ctx.project_root)
            .map_err(|error| {
                FunctionCallError::RespondToModel(format!("render_preflight: {error}"))
            })?;
        let project = Project::read(&ctx.project_root).map_err(|error| {
            FunctionCallError::RespondToModel(format!(
                "render_preflight: project read failed: {error}"
            ))
        })?;
        let caption_summary = crate::captions::summarize_captions(&project);
        let motion_scenes = motion_scene_preflight_json(&project.timeline);
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
        Ok(ToolOutput::text(body.to_string()))
    }
}

/// Summarize stored MotionScenes for preflight reporting.
///
/// Stored scenes lower natively into the render plan — text layers
/// into `counts.titles`, rect/solid layers into `counts.annotations` —
/// and never into `counts.video_overlays`. Agents repeatedly read
/// `video_overlays: 0` as "my scenes were dropped", so report the
/// stored scenes explicitly with ids, anchors, and layer counts.
pub fn motion_scene_preflight_json(timeline: &montage_proto::otio::Timeline) -> serde_json::Value {
    let scenes = timeline
        .metadata
        .montage
        .as_ref()
        .map(|metadata| metadata.motion_scenes.as_slice())
        .unwrap_or_default();
    serde_json::json!({
        "count": scenes.len(),
        "scenes": scenes
            .iter()
            .map(|scene| {
                serde_json::json!({
                    "id": scene.id,
                    "start_s": scene.start_s,
                    "duration_s": scene.duration_s,
                    "layer_count": scene.layers.len(),
                })
            })
            .collect::<Vec<_>>(),
        "note": "Stored MotionScenes render natively: text layers are counted in counts.titles and rect/solid layers in counts.annotations. They never appear in counts.video_overlays, so video_overlays: 0 does NOT mean scenes were dropped.",
    })
}

#[cfg(test)]
mod tests {
    use super::motion_scene_preflight_json;

    #[test]
    fn motion_scene_preflight_json_reports_stored_scenes() {
        let mut timeline = montage_proto::otio::Timeline::empty("preflight-scenes");
        timeline.metadata.montage = Some(montage_proto::montage_meta::MontageTimelineMetadata {
            motion_scenes: vec![montage_proto::professional::MotionScene {
                id: "ai-vs-drone-mistake-comparison-manual".into(),
                start_s: 531.14,
                duration_s: 6.0,
                fps: 30.0,
                width: 1920,
                height: 1080,
                layers: vec![montage_proto::professional::MotionSceneLayer {
                    id: "heading".into(),
                    kind: montage_proto::professional::MotionSceneLayerKind::Text,
                    from_s: 0.0,
                    duration_s: 6.0,
                    z_index: 10,
                    params: Default::default(),
                }],
                rationale: None,
            }],
            ..Default::default()
        });

        let summary = motion_scene_preflight_json(&timeline);

        assert_eq!(summary["count"], 1);
        assert_eq!(
            summary["scenes"][0]["id"],
            "ai-vs-drone-mistake-comparison-manual"
        );
        assert_eq!(summary["scenes"][0]["start_s"], 531.14);
        assert_eq!(summary["scenes"][0]["layer_count"], 1);
        let note = summary["note"].as_str().unwrap_or_default();
        assert!(note.contains("video_overlays"));
    }

    #[test]
    fn motion_scene_preflight_json_handles_empty_metadata() {
        let timeline = montage_proto::otio::Timeline::empty("no-scenes");
        let summary = motion_scene_preflight_json(&timeline);
        assert_eq!(summary["count"], 0);
        assert!(summary["scenes"].as_array().is_some_and(Vec::is_empty));
    }
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
) -> Result<serde_json::Value, FunctionCallError> {
    if !args.include_preview_cache {
        return Ok(serde_json::Value::Null);
    }
    let summary = crate::preview_cache::build_preview_cache_summary(project_root).map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "render_preflight: unable to scan preview cache: {e}"
        ))
    })?;
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
