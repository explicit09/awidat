//! `start_render` tool — kick off a background ffmpeg render and
//! return a job id.
//!
//! Per the corpus survey of Codex's `unified_exec` and Crush's
//! `BackgroundShellManager`: **always async**, never sync. Renders take
//! seconds-to-minutes; we never want to block the agent loop on one.
//!
//! Scope vocabulary (v1):
//! - `preview` — re-encode an asset to a low-bitrate H.264 / 480p mp4
//!   under `<project>/renders/`. Default codec, fast.
//! - `segment` — extract a time range from an asset (no re-encode if
//!   format permits stream-copy; otherwise H.264). Future: serve as the
//!   primitive `apply_edl` outputs route through.
//! - `full` — full-quality re-encode of an asset. Higher bitrate.
//! - `timeline` — render the *edited* timeline. Walks
//!   `project.otio.json`, builds one ffmpeg input per video-track clip
//!   (with `-ss`/`-t` aligned to the clip's source_range), and concats
//!   them via the `concat` filter with re-encode at boundaries. The
//!   re-encode kills the DTS-seam click that stream-copy concat
//!   produces at non-keyframe-aligned cut points. `asset` is ignored
//!   in this scope.
//!
//! Output naming: `<project>/renders/<scope>-<asset-stem>-<job_id>.mp4`.
//! Predictable so the user can find it; job_id-suffixed to avoid clobber.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use awidat_proto::professional::ExportPreset;
use awidat_proto::project::Project;
use awidat_render::{
    OutputPathPolicy, RenderJobSpec, RenderPlanLimitation, validate_render_output_path,
};
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `start_render` tool.
pub struct StartRenderTool;

#[derive(Debug, Deserialize)]
struct StartRenderArgs {
    /// `preview` | `segment` | `full` | `timeline`. See module doc.
    scope: String,
    /// Source asset (project-relative). Required for preview/segment/full;
    /// ignored for `timeline` (the project's OTIO is the source).
    #[serde(default)]
    asset: Option<String>,
    /// Optional [start_s, end_s) range. Required for `segment`.
    #[serde(default)]
    range: Option<TimeRange>,
    /// Optional timeline guide marker section. Only used with `scope=timeline`.
    #[serde(default)]
    guide: Option<GuideSection>,
    /// Optional export-codec preset slug. Recognized values: `"hevc"`,
    /// `"prores"`. When set, the preset's codec/container fully overrides
    /// the scope's hardcoded `libx264 + aac` defaults. `None` preserves
    /// the legacy libx264 path for backward compatibility.
    #[serde(default)]
    preset: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TimeRange {
    start_s: f64,
    end_s: f64,
}

#[derive(Debug, Deserialize)]
struct GuideSection {
    track_id: String,
    marker_id: String,
}

#[async_trait]
impl ToolHandler for StartRenderTool {
    fn name(&self) -> &'static str {
        "start_render"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "start_render".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "enum": ["preview", "segment", "full", "timeline"],
                        "description": "preview = low-bitrate 480p of an asset; segment = trim a range from an asset; full = full-bitrate of an asset; timeline = render the edited OTIO timeline (asset is ignored)."
                    },
                    "asset": {
                        "type": "string",
                        "description": "Project-relative source asset path. Required for preview/segment/full; ignored for timeline."
                    },
                    "range": {
                        "type": "object",
                        "properties": {
                            "start_s": { "type": "number", "minimum": 0.0 },
                            "end_s":   { "type": "number", "minimum": 0.0 }
                        },
                        "required": ["start_s", "end_s"],
                        "description": "[start_s, end_s) time range. Required for scope=segment; ignored otherwise."
                    },
                    "guide": {
                        "type": "object",
                        "properties": {
                            "track_id": { "type": "string" },
                            "marker_id": { "type": "string" }
                        },
                        "required": ["track_id", "marker_id"],
                        "description": "Optional guide-track marker range for scope=timeline section exports. Ignored for preview/segment/full."
                    },
                    "preset": {
                        "type": "string",
                        "enum": ["hevc", "prores"],
                        "description": "Optional export-codec preset. 'hevc' = libx265 in MP4 (broadcast/archive); 'prores' = Apple ProRes 422 HQ in MOV (mastering). Without a preset, the legacy libx264+aac defaults are used. DNxHR / AV1 are pending — TODO(overnight-wave1-export-codecs)."
                    }
                },
                "required": ["scope"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        let scope = invocation
            .args
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing-scope>");
        let asset = invocation
            .args
            .get("asset")
            .and_then(|v| v.as_str())
            .unwrap_or("<timeline>");
        let range = invocation
            .args
            .get("range")
            .map(serde_json::Value::to_string)
            .unwrap_or_else(|| "<full>".to_string());
        let guide = invocation
            .args
            .get("guide")
            .map(serde_json::Value::to_string)
            .unwrap_or_else(|| "<full-timeline>".to_string());
        let preset = invocation
            .args
            .get("preset")
            .and_then(|v| v.as_str())
            .unwrap_or("<default>");
        vec![ApprovalKey::new(
            "start_render",
            format!("{scope}:{asset}:{range}:{guide}:{preset}:writes=renders"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: StartRenderArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_render: invalid args ({e}). Required: {{ \"scope\": \"preview|segment|full|timeline\", \"asset\": <str> (omit for timeline) }}."
            ))
        })?;

        let renders_dir = ctx.project_root.join("renders");
        tokio::fs::create_dir_all(&renders_dir).await.ok();
        let timestamp = chrono::Utc::now().format("%H%M%S");

        // Fork on scope. Timeline scope delegates to awidat-render's
        // shared planner so the agent and the desktop's Export button
        // produce identical specs. The asset-based scopes keep their
        // original path-validation flow.
        let (
            argv,
            total_duration_s,
            asset_label,
            output_path,
            limitations,
            asset_path_for_manifest,
            input_paths_for_manifest,
            backend,
            mut render_metadata_for_manifest,
        ) = if args.scope == "timeline" {
            let project = Project::read(&ctx.project_root).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "start_render: failed to read project metadata: {e}"
                ))
            })?;
            if crate::tools::podcast_qc_report::is_podcast_project(&project) {
                let qc_report = crate::tools::podcast_qc_report::build_podcast_qc_report(
                    &ctx.project_root,
                    &project,
                );
                if qc_report["status"] == "blocked" {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "start_render: podcast timeline render blocked by QC. Run \
                         podcast_qc_report and fix the error issue(s) before rendering. \
                         Current QC: {qc_report}"
                    )));
                }
            }
            crate::lessons::apply_learned_project_format_defaults(&ctx.project_root)
                .map_err(|e| FunctionCallError::RespondToModel(format!("start_render: {e}")))?;
            let mut spec = match args.guide.as_ref() {
                Some(guide) => awidat_render::build_timeline_section_render_spec(
                    &ctx.project_root,
                    &guide.track_id,
                    &guide.marker_id,
                ),
                None => awidat_render::build_timeline_render_spec(&ctx.project_root),
            }
            .map_err(|e| {
                    use awidat_render::RenderTimelineError;
                    let msg = match e {
                        RenderTimelineError::EmptyTimeline => {
                            "start_render: timeline has no clips to render. \
                         Add clips via apply_edl first, or use scope=preview to render \
                         the raw asset."
                                .to_string()
                        }
                        RenderTimelineError::NoOtio(p) => format!(
                            "start_render: no project.otio.json found at {} — \
                         this isn't an awidat project root.",
                            p.display()
                        ),
                        RenderTimelineError::OtioParse { message } => format!(
                            "start_render: timeline parse failed ({message}). \
                         Run `awidat validate <project>` for the detailed diagnostic, \
                         then revert the most recent apply_edl that broke the OTIO."
                        ),
                        RenderTimelineError::MissingAsset { clip_name, missing } => format!(
                            "start_render: timeline references missing asset {} \
                         (clip '{clip_name}'). Re-import the source file.",
                            missing.display()
                        ),
                        RenderTimelineError::MissingLut { clip_name, missing } => format!(
                            "start_render: clip '{clip_name}' references missing LUT {}. \
                         Add the LUT under the project root or update the Apply LUT path.",
                            missing.display()
                        ),
                        RenderTimelineError::ClipMissingRange { clip_name } => format!(
                            "start_render: clip '{clip_name}' has no source_range — \
                         can't extract a renderable segment."
                        ),
                        RenderTimelineError::TransitionHandleUnavailable {
                            kind,
                            clip_name,
                            side,
                            needed_s,
                            available_s,
                        } => format!(
                            "start_render: transition {kind:?} around clip '{clip_name}' needs \
                         {needed_s:.3}s {side} handle, but only {available_s:.3}s is available. \
                         Shorten the transition, choose a different alignment, or apply Untrim Clip \
                         to widen the source range before rendering."
                        ),
                        RenderTimelineError::UnsupportedTransition { kind, message } => format!(
                            "start_render: timeline transition {kind:?} cannot be exported: \
                         {message}"
                        ),
                        RenderTimelineError::InvalidTransitionPlacement { message } => {
                            format!("start_render: timeline has an invalid transition: {message}")
                        }
                        RenderTimelineError::InvalidTransitionMetadata { kind, message } => {
                            format!(
                                "start_render: transition {kind:?} has invalid Awidat metadata: \
                         {message}. Use data-only transition primitives with bounded params; do \
                         not put raw FFmpeg, GLSL, shell, or plugin code in transition metadata."
                            )
                        }
                        RenderTimelineError::InvalidClipEffectMetadata {
                            clip_name,
                            effect,
                            message,
                        } => {
                            format!(
                                "start_render: clip '{clip_name}' has invalid {effect} metadata: \
                         {message}. Use the registered awidat effect parameter schema and avoid \
                         raw renderer expressions in clip effect metadata."
                            )
                        }
                        RenderTimelineError::BroadcastOverlayRender(message) => {
                            format!("start_render: broadcast overlay render failed: {message}")
                        }
                        RenderTimelineError::GuideSectionNotFound {
                            guide_track_id,
                            marker_id,
                        } => format!(
                            "start_render: guide section {guide_track_id}/{marker_id} was not found"
                        ),
                        RenderTimelineError::GuideSectionMissingDuration {
                            guide_track_id,
                            marker_id,
                        } => format!(
                            "start_render: guide section {guide_track_id}/{marker_id} needs a positive duration"
                        ),
                    };
                    FunctionCallError::RespondToModel(msg)
                })?;
            if let Some(slug) = args.preset.as_deref() {
                let preset = resolve_export_preset(slug)?;
                spec = awidat_render::professional::apply_export_preset_to_spec(spec, &preset)
                    .map_err(|e| {
                        FunctionCallError::RespondToModel(format!(
                            "start_render: failed to apply export preset '{}': {e}",
                            preset.id
                        ))
                    })?;
            }
            let caption_summary = crate::captions::summarize_captions(&project);
            enrich_render_metadata_with_caption_summary(&mut spec.metadata, &caption_summary);
            (
                spec.args,
                spec.total_duration_s,
                "<timeline>".to_string(),
                spec.output_path,
                spec.limitations,
                None,
                spec.input_paths,
                spec.backend,
                spec.metadata,
            )
        } else {
            // Asset-based scope: preview / segment / full.
            let asset = args.asset.as_deref().ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "start_render: scope='{}' requires an `asset` path",
                    args.scope
                ))
            })?;
            if asset.contains("..") {
                return Err(FunctionCallError::RespondToModel(format!(
                    "start_render: asset '{asset}' must not contain '..' segments"
                )));
            }
            let asset_path = ctx.project_root.join(asset);
            if !asset_path.exists() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "start_render: asset '{asset}' not found at {}",
                    asset_path.display()
                )));
            }
            let stem = asset_stem(asset);
            let output_path =
                renders_dir.join(format!("{}-{}-{}.mp4", args.scope, stem, timestamp));
            validate_render_output_path(
                &ctx.project_root,
                &output_path,
                std::slice::from_ref(&asset_path),
                &[],
                OutputPathPolicy::default(),
            )
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "start_render: output path preflight failed: {e}"
                ))
            })?;
            let argv = build_ffmpeg_argv(&args, asset, &asset_path, &output_path)?;
            let backend = awidat_render::RenderBackendKind::from_start_render_scope(&args.scope)
                .ok_or_else(|| {
                    FunctionCallError::RespondToModel(format!(
                        "start_render: scope '{}' not recognized for render manifest",
                        args.scope
                    ))
                })?;
            (
                argv,
                range_duration(&args.range),
                asset.to_string(),
                output_path,
                Vec::<RenderPlanLimitation>::new(),
                Some(asset_path.clone()),
                vec![asset_path],
                backend,
                BTreeMap::new(),
            )
        };
        enrich_render_metadata_with_backend_capability(&mut render_metadata_for_manifest, &backend);

        let use_master_loudnorm_orchestrator = args.scope == "timeline"
            && args.guide.is_none()
            && args.preset.is_none()
            && matches!(
                awidat_render::read_master_loudnorm_plan(&ctx.project_root),
                Ok(Some(_))
            );
        if use_master_loudnorm_orchestrator {
            render_metadata_for_manifest.insert(
                "master_loudnorm_orchestration".into(),
                "two_pass_background".into(),
            );
        }
        let mut manifest_metadata = serde_json::json!({
            "scope": args.scope.as_str(),
            "asset": asset_label.as_str(),
            "guide": args.guide.as_ref().map(|guide| serde_json::json!({
                "track_id": guide.track_id,
                "marker_id": guide.marker_id,
            })),
            "preset": args.preset.as_deref(),
        });
        if let Some(object) = manifest_metadata.as_object_mut() {
            for (key, value) in &render_metadata_for_manifest {
                object.insert(key.clone(), serde_json::json!(value));
            }
        }
        let manifest = build_start_render_manifest(StartRenderManifestInput {
            project_root: &ctx.project_root,
            scope: &args.scope,
            asset_path: asset_path_for_manifest.as_deref(),
            input_paths: &input_paths_for_manifest,
            output_path: &output_path,
            argv: &argv,
            backend,
            limitations: &limitations,
            metadata: manifest_metadata,
        })?;
        if !use_master_loudnorm_orchestrator {
            awidat_render::write_render_manifest(&manifest.manifest_path, &manifest.manifest)
                .map_err(|e| {
                    FunctionCallError::RespondToModel(format!(
                        "start_render: failed to write render manifest {}: {e}",
                        manifest.manifest_path.display()
                    ))
                })?;
        }

        let spec = RenderJobSpec {
            args: argv,
            backend: manifest.manifest.backend.clone(),
            total_duration_s,
            cwd: Some(ctx.project_root.clone()),
            output_path: output_path.clone(),
            input_paths: input_paths_for_manifest,
            manifest_path: Some(manifest.manifest_path.clone()),
            limitations: limitations.clone(),
            metadata: render_metadata_for_manifest.clone(),
        };
        let job_id = if use_master_loudnorm_orchestrator {
            let _ = spec;
            ctx.job_manager
                .start_master_loudnorm(awidat_render::MasterLoudnormManagedRenderSpec {
                    project_root: ctx.project_root.clone(),
                    output_path: output_path.clone(),
                    manifest_path: manifest.manifest_path.clone(),
                    manifest: manifest.manifest.clone(),
                })
                .await
                .map_err(|e| {
                    FunctionCallError::RespondToModel(format!(
                        "start_render: failed to start master loudnorm render: {e}"
                    ))
                })?
        } else {
            ctx.job_manager.start(spec).await.map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "start_render: failed to start ffmpeg: {e}"
                ))
            })?
        };

        let started_at = chrono::Utc::now().to_rfc3339();
        let body = build_start_render_response(StartRenderResponseInput {
            job_id: &job_id.to_string(),
            scope: &args.scope,
            guide: args.guide.as_ref(),
            asset_label: &asset_label,
            output_path: &output_path,
            manifest_path: &manifest.manifest_path,
            backend: manifest.manifest.backend.clone(),
            render_metadata: &render_metadata_for_manifest,
            limitations: &limitations,
            started_at: &started_at,
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

struct StartRenderResponseInput<'a> {
    job_id: &'a str,
    scope: &'a str,
    guide: Option<&'a GuideSection>,
    asset_label: &'a str,
    output_path: &'a Path,
    manifest_path: &'a Path,
    backend: awidat_render::RenderBackendKind,
    render_metadata: &'a BTreeMap<String, String>,
    limitations: &'a [RenderPlanLimitation],
    started_at: &'a str,
}

fn build_start_render_response(input: StartRenderResponseInput<'_>) -> serde_json::Value {
    let backend = render_backend_json_value(&input.backend);
    let mut backend_evidence = input.render_metadata.clone();
    enrich_render_metadata_with_backend_capability(&mut backend_evidence, &input.backend);
    serde_json::json!({
        "job_id": input.job_id,
        "scope": input.scope,
        "render_kind": render_kind(input.scope, input.guide),
        "asset": input.asset_label,
        "output_path": input.output_path.display().to_string(),
        "manifest_path": input.manifest_path.display().to_string(),
        "backend": backend,
        "backend_evidence": backend_evidence,
        "render_limitations": input.limitations,
        "started_at": input.started_at,
        "guide": input.guide.as_ref().map(|guide| serde_json::json!({
            "track_id": guide.track_id,
            "marker_id": guide.marker_id,
        })),
        "next_step": next_render_step(input.scope, input.job_id),
    })
}

fn render_backend_json_value(backend: &awidat_render::RenderBackendKind) -> String {
    serde_json::to_value(backend)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{backend:?}"))
}

fn render_kind(scope: &str, guide: Option<&GuideSection>) -> &'static str {
    if scope == "timeline" && guide.is_some() {
        "timeline_section_export"
    } else if scope == "timeline" {
        "final_timeline_export"
    } else {
        "diagnostic_asset_render"
    }
}

fn next_render_step(scope: &str, job_id: &str) -> String {
    if scope == "timeline" {
        format!("Call poll_render(job_id=\"{job_id}\") to track the final timeline export.")
    } else {
        format!(
            "Call poll_render(job_id=\"{job_id}\") to track this diagnostic asset render; use scope=\"timeline\" for final editorial output."
        )
    }
}

fn enrich_render_metadata_with_caption_summary(
    metadata: &mut BTreeMap<String, String>,
    summary: &crate::captions::CaptionSummary,
) {
    metadata.extend(crate::captions::caption_summary_metadata(summary));
}

fn enrich_render_metadata_with_backend_capability(
    metadata: &mut BTreeMap<String, String>,
    backend: &awidat_render::RenderBackendKind,
) {
    metadata.extend(crate::capabilities::render_feature_metadata_for_backend(
        backend,
    ));
}

struct StartRenderManifestInput<'a> {
    project_root: &'a Path,
    scope: &'a str,
    asset_path: Option<&'a Path>,
    input_paths: &'a [PathBuf],
    output_path: &'a Path,
    argv: &'a [String],
    backend: awidat_render::RenderBackendKind,
    limitations: &'a [RenderPlanLimitation],
    metadata: serde_json::Value,
}

struct BuiltStartRenderManifest {
    manifest_path: PathBuf,
    manifest: awidat_render::RenderExecutionManifest,
}

fn build_start_render_manifest(
    input: StartRenderManifestInput<'_>,
) -> Result<BuiltStartRenderManifest, FunctionCallError> {
    let mut inputs = Vec::new();
    if let Some(asset_path) = input.asset_path {
        inputs.push(
            awidat_render::fingerprint_file(asset_path, true).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "start_render: failed to fingerprint input {}: {e}",
                    asset_path.display()
                ))
            })?,
        );
    }
    for input_path in input.input_paths {
        if Some(input_path.as_path()) == input.asset_path {
            continue;
        }
        inputs.push(
            awidat_render::fingerprint_file(input_path, true).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "start_render: failed to fingerprint input {}: {e}",
                    input_path.display()
                ))
            })?,
        );
    }
    let project_otio_path = input.project_root.join("project.otio.json");
    let project_hash = optional_file_hash(&project_otio_path)?;
    let timeline_hash = if input.scope == "timeline" {
        project_hash.clone()
    } else {
        None
    };
    let ffmpeg_path = awidat_render::ffmpeg_path().map_err(|e| {
        FunctionCallError::RespondToModel(format!("start_render: failed to locate ffmpeg: {e}"))
    })?;
    let mut replay_argv = vec![ffmpeg_path.to_string_lossy().into_owned()];
    replay_argv.extend(input.argv.iter().cloned());
    let limitations = input
        .limitations
        .iter()
        .map(|limitation| {
            awidat_render::limitation(limitation.kind.clone(), limitation.message.clone())
        })
        .collect();
    let mut metadata = json_object_to_string_map(input.metadata);
    enrich_render_metadata_with_backend_capability(&mut metadata, &input.backend);
    let sidecars =
        awidat_render::fingerprint_ffmpeg_subtitle_sidecars(input.argv).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_render: failed to fingerprint render sidecars: {e}"
            ))
        })?;
    metadata.extend(
        awidat_render::ass_sidecar_layout_metadata(input.argv).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_render: failed to inspect ASS sidecar layout: {e}"
            ))
        })?,
    );
    let manifest = awidat_render::planned_at_now(awidat_render::RenderExecutionManifestInput {
        created_at: String::new(),
        awidat_version: env!("CARGO_PKG_VERSION").into(),
        project_root: input.project_root.to_string_lossy().into_owned(),
        project_hash,
        timeline_hash,
        backend: input.backend,
        replay: awidat_render::RenderReplayPlan::FfmpegArgv {
            argv: replay_argv,
            cwd: Some(input.project_root.to_string_lossy().into_owned()),
        },
        inputs,
        outputs: vec![awidat_render::output_artifact(input.output_path, true)],
        sidecars,
        limitations,
        verification: None,
        metadata,
    });
    Ok(BuiltStartRenderManifest {
        manifest_path: awidat_render::manifest_path_for_output(input.output_path),
        manifest,
    })
}

fn optional_file_hash(path: &Path) -> Result<Option<String>, FunctionCallError> {
    if !path.is_file() {
        return Ok(None);
    }
    awidat_render::fingerprint_file(path, true)
        .map(|fingerprint| Some(fingerprint.sha256))
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_render: failed to fingerprint {}: {e}",
                path.display()
            ))
        })
}

fn json_object_to_string_map(value: serde_json::Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| {
                    let rendered = value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string());
                    (key.clone(), rendered)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn asset_stem(asset: &str) -> String {
    PathBuf::from(asset)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset")
        .to_string()
}

fn range_duration(range: &Option<TimeRange>) -> Option<f64> {
    range.as_ref().map(|r| (r.end_s - r.start_s).max(0.0))
}

fn build_ffmpeg_argv(
    args: &StartRenderArgs,
    _asset_rel: &str,
    asset_path: &std::path::Path,
    output_path: &std::path::Path,
) -> Result<Vec<String>, FunctionCallError> {
    let range = args.range.as_ref().map(|r| (r.start_s, r.end_s));
    build_render_argv(
        &args.scope,
        asset_path,
        output_path,
        range,
        args.preset.as_deref(),
    )
}

/// Build the ffmpeg argv for a `start_render` invocation.
///
/// This is the single source of truth for translating a scope (+ optional
/// time range + optional export-codec preset) into the deterministic
/// ffmpeg argv that the job manager runs. The function is `pub` so the
/// integration tests in `tests/start_render_presets.rs` can assert on the
/// argv shape without spinning up a tool context, and so external
/// callers (UI, MCP, alternative entry points) can reuse the same
/// lowering.
///
/// Preset semantics:
/// - `None` — preserve the legacy hardcoded `libx264 + aac` envelope per
///   scope. This is the back-compat path that existing tools rely on.
/// - `Some("hevc")` / `Some("prores")` — build a stripped-down base argv
///   without baked-in codec args and then let
///   [`awidat_render::professional::apply_export_preset_to_spec`] inject
///   the codec/container/bitrate args. This ensures broadcast and
///   archive workflows produce the right delivery codec.
///
/// Unknown preset slugs are rejected with `FunctionCallError::RespondToModel`.
pub fn build_render_argv(
    scope: &str,
    asset_path: &Path,
    output_path: &Path,
    range: Option<(f64, f64)>,
    preset: Option<&str>,
) -> Result<Vec<String>, FunctionCallError> {
    match preset {
        None => build_legacy_argv(scope, asset_path, output_path, range),
        Some(slug) => {
            let preset = resolve_export_preset(slug)?;
            let base = build_preset_base_argv(scope, asset_path, output_path, range)?;
            apply_preset_to_argv(base, output_path, &preset)
        }
    }
}

/// Resolve a preset slug (the public surface — keep it small and
/// explicit) to a concrete `ExportPreset`.
fn resolve_export_preset(slug: &str) -> Result<ExportPreset, FunctionCallError> {
    match slug {
        "hevc" | "h265" | "libx265" => Ok(ExportPreset::archival_hevc()),
        "prores" | "prores_hq" | "prores_ks" => Ok(ExportPreset::archival_prores()),
        // TODO(overnight-wave1-export-codecs): add "dnxhr" and "av1"
        // slugs once the proto layer grows the matching constructors.
        other => Err(FunctionCallError::RespondToModel(format!(
            "start_render: unknown export preset '{other}'. \
             Supported presets: hevc, prores. \
             (DNxHR / AV1 are pending — TODO(overnight-wave1-export-codecs).)"
        ))),
    }
}

/// Legacy `libx264 + aac` argv, preserved verbatim for back-compat.
fn build_legacy_argv(
    scope: &str,
    asset_path: &Path,
    output_path: &Path,
    range: Option<(f64, f64)>,
) -> Result<Vec<String>, FunctionCallError> {
    let mut argv = vec!["-y".to_string(), "-loglevel".into(), "info".into()];
    match scope {
        "preview" => {
            argv.extend([
                "-i".into(),
                asset_path.to_string_lossy().into_owned(),
                "-vf".into(),
                "scale=-2:480".into(),
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "veryfast".into(),
                "-crf".into(),
                "28".into(),
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                "96k".into(),
                output_path.to_string_lossy().into_owned(),
            ]);
        }
        "segment" => {
            let (start_s, end_s) = range.ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "start_render: scope=segment requires `range: { start_s, end_s }`".into(),
                )
            })?;
            if end_s <= start_s {
                return Err(FunctionCallError::RespondToModel(format!(
                    "start_render: range.end_s ({end_s}) must be > range.start_s ({start_s})"
                )));
            }
            argv.extend([
                "-ss".into(),
                format!("{start_s}"),
                "-i".into(),
                asset_path.to_string_lossy().into_owned(),
                "-t".into(),
                format!("{}", end_s - start_s),
                "-c".into(),
                "copy".into(),
                output_path.to_string_lossy().into_owned(),
            ]);
        }
        "full" => {
            argv.extend([
                "-i".into(),
                asset_path.to_string_lossy().into_owned(),
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "medium".into(),
                "-crf".into(),
                "20".into(),
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                "192k".into(),
                output_path.to_string_lossy().into_owned(),
            ]);
        }
        other => {
            return Err(FunctionCallError::RespondToModel(format!(
                "start_render: scope '{other}' not recognized. Use one of: preview, segment, full, timeline."
            )));
        }
    }
    Ok(argv)
}

/// Preset-aware base argv: input + scope-specific filters but **no**
/// codec/container flags. Codec args come from `apply_preset_to_argv`,
/// which delegates to `awidat_render::professional::apply_export_preset_to_spec`.
fn build_preset_base_argv(
    scope: &str,
    asset_path: &Path,
    output_path: &Path,
    range: Option<(f64, f64)>,
) -> Result<Vec<String>, FunctionCallError> {
    let mut argv = vec!["-y".to_string(), "-loglevel".into(), "info".into()];
    match scope {
        "preview" => {
            argv.extend([
                "-i".into(),
                asset_path.to_string_lossy().into_owned(),
                "-vf".into(),
                "scale=-2:480".into(),
                output_path.to_string_lossy().into_owned(),
            ]);
        }
        "full" => {
            argv.extend([
                "-i".into(),
                asset_path.to_string_lossy().into_owned(),
                output_path.to_string_lossy().into_owned(),
            ]);
        }
        "segment" => {
            // Segment is stream-copy by definition — pairing it with a
            // re-encode preset is contradictory. Surface the conflict
            // loudly instead of silently dropping the preset.
            return Err(FunctionCallError::RespondToModel(
                "start_render: scope=segment is stream-copy only; do not combine with an export preset.".into(),
            ));
        }
        other => {
            return Err(FunctionCallError::RespondToModel(format!(
                "start_render: scope '{other}' not recognized for preset rendering. Use preview or full with a preset."
            )));
        }
    }
    // `range` is intentionally ignored for preview/full preset renders —
    // the scope vocabulary treats range as a segment-only knob. If a
    // future preset needs trim handles, lift `range` into the preset
    // `ExportRange` rather than the bare argv builder.
    let _ = range;
    Ok(argv)
}

/// Apply a resolved export preset to a base argv via the shared
/// render-side lowering. Wraps the spec round-trip needed because
/// `apply_export_preset_to_spec` operates on a `RenderJobSpec`.
fn apply_preset_to_argv(
    base: Vec<String>,
    output_path: &Path,
    preset: &ExportPreset,
) -> Result<Vec<String>, FunctionCallError> {
    let spec = RenderJobSpec {
        args: base,
        backend: awidat_render::RenderBackendKind::AssetFullReencode,
        total_duration_s: None,
        cwd: None,
        output_path: output_path.to_path_buf(),
        input_paths: Vec::new(),
        manifest_path: None,
        limitations: Vec::new(),
        metadata: Default::default(),
    };
    let lowered =
        awidat_render::professional::apply_export_preset_to_spec(spec, preset).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_render: failed to apply export preset '{}': {e}",
                preset.id
            ))
        })?;
    Ok(lowered.args)
}

const DESCRIPTION: &str = "\
Kick off a background ffmpeg render. Returns a job_id IMMEDIATELY — the \
render runs in the background and you call `poll_render` to check status. \
Scopes: 'preview' = 480p H.264 of an asset (fast, low-bitrate); 'segment' \
= trim [start_s, end_s) of an asset via stream-copy (very fast, no \
re-encode); 'full' = high-bitrate H.264 of an asset (slow); 'timeline' \
= render the *edited timeline* (walks project.otio.json, concats every \
video clip's source_range with re-encode at boundaries). Use 'timeline' \
when the user wants 'render the edit' or 'export what's in the \
timeline' — it captures Trim/Untrim/Delete/Split decisions. Preview, \
segment, and full are source-asset diagnostics, not substitutes for \
graph edits or final editorial export. Output \
lands under <project>/renders/. Don't await this tool — it always \
returns within a second.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),

            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "start_render".into(),
            args,
        }
    }

    fn write_podcast_av_mismatch_project(root: &std::path::Path) {
        let mut project = awidat_proto::project::Project::init(root).unwrap();
        let asset = root.join("raw/episode.mov");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"fake").unwrap();
        project
            .timeline
            .metadata
            .awidat
            .as_mut()
            .unwrap()
            .extra
            .insert(
                "awidat_project_type".into(),
                serde_json::json!({"kind": "podcast"}),
            );

        let mut stack = awidat_proto::otio::Stack::empty("root");
        for (name, kind, duration_s) in [
            ("Video 1", awidat_proto::otio::TrackKind::Video, 30.0),
            ("A1", awidat_proto::otio::TrackKind::Audio, 26.0),
        ] {
            let mut clip = awidat_proto::otio::Clip::empty(name);
            clip.media_reference = awidat_proto::otio::MediaReference::External(
                awidat_proto::otio::ExternalReference::new("raw/episode.mov"),
            );
            clip.source_range = Some(awidat_proto::otio::TimeRange::new(
                awidat_proto::otio::RationalTime::zero(24.0),
                awidat_proto::otio::RationalTime::new(duration_s * 24.0, 24.0),
            ));
            let mut track = awidat_proto::otio::Track::empty(name, kind);
            track
                .children
                .push(awidat_proto::otio::TrackChild::Clip(clip));
            stack
                .children
                .push(awidat_proto::otio::StackChild::Track(track));
        }
        project.timeline.tracks = stack;
        project.write(root).unwrap();
    }

    #[test]
    fn start_render_manifest_for_preview_records_ffmpeg_replay() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/x.mp4");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"asset").unwrap();
        let output = dir.path().join("renders/preview-x-120000.mp4");
        let argv = vec![
            "-i".into(),
            asset.to_string_lossy().into_owned(),
            output.to_string_lossy().into_owned(),
        ];

        let built = build_start_render_manifest(StartRenderManifestInput {
            project_root: dir.path(),
            scope: "preview",
            asset_path: Some(&asset),
            input_paths: std::slice::from_ref(&asset),
            output_path: &output,
            argv: &argv,
            backend: awidat_render::RenderBackendKind::AssetPreview,
            limitations: &[],
            metadata: serde_json::json!({"scope": "preview"}),
        })
        .unwrap();

        assert_eq!(
            built.manifest.backend,
            awidat_render::RenderBackendKind::AssetPreview
        );
        assert_eq!(
            built.manifest_path,
            awidat_render::manifest_path_for_output(&output)
        );
        assert_eq!(built.manifest.inputs.len(), 1);
        assert!(matches!(
            built.manifest.replay,
            awidat_render::RenderReplayPlan::FfmpegArgv { .. }
        ));
    }

    #[test]
    fn start_render_manifest_for_timeline_hashes_project_file() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path().join("project.otio.json");
        std::fs::write(&project_path, br#"{"OTIO_SCHEMA":"Timeline.1"}"#).unwrap();
        let output = dir.path().join("renders/timeline.mp4");
        let argv = vec![
            "-i".into(),
            "raw/x.mp4".into(),
            output.to_string_lossy().into_owned(),
        ];

        let built = build_start_render_manifest(StartRenderManifestInput {
            project_root: dir.path(),
            scope: "timeline",
            asset_path: None,
            input_paths: &[],
            output_path: &output,
            argv: &argv,
            backend: awidat_render::RenderBackendKind::TimelineFfmpegReencode,
            limitations: &[],
            metadata: serde_json::json!({"scope": "timeline"}),
        })
        .unwrap();

        assert_eq!(
            built.manifest.backend,
            awidat_render::RenderBackendKind::TimelineFfmpegReencode
        );
        assert!(built.manifest.project_hash.is_some());
        assert!(built.manifest.timeline_hash.is_some());
        assert_eq!(
            built.manifest.metadata["render_feature_id"],
            "ffmpeg_timeline_export"
        );
        assert_eq!(
            built.manifest.metadata["render_feature_export_supported"],
            "supported"
        );
    }

    #[test]
    fn start_render_response_includes_backend_evidence() {
        let body = build_start_render_response(StartRenderResponseInput {
            job_id: "job-1",
            scope: "timeline",
            guide: None,
            asset_label: "<timeline>",
            output_path: std::path::Path::new("/tmp/out.mp4"),
            manifest_path: std::path::Path::new("/tmp/out.render-manifest.json"),
            backend: awidat_render::RenderBackendKind::TimelineFfmpegReencode,
            render_metadata: &BTreeMap::from([
                (
                    "timeline_backend".to_string(),
                    "timeline_ffmpeg_reencode".to_string(),
                ),
                (
                    "timeline_backend_reason".to_string(),
                    "ffmpeg_with_libass_captions".to_string(),
                ),
                ("libass_caption_count".to_string(), "2".to_string()),
            ]),
            limitations: &[],
            started_at: "2026-05-22T10:00:00Z",
        });

        assert_eq!(body["backend"], "timeline_ffmpeg_reencode");
        assert_eq!(body["manifest_path"], "/tmp/out.render-manifest.json");
        assert_eq!(
            body["backend_evidence"]["render_feature_id"],
            "ffmpeg_timeline_export"
        );
        assert_eq!(
            body["backend_evidence"]["render_feature_export_supported"],
            "supported"
        );
        assert_eq!(
            body["backend_evidence"]["timeline_backend"],
            "timeline_ffmpeg_reencode"
        );
        assert_eq!(
            body["backend_evidence"]["timeline_backend_reason"],
            "ffmpeg_with_libass_captions"
        );
        assert_eq!(body["backend_evidence"]["libass_caption_count"], "2");
    }

    #[test]
    fn timeline_render_metadata_includes_caption_summary_evidence() {
        let mut metadata = BTreeMap::from([(
            "timeline_backend_reason".to_string(),
            "ffmpeg_with_libass_captions".to_string(),
        )]);
        let summary = crate::captions::CaptionSummary {
            selected_authority: crate::captions::CaptionAuthority::CaptionOverlays,
            editable_track_count: 0,
            editable_cue_count: 0,
            caption_overlay_count: 2,
            word_timed_caption_overlay_count: 1,
            safe_area_caption_overlay_count: 1,
            mobile_safe_area_caption_overlay_count: 1,
            missing_safe_area_caption_overlay_count: 1,
            sidecar_cue_count: 0,
            has_exportable_captions: true,
            warnings: vec!["1 caption overlays missing safe_area metadata".into()],
        };

        enrich_render_metadata_with_caption_summary(&mut metadata, &summary);

        assert_eq!(
            metadata["timeline_backend_reason"],
            "ffmpeg_with_libass_captions"
        );
        assert_eq!(metadata["caption_authority"], "caption_overlays");
        assert_eq!(metadata["caption_overlay_count"], "2");
        assert_eq!(metadata["word_timed_caption_overlay_count"], "1");
        assert_eq!(metadata["safe_area_caption_overlay_count"], "1");
        assert_eq!(metadata["mobile_safe_area_caption_overlay_count"], "1");
        assert_eq!(metadata["missing_safe_area_caption_overlay_count"], "1");
        assert_eq!(metadata["caption_warning_count"], "1");
    }

    #[tokio::test]
    async fn missing_asset_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = StartRenderTool
            .handle(
                invoke(serde_json::json!({"scope": "preview", "asset": "raw/missing.mp4"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => assert!(msg.contains("not found")),
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn segment_without_range_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("raw/x.mp4");
        std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
        std::fs::write(&asset, b"stub").unwrap();
        let err = StartRenderTool
            .handle(
                invoke(serde_json::json!({"scope": "segment", "asset": "raw/x.mp4"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("segment requires"))
        );
    }

    #[tokio::test]
    async fn dotdot_asset_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let err = StartRenderTool
            .handle(
                invoke(serde_json::json!({"scope": "preview", "asset": "../escape.mp4"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("'..'")));
    }

    #[test]
    fn argv_for_preview_includes_libx264() {
        let asset = std::path::Path::new("/proj/raw/x.mp4");
        let out = std::path::Path::new("/proj/renders/preview-x-000000.mp4");
        let args = StartRenderArgs {
            scope: "preview".into(),
            asset: Some("raw/x.mp4".into()),
            range: None,
            guide: None,
            preset: None,
        };
        let argv = build_ffmpeg_argv(&args, "raw/x.mp4", asset, out).unwrap();
        assert!(argv.contains(&"libx264".to_string()));
        assert!(argv.contains(&out.to_string_lossy().into_owned()));
    }

    #[test]
    fn argv_for_segment_uses_stream_copy() {
        let asset = std::path::Path::new("/proj/raw/x.mp4");
        let out = std::path::Path::new("/proj/renders/segment-x-000000.mp4");
        let args = StartRenderArgs {
            scope: "segment".into(),
            asset: Some("raw/x.mp4".into()),
            range: Some(TimeRange {
                start_s: 1.0,
                end_s: 3.5,
            }),
            guide: None,
            preset: None,
        };
        let argv = build_ffmpeg_argv(&args, "raw/x.mp4", asset, out).unwrap();
        assert!(argv.iter().any(|a| a == "copy"));
        assert!(argv.iter().any(|a| a == "1"));
        assert!(argv.iter().any(|a| a == "2.5"));
    }

    #[test]
    fn description_requires_timeline_scope_for_edits() {
        assert!(DESCRIPTION.contains("render the *edited timeline*"));
        assert!(DESCRIPTION.contains("not substitutes for graph edits"));
        assert!(DESCRIPTION.contains("final editorial export"));
    }

    #[test]
    fn schema_exposes_timeline_guide_section_selection() {
        let schema = StartRenderTool.schema().input_schema;
        assert_eq!(
            schema["properties"]["guide"]["properties"]["track_id"]["type"],
            "string"
        );
        assert_eq!(
            schema["properties"]["guide"]["properties"]["marker_id"]["type"],
            "string"
        );
        assert!(
            schema["properties"]["guide"]["description"]
                .as_str()
                .unwrap()
                .contains("scope=timeline")
        );
    }

    // Pure-function tests for the timeline-render planner moved to
    // `awidat-render::timeline` alongside the implementation. The
    // tests below exercise the tool's error-mapping path end-to-end.

    #[tokio::test]
    async fn timeline_scope_without_otio_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = StartRenderTool
            .handle(
                invoke(serde_json::json!({"scope": "timeline"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("project.otio.json"))
        );
    }

    #[tokio::test]
    async fn timeline_scope_with_empty_timeline_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        // Init a real project (empty timeline).
        awidat_proto::project::Project::init(dir.path()).unwrap();
        let err = StartRenderTool
            .handle(
                invoke(serde_json::json!({"scope": "timeline"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("no clips")));
    }

    #[tokio::test]
    async fn timeline_scope_blocks_podcast_render_when_qc_is_blocked() {
        let dir = tempfile::tempdir().unwrap();
        write_podcast_av_mismatch_project(dir.path());
        let err = StartRenderTool
            .handle(
                invoke(serde_json::json!({"scope": "timeline"})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, FunctionCallError::RespondToModel(msg) if msg.contains("podcast timeline render blocked by QC") && msg.contains("primary_av_duration_mismatch"))
        );
    }
}
