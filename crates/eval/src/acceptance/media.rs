use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use awidat_render::{RenderJobSpec, build_timeline_render_spec, ffmpeg_path, ffprobe_path};
use serde::Serialize;
use serde_json::json;

use super::case::{AcceptanceCase, AcceptanceTranscriptExpect, SyntheticMedia, TranscriptSegment};

pub(crate) fn write_synthetic_media(output_path: &Path, media: &SyntheticMedia) -> Result<()> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let ffmpeg = ffmpeg_path().context("locate ffmpeg")?;
    let total_duration_s = media.total_duration_s();
    let mut cmd = Command::new(ffmpeg);
    cmd.arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg(format!(
            "testsrc=size={}x{}:rate={}:duration={total_duration_s}",
            media.width, media.height, media.fps
        ));

    for segment in &media.segments {
        cmd.arg("-f").arg("lavfi").arg("-i");
        match segment.kind.as_str() {
            "tone" => {
                let frequency = segment.frequency_hz.unwrap_or(440);
                cmd.arg(format!(
                    "sine=frequency={frequency}:sample_rate=44100:duration={}",
                    segment.duration_s
                ));
            }
            "silence" => {
                cmd.arg(format!("anullsrc=r=44100:cl=mono:d={}", segment.duration_s));
            }
            other => bail!("unsupported synthetic media segment kind {other:?}"),
        };
    }

    let inputs = (0..media.segments.len())
        .map(|idx| format!("[{}:a]", idx + 1))
        .collect::<String>();
    cmd.arg("-filter_complex").arg(format!(
        "{inputs}concat=n={}:v=0:a=1[a]",
        media.segments.len()
    ));
    cmd.arg("-map")
        .arg("0:v")
        .arg("-map")
        .arg("[a]")
        .arg("-shortest")
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("ultrafast")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-c:a")
        .arg("aac")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output_path);

    run_command(cmd, "synthesize acceptance media")?;
    Ok(())
}

pub(crate) fn write_real_media_excerpt(
    output_path: &Path,
    source_path: &Path,
    case: &AcceptanceCase,
) -> Result<()> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if !source_path.is_file() {
        bail!(
            "real acceptance source video not found: {}",
            source_path.display()
        );
    }

    let ffmpeg = ffmpeg_path().context("locate ffmpeg")?;
    let mut cmd = Command::new(ffmpeg);
    cmd.arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-ss")
        .arg(format!("{}", case.project.source_start_s.unwrap_or(0.0)))
        .arg("-i")
        .arg(source_path)
        .arg("-t")
        .arg(format!("{}", case.project.source_duration_s))
        .arg("-map")
        .arg("0:v:0")
        .arg("-map")
        .arg("0:a:0?")
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("ultrafast")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-c:a")
        .arg("aac")
        .arg("-movflags")
        .arg("+faststart")
        .arg(output_path);

    run_command(cmd, "extract real acceptance media excerpt")?;
    Ok(())
}

pub(crate) fn write_transcript_sidecar(
    project_root: &Path,
    asset: &str,
    transcript: &AcceptanceTranscriptExpect,
) -> Result<PathBuf> {
    let path = project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset}.json"));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let payload = json!({
        "indexer": "whisper",
        "asset_id": asset,
        "data": {
            "segments": transcript.segments.iter().map(segment_json).collect::<Vec<_>>(),
            "words": transcript.segments.iter().flat_map(words_json).collect::<Vec<_>>()
        }
    });
    fs::write(&path, serde_json::to_vec_pretty(&payload)?)?;
    Ok(path)
}

fn segment_json(segment: &TranscriptSegment) -> serde_json::Value {
    json!({
        "text": segment.text,
        "start_s": segment.start_s,
        "end_s": segment.end_s
    })
}

fn words_json(segment: &TranscriptSegment) -> Vec<serde_json::Value> {
    let words = segment.text.split_whitespace().collect::<Vec<_>>();
    if words.is_empty() {
        return Vec::new();
    }
    let duration_s = segment.end_s - segment.start_s;
    let word_count = words.len();
    let word_duration_s = duration_s / word_count as f64;
    words
        .into_iter()
        .enumerate()
        .map(|(idx, text)| {
            let start_s = segment.start_s + word_duration_s * idx as f64;
            let end_s = if idx + 1 == word_count {
                segment.end_s
            } else {
                start_s + word_duration_s
            };
            json!({
                "text": text,
                "start_s": start_s,
                "end_s": end_s
            })
        })
        .collect()
}
pub(crate) fn render_project(project_root: &Path) -> Result<RenderOutput> {
    if let Ok(cli) = std::env::var("AWIDAT_ACCEPTANCE_CLI")
        && !cli.trim().is_empty()
    {
        let mut cmd = Command::new(cli);
        cmd.arg("render").arg(project_root);
        run_command(cmd, "run external Awidat CLI render")?;
        let output_path = newest_render(project_root)
            .with_context(|| format!("find rendered MP4 under {}", project_root.display()))?;
        return Ok(RenderOutput {
            output_path,
            driver: "external_cli".into(),
        });
    }

    let spec = build_timeline_render_spec(project_root).context("plan timeline render")?;
    run_render_spec(&spec)?;
    Ok(RenderOutput {
        output_path: spec.output_path,
        driver: "shared_render_spec".into(),
    })
}

fn run_render_spec(spec: &RenderJobSpec) -> Result<()> {
    let ffmpeg = ffmpeg_path().context("locate ffmpeg")?;
    let mut cmd = Command::new(ffmpeg);
    cmd.args(&spec.args);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    run_command(cmd, "render acceptance timeline")
}

fn newest_render(project_root: &Path) -> Result<PathBuf> {
    let renders_dir = project_root.join("renders");
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&renders_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if extension != "mp4" {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified = metadata.modified()?;
        candidates.push((modified, path));
    }
    candidates.sort_by_key(|(modified, _)| *modified);
    candidates
        .pop()
        .map(|(_, path)| path)
        .context("no rendered MP4 found")
}

pub(crate) fn write_ffprobe_json(
    output_path: &Path,
    artifact_path: &Path,
) -> Result<serde_json::Value> {
    let ffprobe = ffprobe_path().context("locate ffprobe")?;
    let mut cmd = Command::new(ffprobe);
    cmd.arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration:stream=codec_type,width,height,r_frame_rate")
        .arg("-of")
        .arg("json")
        .arg(output_path);
    let output = run_command_output(cmd, "write ffprobe JSON")?;
    fs::write(artifact_path, &output.stdout)?;
    serde_json::from_slice(&output.stdout).context("parse ffprobe JSON artifact")
}
pub(crate) fn detect_black_segments(
    output_path: &Path,
    min_duration_s: f64,
    max_black_segment_s: f64,
) -> Result<Vec<BlackSegment>> {
    let ffmpeg = ffmpeg_path().context("locate ffmpeg")?;
    let mut cmd = Command::new(ffmpeg);
    cmd.arg("-hide_banner")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("info")
        .arg("-i")
        .arg(output_path)
        .arg("-vf")
        .arg(format!("blackdetect=d={min_duration_s}:pix_th=0.10"))
        .arg("-an")
        .arg("-f")
        .arg("null")
        .arg("-");
    let output = run_command_output(cmd, "run blackdetect")?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(parse_black_segments(&stderr)
        .into_iter()
        .filter(|segment| segment.duration_s > max_black_segment_s)
        .collect())
}

pub(crate) fn parse_black_segments(stderr: &str) -> Vec<BlackSegment> {
    stderr
        .lines()
        .filter(|line| line.contains("black_start:"))
        .filter_map(parse_black_segment_line)
        .collect()
}

fn parse_black_segment_line(line: &str) -> Option<BlackSegment> {
    let mut start_s = None;
    let mut end_s = None;
    let mut duration_s = None;
    for token in line.split_whitespace() {
        if let Some(value) = token.strip_prefix("black_start:") {
            start_s = value.parse::<f64>().ok();
        } else if let Some(value) = token.strip_prefix("black_end:") {
            end_s = value.parse::<f64>().ok();
        } else if let Some(value) = token.strip_prefix("black_duration:") {
            duration_s = value.parse::<f64>().ok();
        }
    }
    let start_s = start_s?;
    let end_s = end_s?;
    Some(BlackSegment {
        start_s,
        end_s,
        duration_s: duration_s.unwrap_or(end_s - start_s),
    })
}

pub(crate) fn silences_to_json(ranges: &[awidat_render::SilenceRange]) -> Vec<serde_json::Value> {
    ranges
        .iter()
        .map(|range| {
            json!({
                "start_s": range.start_s,
                "end_s": range.end_s,
                "duration_s": range.end_s - range.start_s,
                "db_floor": range.db_floor,
            })
        })
        .collect()
}
fn run_command(cmd: Command, label: &str) -> Result<()> {
    let _ = run_command_output(cmd, label)?;
    Ok(())
}

fn run_command_output(mut cmd: Command, label: &str) -> Result<std::process::Output> {
    let output = cmd
        .output()
        .with_context(|| format!("{label}: failed to spawn process"))?;
    if !output.status.success() {
        bail!(
            "{label} failed with status {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output)
}
#[derive(Debug)]
pub(crate) struct RenderOutput {
    pub(crate) output_path: PathBuf,
    pub(crate) driver: String,
}
#[derive(Debug, Serialize)]
pub(crate) struct BlackSegment {
    pub(crate) start_s: f64,
    pub(crate) end_s: f64,
    pub(crate) duration_s: f64,
}
