//! `start_render` — kick off an ffmpeg render. Ported from
//! `crates/core/src/tools/start_render.rs` to the in-process MCP
//! server.
//!
//! The original tool returned a job id immediately and let a per-session
//! `JobManager` shepherd the render in the background. The MCP server
//! has no enclosing `Session` / `JobManager`, so this port runs ffmpeg
//! INLINE: build the argv + manifest, then await the ffmpeg subprocess
//! to completion before returning. The returned JSON exposes the final
//! output path, exit status, and manifest location so the agent can
//! call `verify_render` on the finished file. The master-loudnorm
//! two-pass orchestration is stubbed out here — it requires the
//! `JobManager` worker contract and is not yet ported.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use montage_proto::professional::ExportPreset;
use montage_proto::project::Project;
use montage_render::{OutputPathPolicy, RenderPlanLimitation, validate_render_output_path};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `start_render`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct StartRenderArgs {
    /// `preview` | `segment` | `full` | `timeline`.
    pub scope: String,
    /// Source asset (project-relative). Required for preview/segment/full;
    /// ignored for `timeline`.
    #[serde(default)]
    pub asset: Option<String>,
    /// Optional [start_s, end_s) range. Required for `segment`.
    #[serde(default)]
    pub range: Option<TimeRange>,
    /// Optional timeline guide marker section. Only used with `scope=timeline`.
    #[serde(default)]
    pub guide: Option<GuideSection>,
    /// Optional export-codec preset slug. Recognized values: `"hevc"`,
    /// `"prores"`.
    #[serde(default)]
    pub preset: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TimeRange {
    pub start_s: f64,
    pub end_s: f64,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct GuideSection {
    pub track_id: String,
    pub marker_id: String,
}

pub async fn run(args: StartRenderArgs, ctx: McpToolCtx) -> Result<String, String> {
    let renders_dir = ctx.project_root.join("renders");
    tokio::fs::create_dir_all(&renders_dir).await.ok();
    let timestamp = chrono::Utc::now().format("%H%M%S");

    // Fork on scope.
    let (
        argv,
        _total_duration_s,
        asset_label,
        output_path,
        limitations,
        asset_path_for_manifest,
        input_paths_for_manifest,
        backend,
        mut render_metadata_for_manifest,
    ) = if args.scope == "timeline" {
        let project = Project::read(&ctx.project_root)
            .map_err(|e| format!("start_render: failed to read project metadata: {e}"))?;
        if crate::podcast_analysis::is_podcast_project(&project) {
            let qc_report =
                crate::podcast_analysis::build_podcast_qc_report(&ctx.project_root, &project);
            if qc_report["status"] == "blocked" {
                return Err(format!(
                    "start_render: podcast timeline render blocked by QC. Run \
                     podcast_qc_report and fix the error issue(s) before rendering. \
                     Current QC: {qc_report}"
                ));
            }
        }
        crate::lessons::apply_learned_project_format_defaults(&ctx.project_root)
            .map_err(|e| format!("start_render: {e}"))?;
        let mut spec = match args.guide.as_ref() {
            Some(guide) => montage_render::build_timeline_section_render_spec(
                &ctx.project_root,
                &guide.track_id,
                &guide.marker_id,
            ),
            None => montage_render::build_timeline_render_spec(&ctx.project_root),
        }
        .map_err(|e| {
            use montage_render::RenderTimelineError;
            match e {
                RenderTimelineError::EmptyTimeline => {
                    "start_render: timeline has no clips to render. \
                     Add clips via apply_edl first, or use scope=preview to render \
                     the raw asset."
                        .to_string()
                }
                RenderTimelineError::NoOtio(p) => format!(
                    "start_render: no project.otio.json found at {} — \
                     this isn't an montage project root.",
                    p.display()
                ),
                RenderTimelineError::OtioParse { message } => format!(
                    "start_render: timeline parse failed ({message}). \
                     Run `montage validate <project>` for the detailed diagnostic, \
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
                RenderTimelineError::AudioRemovalUnsupported { clip_name, reason } => format!(
                    "start_render: clip '{clip_name}' audio removal can't be exported: {reason}. \
                     Remove the speed change or split edit on that clip, or clear the audio \
                     removal (Remove Audio with clear:true) before rendering."
                ),
                RenderTimelineError::UnsupportedTransition { kind, message } => format!(
                    "start_render: timeline transition {kind:?} cannot be exported: \
                     {message}"
                ),
                RenderTimelineError::InvalidTransitionPlacement { message } => {
                    format!("start_render: timeline has an invalid transition: {message}")
                }
                RenderTimelineError::InvalidTransitionMetadata { kind, message } => format!(
                    "start_render: transition {kind:?} has invalid Montage metadata: \
                     {message}. Use data-only transition primitives with bounded params; do \
                     not put raw FFmpeg, GLSL, shell, or plugin code in transition metadata."
                ),
                RenderTimelineError::InvalidClipEffectMetadata {
                    clip_name,
                    effect,
                    message,
                } => format!(
                    "start_render: clip '{clip_name}' has invalid {effect} metadata: \
                     {message}. Use the registered montage effect parameter schema and avoid \
                     raw renderer expressions in clip effect metadata."
                ),
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
                RenderTimelineError::HeadRenderMissingDuration => {
                    "start_render: head render needs a positive duration".to_string()
                }
            }
        })?;
        if let Some(slug) = args.preset.as_deref() {
            let preset = resolve_export_preset(slug)?;
            spec = montage_render::professional::apply_export_preset_to_spec(spec, &preset)
                .map_err(|e| {
                    format!(
                        "start_render: failed to apply export preset '{}': {e}",
                        preset.id
                    )
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
            format!(
                "start_render: scope='{}' requires an `asset` path",
                args.scope
            )
        })?;
        if asset.contains("..") {
            return Err(format!(
                "start_render: asset '{asset}' must not contain '..' segments"
            ));
        }
        let asset_path = ctx.project_root.join(asset);
        if !asset_path.exists() {
            return Err(format!(
                "start_render: asset '{asset}' not found at {}",
                asset_path.display()
            ));
        }
        let stem = asset_stem(asset);
        let output_path = renders_dir.join(format!("{}-{}-{}.mp4", args.scope, stem, timestamp));
        validate_render_output_path(
            &ctx.project_root,
            &output_path,
            std::slice::from_ref(&asset_path),
            &[],
            OutputPathPolicy::default(),
        )
        .map_err(|e| format!("start_render: output path preflight failed: {e}"))?;
        let argv = build_ffmpeg_argv(&args, asset, &asset_path, &output_path)?;
        let backend = montage_render::RenderBackendKind::from_start_render_scope(&args.scope)
            .ok_or_else(|| {
                format!(
                    "start_render: scope '{}' not recognized for render manifest",
                    args.scope
                )
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

    // Master-loudnorm two-pass orchestration is not yet supported by
    // the in-process MCP server; if a plan is present we surface a
    // clear error so the caller knows to use the desktop / CLI path.
    let has_master_loudnorm_plan = args.scope == "timeline"
        && args.guide.is_none()
        && args.preset.is_none()
        && matches!(
            montage_render::read_master_loudnorm_plan(&ctx.project_root),
            Ok(Some(_))
        );
    if has_master_loudnorm_plan {
        return Err(
            "start_render: this project has a master-loudnorm two-pass plan that the \
             in-process MCP server cannot orchestrate yet. Run the timeline render from \
             the desktop UI or the `montage render` CLI until out-of-process job state \
             is wired up."
                .into(),
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
    montage_render::write_render_manifest(&manifest.manifest_path, &manifest.manifest).map_err(
        |e| {
            format!(
                "start_render: failed to write render manifest {}: {e}",
                manifest.manifest_path.display()
            )
        },
    )?;

    // Run ffmpeg inline. No job_manager, no background task — the MCP
    // server has no place to hold an in-flight job between calls.
    let started_at = chrono::Utc::now();
    let ffmpeg_bin = montage_render::ffmpeg_path()
        .map_err(|e| format!("start_render: failed to locate ffmpeg: {e}"))?;
    let mut command = Command::new(&ffmpeg_bin);
    command.args(&argv).current_dir(&ctx.project_root);
    let output = command
        .output()
        .await
        .map_err(|e| format!("start_render: failed to spawn ffmpeg: {e}"))?;
    let finished_at = chrono::Utc::now();
    let exit_code = output.status.code();
    let state = if output.status.success() {
        "done"
    } else {
        "failed"
    };

    let body = build_start_render_response(StartRenderResponseInput {
        scope: &args.scope,
        guide: args.guide.as_ref(),
        asset_label: &asset_label,
        output_path: &output_path,
        manifest_path: &manifest.manifest_path,
        backend: manifest.manifest.backend.clone(),
        render_metadata: &render_metadata_for_manifest,
        limitations: &limitations,
        started_at: &started_at.to_rfc3339(),
        finished_at: &finished_at.to_rfc3339(),
        state,
        exit_code,
        stderr_tail: &tail_str(&String::from_utf8_lossy(&output.stderr), 4096),
    });
    Ok(body.to_string())
}

struct StartRenderResponseInput<'a> {
    scope: &'a str,
    guide: Option<&'a GuideSection>,
    asset_label: &'a str,
    output_path: &'a Path,
    manifest_path: &'a Path,
    backend: montage_render::RenderBackendKind,
    render_metadata: &'a BTreeMap<String, String>,
    limitations: &'a [RenderPlanLimitation],
    started_at: &'a str,
    finished_at: &'a str,
    state: &'a str,
    exit_code: Option<i32>,
    stderr_tail: &'a str,
}

fn build_start_render_response(input: StartRenderResponseInput<'_>) -> serde_json::Value {
    let backend = render_backend_json_value(&input.backend);
    let mut backend_evidence = input.render_metadata.clone();
    enrich_render_metadata_with_backend_capability(&mut backend_evidence, &input.backend);
    serde_json::json!({
        "scope": input.scope,
        "render_kind": render_kind(input.scope, input.guide),
        "asset": input.asset_label,
        "output_path": input.output_path.display().to_string(),
        "manifest_path": input.manifest_path.display().to_string(),
        "backend": backend,
        "backend_evidence": backend_evidence,
        "render_limitations": input.limitations,
        "started_at": input.started_at,
        "finished_at": input.finished_at,
        "state": input.state,
        "exit_code": input.exit_code,
        "stderr_tail": input.stderr_tail,
        "guide": input.guide.as_ref().map(|guide| serde_json::json!({
            "track_id": guide.track_id,
            "marker_id": guide.marker_id,
        })),
        "next_step": next_render_step(input.scope, input.state, &input.output_path.display().to_string()),
    })
}

fn render_backend_json_value(backend: &montage_render::RenderBackendKind) -> String {
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

fn next_render_step(scope: &str, state: &str, output_path: &str) -> String {
    if state != "done" {
        return format!(
            "Render did not succeed (state={state}); inspect stderr_tail and re-run after fixing inputs."
        );
    }
    if scope == "timeline" {
        format!(
            "Call verify_render(output_path=\"{output_path}\") to verify the final timeline export."
        )
    } else {
        format!(
            "Diagnostic asset render complete at {output_path}; call verify_render or run scope=\"timeline\" for final editorial output."
        )
    }
}

fn tail_str(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        s.to_string()
    } else {
        let start = s.len() - max_bytes;
        format!("[…earlier log truncated…]\n{}", &s[start..])
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
    backend: &montage_render::RenderBackendKind,
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
    backend: montage_render::RenderBackendKind,
    limitations: &'a [RenderPlanLimitation],
    metadata: serde_json::Value,
}

struct BuiltStartRenderManifest {
    manifest_path: PathBuf,
    manifest: montage_render::RenderExecutionManifest,
}

fn build_start_render_manifest(
    input: StartRenderManifestInput<'_>,
) -> Result<BuiltStartRenderManifest, String> {
    let mut inputs = Vec::new();
    if let Some(asset_path) = input.asset_path {
        inputs.push(
            montage_render::fingerprint_file(asset_path, true).map_err(|e| {
                format!(
                    "start_render: failed to fingerprint input {}: {e}",
                    asset_path.display()
                )
            })?,
        );
    }
    for input_path in input.input_paths {
        if Some(input_path.as_path()) == input.asset_path {
            continue;
        }
        inputs.push(
            montage_render::fingerprint_file(input_path, true).map_err(|e| {
                format!(
                    "start_render: failed to fingerprint input {}: {e}",
                    input_path.display()
                )
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
    let ffmpeg_path = montage_render::ffmpeg_path()
        .map_err(|e| format!("start_render: failed to locate ffmpeg: {e}"))?;
    let mut replay_argv = vec![ffmpeg_path.to_string_lossy().into_owned()];
    replay_argv.extend(input.argv.iter().cloned());
    let limitations = input
        .limitations
        .iter()
        .map(|limitation| {
            montage_render::limitation(limitation.kind.clone(), limitation.message.clone())
        })
        .collect();
    let mut metadata = json_object_to_string_map(input.metadata);
    enrich_render_metadata_with_backend_capability(&mut metadata, &input.backend);
    let sidecars = montage_render::fingerprint_ffmpeg_subtitle_sidecars(input.argv)
        .map_err(|e| format!("start_render: failed to fingerprint render sidecars: {e}"))?;
    metadata.extend(
        montage_render::ass_sidecar_layout_metadata(input.argv)
            .map_err(|e| format!("start_render: failed to inspect ASS sidecar layout: {e}"))?,
    );
    let manifest = montage_render::planned_at_now(montage_render::RenderExecutionManifestInput {
        created_at: String::new(),
        montage_version: env!("CARGO_PKG_VERSION").into(),
        project_root: input.project_root.to_string_lossy().into_owned(),
        project_hash,
        timeline_hash,
        backend: input.backend,
        replay: montage_render::RenderReplayPlan::FfmpegArgv {
            argv: replay_argv,
            cwd: Some(input.project_root.to_string_lossy().into_owned()),
        },
        inputs,
        outputs: vec![montage_render::output_artifact(input.output_path, true)],
        sidecars,
        limitations,
        verification: None,
        metadata,
    });
    Ok(BuiltStartRenderManifest {
        manifest_path: montage_render::manifest_path_for_output(input.output_path),
        manifest,
    })
}

fn optional_file_hash(path: &Path) -> Result<Option<String>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    montage_render::fingerprint_file(path, true)
        .map(|fingerprint| Some(fingerprint.sha256))
        .map_err(|e| {
            format!(
                "start_render: failed to fingerprint {}: {e}",
                path.display()
            )
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
) -> Result<Vec<String>, String> {
    let range = args.range.as_ref().map(|r| (r.start_s, r.end_s));
    build_render_argv(
        &args.scope,
        asset_path,
        output_path,
        range,
        args.preset.as_deref(),
    )
}

/// Build the ffmpeg argv for a `start_render` invocation. Mirrors the
/// public helper of the same name in `crates/core/src/tools/start_render.rs`,
/// kept in sync so the MCP-side and the old harness produce identical
/// argv shapes when stitched together with the same preset.
pub fn build_render_argv(
    scope: &str,
    asset_path: &Path,
    output_path: &Path,
    range: Option<(f64, f64)>,
    preset: Option<&str>,
) -> Result<Vec<String>, String> {
    match preset {
        None => build_legacy_argv(scope, asset_path, output_path, range),
        Some(slug) => {
            let preset = resolve_export_preset(slug)?;
            let base = build_preset_base_argv(scope, asset_path, output_path, range)?;
            apply_preset_to_argv(base, output_path, &preset)
        }
    }
}

fn resolve_export_preset(slug: &str) -> Result<ExportPreset, String> {
    match slug {
        "hevc" | "h265" | "libx265" => Ok(ExportPreset::archival_hevc()),
        "prores" | "prores_hq" | "prores_ks" => Ok(ExportPreset::archival_prores()),
        other => Err(format!(
            "start_render: unknown export preset '{other}'. \
             Supported presets: hevc, prores."
        )),
    }
}

fn build_legacy_argv(
    scope: &str,
    asset_path: &Path,
    output_path: &Path,
    range: Option<(f64, f64)>,
) -> Result<Vec<String>, String> {
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
                "start_render: scope=segment requires `range: { start_s, end_s }`".to_string()
            })?;
            if end_s <= start_s {
                return Err(format!(
                    "start_render: range.end_s ({end_s}) must be > range.start_s ({start_s})"
                ));
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
            return Err(format!(
                "start_render: scope '{other}' not recognized. Use one of: preview, segment, full, timeline."
            ));
        }
    }
    Ok(argv)
}

fn build_preset_base_argv(
    scope: &str,
    asset_path: &Path,
    output_path: &Path,
    range: Option<(f64, f64)>,
) -> Result<Vec<String>, String> {
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
            return Err(
                "start_render: scope=segment is stream-copy only; do not combine with an export preset.".into(),
            );
        }
        other => {
            return Err(format!(
                "start_render: scope '{other}' not recognized for preset rendering. Use preview or full with a preset."
            ));
        }
    }
    let _ = range;
    Ok(argv)
}

fn apply_preset_to_argv(
    base: Vec<String>,
    output_path: &Path,
    preset: &ExportPreset,
) -> Result<Vec<String>, String> {
    let spec = montage_render::RenderJobSpec {
        args: base,
        backend: montage_render::RenderBackendKind::AssetFullReencode,
        total_duration_s: None,
        cwd: None,
        output_path: output_path.to_path_buf(),
        input_paths: Vec::new(),
        manifest_path: None,
        limitations: Vec::new(),
        metadata: Default::default(),
    };
    let lowered =
        montage_render::professional::apply_export_preset_to_spec(spec, preset).map_err(|e| {
            format!(
                "start_render: failed to apply export preset '{}': {e}",
                preset.id
            )
        })?;
    Ok(lowered.args)
}

pub const DESCRIPTION: &str = "\
Run an ffmpeg render to completion and return the result. NOTE: this \
in-process MCP port runs ffmpeg inline — it does NOT return a job id; \
it awaits the render and returns once ffmpeg exits. Scopes: 'preview' = \
480p H.264 of an asset (fast); 'segment' = trim [start_s, end_s) of an \
asset via stream-copy; 'full' = high-bitrate H.264 of an asset; \
'timeline' = render the *edited timeline* by walking project.otio.json. \
Output lands under <project>/renders/. Long renders block the agent \
turn — for hour-long timeline exports use the desktop UI or `montage \
render` CLI instead. The response includes output_path, manifest_path, \
final state, exit_code, and a stderr tail.";
