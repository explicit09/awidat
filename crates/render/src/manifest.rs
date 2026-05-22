//! Durable render execution manifests and replay support.
//!
//! The manifest captures the planned render backend, replay command,
//! input fingerprints, output artifacts, sidecars, and known limitations
//! for a render/export job. It is intentionally independent from the
//! agent tools so CLI, TUI, desktop, eval, and future remux paths can
//! share the same evidence format.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{OutputPathPolicy, validate_render_output_path};

/// Current render execution manifest schema version.
pub const RENDER_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Errors returned while building or writing render manifests.
#[derive(Debug, Error)]
pub enum RenderManifestError {
    /// Filesystem operation failed.
    #[error("render manifest IO failed at {path}: {source}")]
    Io {
        /// Path being read or written.
        path: String,
        /// Source IO error.
        #[source]
        source: io::Error,
    },
    /// JSON serialization failed.
    #[error("render manifest JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

/// Errors returned while validating or replaying render manifests.
#[derive(Debug, Error)]
pub enum RenderReplayError {
    /// The manifest could not be read from disk.
    #[error("read render manifest {path}: {source}")]
    ReadManifest {
        /// Manifest path.
        path: String,
        /// Source manifest error.
        #[source]
        source: RenderManifestError,
    },
    /// The manifest JSON could not be parsed.
    #[error("parse render manifest {path}: {source}")]
    ParseManifest {
        /// Manifest path.
        path: String,
        /// Source JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// The manifest schema is not supported by this binary.
    #[error("render manifest {path} has unsupported schema version {version}")]
    UnsupportedSchema {
        /// Manifest path.
        path: String,
        /// Unsupported schema version.
        version: u32,
    },
    /// The manifest records a backend that cannot be replayed yet.
    #[error("render manifest {path} cannot be replayed: {reason}")]
    UnsupportedPlan {
        /// Manifest path.
        path: String,
        /// Non-support reason.
        reason: String,
    },
    /// The FFmpeg argv replay plan had no command.
    #[error("render manifest {path} ffmpeg argv is empty")]
    EmptyArgv {
        /// Manifest path.
        path: String,
    },
    /// The replay cwd does not exist.
    #[error("render manifest {path} replay cwd does not exist: {cwd}")]
    MissingCwd {
        /// Manifest path.
        path: String,
        /// Missing cwd.
        cwd: String,
    },
    /// Output path safety validation failed.
    #[error("render manifest {path} output path preflight failed: {source}")]
    OutputPreflight {
        /// Manifest path.
        path: String,
        /// Source safety error.
        #[source]
        source: crate::OutputPathSafetyError,
    },
    /// The FFmpeg process could not be spawned.
    #[error("render manifest {path} ffmpeg replay failed to spawn: {source}")]
    Spawn {
        /// Manifest path.
        path: String,
        /// Source IO error.
        #[source]
        source: io::Error,
    },
}

/// Result from replaying a render execution manifest.
#[derive(Debug)]
pub struct RenderReplayOutcome {
    /// Manifest path replayed.
    pub manifest_path: PathBuf,
    /// Required output paths declared by the manifest.
    pub output_paths: Vec<PathBuf>,
    /// Process exit status.
    pub status: ExitStatus,
}

/// Render execution record written beside render outputs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderExecutionManifest {
    /// Schema version for compatibility checks.
    pub schema_version: u32,
    /// Stable content ID for planned manifest fields.
    pub manifest_id: String,
    /// RFC3339 creation timestamp.
    pub created_at: String,
    /// Awidat crate version that produced the manifest.
    pub awidat_version: String,
    /// Project root used to plan the render.
    pub project_root: String,
    /// Optional hash of the project file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_hash: Option<String>,
    /// Optional timeline-specific hash.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline_hash: Option<String>,
    /// Backend class selected for this render.
    pub backend: RenderBackendKind,
    /// Replay plan for this phase.
    pub replay: RenderReplayPlan,
    /// Input media fingerprints known at planning time.
    pub inputs: Vec<RenderInputFingerprint>,
    /// Planned output artifacts.
    pub outputs: Vec<RenderOutputArtifact>,
    /// Sidecar fingerprints known at planning time.
    pub sidecars: Vec<RenderSidecarFingerprint>,
    /// Planner or backend limitations.
    pub limitations: Vec<RenderManifestLimitation>,
    /// Optional verification summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<RenderVerificationSummary>,
    /// Small call-site metadata values.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

/// Input used to create a planned render manifest.
#[derive(Debug, Clone)]
pub struct RenderExecutionManifestInput {
    /// RFC3339 creation timestamp.
    pub created_at: String,
    /// Awidat crate version that produced the manifest.
    pub awidat_version: String,
    /// Project root used to plan the render.
    pub project_root: String,
    /// Optional hash of the project file.
    pub project_hash: Option<String>,
    /// Optional timeline-specific hash.
    pub timeline_hash: Option<String>,
    /// Backend class selected for this render.
    pub backend: RenderBackendKind,
    /// Replay plan for this phase.
    pub replay: RenderReplayPlan,
    /// Input media fingerprints known at planning time.
    pub inputs: Vec<RenderInputFingerprint>,
    /// Planned output artifacts.
    pub outputs: Vec<RenderOutputArtifact>,
    /// Sidecar fingerprints known at planning time.
    pub sidecars: Vec<RenderSidecarFingerprint>,
    /// Planner or backend limitations.
    pub limitations: Vec<RenderManifestLimitation>,
    /// Optional verification summary.
    pub verification: Option<RenderVerificationSummary>,
    /// Small call-site metadata values.
    pub metadata: BTreeMap<String, String>,
}

/// Render backend selected for a job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderBackendKind {
    /// Asset preview render.
    AssetPreview,
    /// Asset segment stream-copy render.
    AssetSegmentStreamCopy,
    /// Full asset re-encode render.
    AssetFullReencode,
    /// Timeline FFmpeg re-encode render.
    TimelineFfmpegReencode,
    /// Timeline raw-stream GPU render.
    TimelineRawStreamGpu,
    /// Delivery package export render.
    PackageExport,
    /// Stream export/remux render.
    StreamExportRemux,
}

/// Replay plan embedded in a render execution manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderReplayPlan {
    /// Replay by invoking FFmpeg with a full argv vector.
    FfmpegArgv {
        /// Full argv including the FFmpeg binary at index 0.
        argv: Vec<String>,
        /// Optional current working directory.
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    /// Manifest backend is recorded but not replayable in this phase.
    Unsupported {
        /// Human-readable non-support reason.
        reason: String,
    },
}

/// Fingerprint for a render input file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderInputFingerprint {
    /// Absolute or project-relative file path.
    pub path: String,
    /// SHA-256 hex digest.
    pub sha256: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Whether this input is required for replay.
    pub required: bool,
}

/// Planned or completed render output artifact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderOutputArtifact {
    /// Absolute or project-relative file path.
    pub path: String,
    /// Optional SHA-256 hex digest after output exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Optional file size after output exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Whether this output is required for success.
    pub required: bool,
}

/// Fingerprint for a render sidecar file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderSidecarFingerprint {
    /// Absolute or project-relative file path.
    pub path: String,
    /// SHA-256 hex digest.
    pub sha256: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Whether this sidecar is required for the render package.
    pub required: bool,
}

/// Known limitation recorded by a render planner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderManifestLimitation {
    /// Stable limitation code.
    pub code: String,
    /// Human-readable limitation message.
    pub message: String,
}

/// Optional summary of verifier output associated with a render.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderVerificationSummary {
    /// Verification status.
    pub status: String,
    /// Path to the verification report.
    pub report_path: String,
}

impl RenderExecutionManifest {
    /// Build a planned manifest and derive its stable `manifest_id`.
    pub fn planned(input: RenderExecutionManifestInput) -> Self {
        let mut manifest = Self {
            schema_version: RENDER_MANIFEST_SCHEMA_VERSION,
            manifest_id: String::new(),
            created_at: input.created_at,
            awidat_version: input.awidat_version,
            project_root: input.project_root,
            project_hash: input.project_hash,
            timeline_hash: input.timeline_hash,
            backend: input.backend,
            replay: input.replay,
            inputs: input.inputs,
            outputs: input.outputs,
            sidecars: input.sidecars,
            limitations: input.limitations,
            verification: input.verification,
            metadata: input.metadata,
        };
        manifest.manifest_id = stable_manifest_id(&manifest);
        manifest
    }
}

impl RenderBackendKind {
    /// Map public `start_render` scope strings to backend classes.
    pub fn from_start_render_scope(scope: &str) -> Option<Self> {
        match scope {
            "preview" => Some(Self::AssetPreview),
            "segment" => Some(Self::AssetSegmentStreamCopy),
            "full" => Some(Self::AssetFullReencode),
            "timeline" => Some(Self::TimelineFfmpegReencode),
            _ => None,
        }
    }
}

#[derive(Serialize)]
struct StableManifestView<'a> {
    schema_version: u32,
    awidat_version: &'a str,
    project_root: &'a str,
    project_hash: &'a Option<String>,
    timeline_hash: &'a Option<String>,
    backend: &'a RenderBackendKind,
    replay: &'a RenderReplayPlan,
    inputs: &'a [RenderInputFingerprint],
    outputs: &'a [RenderOutputArtifact],
    sidecars: &'a [RenderSidecarFingerprint],
    limitations: &'a [RenderManifestLimitation],
    verification: &'a Option<RenderVerificationSummary>,
    metadata: &'a BTreeMap<String, String>,
}

fn stable_manifest_id(manifest: &RenderExecutionManifest) -> String {
    let stable = StableManifestView {
        schema_version: manifest.schema_version,
        awidat_version: &manifest.awidat_version,
        project_root: &manifest.project_root,
        project_hash: &manifest.project_hash,
        timeline_hash: &manifest.timeline_hash,
        backend: &manifest.backend,
        replay: &manifest.replay,
        inputs: &manifest.inputs,
        outputs: &manifest.outputs,
        sidecars: &manifest.sidecars,
        limitations: &manifest.limitations,
        verification: &manifest.verification,
        metadata: &manifest.metadata,
    };
    let bytes = serde_json::to_vec(&stable).unwrap_or_else(|error| error.to_string().into_bytes());
    hex_sha256(&bytes)
}

/// Return the manifest path for a render output path.
pub fn manifest_path_for_output(output_path: &Path) -> PathBuf {
    let stem = output_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("render");
    output_path.with_file_name(format!("{stem}.render-manifest.json"))
}

/// Fingerprint a file with SHA-256.
pub fn fingerprint_file(
    path: &Path,
    required: bool,
) -> Result<RenderInputFingerprint, RenderManifestError> {
    let mut file = fs::File::open(path).map_err(|source| RenderManifestError::Io {
        path: path.to_string_lossy().into_owned(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| RenderManifestError::Io {
        path: path.to_string_lossy().into_owned(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| RenderManifestError::Io {
                path: path.to_string_lossy().into_owned(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(RenderInputFingerprint {
        path: path.to_string_lossy().into_owned(),
        sha256: format!("{:x}", hasher.finalize()),
        size_bytes: metadata.len(),
        required,
    })
}

/// Build a planned output artifact entry.
pub fn output_artifact(path: &Path, required: bool) -> RenderOutputArtifact {
    RenderOutputArtifact {
        path: path.to_string_lossy().into_owned(),
        sha256: None,
        size_bytes: None,
        required,
    }
}

/// Build a render manifest limitation entry.
pub fn limitation(code: impl Into<String>, message: impl Into<String>) -> RenderManifestLimitation {
    RenderManifestLimitation {
        code: code.into(),
        message: message.into(),
    }
}

/// Write a render execution manifest as pretty JSON.
pub fn write_render_manifest(
    path: &Path,
    manifest: &RenderExecutionManifest,
) -> Result<(), RenderManifestError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| RenderManifestError::Io {
            path: parent.to_string_lossy().into_owned(),
            source,
        })?;
    }
    let mut bytes = serde_json::to_vec_pretty(manifest)?;
    bytes.push(b'\n');
    fs::write(path, bytes).map_err(|source| RenderManifestError::Io {
        path: path.to_string_lossy().into_owned(),
        source,
    })
}

/// Read a render execution manifest from disk.
pub fn read_render_manifest(path: &Path) -> Result<RenderExecutionManifest, RenderReplayError> {
    let bytes = fs::read(path).map_err(|source| RenderReplayError::ReadManifest {
        path: path.to_string_lossy().into_owned(),
        source: RenderManifestError::Io {
            path: path.to_string_lossy().into_owned(),
            source,
        },
    })?;
    serde_json::from_slice(&bytes).map_err(|source| RenderReplayError::ParseManifest {
        path: path.to_string_lossy().into_owned(),
        source,
    })
}

/// Validate that a manifest can be replayed before spawning a process.
pub fn validate_replay_manifest(
    manifest: &RenderExecutionManifest,
    manifest_path: &Path,
) -> Result<(), RenderReplayError> {
    let path = manifest_path.to_string_lossy().into_owned();
    if manifest.schema_version != RENDER_MANIFEST_SCHEMA_VERSION {
        return Err(RenderReplayError::UnsupportedSchema {
            path,
            version: manifest.schema_version,
        });
    }
    match &manifest.replay {
        RenderReplayPlan::Unsupported { reason } => Err(RenderReplayError::UnsupportedPlan {
            path,
            reason: reason.clone(),
        }),
        RenderReplayPlan::FfmpegArgv { argv, cwd } => {
            if argv.is_empty() {
                return Err(RenderReplayError::EmptyArgv { path });
            }
            if let Some(cwd) = cwd
                && !Path::new(cwd).is_dir()
            {
                return Err(RenderReplayError::MissingCwd {
                    path,
                    cwd: cwd.clone(),
                });
            }
            for output in &manifest.outputs {
                if output.required {
                    validate_render_output_path(
                        Path::new(&manifest.project_root),
                        Path::new(&output.path),
                        &[],
                        &[],
                        OutputPathPolicy::default(),
                    )
                    .map_err(|source| RenderReplayError::OutputPreflight {
                        path: manifest_path.to_string_lossy().into_owned(),
                        source,
                    })?;
                }
            }
            Ok(())
        }
    }
}

/// Replay an FFmpeg argv render manifest.
pub fn replay_render_manifest(
    manifest_path: &Path,
) -> Result<RenderReplayOutcome, RenderReplayError> {
    let manifest = read_render_manifest(manifest_path)?;
    validate_replay_manifest(&manifest, manifest_path)?;
    let RenderReplayPlan::FfmpegArgv { argv, cwd } = &manifest.replay else {
        unreachable!("validate_replay_manifest rejects unsupported plans")
    };
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let status = command
        .status()
        .map_err(|source| RenderReplayError::Spawn {
            path: manifest_path.to_string_lossy().into_owned(),
            source,
        })?;
    Ok(RenderReplayOutcome {
        manifest_path: manifest_path.to_path_buf(),
        output_paths: manifest
            .outputs
            .iter()
            .filter(|output| output.required)
            .map(|output| PathBuf::from(&output.path))
            .collect(),
        status,
    })
}

/// Build a planned manifest with the current UTC timestamp.
pub fn planned_at_now(input: RenderExecutionManifestInput) -> RenderExecutionManifest {
    RenderExecutionManifest::planned(RenderExecutionManifestInput {
        created_at: Utc::now().to_rfc3339(),
        ..input
    })
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;

    use super::*;

    fn planned_manifest(created_at: &str) -> RenderExecutionManifest {
        RenderExecutionManifest::planned(RenderExecutionManifestInput {
            created_at: created_at.into(),
            awidat_version: "0.1.0-test".into(),
            project_root: "/project".into(),
            project_hash: Some("project-hash".into()),
            timeline_hash: Some("timeline-hash".into()),
            backend: RenderBackendKind::TimelineFfmpegReencode,
            replay: RenderReplayPlan::FfmpegArgv {
                argv: vec![
                    "/usr/bin/ffmpeg".into(),
                    "-i".into(),
                    "/project/raw/a.mp4".into(),
                    "/project/renders/out.mp4".into(),
                ],
                cwd: Some("/project".into()),
            },
            inputs: vec![RenderInputFingerprint {
                path: "/project/raw/a.mp4".into(),
                sha256: "asset-hash".into(),
                size_bytes: 5,
                required: true,
            }],
            outputs: vec![RenderOutputArtifact {
                path: "/project/renders/out.mp4".into(),
                sha256: None,
                size_bytes: None,
                required: true,
            }],
            sidecars: Vec::new(),
            limitations: Vec::new(),
            verification: None,
            metadata: BTreeMap::new(),
        })
    }

    #[test]
    fn manifest_id_ignores_created_at() {
        let first = planned_manifest("2026-05-22T10:00:00Z");
        let second = planned_manifest("2026-05-22T11:00:00Z");
        assert_eq!(first.manifest_id, second.manifest_id);
    }

    #[test]
    fn content_sha256_hashes_file_bytes() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("input.bin");
        std::fs::write(&path, b"abc").unwrap();

        let fingerprint = fingerprint_file(&path, true).unwrap();

        assert_eq!(
            fingerprint.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(fingerprint.size_bytes, 3);
        assert_eq!(fingerprint.path, path.to_string_lossy());
        assert!(fingerprint.required);
    }

    #[test]
    fn manifest_path_sits_next_to_output() {
        let path =
            manifest_path_for_output(std::path::Path::new("/project/renders/final-youtube.mp4"));

        assert_eq!(
            path,
            std::path::Path::new("/project/renders/final-youtube.render-manifest.json")
        );
    }

    #[test]
    fn backend_classification_maps_render_scopes() {
        assert_eq!(
            RenderBackendKind::from_start_render_scope("preview"),
            Some(RenderBackendKind::AssetPreview)
        );
        assert_eq!(
            RenderBackendKind::from_start_render_scope("segment"),
            Some(RenderBackendKind::AssetSegmentStreamCopy)
        );
        assert_eq!(
            RenderBackendKind::from_start_render_scope("full"),
            Some(RenderBackendKind::AssetFullReencode)
        );
        assert_eq!(
            RenderBackendKind::from_start_render_scope("timeline"),
            Some(RenderBackendKind::TimelineFfmpegReencode)
        );
        assert_eq!(RenderBackendKind::from_start_render_scope("other"), None);
    }

    #[test]
    fn write_manifest_serializes_pretty_json() {
        let dir = tempdir().unwrap();
        let manifest_path = dir.path().join("out.render-manifest.json");
        let manifest = planned_manifest("2026-05-22T10:00:00Z");

        write_render_manifest(&manifest_path, &manifest).unwrap();

        let json = std::fs::read_to_string(&manifest_path).unwrap();
        assert!(json.contains("\"schema_version\": 1"));
        assert!(json.contains("\"backend\": \"timeline_ffmpeg_reencode\""));
        assert!(json.ends_with('\n'));
    }

    #[test]
    fn unsupported_replay_plan_fails_before_spawn() {
        let mut manifest = planned_manifest("2026-05-22T10:00:00Z");
        manifest.replay = RenderReplayPlan::Unsupported {
            reason: "raw-stream replay is not implemented".into(),
        };
        let dir = tempdir().unwrap();
        let path = dir.path().join("m.render-manifest.json");

        let err = validate_replay_manifest(&manifest, &path).unwrap_err();

        assert!(
            err.to_string()
                .contains("raw-stream replay is not implemented")
        );
    }

    #[test]
    fn empty_ffmpeg_argv_fails_before_spawn() {
        let mut manifest = planned_manifest("2026-05-22T10:00:00Z");
        manifest.replay = RenderReplayPlan::FfmpegArgv {
            argv: Vec::new(),
            cwd: None,
        };
        let dir = tempdir().unwrap();
        let path = dir.path().join("m.render-manifest.json");

        let err = validate_replay_manifest(&manifest, &path).unwrap_err();

        assert!(err.to_string().contains("argv is empty"));
    }

    #[test]
    fn missing_replay_cwd_fails_before_spawn() {
        let mut manifest = planned_manifest("2026-05-22T10:00:00Z");
        manifest.replay = RenderReplayPlan::FfmpegArgv {
            argv: vec!["ffmpeg".into(), "-version".into()],
            cwd: Some("/definitely/not/a/project".into()),
        };
        let dir = tempdir().unwrap();
        let path = dir.path().join("m.render-manifest.json");

        let err = validate_replay_manifest(&manifest, &path).unwrap_err();

        assert!(err.to_string().contains("cwd does not exist"));
    }
}
