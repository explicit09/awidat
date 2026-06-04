//! Instagram (Graph API) content-publishing adapter (domain layer, no HTTP).
//!
//! Mirrors `youtube_upload.rs` / `tiktok_upload.rs`. The verified Graph API flow
//! (Content Publishing) is a container-then-publish, PULL model:
//!   1. `POST /{ig-user-id}/media` with `media_type` (`REELS`/`VIDEO`/`IMAGE`),
//!      `video_url`/`image_url`, `caption` → returns a container `creation_id`.
//!   2. poll `GET /{creation-id}?fields=status_code` → `IN_PROGRESS` / `FINISHED`
//!      / `ERROR`.
//!   3. on `FINISHED`: `POST /{ig-user-id}/media_publish` with `creation_id` →
//!      `media_id`; then `GET /{media-id}?fields=permalink`.
//!
//! Gate facts (Step 0): Instagram Business/Creator (professional) account +
//! `instagram_content_publish` permission + Meta App Review; the video must be a
//! publicly fetchable URL (PULL, like TikTok → Supabase Storage signed URL);
//! daily publish rate limit historically ~25/24h.
//!
//! Model: the upload adapter creates the container and returns `processing:true`
//! with `provider_post_id = creation_id`. The status adapter polls the container
//! status, calls `media_publish` once `FINISHED`, then resolves the permalink →
//! `Published`. The live HTTP client lives in the server crate.

use crate::model::Provider;
use crate::upload_adapter::{UploadAdapter, UploadAdapterError, UploadRequest, UploadResult};
use crate::upload_status::{
    UploadProcessingStatus, UploadStatusAdapter, UploadStatusAdapterError, UploadStatusRequest,
    UploadStatusResult,
};

/// Default container media type for rendered short-form video.
pub const IG_MEDIA_TYPE_REELS: &str = "REELS";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstagramContainerRequest {
    /// PULL model — a server-reachable signed URL for the artifact.
    pub video_url: String,
    pub caption: String,
    pub media_type: String,
    pub access_token_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstagramContainerResponse {
    pub creation_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstagramPublishResponse {
    pub media_id: String,
    /// Resolved permalink, if the client fetched it alongside publish.
    pub permalink: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstagramContainerState {
    InProgress,
    Finished,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstagramUploadClientError {
    NotProfessional,
    MissingScope,
    RateLimited,
    NetworkOrServer(String),
}

/// HTTP boundary for creating an Instagram media container.
pub trait InstagramUploadClient {
    fn create_container(
        &self,
        request: &InstagramContainerRequest,
    ) -> Result<InstagramContainerResponse, InstagramUploadClientError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstagramStatusClientError {
    NotProfessional,
    MissingScope,
    RateLimited,
    NetworkOrServer(String),
}

/// HTTP boundary the status adapter drives: poll the container, then publish it
/// once it has finished processing.
pub trait InstagramStatusClient {
    fn container_status(
        &self,
        creation_id: &str,
        access_token_ref: &str,
    ) -> Result<InstagramContainerState, InstagramStatusClientError>;

    fn publish_container(
        &self,
        creation_id: &str,
        access_token_ref: &str,
    ) -> Result<InstagramPublishResponse, InstagramStatusClientError>;
}

#[derive(Clone, Debug)]
pub struct InstagramUploadAdapter<C> {
    client: C,
}

impl<C> InstagramUploadAdapter<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

#[derive(Clone, Debug)]
pub struct InstagramStatusAdapter<C> {
    client: C,
}

impl<C> InstagramStatusAdapter<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

impl<C: InstagramUploadClient> UploadAdapter for InstagramUploadAdapter<C> {
    fn provider(&self) -> Provider {
        Provider::Instagram
    }

    fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError> {
        if request.provider != Provider::Instagram {
            return Err(UploadAdapterError::ProviderMismatch);
        }
        if request.access_token_ref.trim().is_empty() {
            return Err(UploadAdapterError::MissingUploadToken);
        }
        // Instagram captions are optional; default to the title or empty.
        let caption = if request.title.trim().is_empty() {
            request.description.clone().unwrap_or_default()
        } else {
            request.title.trim().to_string()
        };

        let container = self
            .client
            .create_container(&InstagramContainerRequest {
                video_url: request.artifact_ref.clone(),
                caption,
                media_type: IG_MEDIA_TYPE_REELS.to_string(),
                access_token_ref: request.access_token_ref.clone(),
            })
            .map_err(instagram_client_error)?;

        Ok(UploadResult {
            // The container must finish processing before publish; the status
            // adapter drives that and resolves the final media id/permalink.
            provider_post_id: container.creation_id,
            provider_post_url: String::new(),
            processing: true,
        })
    }
}

impl<C: InstagramStatusClient> UploadStatusAdapter for InstagramStatusAdapter<C> {
    fn provider(&self) -> Provider {
        Provider::Instagram
    }

    fn poll_status(
        &self,
        request: &UploadStatusRequest,
    ) -> Result<UploadStatusResult, UploadStatusAdapterError> {
        if request.provider != Provider::Instagram {
            return Err(UploadStatusAdapterError::ProviderMismatch);
        }
        let creation_id = request.provider_post_id.trim();
        if creation_id.is_empty() {
            return Err(UploadStatusAdapterError::MissingProviderPostId);
        }

        let state = self
            .client
            .container_status(creation_id, &request.access_token_ref)
            .map_err(instagram_status_client_error)?;

        match state {
            InstagramContainerState::InProgress => Ok(UploadStatusResult {
                provider_post_id: creation_id.to_string(),
                provider_post_url: None,
                status: UploadProcessingStatus::Processing,
                normalized_error: None,
                raw_error_ref: None,
            }),
            InstagramContainerState::Error => Ok(UploadStatusResult {
                raw_error_ref: Some(format!("instagram/container/{creation_id}/error")),
                provider_post_id: creation_id.to_string(),
                provider_post_url: None,
                status: UploadProcessingStatus::Failed,
                normalized_error: Some("platform_processing_failed".into()),
            }),
            InstagramContainerState::Finished => {
                // Container is ready — publish it, then resolve the permalink.
                let published = self
                    .client
                    .publish_container(creation_id, &request.access_token_ref)
                    .map_err(instagram_status_client_error)?;
                Ok(UploadStatusResult {
                    provider_post_url: published.permalink,
                    provider_post_id: published.media_id,
                    status: UploadProcessingStatus::Published,
                    normalized_error: None,
                    raw_error_ref: None,
                })
            }
        }
    }
}

fn instagram_client_error(error: InstagramUploadClientError) -> UploadAdapterError {
    match error {
        InstagramUploadClientError::NotProfessional => UploadAdapterError::RequiresAction {
            reason: "instagram_professional_account_required".into(),
        },
        InstagramUploadClientError::MissingScope => UploadAdapterError::RequiresAction {
            reason: "missing_scope".into(),
        },
        InstagramUploadClientError::RateLimited => UploadAdapterError::NetworkOrServer {
            message: "rate_limited".into(),
        },
        InstagramUploadClientError::NetworkOrServer(message) => {
            UploadAdapterError::NetworkOrServer { message }
        }
    }
}

fn instagram_status_client_error(error: InstagramStatusClientError) -> UploadStatusAdapterError {
    // The status path has no RequiresAction channel (the UploadStatusAdapterError
    // surface is intentionally narrow), so auth/eligibility regressions surface
    // as a retryable server error; the worker's account re-check on the next
    // execute tick flips the account to NeedsReauth.
    let message = match error {
        InstagramStatusClientError::NetworkOrServer(message) => message,
        other => format!("{other:?}"),
    };
    UploadStatusAdapterError::NetworkOrServer { message }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::upload_adapter::UploadPrivacy;
    use std::cell::RefCell;

    fn request() -> UploadRequest {
        UploadRequest {
            job_id: "job_1".into(),
            provider: Provider::Instagram,
            connected_account_id: "acct_1".into(),
            artifact_ref: "https://storage.example/render.mp4".into(),
            title: "Launch clip".into(),
            description: Some("Description".into()),
            tags: vec!["awidat".into()],
            thumbnail_ref: None,
            privacy: UploadPrivacy::Public,
            scheduled_for: Some(2_000),
            access_token_ref: "token-secret-ref".into(),
        }
    }

    #[derive(Default)]
    struct RecordingInstagramClient {
        seen: RefCell<Option<InstagramContainerRequest>>,
        error: Option<InstagramUploadClientError>,
    }

    impl InstagramUploadClient for RecordingInstagramClient {
        fn create_container(
            &self,
            request: &InstagramContainerRequest,
        ) -> Result<InstagramContainerResponse, InstagramUploadClientError> {
            *self.seen.borrow_mut() = Some(request.clone());
            if let Some(error) = self.error.clone() {
                return Err(error);
            }
            Ok(InstagramContainerResponse {
                creation_id: "container_99".into(),
            })
        }
    }

    #[test]
    fn upload_creates_container_and_returns_processing() {
        let adapter = InstagramUploadAdapter::new(RecordingInstagramClient::default());
        let result = adapter
            .upload(&request())
            .unwrap_or_else(|err| panic!("upload: {err:?}"));
        assert_eq!(result.provider_post_id, "container_99");
        assert!(result.processing);
        let seen = adapter.client.seen.borrow().clone().expect("request seen");
        assert_eq!(seen.video_url, "https://storage.example/render.mp4");
        assert_eq!(seen.caption, "Launch clip");
        assert_eq!(seen.media_type, IG_MEDIA_TYPE_REELS);
        assert_eq!(seen.access_token_ref, "token-secret-ref");
    }

    #[test]
    fn upload_rejects_wrong_provider() {
        let adapter = InstagramUploadAdapter::new(RecordingInstagramClient::default());
        let mut req = request();
        req.provider = Provider::YouTube;
        assert_eq!(
            adapter.upload(&req),
            Err(UploadAdapterError::ProviderMismatch)
        );
    }

    #[test]
    fn upload_rejects_empty_token() {
        let adapter = InstagramUploadAdapter::new(RecordingInstagramClient::default());
        let mut req = request();
        req.access_token_ref = "".into();
        assert_eq!(
            adapter.upload(&req),
            Err(UploadAdapterError::MissingUploadToken)
        );
    }

    #[test]
    fn upload_maps_not_professional_to_requires_action() {
        let adapter = InstagramUploadAdapter::new(RecordingInstagramClient {
            seen: RefCell::new(None),
            error: Some(InstagramUploadClientError::NotProfessional),
        });
        assert_eq!(
            adapter.upload(&request()),
            Err(UploadAdapterError::RequiresAction {
                reason: "instagram_professional_account_required".into()
            })
        );
    }

    #[test]
    fn upload_maps_rate_limited_to_network_error() {
        let adapter = InstagramUploadAdapter::new(RecordingInstagramClient {
            seen: RefCell::new(None),
            error: Some(InstagramUploadClientError::RateLimited),
        });
        assert_eq!(
            adapter.upload(&request()),
            Err(UploadAdapterError::NetworkOrServer {
                message: "rate_limited".into()
            })
        );
    }

    // ── Status adapter ──────────────────────────────────────────────────────

    struct StubStatusClient {
        status: InstagramContainerState,
        publish: Result<InstagramPublishResponse, InstagramStatusClientError>,
        publish_calls: RefCell<usize>,
    }

    impl StubStatusClient {
        fn new(status: InstagramContainerState) -> Self {
            Self {
                status,
                publish: Ok(InstagramPublishResponse {
                    media_id: "media_5".into(),
                    permalink: Some("https://instagram.com/p/abc".into()),
                }),
                publish_calls: RefCell::new(0),
            }
        }
    }

    impl InstagramStatusClient for StubStatusClient {
        fn container_status(
            &self,
            _creation_id: &str,
            _token: &str,
        ) -> Result<InstagramContainerState, InstagramStatusClientError> {
            Ok(self.status.clone())
        }

        fn publish_container(
            &self,
            _creation_id: &str,
            _token: &str,
        ) -> Result<InstagramPublishResponse, InstagramStatusClientError> {
            *self.publish_calls.borrow_mut() += 1;
            self.publish.clone()
        }
    }

    fn status_request() -> UploadStatusRequest {
        UploadStatusRequest {
            job_id: "job_1".into(),
            provider: Provider::Instagram,
            connected_account_id: "acct_1".into(),
            provider_post_id: "container_99".into(),
            access_token_ref: "token-secret-ref".into(),
        }
    }

    #[test]
    fn status_in_progress_stays_processing_and_does_not_publish() {
        let adapter =
            InstagramStatusAdapter::new(StubStatusClient::new(InstagramContainerState::InProgress));
        let result = adapter
            .poll_status(&status_request())
            .unwrap_or_else(|err| panic!("status: {err:?}"));
        assert_eq!(result.status, UploadProcessingStatus::Processing);
        assert_eq!(
            *adapter.client.publish_calls.borrow(),
            0,
            "no publish while in progress"
        );
    }

    #[test]
    fn status_finished_publishes_and_resolves_permalink() {
        let adapter =
            InstagramStatusAdapter::new(StubStatusClient::new(InstagramContainerState::Finished));
        let result = adapter
            .poll_status(&status_request())
            .unwrap_or_else(|err| panic!("status: {err:?}"));
        assert_eq!(result.status, UploadProcessingStatus::Published);
        assert_eq!(result.provider_post_id, "media_5");
        assert_eq!(
            result.provider_post_url.as_deref(),
            Some("https://instagram.com/p/abc")
        );
        assert_eq!(
            *adapter.client.publish_calls.borrow(),
            1,
            "published once finished"
        );
    }

    #[test]
    fn status_error_maps_to_failed() {
        let adapter =
            InstagramStatusAdapter::new(StubStatusClient::new(InstagramContainerState::Error));
        let result = adapter
            .poll_status(&status_request())
            .unwrap_or_else(|err| panic!("status: {err:?}"));
        assert_eq!(result.status, UploadProcessingStatus::Failed);
        assert_eq!(
            result.normalized_error.as_deref(),
            Some("platform_processing_failed")
        );
    }

    #[test]
    fn container_request_carries_only_token_ref() {
        let adapter = InstagramUploadAdapter::new(RecordingInstagramClient::default());
        adapter
            .upload(&request())
            .unwrap_or_else(|err| panic!("upload: {err:?}"));
        let seen = adapter.client.seen.borrow().clone().expect("request seen");
        assert_eq!(seen.access_token_ref, "token-secret-ref");
        assert!(!seen.access_token_ref.contains("access_token"));
    }
}
