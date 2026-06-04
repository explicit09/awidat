use awidat_social::youtube_upload::{
    ArtifactBody, ArtifactSource, ArtifactSourceError, YOUTUBE_MAX_BYTES,
};
use std::path::{Path, PathBuf};

/// Resolves `file:///absolute/path` artifact refs to disk content.
///
/// Phase 5 will add Supabase Storage signed-URL resolution; until then
/// the desktop writes render output to a local path and passes a `file://` ref.
///
/// SECURITY: every resolved path is canonicalized and confirmed to live under
/// `base_dir`. This prevents path-traversal / arbitrary-file-read: a malicious
/// `file:///etc/passwd` or `file://<base>/../../secret` ref is rejected because
/// the canonical path escapes the configured artifact root. Symlinks are
/// resolved by `canonicalize`, so a symlink inside `base_dir` pointing outside
/// it is also caught.
pub struct FileArtifactSource {
    base_dir: PathBuf,
}

impl FileArtifactSource {
    /// Construct with the canonical artifact root. The base is canonicalized
    /// once up front; if it does not exist, every `open` will fail closed.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let raw = base_dir.into();
        let base_dir = std::fs::canonicalize(&raw).unwrap_or(raw);
        Self { base_dir }
    }
}

impl ArtifactSource for FileArtifactSource {
    fn open(&self, artifact_ref: &str) -> Result<ArtifactBody, ArtifactSourceError> {
        let requested = parse_file_uri(artifact_ref)?;
        let path = self.resolve_within_base(&requested, artifact_ref)?;

        let metadata = std::fs::metadata(&path)
            .map_err(|e| ArtifactSourceError::NotFound(format!("{artifact_ref}: {e}")))?;
        let total_bytes = metadata.len();
        if total_bytes > YOUTUBE_MAX_BYTES {
            return Err(ArtifactSourceError::SizeExceeded {
                max_bytes: YOUTUBE_MAX_BYTES,
                actual_bytes: total_bytes,
            });
        }
        let data = std::fs::read(&path)
            .map_err(|e| ArtifactSourceError::IoError(format!("{artifact_ref}: {e}")))?;
        Ok(ArtifactBody { total_bytes, data })
    }
}

impl FileArtifactSource {
    /// Canonicalize `requested` and confirm it sits under `base_dir`.
    ///
    /// Returns `NotFound` for anything that escapes the root (including missing
    /// files, since a non-existent path can't be confirmed inside the base).
    fn resolve_within_base(
        &self,
        requested: &Path,
        artifact_ref: &str,
    ) -> Result<PathBuf, ArtifactSourceError> {
        // Reject obvious traversal before touching the filesystem.
        if requested
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(ArtifactSourceError::NotFound(format!(
                "artifact_ref escapes artifact root: {artifact_ref}"
            )));
        }

        let canonical = std::fs::canonicalize(requested)
            .map_err(|e| ArtifactSourceError::NotFound(format!("{artifact_ref}: {e}")))?;

        if !canonical.starts_with(&self.base_dir) {
            return Err(ArtifactSourceError::NotFound(format!(
                "artifact_ref escapes artifact root: {artifact_ref}"
            )));
        }
        Ok(canonical)
    }
}

fn parse_file_uri(artifact_ref: &str) -> Result<PathBuf, ArtifactSourceError> {
    artifact_ref
        .strip_prefix("file://")
        .map(PathBuf::from)
        .ok_or_else(|| {
            ArtifactSourceError::NotFound(format!(
                "unsupported artifact_ref scheme (expected file://): {artifact_ref}"
            ))
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    /// Write `name` with `contents` inside `dir`, returning its `file://` ref.
    fn write_artifact(dir: &TempDir, name: &str, contents: &[u8]) -> String {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents).unwrap();
        format!("file://{}", path.to_string_lossy())
    }

    #[test]
    fn reads_file_contents_within_base() {
        let dir = TempDir::new().unwrap();
        let r = write_artifact(&dir, "render.mp4", b"hello world");
        let source = FileArtifactSource::new(dir.path());
        let body = source.open(&r).unwrap();
        assert_eq!(body.data, b"hello world");
        assert_eq!(body.total_bytes, 11);
    }

    #[test]
    fn missing_file_returns_not_found() {
        let dir = TempDir::new().unwrap();
        let source = FileArtifactSource::new(dir.path());
        let missing = format!("file://{}/no-such.mp4", dir.path().to_string_lossy());
        let err = source.open(&missing).unwrap_err();
        assert!(matches!(err, ArtifactSourceError::NotFound(_)));
    }

    #[test]
    fn non_file_scheme_returns_not_found() {
        let dir = TempDir::new().unwrap();
        let source = FileArtifactSource::new(dir.path());
        let err = source
            .open("supabase-storage://bucket/key.mp4")
            .unwrap_err();
        assert!(matches!(err, ArtifactSourceError::NotFound(_)));
    }

    #[test]
    fn absolute_path_outside_base_is_rejected() {
        // A real file that exists but lives outside the artifact root.
        let outside = TempDir::new().unwrap();
        let secret = write_artifact(&outside, "secret.txt", b"top secret");

        let base = TempDir::new().unwrap();
        let source = FileArtifactSource::new(base.path());
        let err = source.open(&secret).unwrap_err();
        assert!(
            matches!(err, ArtifactSourceError::NotFound(_)),
            "file outside base must be rejected, got {err:?}"
        );
    }

    #[test]
    fn parent_dir_traversal_is_rejected() {
        let base = TempDir::new().unwrap();
        let source = FileArtifactSource::new(base.path());
        let traversal = format!(
            "file://{}/../../../etc/passwd",
            base.path().to_string_lossy()
        );
        let err = source.open(&traversal).unwrap_err();
        assert!(matches!(err, ArtifactSourceError::NotFound(_)));
    }
}
