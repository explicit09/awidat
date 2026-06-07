use crate::model::Provider;
use crate::upload_adapter::{
    UploadAdapter, UploadAdapterError, UploadPrivacy, UploadRequest, UploadResult,
};
use crate::upload_status::{
    UploadProcessingStatus, UploadStatusAdapter, UploadStatusAdapterError, UploadStatusRequest,
    UploadStatusResult,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YouTubeUploadRequest {
    pub artifact_ref: String,
    pub thumbnail_ref: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub privacy: String,
    pub scheduled_for: Option<i64>,
    pub access_token_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YouTubeUploadResponse {
    pub video_id: String,
    pub processing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum YouTubeUploadClientError {
    MissingScope,
    AccountNotEligible,
    NetworkOrServer(String),
}

pub trait YouTubeUploadClient {
    fn upload_video(
        &self,
        request: &YouTubeUploadRequest,
    ) -> Result<YouTubeUploadResponse, YouTubeUploadClientError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YouTubeStatusRequest {
    pub provider_post_id: String,
    pub access_token_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YouTubeStatusResponse {
    pub video_id: String,
    pub state: YouTubeProcessingState,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum YouTubeProcessingState {
    Processing,
    Processed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum YouTubeStatusClientError {
    NetworkOrServer(String),
}

pub trait YouTubeStatusClient {
    fn poll_status(
        &self,
        request: &YouTubeStatusRequest,
    ) -> Result<YouTubeStatusResponse, YouTubeStatusClientError>;
}

#[derive(Clone, Debug)]
pub struct YouTubeUploadAdapter<C> {
    client: C,
}

impl<C> YouTubeUploadAdapter<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

#[derive(Clone, Debug)]
pub struct YouTubeStatusAdapter<C> {
    client: C,
}

impl<C> YouTubeStatusAdapter<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

impl<C: YouTubeUploadClient> UploadAdapter for YouTubeUploadAdapter<C> {
    fn provider(&self) -> Provider {
        Provider::YouTube
    }

    fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError> {
        if request.provider != Provider::YouTube {
            return Err(UploadAdapterError::ProviderMismatch);
        }
        if request.title.trim().is_empty() {
            return Err(UploadAdapterError::MediaConstraintFailed {
                reason: "youtube_title_required".into(),
            });
        }
        if request.access_token_ref.trim().is_empty() {
            return Err(UploadAdapterError::MissingUploadToken);
        }

        let youtube_request = YouTubeUploadRequest {
            artifact_ref: request.artifact_ref.clone(),
            thumbnail_ref: request.thumbnail_ref.clone(),
            title: request.title.trim().to_string(),
            description: request.description.clone(),
            tags: request.tags.clone(),
            privacy: youtube_privacy(&request.privacy).to_string(),
            scheduled_for: request.scheduled_for,
            access_token_ref: request.access_token_ref.clone(),
        };
        let response = self
            .client
            .upload_video(&youtube_request)
            .map_err(youtube_client_error)?;

        Ok(UploadResult {
            provider_post_url: format!("https://www.youtube.com/watch?v={}", response.video_id),
            provider_post_id: response.video_id,
            processing: response.processing,
        })
    }
}

impl<C: YouTubeStatusClient> UploadStatusAdapter for YouTubeStatusAdapter<C> {
    fn provider(&self) -> Provider {
        Provider::YouTube
    }

    fn poll_status(
        &self,
        request: &UploadStatusRequest,
    ) -> Result<UploadStatusResult, UploadStatusAdapterError> {
        if request.provider != Provider::YouTube {
            return Err(UploadStatusAdapterError::ProviderMismatch);
        }
        let provider_post_id = request.provider_post_id.trim();
        if provider_post_id.is_empty() {
            return Err(UploadStatusAdapterError::MissingProviderPostId);
        }

        let youtube_request = YouTubeStatusRequest {
            provider_post_id: provider_post_id.to_string(),
            access_token_ref: request.access_token_ref.clone(),
        };
        let response = self
            .client
            .poll_status(&youtube_request)
            .map_err(youtube_status_client_error)?;

        Ok(match response.state {
            YouTubeProcessingState::Processing => UploadStatusResult {
                provider_post_id: response.video_id,
                provider_post_url: None,
                status: UploadProcessingStatus::Processing,
                normalized_error: None,
                raw_error_ref: None,
            },
            YouTubeProcessingState::Processed => UploadStatusResult {
                provider_post_url: Some(format!(
                    "https://www.youtube.com/watch?v={}",
                    response.video_id
                )),
                provider_post_id: response.video_id,
                status: UploadProcessingStatus::Published,
                normalized_error: None,
                raw_error_ref: None,
            },
            YouTubeProcessingState::Failed => {
                let failure_reason = response
                    .failure_reason
                    .unwrap_or_else(|| "unknown".to_string());
                UploadStatusResult {
                    raw_error_ref: Some(format!(
                        "youtube/status/{}/{}",
                        response.video_id, failure_reason
                    )),
                    provider_post_id: response.video_id,
                    provider_post_url: None,
                    status: UploadProcessingStatus::Failed,
                    normalized_error: Some("platform_processing_failed".into()),
                }
            }
        })
    }
}

fn youtube_privacy(privacy: &UploadPrivacy) -> &'static str {
    match privacy {
        UploadPrivacy::Private => "private",
        UploadPrivacy::Unlisted => "unlisted",
        UploadPrivacy::Public => "public",
    }
}

fn youtube_status_client_error(error: YouTubeStatusClientError) -> UploadStatusAdapterError {
    match error {
        YouTubeStatusClientError::NetworkOrServer(message) => {
            UploadStatusAdapterError::NetworkOrServer { message }
        }
    }
}

fn youtube_client_error(error: YouTubeUploadClientError) -> UploadAdapterError {
    match error {
        YouTubeUploadClientError::MissingScope => UploadAdapterError::RequiresAction {
            reason: "missing_scope".into(),
        },
        YouTubeUploadClientError::AccountNotEligible => UploadAdapterError::RequiresAction {
            reason: "account_not_eligible".into(),
        },
        YouTubeUploadClientError::NetworkOrServer(message) => {
            UploadAdapterError::NetworkOrServer { message }
        }
    }
}

// ── Token + artifact resolution seams (always compiled, never networked) ──────

/// Resolves an opaque `access_token_ref` (e.g. `"token_secret:<account_id>"`)
/// to a live bearer token without touching the serializable `UploadRequest`.
pub trait AccessTokenResolver: Send + Sync {
    fn bearer_for(&self, access_token_ref: &str) -> Result<String, AccessTokenResolverError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessTokenResolverError {
    NotFound(String),
    DecryptionFailed(String),
    Expired(String),
    RefreshFailed(String),
}

impl std::fmt::Display for AccessTokenResolverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(s) => write!(f, "token not found: {s}"),
            Self::DecryptionFailed(s) => write!(f, "token decryption failed: {s}"),
            Self::Expired(s) => write!(f, "token expired and could not be refreshed: {s}"),
            Self::RefreshFailed(s) => write!(f, "token refresh failed: {s}"),
        }
    }
}

/// Provides the raw bytes of an artifact (video file) given an opaque `artifact_ref`.
/// The reader also advertises the total byte length for `Content-Length` / `Content-Range`.
pub trait ArtifactSource: Send + Sync {
    fn open(&self, artifact_ref: &str) -> Result<ArtifactBody, ArtifactSourceError>;
}

pub struct ArtifactBody {
    pub total_bytes: u64,
    /// Read the full artifact into memory. Phase 3 loads the whole file;
    /// a future phase may stream from Supabase Storage.
    pub data: Vec<u8>,
}

impl std::fmt::Debug for ArtifactBody {
    /// Elides the byte payload so logs/test failures don't dump the whole file.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtifactBody")
            .field("total_bytes", &self.total_bytes)
            .field("data", &format_args!("<{} bytes>", self.data.len()))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactSourceError {
    NotFound(String),
    IoError(String),
    SizeExceeded { max_bytes: u64, actual_bytes: u64 },
}

impl std::fmt::Display for ArtifactSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(s) => write!(f, "artifact not found: {s}"),
            Self::IoError(s) => write!(f, "artifact IO error: {s}"),
            Self::SizeExceeded {
                max_bytes,
                actual_bytes,
            } => write!(
                f,
                "artifact too large: {actual_bytes} bytes exceeds {max_bytes} byte limit"
            ),
        }
    }
}

/// In-memory resolver for unit tests.
pub struct FixedTokenResolver(pub String);

impl AccessTokenResolver for FixedTokenResolver {
    fn bearer_for(&self, _ref: &str) -> Result<String, AccessTokenResolverError> {
        Ok(self.0.clone())
    }
}

/// In-memory artifact source for unit tests.
pub struct FixedArtifactSource(pub Vec<u8>);

impl ArtifactSource for FixedArtifactSource {
    fn open(&self, _ref: &str) -> Result<ArtifactBody, ArtifactSourceError> {
        Ok(ArtifactBody {
            total_bytes: self.0.len() as u64,
            data: self.0.clone(),
        })
    }
}

// ── Live YouTube upload client ────────────────────────────────────────────────

/// Maximum file size the YouTube Data API accepts (256 GiB).
pub const YOUTUBE_MAX_BYTES: u64 = 256 * 1024 * 1024 * 1024;

/// Default chunk size for resumable upload (8 MiB, must be multiple of 256 KiB).
pub const YOUTUBE_CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// Production base URL for the YouTube resumable-upload endpoint.
pub const YOUTUBE_UPLOAD_BASE: &str = "https://www.googleapis.com/upload/youtube/v3/videos";

/// Production base URL for the YouTube Data API `videos` (status) endpoint.
pub const YOUTUBE_VIDEOS_BASE: &str = "https://www.googleapis.com/youtube/v3/videos";

pub struct YouTubeClientConfig {
    /// When true, forces `privacyStatus = "private"` regardless of the job's
    /// requested privacy. Must be true until the YouTube TOS audit clears.
    pub force_private: bool,
    /// Resumable upload chunk size in bytes. Must be a multiple of 256 KiB.
    pub chunk_size: usize,
    /// Base URL for the resumable-upload initiate POST. Defaults to the real
    /// Google endpoint; overridden in integration tests to point at a mock.
    pub upload_base: String,
}

impl Default for YouTubeClientConfig {
    fn default() -> Self {
        Self {
            force_private: true,
            chunk_size: YOUTUBE_CHUNK_SIZE,
            upload_base: YOUTUBE_UPLOAD_BASE.to_string(),
        }
    }
}

#[cfg(feature = "youtube-live")]
pub mod live {
    use super::*;

    /// Concrete YouTube upload client using the Data API v3 resumable upload protocol.
    ///
    /// The sync `upload_video` and `poll_status` methods bridge to async reqwest via
    /// `tokio::runtime::Handle::current().block_on(...)`. This is intentional: the
    /// FSM and `UploadAdapter` traits are synchronous; only HTTP work enters async.
    pub struct LiveYouTubeUploadClient<R, A> {
        token_resolver: R,
        artifact_source: A,
        config: YouTubeClientConfig,
        http: reqwest::Client,
    }

    impl<R: AccessTokenResolver, A: ArtifactSource> LiveYouTubeUploadClient<R, A> {
        pub fn new(token_resolver: R, artifact_source: A, config: YouTubeClientConfig) -> Self {
            Self {
                token_resolver,
                artifact_source,
                config,
                http: reqwest::Client::new(),
            }
        }

        async fn do_upload(
            &self,
            request: &YouTubeUploadRequest,
            token: String,
            body: ArtifactBody,
        ) -> Result<YouTubeUploadResponse, YouTubeUploadClientError> {
            if body.total_bytes > YOUTUBE_MAX_BYTES {
                return Err(YouTubeUploadClientError::NetworkOrServer(format!(
                    "file too large: {} bytes (max {})",
                    body.total_bytes, YOUTUBE_MAX_BYTES
                )));
            }

            let privacy = if self.config.force_private {
                "private"
            } else {
                request.privacy.as_str()
            };

            // Build the video resource body.
            let snippet = serde_json::json!({
                "title": request.title,
                "description": request.description.as_deref().unwrap_or(""),
                "tags": request.tags,
            });
            let status = serde_json::json!({ "privacyStatus": privacy });
            let video_resource = serde_json::json!({
                "snippet": snippet,
                "status": status,
            });

            // Step 1: initiate the resumable upload session.
            let initiate_resp = self
                .http
                .post(&self.config.upload_base)
                .query(&[("uploadType", "resumable"), ("part", "snippet,status")])
                .bearer_auth(&token)
                .header("X-Upload-Content-Type", "video/*")
                .header("X-Upload-Content-Length", body.total_bytes.to_string())
                .json(&video_resource)
                .send()
                .await
                .map_err(|e| YouTubeUploadClientError::NetworkOrServer(e.to_string()))?;

            let initiate_status = initiate_resp.status().as_u16();
            if initiate_status == 401 {
                return Err(YouTubeUploadClientError::MissingScope);
            }
            if initiate_status == 403 {
                let body_text = initiate_resp.text().await.unwrap_or_default();
                if body_text.contains("insufficientPermissions") || body_text.contains("forbidden")
                {
                    return Err(YouTubeUploadClientError::MissingScope);
                }
                return Err(YouTubeUploadClientError::AccountNotEligible);
            }
            if !initiate_resp.status().is_success() {
                let body_text = initiate_resp.text().await.unwrap_or_default();
                return Err(YouTubeUploadClientError::NetworkOrServer(format!(
                    "initiate {initiate_status}: {body_text}"
                )));
            }

            let session_uri = initiate_resp
                .headers()
                .get("Location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    YouTubeUploadClientError::NetworkOrServer(
                        "initiate response missing Location header".into(),
                    )
                })?
                .to_string();

            // Step 2: upload bytes in chunks via the session URI.
            let total = body.total_bytes as usize;
            let chunk_size = self.config.chunk_size;
            let mut offset = 0usize;

            loop {
                let end = (offset + chunk_size).min(total);
                let chunk = &body.data[offset..end];
                let content_range = if total == 0 {
                    "bytes */*".to_string()
                } else {
                    format!("bytes {}-{}/{}", offset, end - 1, total)
                };

                let chunk_resp = self
                    .http
                    .put(&session_uri)
                    .header("Content-Length", chunk.len().to_string())
                    .header("Content-Range", content_range)
                    .body(chunk.to_vec())
                    .send()
                    .await
                    .map_err(|e| YouTubeUploadClientError::NetworkOrServer(e.to_string()))?;

                let chunk_status = chunk_resp.status().as_u16();

                match chunk_status {
                    200 | 201 => {
                        // Upload complete; parse the video resource.
                        let json: serde_json::Value = chunk_resp.json().await.map_err(|e| {
                            YouTubeUploadClientError::NetworkOrServer(e.to_string())
                        })?;
                        let video_id = json["id"]
                            .as_str()
                            .ok_or_else(|| {
                                YouTubeUploadClientError::NetworkOrServer(
                                    "upload response missing id".into(),
                                )
                            })?
                            .to_string();
                        let upload_status = json["status"]["uploadStatus"]
                            .as_str()
                            .unwrap_or("uploaded");
                        let processing = upload_status == "uploaded"
                            || upload_status == "processing"
                            || upload_status == "processing_upload";
                        return Ok(YouTubeUploadResponse {
                            video_id,
                            processing,
                        });
                    }
                    308 => {
                        // Resume Incomplete — advance offset from Range header.
                        let confirmed = chunk_resp
                            .headers()
                            .get("Range")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|r| r.split('-').nth(1))
                            .and_then(|s| s.parse::<usize>().ok())
                            .map(|last_byte| last_byte + 1)
                            .unwrap_or(offset);
                        offset = confirmed;
                    }
                    401 => return Err(YouTubeUploadClientError::MissingScope),
                    403 => {
                        let body_text = chunk_resp.text().await.unwrap_or_default();
                        if body_text.contains("insufficientPermissions") {
                            return Err(YouTubeUploadClientError::MissingScope);
                        }
                        return Err(YouTubeUploadClientError::AccountNotEligible);
                    }
                    _ => {
                        let body_text = chunk_resp.text().await.unwrap_or_default();
                        return Err(YouTubeUploadClientError::NetworkOrServer(format!(
                            "chunk PUT {chunk_status}: {body_text}"
                        )));
                    }
                }
            }
        }
    }

    impl<R: AccessTokenResolver, A: ArtifactSource> YouTubeUploadClient
        for LiveYouTubeUploadClient<R, A>
    {
        fn upload_video(
            &self,
            request: &YouTubeUploadRequest,
        ) -> Result<YouTubeUploadResponse, YouTubeUploadClientError> {
            let token = self
                .token_resolver
                .bearer_for(&request.access_token_ref)
                .map_err(|e| YouTubeUploadClientError::NetworkOrServer(e.to_string()))?;
            let body = self
                .artifact_source
                .open(&request.artifact_ref)
                .map_err(|e| match e {
                    ArtifactSourceError::SizeExceeded { .. } => {
                        YouTubeUploadClientError::NetworkOrServer(e.to_string())
                    }
                    _ => YouTubeUploadClientError::NetworkOrServer(e.to_string()),
                })?;

            tokio::runtime::Handle::current().block_on(self.do_upload(request, token, body))
        }
    }

    /// Concrete YouTube status client.
    pub struct LiveYouTubeStatusClient<R> {
        token_resolver: R,
        videos_base: String,
        http: reqwest::Client,
    }

    impl<R: AccessTokenResolver> LiveYouTubeStatusClient<R> {
        pub fn new(token_resolver: R) -> Self {
            Self {
                token_resolver,
                videos_base: YOUTUBE_VIDEOS_BASE.to_string(),
                http: reqwest::Client::new(),
            }
        }

        /// Construct with a custom `videos` endpoint base URL (integration tests).
        pub fn with_base(token_resolver: R, videos_base: String) -> Self {
            Self {
                token_resolver,
                videos_base,
                http: reqwest::Client::new(),
            }
        }

        async fn do_poll(
            &self,
            request: &YouTubeStatusRequest,
            token: String,
        ) -> Result<YouTubeStatusResponse, YouTubeStatusClientError> {
            let resp = self
                .http
                .get(&self.videos_base)
                .query(&[
                    ("part", "status,processingDetails"),
                    ("id", &request.provider_post_id),
                ])
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| YouTubeStatusClientError::NetworkOrServer(e.to_string()))?;

            if !resp.status().is_success() {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                return Err(YouTubeStatusClientError::NetworkOrServer(format!(
                    "videos API {status}: {body}"
                )));
            }

            let json: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| YouTubeStatusClientError::NetworkOrServer(e.to_string()))?;

            let item = &json["items"][0];
            let upload_status = item["status"]["uploadStatus"].as_str().unwrap_or("unknown");
            let processing_status = item["processingDetails"]["processingStatus"]
                .as_str()
                .unwrap_or("unknown");
            let rejection_reason = item["status"]["rejectionReason"]
                .as_str()
                .map(ToOwned::to_owned);
            let processing_failure = item["processingDetails"]["processingFailureReason"]
                .as_str()
                .map(ToOwned::to_owned);

            let (state, failure_reason) = match (upload_status, processing_status) {
                (_, "succeeded") | ("processed", _) => (YouTubeProcessingState::Processed, None),
                ("failed", _) | (_, "failed") | (_, "terminated") => {
                    let reason = rejection_reason
                        .or(processing_failure)
                        .unwrap_or_else(|| "unknown".to_string());
                    (YouTubeProcessingState::Failed, Some(reason))
                }
                _ => (YouTubeProcessingState::Processing, None),
            };

            Ok(YouTubeStatusResponse {
                video_id: request.provider_post_id.clone(),
                state,
                failure_reason,
            })
        }
    }

    impl<R: AccessTokenResolver> YouTubeStatusClient for LiveYouTubeStatusClient<R> {
        fn poll_status(
            &self,
            request: &YouTubeStatusRequest,
        ) -> Result<YouTubeStatusResponse, YouTubeStatusClientError> {
            let token = self
                .token_resolver
                .bearer_for(&request.access_token_ref)
                .map_err(|e| YouTubeStatusClientError::NetworkOrServer(e.to_string()))?;

            tokio::runtime::Handle::current().block_on(self.do_poll(request, token))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;
    use crate::upload_adapter::{UploadAdapter, UploadAdapterError, UploadPrivacy, UploadRequest};

    #[derive(Clone, Debug, Default)]
    struct RecordingYouTubeClient {
        response: Option<YouTubeUploadResponse>,
        error: Option<YouTubeUploadClientError>,
    }

    impl YouTubeUploadClient for RecordingYouTubeClient {
        fn upload_video(
            &self,
            request: &YouTubeUploadRequest,
        ) -> Result<YouTubeUploadResponse, YouTubeUploadClientError> {
            assert_eq!(request.artifact_ref, "file:///tmp/render.mp4");
            assert_eq!(request.thumbnail_ref, Some("file:///tmp/thumb.jpg".into()));
            assert_eq!(request.access_token_ref, "token-secret-ref");
            assert_eq!(request.title, "Launch clip");
            assert_eq!(request.description, Some("Description".into()));
            assert_eq!(request.tags, vec!["awidat"]);
            assert_eq!(request.privacy, "private");
            assert_eq!(request.scheduled_for, Some(2_000));
            if let Some(error) = self.error.clone() {
                return Err(error);
            }
            Ok(self
                .response
                .clone()
                .unwrap_or_else(|| YouTubeUploadResponse {
                    video_id: "yt_video_1".into(),
                    processing: false,
                }))
        }
    }

    #[test]
    fn youtube_adapter_maps_upload_response_to_provider_post() {
        let adapter = YouTubeUploadAdapter::new(RecordingYouTubeClient::default());
        let result = adapter
            .upload(&UploadRequest {
                job_id: "job_1".into(),
                provider: Provider::YouTube,
                connected_account_id: "acct_1".into(),
                artifact_ref: "file:///tmp/render.mp4".into(),
                title: "Launch clip".into(),
                description: Some("Description".into()),
                tags: vec!["awidat".into()],
                thumbnail_ref: Some("file:///tmp/thumb.jpg".into()),
                privacy: UploadPrivacy::Private,
                tiktok_interactions: Default::default(),
                scheduled_for: Some(2_000),
                access_token_ref: "token-secret-ref".into(),
            })
            .unwrap_or_else(|err| panic!("youtube upload: {err:?}"));

        assert_eq!(result.provider_post_id, "yt_video_1");
        assert_eq!(
            result.provider_post_url,
            "https://www.youtube.com/watch?v=yt_video_1"
        );
        assert!(!result.processing);
    }

    #[test]
    fn youtube_adapter_rejects_missing_title_and_wrong_provider() {
        let adapter = YouTubeUploadAdapter::new(RecordingYouTubeClient::default());
        let mut request = UploadRequest {
            job_id: "job_1".into(),
            provider: Provider::TikTok,
            connected_account_id: "acct_1".into(),
            artifact_ref: "file:///tmp/render.mp4".into(),
            title: "Launch clip".into(),
            description: None,
            tags: Vec::new(),
            thumbnail_ref: None,
            privacy: UploadPrivacy::Private,
            tiktok_interactions: Default::default(),
            scheduled_for: None,
            access_token_ref: "token-secret-ref".into(),
        };
        assert_eq!(
            adapter.upload(&request),
            Err(UploadAdapterError::ProviderMismatch)
        );

        request.provider = Provider::YouTube;
        request.title = "   ".into();
        assert_eq!(
            adapter.upload(&request),
            Err(UploadAdapterError::MediaConstraintFailed {
                reason: "youtube_title_required".into(),
            })
        );
    }

    #[test]
    fn youtube_adapter_rejects_missing_token_and_maps_client_errors() {
        let adapter = YouTubeUploadAdapter::new(RecordingYouTubeClient::default());
        let mut request = youtube_request();
        request.access_token_ref = "   ".into();

        assert_eq!(
            adapter.upload(&request),
            Err(UploadAdapterError::MissingUploadToken)
        );

        let adapter = YouTubeUploadAdapter::new(RecordingYouTubeClient {
            response: None,
            error: Some(YouTubeUploadClientError::MissingScope),
        });
        assert_eq!(
            adapter.upload(&youtube_request()),
            Err(UploadAdapterError::RequiresAction {
                reason: "missing_scope".into(),
            })
        );

        let adapter = YouTubeUploadAdapter::new(RecordingYouTubeClient {
            response: None,
            error: Some(YouTubeUploadClientError::AccountNotEligible),
        });
        assert_eq!(
            adapter.upload(&youtube_request()),
            Err(UploadAdapterError::RequiresAction {
                reason: "account_not_eligible".into(),
            })
        );

        let adapter = YouTubeUploadAdapter::new(RecordingYouTubeClient {
            response: None,
            error: Some(YouTubeUploadClientError::NetworkOrServer(
                "temporary outage".into(),
            )),
        });
        assert_eq!(
            adapter.upload(&youtube_request()),
            Err(UploadAdapterError::NetworkOrServer {
                message: "temporary outage".into(),
            })
        );
    }

    #[test]
    fn youtube_adapter_maps_privacy_values() {
        assert_eq!(youtube_privacy(&UploadPrivacy::Private), "private");
        assert_eq!(youtube_privacy(&UploadPrivacy::Unlisted), "unlisted");
        assert_eq!(youtube_privacy(&UploadPrivacy::Public), "public");
    }

    fn youtube_request() -> UploadRequest {
        UploadRequest {
            job_id: "job_1".into(),
            provider: Provider::YouTube,
            connected_account_id: "acct_1".into(),
            artifact_ref: "file:///tmp/render.mp4".into(),
            title: "Launch clip".into(),
            description: Some("Description".into()),
            tags: vec!["awidat".into()],
            thumbnail_ref: Some("file:///tmp/thumb.jpg".into()),
            privacy: UploadPrivacy::Private,
            tiktok_interactions: Default::default(),
            scheduled_for: Some(2_000),
            access_token_ref: "token-secret-ref".into(),
        }
    }

    mod youtube_status {
        use super::*;
        use crate::upload_status::{
            UploadProcessingStatus, UploadStatusAdapter, UploadStatusAdapterError,
            UploadStatusRequest,
        };

        #[derive(Clone, Debug, Default)]
        struct RecordingYouTubeStatusClient {
            response: Option<YouTubeStatusResponse>,
            error: Option<YouTubeStatusClientError>,
        }

        impl YouTubeStatusClient for RecordingYouTubeStatusClient {
            fn poll_status(
                &self,
                request: &YouTubeStatusRequest,
            ) -> Result<YouTubeStatusResponse, YouTubeStatusClientError> {
                assert_eq!(request.provider_post_id, "yt_video_1");
                assert_eq!(request.access_token_ref, "token-secret-ref");
                if let Some(error) = self.error.clone() {
                    return Err(error);
                }
                Ok(self
                    .response
                    .clone()
                    .unwrap_or_else(|| YouTubeStatusResponse {
                        video_id: "yt_video_1".into(),
                        state: YouTubeProcessingState::Processed,
                        failure_reason: None,
                    }))
            }
        }

        #[test]
        fn maps_processing_status() {
            let adapter = YouTubeStatusAdapter::new(RecordingYouTubeStatusClient {
                response: Some(YouTubeStatusResponse {
                    video_id: "yt_video_1".into(),
                    state: YouTubeProcessingState::Processing,
                    failure_reason: None,
                }),
                error: None,
            });

            let result = adapter
                .poll_status(&status_request())
                .unwrap_or_else(|err| panic!("poll youtube status: {err:?}"));

            assert_eq!(result.status, UploadProcessingStatus::Processing);
            assert_eq!(result.provider_post_id, "yt_video_1");
            assert_eq!(result.provider_post_url, None);
        }

        #[test]
        fn maps_processed_status_to_published_url() {
            let adapter = YouTubeStatusAdapter::new(RecordingYouTubeStatusClient::default());

            let result = adapter
                .poll_status(&status_request())
                .unwrap_or_else(|err| panic!("poll youtube status: {err:?}"));

            assert_eq!(result.status, UploadProcessingStatus::Published);
            assert_eq!(
                result.provider_post_url.as_deref(),
                Some("https://www.youtube.com/watch?v=yt_video_1")
            );
        }

        #[test]
        fn maps_failed_status_to_provider_failure() {
            let adapter = YouTubeStatusAdapter::new(RecordingYouTubeStatusClient {
                response: Some(YouTubeStatusResponse {
                    video_id: "yt_video_1".into(),
                    state: YouTubeProcessingState::Failed,
                    failure_reason: Some("copyright_claim".into()),
                }),
                error: None,
            });

            let result = adapter
                .poll_status(&status_request())
                .unwrap_or_else(|err| panic!("poll youtube status: {err:?}"));

            assert_eq!(result.status, UploadProcessingStatus::Failed);
            assert_eq!(
                result.normalized_error.as_deref(),
                Some("platform_processing_failed")
            );
            assert_eq!(
                result.raw_error_ref.as_deref(),
                Some("youtube/status/yt_video_1/copyright_claim")
            );
        }

        #[test]
        fn rejects_wrong_provider_and_missing_post_id() {
            let adapter = YouTubeStatusAdapter::new(RecordingYouTubeStatusClient::default());
            let mut request = status_request();
            request.provider = Provider::TikTok;

            assert_eq!(
                adapter.poll_status(&request),
                Err(UploadStatusAdapterError::ProviderMismatch)
            );

            request.provider = Provider::YouTube;
            request.provider_post_id = "   ".into();
            assert_eq!(
                adapter.poll_status(&request),
                Err(UploadStatusAdapterError::MissingProviderPostId)
            );
        }

        fn status_request() -> UploadStatusRequest {
            UploadStatusRequest {
                job_id: "job_1".into(),
                provider: Provider::YouTube,
                connected_account_id: "acct_1".into(),
                provider_post_id: "yt_video_1".into(),
                access_token_ref: "token-secret-ref".into(),
            }
        }
    }
}
