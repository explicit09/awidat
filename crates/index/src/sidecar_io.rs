//! Sidecar lookup. Used by the editorial tools (`read_index`,
//! `inspect_clip`, `find_moment`) to get at indexer output without each
//! tool re-implementing path resolution.
//!
//! Sidecars live at `<project>/index/<indexer>/<asset_id>.json` per
//! `INDEX_SCHEMA.md`. The path-derivation goes through
//! `AssetId::sidecar_relative_path()` so the path-traversal validation
//! we built in week 1 covers tool callers too.

use std::path::{Path, PathBuf};

use montage_proto::index::AssetId;
use montage_proto::project::files;

/// Errors from sidecar lookup. Tool callers map these into
/// `RespondToModel` strings; the per-variant Display strings are written
/// to be agent-actionable.
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    /// Asset id failed safe-path validation (`..`, absolute, control chars).
    /// The model gets the underlying message.
    #[error("asset_id '{asset}' failed safe-path validation; pick a project-relative path")]
    UnsafeAssetId {
        /// The bad asset id string.
        asset: String,
    },
    /// Sidecar file doesn't exist on disk yet.
    #[error("no sidecar at {path}; run `montage index --indexer {indexer}` first")]
    NotFound {
        /// Indexer name we looked up.
        indexer: String,
        /// Resolved path (helpful in errors).
        path: PathBuf,
    },
    /// Sidecar was unreadable / malformed.
    #[error("sidecar at {path} is malformed: {message}")]
    Malformed {
        /// Path of the broken sidecar.
        path: PathBuf,
        /// Diagnostic.
        message: String,
    },
}

/// Resolve the on-disk sidecar path for `(project_root, indexer, asset_id)`.
/// Does not check existence — use [`read_sidecar`] for that.
pub fn sidecar_path(
    project_root: &Path,
    indexer: &str,
    asset_id: &AssetId,
) -> Result<PathBuf, SidecarError> {
    let rel = asset_id
        .sidecar_relative_path()
        .ok_or_else(|| SidecarError::UnsafeAssetId {
            asset: asset_id.to_string(),
        })?;
    Ok(project_root.join(files::INDEX_DIR).join(indexer).join(rel))
}

/// Read and parse the sidecar at `(project_root, indexer, asset_id)`.
/// The result is the full sidecar JSON value (header + `data`); tools
/// project the fields they care about.
pub fn read_sidecar(
    project_root: &Path,
    indexer: &str,
    asset_id: &AssetId,
) -> Result<serde_json::Value, SidecarError> {
    let path = sidecar_path(project_root, indexer, asset_id)?;
    if !path.exists() {
        return Err(SidecarError::NotFound {
            indexer: indexer.to_string(),
            path,
        });
    }
    let bytes = std::fs::read(&path).map_err(|e| SidecarError::Malformed {
        path: path.clone(),
        message: format!("io: {e}"),
    })?;
    serde_json::from_slice(&bytes).map_err(|e| SidecarError::Malformed {
        path,
        message: e.to_string(),
    })
}

/// Iterate over every sidecar under `<project>/index/<indexer>/`. Yields
/// `(asset_id_relative_path, parsed_value)`. Used by `find_moment` to
/// scan one indexer's whole output.
///
/// Skips malformed sidecars silently (logged via `tracing`); they're
/// surfaced separately by `montage validate`.
pub fn walk_indexer(
    project_root: &Path,
    indexer: &str,
) -> Result<impl Iterator<Item = (String, serde_json::Value)>, SidecarError> {
    let dir = project_root.join(files::INDEX_DIR).join(indexer);
    if !dir.is_dir() {
        return Ok(Vec::new().into_iter());
    }
    let mut out = Vec::new();
    walk_inner(&dir, &dir, &mut out);
    Ok(out.into_iter())
}

fn walk_inner(root: &Path, here: &Path, out: &mut Vec<(String, serde_json::Value)>) {
    let Ok(entries) = std::fs::read_dir(here) else {
        return;
    };
    for ent in entries.flatten() {
        let p = ent.path();
        if p.is_dir() {
            walk_inner(root, &p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("json") {
            let Ok(bytes) = std::fs::read(&p) else {
                continue;
            };
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                tracing::warn!(path = %p.display(), "sidecar malformed; skipping");
                continue;
            };
            // Asset id = path relative to indexer dir, minus the .json.
            let Ok(rel) = p.strip_prefix(root) else {
                continue;
            };
            let s = rel.to_string_lossy();
            let asset = s.strip_suffix(".json").unwrap_or(&s).to_string();
            out.push((asset, v));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn sidecar_path_resolves() {
        let p = sidecar_path(
            Path::new("/tmp/proj"),
            "whisper",
            &AssetId::new("raw/foo.wav"),
        )
        .unwrap();
        assert_eq!(p, PathBuf::from("/tmp/proj/index/whisper/raw/foo.wav.json"));
    }

    #[test]
    fn unsafe_asset_id_is_rejected() {
        let err = sidecar_path(
            Path::new("/tmp/proj"),
            "whisper",
            &AssetId::new("../escape"),
        )
        .unwrap_err();
        assert!(matches!(err, SidecarError::UnsafeAssetId { .. }));
    }

    #[test]
    fn read_missing_returns_notfound() {
        let dir = tempfile::tempdir().unwrap();
        let err =
            read_sidecar(dir.path(), "whisper", &AssetId::new("raw/missing.wav")).unwrap_err();
        match err {
            SidecarError::NotFound { indexer, .. } => assert_eq!(indexer, "whisper"),
            other => panic!("want NotFound, got {other:?}"),
        }
    }

    #[test]
    fn read_existing_returns_value() {
        let dir = tempfile::tempdir().unwrap();
        let sc = dir.path().join("index/whisper/raw/x.wav.json");
        write(&sc, r#"{"indexer":"whisper","data":{"words":[]}}"#);
        let v = read_sidecar(dir.path(), "whisper", &AssetId::new("raw/x.wav")).unwrap();
        assert_eq!(v["indexer"], "whisper");
    }

    #[test]
    fn walk_indexer_yields_each_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("index/whisper/raw/a.wav.json"),
            r#"{"indexer":"whisper","data":{}}"#,
        );
        write(
            &dir.path().join("index/whisper/raw/b.wav.json"),
            r#"{"indexer":"whisper","data":{}}"#,
        );
        write(
            &dir.path().join("index/whisper/raw/bad.wav.json"),
            "not json",
        );
        let mut got: Vec<String> = walk_indexer(dir.path(), "whisper")
            .unwrap()
            .map(|(asset, _)| asset)
            .collect();
        got.sort();
        // The bad sidecar is skipped; we keep the two good ones.
        assert_eq!(got, vec!["raw/a.wav".to_string(), "raw/b.wav".to_string()]);
    }

    #[test]
    fn walk_indexer_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let n = walk_indexer(dir.path(), "ghost").unwrap().count();
        assert_eq!(n, 0);
    }
}
