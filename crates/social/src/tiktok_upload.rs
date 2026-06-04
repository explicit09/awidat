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

/// TikTok `privacy_level` values for the direct-post init call.
pub const TIKTOK_SELF_ONLY: &str = "SELF_ONLY";
pub const TIKTOK_PUBLIC: &str = "PUBLIC_TO_EVERYONE";
pub const TIKTOK_FRIENDS: &str = "MUTUAL_FOLLOW_FRIENDS";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TikTokUploadRequest {
    /// PULL_FROM_URL source — a server-reachable signed URL for the artifact.
    pub video_url: String,
    pub caption: String,
    /// Resolved TikTok `privacy_level` (already clamped where required).
    pub privacy_level: String,
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
    AccountNotEligible,
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

        let tiktok_request = TikTokUploadRequest {
            video_url: request.artifact_ref.clone(),
            caption,
            privacy_level: tiktok_privacy_level(&request.privacy, self.eligible_for_public)
                .to_string(),
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
        TikTokUploadClientError::AccountNotEligible => UploadAdapterError::RequiresAction {
            reason: "account_not_eligible".into(),
        },
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn request(privacy: UploadPrivacy) -> UploadRequest {
        UploadRequest {
            job_id: "job_1".into(),
            provider: Provider::TikTok,
            connected_account_id: "acct_1".into(),
            artifact_ref: "https://storage.example/render.mp4".into(),
            title: "Launch clip".into(),
            description: Some("Description".into()),
            tags: vec!["awidat".into()],
            thumbnail_ref: None,
            privacy,
            scheduled_for: Some(2_000),
            access_token_ref: "token-secret-ref".into(),
        }
    }

    #[derive(Default)]
    struct RecordingTikTokClient {
        seen: RefCell<Option<TikTokUploadRequest>>,
        error: Option<TikTokUploadClientError>,
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
            failing_adapter(TikTokUploadClientError::AccountNotEligible)
                .upload(&request(UploadPrivacy::Private)),
            Err(UploadAdapterError::RequiresAction {
                reason: "account_not_eligible".into()
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
}
