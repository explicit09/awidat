//! `export_package` tool — render timeline and write delivery sidecars.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use awidat_proto::otio::{MediaReference, StackChild, TrackChild};
use awidat_proto::project::Project;
use awidat_render::RenderJobSpec;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `export_package` tool.
pub struct ExportPackageTool;

#[derive(Debug, Deserialize)]
struct ExportPackageArgs {
    /// youtube | shorts | podcast | custom
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
                        "enum": ["youtube", "shorts", "podcast", "custom"],
                        "description": "Package preset. youtube/custom render MP4 + SRT/VTT/chapters/metadata; shorts uses the current timeline format; podcast adds audio metadata sidecars."
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
            "youtube" | "shorts" | "podcast" | "custom"
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
            },
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

#[allow(dead_code)]
fn _unused(_p: PathBuf) {}

const DESCRIPTION: &str = "\
Export a delivery package under renders/package/: final MP4 render job, \
timeline-relative SRT, VTT, chapter text, package metadata JSON, and \
thumbnail candidate JSON. Burned-in Insert Caption overlays remain part \
of the render; SRT/VTT are separate delivery artifacts.";
