//! Tier-1 mechanical inspection of render metadata and artifacts.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::HardGates;

/// One deterministic validator outcome.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    /// Stable failure taxonomy code.
    pub code: String,
    /// Whether the requirement passed.
    pub passed: bool,
    /// Human-readable measured result.
    pub detail: String,
    /// Structured measurements used for the verdict.
    pub evidence: Value,
}

/// Aggregated tier-1 report.
#[derive(Debug, Clone, Serialize)]
pub struct MechanicalReport {
    /// True only when every mechanical check passes.
    pub passed: bool,
    /// Individual deterministic outcomes.
    pub checks: Vec<CheckResult>,
}

/// Failure to parse or evaluate mechanical evidence.
#[derive(Debug, Error)]
pub enum MechanicalError {
    /// ffprobe output was not valid JSON.
    #[error("invalid ffprobe JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// A configured requirement is malformed.
    #[error("invalid mechanical requirement: {0}")]
    Requirement(String),
    /// A render manifest could not be loaded through Montage's schema.
    #[error("reading render manifest {}: {message}", path.display())]
    Manifest {
        /// Manifest path.
        path: PathBuf,
        /// Montage parser diagnostic.
        message: String,
    },
}

#[derive(Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

/// Inspect captured `ffprobe -print_format json -show_format -show_streams` output.
pub fn inspect_ffprobe(
    input: impl AsRef<str>,
    requirements: &HardGates,
) -> Result<MechanicalReport, MechanicalError> {
    let probe: ProbeOutput = serde_json::from_str(input.as_ref())?;
    let mut checks = Vec::new();

    if requirements.playable {
        let duration = probe
            .format
            .as_ref()
            .and_then(|format| format.duration.as_deref())
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0);
        push_check(
            &mut checks,
            "PLAYABLE",
            duration.is_some(),
            duration.map_or_else(
                || "ffprobe did not report a positive duration".into(),
                |value| format!("ffprobe duration is {value:.3}s"),
            ),
            json!({"duration_seconds": duration}),
        );
    }

    let video = probe
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"));
    let audio = probe
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("audio"));
    push_check(
        &mut checks,
        "VIDEO_STREAM_MISSING",
        video.is_some(),
        if video.is_some() {
            "video stream present".into()
        } else {
            "no video stream present".into()
        },
        json!({"present": video.is_some()}),
    );
    push_check(
        &mut checks,
        "AUDIO_STREAM_MISSING",
        audio.is_some(),
        if audio.is_some() {
            "audio stream present".into()
        } else {
            "no audio stream present".into()
        },
        json!({"present": audio.is_some()}),
    );

    if let Some(video) = video {
        let codec_ok = video
            .codec_name
            .as_ref()
            .is_some_and(|name| !name.is_empty());
        push_check(
            &mut checks,
            "VIDEO_CODEC",
            codec_ok,
            video.codec_name.as_ref().map_or_else(
                || "video codec missing".into(),
                |name| format!("video codec is {name}"),
            ),
            json!({"codec": video.codec_name}),
        );

        let fps = video
            .avg_frame_rate
            .as_deref()
            .and_then(parse_fraction)
            .filter(|value| value.is_finite() && *value > 0.0);
        push_check(
            &mut checks,
            "VIDEO_FPS",
            fps.is_some(),
            fps.map_or_else(
                || "video frame rate missing or invalid".into(),
                |value| format!("video frame rate is {value:.3} fps"),
            ),
            json!({"frames_per_second": fps}),
        );

        if let Some(required) = &requirements.aspect_ratio {
            let (ratio_width, ratio_height) = parse_aspect_ratio(required)?;
            let dimensions = video.width.zip(video.height);
            let passed = dimensions.is_some_and(|(width, height)| {
                let difference = i64::from(width) * i64::from(ratio_height)
                    - i64::from(height) * i64::from(ratio_width);
                difference.unsigned_abs() <= u64::from(ratio_width.max(ratio_height))
            });
            push_check(
                &mut checks,
                "ASPECT_RATIO",
                passed,
                dimensions.map_or_else(
                    || "video dimensions missing".into(),
                    |(width, height)| format!("video is {width}x{height}; required {required}"),
                ),
                json!({
                    "width": video.width,
                    "height": video.height,
                    "required": required,
                }),
            );
        }
    }

    Ok(MechanicalReport {
        passed: checks.iter().all(|check| check.passed),
        checks,
    })
}

/// Inspect a Montage render manifest and its required output artifact.
pub fn inspect_manifest(
    manifest_path: impl AsRef<Path>,
    expected_output: impl AsRef<Path>,
) -> Result<MechanicalReport, MechanicalError> {
    let manifest_path = manifest_path.as_ref();
    let expected_output = expected_output.as_ref();
    let manifest = montage_render::read_render_manifest(manifest_path).map_err(|error| {
        MechanicalError::Manifest {
            path: manifest_path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    let mut checks = Vec::new();
    let schema_ok = manifest.schema_version == montage_render::RENDER_MANIFEST_SCHEMA_VERSION;
    push_check(
        &mut checks,
        "MANIFEST_SCHEMA",
        schema_ok,
        if schema_ok {
            format!("manifest schema {} is supported", manifest.schema_version)
        } else {
            format!("manifest schema {} is unsupported", manifest.schema_version)
        },
        json!({
            "schema_version": manifest.schema_version,
            "supported": montage_render::RENDER_MANIFEST_SCHEMA_VERSION,
        }),
    );

    let declared = manifest.outputs.iter().find(|artifact| {
        resolve_manifest_path(&manifest.project_root, &artifact.path) == expected_output
    });
    push_check(
        &mut checks,
        "MANIFEST_OUTPUT_DECLARED",
        declared.is_some_and(|artifact| artifact.required),
        if declared.is_some_and(|artifact| artifact.required) {
            "expected output is declared as required".into()
        } else {
            "expected output is not declared as required".into()
        },
        json!({"expected_output": expected_output}),
    );
    let output_exists = expected_output.is_file();
    push_check(
        &mut checks,
        "MANIFEST_OUTPUT_MISSING",
        output_exists,
        if output_exists {
            "required output exists".into()
        } else {
            format!("required output is missing: {}", expected_output.display())
        },
        json!({"path": expected_output, "exists": output_exists}),
    );

    for artifact in manifest.inputs.iter().filter(|artifact| artifact.required) {
        let path = resolve_manifest_path(&manifest.project_root, &artifact.path);
        let exists = path.is_file();
        push_check(
            &mut checks,
            "MANIFEST_INPUT_MISSING",
            exists,
            if exists {
                format!("required input exists: {}", path.display())
            } else {
                format!("required input is missing: {}", path.display())
            },
            json!({"path": path, "exists": exists}),
        );
    }
    for artifact in manifest
        .sidecars
        .iter()
        .filter(|artifact| artifact.required)
    {
        let path = resolve_manifest_path(&manifest.project_root, &artifact.path);
        let exists = path.is_file();
        push_check(
            &mut checks,
            "MANIFEST_SIDECAR_MISSING",
            exists,
            if exists {
                format!("required sidecar exists: {}", path.display())
            } else {
                format!("required sidecar is missing: {}", path.display())
            },
            json!({"path": path, "exists": exists}),
        );
    }

    Ok(report(checks))
}

/// Parse an OTIO timeline through Montage's forward-compatible reader.
pub fn inspect_otio(path: impl AsRef<Path>) -> MechanicalReport {
    let path = path.as_ref();
    let mut warnings = Vec::new();
    let parsed = montage_proto::project::read_otio_timeline(path, &mut warnings);
    let check = match parsed {
        Ok(_) => CheckResult {
            code: "OTIO_INVALID".into(),
            passed: true,
            detail: format!("OTIO parses with {} schema warning(s)", warnings.len()),
            evidence: json!({"path": path, "schema_warnings": warnings.len()}),
        },
        Err(error) => CheckResult {
            code: "OTIO_INVALID".into(),
            passed: false,
            detail: format!("OTIO failed to parse: {error}"),
            evidence: json!({"path": path, "error": error.to_string()}),
        },
    };
    report(vec![check])
}

/// Run ffprobe and inspect its captured JSON. Spawn and exit failures are blocking checks.
pub fn run_ffprobe(output: impl AsRef<Path>, requirements: &HardGates) -> MechanicalReport {
    let output = output.as_ref();
    let result = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(output)
        .output();
    match result {
        Ok(command) if command.status.success() => {
            match String::from_utf8(command.stdout)
                .ok()
                .and_then(|stdout| inspect_ffprobe(stdout, requirements).ok())
            {
                Some(report) => report,
                None => command_failure(
                    "FFPROBE_INVALID_OUTPUT",
                    "ffprobe output was not valid UTF-8 JSON",
                ),
            }
        }
        Ok(command) => command_failure(
            "FFPROBE_FAILED",
            &format!(
                "ffprobe exited with {}: {}",
                command.status,
                String::from_utf8_lossy(&command.stderr).trim()
            ),
        ),
        Err(error) => command_failure(
            "FFPROBE_MISSING",
            &format!("could not run ffprobe: {error}"),
        ),
    }
}

/// Run the existing `montage validate` command for a project path.
pub fn run_montage_validate(project: impl AsRef<Path>) -> MechanicalReport {
    let project = project.as_ref();
    match Command::new("montage")
        .arg("validate")
        .arg(project)
        .output()
    {
        Ok(command) if command.status.success() => report(vec![CheckResult {
            code: "MONTAGE_VALIDATE".into(),
            passed: true,
            detail: "montage validate passed".into(),
            evidence: json!({"project": project}),
        }]),
        Ok(command) => command_failure(
            "MONTAGE_VALIDATE_FAILED",
            &format!(
                "montage validate exited with {}: {}",
                command.status,
                String::from_utf8_lossy(&command.stderr).trim()
            ),
        ),
        Err(error) => command_failure(
            "MONTAGE_VALIDATE_MISSING",
            &format!("could not run montage validate: {error}"),
        ),
    }
}

fn parse_fraction(input: &str) -> Option<f64> {
    let (numerator, denominator) = input.split_once('/')?;
    let numerator = numerator.parse::<f64>().ok()?;
    let denominator = denominator.parse::<f64>().ok()?;
    if denominator == 0.0 {
        None
    } else {
        Some(numerator / denominator)
    }
}

fn parse_aspect_ratio(input: &str) -> Result<(u32, u32), MechanicalError> {
    let (width, height) = input
        .split_once(':')
        .ok_or_else(|| MechanicalError::Requirement(format!("aspect ratio {input:?}")))?;
    let width = width
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| MechanicalError::Requirement(format!("aspect ratio {input:?}")))?;
    let height = height
        .parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| MechanicalError::Requirement(format!("aspect ratio {input:?}")))?;
    Ok((width, height))
}

fn push_check(
    checks: &mut Vec<CheckResult>,
    code: &str,
    passed: bool,
    detail: String,
    evidence: Value,
) {
    checks.push(CheckResult {
        code: code.into(),
        passed,
        detail,
        evidence,
    });
}

fn resolve_manifest_path(project_root: &str, artifact: &str) -> PathBuf {
    let artifact = PathBuf::from(artifact);
    if artifact.is_absolute() {
        artifact
    } else {
        Path::new(project_root).join(artifact)
    }
}

fn command_failure(code: &str, detail: &str) -> MechanicalReport {
    report(vec![CheckResult {
        code: code.into(),
        passed: false,
        detail: detail.into(),
        evidence: json!({"error": detail}),
    }])
}

fn report(checks: Vec<CheckResult>) -> MechanicalReport {
    MechanicalReport {
        passed: checks.iter().all(|check| check.passed),
        checks,
    }
}
