//! `export_package` tool — render timeline and write delivery sidecars.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use awidat_proto::otio::{MediaReference, StackChild, TrackChild, TrackKind};
use awidat_proto::professional::{DeliveryPreflightInput, DeliveryProfile, PreflightReport};
use awidat_proto::project::Project;
use awidat_render::RenderJobSpec;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `export_package` tool.
pub struct ExportPackageTool;

#[derive(Debug, Deserialize)]
struct ExportPackageArgs {
    /// youtube | shorts | podcast | custom | turnover
    format: String,
}

#[async_trait]
impl ToolHandler for ExportPackageTool {
    fn name(&self) -> &'static str {
        "export_package"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "export_package".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "format": {
                        "type": "string",
                        "enum": ["youtube", "shorts", "podcast", "custom", "turnover"],
                        "description": "Package preset. youtube/custom render MP4 + SRT/VTT/chapters/metadata; shorts uses the current timeline format; podcast adds audio metadata sidecars; turnover writes conform-oriented JSON/CSV without rendering."
                    }
                },
                "required": ["format"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: ExportPackageArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "export_package: invalid args ({e}). Required: {{ \"format\": \"youtube|shorts|podcast|custom\" }}."
            ))
        })?;
        if !matches!(
            args.format.as_str(),
            "youtube" | "shorts" | "podcast" | "custom" | "turnover"
        ) {
            return Err(FunctionCallError::RespondToModel(format!(
                "export_package: unknown format {:?}",
                args.format
            )));
        }

        let package_dir = ctx.project_root.join("renders").join("package");
        tokio::fs::create_dir_all(&package_dir).await.map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "export_package: failed to create {}: {e}",
                package_dir.display()
            ))
        })?;

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("export_package: project read failed: {e}"))
        })?;
        if args.format == "turnover" {
            let package = write_turnover_package(&ctx.project_root, &project, &package_dir).await?;
            let body = serde_json::json!({
                "render_kind": "turnover_package",
                "format": args.format,
                "package_dir": package.package_dir,
                "artifacts": package.artifacts,
                "clip_count": package.clip_count,
                "transition_count": package.transition_count,
                "next_step": "Review the turnover manifest and department cut lists before generating AAF/XML handoff from a dedicated interchange adapter."
            });
            return Ok(ToolOutput::text(body.to_string()));
        }
        let applied_format_defaults =
            crate::lessons::apply_learned_project_format_defaults(&ctx.project_root)
                .map_err(|e| FunctionCallError::RespondToModel(format!("export_package: {e}")))?;
        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("export_package: project read failed: {e}"))
        })?;
        let cues = collect_timeline_cues(&ctx.project_root, &project)?;
        let chapters = collect_chapters(&cues);

        let stem = format!("final-{}", args.format);
        let mp4_path = package_dir.join(format!("{stem}.mp4"));
        let srt_path = package_dir.join(format!("{stem}.srt"));
        let vtt_path = package_dir.join(format!("{stem}.vtt"));
        let chapter_path = package_dir.join(format!("{stem}-chapters.txt"));
        let metadata_path = package_dir.join(format!("{stem}-metadata.json"));
        let thumbnail_path = package_dir.join(format!("{stem}-thumbnail-candidates.json"));
        let preflight_path = package_dir.join(format!("{stem}-preflight.json"));

        tokio::fs::write(&srt_path, format_srt(&cues))
            .await
            .map_err(io_err("write srt"))?;
        tokio::fs::write(&vtt_path, format_vtt(&cues))
            .await
            .map_err(io_err("write vtt"))?;
        tokio::fs::write(&chapter_path, format_chapters(&chapters))
            .await
            .map_err(io_err("write chapters"))?;
        tokio::fs::write(&thumbnail_path, thumbnail_candidates(&cues).to_string())
            .await
            .map_err(io_err("write thumbnail candidates"))?;

        let mut spec =
            awidat_render::build_timeline_render_spec(&ctx.project_root).map_err(render_err)?;
        if let Some(last) = spec.args.last_mut() {
            *last = mp4_path.to_string_lossy().to_string();
        }
        spec.output_path = mp4_path.clone();
        let preflight =
            build_package_preflight_report(&project, &args.format, &cues, spec.total_duration_s);
        write_json(&preflight_path, &preflight).await?;
        let job_id = ctx
            .job_manager
            .start(RenderJobSpec {
                args: spec.args,
                total_duration_s: spec.total_duration_s,
                cwd: Some(ctx.project_root.clone()),
                output_path: mp4_path.clone(),
                limitations: spec.limitations.clone(),
            })
            .await
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "export_package: failed to start timeline render: {e}"
                ))
            })?;

        let metadata = serde_json::json!({
            "format": args.format,
            "created_at": chrono::Utc::now().to_rfc3339(),
            "render_job_id": job_id.to_string(),
            "artifacts": {
                "mp4": mp4_path,
                "srt": srt_path,
                "vtt": vtt_path,
                "chapters": chapter_path,
                "metadata": metadata_path,
                "thumbnail_candidates": thumbnail_path,
                "preflight": preflight_path,
            },
            "preflight_profile_id": preflight.profile_id,
            "preflight_finding_count": preflight.findings.len(),
            "cue_count": cues.len(),
            "chapter_count": chapters.len(),
            "subtitle_timing": "timeline_relative",
            "output_format": output_format_metadata(&project),
            "learned_format_defaults_applied": applied_format_defaults.aspect_ratio.is_some(),
        });
        tokio::fs::write(
            &metadata_path,
            serde_json::to_string_pretty(&metadata).unwrap_or_else(|_| metadata.to_string()),
        )
        .await
        .map_err(io_err("write metadata"))?;

        let body = serde_json::json!({
            "job_id": job_id.to_string(),
            "render_kind": "package_export",
            "format": args.format,
            "package_dir": package_dir,
            "artifacts": metadata["artifacts"],
            "next_step": format!("Call poll_render(job_id=\"{job_id}\") to track the final MP4. Subtitle and metadata sidecars are already written.")
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

#[derive(Debug, Clone, Serialize)]
struct TurnoverPackageSummary {
    package_dir: PathBuf,
    artifacts: serde_json::Value,
    clip_count: usize,
    transition_count: usize,
}

#[derive(Debug, Clone, Serialize)]
struct TurnoverManifest {
    schema_version: String,
    package_kind: String,
    created_at: String,
    project_name: String,
    departments: Vec<String>,
    source_assets: Vec<String>,
    media_files: Vec<TurnoverMediaFile>,
    clips: Vec<TurnoverClip>,
    transitions: Vec<TurnoverTransition>,
}

#[derive(Debug, Clone, Serialize)]
struct DepartmentTurnover {
    schema_version: String,
    department: String,
    created_at: String,
    project_name: String,
    clips: Vec<TurnoverClip>,
    transitions: Vec<TurnoverTransition>,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TurnoverClip {
    clip_id: String,
    clip_name: String,
    track: String,
    track_kind: String,
    asset: Option<String>,
    timeline_start_s: f64,
    timeline_end_s: f64,
    source_start_s: Option<f64>,
    source_end_s: Option<f64>,
    source_available_start_s: Option<f64>,
    source_available_end_s: Option<f64>,
    handle_in_s: Option<f64>,
    handle_out_s: Option<f64>,
    duration_s: f64,
    active: bool,
    effects: Vec<String>,
    markers: Vec<TurnoverMarker>,
    link_group_id: Option<String>,
    audio_lead_s: Option<f64>,
    audio_trail_s: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct TurnoverMarker {
    name: String,
    start_s: f64,
    duration_s: f64,
}

#[derive(Debug, Clone, Serialize)]
struct TurnoverTransition {
    track: String,
    track_kind: String,
    kind: String,
    timeline_start_s: f64,
    timeline_end_s: f64,
    in_offset_s: f64,
    out_offset_s: f64,
}

#[derive(Debug, Clone, Serialize)]
struct TurnoverMediaFile {
    asset: String,
    path: String,
    status: String,
    size_bytes: Option<u64>,
    sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proxy_path: Option<String>,
    proxy_status: String,
    error: Option<String>,
}

async fn write_turnover_package(
    project_root: &Path,
    project: &Project,
    package_dir: &Path,
) -> Result<TurnoverPackageSummary, FunctionCallError> {
    let turnover_dir = package_dir.join("turnover");
    tokio::fs::create_dir_all(&turnover_dir)
        .await
        .map_err(io_err("create turnover package dir"))?;

    let created_at = chrono::Utc::now().to_rfc3339();
    let clips = collect_turnover_clips(project);
    let transitions = collect_turnover_transitions(project);
    let source_assets = collect_turnover_source_assets(project, &clips);
    let manifest = TurnoverManifest {
        schema_version: "awidat.turnover.v1".into(),
        package_kind: "department_turnover".into(),
        created_at: created_at.clone(),
        project_name: project.timeline.name.clone(),
        departments: vec!["sound".into(), "color".into(), "vfx".into()],
        media_files: collect_turnover_media_files(project_root, &source_assets),
        source_assets,
        clips,
        transitions,
    };

    let manifest_path = turnover_dir.join("turnover-manifest.json");
    let cut_list_path = turnover_dir.join("turnover-cut-list.csv");
    let cmx3600_path = turnover_dir.join("turnover-cmx3600.edl");
    let fcpxml_path = turnover_dir.join("turnover.fcpxml");
    let sound_path = turnover_dir.join("sound-turnover.json");
    let color_path = turnover_dir.join("color-turnover.json");
    let vfx_path = turnover_dir.join("vfx-turnover.json");

    write_json(&manifest_path, &manifest).await?;
    tokio::fs::write(
        &cut_list_path,
        format_turnover_cut_list_csv(&manifest.clips),
    )
    .await
    .map_err(io_err("write turnover cut list"))?;
    tokio::fs::write(
        &cmx3600_path,
        format_turnover_cmx3600_edl(&manifest.project_name, &manifest.clips),
    )
    .await
    .map_err(io_err("write turnover cmx3600 edl"))?;
    tokio::fs::write(
        &fcpxml_path,
        format_turnover_fcpxml(&manifest.project_name, &manifest.clips),
    )
    .await
    .map_err(io_err("write turnover fcpxml"))?;
    write_json(
        &sound_path,
        &department_turnover(project, &created_at, "sound", &manifest),
    )
    .await?;
    write_json(
        &color_path,
        &department_turnover(project, &created_at, "color", &manifest),
    )
    .await?;
    write_json(
        &vfx_path,
        &department_turnover(project, &created_at, "vfx", &manifest),
    )
    .await?;

    let rel = |path: &Path| {
        path.strip_prefix(project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string()
    };
    Ok(TurnoverPackageSummary {
        package_dir: turnover_dir,
        artifacts: serde_json::json!({
            "manifest": rel(&manifest_path),
            "cut_list_csv": rel(&cut_list_path),
            "cmx3600_edl": rel(&cmx3600_path),
            "fcpxml": rel(&fcpxml_path),
            "sound": rel(&sound_path),
            "color": rel(&color_path),
            "vfx": rel(&vfx_path),
        }),
        clip_count: manifest.clips.len(),
        transition_count: manifest.transitions.len(),
    })
}

async fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), FunctionCallError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| {
        FunctionCallError::RespondToModel(format!("export_package: serialize json failed: {e}"))
    })?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(io_err("write turnover json"))
}

fn collect_turnover_clips(project: &Project) -> Vec<TurnoverClip> {
    let mut clips = Vec::new();
    for stack_child in &project.timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let mut cursor_s = 0.0;
        for child in &track.children {
            let duration_s = child_duration_s(child);
            if let TrackChild::Clip(clip) = child {
                let source_fields = clip_source_fields(clip);
                let clip_id = clip_uuid_or_name(clip);
                let awidat = clip.metadata.awidat.as_ref();
                clips.push(TurnoverClip {
                    clip_id,
                    clip_name: clip.name.clone(),
                    track: track.name.clone(),
                    track_kind: track_kind_label(track.kind).into(),
                    asset: source_fields.asset,
                    timeline_start_s: cursor_s,
                    timeline_end_s: cursor_s + duration_s,
                    source_start_s: source_fields.source_start_s,
                    source_end_s: source_fields.source_end_s,
                    source_available_start_s: source_fields.source_available_start_s,
                    source_available_end_s: source_fields.source_available_end_s,
                    handle_in_s: source_fields.handle_in_s,
                    handle_out_s: source_fields.handle_out_s,
                    duration_s,
                    active: clip.active,
                    effects: clip
                        .effects
                        .iter()
                        .map(|effect| effect.effect_name.clone())
                        .collect(),
                    markers: clip
                        .markers
                        .iter()
                        .map(|marker| TurnoverMarker {
                            name: marker.name.clone(),
                            start_s: marker.marked_range.start_time.to_seconds(),
                            duration_s: marker.marked_range.duration.to_seconds(),
                        })
                        .collect(),
                    link_group_id: awidat
                        .and_then(|metadata| metadata.extra.get("link_group_id"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    audio_lead_s: awidat
                        .and_then(|metadata| metadata.split_edit.as_ref())
                        .and_then(|split| split.audio_lead_s),
                    audio_trail_s: awidat
                        .and_then(|metadata| metadata.split_edit.as_ref())
                        .and_then(|split| split.audio_trail_s),
                });
            }
            cursor_s += duration_s;
        }
    }
    clips
}

fn collect_turnover_transitions(project: &Project) -> Vec<TurnoverTransition> {
    let mut transitions = Vec::new();
    for stack_child in &project.timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let mut cursor_s = 0.0;
        for child in &track.children {
            let duration_s = child_duration_s(child);
            if let TrackChild::Transition(transition) = child {
                transitions.push(TurnoverTransition {
                    track: track.name.clone(),
                    track_kind: track_kind_label(track.kind).into(),
                    kind: transition.transition_type.clone(),
                    timeline_start_s: cursor_s,
                    timeline_end_s: cursor_s + duration_s,
                    in_offset_s: transition.in_offset.to_seconds(),
                    out_offset_s: transition.out_offset.to_seconds(),
                });
            }
            cursor_s += duration_s;
        }
    }
    transitions
}

fn collect_turnover_source_assets(project: &Project, clips: &[TurnoverClip]) -> Vec<String> {
    let mut assets = BTreeSet::new();
    if let Some(metadata) = project.timeline.metadata.awidat.as_ref() {
        assets.extend(metadata.source_assets.iter().cloned());
    }
    assets.extend(clips.iter().filter_map(|clip| clip.asset.clone()));
    assets.into_iter().collect()
}

fn collect_turnover_media_files(project_root: &Path, assets: &[String]) -> Vec<TurnoverMediaFile> {
    assets
        .iter()
        .map(|asset| turnover_media_file(project_root, asset))
        .collect()
}

fn turnover_media_file(project_root: &Path, asset: &str) -> TurnoverMediaFile {
    let path = resolve_project_asset_path(project_root, asset);
    let path_string = path.to_string_lossy().to_string();
    let proxy_path = turnover_proxy_path(project_root, &path);
    let proxy_path_string = proxy_path.to_string_lossy().to_string();
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return TurnoverMediaFile {
                asset: asset.into(),
                path: path_string,
                status: "missing".into(),
                size_bytes: None,
                sha256: None,
                proxy_path: None,
                proxy_status: "blocked".into(),
                error: None,
            };
        }
        Err(error) => {
            return TurnoverMediaFile {
                asset: asset.into(),
                path: path_string,
                status: "unreadable".into(),
                size_bytes: None,
                sha256: None,
                proxy_path: None,
                proxy_status: "blocked".into(),
                error: Some(error.to_string()),
            };
        }
    };
    if !metadata.is_file() {
        return TurnoverMediaFile {
            asset: asset.into(),
            path: path_string,
            status: "unreadable".into(),
            size_bytes: None,
            sha256: None,
            proxy_path: None,
            proxy_status: "blocked".into(),
            error: Some("not a regular file".into()),
        };
    }
    let (proxy_path, proxy_status) = if proxy_path.is_file() {
        (Some(proxy_path_string), "online".into())
    } else {
        (None, "missing".into())
    };
    match awidat_index::asset_sha256(&path) {
        Ok(sha256) => TurnoverMediaFile {
            asset: asset.into(),
            path: path_string,
            status: "online".into(),
            size_bytes: Some(metadata.len()),
            sha256: Some(sha256),
            proxy_path,
            proxy_status,
            error: None,
        },
        Err(error) => TurnoverMediaFile {
            asset: asset.into(),
            path: path_string,
            status: "unreadable".into(),
            size_bytes: Some(metadata.len()),
            sha256: None,
            proxy_path,
            proxy_status,
            error: Some(error.to_string()),
        },
    }
}

fn turnover_proxy_path(project_root: &Path, asset_path: &Path) -> PathBuf {
    let stem = asset_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("asset");
    project_root
        .join(".awidat")
        .join("proxies")
        .join(format!("{stem}-{:08x}.mp4", stable_path_hash(asset_path)))
}

fn stable_path_hash(path: &Path) -> u32 {
    let mut hash = 0x811c9dc5_u32;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

fn resolve_project_asset_path(project_root: &Path, asset: &str) -> PathBuf {
    let path = Path::new(asset);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

fn department_turnover(
    project: &Project,
    created_at: &str,
    department: &str,
    manifest: &TurnoverManifest,
) -> DepartmentTurnover {
    DepartmentTurnover {
        schema_version: "awidat.turnover.v1".into(),
        department: department.into(),
        created_at: created_at.into(),
        project_name: project.timeline.name.clone(),
        clips: manifest
            .clips
            .iter()
            .filter(|clip| department_accepts_clip(department, clip))
            .cloned()
            .collect(),
        transitions: manifest
            .transitions
            .iter()
            .filter(|transition| department_accepts_transition(department, transition))
            .cloned()
            .collect(),
        notes: department_notes(department),
    }
}

fn department_accepts_clip(department: &str, clip: &TurnoverClip) -> bool {
    match department {
        "sound" => {
            clip.track_kind == "audio"
                || clip.audio_lead_s.is_some()
                || clip.audio_trail_s.is_some()
                || clip
                    .effects
                    .iter()
                    .any(|effect| effect.contains("audio") || effect == "awidat.volume")
        }
        "color" => clip.track_kind == "video" && clip.track != "Titles" && clip.asset.is_some(),
        "vfx" => {
            clip.track_kind == "video"
                && (clip.track != "V1"
                    || clip.effects.iter().any(|effect| {
                        effect.contains("title")
                            || effect.contains("lut")
                            || effect.contains("color")
                            || effect.contains("composite")
                    }))
        }
        _ => false,
    }
}

fn department_accepts_transition(department: &str, transition: &TurnoverTransition) -> bool {
    match department {
        "sound" => transition.track_kind == "audio",
        "color" | "vfx" => transition.track_kind == "video",
        _ => false,
    }
}

fn department_notes(department: &str) -> Vec<String> {
    match department {
        "sound" => vec![
            "Timeline seconds are flattened per track; confirm split edits and linked production audio before AAF export.".into(),
        ],
        "color" => vec![
            "Use source_start_s/source_end_s plus source asset paths for conform; LUT/color effects are editorial intent, not final grade.".into(),
        ],
        "vfx" => vec![
            "This is a candidate shot list from timeline structure and effects; confirm handles, plates, and vendor shot names manually.".into(),
        ],
        _ => Vec::new(),
    }
}

struct TurnoverClipSourceFields {
    asset: Option<String>,
    source_start_s: Option<f64>,
    source_end_s: Option<f64>,
    source_available_start_s: Option<f64>,
    source_available_end_s: Option<f64>,
    handle_in_s: Option<f64>,
    handle_out_s: Option<f64>,
}

fn clip_source_fields(clip: &awidat_proto::otio::Clip) -> TurnoverClipSourceFields {
    let (asset, source_available_start_s, source_available_end_s) = match &clip.media_reference {
        MediaReference::External(ext) => {
            let available_start = ext
                .available_range
                .as_ref()
                .map(|range| range.start_time.to_seconds());
            let available_end = ext
                .available_range
                .as_ref()
                .map(|range| range.start_time.to_seconds() + range.duration.to_seconds());
            (Some(ext.target_url.clone()), available_start, available_end)
        }
        MediaReference::Missing(_) => (None, None, None),
    };
    let source_start_s = clip
        .source_range
        .as_ref()
        .map(|range| range.start_time.to_seconds());
    let source_end_s = clip
        .source_range
        .as_ref()
        .map(|range| range.start_time.to_seconds() + range.duration.to_seconds());
    let handle_in_s = match (source_available_start_s, source_start_s) {
        (Some(available_start), Some(source_start)) => {
            Some((source_start - available_start).max(0.0))
        }
        _ => None,
    };
    let handle_out_s = match (source_available_end_s, source_end_s) {
        (Some(available_end), Some(source_end)) => Some((available_end - source_end).max(0.0)),
        _ => None,
    };
    TurnoverClipSourceFields {
        asset,
        source_start_s,
        source_end_s,
        source_available_start_s,
        source_available_end_s,
        handle_in_s,
        handle_out_s,
    }
}

fn clip_uuid_or_name(clip: &awidat_proto::otio::Clip) -> String {
    clip.metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.extra.get("clip_uuid"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(clip.name.as_str())
        .to_string()
}

fn track_kind_label(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
    }
}

fn format_turnover_cut_list_csv(clips: &[TurnoverClip]) -> String {
    let mut out = String::from(
        "clip_id,clip_name,track,track_kind,asset,timeline_start_s,timeline_end_s,source_start_s,source_end_s,source_available_start_s,source_available_end_s,handle_in_s,handle_out_s,duration_s,active,effects,link_group_id,audio_lead_s,audio_trail_s,markers\n",
    );
    for clip in clips {
        let row = [
            clip.clip_id.clone(),
            clip.clip_name.clone(),
            clip.track.clone(),
            clip.track_kind.clone(),
            clip.asset.clone().unwrap_or_default(),
            fmt_seconds(clip.timeline_start_s),
            fmt_seconds(clip.timeline_end_s),
            clip.source_start_s.map(fmt_seconds).unwrap_or_default(),
            clip.source_end_s.map(fmt_seconds).unwrap_or_default(),
            clip.source_available_start_s
                .map(fmt_seconds)
                .unwrap_or_default(),
            clip.source_available_end_s
                .map(fmt_seconds)
                .unwrap_or_default(),
            clip.handle_in_s.map(fmt_seconds).unwrap_or_default(),
            clip.handle_out_s.map(fmt_seconds).unwrap_or_default(),
            fmt_seconds(clip.duration_s),
            clip.active.to_string(),
            clip.effects.join(";"),
            clip.link_group_id.clone().unwrap_or_default(),
            clip.audio_lead_s.map(fmt_seconds).unwrap_or_default(),
            clip.audio_trail_s.map(fmt_seconds).unwrap_or_default(),
            format_turnover_markers(&clip.markers),
        ];
        out.push_str(
            &row.iter()
                .map(|field| csv_escape(field))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

fn format_turnover_cmx3600_edl(project_name: &str, clips: &[TurnoverClip]) -> String {
    let mut out = format!("TITLE: {}\nFCM: NON-DROP FRAME\n\n", edl_text(project_name));
    for (event_number, clip) in (1_u32..).zip(clips.iter().filter(|clip| {
        clip.active
            && clip.track_kind == "video"
            && clip.asset.is_some()
            && clip.source_start_s.is_some()
            && clip.source_end_s.is_some()
    })) {
        let source_in = clip.source_start_s.unwrap_or_default();
        let source_out = clip.source_end_s.unwrap_or(source_in + clip.duration_s);
        out.push_str(&format!(
            "{event_number:03}  {:<8} V     C        {} {} {} {}\n",
            edl_reel_name(clip.asset.as_deref().unwrap_or_default()),
            edl_timecode(source_in),
            edl_timecode(source_out),
            edl_timecode(clip.timeline_start_s),
            edl_timecode(clip.timeline_end_s)
        ));
        out.push_str(&format!(
            "* FROM CLIP NAME: {}\n",
            edl_text(&clip.clip_name)
        ));
        if let Some(asset) = &clip.asset {
            out.push_str(&format!("* SOURCE FILE: {}\n", edl_text(asset)));
        }
        if !clip.effects.is_empty() {
            out.push_str(&format!(
                "* EFFECTS: {}\n",
                edl_text(&clip.effects.join(";"))
            ));
        }
        out.push('\n');
    }
    out
}

fn edl_reel_name(asset: &str) -> String {
    let stem = Path::new(asset)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("AX");
    let mut reel = stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .take(8)
        .collect::<String>();
    if reel.is_empty() {
        reel = "AX".to_string();
    }
    reel
}

fn edl_timecode(seconds: f64) -> String {
    let frame_rate = 24_u64;
    let total_frames = (seconds.max(0.0) * frame_rate as f64).round() as u64;
    let frames = total_frames % frame_rate;
    let total_seconds = total_frames / frame_rate;
    let secs = total_seconds % 60;
    let total_minutes = total_seconds / 60;
    let mins = total_minutes % 60;
    let hours = total_minutes / 60;
    format!("{hours:02}:{mins:02}:{secs:02}:{frames:02}")
}

fn edl_text(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

fn format_turnover_fcpxml(project_name: &str, clips: &[TurnoverClip]) -> String {
    let clips = clips
        .iter()
        .filter(|clip| {
            clip.active
                && clip.track_kind == "video"
                && clip.asset.is_some()
                && clip.source_start_s.is_some()
        })
        .collect::<Vec<_>>();
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<fcpxml version=\"1.10\">\n");
    out.push_str("  <resources>\n");
    for (index, clip) in clips.iter().enumerate() {
        let asset = clip.asset.as_deref().unwrap_or_default();
        out.push_str(&format!(
            "    <asset id=\"asset-{}\" name=\"{}\" src=\"file://{}\" hasVideo=\"1\" />\n",
            index + 1,
            xml_attr(asset_file_name(asset)),
            xml_attr(asset)
        ));
    }
    out.push_str("  </resources>\n");
    out.push_str(&format!(
        "  <library>\n    <event name=\"{}\">\n      <project name=\"{}\">\n        <sequence format=\"r1\" tcStart=\"0s\" tcFormat=\"NDF\">\n          <spine>\n",
        xml_attr(project_name),
        xml_attr(project_name)
    ));
    for (index, clip) in clips.iter().enumerate() {
        out.push_str(&format!(
            "            <asset-clip name=\"{}\" ref=\"asset-{}\" offset=\"{}\" start=\"{}\" duration=\"{}\" />\n",
            xml_attr(&clip.clip_name),
            index + 1,
            fcpx_time(clip.timeline_start_s),
            fcpx_time(clip.source_start_s.unwrap_or_default()),
            fcpx_time(clip.duration_s)
        ));
    }
    out.push_str(
        "          </spine>\n        </sequence>\n      </project>\n    </event>\n  </library>\n",
    );
    out.push_str("</fcpxml>\n");
    out
}

fn asset_file_name(asset: &str) -> &str {
    Path::new(asset)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(asset)
}

fn fcpx_time(seconds: f64) -> String {
    let frames = (seconds.max(0.0) * 24.0).round() as u64;
    format!("{frames}/24s")
}

fn xml_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn format_turnover_markers(markers: &[TurnoverMarker]) -> String {
    markers
        .iter()
        .map(|marker| {
            format!(
                "{}@{}+{}",
                marker.name,
                fmt_seconds(marker.start_s),
                fmt_seconds(marker.duration_s)
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn fmt_seconds(value: f64) -> String {
    format!("{value:.3}")
}

fn build_package_preflight_report(
    project: &Project,
    format: &str,
    cues: &[Cue],
    duration_s: Option<f64>,
) -> PreflightReport {
    let profile = delivery_profile_for_format(format, project);
    let input = DeliveryPreflightInput {
        aspect_ratio: delivery_aspect_ratio(project, &profile),
        duration_s,
        video_bitrate_kbps: delivery_video_bitrate_kbps(project),
        integrated_lufs: delivery_loudness_measurement(project),
        has_captions: !cues.is_empty() || has_caption_overlays(project),
        has_required_metadata: has_required_package_metadata(project),
        has_thumbnail: has_delivery_thumbnail(project),
        safe_area_violations: delivery_safe_area_violations(project),
    };
    profile.run_preflight(input)
}

fn delivery_profile_for_format(format: &str, project: &Project) -> DeliveryProfile {
    match format {
        "shorts" => {
            let mut profile = DeliveryProfile::youtube_1080p();
            profile.id = "youtube_shorts_1080x1920".into();
            profile.name = "YouTube Shorts 1080x1920".into();
            profile.platform = Some("youtube_shorts".into());
            profile.aspect_ratio = "9:16".into();
            profile.width = 1080;
            profile.height = 1920;
            profile
        }
        _ => project
            .timeline
            .metadata
            .awidat
            .as_ref()
            .and_then(|metadata| metadata.delivery_profiles.first())
            .cloned()
            .unwrap_or_else(DeliveryProfile::youtube_1080p),
    }
}

fn delivery_aspect_ratio(project: &Project, profile: &DeliveryProfile) -> String {
    output_format_metadata(project)
        .and_then(|value| {
            value
                .get("aspect_ratio")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| profile.aspect_ratio.clone())
}

fn delivery_video_bitrate_kbps(project: &Project) -> Option<u32> {
    output_format_metadata(project)?
        .get("video_bitrate_kbps")?
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
}

fn delivery_safe_area_violations(project: &Project) -> u32 {
    output_format_metadata(project)
        .and_then(|value| {
            value
                .get("safe_area_violations")
                .and_then(serde_json::Value::as_u64)
                .and_then(|count| u32::try_from(count).ok())
        })
        .unwrap_or(0)
}

fn delivery_loudness_measurement(project: &Project) -> Option<f64> {
    project
        .timeline
        .metadata
        .awidat
        .as_ref()?
        .extra
        .get("loudness_measurement")?
        .get("integrated_lufs")?
        .as_f64()
        .filter(|value| value.is_finite())
}

fn has_required_package_metadata(project: &Project) -> bool {
    let Some(package_metadata) = project
        .timeline
        .metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.extra.get("package_metadata"))
    else {
        return false;
    };
    ["title", "description"].iter().all(|key| {
        package_metadata
            .get(*key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    })
}

fn has_delivery_thumbnail(project: &Project) -> bool {
    output_format_metadata(project)
        .and_then(|value| {
            value
                .get("thumbnail_path")
                .or_else(|| value.get("thumbnail"))
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .map(str::to_string)
        })
        .is_some_and(|value| !value.is_empty())
}

fn has_caption_overlays(project: &Project) -> bool {
    project.timeline.tracks.children.iter().any(|stack_child| {
        let StackChild::Track(track) = stack_child else {
            return false;
        };
        track.children.iter().any(|child| {
            let TrackChild::Clip(clip) = child else {
                return false;
            };
            clip.effects.iter().any(|effect| {
                effect.effect_name == "awidat.title"
                    && effect
                        .metadata
                        .get("role")
                        .and_then(serde_json::Value::as_str)
                        == Some("caption")
            })
        })
    })
}

fn output_format_metadata(project: &Project) -> Option<serde_json::Value> {
    project
        .timeline
        .metadata
        .awidat
        .as_ref()?
        .extra
        .get("output_format")
        .cloned()
}

#[derive(Debug, Clone)]
struct Cue {
    start_s: f64,
    end_s: f64,
    text: String,
}

fn collect_timeline_cues(
    project_root: &Path,
    project: &Project,
) -> Result<Vec<Cue>, FunctionCallError> {
    let mut cues = Vec::new();
    for child in &project.timeline.tracks.children {
        let StackChild::Track(track) = child else {
            continue;
        };
        if !matches!(track.kind, awidat_proto::otio::TrackKind::Video) || track.name == "Titles" {
            continue;
        }
        let mut cursor_s = 0.0;
        for tchild in &track.children {
            let dur_s = child_duration_s(tchild);
            if let TrackChild::Clip(clip) = tchild {
                let MediaReference::External(ext) = &clip.media_reference else {
                    cursor_s += dur_s;
                    continue;
                };
                let Some(range) = clip.source_range.as_ref() else {
                    cursor_s += dur_s;
                    continue;
                };
                let source_start = range.start_time.to_seconds();
                let source_end = source_start + range.duration.to_seconds();
                let speed = read_effect_number(clip, "awidat.speed", "factor").unwrap_or(1.0);
                let segments = read_transcript_segments(project_root, &ext.target_url);
                for seg in segments {
                    let overlap_start = seg.start_s.max(source_start);
                    let overlap_end = seg.end_s.min(source_end);
                    if overlap_end <= overlap_start {
                        continue;
                    }
                    cues.push(Cue {
                        start_s: cursor_s + (overlap_start - source_start) / speed,
                        end_s: cursor_s + (overlap_end - source_start) / speed,
                        text: seg.text,
                    });
                }
            }
            cursor_s += dur_s;
        }
    }
    cues.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
    Ok(cues)
}

#[derive(Debug, Clone)]
struct TranscriptSegment {
    start_s: f64,
    end_s: f64,
    text: String,
}

fn read_transcript_segments(project_root: &Path, asset_id: &str) -> Vec<TranscriptSegment> {
    let asset = awidat_proto::index::AssetId::new(asset_id.to_string());
    let Ok(sidecar) = awidat_index::read_sidecar(project_root, "whisper", &asset) else {
        return Vec::new();
    };
    let Some(segments) = sidecar.pointer("/data/segments").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    segments
        .iter()
        .filter_map(|seg| {
            let start_s = seg.get("start").or_else(|| seg.get("start_s"))?.as_f64()?;
            let end_s = seg.get("end").or_else(|| seg.get("end_s"))?.as_f64()?;
            let text = seg.get("text")?.as_str()?.trim().to_string();
            (!text.is_empty() && end_s > start_s).then_some(TranscriptSegment {
                start_s,
                end_s,
                text,
            })
        })
        .collect()
}

fn read_effect_number(
    clip: &awidat_proto::otio::Clip,
    effect_name: &str,
    field: &str,
) -> Option<f64> {
    clip.effects
        .iter()
        .find(|e| e.effect_name == effect_name)
        .and_then(|e| e.metadata.get(field))
        .and_then(serde_json::Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
}

fn child_duration_s(child: &TrackChild) -> f64 {
    match child {
        TrackChild::Clip(c) => {
            c.source_range
                .as_ref()
                .map(|r| r.duration.to_seconds())
                .unwrap_or(0.0)
                / read_effect_number(c, "awidat.speed", "factor").unwrap_or(1.0)
        }
        TrackChild::Gap(g) => g.source_range.duration.to_seconds(),
        TrackChild::Transition(t) => t.in_offset.to_seconds() + t.out_offset.to_seconds(),
        TrackChild::Stack(_) => 0.0,
    }
}

fn format_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (i, cue) in cues.iter().enumerate() {
        out.push_str(&(i + 1).to_string());
        out.push('\n');
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            fmt_srt_time(cue.start_s),
            fmt_srt_time(cue.end_s),
            cue.text
        ));
    }
    out
}

fn format_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for cue in cues {
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            fmt_vtt_time(cue.start_s),
            fmt_vtt_time(cue.end_s),
            cue.text
        ));
    }
    out
}

fn fmt_srt_time(t: f64) -> String {
    fmt_time(t, ',')
}

fn fmt_vtt_time(t: f64) -> String {
    fmt_time(t, '.')
}

fn fmt_time(t: f64, sep: char) -> String {
    let t = t.max(0.0);
    let total_ms = (t * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let total_m = total_s / 60;
    let m = total_m % 60;
    let h = total_m / 60;
    format!("{h:02}:{m:02}:{s:02}{sep}{ms:03}")
}

fn collect_chapters(cues: &[Cue]) -> Vec<(f64, String)> {
    let mut chapters = vec![(0.0, "Intro".to_string())];
    let mut next = 300.0;
    for cue in cues {
        if cue.start_s >= next {
            chapters.push((cue.start_s, chapter_title(&cue.text)));
            next += 300.0;
        }
    }
    chapters
}

fn chapter_title(text: &str) -> String {
    let mut title = text
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        title = "Chapter".into();
    }
    title
}

fn format_chapters(chapters: &[(f64, String)]) -> String {
    chapters
        .iter()
        .map(|(t, title)| format!("{} {title}", fmt_chapter_time(*t)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn fmt_chapter_time(t: f64) -> String {
    let total_s = t.max(0.0).floor() as u64;
    format!("{:02}:{:02}", total_s / 60, total_s % 60)
}

fn thumbnail_candidates(cues: &[Cue]) -> serde_json::Value {
    let mut candidates = Vec::new();
    for cue in cues.iter().filter(|c| c.text.len() > 40).take(5) {
        candidates.push(serde_json::json!({
            "time_s": cue.start_s,
            "reason": "dense spoken moment",
            "text": cue.text,
        }));
    }
    serde_json::json!({ "candidates": candidates })
}

fn io_err(label: &'static str) -> impl Fn(std::io::Error) -> FunctionCallError {
    move |e| FunctionCallError::RespondToModel(format!("export_package: {label} failed: {e}"))
}

fn render_err(e: awidat_render::RenderTimelineError) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!("export_package: timeline render plan failed: {e}"))
}

const DESCRIPTION: &str = "\
Export a delivery package under renders/package/: final MP4 render job, \
timeline-relative SRT, VTT, chapter text, package metadata JSON, \
thumbnail candidate JSON, or a turnover package for sound/color/VFX \
review. Burned-in Insert Caption overlays remain part of rendered \
exports; SRT/VTT are separate delivery artifacts.";

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::awidat_meta::{AwidatClipMetadata, SplitEditSpec};
    use awidat_proto::otio::{
        Clip, Effect, ExternalReference, Gap, MediaReference, RationalTime, StackChild, TimeRange,
        Timeline, Track,
    };
    use awidat_proto::professional::{FindingSeverity, PreflightCheckKind};
    use awidat_proto::project::EditPlan;

    fn time_range(start_s: f64, duration_s: f64) -> TimeRange {
        TimeRange::new(
            RationalTime::new(start_s * 24.0, 24.0),
            RationalTime::new(duration_s * 24.0, 24.0),
        )
    }

    fn project_with_timeline(timeline: Timeline) -> Project {
        Project {
            root: PathBuf::new(),
            timeline,
            edit_plan: EditPlan::empty(),
            manifest: None,
            warnings: Vec::new(),
        }
    }

    fn clip(name: &str, asset: &str, start_s: f64, duration_s: f64, uuid: &str) -> Clip {
        let mut clip = Clip::empty(name);
        clip.source_range = Some(time_range(start_s, duration_s));
        clip.media_reference = MediaReference::External(ExternalReference::new(asset));
        let mut awidat = AwidatClipMetadata::default();
        awidat
            .extra
            .insert("clip_uuid".into(), serde_json::json!(uuid));
        clip.metadata.awidat = Some(awidat);
        clip
    }

    #[test]
    fn turnover_clips_preserve_timeline_and_source_ranges() {
        let mut timeline = Timeline::empty("turnover-test");
        let mut v1 = Track::empty("V1", TrackKind::Video);
        v1.children
            .push(TrackChild::Gap(Gap::of_duration(2.0, 24.0)));
        let mut video = clip("hero", "raw/cam-a.mov", 10.0, 5.0, "c-hero");
        if let MediaReference::External(reference) = &mut video.media_reference {
            reference.available_range = Some(TimeRange::new(
                RationalTime::new(8.0 * 24.0, 24.0),
                RationalTime::new(12.0 * 24.0, 24.0),
            ));
        }
        video.effects.push(Effect::new("awidat.lut"));
        v1.children.push(TrackChild::Clip(video));
        timeline.tracks.children.push(StackChild::Track(v1));

        let project = project_with_timeline(timeline);
        let clips = collect_turnover_clips(&project);

        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].clip_id, "c-hero");
        assert_eq!(clips[0].timeline_start_s, 2.0);
        assert_eq!(clips[0].timeline_end_s, 7.0);
        assert_eq!(clips[0].source_start_s, Some(10.0));
        assert_eq!(clips[0].source_end_s, Some(15.0));
        assert_eq!(clips[0].source_available_start_s, Some(8.0));
        assert_eq!(clips[0].source_available_end_s, Some(20.0));
        assert_eq!(clips[0].handle_in_s, Some(2.0));
        assert_eq!(clips[0].handle_out_s, Some(5.0));
        assert_eq!(clips[0].effects, vec!["awidat.lut"]);
    }

    #[tokio::test]
    async fn turnover_package_writes_interchange_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let package_dir = dir.path().join("renders/package/turnover-test");
        let mut timeline = Timeline::empty("turnover edl test");
        let mut v1 = Track::empty("V1", TrackKind::Video);
        v1.children
            .push(TrackChild::Gap(Gap::of_duration(2.0, 24.0)));
        v1.children.push(TrackChild::Clip(clip(
            "hero",
            "raw/cam-a.mov",
            10.0,
            5.0,
            "c-hero",
        )));
        timeline.tracks.children.push(StackChild::Track(v1));
        let project = project_with_timeline(timeline);

        let summary = write_turnover_package(dir.path(), &project, &package_dir)
            .await
            .unwrap();
        let edl_rel = summary
            .artifacts
            .get("cmx3600_edl")
            .and_then(serde_json::Value::as_str)
            .expect("cmx3600_edl artifact");
        let edl = std::fs::read_to_string(dir.path().join(edl_rel)).unwrap();

        assert!(edl.contains("TITLE: turnover edl test"));
        assert!(edl.contains("001  CAM_A"));
        assert!(edl.contains("V     C"));
        assert!(edl.contains("00:00:10:00 00:00:15:00 00:00:02:00 00:00:07:00"));
        assert!(edl.contains("* FROM CLIP NAME: hero"));
        assert!(edl.contains("* SOURCE FILE: raw/cam-a.mov"));

        let fcpxml_rel = summary
            .artifacts
            .get("fcpxml")
            .and_then(serde_json::Value::as_str)
            .expect("fcpxml artifact");
        let fcpxml = std::fs::read_to_string(dir.path().join(fcpxml_rel)).unwrap();

        assert!(fcpxml.contains("<fcpxml version=\"1.10\">"));
        assert!(
            fcpxml
                .contains("<asset id=\"asset-1\" name=\"cam-a.mov\" src=\"file://raw/cam-a.mov\"")
        );
        assert!(fcpxml.contains("<asset-clip name=\"hero\" ref=\"asset-1\" offset=\"48/24s\" start=\"240/24s\" duration=\"120/24s\""));
    }

    #[test]
    fn department_turnovers_route_sound_color_and_vfx_views() {
        let mut timeline = Timeline::empty("department-test");
        let mut v1 = Track::empty("V1", TrackKind::Video);
        v1.children.push(TrackChild::Clip(clip(
            "program",
            "raw/program.mov",
            0.0,
            4.0,
            "c-program",
        )));
        let mut v2 = Track::empty("V2", TrackKind::Video);
        v2.children.push(TrackChild::Clip(clip(
            "screen replacement",
            "raw/plate.mov",
            2.0,
            3.0,
            "c-vfx",
        )));
        let mut a1 = Track::empty("A1", TrackKind::Audio);
        let mut audio = clip("dialogue", "raw/dialogue.wav", 0.0, 4.0, "c-audio");
        let awidat = audio
            .metadata
            .awidat
            .get_or_insert_with(AwidatClipMetadata::default);
        awidat.split_edit = Some(SplitEditSpec {
            audio_trail_s: Some(0.5),
            ..SplitEditSpec::default()
        });
        a1.children.push(TrackChild::Clip(audio));
        timeline.tracks.children.push(StackChild::Track(v1));
        timeline.tracks.children.push(StackChild::Track(v2));
        timeline.tracks.children.push(StackChild::Track(a1));

        let project = project_with_timeline(timeline);
        let manifest = TurnoverManifest {
            schema_version: "awidat.turnover.v1".into(),
            package_kind: "department_turnover".into(),
            created_at: "now".into(),
            project_name: project.timeline.name.clone(),
            departments: vec!["sound".into(), "color".into(), "vfx".into()],
            source_assets: Vec::new(),
            media_files: Vec::new(),
            clips: collect_turnover_clips(&project),
            transitions: Vec::new(),
        };

        let sound = department_turnover(&project, "now", "sound", &manifest);
        let color = department_turnover(&project, "now", "color", &manifest);
        let vfx = department_turnover(&project, "now", "vfx", &manifest);

        assert_eq!(sound.clips.len(), 1);
        assert_eq!(sound.clips[0].clip_id, "c-audio");
        assert_eq!(color.clips.len(), 2);
        assert_eq!(vfx.clips.len(), 1);
        assert_eq!(vfx.clips[0].clip_id, "c-vfx");
    }

    #[test]
    fn turnover_csv_escapes_human_fields() {
        let csv = format_turnover_cut_list_csv(&[TurnoverClip {
            clip_id: "c-1".into(),
            clip_name: "clip, \"quoted\"".into(),
            track: "V1".into(),
            track_kind: "video".into(),
            asset: Some("raw/a.mov".into()),
            timeline_start_s: 0.0,
            timeline_end_s: 1.0,
            source_start_s: Some(10.0),
            source_end_s: Some(11.0),
            source_available_start_s: Some(8.0),
            source_available_end_s: Some(20.0),
            handle_in_s: Some(2.0),
            handle_out_s: Some(9.0),
            duration_s: 1.0,
            active: true,
            effects: vec!["awidat.lut".into()],
            markers: vec![TurnoverMarker {
                name: "note, \"sync\"".into(),
                start_s: 0.25,
                duration_s: 0.5,
            }],
            link_group_id: Some("link-1".into()),
            audio_lead_s: Some(0.125),
            audio_trail_s: Some(0.5),
        }]);

        assert!(csv.contains("\"clip, \"\"quoted\"\"\""));
        assert!(csv.contains("0.000,1.000,10.000,11.000,8.000,20.000,2.000,9.000,1.000"));
        assert!(csv.contains("duration_s,active,effects,link_group_id"));
        assert!(csv.contains("1.000,true,awidat.lut,link-1"));
        assert!(csv.contains("link_group_id,audio_lead_s,audio_trail_s,markers"));
        assert!(csv.contains("link-1,0.125,0.500"));
        assert!(csv.contains("\"note, \"\"sync\"\"@0.250+0.500\""));
    }

    #[test]
    fn youtube_package_preflight_flags_missing_delivery_requirements() {
        let project = project_with_timeline(Timeline::empty("delivery-qc-test"));
        let report = build_package_preflight_report(&project, "youtube", &[], Some(30.0));

        assert_eq!(report.profile_id, "youtube_1080p");
        assert!(report.findings.iter().any(|finding| {
            finding.check == PreflightCheckKind::Captions
                && finding.severity == FindingSeverity::Warning
        }));
        assert!(report.findings.iter().any(|finding| {
            finding.check == PreflightCheckKind::Metadata
                && finding.severity == FindingSeverity::Warning
        }));
        assert!(report.findings.iter().any(|finding| {
            finding.check == PreflightCheckKind::Loudness
                && finding.severity == FindingSeverity::Error
        }));
    }

    #[test]
    fn package_preflight_flags_bitrate_above_delivery_profile_target() {
        let mut timeline = Timeline::empty("delivery-bitrate-test");
        let awidat = timeline.metadata.awidat.as_mut().unwrap();
        let mut profile = DeliveryProfile::youtube_1080p();
        profile.preflight_checks = vec![PreflightCheckKind::Bitrate];
        awidat.delivery_profiles = vec![profile];
        awidat.extra.insert(
            "output_format".into(),
            serde_json::json!({
                "aspect_ratio": "16:9",
                "video_bitrate_kbps": 25_000
            }),
        );
        let project = project_with_timeline(timeline);

        let report = build_package_preflight_report(&project, "youtube", &[], Some(30.0));

        assert!(report.findings.iter().any(|finding| {
            finding.check == PreflightCheckKind::Bitrate
                && finding.severity == FindingSeverity::Warning
                && finding.message.contains("above target")
        }));
    }

    #[test]
    fn package_preflight_flags_safe_area_violations_from_output_metadata() {
        let mut timeline = Timeline::empty("delivery-safe-area-test");
        let awidat = timeline.metadata.awidat.as_mut().unwrap();
        let mut profile = DeliveryProfile::youtube_1080p();
        profile.preflight_checks = vec![PreflightCheckKind::SafeAreas];
        awidat.delivery_profiles = vec![profile];
        awidat.extra.insert(
            "output_format".into(),
            serde_json::json!({
                "aspect_ratio": "16:9",
                "safe_area_violations": 3
            }),
        );
        let project = project_with_timeline(timeline);

        let report = build_package_preflight_report(&project, "youtube", &[], Some(30.0));

        assert!(report.findings.iter().any(|finding| {
            finding.check == PreflightCheckKind::SafeAreas
                && finding.severity == FindingSeverity::Warning
                && finding.message.contains("3 safe-area violations")
        }));
    }

    #[test]
    fn package_preflight_accepts_thumbnail_from_output_metadata() {
        let mut timeline = Timeline::empty("delivery-thumbnail-test");
        let awidat = timeline.metadata.awidat.as_mut().unwrap();
        let mut profile = DeliveryProfile::youtube_1080p();
        profile.preflight_checks = vec![PreflightCheckKind::Thumbnail];
        awidat.delivery_profiles = vec![profile];
        awidat.extra.insert(
            "output_format".into(),
            serde_json::json!({
                "aspect_ratio": "16:9",
                "thumbnail_path": "thumbs/main.jpg"
            }),
        );
        let project = project_with_timeline(timeline);

        let report = build_package_preflight_report(&project, "youtube", &[], Some(30.0));

        assert!(
            !report
                .findings
                .iter()
                .any(|finding| finding.check == PreflightCheckKind::Thumbnail)
        );
    }

    #[test]
    fn turnover_media_files_report_presence_size_and_checksum() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("raw")).unwrap();
        std::fs::write(dir.path().join("raw/present.mov"), b"hello world").unwrap();
        let proxy_path =
            test_proxy_path_for(dir.path(), &dir.path().join("raw").join("present.mov"));
        std::fs::create_dir_all(proxy_path.parent().unwrap()).unwrap();
        std::fs::write(&proxy_path, b"proxy").unwrap();

        let files = collect_turnover_media_files(
            dir.path(),
            &["raw/present.mov".into(), "raw/missing.mov".into()],
        );

        assert_eq!(files.len(), 2);
        let present = files
            .iter()
            .find(|file| file.asset == "raw/present.mov")
            .expect("present file");
        assert_eq!(present.status, "online");
        assert_eq!(present.size_bytes, Some(11));
        assert_eq!(
            present.sha256.as_deref(),
            Some("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9")
        );
        let present_json = serde_json::to_value(present).unwrap();
        assert_eq!(present_json["proxy_status"], "online");
        assert_eq!(
            present_json["proxy_path"],
            proxy_path.to_string_lossy().to_string()
        );

        let missing = files
            .iter()
            .find(|file| file.asset == "raw/missing.mov")
            .expect("missing file");
        assert_eq!(missing.status, "missing");
        assert_eq!(missing.size_bytes, None);
        assert_eq!(missing.sha256, None);
        let missing_json = serde_json::to_value(missing).unwrap();
        assert_eq!(missing_json["proxy_status"], "blocked");
        assert!(missing_json.get("proxy_path").is_none());
    }

    fn test_proxy_path_for(
        project_root: &std::path::Path,
        asset_path: &std::path::Path,
    ) -> PathBuf {
        let stem = asset_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("asset");
        project_root.join(".awidat").join("proxies").join(format!(
            "{stem}-{:08x}.mp4",
            test_stable_path_hash(asset_path)
        ))
    }

    fn test_stable_path_hash(path: &std::path::Path) -> u32 {
        let mut hash = 0x811c9dc5_u32;
        for byte in path.to_string_lossy().as_bytes() {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(0x01000193);
        }
        hash
    }
}
