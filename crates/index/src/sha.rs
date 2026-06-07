//! Asset identity helpers.
//!
//! Large video files sit on the hot path for desktop re-indexing. The
//! dispatcher uses a source metadata fingerprint for idempotency so a
//! 3 GB clip can reach the indexer immediately instead of blocking on a
//! full-file hash. The full SHA-256 helper remains available for callers
//! that explicitly need byte-level content identity.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

/// Compute a fast, source-stable asset fingerprint from file metadata.
///
/// This mirrors the older Swift editor's transcription cache strategy:
/// use stable source metadata instead of content-hashing large media on
/// every run. The value is still SHA-256-shaped because existing sidecar
/// schemas name the field `asset_sha256`.
pub fn asset_fingerprint(path: &Path) -> std::io::Result<String> {
    let meta = std::fs::metadata(path)?;
    let modified = meta.modified().unwrap_or(UNIX_EPOCH);
    let modified = modified.duration_since(UNIX_EPOCH).unwrap_or_default();
    let identity = format!(
        "montage-asset-fingerprint-v1\0{}\0{}\0{}",
        meta.len(),
        modified.as_secs(),
        modified.subsec_nanos()
    );
    Ok(format!("{:x}", Sha256::digest(identity.as_bytes())))
}

/// Compute the SHA-256 of an asset file as a lowercase hex string.
pub fn asset_sha256(path: &Path) -> std::io::Result<String> {
    let f = File::open(path)?;
    let mut reader = BufReader::with_capacity(64 * 1024, f);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_has_known_sha() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("empty");
        std::fs::write(&p, b"").unwrap();
        let h = asset_sha256(&p).unwrap();
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn small_payload_hashes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("hi");
        std::fs::write(&p, b"hello world").unwrap();
        let h = asset_sha256(&p).unwrap();
        assert_eq!(
            h,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
