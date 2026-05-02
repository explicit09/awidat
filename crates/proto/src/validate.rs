//! Cross-cutting validation across a loaded project.
//!
//! [`Project::read`] already validates the OTIO timeline. This module
//! validates the *index sidecar* contract: that every `index/manifest.json`
//! entry has a real directory, that every sidecar present has a header that
//! matches the manifest, and that orphaned sidecars (sidecars not in the
//! manifest) surface as warnings.
//!
//! Validation here is **non-fatal by default**: anything wrong with the
//! index produces a [`ValidationWarning`] rather than failing the read,
//! because indexers are external MCP servers and the project is still
//! useful even when the index is partial. The CLI prints these warnings
//! after the summary.
//!
//! [`Project::read`]: crate::project::Project::read

use std::fs;
use std::path::Path;

use crate::ProtoError;
use crate::error::JsonPath;
use crate::index::{IndexSidecar, IndexSidecarHeader, Manifest, is_valid_indexer_id};
use crate::project::{Project, files};

/// Aggregate report from [`validate_project`].
#[derive(Debug, Default, Clone)]
pub struct ValidationReport {
    /// Schema forward-compat warnings emitted during read.
    pub schema_warnings: Vec<crate::otio::SchemaWarning>,
    /// Index-related issues that don't fail the project.
    pub index_warnings: Vec<ValidationWarning>,
    /// One-line summary fields for the CLI to pretty-print.
    pub summary: ProjectSummary,
}

/// Surface-level facts about the project, for the CLI's success line.
#[derive(Debug, Default, Clone)]
pub struct ProjectSummary {
    /// Count of tracks at the top level.
    pub track_count: usize,
    /// Count of clips across all tracks (recursive).
    pub clip_count: usize,
    /// Count of markers across all clips (recursive).
    pub marker_count: usize,
    /// Count of effects across all clips (recursive).
    pub effect_count: usize,
    /// Count of indexers in the manifest.
    pub indexer_count: usize,
    /// Total sidecars seen on disk under `index/<indexer>/`.
    pub sidecar_count: usize,
    /// Edit-plan item count.
    pub plan_item_count: usize,
}

/// One non-fatal issue with the index layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationWarning {
    /// Manifest mentions an indexer that has no directory under `index/`.
    IndexerDirMissing {
        /// Indexer name.
        indexer: String,
    },
    /// Sidecar header.indexer doesn't match the directory it's in.
    SidecarHeaderMismatch {
        /// Sidecar file path.
        path: String,
        /// What was on disk.
        found_indexer: String,
        /// What we expected.
        expected_indexer: String,
    },
    /// Sidecar's asset_id doesn't match a manifest entry's asset list.
    SidecarAssetNotInManifest {
        /// Indexer name.
        indexer: String,
        /// The asset id in question.
        asset: String,
        /// Sidecar path.
        path: String,
    },
    /// Sidecar file is malformed JSON.
    SidecarMalformed {
        /// Sidecar path.
        path: String,
        /// Parser error.
        message: String,
    },
    /// A directory exists under `index/` that the manifest doesn't list.
    OrphanIndexerDir {
        /// Indexer name (= directory name).
        indexer: String,
    },
    /// Indexer id violates the canonical naming rules.
    InvalidIndexerName {
        /// Indexer name.
        indexer: String,
    },
    /// Asset id is not a safe project-relative path.
    UnsafeAssetId {
        /// Asset id.
        asset: String,
        /// Where the unsafe id was found.
        location: String,
    },
    /// Sidecar file path does not match its `asset_id`.
    SidecarPathMismatch {
        /// Sidecar file path.
        path: String,
        /// Expected sidecar file path.
        expected_path: String,
        /// Asset id.
        asset: String,
    },
}

/// Run all cross-cutting validation against a loaded [`Project`].
pub fn validate_project(project: &Project) -> Result<ValidationReport, ProtoError> {
    let mut report = ValidationReport {
        schema_warnings: project.warnings.clone(),
        ..Default::default()
    };

    // Surface fields.
    let s = &mut report.summary;
    s.plan_item_count = project.edit_plan.items.len();
    s.track_count = project.timeline.tracks.children.len();
    walk_counts(
        &project.timeline.tracks,
        &mut s.clip_count,
        &mut s.marker_count,
        &mut s.effect_count,
    );

    // Index validation.
    let index_dir = project.root.join(files::INDEX_DIR);
    let manifest_path = index_dir.join(files::INDEX_MANIFEST);

    if let Some(manifest) = &project.manifest {
        s.indexer_count = manifest.indexers.len();
        validate_index(manifest, &index_dir, &mut report.index_warnings, s)?;
    } else if !manifest_path.exists() {
        // Fresh project: that's fine. No warning.
    }

    Ok(report)
}

fn walk_counts(
    stack: &crate::otio::Stack,
    clips: &mut usize,
    markers: &mut usize,
    effects: &mut usize,
) {
    for child in &stack.children {
        match child {
            crate::otio::StackChild::Track(t) => count_track(t, clips, markers, effects),
            crate::otio::StackChild::Stack(s) => walk_counts(s, clips, markers, effects),
            crate::otio::StackChild::Clip(c) => count_clip(c, clips, markers, effects),
            crate::otio::StackChild::Gap(_) => {}
        }
    }
}

fn count_track(
    t: &crate::otio::Track,
    clips: &mut usize,
    markers: &mut usize,
    effects: &mut usize,
) {
    for child in &t.children {
        match child {
            crate::otio::TrackChild::Clip(c) => count_clip(c, clips, markers, effects),
            crate::otio::TrackChild::Gap(_) | crate::otio::TrackChild::Transition(_) => {}
            crate::otio::TrackChild::Stack(s) => walk_counts(s, clips, markers, effects),
        }
    }
}

fn count_clip(c: &crate::otio::Clip, clips: &mut usize, markers: &mut usize, effects: &mut usize) {
    *clips += 1;
    *markers += c.markers.len();
    *effects += c.effects.len();
}

fn validate_index(
    manifest: &Manifest,
    index_dir: &Path,
    warnings: &mut Vec<ValidationWarning>,
    summary: &mut ProjectSummary,
) -> Result<(), ProtoError> {
    use std::collections::HashSet;

    // Manifest perspective: every listed indexer should have a directory.
    let listed: HashSet<&str> = manifest.indexers.iter().map(|e| e.name.as_str()).collect();
    for entry in &manifest.indexers {
        validate_manifest_entry(entry, warnings);

        let dir = index_dir.join(&entry.name);
        if !dir.is_dir() {
            warnings.push(ValidationWarning::IndexerDirMissing {
                indexer: entry.name.clone(),
            });
            continue;
        }
        // Walk the indexer dir, validating each sidecar header.
        let mut sidecar_paths = Vec::new();
        collect_json_files(&dir, &mut sidecar_paths)?;
        for path in sidecar_paths {
            summary.sidecar_count += 1;
            validate_sidecar(entry, &dir, &path, warnings);
        }
    }

    // Disk perspective: any directory under index/ that isn't in the manifest
    // is an orphan.
    if index_dir.is_dir() {
        for entry_res in fs::read_dir(index_dir).map_err(|e| ProtoError::Io {
            path: index_dir.display().to_string(),
            source: e,
        })? {
            let dent = entry_res.map_err(|e| ProtoError::Io {
                path: index_dir.display().to_string(),
                source: e,
            })?;
            let path = dent.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|s| s.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if !listed.contains(name) {
                warnings.push(ValidationWarning::OrphanIndexerDir {
                    indexer: name.to_string(),
                });
            }
        }
    }

    Ok(())
}

fn validate_manifest_entry(
    entry: &crate::index::IndexerEntry,
    warnings: &mut Vec<ValidationWarning>,
) {
    if !is_valid_indexer_id(&entry.name) {
        warnings.push(ValidationWarning::InvalidIndexerName {
            indexer: entry.name.clone(),
        });
    }
    for asset in &entry.assets {
        if let Err(err) = asset.validate(files::INDEX_MANIFEST, &JsonPath::root().field("assets")) {
            warnings.push(ValidationWarning::UnsafeAssetId {
                asset: asset.to_string(),
                location: err.to_string(),
            });
        }
    }
}

fn validate_sidecar(
    entry: &crate::index::IndexerEntry,
    indexer_dir: &Path,
    path: &Path,
    warnings: &mut Vec<ValidationWarning>,
) {
    let Some(header) = read_sidecar_header(path, warnings) else {
        return;
    };

    validate_sidecar_asset_path(&header, indexer_dir, path, warnings);

    if header.indexer != entry.name {
        warnings.push(ValidationWarning::SidecarHeaderMismatch {
            path: path.display().to_string(),
            found_indexer: header.indexer.clone(),
            expected_indexer: entry.name.clone(),
        });
    }
    if !entry.assets.contains(&header.asset_id) {
        warnings.push(ValidationWarning::SidecarAssetNotInManifest {
            indexer: entry.name.clone(),
            asset: header.asset_id.to_string(),
            path: path.display().to_string(),
        });
    }
}

fn read_sidecar_header(
    path: &Path,
    warnings: &mut Vec<ValidationWarning>,
) -> Option<IndexSidecarHeader> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            warnings.push(ValidationWarning::SidecarMalformed {
                path: path.display().to_string(),
                message: format!("io: {e}"),
            });
            return None;
        }
    };
    let parsed: Result<IndexSidecar<serde_json::Value>, _> = serde_json::from_str(&text);
    match parsed {
        Ok(s) => Some(s.header),
        Err(e) => {
            warnings.push(ValidationWarning::SidecarMalformed {
                path: path.display().to_string(),
                message: e.to_string(),
            });
            None
        }
    }
}

fn validate_sidecar_asset_path(
    header: &IndexSidecarHeader,
    indexer_dir: &Path,
    path: &Path,
    warnings: &mut Vec<ValidationWarning>,
) {
    if let Err(err) = header.asset_id.validate(
        &path.display().to_string(),
        &JsonPath::root().field("asset_id"),
    ) {
        warnings.push(ValidationWarning::UnsafeAssetId {
            asset: header.asset_id.to_string(),
            location: err.to_string(),
        });
        return;
    }

    let Some(rel_path) = header.asset_id.sidecar_relative_path() else {
        return;
    };
    let expected_path = indexer_dir.join(rel_path);
    if path != expected_path {
        warnings.push(ValidationWarning::SidecarPathMismatch {
            path: path.display().to_string(),
            expected_path: expected_path.display().to_string(),
            asset: header.asset_id.to_string(),
        });
    }
}

fn collect_json_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), ProtoError> {
    for entry_res in fs::read_dir(dir).map_err(|e| ProtoError::Io {
        path: dir.display().to_string(),
        source: e,
    })? {
        let dent = entry_res.map_err(|e| ProtoError::Io {
            path: dir.display().to_string(),
            source: e,
        })?;
        let file_type = dent.file_type().map_err(|e| ProtoError::Io {
            path: dent.path().display().to_string(),
            source: e,
        })?;
        let path = dent.path();
        if file_type.is_dir() {
            collect_json_files(&path, out)?;
        } else if file_type.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{AssetId, IndexerEntry};
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    fn tmp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        p.push(format!("awidat-vt-{nanos}-{}-{n}", std::process::id()));
        p
    }

    #[test]
    fn fresh_project_validates_clean() {
        let root = tmp_dir();
        Project::init(&root).unwrap();
        let p = Project::read(&root).unwrap();
        let r = validate_project(&p).unwrap();
        assert!(r.index_warnings.is_empty());
        assert_eq!(r.summary.track_count, 0);
        assert_eq!(r.summary.indexer_count, 0);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn warns_on_missing_indexer_dir() {
        let root = tmp_dir();
        Project::init(&root).unwrap();
        // Hand-edit the manifest to claim an indexer ran, but don't create
        // its directory.
        let mp = root.join(files::INDEX_DIR).join(files::INDEX_MANIFEST);
        let m = Manifest {
            version: "0.1".into(),
            indexers: vec![IndexerEntry {
                name: "whisper".into(),
                version: "1.0".into(),
                schema_version: "1".into(),
                assets: vec![AssetId::new("raw/foo.mp4")],
                last_run: Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap(),
            }],
        };
        fs::write(&mp, serde_json::to_string_pretty(&m).unwrap()).unwrap();

        let p = Project::read(&root).unwrap();
        let r = validate_project(&p).unwrap();
        assert!(r
            .index_warnings
            .iter()
            .any(|w| matches!(w, ValidationWarning::IndexerDirMissing { indexer } if indexer == "whisper")));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn flags_orphan_dir_and_validates_sidecar() {
        let root = tmp_dir();
        Project::init(&root).unwrap();

        // Manifest claims indexer "whisper" ran on raw/foo.mp4.
        let mp = root.join(files::INDEX_DIR).join(files::INDEX_MANIFEST);
        let m = Manifest {
            version: "0.1".into(),
            indexers: vec![IndexerEntry {
                name: "whisper".into(),
                version: "1.0".into(),
                schema_version: "1".into(),
                assets: vec![AssetId::new("raw/foo.mp4")],
                last_run: Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap(),
            }],
        };
        fs::write(&mp, serde_json::to_string_pretty(&m).unwrap()).unwrap();

        // Create a real sidecar.
        let whisper_dir = root.join(files::INDEX_DIR).join("whisper");
        fs::create_dir_all(&whisper_dir).unwrap();
        fs::write(
            whisper_dir.join("foo.mp4.json"),
            serde_json::to_string(&serde_json::json!({
                "indexer": "whisper",
                "indexer_version": "1.0",
                "schema_version": "1",
                "asset_id": "raw/foo.mp4",
                "asset_sha256": "abc",
                "produced_at": "2026-05-02T12:00:00Z",
                "data": {"words": []}
            }))
            .unwrap(),
        )
        .unwrap();

        // Create an orphan directory.
        fs::create_dir_all(root.join(files::INDEX_DIR).join("ghost")).unwrap();

        let p = Project::read(&root).unwrap();
        let r = validate_project(&p).unwrap();
        assert_eq!(r.summary.sidecar_count, 1);
        assert!(r.index_warnings.iter().any(
            |w| matches!(w, ValidationWarning::OrphanIndexerDir { indexer } if indexer == "ghost")
        ));
        // No header-mismatch / no asset-not-in-manifest warning.
        assert!(
            !r.index_warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::SidecarHeaderMismatch { .. }))
        );
        assert!(
            !r.index_warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::SidecarAssetNotInManifest { .. }))
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn flags_header_mismatch() {
        let root = tmp_dir();
        Project::init(&root).unwrap();

        let mp = root.join(files::INDEX_DIR).join(files::INDEX_MANIFEST);
        let m = Manifest {
            version: "0.1".into(),
            indexers: vec![IndexerEntry {
                name: "whisper".into(),
                version: "1".into(),
                schema_version: "1".into(),
                assets: vec![AssetId::new("raw/foo.mp4")],
                last_run: Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap(),
            }],
        };
        fs::write(&mp, serde_json::to_string_pretty(&m).unwrap()).unwrap();

        let whisper_dir = root.join(files::INDEX_DIR).join("whisper");
        fs::create_dir_all(&whisper_dir).unwrap();
        // Sidecar in whisper/ but header.indexer says "scenedetect".
        fs::write(
            whisper_dir.join("foo.mp4.json"),
            serde_json::to_string(&serde_json::json!({
                "indexer": "scenedetect",
                "indexer_version": "1",
                "schema_version": "1",
                "asset_id": "raw/foo.mp4",
                "asset_sha256": "abc",
                "produced_at": "2026-05-02T12:00:00Z",
                "data": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let p = Project::read(&root).unwrap();
        let r = validate_project(&p).unwrap();
        assert!(
            r.index_warnings
                .iter()
                .any(|w| matches!(w, ValidationWarning::SidecarHeaderMismatch { .. }))
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn validates_nested_sidecars_that_preserve_asset_path() {
        let root = tmp_dir();
        Project::init(&root).unwrap();

        let mp = root.join(files::INDEX_DIR).join(files::INDEX_MANIFEST);
        let m = Manifest {
            version: "0.1".into(),
            indexers: vec![IndexerEntry {
                name: "whisper".into(),
                version: "1".into(),
                schema_version: "1".into(),
                assets: vec![AssetId::new("raw/foo.mp4")],
                last_run: Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap(),
            }],
        };
        fs::write(&mp, serde_json::to_string_pretty(&m).unwrap()).unwrap();

        let nested_dir = root.join(files::INDEX_DIR).join("whisper").join("raw");
        fs::create_dir_all(&nested_dir).unwrap();
        fs::write(
            nested_dir.join("foo.mp4.json"),
            serde_json::to_string(&serde_json::json!({
                "indexer": "whisper",
                "indexer_version": "1",
                "schema_version": "1",
                "asset_id": "raw/foo.mp4",
                "asset_sha256": "abc",
                "produced_at": "2026-05-02T12:00:00Z",
                "data": {"words": []}
            }))
            .unwrap(),
        )
        .unwrap();

        let p = Project::read(&root).unwrap();
        let r = validate_project(&p).unwrap();
        assert_eq!(r.summary.sidecar_count, 1);
        assert!(r.index_warnings.is_empty());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn warns_on_unsafe_index_paths() {
        let root = tmp_dir();
        Project::init(&root).unwrap();

        let mp = root.join(files::INDEX_DIR).join(files::INDEX_MANIFEST);
        let m = Manifest {
            version: "0.1".into(),
            indexers: vec![IndexerEntry {
                name: "Bad_Name".into(),
                version: "1".into(),
                schema_version: "1".into(),
                assets: vec![AssetId::new("../raw/foo.mp4")],
                last_run: Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap(),
            }],
        };
        fs::write(&mp, serde_json::to_string_pretty(&m).unwrap()).unwrap();

        let bad_dir = root.join(files::INDEX_DIR).join("Bad_Name");
        fs::create_dir_all(&bad_dir).unwrap();
        fs::write(
            bad_dir.join("foo.mp4.json"),
            serde_json::to_string(&serde_json::json!({
                "indexer": "Bad_Name",
                "indexer_version": "1",
                "schema_version": "1",
                "asset_id": "../raw/foo.mp4",
                "asset_sha256": "abc",
                "produced_at": "2026-05-02T12:00:00Z",
                "data": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let p = Project::read(&root).unwrap();
        let r = validate_project(&p).unwrap();
        assert!(r
            .index_warnings
            .iter()
            .any(|w| matches!(w, ValidationWarning::InvalidIndexerName { indexer } if indexer == "Bad_Name")));
        assert!(r
            .index_warnings
            .iter()
            .any(|w| matches!(w, ValidationWarning::UnsafeAssetId { asset, .. } if asset == "../raw/foo.mp4")));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn warns_when_sidecar_path_does_not_match_asset_id() {
        let root = tmp_dir();
        Project::init(&root).unwrap();

        let mp = root.join(files::INDEX_DIR).join(files::INDEX_MANIFEST);
        let m = Manifest {
            version: "0.1".into(),
            indexers: vec![IndexerEntry {
                name: "whisper".into(),
                version: "1".into(),
                schema_version: "1".into(),
                assets: vec![AssetId::new("raw/foo.mp4")],
                last_run: Utc.with_ymd_and_hms(2026, 5, 2, 0, 0, 0).unwrap(),
            }],
        };
        fs::write(&mp, serde_json::to_string_pretty(&m).unwrap()).unwrap();

        let misplaced_dir = root.join(files::INDEX_DIR).join("whisper").join("wrong");
        fs::create_dir_all(&misplaced_dir).unwrap();
        fs::write(
            misplaced_dir.join("foo.mp4.json"),
            serde_json::to_string(&serde_json::json!({
                "indexer": "whisper",
                "indexer_version": "1",
                "schema_version": "1",
                "asset_id": "raw/foo.mp4",
                "asset_sha256": "abc",
                "produced_at": "2026-05-02T12:00:00Z",
                "data": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let p = Project::read(&root).unwrap();
        let r = validate_project(&p).unwrap();
        assert!(r
            .index_warnings
            .iter()
            .any(|w| matches!(w, ValidationWarning::SidecarPathMismatch { asset, .. } if asset == "raw/foo.mp4")));
        fs::remove_dir_all(&root).ok();
    }
}
