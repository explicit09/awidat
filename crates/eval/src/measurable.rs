//! Tier-2 measurable evidence from existing indexer sidecars and ffmpeg detectors.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

use crate::{CheckResult, HardGates};

/// Paths to the sidecars already emitted by Montage indexers.
#[derive(Debug, Clone)]
pub struct SidecarPaths {
    /// audio-energy-mcp JSON body or envelope.
    pub audio_energy: PathBuf,
    /// frame-quality-mcp JSON body or envelope.
    pub frame_quality: PathBuf,
    /// composition-mcp JSON body or envelope.
    pub composition: PathBuf,
}

/// Typed evidence loaded without recomputing indexer signals.
#[derive(Debug)]
pub struct SidecarEvidence {
    /// Audio loudness and silence evidence.
    pub audio: AudioEnergyEvidence,
    /// Ranked frame-quality evidence.
    pub frame_quality: FrameQualityEvidence,
    /// Composition verifier evidence.
    pub composition: CompositionEvidence,
}

/// Required audio-energy measurements.
#[derive(Debug)]
pub struct AudioEnergyEvidence {
    /// Integrated program loudness in LUFS.
    pub integrated_lufs: f64,
    /// True peak in dBFS.
    pub true_peak_dbfs: f64,
    /// Silence windows emitted by the indexer.
    pub silences: Vec<Span>,
}

/// Frame-quality evidence used by contact-sheet selection.
#[derive(Debug, Deserialize)]
pub struct FrameQualityEvidence {
    /// Ranked thumbnail candidates.
    pub thumbnail_candidates: Vec<ThumbnailCandidate>,
}

/// One ranked thumbnail candidate.
#[derive(Debug, Deserialize)]
pub struct ThumbnailCandidate {
    /// Source time in seconds.
    pub t_s: f64,
    /// Normalized quality score.
    pub thumbnail_score: f64,
}

/// Composition regions and deterministic self-verification.
#[derive(Debug, Deserialize)]
pub struct CompositionEvidence {
    /// Indexer verification summary.
    pub verification: CompositionVerification,
}

/// composition-mcp verification summary.
#[derive(Debug, Deserialize)]
pub struct CompositionVerification {
    /// True when all emitted regions passed structural checks.
    pub passed: bool,
    /// Number of regions checked.
    pub checked_regions: usize,
    /// Deterministic issue descriptions.
    pub issues: Vec<String>,
}

/// Closed-open time span in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Span {
    /// Inclusive start time.
    pub start: f64,
    /// Exclusive end time.
    pub end: f64,
}

/// Parsed ffmpeg detector spans for one rendered output.
#[derive(Debug, Default, Serialize)]
pub struct DetectorEvidence {
    /// Detected silence windows.
    pub silences: Vec<Span>,
    /// Detected black-frame windows.
    pub black_frames: Vec<Span>,
    /// Detected frozen-video windows.
    pub freezes: Vec<Span>,
}

/// Aggregated tier-2 report.
#[derive(Debug, Serialize)]
pub struct MeasurableReport {
    /// True only when every configured measurable check passes.
    pub passed: bool,
    /// Individual deterministic outcomes.
    pub checks: Vec<CheckResult>,
}

impl Span {
    /// Construct a measured span.
    pub const fn new(start: f64, end: f64) -> Self {
        Self { start, end }
    }

    /// Span duration in seconds.
    pub fn duration(self) -> f64 {
        self.end - self.start
    }
}

/// Failure to read or validate sidecar evidence.
#[derive(Debug, Error)]
pub enum SidecarEvidenceError {
    /// Reading a sidecar failed.
    #[error("reading {kind} sidecar {}: {source}", path.display())]
    Read {
        /// Sidecar class.
        kind: &'static str,
        /// Sidecar path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// A sidecar did not match its required schema.
    #[error("parsing {kind} sidecar {}: {source}", path.display())]
    Parse {
        /// Sidecar class.
        kind: &'static str,
        /// Sidecar path.
        path: PathBuf,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// Parsed measurements violate deterministic bounds.
    #[error("invalid {kind} sidecar: {message}")]
    Contract {
        /// Sidecar class.
        kind: &'static str,
        /// Contract diagnostic.
        message: String,
    },
}

/// Malformed ffmpeg detector output.
#[derive(Debug, Error)]
#[error("invalid {detector} output: {message}")]
pub struct DetectorParseError {
    detector: &'static str,
    message: String,
}

/// Failure to run or persist ffmpeg detector evidence.
#[derive(Debug, Error)]
pub enum DetectorRunError {
    /// ffmpeg could not be spawned.
    #[error("running ffmpeg detectors: {0}")]
    Spawn(#[source] std::io::Error),
    /// ffmpeg returned a non-success status.
    #[error("ffmpeg detectors failed with {status}: {stderr}")]
    Failed {
        /// Process exit status.
        status: std::process::ExitStatus,
        /// Captured diagnostic output.
        stderr: String,
    },
    /// Detector records were malformed.
    #[error(transparent)]
    Parse(#[from] DetectorParseError),
    /// Evidence persistence failed.
    #[error("writing detector evidence {}: {source}", path.display())]
    Write {
        /// Artifact path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// Parsed JSON encoding failed.
    #[error("serializing detector evidence: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Deserialize)]
struct AudioBody {
    loudness_integrated_lufs: f64,
    true_peak_dbfs: f64,
    #[serde(default)]
    silences: Vec<AudioSpan>,
}

#[derive(Deserialize)]
struct AudioSpan {
    start_s: f64,
    end_s: f64,
}

impl SidecarEvidence {
    /// Load the three existing sidecar bodies, accepting MCP `{ "data": ... }` envelopes.
    pub fn load(paths: &SidecarPaths) -> Result<Self, SidecarEvidenceError> {
        let audio: AudioBody = read_body("audio-energy", &paths.audio_energy)?;
        let frame_quality: FrameQualityEvidence = read_body("frame-quality", &paths.frame_quality)?;
        let composition: CompositionEvidence = read_body("composition", &paths.composition)?;

        validate_finite(
            "audio-energy",
            "loudness_integrated_lufs",
            audio.loudness_integrated_lufs,
        )?;
        validate_finite("audio-energy", "true_peak_dbfs", audio.true_peak_dbfs)?;
        let silences = audio
            .silences
            .into_iter()
            .map(|span| Span::new(span.start_s, span.end_s))
            .collect::<Vec<_>>();
        if silences.iter().any(|span| {
            !span.start.is_finite()
                || !span.end.is_finite()
                || span.start < 0.0
                || span.end <= span.start
        }) {
            return Err(SidecarEvidenceError::Contract {
                kind: "audio-energy",
                message: "silence spans must be finite, non-negative, and increasing".into(),
            });
        }
        for candidate in &frame_quality.thumbnail_candidates {
            validate_finite("frame-quality", "thumbnail t_s", candidate.t_s)?;
            if !candidate.thumbnail_score.is_finite()
                || !(0.0..=1.0).contains(&candidate.thumbnail_score)
            {
                return Err(SidecarEvidenceError::Contract {
                    kind: "frame-quality",
                    message: "thumbnail_score must be finite and between 0 and 1".into(),
                });
            }
        }

        Ok(Self {
            audio: AudioEnergyEvidence {
                integrated_lufs: audio.loudness_integrated_lufs,
                true_peak_dbfs: audio.true_peak_dbfs,
                silences,
            },
            frame_quality,
            composition,
        })
    }
}

/// Parse ffmpeg `silencedetect` start/end records.
pub fn parse_silencedetect(input: impl AsRef<str>) -> Result<Vec<Span>, DetectorParseError> {
    parse_detector(
        input.as_ref(),
        "silencedetect",
        "silence_start:",
        "silence_end:",
    )
}

/// Parse ffmpeg `blackdetect` start/end records.
pub fn parse_blackdetect(input: impl AsRef<str>) -> Result<Vec<Span>, DetectorParseError> {
    parse_detector(input.as_ref(), "blackdetect", "black_start:", "black_end:")
}

/// Parse ffmpeg `freezedetect` start/end records.
pub fn parse_freezedetect(input: impl AsRef<str>) -> Result<Vec<Span>, DetectorParseError> {
    parse_detector(
        input.as_ref(),
        "freezedetect",
        "freeze_start:",
        "freeze_end:",
    )
}

/// Evaluate existing sidecar evidence and ffmpeg detector spans against hard gates.
pub fn evaluate_measurable(
    requirements: &HardGates,
    sidecars: &SidecarEvidence,
    detectors: &DetectorEvidence,
) -> MeasurableReport {
    let mut checks = Vec::new();
    if let Some(maximum) = requirements.max_remaining_silence_seconds {
        let longest = detectors
            .silences
            .iter()
            .map(|span| span.duration())
            .fold(0.0_f64, f64::max);
        add_check(
            &mut checks,
            "SILENCE_TOO_LONG",
            longest <= maximum,
            format!("longest remaining silence is {longest:.3}s; maximum is {maximum:.3}s"),
            json!({"longest_seconds": longest, "maximum_seconds": maximum, "spans": detectors.silences}),
        );
    }
    if requirements.no_black_frames {
        add_check(
            &mut checks,
            "BLACK_FRAMES",
            detectors.black_frames.is_empty(),
            format!(
                "{} black-frame span(s) detected",
                detectors.black_frames.len()
            ),
            json!({"spans": detectors.black_frames}),
        );
    }
    if requirements.no_freeze_frames {
        add_check(
            &mut checks,
            "FREEZE_FRAMES",
            detectors.freezes.is_empty(),
            format!("{} frozen span(s) detected", detectors.freezes.len()),
            json!({"spans": detectors.freezes}),
        );
    }
    add_check(
        &mut checks,
        "LOUDNESS_EVIDENCE",
        true,
        format!(
            "audio-energy reports {:.2} LUFS and {:.2} dBFS true peak",
            sidecars.audio.integrated_lufs, sidecars.audio.true_peak_dbfs
        ),
        json!({
            "integrated_lufs": sidecars.audio.integrated_lufs,
            "true_peak_dbfs": sidecars.audio.true_peak_dbfs,
        }),
    );
    add_check(
        &mut checks,
        "FRAME_QUALITY_EVIDENCE",
        !sidecars.frame_quality.thumbnail_candidates.is_empty(),
        format!(
            "frame-quality reports {} thumbnail candidate(s)",
            sidecars.frame_quality.thumbnail_candidates.len()
        ),
        json!({"candidate_count": sidecars.frame_quality.thumbnail_candidates.len()}),
    );
    add_check(
        &mut checks,
        "COMPOSITION_VERIFICATION",
        sidecars.composition.verification.passed,
        format!(
            "composition checked {} region(s) with {} issue(s)",
            sidecars.composition.verification.checked_regions,
            sidecars.composition.verification.issues.len()
        ),
        json!({
            "checked_regions": sidecars.composition.verification.checked_regions,
            "issues": sidecars.composition.verification.issues,
        }),
    );
    MeasurableReport {
        passed: checks.iter().all(|check| check.passed),
        checks,
    }
}

/// Run ffmpeg silence/black/freeze detection and persist attempt evidence.
pub fn run_ffmpeg_detectors(
    render: impl AsRef<Path>,
    attempt_root: impl AsRef<Path>,
) -> Result<DetectorEvidence, DetectorRunError> {
    run_ffmpeg_detectors_with("ffmpeg", render, attempt_root)
}

fn run_ffmpeg_detectors_with(
    ffmpeg: impl AsRef<Path>,
    render: impl AsRef<Path>,
    attempt_root: impl AsRef<Path>,
) -> Result<DetectorEvidence, DetectorRunError> {
    let output = Command::new(ffmpeg.as_ref())
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(render.as_ref())
        .args([
            "-vf",
            "blackdetect=d=0.1:pix_th=0.10,freezedetect=n=-60dB:d=2",
            "-af",
            "silencedetect=n=-50dB:d=0.1",
            "-f",
            "null",
            "-",
        ])
        .output()
        .map_err(DetectorRunError::Spawn)?;
    if !output.status.success() {
        return Err(DetectorRunError::Failed {
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let attempt_root = attempt_root.as_ref();
    write_detector_artifact(
        &attempt_root.join("ffmpeg-detectors.stderr"),
        &output.stderr,
    )?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let evidence = DetectorEvidence {
        silences: parse_silencedetect(&stderr)?,
        black_frames: parse_blackdetect(&stderr)?,
        freezes: parse_freezedetect(&stderr)?,
    };
    write_detector_json(&attempt_root.join("silence.json"), &evidence.silences)?;
    write_detector_json(&attempt_root.join("black.json"), &evidence.black_frames)?;
    write_detector_json(&attempt_root.join("freeze.json"), &evidence.freezes)?;
    Ok(evidence)
}

fn read_body<T: serde::de::DeserializeOwned>(
    kind: &'static str,
    path: &Path,
) -> Result<T, SidecarEvidenceError> {
    let bytes = std::fs::read(path).map_err(|source| SidecarEvidenceError::Read {
        kind,
        path: path.to_path_buf(),
        source,
    })?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|source| SidecarEvidenceError::Parse {
            kind,
            path: path.to_path_buf(),
            source,
        })?;
    let body = value.get("data").cloned().unwrap_or(value);
    serde_json::from_value(body).map_err(|source| SidecarEvidenceError::Parse {
        kind,
        path: path.to_path_buf(),
        source,
    })
}

fn validate_finite(kind: &'static str, name: &str, value: f64) -> Result<(), SidecarEvidenceError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(SidecarEvidenceError::Contract {
            kind,
            message: format!("{name} must be finite"),
        })
    }
}

fn parse_detector(
    input: &str,
    detector: &'static str,
    start_key: &str,
    end_key: &str,
) -> Result<Vec<Span>, DetectorParseError> {
    let mut spans = Vec::new();
    let mut pending = None;
    for line in input.lines() {
        if let Some(start) = extract_number(line, start_key)?
            && pending.replace(start).is_some()
        {
            return Err(DetectorParseError {
                detector,
                message: "unmatched start before the previous span ended".into(),
            });
        }
        if let Some(end) = extract_number(line, end_key)? {
            let Some(start) = pending.take() else {
                return Err(DetectorParseError {
                    detector,
                    message: "unmatched end without a preceding start".into(),
                });
            };
            if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start {
                return Err(DetectorParseError {
                    detector,
                    message: format!("invalid span {start}..{end}"),
                });
            }
            spans.push(Span::new(start, end));
        }
    }
    if pending.is_some() {
        return Err(DetectorParseError {
            detector,
            message: "unmatched start at end of detector output".into(),
        });
    }
    Ok(spans)
}

fn extract_number(line: &str, key: &str) -> Result<Option<f64>, DetectorParseError> {
    let Some(offset) = line.find(key) else {
        return Ok(None);
    };
    let value = line[offset + key.len()..].trim_start();
    let end = value
        .find(|character: char| {
            !(character.is_ascii_digit() || matches!(character, '-' | '+' | '.' | 'e' | 'E'))
        })
        .unwrap_or(value.len());
    let token = &value[..end];
    token
        .parse::<f64>()
        .map(Some)
        .map_err(|_| DetectorParseError {
            detector: "ffmpeg detector",
            message: format!("{key} is not followed by a number"),
        })
}

fn add_check(
    checks: &mut Vec<CheckResult>,
    code: &str,
    passed: bool,
    detail: String,
    evidence: serde_json::Value,
) {
    checks.push(CheckResult {
        code: code.into(),
        passed,
        detail,
        evidence,
    });
}

fn write_detector_json(path: &Path, spans: &[Span]) -> Result<(), DetectorRunError> {
    write_detector_artifact(path, &serde_json::to_vec_pretty(spans)?)
}

fn write_detector_artifact(path: &Path, bytes: &[u8]) -> Result<(), DetectorRunError> {
    std::fs::write(path, bytes).map_err(|source| DetectorRunError::Write {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn detector_command_persists_raw_and_parsed_evidence() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("temporary dir: {error}"));
        let ffmpeg = temp.path().join("fake-ffmpeg");
        std::fs::write(
            &ffmpeg,
            "#!/bin/sh\n\
             echo 'silence_start: 12' >&2\n\
             echo 'silence_end: 14.4' >&2\n\
             echo 'black_start:30 black_end:31.2 black_duration:1.2' >&2\n\
             echo 'freeze_start: 42' >&2\n\
             echo 'freeze_end: 45' >&2\n",
        )
        .unwrap_or_else(|error| panic!("fake ffmpeg should write: {error}"));
        let mut permissions = std::fs::metadata(&ffmpeg)
            .unwrap_or_else(|error| panic!("fake ffmpeg metadata: {error}"))
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&ffmpeg, permissions)
            .unwrap_or_else(|error| panic!("fake ffmpeg permissions: {error}"));
        let attempt = temp.path().join("attempt_1");
        std::fs::create_dir(&attempt).unwrap_or_else(|error| panic!("attempt directory: {error}"));

        let evidence = super::run_ffmpeg_detectors_with(&ffmpeg, "render.mp4", &attempt)
            .unwrap_or_else(|error| panic!("detectors should run: {error}"));

        assert_eq!(evidence.silences, vec![super::Span::new(12.0, 14.4)]);
        assert_eq!(evidence.black_frames, vec![super::Span::new(30.0, 31.2)]);
        assert_eq!(evidence.freezes, vec![super::Span::new(42.0, 45.0)]);
        assert!(attempt.join("ffmpeg-detectors.stderr").is_file());
        assert!(attempt.join("silence.json").is_file());
        assert!(attempt.join("black.json").is_file());
        assert!(attempt.join("freeze.json").is_file());
    }
}
