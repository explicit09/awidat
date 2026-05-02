//! Streaming SHA-256 of a file. Hashes a 1h video on M-series silicon in
//! a few seconds; not a hot path, doesn't need to be parallel inside.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

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
