//! Read/write the `index/manifest.json` file. Tiny module; pulled out so
//! `lib.rs` doesn't grow inline I/O helpers.

use std::path::Path;

use montage_proto::index::Manifest;

use crate::IndexError;

/// Read the manifest at `path`. Returns `Ok(None)` if absent.
pub fn read_manifest(path: &Path) -> Result<Option<Manifest>, IndexError> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(path).map_err(|e| IndexError::ManifestIo {
        path: path.display().to_string(),
        source: e,
    })?;
    let m: Manifest = serde_json::from_slice(&bytes).map_err(|e| IndexError::ManifestIo {
        path: path.display().to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
    })?;
    Ok(Some(m))
}

/// Write the manifest to `path`, pretty-printed.
pub fn write_manifest(path: &Path, manifest: &Manifest) -> Result<(), IndexError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| IndexError::ManifestIo {
            path: parent.display().to_string(),
            source: e,
        })?;
    }
    let body = serde_json::to_vec_pretty(manifest).map_err(|e| IndexError::ManifestIo {
        path: path.display().to_string(),
        source: std::io::Error::other(e),
    })?;
    std::fs::write(path, body).map_err(|e| IndexError::ManifestIo {
        path: path.display().to_string(),
        source: e,
    })
}
