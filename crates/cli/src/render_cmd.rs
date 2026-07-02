//! `montage render` — synchronous timeline export for non-interactive runs.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use montage_render::RenderJobSpec;

#[derive(Debug, Clone, Copy)]
pub struct RenderArgs {
    pub duration_s: Option<f64>,
}

const STAGED_EXACT_CHUNK_SIZE: usize = 32;

/// Derive a short id from the output mp4 path used to name ffmpeg log
/// sidecars (`<id>.ffmpeg.stdout.log` / `<id>.ffmpeg.stderr.log`).
fn manifest_id(output_path: &Path) -> String {
    output_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(std::string::ToString::to_string)
        .unwrap_or_else(|| "timeline".to_string())
}

pub fn run(project_root: &Path, args: RenderArgs) -> Result<()> {
    let mut spec = build_render_spec(project_root, args.duration_s)?;
    let ffmpeg = montage_render::ffmpeg_path().context("failed to locate ffmpeg")?;
    println!(
        "Rendering timeline ({:.2}s) → {}",
        spec.total_duration_s.unwrap_or_default(),
        spec.output_path.display()
    );
    for limitation in &spec.limitations {
        println!("  ! {}: {}", limitation.kind, limitation.message);
    }
    let staged_plan = if should_use_staged_exact_render(&spec) {
        spec.metadata
            .insert("render_driver".into(), "cli_render_staged_exact".into());
        Some(plan_staged_exact_render(project_root, &spec)?)
    } else {
        None
    };
    let manifest_path = write_cli_render_manifest(project_root, &spec)?;

    if let Some(plan) = staged_plan {
        run_staged_exact_render(&ffmpeg, &spec, &plan)?;
    } else {
        run_ffmpeg_with_logs(&ffmpeg, &spec.args, spec.cwd.as_deref(), &spec.output_path)?;
    }
    montage_render::finalize_render_manifest_file(&manifest_path)
        .with_context(|| format!("finalize render manifest {}", manifest_path.display()))?;
    println!("Render complete: {}", spec.output_path.display());
    println!("Render manifest: {}", manifest_path.display());
    Ok(())
}

fn run_ffmpeg_with_logs(
    ffmpeg: &Path,
    args: &[String],
    cwd: Option<&Path>,
    output_path: &Path,
) -> Result<()> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    // Capture stdout/stderr to per-render log files. Two prior failure
    // modes ruled out:
    //   * Command::output(): collected both pipes into memory; the
    //     ~16 KB kernel pipe buffer filled (ffmpeg info-level emits
    //     concat warnings + per-frame progress), ffmpeg blocked on
    //     write, montage blocked on read = deadlock at 0 % CPU. Hit
    //     this with PID 39904.
    //   * Stdio::inherit(): inherits the montage parent's terminal;
    //     when montage runs in the background, ffmpeg writes to a tty
    //     it doesn't own → SIGTTOU → process STOPPED. Hit this with
    //     PID 46066 (STAT=T even after SIGCONT).
    // Logs land next to the output mp4 so the user (or a postmortem
    // tool) can read what ffmpeg did. Use separate threads to drain
    // each pipe so neither blocks the other.
    let log_dir = output_path.parent().map(std::path::Path::to_path_buf);
    let stdout_path = log_dir
        .as_ref()
        .map(|d| d.join(format!("{}.ffmpeg.stdout.log", manifest_id(output_path))));
    let stderr_path = log_dir
        .as_ref()
        .map(|d| d.join(format!("{}.ffmpeg.stderr.log", manifest_id(output_path))));
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().context("failed to spawn ffmpeg")?;
    let stdout_thread = child.stdout.take().and_then(|mut pipe| {
        stdout_path.as_ref().map(|path| {
            let path = path.clone();
            std::thread::spawn(move || {
                if let Ok(mut file) = std::fs::File::create(&path) {
                    let _ = std::io::copy(&mut pipe, &mut file);
                }
            })
        })
    });
    let stderr_thread = child.stderr.take().and_then(|mut pipe| {
        stderr_path.as_ref().map(|path| {
            let path = path.clone();
            std::thread::spawn(move || {
                if let Ok(mut file) = std::fs::File::create(&path) {
                    let _ = std::io::copy(&mut pipe, &mut file);
                }
            })
        })
    });
    let status = child.wait().context("ffmpeg wait failed")?;
    if let Some(handle) = stdout_thread {
        let _ = handle.join();
    }
    if let Some(handle) = stderr_thread {
        let _ = handle.join();
    }
    if !status.success() {
        let stderr_tail = stderr_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| {
                s.lines()
                    .rev()
                    .take(40)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        bail!("ffmpeg render failed with status {status}\nlast stderr:\n{stderr_tail}");
    }
    Ok(())
}

fn should_use_staged_exact_render(spec: &RenderJobSpec) -> bool {
    spec.metadata
        .get("timeline_backend_reason")
        .is_some_and(|reason| reason == "single_asset_concat_demuxer")
}

struct StagedExactRenderPlan {
    chunks: Vec<StagedExactRenderChunk>,
    concat_list_path: PathBuf,
    final_args: Vec<String>,
}

struct StagedExactRenderChunk {
    segments: Vec<montage_render::TimelineSegment>,
    output_path: PathBuf,
}

fn plan_staged_exact_render(
    project_root: &Path,
    spec: &RenderJobSpec,
) -> Result<StagedExactRenderPlan> {
    let segments = montage_render::collect_timeline_segments(project_root)
        .with_context(|| format!("collect timeline segments for {}", project_root.display()))?;
    let work_dir = staged_exact_work_dir(&spec.output_path);
    let chunks = chunk_timeline_segments(&segments, STAGED_EXACT_CHUNK_SIZE)
        .into_iter()
        .enumerate()
        .map(|(index, segments)| StagedExactRenderChunk {
            segments,
            output_path: work_dir.join(format!("chunk-{index:04}.mp4")),
        })
        .collect::<Vec<_>>();
    let concat_list_path = work_dir.join("chunks.ffconcat");
    let final_args =
        build_staged_concat_argv(&concat_list_path, &spec.output_path, spec.total_duration_s);
    Ok(StagedExactRenderPlan {
        chunks,
        concat_list_path,
        final_args,
    })
}

fn staged_exact_work_dir(output_path: &Path) -> PathBuf {
    let stem = output_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("timeline");
    output_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{stem}.staged"))
}

fn chunk_timeline_segments(
    segments: &[montage_render::TimelineSegment],
    chunk_size: usize,
) -> Vec<Vec<montage_render::TimelineSegment>> {
    let chunk_size = chunk_size.max(1);
    segments
        .chunks(chunk_size)
        .map(<[montage_render::TimelineSegment]>::to_vec)
        .collect()
}

fn build_staged_concat_argv(
    concat_list_path: &Path,
    output_path: &Path,
    total_duration_s: Option<f64>,
) -> Vec<String> {
    let mut argv = vec![
        "-y".into(),
        "-loglevel".into(),
        "info".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        concat_list_path.to_string_lossy().into_owned(),
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "0:a:0?".into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "20".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
    ];
    if let Some(duration_s) = total_duration_s.filter(|duration_s| duration_s.is_finite()) {
        argv.extend(["-t".into(), format!("{duration_s:.6}")]);
    }
    argv.push(output_path.to_string_lossy().into_owned());
    argv
}

fn run_staged_exact_render(
    ffmpeg: &Path,
    spec: &RenderJobSpec,
    plan: &StagedExactRenderPlan,
) -> Result<()> {
    if plan.chunks.is_empty() {
        bail!("staged exact render has no timeline chunks");
    }
    if let Some(parent) = plan.concat_list_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create staged render dir {}", parent.display()))?;
    }
    for chunk in &plan.chunks {
        let args = montage_render::build_timeline_argv(&chunk.segments, &chunk.output_path);
        run_ffmpeg_with_logs(ffmpeg, &args, spec.cwd.as_deref(), &chunk.output_path)?;
    }
    write_staged_concat_list(&plan.concat_list_path, &plan.chunks)?;
    run_ffmpeg_with_logs(
        ffmpeg,
        &plan.final_args,
        spec.cwd.as_deref(),
        &spec.output_path,
    )?;
    if let Some(parent) = plan.concat_list_path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
    Ok(())
}

fn write_staged_concat_list(list_path: &Path, chunks: &[StagedExactRenderChunk]) -> Result<()> {
    let mut body = String::from("ffconcat version 1.0\n");
    for chunk in chunks {
        body.push_str("file ");
        body.push_str(&ffconcat_quoted_path(&chunk.output_path)?);
        body.push('\n');
    }
    std::fs::write(list_path, body)
        .with_context(|| format!("write staged concat list {}", list_path.display()))
}

fn ffconcat_quoted_path(path: &Path) -> Result<String> {
    let path = path.to_string_lossy();
    if path.contains('\n') || path.contains('\r') {
        bail!("concat path contains a newline: {path}");
    }
    Ok(format!("'{}'", path.replace('\'', "'\\''")))
}

fn build_render_spec(project_root: &Path, duration_s: Option<f64>) -> Result<RenderJobSpec> {
    match duration_s {
        Some(duration_s) => {
            montage_render::build_timeline_head_render_spec(project_root, duration_s).with_context(
                || {
                    format!(
                        "failed to plan bounded timeline render for {}",
                        project_root.display()
                    )
                },
            )
        }
        None => montage_render::build_timeline_render_spec(project_root).with_context(|| {
            format!(
                "failed to plan timeline render for {}",
                project_root.display()
            )
        }),
    }
}

fn write_cli_render_manifest(
    project_root: &Path,
    spec: &RenderJobSpec,
) -> Result<std::path::PathBuf> {
    let project_path = project_root.join("project.otio.json");
    let project_hash = if project_path.is_file() {
        Some(
            montage_render::fingerprint_file(&project_path, true)
                .with_context(|| format!("fingerprint {}", project_path.display()))?
                .sha256,
        )
    } else {
        None
    };
    let ffmpeg = montage_render::ffmpeg_path().context("failed to locate ffmpeg for manifest")?;
    let mut argv = vec![ffmpeg.to_string_lossy().into_owned()];
    argv.extend(spec.args.iter().cloned());
    let mut metadata = spec.metadata.clone();
    metadata
        .entry("render_driver".into())
        .or_insert_with(|| "cli_render".into());
    metadata.extend(montage_core::capabilities::render_feature_metadata_for_backend(&spec.backend));
    enrich_caption_metadata(project_root, &mut metadata)?;
    let sidecars = montage_render::fingerprint_ffmpeg_subtitle_sidecars(&spec.args)
        .context("fingerprint render sidecars")?;
    let manifest = montage_render::planned_at_now(montage_render::RenderExecutionManifestInput {
        created_at: String::new(),
        montage_version: env!("CARGO_PKG_VERSION").into(),
        project_root: project_root.to_string_lossy().into_owned(),
        project_hash: project_hash.clone(),
        timeline_hash: project_hash,
        backend: spec.backend.clone(),
        replay: montage_render::RenderReplayPlan::FfmpegArgv {
            argv,
            cwd: spec
                .cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().into_owned()),
        },
        inputs: montage_render::fingerprint_manifest_inputs_sampled(
            project_root,
            &spec.input_paths,
        )
        .context("fingerprint render inputs")?,
        outputs: vec![montage_render::output_artifact(&spec.output_path, true)],
        sidecars,
        limitations: spec
            .limitations
            .iter()
            .map(|limitation| {
                montage_render::limitation(limitation.kind.clone(), limitation.message.clone())
            })
            .collect(),
        verification: None,
        metadata,
    });
    let manifest_path = montage_render::manifest_path_for_output(&spec.output_path);
    montage_render::write_render_manifest(&manifest_path, &manifest)
        .with_context(|| format!("write render manifest {}", manifest_path.display()))?;
    Ok(manifest_path)
}

fn enrich_caption_metadata(
    project_root: &Path,
    metadata: &mut std::collections::BTreeMap<String, String>,
) -> Result<()> {
    let project = montage_proto::project::Project::read(project_root).with_context(|| {
        format!(
            "read project for caption metadata {}",
            project_root.display()
        )
    })?;
    let summary = montage_core::captions::summarize_captions(&project);
    metadata.extend(montage_core::captions::caption_summary_metadata(&summary));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use montage_proto::otio::{Clip, Effect, StackChild, Track, TrackChild, TrackKind};
    use montage_proto::project::Project;

    fn caption_clip(name: &str, safe_area: Option<&str>) -> Clip {
        let mut clip = Clip::empty(name);
        let mut effect = Effect::new("montage.title");
        effect
            .metadata
            .insert("role".into(), serde_json::json!("caption"));
        effect.metadata.insert(
            "word_timings".into(),
            serde_json::json!([
                {"text": "Hello", "start_s": 0.0, "end_s": 0.2},
                {"text": "world", "start_s": 0.2, "end_s": 0.5}
            ]),
        );
        if let Some(safe_area) = safe_area {
            effect
                .metadata
                .insert("safe_area".into(), serde_json::json!(safe_area));
        }
        clip.effects.push(effect);
        clip
    }

    fn init_project(dir: &tempfile::TempDir) -> Project {
        Project::init(dir.path()).unwrap()
    }

    #[test]
    fn cli_render_manifest_records_timeline_backend() {
        let dir = tempfile::tempdir().unwrap();
        init_project(&dir).write(dir.path()).unwrap();
        let output_path = dir.path().join("renders/timeline.mp4");
        let spec = RenderJobSpec {
            args: vec![
                "-i".into(),
                "raw/x.mp4".into(),
                output_path.to_string_lossy().into_owned(),
            ],
            backend: montage_render::RenderBackendKind::TimelineFfmpegReencode,
            total_duration_s: Some(1.0),
            cwd: Some(dir.path().to_path_buf()),
            output_path: output_path.clone(),
            input_paths: Vec::new(),
            manifest_path: None,
            limitations: Vec::new(),
            metadata: std::collections::BTreeMap::from([(
                "timeline_backend_reason".into(),
                "ffmpeg_with_libass_captions".into(),
            )]),
        };

        let manifest_path = write_cli_render_manifest(dir.path(), &spec).unwrap();

        assert_eq!(
            manifest_path,
            montage_render::manifest_path_for_output(&output_path)
        );
        let manifest = montage_render::read_render_manifest(&manifest_path).unwrap();
        assert_eq!(
            manifest.backend,
            montage_render::RenderBackendKind::TimelineFfmpegReencode
        );
        assert_eq!(
            manifest.metadata["timeline_backend_reason"],
            "ffmpeg_with_libass_captions"
        );
        assert_eq!(
            manifest.metadata["render_feature_id"],
            "ffmpeg_timeline_export"
        );
        assert_eq!(
            manifest.metadata["render_feature_export_supported"],
            "supported"
        );
        assert_eq!(manifest.metadata["render_driver"], "cli_render");
    }

    #[test]
    fn cli_render_manifest_records_caption_summary_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = init_project(&dir);
        let mut titles = Track::empty("Titles", TrackKind::Video);
        titles
            .children
            .push(TrackChild::Clip(caption_clip("caption-a", Some("mobile"))));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(titles));
        project.write(dir.path()).unwrap();

        let output_path = dir.path().join("renders/timeline.mp4");
        let spec = RenderJobSpec {
            args: vec![
                "-i".into(),
                "raw/x.mp4".into(),
                output_path.to_string_lossy().into_owned(),
            ],
            backend: montage_render::RenderBackendKind::TimelineFfmpegReencode,
            total_duration_s: Some(1.0),
            cwd: Some(dir.path().to_path_buf()),
            output_path,
            input_paths: Vec::new(),
            manifest_path: None,
            limitations: Vec::new(),
            metadata: std::collections::BTreeMap::from([(
                "timeline_backend_reason".into(),
                "ffmpeg_with_libass_captions".into(),
            )]),
        };

        let manifest_path = write_cli_render_manifest(dir.path(), &spec).unwrap();
        let manifest = montage_render::read_render_manifest(&manifest_path).unwrap();

        assert_eq!(manifest.metadata["caption_authority"], "caption_overlays");
        assert_eq!(manifest.metadata["caption_overlay_count"], "1");
        assert_eq!(manifest.metadata["word_timed_caption_overlay_count"], "1");
        assert_eq!(manifest.metadata["safe_area_caption_overlay_count"], "1");
        assert_eq!(
            manifest.metadata["mobile_safe_area_caption_overlay_count"],
            "1"
        );
        assert_eq!(
            manifest.metadata["missing_safe_area_caption_overlay_count"],
            "0"
        );
    }

    #[test]
    fn cli_render_manifest_records_ffmpeg_subtitle_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        init_project(&dir).write(dir.path()).unwrap();
        let ass_path = dir.path().join("renders/.ass/caption.ass");
        std::fs::create_dir_all(ass_path.parent().unwrap()).unwrap();
        std::fs::write(&ass_path, b"[Script Info]\n").unwrap();
        let output_path = dir.path().join("renders/timeline.mp4");
        let spec = RenderJobSpec {
            args: vec![
                "-filter_complex".into(),
                format!("[outv]subtitles='{}'[titled_v]", ass_path.display()),
                output_path.to_string_lossy().into_owned(),
            ],
            backend: montage_render::RenderBackendKind::TimelineFfmpegReencode,
            total_duration_s: Some(1.0),
            cwd: Some(dir.path().to_path_buf()),
            output_path,
            input_paths: Vec::new(),
            manifest_path: None,
            limitations: Vec::new(),
            metadata: std::collections::BTreeMap::new(),
        };

        let manifest_path = write_cli_render_manifest(dir.path(), &spec).unwrap();
        let manifest = montage_render::read_render_manifest(&manifest_path).unwrap();

        assert_eq!(manifest.sidecars.len(), 1);
        assert_eq!(manifest.sidecars[0].path, ass_path.to_string_lossy());
        assert!(manifest.sidecars[0].required);
    }

    #[test]
    fn single_asset_concat_demuxer_spec_uses_staged_exact_render() {
        let output_path = PathBuf::from("renders/timeline.mp4");
        let spec = RenderJobSpec {
            args: Vec::new(),
            backend: montage_render::RenderBackendKind::TimelineFfmpegReencode,
            total_duration_s: Some(1.0),
            cwd: None,
            output_path,
            input_paths: Vec::new(),
            manifest_path: None,
            limitations: Vec::new(),
            metadata: std::collections::BTreeMap::from([(
                "timeline_backend_reason".into(),
                "single_asset_concat_demuxer".into(),
            )]),
        };

        assert!(should_use_staged_exact_render(&spec));
    }

    #[test]
    fn chunk_timeline_segments_caps_exact_render_batch_size() {
        let segments = (0..70)
            .map(|index| montage_render::TimelineSegment {
                clip_name: format!("c{index}"),
                duration_s: 1.0,
                ..Default::default()
            })
            .collect::<Vec<_>>();

        let chunks = chunk_timeline_segments(&segments, 32);

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 32);
        assert_eq!(chunks[1].len(), 32);
        assert_eq!(chunks[2].len(), 6);
    }

    #[test]
    fn staged_concat_argv_reencodes_chunk_sequence() {
        let argv = build_staged_concat_argv(
            Path::new("/tmp/montage chunks/chunks.ffconcat"),
            Path::new("/tmp/out.mp4"),
            Some(4053.266),
        );
        let cmd = argv.join(" ");

        assert!(cmd.contains("-f concat"));
        assert!(cmd.contains("-safe 0"));
        assert!(cmd.contains("-map 0:v:0"));
        assert!(cmd.contains("-map 0:a:0?"));
        assert!(cmd.contains("-c:v libx264"));
        assert!(cmd.contains("-c:a aac"));
        assert!(cmd.contains("-t 4053.266000"));
        assert_eq!(argv.last().map(String::as_str), Some("/tmp/out.mp4"));
    }
}
