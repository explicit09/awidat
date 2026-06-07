//! TikTok content-publishing adapter (domain layer, no HTTP).
//!
//! Mirrors `youtube_upload.rs`: a mockable `TikTokUploadClient` trait is the
//! HTTP boundary, `TikTokUploadAdapter` implements the shared `UploadAdapter`,
//! and `TikTokStatusAdapter` implements `UploadStatusAdapter`. The live HTTP
//! client lives in the server crate (the domain crate stays HTTP-free).
//!
//! TikTok's direct-post API is always asynchronous: init returns a `publish_id`,
//! and the final post id/share URL only appear once processing completes — so
//! the upload adapter always reports `processing: true` and the status adapter
//! resolves the job. Until the app's `video.publish` audit clears, posting is
//! clamped to `SELF_ONLY` (private) regardless of requested privacy; the worker
//! threads the account's `eligible_for_public` into the adapter so the clamp
//! flips per-platform with no code change.

use crate::model::Provider;
use crate::upload_adapter::{
    UploadAdapter, UploadAdapterError, UploadPrivacy, UploadRequest, UploadResult,
};
use crate::upload_status::{
    UploadProcessingStatus, UploadStatusAdapter, UploadStatusAdapterError, UploadStatusRequest,
    UploadStatusResult,
};
use crate::youtube_upload::{
    AccessTokenResolver, ArtifactBody, ArtifactSource, ArtifactSourceError,
};
use std::time::Duration;

/// TikTok `privacy_level` values for the direct-post init call.
pub const TIKTOK_SELF_ONLY: &str = "SELF_ONLY";
pub const TIKTOK_PUBLIC: &str = "PUBLIC_TO_EVERYONE";
pub const TIKTOK_FRIENDS: &str = "MUTUAL_FOLLOW_FRIENDS";
pub const TIKTOK_CAPTION_MAX_CHARS: usize = 150;
const TIKTOK_HTTP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TikTokUploadRequest {
    /// PULL_FROM_URL source — a server-reachable signed URL for the artifact.
    pub video_url: String,
    pub caption: String,
    /// Resolved TikTok `privacy_level` (already clamped where required).
    pub privacy_level: String,
    pub disable_duet: bool,
    pub disable_comment: bool,
    pub disable_stitch: bool,
    pub access_token_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TikTokInitResponse {
    /// TikTok's handle for the async publish; the final post id/url resolve later.
    pub publish_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TikTokUploadClientError {
    MissingScope,
    AccountNotEligible { reason: String },
    RateLimited,
    NetworkOrServer(String),
}

/// HTTP boundary for initiating a TikTok publish. Implemented live in the server
/// crate; mocked in tests.
pub trait TikTokUploadClient {
    fn init_video_publish(
        &self,
        request: &TikTokUploadRequest,
    ) -> Result<TikTokInitResponse, TikTokUploadClientError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TikTokProcessingState {
    Processing,
    Published,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TikTokStatusRequest {
    pub publish_id: String,
    pub access_token_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TikTokStatusResponse {
    pub publish_id: String,
    pub state: TikTokProcessingState,
    /// Resolved share URL once published.
    pub share_url: Option<String>,
    /// Resolved post id once published.
    pub post_id: Option<String>,
    pub failure_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TikTokStatusClientError {
    NetworkOrServer(String),
}

pub trait TikTokStatusClient {
    fn fetch_status(
        &self,
        request: &TikTokStatusRequest,
    ) -> Result<TikTokStatusResponse, TikTokStatusClientError>;
}

/// Resolve the TikTok `privacy_level` for a requested visibility, applying the
/// sandbox clamp: when the account is not yet cleared for public posting, every
/// upload is forced to `SELF_ONLY`.
pub fn tiktok_privacy_level(privacy: &UploadPrivacy, eligible_for_public: bool) -> &'static str {
    if !eligible_for_public {
        return TIKTOK_SELF_ONLY;
    }
    match privacy {
        UploadPrivacy::Private => TIKTOK_SELF_ONLY,
        UploadPrivacy::Unlisted => TIKTOK_FRIENDS,
        UploadPrivacy::Public => TIKTOK_PUBLIC,
    }
}

#[derive(Clone, Debug)]
pub struct TikTokUploadAdapter<C> {
    client: C,
    /// Whether the account may post publicly (audit cleared). When false the
    /// adapter clamps every upload to `SELF_ONLY`.
    eligible_for_public: bool,
}

impl<C> TikTokUploadAdapter<C> {
    /// Construct an adapter that clamps to private (pre-audit default).
    pub fn new(client: C) -> Self {
        Self {
            client,
            eligible_for_public: false,
        }
    }

    /// Construct an adapter whose public-posting clamp follows the account's
    /// eligibility (the worker passes the eligibility-derived flag).
    pub fn with_public_eligibility(client: C, eligible_for_public: bool) -> Self {
        Self {
            client,
            eligible_for_public,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TikTokStatusAdapter<C> {
    client: C,
}

impl<C> TikTokStatusAdapter<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

impl<C: TikTokUploadClient> UploadAdapter for TikTokUploadAdapter<C> {
    fn provider(&self) -> Provider {
        Provider::TikTok
    }

    fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError> {
        if request.provider != Provider::TikTok {
            return Err(UploadAdapterError::ProviderMismatch);
        }
        if request.access_token_ref.trim().is_empty() {
            return Err(UploadAdapterError::MissingUploadToken);
        }
        // TikTok requires a caption; fall back to the title.
        let caption = if request.title.trim().is_empty() {
            request.description.clone().unwrap_or_default()
        } else {
            request.title.trim().to_string()
        };
        if caption.trim().is_empty() {
            return Err(UploadAdapterError::MediaConstraintFailed {
                reason: "tiktok_caption_required".into(),
            });
        }
        if caption.chars().count() > TIKTOK_CAPTION_MAX_CHARS {
            return Err(UploadAdapterError::MediaConstraintFailed {
                reason: "tiktok_caption_too_long".into(),
            });
        }

        let tiktok_request = TikTokUploadRequest {
            video_url: request.artifact_ref.clone(),
            caption,
            privacy_level: tiktok_privacy_level(&request.privacy, self.eligible_for_public)
                .to_string(),
            disable_duet: request.tiktok_interactions.disable_duet,
            disable_comment: request.tiktok_interactions.disable_comment,
            disable_stitch: request.tiktok_interactions.disable_stitch,
            access_token_ref: request.access_token_ref.clone(),
        };
        let response = self
            .client
            .init_video_publish(&tiktok_request)
            .map_err(tiktok_client_error)?;

        Ok(UploadResult {
            // The post id/url are not known until processing completes; the FSM
            // moves the job to Processing and the status adapter resolves it.
            provider_post_id: response.publish_id,
            provider_post_url: String::new(),
            processing: true,
        })
    }
}

impl<C: TikTokStatusClient> UploadStatusAdapter for TikTokStatusAdapter<C> {
    fn provider(&self) -> Provider {
        Provider::TikTok
    }

    fn poll_status(
        &self,
        request: &UploadStatusRequest,
    ) -> Result<UploadStatusResult, UploadStatusAdapterError> {
        if request.provider != Provider::TikTok {
            return Err(UploadStatusAdapterError::ProviderMismatch);
        }
        let publish_id = request.provider_post_id.trim();
        if publish_id.is_empty() {
            return Err(UploadStatusAdapterError::MissingProviderPostId);
        }

        let response = self
            .client
            .fetch_status(&TikTokStatusRequest {
                publish_id: publish_id.to_string(),
                access_token_ref: request.access_token_ref.clone(),
            })
            .map_err(tiktok_status_client_error)?;

        Ok(match response.state {
            TikTokProcessingState::Processing => UploadStatusResult {
                provider_post_id: response.publish_id,
                provider_post_url: None,
                status: UploadProcessingStatus::Processing,
                normalized_error: None,
                raw_error_ref: None,
            },
            TikTokProcessingState::Published => {
                let post_id = response.post_id.unwrap_or(response.publish_id);
                UploadStatusResult {
                    provider_post_url: response.share_url,
                    provider_post_id: post_id,
                    status: UploadProcessingStatus::Published,
                    normalized_error: None,
                    raw_error_ref: None,
                }
            }
            TikTokProcessingState::Failed => {
                let failure_reason = response
                    .failure_reason
                    .unwrap_or_else(|| "unknown".to_string());
                UploadStatusResult {
                    raw_error_ref: Some(format!(
                        "tiktok/status/{}/{}",
                        response.publish_id, failure_reason
                    )),
                    provider_post_id: response.publish_id,
                    provider_post_url: None,
                    status: UploadProcessingStatus::Failed,
                    normalized_error: Some("platform_processing_failed".into()),
                }
            }
        })
    }
}

fn tiktok_client_error(error: TikTokUploadClientError) -> UploadAdapterError {
    match error {
        TikTokUploadClientError::MissingScope => UploadAdapterError::RequiresAction {
            reason: "missing_scope".into(),
        },
        TikTokUploadClientError::AccountNotEligible { reason } => {
            UploadAdapterError::RequiresAction { reason }
        }
        TikTokUploadClientError::RateLimited => UploadAdapterError::NetworkOrServer {
            message: "rate_limited".into(),
        },
        TikTokUploadClientError::NetworkOrServer(message) => {
            UploadAdapterError::NetworkOrServer { message }
        }
    }
}

fn tiktok_status_client_error(error: TikTokStatusClientError) -> UploadStatusAdapterError {
    match error {
        TikTokStatusClientError::NetworkOrServer(message) => {
            UploadStatusAdapterError::NetworkOrServer { message }
        }
    }
}

pub const TIKTOK_API_BASE: &str = "https://open.tiktokapis.com";
const TIKTOK_SINGLE_UPLOAD_MAX_BYTES: u64 = 128 * 1024 * 1024;

pub struct LiveTikTokUploadClient<R, A> {
    token_resolver: R,
    artifact_source: A,
    api_base: String,
    http: reqwest::Client,
}

impl<R: AccessTokenResolver, A: ArtifactSource> LiveTikTokUploadClient<R, A> {
    pub fn new(token_resolver: R, artifact_source: A) -> Self {
        Self::with_base(token_resolver, artifact_source, TIKTOK_API_BASE.to_string())
    }

    pub fn with_base(token_resolver: R, artifact_source: A, api_base: String) -> Self {
        Self::with_base_and_timeout(
            token_resolver,
            artifact_source,
            api_base,
            TIKTOK_HTTP_TIMEOUT,
        )
    }

    pub fn with_base_and_timeout(
        token_resolver: R,
        artifact_source: A,
        api_base: String,
        timeout: Duration,
    ) -> Self {
        Self {
            token_resolver,
            artifact_source,
            api_base,
            http: reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    async fn do_init(
        &self,
        request: &TikTokUploadRequest,
        token: String,
        artifact: ArtifactBody,
    ) -> Result<TikTokInitResponse, TikTokUploadClientError> {
        if artifact.total_bytes == 0 {
            return Err(TikTokUploadClientError::NetworkOrServer(
                "tiktok upload artifact is empty".into(),
            ));
        }
        if artifact.total_bytes > TIKTOK_SINGLE_UPLOAD_MAX_BYTES {
            return Err(tiktok_size_error(artifact.total_bytes));
        }
        let url = format!(
            "{}/v2/post/publish/video/init/",
            self.api_base.trim_end_matches('/')
        );
        let chunk_size = tiktok_chunk_size(artifact.total_bytes);
        let total_chunk_count = tiktok_total_chunk_count(artifact.total_bytes, chunk_size);
        let body = serde_json::json!({
            "post_info": {
                "title": request.caption,
                "privacy_level": request.privacy_level,
                "disable_duet": request.disable_duet,
                "disable_comment": request.disable_comment,
                "disable_stitch": request.disable_stitch,
                "brand_content_toggle": false,
                "brand_organic_toggle": false,
                "is_aigc": false
            },
            "source_info": {
                "source": "FILE_UPLOAD",
                "video_size": artifact.total_bytes,
                "chunk_size": chunk_size,
                "total_chunk_count": total_chunk_count
            }
        });
        let resp = self
            .http
            .post(url)
            .bearer_auth(token)
            .header("Content-Type", "application/json; charset=UTF-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| TikTokUploadClientError::NetworkOrServer(e.to_string()))?;
        let status = resp.status().as_u16();
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| TikTokUploadClientError::NetworkOrServer(e.to_string()))?;
        let code = json["error"]["code"].as_str().unwrap_or("ok");
        if status == 401 || code == "scope_not_authorized" || code == "access_token_invalid" {
            return Err(TikTokUploadClientError::MissingScope);
        }
        if status == 403 || code == "unaudited_client_can_only_post_to_private_accounts" {
            let reason = if code == "ok" || code.trim().is_empty() {
                "account_not_eligible"
            } else {
                code
            };
            return Err(TikTokUploadClientError::AccountNotEligible {
                reason: reason.to_string(),
            });
        }
        if status == 429 || code == "rate_limit_exceeded" {
            return Err(TikTokUploadClientError::RateLimited);
        }
        if !resp_status_success(status) || code != "ok" {
            return Err(TikTokUploadClientError::NetworkOrServer(format!(
                "tiktok init {status}: {json}"
            )));
        }
        let publish_id = json["data"]["publish_id"]
            .as_str()
            .ok_or_else(|| {
                TikTokUploadClientError::NetworkOrServer(
                    "tiktok init response missing publish_id".into(),
                )
            })?
            .to_string();
        let upload_url = json["data"]["upload_url"]
            .as_str()
            .ok_or_else(|| {
                TikTokUploadClientError::NetworkOrServer(
                    "tiktok init response missing upload_url".into(),
                )
            })?
            .to_string();
        self.do_upload_file(&upload_url, artifact).await?;
        Ok(TikTokInitResponse { publish_id })
    }

    async fn do_upload_file(
        &self,
        upload_url: &str,
        artifact: ArtifactBody,
    ) -> Result<(), TikTokUploadClientError> {
        if artifact.total_bytes > TIKTOK_SINGLE_UPLOAD_MAX_BYTES {
            return Err(tiktok_size_error(artifact.total_bytes));
        }
        let last_byte = artifact.total_bytes.saturating_sub(1);
        let resp = self
            .http
            .put(upload_url)
            .header("Content-Type", "video/mp4")
            .header("Content-Length", artifact.total_bytes.to_string())
            .header(
                "Content-Range",
                format!("bytes 0-{last_byte}/{}", artifact.total_bytes),
            )
            .body(artifact.data)
            .send()
            .await
            .map_err(|e| TikTokUploadClientError::NetworkOrServer(e.to_string()))?;
        let status = resp.status().as_u16();
        if !resp_status_success(status) {
            let body = resp.text().await.unwrap_or_default();
            return Err(TikTokUploadClientError::NetworkOrServer(format!(
                "tiktok upload PUT {status}: {body}"
            )));
        }
        Ok(())
    }
}

impl<R: AccessTokenResolver, A: ArtifactSource> TikTokUploadClient
    for LiveTikTokUploadClient<R, A>
{
    fn init_video_publish(
        &self,
        request: &TikTokUploadRequest,
    ) -> Result<TikTokInitResponse, TikTokUploadClientError> {
        let token = self
            .token_resolver
            .bearer_for(&request.access_token_ref)
            .map_err(|e| TikTokUploadClientError::NetworkOrServer(e.to_string()))?;
        let artifact = self
            .artifact_source
            .open(&request.video_url)
            .map_err(tiktok_artifact_error)?;
        tokio::runtime::Handle::current().block_on(self.do_init(request, token, artifact))
    }
}

fn tiktok_chunk_size(total_bytes: u64) -> u64 {
    total_bytes.min(64 * 1024 * 1024)
}

fn tiktok_total_chunk_count(total_bytes: u64, chunk_size: u64) -> u64 {
    let full_chunks = total_bytes / chunk_size;
    let remainder = total_bytes % chunk_size;
    if remainder == 0 {
        full_chunks.max(1)
    } else {
        full_chunks + 1
    }
}

fn tiktok_artifact_error(error: ArtifactSourceError) -> TikTokUploadClientError {
    TikTokUploadClientError::NetworkOrServer(error.to_string())
}

fn tiktok_size_error(actual_bytes: u64) -> TikTokUploadClientError {
    TikTokUploadClientError::NetworkOrServer(format!(
        "tiktok FILE_UPLOAD currently supports artifacts up to {TIKTOK_SINGLE_UPLOAD_MAX_BYTES} bytes; got {actual_bytes}"
    ))
}

pub struct LiveTikTokStatusClient<R> {
    token_resolver: R,
    api_base: String,
    http: reqwest::Client,
}

impl<R: crate::youtube_upload::AccessTokenResolver> LiveTikTokStatusClient<R> {
    pub fn new(token_resolver: R) -> Self {
        Self::with_base(token_resolver, TIKTOK_API_BASE.to_string())
    }

    pub fn with_base(token_resolver: R, api_base: String) -> Self {
        Self {
            token_resolver,
            api_base,
            http: reqwest::Client::builder()
                .timeout(TIKTOK_HTTP_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    async fn do_fetch(
        &self,
        request: &TikTokStatusRequest,
        token: String,
    ) -> Result<TikTokStatusResponse, TikTokStatusClientError> {
        let url = format!(
            "{}/v2/post/publish/status/fetch/",
            self.api_base.trim_end_matches('/')
        );
        let resp = self
            .http
            .post(url)
            .bearer_auth(token)
            .header("Content-Type", "application/json; charset=UTF-8")
            .json(&serde_json::json!({ "publish_id": request.publish_id }))
            .send()
            .await
            .map_err(|e| TikTokStatusClientError::NetworkOrServer(e.to_string()))?;
        let status = resp.status().as_u16();
        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| TikTokStatusClientError::NetworkOrServer(e.to_string()))?;
        let code = json["error"]["code"].as_str().unwrap_or("ok");
        if !resp_status_success(status) || code != "ok" {
            return Err(TikTokStatusClientError::NetworkOrServer(format!(
                "tiktok status {status}: {json}"
            )));
        }

        let data = &json["data"];
        let publish_id = data["publish_id"]
            .as_str()
            .unwrap_or(&request.publish_id)
            .to_string();
        let state = match data["status"].as_str().unwrap_or("PROCESSING_UPLOAD") {
            "PUBLISH_COMPLETE" | "SEND_TO_USER_INBOX" => TikTokProcessingState::Published,
            "FAILED" | "PUBLISH_FAILED" => TikTokProcessingState::Failed,
            _ => TikTokProcessingState::Processing,
        };
        let post_id = data["publicaly_available_post_id"]
            .as_array()
            .and_then(|ids| ids.first())
            .and_then(|id| id.as_str())
            .map(ToOwned::to_owned)
            .or_else(|| {
                data["publicly_available_post_id"]
                    .as_str()
                    .map(ToOwned::to_owned)
            });
        let share_url = data["share_url"].as_str().map(ToOwned::to_owned);
        let failure_reason = data["fail_reason"]
            .as_str()
            .or_else(|| data["failure_reason"].as_str())
            .map(ToOwned::to_owned);

        Ok(TikTokStatusResponse {
            publish_id,
            state,
            share_url,
            post_id,
            failure_reason,
        })
    }
}

impl<R: crate::youtube_upload::AccessTokenResolver> TikTokStatusClient
    for LiveTikTokStatusClient<R>
{
    fn fetch_status(
        &self,
        request: &TikTokStatusRequest,
    ) -> Result<TikTokStatusResponse, TikTokStatusClientError> {
        let token = self
            .token_resolver
            .bearer_for(&request.access_token_ref)
            .map_err(|e| TikTokStatusClientError::NetworkOrServer(e.to_string()))?;
        tokio::runtime::Handle::current().block_on(self.do_fetch(request, token))
    }
}

fn resp_status_success(status: u16) -> bool {
    (200..300).contains(&status)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::youtube_upload::{ArtifactBody, ArtifactSource};
    use std::cell::RefCell;

    fn request(privacy: UploadPrivacy) -> UploadRequest {
        UploadRequest {
            job_id: "job_1".into(),
            provider: Provider::TikTok,
            connected_account_id: "acct_1".into(),
            artifact_ref: "https://storage.example/render.mp4".into(),
            title: "Launch clip".into(),
            description: Some("Description".into()),
            tags: vec!["montage".into()],
            thumbnail_ref: None,
            privacy,
            tiktok_interactions: Default::default(),
            scheduled_for: Some(2_000),
            access_token_ref: "token-secret-ref".into(),
        }
    }

    #[derive(Default)]
    struct RecordingTikTokClient {
        seen: RefCell<Option<TikTokUploadRequest>>,
        error: Option<TikTokUploadClientError>,
    }

    struct OversizedArtifactSource;

    impl ArtifactSource for OversizedArtifactSource {
        fn open(&self, _artifact_ref: &str) -> Result<ArtifactBody, ArtifactSourceError> {
            Ok(ArtifactBody {
                total_bytes: TIKTOK_SINGLE_UPLOAD_MAX_BYTES + 1,
                data: Vec::new(),
            })
        }
    }

    impl TikTokUploadClient for RecordingTikTokClient {
        fn init_video_publish(
            &self,
            request: &TikTokUploadRequest,
        ) -> Result<TikTokInitResponse, TikTokUploadClientError> {
            *self.seen.borrow_mut() = Some(request.clone());
            if let Some(error) = self.error.clone() {
                return Err(error);
            }
            Ok(TikTokInitResponse {
                publish_id: "pub_123".into(),
            })
        }
    }

    #[test]
    fn upload_maps_init_to_processing_result() {
        let adapter =
            TikTokUploadAdapter::with_public_eligibility(RecordingTikTokClient::default(), true);
        let result = adapter
            .upload(&request(UploadPrivacy::Public))
            .unwrap_or_else(|err| panic!("upload: {err:?}"));

        assert_eq!(result.provider_post_id, "pub_123");
        assert!(result.processing, "TikTok is always async");
        assert_eq!(result.provider_post_url, "");
        let seen = adapter.client.seen.borrow().clone().expect("request seen");
        assert_eq!(seen.video_url, "https://storage.example/render.mp4");
        assert_eq!(seen.caption, "Launch clip");
        assert_eq!(seen.privacy_level, TIKTOK_PUBLIC);
        assert_eq!(seen.access_token_ref, "token-secret-ref");
    }

    #[test]
    fn upload_forwards_tiktok_interaction_settings() {
        let adapter =
            TikTokUploadAdapter::with_public_eligibility(RecordingTikTokClient::default(), true);
        let mut upload_request = request(UploadPrivacy::Private);
        upload_request.tiktok_interactions.disable_duet = true;
        upload_request.tiktok_interactions.disable_comment = true;
        upload_request.tiktok_interactions.disable_stitch = true;

        adapter
            .upload(&upload_request)
            .unwrap_or_else(|err| panic!("upload: {err:?}"));

        let seen = adapter.client.seen.borrow().clone().expect("request seen");
        assert!(seen.disable_duet);
        assert!(seen.disable_comment);
        assert!(seen.disable_stitch);
    }

    #[test]
    fn upload_clamps_privacy_to_self_only_when_not_eligible_for_public() {
        // Requested Public, but account is not audit-cleared → SELF_ONLY.
        let adapter = TikTokUploadAdapter::new(RecordingTikTokClient::default());
        adapter
            .upload(&request(UploadPrivacy::Public))
            .unwrap_or_else(|err| panic!("upload: {err:?}"));
        let seen = adapter.client.seen.borrow().clone().expect("request seen");
        assert_eq!(seen.privacy_level, TIKTOK_SELF_ONLY);
    }

    #[test]
    fn privacy_level_mapping_respects_eligibility() {
        assert_eq!(
            tiktok_privacy_level(&UploadPrivacy::Public, true),
            TIKTOK_PUBLIC
        );
        assert_eq!(
            tiktok_privacy_level(&UploadPrivacy::Unlisted, true),
            TIKTOK_FRIENDS
        );
        assert_eq!(
            tiktok_privacy_level(&UploadPrivacy::Private, true),
            TIKTOK_SELF_ONLY
        );
        // Clamp wins regardless of requested level.
        assert_eq!(
            tiktok_privacy_level(&UploadPrivacy::Public, false),
            TIKTOK_SELF_ONLY
        );
    }

    #[test]
    fn upload_rejects_wrong_provider() {
        let adapter = TikTokUploadAdapter::new(RecordingTikTokClient::default());
        let mut req = request(UploadPrivacy::Private);
        req.provider = Provider::YouTube;
        assert_eq!(
            adapter.upload(&req),
            Err(UploadAdapterError::ProviderMismatch)
        );
    }

    #[test]
    fn upload_rejects_empty_token() {
        let adapter = TikTokUploadAdapter::new(RecordingTikTokClient::default());
        let mut req = request(UploadPrivacy::Private);
        req.access_token_ref = "  ".into();
        assert_eq!(
            adapter.upload(&req),
            Err(UploadAdapterError::MissingUploadToken)
        );
    }

    #[test]
    fn upload_rejects_empty_caption() {
        let adapter = TikTokUploadAdapter::new(RecordingTikTokClient::default());
        let mut req = request(UploadPrivacy::Private);
        req.title = "   ".into();
        req.description = None;
        assert_eq!(
            adapter.upload(&req),
            Err(UploadAdapterError::MediaConstraintFailed {
                reason: "tiktok_caption_required".into(),
            })
        );
    }

    #[test]
    fn upload_rejects_caption_over_tiktok_limit() {
        let adapter = TikTokUploadAdapter::new(RecordingTikTokClient::default());
        let mut req = request(UploadPrivacy::Private);
        req.title = "x".repeat(151);

        assert_eq!(
            adapter.upload(&req),
            Err(UploadAdapterError::MediaConstraintFailed {
                reason: "tiktok_caption_too_long".into(),
            })
        );
    }

    fn failing_adapter(
        error: TikTokUploadClientError,
    ) -> TikTokUploadAdapter<RecordingTikTokClient> {
        TikTokUploadAdapter::new(RecordingTikTokClient {
            seen: RefCell::new(None),
            error: Some(error),
        })
    }

    #[test]
    fn upload_maps_client_errors() {
        assert_eq!(
            failing_adapter(TikTokUploadClientError::MissingScope)
                .upload(&request(UploadPrivacy::Private)),
            Err(UploadAdapterError::RequiresAction {
                reason: "missing_scope".into()
            })
        );
        assert_eq!(
            failing_adapter(TikTokUploadClientError::AccountNotEligible {
                reason: "url_ownership_unverified".into(),
            })
            .upload(&request(UploadPrivacy::Private)),
            Err(UploadAdapterError::RequiresAction {
                reason: "url_ownership_unverified".into()
            })
        );
        assert_eq!(
            failing_adapter(TikTokUploadClientError::RateLimited)
                .upload(&request(UploadPrivacy::Private)),
            Err(UploadAdapterError::NetworkOrServer {
                message: "rate_limited".into()
            })
        );
        assert_eq!(
            failing_adapter(TikTokUploadClientError::NetworkOrServer("boom".into()))
                .upload(&request(UploadPrivacy::Private)),
            Err(UploadAdapterError::NetworkOrServer {
                message: "boom".into()
            })
        );
    }

    // ── Status adapter ──────────────────────────────────────────────────────

    struct StubStatusClient {
        response: TikTokStatusResponse,
    }

    impl TikTokStatusClient for StubStatusClient {
        fn fetch_status(
            &self,
            _request: &TikTokStatusRequest,
        ) -> Result<TikTokStatusResponse, TikTokStatusClientError> {
            Ok(self.response.clone())
        }
    }

    fn status_request() -> UploadStatusRequest {
        UploadStatusRequest {
            job_id: "job_1".into(),
            provider: Provider::TikTok,
            connected_account_id: "acct_1".into(),
            provider_post_id: "pub_123".into(),
            access_token_ref: "token-secret-ref".into(),
        }
    }

    #[test]
    fn status_maps_processing() {
        let adapter = TikTokStatusAdapter::new(StubStatusClient {
            response: TikTokStatusResponse {
                publish_id: "pub_123".into(),
                state: TikTokProcessingState::Processing,
                share_url: None,
                post_id: None,
                failure_reason: None,
            },
        });
        let result = adapter
            .poll_status(&status_request())
            .unwrap_or_else(|err| panic!("status: {err:?}"));
        assert_eq!(result.status, UploadProcessingStatus::Processing);
        assert_eq!(result.provider_post_url, None);
    }

    #[test]
    fn status_maps_published_with_share_url() {
        let adapter = TikTokStatusAdapter::new(StubStatusClient {
            response: TikTokStatusResponse {
                publish_id: "pub_123".into(),
                state: TikTokProcessingState::Published,
                share_url: Some("https://www.tiktok.com/@a/video/9".into()),
                post_id: Some("9".into()),
                failure_reason: None,
            },
        });
        let result = adapter
            .poll_status(&status_request())
            .unwrap_or_else(|err| panic!("status: {err:?}"));
        assert_eq!(result.status, UploadProcessingStatus::Published);
        assert_eq!(result.provider_post_id, "9");
        assert_eq!(
            result.provider_post_url.as_deref(),
            Some("https://www.tiktok.com/@a/video/9")
        );
    }

    #[test]
    fn status_maps_failed() {
        let adapter = TikTokStatusAdapter::new(StubStatusClient {
            response: TikTokStatusResponse {
                publish_id: "pub_123".into(),
                state: TikTokProcessingState::Failed,
                share_url: None,
                post_id: None,
                failure_reason: Some("spam_risk".into()),
            },
        });
        let result = adapter
            .poll_status(&status_request())
            .unwrap_or_else(|err| panic!("status: {err:?}"));
        assert_eq!(result.status, UploadProcessingStatus::Failed);
        assert_eq!(
            result.normalized_error.as_deref(),
            Some("platform_processing_failed")
        );
        assert_eq!(
            result.raw_error_ref.as_deref(),
            Some("tiktok/status/pub_123/spam_risk")
        );
    }

    #[test]
    fn upload_request_redacts_token_material() {
        // The TikTok request carries only the opaque token ref, never a real
        // token. (TikTokUploadRequest is not Serialize by design — assert the
        // field is the ref string, matching the redaction property.)
        let adapter = TikTokUploadAdapter::new(RecordingTikTokClient::default());
        adapter
            .upload(&request(UploadPrivacy::Private))
            .unwrap_or_else(|err| panic!("upload: {err:?}"));
        let seen = adapter.client.seen.borrow().clone().expect("request seen");
        assert_eq!(seen.access_token_ref, "token-secret-ref");
        assert!(!seen.access_token_ref.contains("access_token"));
    }

    #[tokio::test]
    async fn live_tiktok_upload_client_initializes_direct_post_with_file_upload() {
        use crate::youtube_upload::{FixedArtifactSource, FixedTokenResolver};
        use wiremock::matchers::{bearer_token, body_json, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let upload_url = format!("{}/video/upload/session_1", server.uri());
        let artifact = vec![7; 6 * 1024 * 1024 + 123];
        let artifact_len = artifact.len() as u64;
        let last_byte = artifact_len - 1;
        Mock::given(method("POST"))
            .and(path("/v2/post/publish/video/init/"))
            .and(bearer_token("tt-access"))
            .and(body_json(serde_json::json!({
                "post_info": {
                    "title": "Launch clip",
                    "privacy_level": "SELF_ONLY",
                    "disable_duet": false,
                    "disable_comment": false,
                    "disable_stitch": false,
                    "brand_content_toggle": false,
                    "brand_organic_toggle": false,
                    "is_aigc": false
                },
                "source_info": {
                    "source": "FILE_UPLOAD",
                    "video_size": artifact_len,
                    "chunk_size": artifact_len,
                    "total_chunk_count": 1
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "publish_id": "v_pub_url~v2.123",
                    "upload_url": upload_url
                },
                "error": { "code": "ok", "message": "", "log_id": "log_1" }
            })))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/video/upload/session_1"))
            .and(header("content-type", "video/mp4"))
            .and(header("content-length", artifact_len.to_string()))
            .and(header(
                "content-range",
                format!("bytes 0-{last_byte}/{artifact_len}"),
            ))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let client = LiveTikTokUploadClient::with_base(
            FixedTokenResolver("tt-access".into()),
            FixedArtifactSource(artifact),
            server.uri(),
        );
        let response = tokio::task::spawn_blocking(move || {
            client.init_video_publish(&TikTokUploadRequest {
                video_url: "https://storage.example/render.mp4".into(),
                caption: "Launch clip".into(),
                privacy_level: TIKTOK_SELF_ONLY.into(),
                disable_duet: false,
                disable_comment: false,
                disable_stitch: false,
                access_token_ref: "token_secret:acct_1".into(),
            })
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(response.publish_id, "v_pub_url~v2.123");
    }

    #[tokio::test]
    async fn live_tiktok_upload_client_rejects_oversized_artifact_before_init() {
        use crate::youtube_upload::FixedTokenResolver;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/post/publish/video/init/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "publish_id": "should_not_start",
                    "upload_url": format!("{}/video/upload/session_1", server.uri())
                },
                "error": { "code": "ok", "message": "", "log_id": "log_1" }
            })))
            .expect(0)
            .mount(&server)
            .await;

        let client = LiveTikTokUploadClient::with_base(
            FixedTokenResolver("tt-access".into()),
            OversizedArtifactSource,
            server.uri(),
        );
        let response = tokio::task::spawn_blocking(move || {
            client.init_video_publish(&TikTokUploadRequest {
                video_url: "https://storage.example/render.mp4".into(),
                caption: "Launch clip".into(),
                privacy_level: TIKTOK_SELF_ONLY.into(),
                disable_duet: false,
                disable_comment: false,
                disable_stitch: false,
                access_token_ref: "token_secret:acct_1".into(),
            })
        })
        .await
        .unwrap();

        assert_eq!(
            response,
            Err(TikTokUploadClientError::NetworkOrServer(format!(
                "tiktok FILE_UPLOAD currently supports artifacts up to {} bytes; got {}",
                TIKTOK_SINGLE_UPLOAD_MAX_BYTES,
                TIKTOK_SINGLE_UPLOAD_MAX_BYTES + 1
            )))
        );
    }

    #[tokio::test]
    async fn live_tiktok_upload_client_preserves_forbidden_provider_code() {
        use crate::youtube_upload::{FixedArtifactSource, FixedTokenResolver};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/post/publish/video/init/"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "data": {},
                "error": {
                    "code": "url_ownership_unverified",
                    "message": "verify the URL domain",
                    "log_id": "log_domain"
                }
            })))
            .mount(&server)
            .await;

        let client = LiveTikTokUploadClient::with_base(
            FixedTokenResolver("tt-access".into()),
            FixedArtifactSource(vec![1, 2, 3, 4]),
            server.uri(),
        );
        let response = tokio::task::spawn_blocking(move || {
            client.init_video_publish(&TikTokUploadRequest {
                video_url: "https://storage.example/render.mp4".into(),
                caption: "Launch clip".into(),
                privacy_level: TIKTOK_SELF_ONLY.into(),
                disable_duet: false,
                disable_comment: false,
                disable_stitch: false,
                access_token_ref: "token_secret:acct_1".into(),
            })
        })
        .await
        .unwrap();

        assert_eq!(
            response,
            Err(TikTokUploadClientError::AccountNotEligible {
                reason: "url_ownership_unverified".into(),
            })
        );
    }

    #[tokio::test]
    async fn live_tiktok_upload_client_times_out_provider_init() {
        use crate::youtube_upload::{FixedArtifactSource, FixedTokenResolver};
        use std::time::Duration;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/post/publish/video/init/"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
            .mount(&server)
            .await;

        let client = LiveTikTokUploadClient::with_base_and_timeout(
            FixedTokenResolver("tt-access".into()),
            FixedArtifactSource(vec![1, 2, 3, 4]),
            server.uri(),
            Duration::from_millis(50),
        );
        let response = tokio::task::spawn_blocking(move || {
            client.init_video_publish(&TikTokUploadRequest {
                video_url: "https://storage.example/render.mp4".into(),
                caption: "Launch clip".into(),
                privacy_level: TIKTOK_SELF_ONLY.into(),
                disable_duet: false,
                disable_comment: false,
                disable_stitch: false,
                access_token_ref: "token_secret:acct_1".into(),
            })
        })
        .await
        .unwrap();

        assert!(
            matches!(response, Err(TikTokUploadClientError::NetworkOrServer(_))),
            "slow TikTok init should fail instead of blocking the scheduler tick"
        );
    }

    #[tokio::test]
    async fn live_tiktok_status_client_maps_publish_complete() {
        use crate::youtube_upload::FixedTokenResolver;
        use wiremock::matchers::{bearer_token, body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v2/post/publish/status/fetch/"))
            .and(bearer_token("tt-access"))
            .and(body_json(serde_json::json!({
                "publish_id": "v_pub_url~v2.123"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "publish_id": "v_pub_url~v2.123",
                    "status": "PUBLISH_COMPLETE",
                    "publicaly_available_post_id": ["7123456789"],
                    "share_url": "https://www.tiktok.com/@creator/video/7123456789"
                },
                "error": { "code": "ok", "message": "", "log_id": "log_1" }
            })))
            .mount(&server)
            .await;

        let client =
            LiveTikTokStatusClient::with_base(FixedTokenResolver("tt-access".into()), server.uri());
        let response = tokio::task::spawn_blocking(move || {
            client.fetch_status(&TikTokStatusRequest {
                publish_id: "v_pub_url~v2.123".into(),
                access_token_ref: "token_secret:acct_1".into(),
            })
        })
        .await
        .unwrap()
        .unwrap();

        assert_eq!(response.state, TikTokProcessingState::Published);
        assert_eq!(response.post_id.as_deref(), Some("7123456789"));
        assert_eq!(
            response.share_url.as_deref(),
            Some("https://www.tiktok.com/@creator/video/7123456789")
        );
    }
}
