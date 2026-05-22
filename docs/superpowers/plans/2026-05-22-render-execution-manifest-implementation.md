# Render Execution Manifest Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a manifest-backed render execution spine that records existing render/export jobs and replays FFmpeg argv manifests.

**Architecture:** Put manifest schema, hashing, writing, and replay in `awidat-render` so every render path can share one execution record. Keep `awidat-core` tool changes small: classify the existing render job, call render helpers, return `manifest_path`, and leave `JobManager` behavior unchanged.

**Tech Stack:** Rust 2024, `serde`, `serde_json`, `sha2`, `chrono`, `thiserror`, `std::process::Command`, existing `awidat_render::validate_render_output_path`, existing `awidat_render::JobManager`.

---

## File Structure

- Create `crates/render/src/manifest.rs`: manifest types, stable manifest IDs, SHA-256 file fingerprints, manifest path naming, JSON writes, FFmpeg argv replay validation and execution.
- Modify `crates/render/src/lib.rs`: export the manifest module public API.
- Modify `crates/render/Cargo.toml`: add workspace `sha2` dependency.
- Modify `crates/core/src/tools/start_render.rs`: build and write manifests before starting background render jobs; include `manifest_path` in JSON response.
- Modify `crates/core/src/tools/export_package.rs`: build and write package export manifests before starting final package render jobs; include `manifest_path` in JSON response and metadata.
- Modify `crates/cli/src/main.rs`: add `awidat replay-render <manifest>` subcommand that invokes render replay.
- Modify `crates/eval/src/acceptance/media.rs`: have shared acceptance renders emit a render execution manifest.
- Modify `crates/eval/src/acceptance/artifacts.rs`: include `render_manifest` in required artifact bundle entries.
- Modify `crates/eval/src/acceptance/scorecard.rs`: expose the render manifest path in scorecard artifacts.
- Modify `crates/eval/src/acceptance/runner.rs`: pass the render manifest path through scorecard and artifact bundle generation.

## Task 1: Render Manifest Module

**Files:**
- Create: `crates/render/src/manifest.rs`
- Modify: `crates/render/src/lib.rs`
- Modify: `crates/render/Cargo.toml`
- Test: `crates/render/src/manifest.rs`

- [ ] **Step 1: Add the render crate SHA-256 dependency**

Edit `crates/render/Cargo.toml` and add the workspace dependency in `[dependencies]`:

```toml
sha2 = { workspace = true }
```

- [ ] **Step 2: Write failing manifest tests**

Create `crates/render/src/manifest.rs` with only the test module below. The tests intentionally reference symbols that do not exist yet.

```rust
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
        let path = manifest_path_for_output(std::path::Path::new(
            "/project/renders/final-youtube.mp4",
        ));

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
}
```

- [ ] **Step 3: Run the failing tests**

Run:

```bash
cargo test -p awidat-render manifest -- --nocapture
```

Expected: FAIL with unresolved `RenderExecutionManifest`, `RenderBackendKind`, `fingerprint_file`, `manifest_path_for_output`, and `write_render_manifest` symbols.

- [ ] **Step 4: Implement manifest types and helpers**

Replace `crates/render/src/manifest.rs` with this implementation, preserving the tests at the bottom:

```rust
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const RENDER_MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum RenderManifestError {
    #[error("render manifest IO failed at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("render manifest JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderExecutionManifest {
    pub schema_version: u32,
    pub manifest_id: String,
    pub created_at: String,
    pub awidat_version: String,
    pub project_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline_hash: Option<String>,
    pub backend: RenderBackendKind,
    pub replay: RenderReplayPlan,
    pub inputs: Vec<RenderInputFingerprint>,
    pub outputs: Vec<RenderOutputArtifact>,
    pub sidecars: Vec<RenderSidecarFingerprint>,
    pub limitations: Vec<RenderManifestLimitation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<RenderVerificationSummary>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct RenderExecutionManifestInput {
    pub created_at: String,
    pub awidat_version: String,
    pub project_root: String,
    pub project_hash: Option<String>,
    pub timeline_hash: Option<String>,
    pub backend: RenderBackendKind,
    pub replay: RenderReplayPlan,
    pub inputs: Vec<RenderInputFingerprint>,
    pub outputs: Vec<RenderOutputArtifact>,
    pub sidecars: Vec<RenderSidecarFingerprint>,
    pub limitations: Vec<RenderManifestLimitation>,
    pub verification: Option<RenderVerificationSummary>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RenderBackendKind {
    AssetPreview,
    AssetSegmentStreamCopy,
    AssetFullReencode,
    TimelineFfmpegReencode,
    TimelineRawStreamGpu,
    PackageExport,
    StreamExportRemux,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderReplayPlan {
    FfmpegArgv {
        argv: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    Unsupported {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderInputFingerprint {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderOutputArtifact {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderSidecarFingerprint {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderManifestLimitation {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderVerificationSummary {
    pub status: String,
    pub report_path: String,
}

impl RenderExecutionManifest {
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
    let bytes = serde_json::to_vec(&stable).unwrap_or_default();
    hex_sha256(&bytes)
}

pub fn manifest_path_for_output(output_path: &Path) -> PathBuf {
    let stem = output_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("render");
    output_path.with_file_name(format!("{stem}.render-manifest.json"))
}

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

pub fn output_artifact(path: &Path, required: bool) -> RenderOutputArtifact {
    RenderOutputArtifact {
        path: path.to_string_lossy().into_owned(),
        sha256: None,
        size_bytes: None,
        required,
    }
}

pub fn limitation(code: impl Into<String>, message: impl Into<String>) -> RenderManifestLimitation {
    RenderManifestLimitation {
        code: code.into(),
        message: message.into(),
    }
}

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
```

Then append the test module from Step 2 below the implementation.

- [ ] **Step 5: Export the manifest API**

Modify `crates/render/src/lib.rs`:

```rust
pub mod manifest;
```

Add public re-exports near the other `pub use` statements:

```rust
pub use manifest::{
    RENDER_MANIFEST_SCHEMA_VERSION, RenderBackendKind, RenderExecutionManifest,
    RenderExecutionManifestInput, RenderInputFingerprint, RenderManifestError,
    RenderManifestLimitation, RenderOutputArtifact, RenderReplayPlan, RenderSidecarFingerprint,
    RenderVerificationSummary, fingerprint_file, limitation, manifest_path_for_output,
    output_artifact, planned_at_now, write_render_manifest,
};
```

- [ ] **Step 6: Run manifest tests**

Run:

```bash
cargo test -p awidat-render manifest -- --nocapture
```

Expected: PASS for all manifest tests.

- [ ] **Step 7: Commit Task 1**

Run:

```bash
git add crates/render/Cargo.toml crates/render/src/lib.rs crates/render/src/manifest.rs
git commit -m "feat: add render execution manifest model"
```

## Task 2: FFmpeg Manifest Replay

**Files:**
- Modify: `crates/render/src/manifest.rs`
- Test: `crates/render/src/manifest.rs`

- [ ] **Step 1: Add failing replay tests**

Add these tests to the existing `#[cfg(test)] mod tests` in `crates/render/src/manifest.rs`:

```rust
#[test]
fn unsupported_replay_plan_fails_before_spawn() {
    let mut manifest = planned_manifest("2026-05-22T10:00:00Z");
    manifest.replay = RenderReplayPlan::Unsupported {
        reason: "raw-stream replay is not implemented".into(),
    };
    let path = tempdir().unwrap().path().join("m.render-manifest.json");

    let err = validate_replay_manifest(&manifest, &path).unwrap_err();

    assert!(err.to_string().contains("raw-stream replay is not implemented"));
}

#[test]
fn empty_ffmpeg_argv_fails_before_spawn() {
    let mut manifest = planned_manifest("2026-05-22T10:00:00Z");
    manifest.replay = RenderReplayPlan::FfmpegArgv {
        argv: Vec::new(),
        cwd: None,
    };
    let path = tempdir().unwrap().path().join("m.render-manifest.json");

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
    let path = tempdir().unwrap().path().join("m.render-manifest.json");

    let err = validate_replay_manifest(&manifest, &path).unwrap_err();

    assert!(err.to_string().contains("cwd does not exist"));
}
```

- [ ] **Step 2: Run the failing replay tests**

Run:

```bash
cargo test -p awidat-render manifest::tests::unsupported_replay_plan_fails_before_spawn manifest::tests::empty_ffmpeg_argv_fails_before_spawn manifest::tests::missing_replay_cwd_fails_before_spawn -- --nocapture
```

Expected: FAIL with missing `validate_replay_manifest`.

- [ ] **Step 3: Implement replay validation and execution**

Add this code to `crates/render/src/manifest.rs`:

```rust
use std::process::{Command, ExitStatus};

use crate::{OutputPathPolicy, validate_render_output_path};

#[derive(Debug, Error)]
pub enum RenderReplayError {
    #[error("read render manifest {path}: {source}")]
    ReadManifest {
        path: String,
        #[source]
        source: RenderManifestError,
    },
    #[error("parse render manifest {path}: {source}")]
    ParseManifest {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("render manifest {path} has unsupported schema version {version}")]
    UnsupportedSchema { path: String, version: u32 },
    #[error("render manifest {path} cannot be replayed: {reason}")]
    UnsupportedPlan { path: String, reason: String },
    #[error("render manifest {path} ffmpeg argv is empty")]
    EmptyArgv { path: String },
    #[error("render manifest {path} replay cwd does not exist: {cwd}")]
    MissingCwd { path: String, cwd: String },
    #[error("render manifest {path} output path preflight failed: {source}")]
    OutputPreflight {
        path: String,
        #[source]
        source: crate::OutputPathSafetyError,
    },
    #[error("render manifest {path} ffmpeg replay failed to spawn: {source}")]
    Spawn {
        path: String,
        #[source]
        source: io::Error,
    },
}

#[derive(Debug)]
pub struct RenderReplayOutcome {
    pub manifest_path: PathBuf,
    pub output_paths: Vec<PathBuf>,
    pub status: ExitStatus,
}

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
    let status = command.status().map_err(|source| RenderReplayError::Spawn {
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
```

- [ ] **Step 4: Export replay API**

Modify the `pub use manifest::{...};` list in `crates/render/src/lib.rs` to include:

```rust
RenderReplayError, RenderReplayOutcome, read_render_manifest, replay_render_manifest,
validate_replay_manifest,
```

- [ ] **Step 5: Run replay tests**

Run:

```bash
cargo test -p awidat-render manifest -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit Task 2**

Run:

```bash
git add crates/render/src/lib.rs crates/render/src/manifest.rs
git commit -m "feat: replay ffmpeg render manifests"
```

## Task 3: `start_render` Manifest Integration

**Files:**
- Modify: `crates/core/src/tools/start_render.rs`
- Test: `crates/core/src/tools/start_render.rs`

- [ ] **Step 1: Add failing `start_render` manifest helper tests**

Add these tests inside the existing `#[cfg(test)] mod tests` in `crates/core/src/tools/start_render.rs`:

```rust
#[test]
fn start_render_manifest_for_preview_records_ffmpeg_replay() {
    let dir = tempfile::tempdir().unwrap();
    let asset = dir.path().join("raw/x.mp4");
    std::fs::create_dir_all(asset.parent().unwrap()).unwrap();
    std::fs::write(&asset, b"asset").unwrap();
    let output = dir.path().join("renders/preview-x-120000.mp4");
    let argv = vec!["-i".into(), asset.to_string_lossy().into_owned(), output.to_string_lossy().into_owned()];

    let built = build_start_render_manifest(StartRenderManifestInput {
        project_root: dir.path(),
        scope: "preview",
        asset_path: Some(&asset),
        output_path: &output,
        argv: &argv,
        limitations: &[],
        metadata: serde_json::json!({"scope": "preview"}),
    })
    .unwrap();

    assert_eq!(built.manifest.backend, awidat_render::RenderBackendKind::AssetPreview);
    assert_eq!(built.manifest_path, awidat_render::manifest_path_for_output(&output));
    assert_eq!(built.manifest.inputs.len(), 1);
    assert!(matches!(
        built.manifest.replay,
        awidat_render::RenderReplayPlan::FfmpegArgv { .. }
    ));
}

#[test]
fn start_render_manifest_for_timeline_hashes_project_file() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("project.otio.json");
    std::fs::write(&project_path, br#"{"OTIO_SCHEMA":"Timeline.1"}"#).unwrap();
    let output = dir.path().join("renders/timeline.mp4");
    let argv = vec!["-i".into(), "raw/x.mp4".into(), output.to_string_lossy().into_owned()];

    let built = build_start_render_manifest(StartRenderManifestInput {
        project_root: dir.path(),
        scope: "timeline",
        asset_path: None,
        output_path: &output,
        argv: &argv,
        limitations: &[],
        metadata: serde_json::json!({"scope": "timeline"}),
    })
    .unwrap();

    assert_eq!(
        built.manifest.backend,
        awidat_render::RenderBackendKind::TimelineFfmpegReencode
    );
    assert!(built.manifest.project_hash.is_some());
    assert!(built.manifest.timeline_hash.is_some());
}
```

- [ ] **Step 2: Run failing `start_render` tests**

Run:

```bash
cargo test -p awidat-core start_render_manifest -- --nocapture
```

Expected: FAIL with missing `StartRenderManifestInput` and `build_start_render_manifest`.

- [ ] **Step 3: Add helper structs and manifest builder**

Add these imports near the top of `crates/core/src/tools/start_render.rs`:

```rust
use std::collections::BTreeMap;
```

Add this helper code above `asset_stem`:

```rust
struct StartRenderManifestInput<'a> {
    project_root: &'a Path,
    scope: &'a str,
    asset_path: Option<&'a Path>,
    output_path: &'a Path,
    argv: &'a [String],
    limitations: &'a [RenderPlanLimitation],
    metadata: serde_json::Value,
}

struct BuiltStartRenderManifest {
    manifest_path: PathBuf,
    manifest: awidat_render::RenderExecutionManifest,
}

fn build_start_render_manifest(
    input: StartRenderManifestInput<'_>,
) -> Result<BuiltStartRenderManifest, FunctionCallError> {
    let backend = awidat_render::RenderBackendKind::from_start_render_scope(input.scope)
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "start_render: scope '{}' not recognized for render manifest",
                input.scope
            ))
        })?;
    let mut inputs = Vec::new();
    if let Some(asset_path) = input.asset_path {
        inputs.push(awidat_render::fingerprint_file(asset_path, true).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_render: failed to fingerprint input {}: {e}",
                asset_path.display()
            ))
        })?);
    }
    let project_otio_path = input.project_root.join("project.otio.json");
    let project_hash = optional_file_hash(&project_otio_path)?;
    let timeline_hash = if input.scope == "timeline" {
        project_hash.clone()
    } else {
        None
    };
    let ffmpeg_path = awidat_render::ffmpeg_path().map_err(|e| {
        FunctionCallError::RespondToModel(format!("start_render: failed to locate ffmpeg: {e}"))
    })?;
    let mut replay_argv = vec![ffmpeg_path.to_string_lossy().into_owned()];
    replay_argv.extend(input.argv.iter().cloned());
    let limitations = input
        .limitations
        .iter()
        .map(|limitation| {
            awidat_render::limitation(
                limitation.code.clone(),
                limitation.message.clone(),
            )
        })
        .collect();
    let metadata = json_object_to_string_map(input.metadata);
    let manifest = awidat_render::planned_at_now(awidat_render::RenderExecutionManifestInput {
        created_at: String::new(),
        awidat_version: env!("CARGO_PKG_VERSION").into(),
        project_root: input.project_root.to_string_lossy().into_owned(),
        project_hash,
        timeline_hash,
        backend,
        replay: awidat_render::RenderReplayPlan::FfmpegArgv {
            argv: replay_argv,
            cwd: Some(input.project_root.to_string_lossy().into_owned()),
        },
        inputs,
        outputs: vec![awidat_render::output_artifact(input.output_path, true)],
        sidecars: Vec::new(),
        limitations,
        verification: None,
        metadata,
    });
    Ok(BuiltStartRenderManifest {
        manifest_path: awidat_render::manifest_path_for_output(input.output_path),
        manifest,
    })
}

fn optional_file_hash(path: &Path) -> Result<Option<String>, FunctionCallError> {
    if !path.is_file() {
        return Ok(None);
    }
    awidat_render::fingerprint_file(path, true)
        .map(|fingerprint| Some(fingerprint.sha256))
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "start_render: failed to fingerprint {}: {e}",
                path.display()
            ))
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
```

- [ ] **Step 4: Write manifests before starting jobs**

In `StartRenderTool::handle`, change the branch tuple from:

```rust
let (argv, total_duration_s, asset_label, output_path, limitations) = if args.scope
```

to:

```rust
let (argv, total_duration_s, asset_label, output_path, limitations, asset_path_for_manifest) = if args.scope
```

In the timeline branch tuple, add `None` as the final element. In the asset branch tuple, add `Some(asset_path.clone())` as the final element.

Immediately before creating `let spec = RenderJobSpec { ... }`, insert:

```rust
let manifest = build_start_render_manifest(StartRenderManifestInput {
    project_root: &ctx.project_root,
    scope: &args.scope,
    asset_path: asset_path_for_manifest.as_deref(),
    output_path: &output_path,
    argv: &argv,
    limitations: &limitations,
    metadata: serde_json::json!({
        "scope": args.scope,
        "asset": asset_label,
        "guide": args.guide.as_ref().map(|guide| serde_json::json!({
            "track_id": guide.track_id,
            "marker_id": guide.marker_id,
        })),
        "preset": args.preset,
    }),
})?;
awidat_render::write_render_manifest(&manifest.manifest_path, &manifest.manifest).map_err(|e| {
    FunctionCallError::RespondToModel(format!(
        "start_render: failed to write render manifest {}: {e}",
        manifest.manifest_path.display()
    ))
})?;
```

Add `"manifest_path"` to the response body:

```rust
"manifest_path": manifest.manifest_path.display().to_string(),
```

- [ ] **Step 5: Run `start_render` manifest tests**

Run:

```bash
cargo test -p awidat-core start_render_manifest -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Run existing `start_render` tests**

Run:

```bash
cargo test -p awidat-core start_render -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit Task 3**

Run:

```bash
git add crates/core/src/tools/start_render.rs
git commit -m "feat: write manifests for start_render jobs"
```

## Task 4: `export_package` Manifest Integration

**Files:**
- Modify: `crates/core/src/tools/export_package.rs`
- Test: `crates/core/src/tools/export_package.rs`

- [ ] **Step 1: Add failing package manifest helper test**

Add this test module to the end of `crates/core/src/tools/export_package.rs` if no test module exists, or append the test to the existing module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_export_manifest_records_preset_and_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path().join("project.otio.json");
        std::fs::write(&project_path, br#"{"OTIO_SCHEMA":"Timeline.1"}"#).unwrap();
        let package_dir = dir.path().join("renders/package");
        std::fs::create_dir_all(&package_dir).unwrap();
        let mp4_path = package_dir.join("final-youtube.mp4");
        let srt_path = package_dir.join("final-youtube.srt");
        std::fs::write(&srt_path, b"1\n00:00:00,000 --> 00:00:01,000\nHi\n").unwrap();
        let argv = vec!["-i".into(), "raw/x.mp4".into(), mp4_path.to_string_lossy().into_owned()];
        let export_preset = ExportPreset::youtube_h264();

        let built = build_package_render_manifest(PackageRenderManifestInput {
            project_root: dir.path(),
            output_path: &mp4_path,
            argv: &argv,
            sidecar_paths: &[srt_path.as_path()],
            limitations: &[],
            format: "youtube",
            export_preset_id: &export_preset.id,
            hardware_acceleration: "off",
        })
        .unwrap();

        assert_eq!(
            built.manifest.backend,
            awidat_render::RenderBackendKind::PackageExport
        );
        assert_eq!(built.manifest.metadata["format"], "youtube");
        assert_eq!(built.manifest.metadata["export_preset_id"], export_preset.id);
        assert_eq!(built.manifest.sidecars.len(), 1);
    }
}
```

- [ ] **Step 2: Run the failing package manifest test**

Run:

```bash
cargo test -p awidat-core package_export_manifest -- --nocapture
```

Expected: FAIL with missing `PackageRenderManifestInput` and `build_package_render_manifest`.

- [ ] **Step 3: Add package manifest helper**

Add `use std::collections::BTreeMap;` near the top of `crates/core/src/tools/export_package.rs`.

Add this helper code above `parse_hardware_acceleration`:

```rust
struct PackageRenderManifestInput<'a> {
    project_root: &'a Path,
    output_path: &'a Path,
    argv: &'a [String],
    sidecar_paths: &'a [&'a Path],
    limitations: &'a [awidat_render::RenderPlanLimitation],
    format: &'a str,
    export_preset_id: &'a str,
    hardware_acceleration: &'a str,
}

struct BuiltPackageRenderManifest {
    manifest_path: PathBuf,
    manifest: awidat_render::RenderExecutionManifest,
}

fn build_package_render_manifest(
    input: PackageRenderManifestInput<'_>,
) -> Result<BuiltPackageRenderManifest, FunctionCallError> {
    let project_path = input.project_root.join("project.otio.json");
    let project_hash = optional_file_hash(&project_path)?;
    let ffmpeg_path = awidat_render::ffmpeg_path().map_err(|e| {
        FunctionCallError::RespondToModel(format!("export_package: failed to locate ffmpeg: {e}"))
    })?;
    let mut replay_argv = vec![ffmpeg_path.to_string_lossy().into_owned()];
    replay_argv.extend(input.argv.iter().cloned());
    let mut sidecars = Vec::new();
    for path in input.sidecar_paths {
        let fingerprint = awidat_render::fingerprint_file(path, true).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "export_package: failed to fingerprint sidecar {}: {e}",
                path.display()
            ))
        })?;
        sidecars.push(awidat_render::RenderSidecarFingerprint {
            path: fingerprint.path,
            sha256: fingerprint.sha256,
            size_bytes: fingerprint.size_bytes,
            required: fingerprint.required,
        });
    }
    let limitations = input
        .limitations
        .iter()
        .map(|limitation| awidat_render::limitation(limitation.code.clone(), limitation.message.clone()))
        .collect();
    let metadata = BTreeMap::from([
        ("format".into(), input.format.into()),
        ("export_preset_id".into(), input.export_preset_id.into()),
        ("hardware_acceleration".into(), input.hardware_acceleration.into()),
    ]);
    let manifest = awidat_render::planned_at_now(awidat_render::RenderExecutionManifestInput {
        created_at: String::new(),
        awidat_version: env!("CARGO_PKG_VERSION").into(),
        project_root: input.project_root.to_string_lossy().into_owned(),
        project_hash: project_hash.clone(),
        timeline_hash: project_hash,
        backend: awidat_render::RenderBackendKind::PackageExport,
        replay: awidat_render::RenderReplayPlan::FfmpegArgv {
            argv: replay_argv,
            cwd: Some(input.project_root.to_string_lossy().into_owned()),
        },
        inputs: Vec::new(),
        outputs: vec![awidat_render::output_artifact(input.output_path, true)],
        sidecars,
        limitations,
        verification: None,
        metadata,
    });
    Ok(BuiltPackageRenderManifest {
        manifest_path: awidat_render::manifest_path_for_output(input.output_path),
        manifest,
    })
}

fn optional_file_hash(path: &Path) -> Result<Option<String>, FunctionCallError> {
    if !path.is_file() {
        return Ok(None);
    }
    awidat_render::fingerprint_file(path, true)
        .map(|fingerprint| Some(fingerprint.sha256))
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "export_package: failed to fingerprint {}: {e}",
                path.display()
            ))
        })
}
```

- [ ] **Step 4: Write the package manifest before starting the render job**

After `spec = awidat_render::professional::apply_export_preset_to_spec(...) ?;` and before `let job_id = ctx.job_manager.start(...)`, insert:

```rust
let hardware_label = format!("{hardware_policy:?}").to_lowercase();
let sidecar_paths = [
    srt_path.as_path(),
    vtt_path.as_path(),
    chapter_path.as_path(),
    thumbnail_path.as_path(),
    preflight_path.as_path(),
    recipe_json_path.as_path(),
    recipe_md_path.as_path(),
];
let render_manifest = build_package_render_manifest(PackageRenderManifestInput {
    project_root: &ctx.project_root,
    output_path: &mp4_path,
    argv: &spec.args,
    sidecar_paths: &sidecar_paths,
    limitations: &spec.limitations,
    format: &args.format,
    export_preset_id: &export_preset.id,
    hardware_acceleration: &hardware_label,
})?;
awidat_render::write_render_manifest(&render_manifest.manifest_path, &render_manifest.manifest)
    .map_err(|e| {
        FunctionCallError::RespondToModel(format!(
            "export_package: failed to write render manifest {}: {e}",
            render_manifest.manifest_path.display()
        ))
    })?;
```

In the metadata JSON, add:

```rust
"render_manifest": render_manifest.manifest_path,
```

In the response body, add:

```rust
"manifest_path": render_manifest.manifest_path.display().to_string(),
```

Replace the metadata `"hardware_acceleration"` expression with `hardware_label` to avoid formatting the same value twice.

- [ ] **Step 5: Run package manifest tests**

Run:

```bash
cargo test -p awidat-core package_export_manifest -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Run export package tests**

Run:

```bash
cargo test -p awidat-core export_package -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit Task 4**

Run:

```bash
git add crates/core/src/tools/export_package.rs
git commit -m "feat: write manifests for package exports"
```

## Task 5: CLI Replay Command

**Files:**
- Modify: `crates/cli/src/main.rs`
- Test: CLI help output through `cargo run`

- [ ] **Step 1: Add the `ReplayRender` subcommand**

In `crates/cli/src/main.rs`, add this variant to the `Command` enum:

```rust
/// Replay an FFmpeg render execution manifest.
ReplayRender {
    /// Path to a `.render-manifest.json` file.
    manifest: PathBuf,
},
```

- [ ] **Step 2: Wire the command in `main`**

Add this match arm in `fn main()`:

```rust
Command::ReplayRender { manifest } => cmd_replay_render(&manifest),
```

Add this function near `print_version`:

```rust
fn cmd_replay_render(manifest: &std::path::Path) -> Result<()> {
    let outcome = awidat_render::replay_render_manifest(manifest)
        .with_context(|| format!("failed to replay render manifest {}", manifest.display()))?;
    println!("replay manifest: {}", outcome.manifest_path.display());
    println!("status: {}", outcome.status);
    for output in outcome.output_paths {
        println!("output: {}", output.display());
    }
    Ok(())
}
```

- [ ] **Step 3: Verify the command is exposed**

Run:

```bash
cargo run -p awidat-cli -- --help
```

Expected: output includes `replay-render`.

- [ ] **Step 4: Verify unsupported manifests fail clearly**

Run:

```bash
tmpdir="$(mktemp -d)"
cat > "$tmpdir/unsupported.render-manifest.json" <<'JSON'
{
  "schema_version": 1,
  "manifest_id": "test",
  "created_at": "2026-05-22T00:00:00Z",
  "awidat_version": "test",
  "project_root": "/tmp",
  "backend": "timeline_raw_stream_gpu",
  "replay": { "kind": "unsupported", "reason": "raw-stream replay is not implemented" },
  "inputs": [],
  "outputs": [],
  "sidecars": [],
  "limitations": []
}
JSON
cargo run -p awidat-cli -- replay-render "$tmpdir/unsupported.render-manifest.json"
```

Expected: command exits non-zero and stderr contains `raw-stream replay is not implemented`.

- [ ] **Step 5: Commit Task 5**

Run:

```bash
git add crates/cli/src/main.rs
git commit -m "feat: add render manifest replay command"
```

## Task 6: Acceptance Artifact Manifest Wiring

**Files:**
- Modify: `crates/eval/src/acceptance/media.rs`
- Modify: `crates/eval/src/acceptance/artifacts.rs`
- Modify: `crates/eval/src/acceptance/scorecard.rs`
- Modify: `crates/eval/src/acceptance/runner.rs`
- Test: existing acceptance unit tests plus new artifact tests

- [ ] **Step 1: Add the render manifest path to media output**

Modify `RenderOutput` in `crates/eval/src/acceptance/media.rs` to include:

```rust
pub(crate) struct RenderOutput {
    pub(crate) output_path: PathBuf,
    pub(crate) render_manifest_path: PathBuf,
    pub(crate) driver: String,
}
```

In the external CLI path, after `let output_path = newest_render(project_root)?;`, set:

```rust
let render_manifest_path = awidat_render::manifest_path_for_output(&output_path);
```

Return it in `RenderOutput`.

In the shared render path, after `run_render_spec(&spec)?;`, call the helper added in Step 2 and return its path.

- [ ] **Step 2: Add shared acceptance manifest writing**

Add this function to `crates/eval/src/acceptance/media.rs`:

```rust
fn write_acceptance_render_manifest(
    project_root: &Path,
    spec: &RenderJobSpec,
) -> Result<PathBuf> {
    let project_path = project_root.join("project.otio.json");
    let project_hash = if project_path.is_file() {
        Some(
            awidat_render::fingerprint_file(&project_path, true)
                .with_context(|| format!("fingerprint {}", project_path.display()))?
                .sha256,
        )
    } else {
        None
    };
    let ffmpeg = ffmpeg_path().context("locate ffmpeg for render manifest")?;
    let mut argv = vec![ffmpeg.to_string_lossy().into_owned()];
    argv.extend(spec.args.iter().cloned());
    let manifest = awidat_render::planned_at_now(awidat_render::RenderExecutionManifestInput {
        created_at: String::new(),
        awidat_version: env!("CARGO_PKG_VERSION").into(),
        project_root: project_root.to_string_lossy().into_owned(),
        project_hash: project_hash.clone(),
        timeline_hash: project_hash,
        backend: awidat_render::RenderBackendKind::TimelineFfmpegReencode,
        replay: awidat_render::RenderReplayPlan::FfmpegArgv {
            argv,
            cwd: spec
                .cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().into_owned()),
        },
        inputs: Vec::new(),
        outputs: vec![awidat_render::output_artifact(&spec.output_path, true)],
        sidecars: Vec::new(),
        limitations: spec
            .limitations
            .iter()
            .map(|limitation| awidat_render::limitation(limitation.code.clone(), limitation.message.clone()))
            .collect(),
        verification: None,
        metadata: std::collections::BTreeMap::from([(
            "render_driver".into(),
            "shared_render_spec".into(),
        )]),
    });
    let path = awidat_render::manifest_path_for_output(&spec.output_path);
    awidat_render::write_render_manifest(&path, &manifest)
        .with_context(|| format!("write acceptance render manifest {}", path.display()))?;
    Ok(path)
}
```

- [ ] **Step 3: Make artifact inputs require `render_manifest_path`**

In `crates/eval/src/acceptance/artifacts.rs`, add this field to both `RequiredArtifactInput` and `ArtifactBundleInput`:

```rust
pub(crate) render_manifest_path: &'a Path,
```

Add this required entry in both `build_artifact_bundle_manifest` and `required_artifact_entries` immediately after `rendered_output`:

```rust
("render_manifest", input.render_manifest_path, true),
```

- [ ] **Step 4: Add scorecard artifact field**

In `crates/eval/src/acceptance/scorecard.rs`, add this field to `AcceptanceArtifacts`:

```rust
pub(crate) render_manifest: String,
```

Add this field to `ScorecardInput`:

```rust
pub(crate) render_manifest_path: &'a Path,
```

In `build_scorecard`, add:

```rust
render_manifest: input.render_manifest_path.to_string_lossy().into_owned(),
```

- [ ] **Step 5: Pass render manifest paths through runner**

In `crates/eval/src/acceptance/runner.rs`, pass `&render.render_manifest_path` into:

```rust
RequiredArtifactInput { render_manifest_path: &render.render_manifest_path, ... }
ScorecardInput { render_manifest_path: &render.render_manifest_path, ... }
ArtifactBundleInput { render_manifest_path: &render.render_manifest_path, ... }
```

- [ ] **Step 6: Update tests that construct scorecards manually**

Modify `crates/eval/src/acceptance/batch_tests.rs` and `crates/eval/src/acceptance/failure_package_tests.rs` JSON fixtures to include:

```json
"render_manifest": "/tmp/run/artifacts/render.render-manifest.json"
```

When a test writes fixture artifact files, create a non-empty `render.render-manifest.json` file beside the existing `scorecard.json`, `final_edl.edl`, and `edit_manifest.json` files.

- [ ] **Step 7: Run acceptance-focused tests**

Run:

```bash
cargo test -p awidat-eval acceptance -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit Task 6**

Run:

```bash
git add crates/eval/src/acceptance/media.rs crates/eval/src/acceptance/artifacts.rs crates/eval/src/acceptance/scorecard.rs crates/eval/src/acceptance/runner.rs crates/eval/src/acceptance/batch_tests.rs crates/eval/src/acceptance/failure_package_tests.rs
git commit -m "feat: include render manifests in acceptance artifacts"
```

## Task 7: Final Verification

**Files:**
- Verify all touched files

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt --all -- --check
```

Expected: PASS. If it fails, run `cargo fmt --all`, inspect `git diff`, then rerun the check.

- [ ] **Step 2: Run focused tests**

Run:

```bash
cargo test -p awidat-render manifest -- --nocapture
cargo test -p awidat-core start_render export_package -- --nocapture
cargo test -p awidat-eval acceptance -- --nocapture
```

Expected: PASS for all three commands.

- [ ] **Step 3: Run broad workspace test when local time allows**

Run:

```bash
cargo test --workspace
```

Expected: PASS. If this is too slow for the local session, record that focused tests passed and the workspace test was not run.

- [ ] **Step 4: Run clippy**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 5: Verify manifest paths are discoverable**

Run:

```bash
rg -n '"manifest_path"|render_manifest|ReplayRender|replay-render' crates
```

Expected: hits in `start_render.rs`, `export_package.rs`, `main.rs`, and acceptance artifact files.

- [ ] **Step 6: Commit verification fixes if needed**

If formatting or clippy required edits, run:

```bash
git add crates
git commit -m "chore: clean up render manifest integration"
```

## Phase Two Handoff: Stream-Copy/Remux Tool

Implement this only after the manifest/replay spine above lands and verifies.

**Feature scope:**
- Add `crates/core/src/tools/remux_stream_export.rs`.
- Accept a constrained agent-facing request that lowers to `awidat_proto::professional::StreamExportContract`.
- Use `awidat_render::professional::plan_stream_export_args`.
- Start a background `JobManager` job.
- Emit `RenderBackendKind::StreamExportRemux`.
- Return `job_id`, `output_path`, `manifest_path`, `stream_export_contract_id`, and `render_limitations`.

**First tests to write:**

```rust
#[tokio::test]
async fn remux_stream_export_writes_manifest_before_starting_job() {
    // Build a temp project, one stub raw asset, and a simple contract.
    // Assert the tool response contains manifest_path.
    // Assert the manifest backend is stream_export_remux.
}

#[test]
fn remux_contract_lowering_uses_shared_stream_export_planner() {
    // Call the helper that builds RenderJobSpec from StreamExportContract.
    // Assert argv contains "-c" "copy" and the planned output path.
}
```

Do not add this tool inside the phase-one branch unless the manifest and replay work is already passing. The tool should consume the new manifest API rather than introduce another one-off render record.
