//! Candidate discovery for real-video behavioral acceptance fixtures.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use awidat_render::{ffmpeg_path, ffprobe_path};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::discover_transcript::{TranscriptDiscoveryCandidate, discover_transcript_candidates};

pub(crate) use super::discover_draft::build_fixture_draft;
pub use super::discover_draft::{FixtureDraftWriteReport, write_fixture_drafts};
pub use super::discover_report::format_discovery_report;

/// Bounded options for real-video source discovery.
#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    /// Directory recursion depth below the source root.
    pub max_depth: usize,
    /// Maximum number of video files to probe.
    pub max_files: usize,
    /// Seconds from the start of each file to scan for silences.
    pub sample_duration_s: f64,
    /// FFmpeg silencedetect threshold.
    pub silence_threshold_db: f64,
    /// Minimum silence duration to count as a fixture candidate.
    pub min_silence_s: f64,
    /// Directory recursion depth for existing Whisper sidecar discovery.
    pub max_transcript_depth: usize,
    /// Maximum number of Whisper sidecars to inspect.
    pub max_transcript_files: usize,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            max_depth: 2,
            max_files: 50,
            sample_duration_s: 90.0,
            silence_threshold_db: -40.0,
            min_silence_s: 0.8,
            max_transcript_depth: 6,
            max_transcript_files: 100,
        }
    }
}

/// Report emitted by `awidat-eval --acceptance-discover`.
#[derive(Debug, Serialize)]
pub struct DiscoveryReport {
    /// Root directory that was scanned.
    pub source_root: String,
    /// RFC3339 timestamp for the report.
    pub generated_at: String,
    /// Effective bounded scan options.
    pub options: DiscoveryOptionsReport,
    /// Number of video files probed.
    pub scanned_files: usize,
    /// Ranked candidate source videos.
    pub candidates: Vec<DiscoveryCandidate>,
    /// Transcript-backed candidates from existing Whisper sidecars.
    pub transcript_candidates: Vec<TranscriptDiscoveryCandidate>,
    /// Per-file probe errors that did not abort the whole scan.
    pub errors: Vec<DiscoveryError>,
}

/// Serializable copy of [`DiscoveryOptions`].
#[derive(Debug, Serialize)]
pub struct DiscoveryOptionsReport {
    /// Directory recursion depth below the source root.
    pub max_depth: usize,
    /// Maximum number of video files to probe.
    pub max_files: usize,
    /// Seconds from the start of each file to scan for silences.
    pub sample_duration_s: f64,
    /// FFmpeg silencedetect threshold.
    pub silence_threshold_db: f64,
    /// Minimum silence duration to count as a fixture candidate.
    pub min_silence_s: f64,
    /// Directory recursion depth for existing Whisper sidecar discovery.
    pub max_transcript_depth: usize,
    /// Maximum number of Whisper sidecars to inspect.
    pub max_transcript_files: usize,
}

impl From<&DiscoveryOptions> for DiscoveryOptionsReport {
    fn from(options: &DiscoveryOptions) -> Self {
        Self {
            max_depth: options.max_depth,
            max_files: options.max_files,
            sample_duration_s: options.sample_duration_s,
            silence_threshold_db: options.silence_threshold_db,
            min_silence_s: options.min_silence_s,
            max_transcript_depth: options.max_transcript_depth,
            max_transcript_files: options.max_transcript_files,
        }
    }
}

/// One ranked video candidate for a future real-behavioral fixture.
#[derive(Debug, Serialize)]
pub struct DiscoveryCandidate {
    /// Absolute source path.
    pub path: String,
    /// Container duration in seconds, when ffprobe reports it.
    pub duration_s: Option<f64>,
    /// First video stream width.
    pub width: Option<u32>,
    /// First video stream height.
    pub height: Option<u32>,
    /// Whether at least one video stream exists.
    pub has_video: bool,
    /// Whether at least one audio stream exists.
    pub has_audio: bool,
    /// Long silences detected in the sampled prefix.
    pub long_silences: Vec<DiscoveredSilence>,
    /// Heuristic score for fixture-authoring priority.
    pub score: f64,
    /// Suggested initial scenario type.
    pub suggested_behavior: String,
    /// Fixture id starter derived from the filename.
    pub fixture_id_hint: String,
    /// Draft runnable fixture JSON for this candidate, when enough evidence exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixture_draft: Option<Value>,
}

/// One detected long silence in a sampled video prefix.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredSilence {
    /// Silence start in source seconds.
    pub start_s: f64,
    /// Silence end in source seconds.
    pub end_s: f64,
    /// Silence duration in seconds.
    pub duration_s: f64,
}

/// Non-fatal discovery error for one source file.
#[derive(Debug, Serialize)]
pub struct DiscoveryError {
    /// Source path that failed to probe.
    pub path: String,
    /// Human-readable error context.
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    #[serde(default)]
    format: Option<FfprobeFormat>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

/// Scan a source directory and rank video files for acceptance fixture authoring.
pub fn discover_video_candidates(
    root: &Path,
    options: &DiscoveryOptions,
) -> Result<DiscoveryReport> {
    if !root.is_dir() {
        bail!(
            "acceptance discovery root is not a directory: {}",
            root.display()
        );
    }
    let mut paths = collect_video_paths(root, options.max_depth)?;
    paths.truncate(options.max_files);

    let mut candidates = Vec::new();
    let mut errors = Vec::new();
    for path in &paths {
        match discover_one(path, options) {
            Ok(candidate) => candidates.push(candidate),
            Err(err) => errors.push(DiscoveryError {
                path: path.to_string_lossy().into_owned(),
                message: format!("{err:#}"),
            }),
        }
    }
    candidates.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
    });
    let transcript_candidates = discover_transcript_candidates(
        root,
        options.max_transcript_depth,
        options.max_transcript_files,
    )?;

    Ok(DiscoveryReport {
        source_root: root.to_string_lossy().into_owned(),
        generated_at: Utc::now().to_rfc3339(),
        options: DiscoveryOptionsReport::from(options),
        scanned_files: paths.len(),
        candidates,
        transcript_candidates,
        errors,
    })
}

pub(crate) fn collect_video_paths(root: &Path, max_depth: usize) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    collect_video_paths_inner(root, max_depth, 0, &mut paths)?;
    paths.sort_by(|left, right| {
        path_depth(root, left)
            .cmp(&path_depth(root, right))
            .then_with(|| left.cmp(right))
    });
    Ok(paths)
}

fn collect_video_paths_inner(
    dir: &Path,
    max_depth: usize,
    depth: usize,
    paths: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        if is_hidden_path(&path) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            if depth < max_depth {
                collect_video_paths_inner(&path, max_depth, depth + 1, paths)?;
            }
        } else if file_type.is_file() && is_video_path(&path) {
            paths.push(path);
        }
    }
    Ok(())
}

fn discover_one(path: &Path, options: &DiscoveryOptions) -> Result<DiscoveryCandidate> {
    let probe = probe_video(path).with_context(|| format!("probe {}", path.display()))?;
    let long_silences = detect_long_silences(path, options)
        .with_context(|| format!("detect silences in {}", path.display()))?;
    let score = candidate_score(&probe, &long_silences);
    let suggested_behavior = if long_silences.is_empty() {
        "render_smoke_or_visual_review".into()
    } else {
        "dead_air_cleanup".into()
    };
    let mut candidate = DiscoveryCandidate {
        path: path.to_string_lossy().into_owned(),
        duration_s: probe.duration_s,
        width: probe.width,
        height: probe.height,
        has_video: probe.has_video,
        has_audio: probe.has_audio,
        long_silences,
        score,
        suggested_behavior,
        fixture_id_hint: fixture_id_hint(path),
        fixture_draft: None,
    };
    candidate.fixture_draft = build_fixture_draft(&candidate, options);
    Ok(candidate)
}

fn probe_video(path: &Path) -> Result<ProbeSummary> {
    let ffprobe = ffprobe_path().context("locate ffprobe")?;
    let output = Command::new(ffprobe)
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration:stream=codec_type,width,height")
        .arg("-of")
        .arg("json")
        .arg(path)
        .output()
        .context("spawn ffprobe")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr));
    }
    let parsed: FfprobeOutput =
        serde_json::from_slice(&output.stdout).context("parse ffprobe JSON")?;
    let mut has_video = false;
    let mut has_audio = false;
    let mut width = None;
    let mut height = None;
    for stream in parsed.streams {
        match stream.codec_type.as_deref() {
            Some("video") => {
                has_video = true;
                width = width.or(stream.width);
                height = height.or(stream.height);
            }
            Some("audio") => has_audio = true,
            _ => {}
        }
    }
    Ok(ProbeSummary {
        duration_s: parsed
            .format
            .and_then(|format| format.duration)
            .and_then(|duration| duration.parse::<f64>().ok()),
        width,
        height,
        has_video,
        has_audio,
    })
}

fn detect_long_silences(path: &Path, options: &DiscoveryOptions) -> Result<Vec<DiscoveredSilence>> {
    if !options.sample_duration_s.is_finite() || options.sample_duration_s <= 0.0 {
        bail!("sample_duration_s must be finite and > 0");
    }
    let ffmpeg = ffmpeg_path().context("locate ffmpeg")?;
    let output = Command::new(ffmpeg)
        .arg("-hide_banner")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("info")
        .arg("-i")
        .arg(path)
        .arg("-t")
        .arg(format!("{}", options.sample_duration_s))
        .arg("-af")
        .arg(format!(
            "silencedetect=noise={}dB:d={}",
            options.silence_threshold_db, options.min_silence_s
        ))
        .arg("-f")
        .arg("null")
        .arg("-")
        .output()
        .context("spawn ffmpeg silencedetect")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(parse_silencedetect_output(&String::from_utf8_lossy(
        &output.stderr,
    )))
}

pub(crate) fn parse_silencedetect_output(stderr: &str) -> Vec<DiscoveredSilence> {
    let mut starts = Vec::new();
    let mut ranges = Vec::new();
    for line in stderr.lines() {
        if let Some(start_s) = parse_silence_start(line) {
            starts.push(start_s);
        } else if let Some((end_s, duration_s)) = parse_silence_end(line)
            && let Some(start_s) = starts.pop()
        {
            ranges.push(DiscoveredSilence {
                start_s,
                end_s,
                duration_s,
            });
        }
    }
    ranges
}

fn parse_silence_start(line: &str) -> Option<f64> {
    let mut tokens = line.split_whitespace();
    while let Some(token) = tokens.next() {
        if let Some(value) = token.strip_prefix("silence_start:") {
            if !value.is_empty() {
                return value.parse::<f64>().ok();
            }
            return tokens.next().and_then(|value| value.parse::<f64>().ok());
        }
    }
    None
}

fn parse_silence_end(line: &str) -> Option<(f64, f64)> {
    let mut end_s = None;
    let mut duration_s = None;
    let mut tokens = line.split_whitespace();
    while let Some(token) = tokens.next() {
        if let Some(value) = token.strip_prefix("silence_end:") {
            end_s = if value.is_empty() {
                tokens.next().and_then(|value| value.parse::<f64>().ok())
            } else {
                value.parse::<f64>().ok()
            };
        } else if let Some(value) = token.strip_prefix("silence_duration:") {
            duration_s = if value.is_empty() {
                tokens.next().and_then(|value| value.parse::<f64>().ok())
            } else {
                value.parse::<f64>().ok()
            };
        }
    }
    let end_s = end_s?;
    Some((end_s, duration_s.unwrap_or(0.0)))
}

fn candidate_score(probe: &ProbeSummary, silences: &[DiscoveredSilence]) -> f64 {
    let silence_score = silences
        .iter()
        .map(|silence| silence.duration_s.min(3.0))
        .sum::<f64>()
        * 10.0;
    let duration_score = probe
        .duration_s
        .map(|duration| {
            if duration <= 120.0 {
                10.0
            } else if duration <= 600.0 {
                5.0
            } else {
                0.0
            }
        })
        .unwrap_or(0.0);
    let stream_score = if probe.has_video && probe.has_audio {
        5.0
    } else {
        0.0
    };
    silence_score + duration_score + stream_score
}

fn is_video_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("mp4" | "mov" | "m4v")
    )
}

fn path_depth(root: &Path, path: &Path) -> usize {
    path.strip_prefix(root)
        .map(|relative| relative.components().count())
        .unwrap_or_else(|_| path.components().count())
}

fn is_hidden_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn fixture_id_hint(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("real-video");
    let sanitized = stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    format!("acceptance::candidate_{sanitized}")
}

#[derive(Debug)]
struct ProbeSummary {
    duration_s: Option<f64>,
    width: Option<u32>,
    height: Option<u32>,
    has_video: bool,
    has_audio: bool,
}
