//! Durable render execution manifests and replay support.
//!
//! The manifest captures the planned render backend, replay command,
//! input fingerprints, output artifacts, sidecars, and known limitations
//! for a render/export job. It is intentionally independent from the
//! agent tools so CLI, TUI, desktop, eval, and future remux paths can
//! share the same evidence format.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{OutputPathPolicy, validate_render_output_path};

/// Current render execution manifest schema version.
pub const RENDER_MANIFEST_SCHEMA_VERSION: u32 = 1;
const SAMPLED_FINGERPRINT_BYTES: u64 = 1024 * 1024;

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
    /// A required input or sidecar was missing during replay preflight.
    #[error("render manifest {path} required {kind} is missing: {artifact}")]
    MissingRequiredArtifact {
        /// Manifest path.
        path: String,
        /// Artifact kind.
        kind: &'static str,
        /// Artifact path.
        artifact: String,
    },
    /// A required input or sidecar no longer matches the manifest fingerprint.
    #[error("render manifest {path} required {kind} fingerprint mismatch at {artifact}")]
    FingerprintMismatch {
        /// Manifest path.
        path: String,
        /// Artifact kind.
        kind: &'static str,
        /// Artifact path.
        artifact: String,
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
    /// SHA-256 hex digest. See `fingerprint_kind` for the hashed scope.
    pub sha256: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Whether this input is required for replay.
    pub required: bool,
    /// Fingerprint scope used for this input.
    #[serde(
        default = "default_input_fingerprint_kind",
        skip_serializing_if = "is_content_sha256"
    )]
    pub fingerprint_kind: RenderInputFingerprintKind,
}

/// Scope used to fingerprint an input media file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderInputFingerprintKind {
    /// Full file content SHA-256.
    ContentSha256,
    /// Bounded identity hash: file metadata plus head/tail samples.
    SampledSha256,
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
///
/// Uses a 4 MB read buffer. Previous 16 KB buffer + software SHA-256
/// made 3.5 GB inputs hash for 50+ minutes on M-series and the render
/// appeared hung. 4 MB amortizes the per-block syscall + SHA round
/// overhead and brings real-world hash throughput from ~1 MB/s to
/// hundreds of MB/s.
pub fn fingerprint_file(
    path: &Path,
    required: bool,
) -> Result<RenderInputFingerprint, RenderManifestError> {
    let file = fs::File::open(path).map_err(|source| RenderManifestError::Io {
        path: path.to_string_lossy().into_owned(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| RenderManifestError::Io {
        path: path.to_string_lossy().into_owned(),
        source,
    })?;
    let mut reader = std::io::BufReader::with_capacity(4 * 1024 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 4 * 1024 * 1024];
    loop {
        let read = reader
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
        fingerprint_kind: RenderInputFingerprintKind::ContentSha256,
    })
}

/// Fingerprint a media input with a bounded SHA-256.
///
/// This is intended for large source media referenced by render manifests.
/// It hashes file size, mtime, and up to 1 MiB from both the head and tail
/// so export can spawn FFmpeg without first reading multi-GB inputs in full.
pub fn fingerprint_file_sampled(
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
    let modified = metadata
        .modified()
        .map_err(|source| RenderManifestError::Io {
            path: path.to_string_lossy().into_owned(),
            source,
        })?;
    let modified = modified
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let size = metadata.len();
    let mut hasher = Sha256::new();
    hasher.update(b"awidat-sampled-input-v1");
    hasher.update(size.to_le_bytes());
    hasher.update(modified.as_secs().to_le_bytes());
    hasher.update(modified.subsec_nanos().to_le_bytes());

    let head_len = size.min(SAMPLED_FINGERPRINT_BYTES) as usize;
    if head_len > 0 {
        let mut head = vec![0_u8; head_len];
        file.read_exact(&mut head)
            .map_err(|source| RenderManifestError::Io {
                path: path.to_string_lossy().into_owned(),
                source,
            })?;
        hasher.update(b"head");
        hasher.update(&head);
    }

    if size > SAMPLED_FINGERPRINT_BYTES {
        file.seek(SeekFrom::End(-(SAMPLED_FINGERPRINT_BYTES as i64)))
            .map_err(|source| RenderManifestError::Io {
                path: path.to_string_lossy().into_owned(),
                source,
            })?;
        let mut tail = vec![0_u8; SAMPLED_FINGERPRINT_BYTES as usize];
        file.read_exact(&mut tail)
            .map_err(|source| RenderManifestError::Io {
                path: path.to_string_lossy().into_owned(),
                source,
            })?;
        hasher.update(b"tail");
        hasher.update(&tail);
    }

    Ok(RenderInputFingerprint {
        path: path.to_string_lossy().into_owned(),
        sha256: format!("{:x}", hasher.finalize()),
        size_bytes: size,
        required,
        fingerprint_kind: RenderInputFingerprintKind::SampledSha256,
    })
}

/// Fingerprint render manifest inputs while caching repeated paths.
///
/// Timeline render specs can reference the same source asset once per
/// timeline segment. The manifest still records every input slot in order,
/// but hashing each unique resolved file once keeps render startup bounded.
pub fn fingerprint_manifest_inputs_sampled(
    project_root: &Path,
    input_paths: &[PathBuf],
) -> Result<Vec<RenderInputFingerprint>, RenderManifestError> {
    fingerprint_manifest_inputs_sampled_with(project_root, input_paths, |path| {
        fingerprint_file_sampled(path, true)
    })
}

fn fingerprint_manifest_inputs_sampled_with<F>(
    project_root: &Path,
    input_paths: &[PathBuf],
    mut fingerprint: F,
) -> Result<Vec<RenderInputFingerprint>, RenderManifestError>
where
    F: FnMut(&Path) -> Result<RenderInputFingerprint, RenderManifestError>,
{
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    let mut order = Vec::with_capacity(input_paths.len());
    for path in input_paths {
        let resolved = if path.is_absolute() {
            path.clone()
        } else {
            project_root.join(path)
        };
        order.push(resolved.clone());
        if seen.insert(resolved.clone()) {
            unique.push(resolved);
        }
    }

    let mut by_path = HashMap::new();
    for path in unique {
        let fingerprint = fingerprint(&path)?;
        by_path.insert(path, fingerprint);
    }

    order
        .into_iter()
        .map(|path| {
            by_path.get(&path).cloned().ok_or_else(|| {
                RenderManifestError::Json(serde_json::Error::io(io::Error::other(format!(
                    "missing cached render input fingerprint for {}",
                    path.display()
                ))))
            })
        })
        .collect()
}

/// Fingerprint ASS/subtitle sidecars referenced by FFmpeg `subtitles=` filters.
pub fn fingerprint_ffmpeg_subtitle_sidecars(
    argv: &[String],
) -> Result<Vec<RenderSidecarFingerprint>, RenderManifestError> {
    let mut sidecars = Vec::new();
    for path in ffmpeg_subtitle_filter_paths(argv) {
        let fingerprint = fingerprint_file(&path, true)?;
        sidecars.push(RenderSidecarFingerprint {
            path: fingerprint.path,
            sha256: fingerprint.sha256,
            size_bytes: fingerprint.size_bytes,
            required: fingerprint.required,
        });
    }
    Ok(sidecars)
}

/// Extract stable layout/readability metadata from ASS sidecars referenced by
/// FFmpeg `subtitles=` filters.
pub fn ass_sidecar_layout_metadata(
    argv: &[String],
) -> Result<std::collections::BTreeMap<String, String>, RenderManifestError> {
    let mut summary = AssSidecarLayoutSummary::default();
    for path in ffmpeg_subtitle_filter_paths(argv) {
        let contents =
            std::fs::read_to_string(&path).map_err(|source| RenderManifestError::Io {
                path: path.to_string_lossy().into_owned(),
                source,
            })?;
        summary.add_document(&contents);
        summary
            .sidecar_paths
            .push(path.to_string_lossy().into_owned());
    }
    Ok(summary.into_metadata())
}

#[derive(Debug, Default)]
struct AssSidecarLayoutSummary {
    sidecar_count: usize,
    playres_values: std::collections::BTreeSet<String>,
    wrapped_sidecar_count: usize,
    safe_area_sidecar_count: usize,
    karaoke_sidecar_count: usize,
    sidecar_paths: Vec<String>,
}

impl AssSidecarLayoutSummary {
    fn add_document(&mut self, contents: &str) {
        self.sidecar_count += 1;
        let playres_x = ass_header_value(contents, "PlayResX");
        let playres_y = ass_header_value(contents, "PlayResY");
        if let (Some(x), Some(y)) = (playres_x, playres_y) {
            self.playres_values.insert(format!("{x}x{y}"));
        }
        if contents.contains("\\N") {
            self.wrapped_sidecar_count += 1;
        }
        if contents.contains("\\k") {
            self.karaoke_sidecar_count += 1;
        }
        if contents.lines().any(style_line_has_safe_area_margins) {
            self.safe_area_sidecar_count += 1;
        }
    }

    fn into_metadata(self) -> std::collections::BTreeMap<String, String> {
        let mut metadata = std::collections::BTreeMap::new();
        if self.sidecar_count == 0 {
            return metadata;
        }
        metadata.insert(
            "libass_layout_sidecar_count".into(),
            self.sidecar_count.to_string(),
        );
        metadata.insert(
            "libass_layout_playres".into(),
            self.playres_values
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
        );
        metadata.insert(
            "libass_layout_wrapped_sidecar_count".into(),
            self.wrapped_sidecar_count.to_string(),
        );
        metadata.insert(
            "libass_layout_safe_area_sidecar_count".into(),
            self.safe_area_sidecar_count.to_string(),
        );
        metadata.insert(
            "libass_layout_karaoke_sidecar_count".into(),
            self.karaoke_sidecar_count.to_string(),
        );
        metadata.insert(
            "libass_layout_sidecar_paths".into(),
            self.sidecar_paths.join(","),
        );
        metadata
    }
}

fn ass_header_value<'a>(contents: &'a str, key: &str) -> Option<&'a str> {
    contents.lines().find_map(|line| {
        let (line_key, value) = line.split_once(':')?;
        line_key
            .trim()
            .eq_ignore_ascii_case(key)
            .then_some(value.trim())
            .filter(|value| !value.is_empty())
    })
}

fn style_line_has_safe_area_margins(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("Style:") else {
        return false;
    };
    let fields = rest.split(',').map(str::trim).collect::<Vec<_>>();
    if fields.len() < 23 {
        return false;
    }
    let margin_l = fields.get(19).and_then(|value| value.parse::<u32>().ok());
    let margin_r = fields.get(20).and_then(|value| value.parse::<u32>().ok());
    let margin_v = fields.get(21).and_then(|value| value.parse::<u32>().ok());
    matches!(
        (margin_l, margin_r, margin_v),
        (Some(left), Some(right), Some(vertical)) if left >= 80 && right >= 80 && vertical >= 54
    )
}

fn ffmpeg_subtitle_filter_paths(argv: &[String]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for arg in argv {
        let mut remaining = arg.as_str();
        while let Some(start) = remaining.find("subtitles=") {
            let value_start = start + "subtitles=".len();
            let value = &remaining[value_start..];
            let (path, consumed) = parse_ffmpeg_subtitles_value(value);
            if !path.as_os_str().is_empty() {
                paths.push(path);
            }
            let next_start = value_start.saturating_add(consumed).min(remaining.len());
            remaining = &remaining[next_start..];
        }
    }
    paths
}

fn parse_ffmpeg_subtitles_value(value: &str) -> (PathBuf, usize) {
    if let Some(stripped) = value.strip_prefix('\'') {
        let mut out = String::new();
        let mut escaped = false;
        for (offset, ch) in stripped.char_indices() {
            if escaped {
                out.push(ch);
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '\'' => return (PathBuf::from(out), offset + 2),
                _ => out.push(ch),
            }
        }
        return (PathBuf::from(out), value.len());
    }
    let end = value.find([':', ',', '[', ';', ' ']).unwrap_or(value.len());
    (PathBuf::from(&value[..end]), end)
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

/// Fill output artifact fingerprints for outputs that exist.
pub fn finalize_render_manifest_outputs(
    manifest: &mut RenderExecutionManifest,
) -> Result<(), RenderManifestError> {
    let project_root = Path::new(&manifest.project_root);
    for output in &mut manifest.outputs {
        let path = resolve_manifest_artifact_path(project_root, &output.path);
        if !path.exists() {
            if output.required {
                return Err(RenderManifestError::Io {
                    path: path.to_string_lossy().into_owned(),
                    source: io::Error::new(io::ErrorKind::NotFound, "required output missing"),
                });
            }
            continue;
        }
        let fingerprint = fingerprint_file(&path, output.required)?;
        output.sha256 = Some(fingerprint.sha256);
        output.size_bytes = Some(fingerprint.size_bytes);
    }
    Ok(())
}

/// Read, finalize, and rewrite a render manifest.
pub fn finalize_render_manifest_file(path: &Path) -> Result<(), RenderManifestError> {
    let bytes = fs::read(path).map_err(|source| RenderManifestError::Io {
        path: path.to_string_lossy().into_owned(),
        source,
    })?;
    let mut manifest: RenderExecutionManifest = serde_json::from_slice(&bytes)?;
    finalize_render_manifest_outputs(&mut manifest)?;
    write_render_manifest(path, &manifest)
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
    let mut manifest = manifest.clone();
    manifest.manifest_id = stable_manifest_id(&manifest);
    let mut bytes = serde_json::to_vec_pretty(&manifest)?;
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
            validate_required_fingerprints(
                &path,
                Path::new(&manifest.project_root),
                &manifest.inputs,
            )?;
            validate_required_sidecars(
                &path,
                Path::new(&manifest.project_root),
                &manifest.sidecars,
            )?;
            for output in &manifest.outputs {
                if output.required {
                    validate_render_output_path(
                        Path::new(&manifest.project_root),
                        Path::new(&output.path),
                        &[],
                        &[],
                        OutputPathPolicy {
                            allow_overwrite: true,
                        },
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

fn default_input_fingerprint_kind() -> RenderInputFingerprintKind {
    RenderInputFingerprintKind::ContentSha256
}

fn is_content_sha256(kind: &RenderInputFingerprintKind) -> bool {
    *kind == RenderInputFingerprintKind::ContentSha256
}

fn resolve_manifest_artifact_path(project_root: &Path, path: &str) -> PathBuf {
    let artifact_path = Path::new(path);
    if artifact_path.is_absolute() {
        artifact_path.to_path_buf()
    } else {
        project_root.join(artifact_path)
    }
}

fn validate_required_fingerprints(
    manifest_path: &str,
    project_root: &Path,
    inputs: &[RenderInputFingerprint],
) -> Result<(), RenderReplayError> {
    for input in inputs.iter().filter(|input| input.required) {
        let path = resolve_manifest_artifact_path(project_root, &input.path);
        if !path.is_file() {
            return Err(RenderReplayError::MissingRequiredArtifact {
                path: manifest_path.to_owned(),
                kind: "input",
                artifact: input.path.clone(),
            });
        }
        let live = match &input.fingerprint_kind {
            RenderInputFingerprintKind::ContentSha256 => fingerprint_file(&path, true),
            RenderInputFingerprintKind::SampledSha256 => fingerprint_file_sampled(&path, true),
        }
        .map_err(|_| RenderReplayError::MissingRequiredArtifact {
            path: manifest_path.to_owned(),
            kind: "input",
            artifact: input.path.clone(),
        })?;
        if live.sha256 != input.sha256 || live.size_bytes != input.size_bytes {
            return Err(RenderReplayError::FingerprintMismatch {
                path: manifest_path.to_owned(),
                kind: "input",
                artifact: input.path.clone(),
            });
        }
    }
    Ok(())
}

fn validate_required_sidecars(
    manifest_path: &str,
    project_root: &Path,
    sidecars: &[RenderSidecarFingerprint],
) -> Result<(), RenderReplayError> {
    for sidecar in sidecars.iter().filter(|sidecar| sidecar.required) {
        let path = resolve_manifest_artifact_path(project_root, &sidecar.path);
        if !path.is_file() {
            return Err(RenderReplayError::MissingRequiredArtifact {
                path: manifest_path.to_owned(),
                kind: "sidecar",
                artifact: sidecar.path.clone(),
            });
        }
        let live = fingerprint_file(&path, true).map_err(|_| {
            RenderReplayError::MissingRequiredArtifact {
                path: manifest_path.to_owned(),
                kind: "sidecar",
                artifact: sidecar.path.clone(),
            }
        })?;
        if live.sha256 != sidecar.sha256 || live.size_bytes != sidecar.size_bytes {
            return Err(RenderReplayError::FingerprintMismatch {
                path: manifest_path.to_owned(),
                kind: "sidecar",
                artifact: sidecar.path.clone(),
            });
        }
    }
    Ok(())
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
                fingerprint_kind: RenderInputFingerprintKind::ContentSha256,
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
        assert_eq!(
            fingerprint.fingerprint_kind,
            RenderInputFingerprintKind::ContentSha256
        );
    }

    #[test]
    fn sampled_sha256_records_bounded_input_kind() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("input.bin");
        std::fs::write(&path, vec![b'a'; (SAMPLED_FINGERPRINT_BYTES + 5) as usize]).unwrap();

        let fingerprint = fingerprint_file_sampled(&path, true).unwrap();

        assert_eq!(fingerprint.size_bytes, SAMPLED_FINGERPRINT_BYTES + 5);
        assert_eq!(fingerprint.path, path.to_string_lossy());
        assert!(fingerprint.required);
        assert_eq!(
            fingerprint.fingerprint_kind,
            RenderInputFingerprintKind::SampledSha256
        );
    }

    #[test]
    fn manifest_input_fingerprints_cache_duplicate_paths() {
        let dir = tempdir().unwrap();
        let project_root = dir.path();
        let media = project_root.join("raw/source.mp4");
        std::fs::create_dir_all(media.parent().unwrap()).unwrap();
        std::fs::write(&media, b"source").unwrap();
        let input_paths = vec![
            PathBuf::from("raw/source.mp4"),
            PathBuf::from("raw/source.mp4"),
            media,
        ];
        let mut calls = 0usize;

        let fingerprints =
            fingerprint_manifest_inputs_sampled_with(project_root, &input_paths, |path| {
                calls += 1;
                Ok(RenderInputFingerprint {
                    path: path.to_string_lossy().into_owned(),
                    sha256: format!("hash-{calls}"),
                    size_bytes: 6,
                    required: true,
                    fingerprint_kind: RenderInputFingerprintKind::SampledSha256,
                })
            })
            .unwrap();

        assert_eq!(calls, 1);
        assert_eq!(fingerprints.len(), 3);
        assert_eq!(fingerprints[0], fingerprints[1]);
        assert_eq!(fingerprints[0], fingerprints[2]);
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
    fn fingerprints_subtitle_sidecars_referenced_by_ffmpeg_args() {
        let dir = tempdir().unwrap();
        let ass_path = dir.path().join("caption.ass");
        std::fs::write(&ass_path, b"[Script Info]\n").unwrap();
        let argv = vec![
            "-filter_complex".to_string(),
            format!("[outv]subtitles='{}'[titled_v]", ass_path.to_string_lossy()),
        ];

        let sidecars = fingerprint_ffmpeg_subtitle_sidecars(&argv).unwrap();

        assert_eq!(sidecars.len(), 1);
        assert_eq!(sidecars[0].path, ass_path.to_string_lossy());
        assert!(sidecars[0].required);
        assert_eq!(sidecars[0].size_bytes, 14);
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

    #[test]
    fn replay_validation_allows_existing_required_output() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("renders/out.mp4");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&output, b"old output").unwrap();
        let mut manifest = planned_manifest("2026-05-22T10:00:00Z");
        manifest.project_root = dir.path().to_string_lossy().into_owned();
        manifest.inputs = Vec::new();
        manifest.outputs = vec![output_artifact(&output, true)];
        manifest.replay = RenderReplayPlan::FfmpegArgv {
            argv: vec!["ffmpeg".into(), "-version".into()],
            cwd: Some(dir.path().to_string_lossy().into_owned()),
        };
        let manifest_path = dir.path().join("out.render-manifest.json");

        validate_replay_manifest(&manifest, &manifest_path).unwrap();
    }

    #[test]
    fn replay_validation_rejects_changed_required_input() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("raw/a.mp4");
        let output = dir.path().join("renders/out.mp4");
        std::fs::create_dir_all(input.parent().unwrap()).unwrap();
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&input, b"first").unwrap();
        let fingerprint = fingerprint_file(&input, true).unwrap();
        std::fs::write(&input, b"changed").unwrap();

        let mut manifest = planned_manifest("2026-05-22T10:00:00Z");
        manifest.project_root = dir.path().to_string_lossy().into_owned();
        manifest.inputs = vec![fingerprint];
        manifest.outputs = vec![output_artifact(&output, true)];
        manifest.replay = RenderReplayPlan::FfmpegArgv {
            argv: vec!["ffmpeg".into(), "-version".into()],
            cwd: Some(dir.path().to_string_lossy().into_owned()),
        };
        let manifest_path = dir.path().join("out.render-manifest.json");

        let err = validate_replay_manifest(&manifest, &manifest_path).unwrap_err();

        assert!(err.to_string().contains("fingerprint mismatch"));
    }

    #[test]
    fn replay_validation_accepts_sampled_required_input() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("raw/a.mp4");
        let output = dir.path().join("renders/out.mp4");
        std::fs::create_dir_all(input.parent().unwrap()).unwrap();
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&input, b"source media").unwrap();
        let fingerprint = fingerprint_file_sampled(&input, true).unwrap();

        let mut manifest = planned_manifest("2026-05-22T10:00:00Z");
        manifest.project_root = dir.path().to_string_lossy().into_owned();
        manifest.inputs = vec![fingerprint];
        manifest.outputs = vec![output_artifact(&output, true)];
        manifest.replay = RenderReplayPlan::FfmpegArgv {
            argv: vec!["ffmpeg".into(), "-version".into()],
            cwd: Some(dir.path().to_string_lossy().into_owned()),
        };
        let manifest_path = dir.path().join("out.render-manifest.json");

        validate_replay_manifest(&manifest, &manifest_path).unwrap();
    }

    #[test]
    fn finalize_manifest_records_existing_output_hashes() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("renders/out.mp4");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&output, b"rendered").unwrap();
        let mut manifest = planned_manifest("2026-05-22T10:00:00Z");
        manifest.project_root = dir.path().to_string_lossy().into_owned();
        manifest.outputs = vec![output_artifact(&output, true)];

        finalize_render_manifest_outputs(&mut manifest).unwrap();

        assert_eq!(manifest.outputs[0].size_bytes, Some(8));
        assert_eq!(
            manifest.outputs[0].sha256.as_deref(),
            Some("69d0044d65bc72753132efe821effd54c8072b5f75703772caa15a13d400dc5a")
        );
    }

    #[test]
    fn manifest_write_refreshes_manifest_id_after_output_finalization() {
        let dir = tempdir().unwrap();
        let output = dir.path().join("renders/out.mp4");
        let manifest_path = dir.path().join("renders/out.render-manifest.json");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&output, b"rendered").unwrap();
        let mut manifest = planned_manifest("2026-05-22T10:00:00Z");
        manifest.project_root = dir.path().to_string_lossy().into_owned();
        manifest.outputs = vec![output_artifact(&output, true)];
        let planned_id = manifest.manifest_id.clone();
        write_render_manifest(&manifest_path, &manifest).unwrap();

        finalize_render_manifest_file(&manifest_path).unwrap();

        let finalized = read_render_manifest(&manifest_path).unwrap();
        assert_ne!(finalized.manifest_id, planned_id);
        assert_eq!(finalized.manifest_id, stable_manifest_id(&finalized));
    }
}
