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
