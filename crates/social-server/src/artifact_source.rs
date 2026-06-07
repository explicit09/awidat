use montage_social::youtube_upload::{
    ArtifactBody, ArtifactSource, ArtifactSourceError, YOUTUBE_MAX_BYTES,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Resolves artifact refs to uploadable bytes.
///
/// Supports:
/// - `file:///absolute/path` for local development, confined to `base_dir`.
/// - `supabase-storage://bucket/object/path.mp4` for server-staged artifacts.
///
/// SECURITY: every resolved path is canonicalized and confirmed to live under
/// `base_dir`. This prevents path-traversal / arbitrary-file-read: a malicious
/// `file:///etc/passwd` or `file://<base>/../../secret` ref is rejected because
/// the canonical path escapes the configured artifact root. Symlinks are
/// resolved by `canonicalize`, so a symlink inside `base_dir` pointing outside
/// it is also caught.
pub struct FileArtifactSource {
    base_dir: PathBuf,
    storage_resolver: Option<Arc<dyn StorageUrlResolver>>,
    http: reqwest::Client,
}

impl FileArtifactSource {
    /// Construct with the canonical artifact root. The base is canonicalized
    /// once up front; if it does not exist, every `open` will fail closed.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        let raw = base_dir.into();
        let base_dir = std::fs::canonicalize(&raw).unwrap_or(raw);
        Self {
            base_dir,
            storage_resolver: None,
            http: reqwest::Client::new(),
        }
    }

    pub fn with_storage_resolver<R>(mut self, resolver: R) -> Self
    where
        R: StorageUrlResolver + 'static,
    {
        self.storage_resolver = Some(Arc::new(resolver));
        self
    }

    pub fn provider_fetch_url(&self, artifact_ref: &str) -> Result<String, ArtifactSourceError> {
        if artifact_ref.starts_with("http://") || artifact_ref.starts_with("https://") {
            return Ok(artifact_ref.to_string());
        }
        if artifact_ref.starts_with("supabase-storage://") {
            let (bucket, object_path) = parse_storage_ref(artifact_ref)?;
            let resolver = self.storage_resolver.as_ref().ok_or_else(|| {
                ArtifactSourceError::NotFound(format!(
                    "storage resolver not configured for artifact_ref: {artifact_ref}"
                ))
            })?;
            return resolver.signed_get_url(&bucket, &object_path);
        }
        Err(ArtifactSourceError::NotFound(format!(
            "artifact_ref is not provider-fetchable: {artifact_ref}"
        )))
    }
}

impl ArtifactSource for FileArtifactSource {
    fn open(&self, artifact_ref: &str) -> Result<ArtifactBody, ArtifactSourceError> {
        if artifact_ref.starts_with("supabase-storage://") {
            return self.open_storage_ref(artifact_ref);
        }
        let requested = parse_file_uri(artifact_ref)?;
        self.open_file_path(&requested, artifact_ref)
    }
}

pub trait StorageUrlResolver: Send + Sync {
    fn signed_get_url(
        &self,
        bucket: &str,
        object_path: &str,
    ) -> Result<String, ArtifactSourceError>;
}

#[derive(Clone)]
pub struct SupabaseStorageResolver {
    supabase_url: String,
    service_key: String,
    expires_in_secs: u32,
    http: reqwest::Client,
}

impl SupabaseStorageResolver {
    pub fn new(
        supabase_url: impl Into<String>,
        service_key: impl Into<String>,
        expires_in_secs: u32,
    ) -> Self {
        Self {
            supabase_url: supabase_url.into().trim_end_matches('/').to_string(),
            service_key: service_key.into(),
            expires_in_secs,
            http: reqwest::Client::new(),
        }
    }

    async fn signed_get_url_async(
        &self,
        bucket: &str,
        object_path: &str,
    ) -> Result<String, ArtifactSourceError> {
        let api_url = storage_sign_api_url(&self.supabase_url, bucket, object_path);
        let resp = self
            .http
            .post(&api_url)
            .header("Authorization", format!("Bearer {}", self.service_key))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "expiresIn": self.expires_in_secs }))
            .send()
            .await
            .map_err(|e| ArtifactSourceError::IoError(format!("storage sign request: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ArtifactSourceError::IoError(format!(
                "storage sign {status}: {body}"
            )));
        }
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ArtifactSourceError::IoError(format!("storage sign JSON: {e}")))?;
        let signed_path = json["signedURL"].as_str().ok_or_else(|| {
            ArtifactSourceError::IoError("storage sign response missing signedURL".into())
        })?;
        Ok(absolute_storage_url(&self.supabase_url, signed_path))
    }
}

impl StorageUrlResolver for SupabaseStorageResolver {
    fn signed_get_url(
        &self,
        bucket: &str,
        object_path: &str,
    ) -> Result<String, ArtifactSourceError> {
        block_on_current(self.signed_get_url_async(bucket, object_path))
    }
}

impl FileArtifactSource {
    fn open_file_path(
        &self,
        requested: &Path,
        artifact_ref: &str,
    ) -> Result<ArtifactBody, ArtifactSourceError> {
        let path = self.resolve_within_base(requested, artifact_ref)?;
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

    fn open_storage_ref(&self, artifact_ref: &str) -> Result<ArtifactBody, ArtifactSourceError> {
        let (bucket, object_path) = parse_storage_ref(artifact_ref)?;
        let resolver = self.storage_resolver.as_ref().ok_or_else(|| {
            ArtifactSourceError::NotFound(format!(
                "storage resolver not configured for artifact_ref: {artifact_ref}"
            ))
        })?;
        let signed_url = resolver.signed_get_url(&bucket, &object_path)?;
        if signed_url.starts_with("file://") {
            let requested = parse_file_uri(&signed_url)?;
            return self.open_file_path(&requested, artifact_ref);
        }
        self.open_http_url(&signed_url, artifact_ref)
    }

    fn open_http_url(
        &self,
        signed_url: &str,
        artifact_ref: &str,
    ) -> Result<ArtifactBody, ArtifactSourceError> {
        let bytes =
            block_on_current(async {
                let resp =
                    self.http.get(signed_url).send().await.map_err(|e| {
                        ArtifactSourceError::IoError(format!("{artifact_ref}: {e}"))
                    })?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    return Err(ArtifactSourceError::IoError(format!(
                        "{artifact_ref}: signed GET {status}: {body}"
                    )));
                }
                resp.bytes()
                    .await
                    .map(|bytes| bytes.to_vec())
                    .map_err(|e| ArtifactSourceError::IoError(format!("{artifact_ref}: {e}")))
            })?;
        let total_bytes = bytes.len() as u64;
        if total_bytes > YOUTUBE_MAX_BYTES {
            return Err(ArtifactSourceError::SizeExceeded {
                max_bytes: YOUTUBE_MAX_BYTES,
                actual_bytes: total_bytes,
            });
        }
        Ok(ArtifactBody {
            total_bytes,
            data: bytes,
        })
    }

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

fn parse_storage_ref(artifact_ref: &str) -> Result<(String, String), ArtifactSourceError> {
    let rest = artifact_ref
        .strip_prefix("supabase-storage://")
        .ok_or_else(|| {
            ArtifactSourceError::NotFound(format!("invalid storage ref: {artifact_ref}"))
        })?;
    let (bucket, object_path) = rest.split_once('/').ok_or_else(|| {
        ArtifactSourceError::NotFound(format!("storage ref missing object path: {artifact_ref}"))
    })?;
    if bucket.is_empty()
        || object_path.is_empty()
        || object_path.starts_with('/')
        || object_path.contains("..")
    {
        return Err(ArtifactSourceError::NotFound(format!(
            "unsafe storage ref: {artifact_ref}"
        )));
    }
    Ok((bucket.to_string(), object_path.to_string()))
}

fn absolute_storage_url(supabase_url: &str, signed_path: &str) -> String {
    if signed_path.starts_with("http://") || signed_path.starts_with("https://") {
        return signed_path.to_string();
    }
    let base = supabase_url.trim_end_matches('/');
    if signed_path.starts_with("/storage/v1/") {
        return format!("{base}{signed_path}");
    }
    format!("{base}/storage/v1{signed_path}")
}

fn storage_sign_api_url(supabase_url: &str, bucket: &str, object_path: &str) -> String {
    let base = supabase_url.trim_end_matches('/');
    let encoded_path = object_path
        .split('/')
        .map(urlencoding::encode)
        .collect::<Vec<_>>()
        .join("/");
    format!("{base}/storage/v1/object/sign/{bucket}/{encoded_path}")
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

fn block_on_current<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.block_on(future)
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|e| panic!("build artifact runtime: {e}"))
            .block_on(future)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct StaticStorageResolver {
        signed_url: String,
    }

    impl StorageUrlResolver for StaticStorageResolver {
        fn signed_get_url(
            &self,
            bucket: &str,
            object_path: &str,
        ) -> Result<String, ArtifactSourceError> {
            assert_eq!(bucket, "renders");
            assert_eq!(object_path, "jobs/job-1/artifact.mp4");
            Ok(self.signed_url.clone())
        }
    }

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
    fn reads_supabase_storage_ref_through_signed_url_resolver() {
        let dir = TempDir::new().unwrap();
        let r = write_artifact(&dir, "render.mp4", b"from storage");
        let source = FileArtifactSource::new(dir.path())
            .with_storage_resolver(StaticStorageResolver { signed_url: r });

        let body = source
            .open("supabase-storage://renders/jobs/job-1/artifact.mp4")
            .unwrap();

        assert_eq!(body.data, b"from storage");
        assert_eq!(body.total_bytes, 12);
    }

    #[tokio::test]
    async fn reads_supabase_storage_ref_through_http_signed_url() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/storage/v1/object/sign/renders/jobs/job%201/final%20%231.mp4",
            ))
            .and(header("authorization", "Bearer service-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "signedURL": "/object/sign/renders/jobs/job%201/final%20%231.mp4?token=abc"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/storage/v1/object/sign/renders/jobs/job%201/final%20%231.mp4",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"downloaded render".to_vec()))
            .mount(&server)
            .await;

        let base = TempDir::new().unwrap();
        let source = FileArtifactSource::new(base.path()).with_storage_resolver(
            SupabaseStorageResolver::new(server.uri(), "service-key", 3600),
        );
        let body = tokio::task::spawn_blocking(move || {
            source.open("supabase-storage://renders/jobs/job 1/final #1.mp4")
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(body.data, b"downloaded render");
        assert_eq!(body.total_bytes, 17);
    }

    #[test]
    fn storage_signed_paths_are_normalized_to_public_storage_urls() {
        assert_eq!(
            absolute_storage_url("https://example.supabase.co", "/object/sign/b/r?token=1"),
            "https://example.supabase.co/storage/v1/object/sign/b/r?token=1"
        );
        assert_eq!(
            absolute_storage_url(
                "https://example.supabase.co",
                "https://cdn.example.test/render.mp4"
            ),
            "https://cdn.example.test/render.mp4"
        );
    }

    #[test]
    fn provider_fetch_url_returns_signed_storage_url_without_downloading_bytes() {
        let base = TempDir::new().unwrap();
        let source =
            FileArtifactSource::new(base.path()).with_storage_resolver(StaticStorageResolver {
                signed_url: "https://storage.example/render.mp4?token=abc".into(),
            });

        let url = source
            .provider_fetch_url("supabase-storage://renders/jobs/job-1/artifact.mp4")
            .unwrap();

        assert_eq!(url, "https://storage.example/render.mp4?token=abc");
    }

    #[test]
    fn storage_sign_url_encodes_real_object_paths() {
        assert_eq!(
            storage_sign_api_url(
                "https://example.supabase.co/",
                "renders",
                "jobs/job 1/final #1.mp4"
            ),
            "https://example.supabase.co/storage/v1/object/sign/renders/jobs/job%201/final%20%231.mp4"
        );
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
