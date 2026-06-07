use crate::model::{
    ConnectedAccountStatus, Provider, PublishJob, PublishJobActorType, PublishJobEvent,
    PublishJobEventType, PublishJobStatus,
};
use crate::store::{SocialStore, SocialStoreError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadProcessingStatus {
    Processing,
    Published,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollUploadStatusInput {
    pub job_id: String,
    pub now: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadStatusRequest {
    pub job_id: String,
    pub provider: Provider,
    pub connected_account_id: String,
    pub provider_post_id: String,
    #[serde(rename = "token_ref")]
    pub access_token_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadStatusResult {
    pub provider_post_id: String,
    pub provider_post_url: Option<String>,
    pub status: UploadProcessingStatus,
    pub normalized_error: Option<String>,
    pub raw_error_ref: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum UploadStatusAdapterError {
    #[error("provider mismatch")]
    ProviderMismatch,
    #[error("missing provider post id")]
    MissingProviderPostId,
    #[error("network or server error: {message}")]
    NetworkOrServer { message: String },
}

pub trait UploadStatusAdapter {
    fn provider(&self) -> Provider;
    fn poll_status(
        &self,
        request: &UploadStatusRequest,
    ) -> Result<UploadStatusResult, UploadStatusAdapterError>;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UploadStatusServiceError {
    #[error(transparent)]
    Store(#[from] SocialStoreError),
    #[error("publish job is not processing")]
    JobNotProcessing,
    #[error("upload provider does not match publish job")]
    ProviderMismatch,
    #[error("processing job does not have a provider post id")]
    MissingProviderPostId,
    #[error("provider status response post id does not match processing job")]
    ProviderPostIdMismatch,
}

pub struct UploadStatusService;

impl UploadStatusService {
    pub fn poll_processing_job(
        store: &mut impl SocialStore,
        adapter: &impl UploadStatusAdapter,
        input: PollUploadStatusInput,
    ) -> Result<PublishJob, UploadStatusServiceError> {
        let job = store.publish_job(&input.job_id)?;
        if job.status != PublishJobStatus::Processing {
            return Err(UploadStatusServiceError::JobNotProcessing);
        }
        if adapter.provider() != job.provider {
            return Err(UploadStatusServiceError::ProviderMismatch);
        }

        let account = store.connected_account(&job.connected_account_id)?;
        if account.provider != job.provider {
            return Err(UploadStatusServiceError::ProviderMismatch);
        }
        if let Some(reason) = account_status_requires_action_reason(&account.status) {
            return persist_requires_action(store, job, reason, input.now);
        }
        if !account.eligibility.eligible {
            let reason = account
                .eligibility
                .reasons
                .first()
                .cloned()
                .unwrap_or_else(|| "account_not_eligible".into());
            return persist_requires_action(store, job, reason, input.now);
        }
        if !account.capabilities.upload_video {
            return persist_requires_action(store, job, "missing_publish_capability", input.now);
        }
        let provider_post_id = job
            .provider_post_id
            .clone()
            .ok_or(UploadStatusServiceError::MissingProviderPostId)?;
        let _token_secret = store.token_secret_for_account(&account.id)?;

        let request = UploadStatusRequest {
            job_id: job.id.clone(),
            provider: job.provider.clone(),
            connected_account_id: account.id.clone(),
            provider_post_id: provider_post_id.clone(),
            access_token_ref: format!("token_secret:{}", account.id),
        };
        let result = adapter
            .poll_status(&request)
            .map_err(status_adapter_error)?;
        if result.status != UploadProcessingStatus::Published
            && result.provider_post_id != provider_post_id
        {
            return Err(UploadStatusServiceError::ProviderPostIdMismatch);
        }

        let next = match result.status {
            UploadProcessingStatus::Processing => job.processing(provider_post_id, input.now),
            UploadProcessingStatus::Published => job.publish_with_optional_url(
                result.provider_post_id.clone(),
                result.provider_post_url.clone(),
                input.now,
            ),
            UploadProcessingStatus::Failed => job.fail(
                result
                    .normalized_error
                    .clone()
                    .unwrap_or_else(|| "platform_processing_failed".into()),
                result
                    .raw_error_ref
                    .clone()
                    .unwrap_or_else(|| "provider_error_ref_unavailable".into()),
                input.now,
            ),
        };
        store.save_publish_job(next.clone())?;
        append_status_event(store, &next, &result, input.now)?;
        Ok(next)
    }
}

fn status_adapter_error(error: UploadStatusAdapterError) -> UploadStatusServiceError {
    match error {
        UploadStatusAdapterError::ProviderMismatch => UploadStatusServiceError::ProviderMismatch,
        UploadStatusAdapterError::MissingProviderPostId => {
            UploadStatusServiceError::MissingProviderPostId
        }
        UploadStatusAdapterError::NetworkOrServer { message } => {
            UploadStatusServiceError::Store(SocialStoreError::Storage(message))
        }
    }
}

fn append_status_event(
    store: &mut impl SocialStore,
    job: &PublishJob,
    result: &UploadStatusResult,
    now: i64,
) -> Result<(), UploadStatusServiceError> {
    let event_id = next_event_id(store, &job.id)?;
    store.append_publish_job_event(PublishJobEvent::new(
        event_id,
        job.id.clone(),
        PublishJobEventType::StatusPolled,
        PublishJobActorType::Provider,
        "provider upload status polled",
        serde_json::json!({
            "provider": job.provider.as_str(),
            "provider_post_id": result.provider_post_id,
            "provider_post_url": result.provider_post_url,
            "status": result.status,
            "normalized_error": result.normalized_error,
            "raw_error_ref": result.raw_error_ref,
        }),
        now,
    ))?;
    Ok(())
}

fn persist_requires_action(
    store: &mut impl SocialStore,
    job: PublishJob,
    reason: impl Into<String>,
    now: i64,
) -> Result<PublishJob, UploadStatusServiceError> {
    let reason = reason.into();
    let next = job.requires_action(reason.clone(), now);
    store.save_publish_job(next.clone())?;
    append_requires_action_event(store, &next, &reason, now)?;
    Ok(next)
}

fn append_requires_action_event(
    store: &mut impl SocialStore,
    job: &PublishJob,
    reason: &str,
    now: i64,
) -> Result<(), UploadStatusServiceError> {
    let event_id = next_requires_action_event_id(store, &job.id)?;
    store.append_publish_job_event(PublishJobEvent::new(
        event_id,
        job.id.clone(),
        PublishJobEventType::RequiresAction,
        PublishJobActorType::Worker,
        "provider status poll requires action",
        serde_json::json!({
            "provider": job.provider.as_str(),
            "reason": reason,
        }),
        now,
    ))?;
    Ok(())
}

fn account_status_requires_action_reason(status: &ConnectedAccountStatus) -> Option<&'static str> {
    match status {
        ConnectedAccountStatus::Connected => None,
        ConnectedAccountStatus::NeedsReauth => Some("account_needs_reauth"),
        ConnectedAccountStatus::MissingScope => Some("missing_scope"),
        ConnectedAccountStatus::Ineligible => Some("account_not_eligible"),
        ConnectedAccountStatus::Disabled => Some("account_disabled"),
        ConnectedAccountStatus::Revoked => Some("account_revoked"),
    }
}

fn next_event_id(
    store: &impl SocialStore,
    job_id: &str,
) -> Result<String, UploadStatusServiceError> {
    let sequence = store.publish_job_events(job_id)?.len() + 1;
    Ok(format!("event_{job_id}_status_polled_{sequence}"))
}

fn next_requires_action_event_id(
    store: &impl SocialStore,
    job_id: &str,
) -> Result<String, UploadStatusServiceError> {
    let sequence = store.publish_job_events(job_id)?.len() + 1;
    Ok(format!("event_{job_id}_requires_action_{sequence}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef,
        Provider, ProviderCapabilities, PublishJob, PublishJobActorType, PublishJobEventType,
        PublishJobStatus,
    };
    use crate::store::{InMemorySocialStore, SocialStore};
    use crate::token::{TestKeyProvider, TokenSecret};
    use std::cell::RefCell;

    #[derive(Debug)]
    struct RecordingStatusAdapter {
        provider: Provider,
        result: Result<UploadStatusResult, UploadStatusAdapterError>,
        requests: RefCell<Vec<UploadStatusRequest>>,
    }

    impl RecordingStatusAdapter {
        fn new(result: UploadStatusResult) -> Self {
            Self {
                provider: Provider::YouTube,
                result: Ok(result),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn failing(error: UploadStatusAdapterError) -> Self {
            Self {
                provider: Provider::YouTube,
                result: Err(error),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn call_count(&self) -> usize {
            self.requests.borrow().len()
        }
    }

    impl UploadStatusAdapter for RecordingStatusAdapter {
        fn provider(&self) -> Provider {
            self.provider.clone()
        }

        fn poll_status(
            &self,
            request: &UploadStatusRequest,
        ) -> Result<UploadStatusResult, UploadStatusAdapterError> {
            self.requests.borrow_mut().push(request.clone());
            self.result.clone()
        }
    }

    #[test]
    fn poll_processing_job_publishes_when_provider_status_is_ready() {
        let mut store = store_with_processing_job();
        let adapter = RecordingStatusAdapter::new(UploadStatusResult {
            provider_post_id: "yt_video_1".into(),
            provider_post_url: Some("https://www.youtube.com/watch?v=yt_video_1".into()),
            status: UploadProcessingStatus::Published,
            normalized_error: None,
            raw_error_ref: None,
        });

        let job = UploadStatusService::poll_processing_job(
            &mut store,
            &adapter,
            PollUploadStatusInput {
                job_id: "job_1".into(),
                now: 2_500,
            },
        )
        .unwrap_or_else(|err| panic!("poll status: {err}"));

        assert_eq!(job.status, PublishJobStatus::Published);
        assert_eq!(
            job.provider_post_url.as_deref(),
            Some("https://www.youtube.com/watch?v=yt_video_1")
        );
        assert_eq!(adapter.call_count(), 1);
        let events = store
            .publish_job_events("job_1")
            .unwrap_or_else(|err| panic!("events: {err}"));
        assert_eq!(events[0].event_type, PublishJobEventType::StatusPolled);
        assert_eq!(events[0].actor_type, PublishJobActorType::Provider);
    }

    #[test]
    fn poll_processing_job_publishes_without_url_for_private_provider_posts() {
        let mut store = store_with_processing_job();
        let adapter = RecordingStatusAdapter::new(UploadStatusResult {
            provider_post_id: "yt_video_1".into(),
            provider_post_url: None,
            status: UploadProcessingStatus::Published,
            normalized_error: None,
            raw_error_ref: None,
        });

        let job = UploadStatusService::poll_processing_job(
            &mut store,
            &adapter,
            PollUploadStatusInput {
                job_id: "job_1".into(),
                now: 2_500,
            },
        )
        .unwrap_or_else(|err| panic!("poll status: {err}"));

        assert_eq!(job.status, PublishJobStatus::Published);
        assert_eq!(job.provider_post_id.as_deref(), Some("yt_video_1"));
        assert_eq!(job.provider_post_url, None);
        let events = store
            .publish_job_events("job_1")
            .unwrap_or_else(|err| panic!("events: {err}"));
        assert_eq!(events[0].event_type, PublishJobEventType::StatusPolled);
    }

    #[test]
    fn poll_processing_job_keeps_processing_when_provider_is_processing() {
        let mut store = store_with_processing_job();
        let adapter = RecordingStatusAdapter::new(UploadStatusResult {
            provider_post_id: "yt_video_1".into(),
            provider_post_url: None,
            status: UploadProcessingStatus::Processing,
            normalized_error: None,
            raw_error_ref: None,
        });

        let job = UploadStatusService::poll_processing_job(
            &mut store,
            &adapter,
            PollUploadStatusInput {
                job_id: "job_1".into(),
                now: 2_500,
            },
        )
        .unwrap_or_else(|err| panic!("poll status: {err}"));

        assert_eq!(job.status, PublishJobStatus::Processing);
        assert_eq!(job.provider_post_id.as_deref(), Some("yt_video_1"));
        assert_eq!(job.provider_post_url, None);
        assert_eq!(job.updated_at, 2_500);
    }

    #[test]
    fn poll_processing_job_rejects_mismatched_provider_post_id() {
        let mut store = store_with_processing_job();
        let adapter = RecordingStatusAdapter::new(UploadStatusResult {
            provider_post_id: "yt_video_other".into(),
            provider_post_url: None,
            status: UploadProcessingStatus::Processing,
            normalized_error: None,
            raw_error_ref: None,
        });

        let error = match UploadStatusService::poll_processing_job(
            &mut store,
            &adapter,
            PollUploadStatusInput {
                job_id: "job_1".into(),
                now: 2_500,
            },
        ) {
            Ok(job) => panic!("expected provider post id mismatch, got {job:?}"),
            Err(error) => error,
        };

        assert_eq!(error, UploadStatusServiceError::ProviderPostIdMismatch);
        let job = store
            .publish_job("job_1")
            .unwrap_or_else(|err| panic!("job: {err}"));
        assert_eq!(job.status, PublishJobStatus::Processing);
        assert_eq!(job.provider_post_id.as_deref(), Some("yt_video_1"));
        assert_eq!(job.updated_at, 2_200);
        let events = store
            .publish_job_events("job_1")
            .unwrap_or_else(|err| panic!("events: {err}"));
        assert!(events.is_empty());
    }

    #[test]
    fn poll_processing_job_allows_final_provider_post_id_on_published_status() {
        let mut store = store_with_processing_job();
        let adapter = RecordingStatusAdapter::new(UploadStatusResult {
            provider_post_id: "ig_media_1".into(),
            provider_post_url: Some("https://www.instagram.com/reel/abc/".into()),
            status: UploadProcessingStatus::Published,
            normalized_error: None,
            raw_error_ref: None,
        });

        let job = UploadStatusService::poll_processing_job(
            &mut store,
            &adapter,
            PollUploadStatusInput {
                job_id: "job_1".into(),
                now: 2_500,
            },
        )
        .unwrap_or_else(|err| panic!("poll status: {err}"));

        assert_eq!(job.status, PublishJobStatus::Published);
        assert_eq!(job.provider_post_id.as_deref(), Some("ig_media_1"));
        assert_eq!(
            job.provider_post_url.as_deref(),
            Some("https://www.instagram.com/reel/abc/")
        );
    }

    #[test]
    fn poll_processing_job_fails_on_provider_failure() {
        let mut store = store_with_processing_job();
        let adapter = RecordingStatusAdapter::new(UploadStatusResult {
            provider_post_id: "yt_video_1".into(),
            provider_post_url: None,
            status: UploadProcessingStatus::Failed,
            normalized_error: Some("platform_processing_failed".into()),
            raw_error_ref: Some("youtube/status/yt_video_1".into()),
        });

        let job = UploadStatusService::poll_processing_job(
            &mut store,
            &adapter,
            PollUploadStatusInput {
                job_id: "job_1".into(),
                now: 2_500,
            },
        )
        .unwrap_or_else(|err| panic!("poll status: {err}"));

        assert_eq!(job.status, PublishJobStatus::Failed);
        assert_eq!(
            job.normalized_error.as_deref(),
            Some("platform_processing_failed")
        );
        assert_eq!(
            job.raw_error_ref.as_deref(),
            Some("youtube/status/yt_video_1")
        );
    }

    #[test]
    fn poll_processing_job_rejects_non_processing_jobs_before_provider_call() {
        let mut store = store_with_processing_job();
        let job = store
            .publish_job("job_1")
            .unwrap_or_else(|err| panic!("job: {err}"))
            .publish(
                "yt_video_1",
                "https://www.youtube.com/watch?v=yt_video_1",
                2_400,
            );
        store
            .save_publish_job(job)
            .unwrap_or_else(|err| panic!("save job: {err}"));
        let adapter = RecordingStatusAdapter::failing(UploadStatusAdapterError::NetworkOrServer {
            message: "should not call provider".into(),
        });

        assert_eq!(
            UploadStatusService::poll_processing_job(
                &mut store,
                &adapter,
                PollUploadStatusInput {
                    job_id: "job_1".into(),
                    now: 2_500,
                },
            ),
            Err(UploadStatusServiceError::JobNotProcessing)
        );
        assert_eq!(adapter.call_count(), 0);
    }

    #[test]
    fn poll_processing_job_requires_action_when_account_needs_reauth() {
        let mut store = store_with_processing_job();
        let mut account = connected_account();
        account.status = ConnectedAccountStatus::NeedsReauth;
        store
            .save_connected_account(account)
            .unwrap_or_else(|err| panic!("save account: {err}"));
        let adapter = RecordingStatusAdapter::new(UploadStatusResult {
            provider_post_id: "yt_video_1".into(),
            provider_post_url: None,
            status: UploadProcessingStatus::Published,
            normalized_error: None,
            raw_error_ref: None,
        });

        let job = UploadStatusService::poll_processing_job(
            &mut store,
            &adapter,
            PollUploadStatusInput {
                job_id: "job_1".into(),
                now: 2_500,
            },
        )
        .unwrap_or_else(|err| panic!("poll status: {err}"));

        assert_eq!(job.status, PublishJobStatus::RequiresAction);
        assert_eq!(
            job.requires_action_reason.as_deref(),
            Some("account_needs_reauth")
        );
        assert_eq!(adapter.call_count(), 0);
        let events = store
            .publish_job_events("job_1")
            .unwrap_or_else(|err| panic!("events: {err}"));
        assert_eq!(events[0].event_type, PublishJobEventType::RequiresAction);
        assert_eq!(events[0].metadata["reason"], "account_needs_reauth");
    }

    #[test]
    fn poll_processing_job_requires_action_when_account_is_ineligible() {
        let mut store = store_with_processing_job();
        let mut account = connected_account();
        account.eligibility = AccountEligibility::blocked("account_not_eligible");
        store
            .save_connected_account(account)
            .unwrap_or_else(|err| panic!("save account: {err}"));
        let adapter = RecordingStatusAdapter::new(UploadStatusResult {
            provider_post_id: "yt_video_1".into(),
            provider_post_url: None,
            status: UploadProcessingStatus::Published,
            normalized_error: None,
            raw_error_ref: None,
        });

        let job = UploadStatusService::poll_processing_job(
            &mut store,
            &adapter,
            PollUploadStatusInput {
                job_id: "job_1".into(),
                now: 2_500,
            },
        )
        .unwrap_or_else(|err| panic!("poll status: {err}"));

        assert_eq!(job.status, PublishJobStatus::RequiresAction);
        assert_eq!(
            job.requires_action_reason.as_deref(),
            Some("account_not_eligible")
        );
        assert_eq!(adapter.call_count(), 0);
    }

    #[test]
    fn poll_processing_job_requires_action_when_publish_capability_is_missing() {
        let mut store = store_with_processing_job();
        let mut account = connected_account();
        account.capabilities.upload_video = false;
        store
            .save_connected_account(account)
            .unwrap_or_else(|err| panic!("save account: {err}"));
        let adapter = RecordingStatusAdapter::new(UploadStatusResult {
            provider_post_id: "yt_video_1".into(),
            provider_post_url: None,
            status: UploadProcessingStatus::Published,
            normalized_error: None,
            raw_error_ref: None,
        });

        let job = UploadStatusService::poll_processing_job(
            &mut store,
            &adapter,
            PollUploadStatusInput {
                job_id: "job_1".into(),
                now: 2_500,
            },
        )
        .unwrap_or_else(|err| panic!("poll status: {err}"));

        assert_eq!(job.status, PublishJobStatus::RequiresAction);
        assert_eq!(
            job.requires_action_reason.as_deref(),
            Some("missing_publish_capability")
        );
        assert_eq!(adapter.call_count(), 0);
    }

    #[test]
    fn poll_status_request_and_event_do_not_include_raw_token_material() {
        let mut store = store_with_processing_job();
        let adapter = RecordingStatusAdapter::new(UploadStatusResult {
            provider_post_id: "yt_video_1".into(),
            provider_post_url: None,
            status: UploadProcessingStatus::Processing,
            normalized_error: None,
            raw_error_ref: None,
        });

        UploadStatusService::poll_processing_job(
            &mut store,
            &adapter,
            PollUploadStatusInput {
                job_id: "job_1".into(),
                now: 2_500,
            },
        )
        .unwrap_or_else(|err| panic!("poll status: {err}"));

        let requests_json = serde_json::to_string(&*adapter.requests.borrow())
            .unwrap_or_else(|err| panic!("serialize requests: {err}"));
        let events_json = serde_json::to_string(
            &store
                .publish_job_events("job_1")
                .unwrap_or_else(|err| panic!("events: {err}")),
        )
        .unwrap_or_else(|err| panic!("serialize events: {err}"));

        assert!(requests_json.contains("token_secret:acct_1"));
        assert!(!requests_json.contains("access-secret"));
        assert!(!requests_json.contains("refresh-secret"));
        assert!(!events_json.contains("access-secret"));
        assert!(!events_json.contains("refresh-secret"));
    }

    fn store_with_processing_job() -> InMemorySocialStore {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account())
            .unwrap_or_else(|err| panic!("save account: {err}"));
        store
            .save_token_secret(token_secret())
            .unwrap_or_else(|err| panic!("save token secret: {err}"));
        store
            .save_publish_job(processing_job())
            .unwrap_or_else(|err| panic!("save job: {err}"));
        store
    }

    fn connected_account() -> ConnectedAccount {
        ConnectedAccount {
            id: "acct_1".into(),
            owner: OwnerRef::User("user_1".into()),
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            display_name: "Montage Channel".into(),
            handle: Some("@montage".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            capabilities: ProviderCapabilities {
                upload_video: true,
                upload_thumbnail: true,
                public_posting: true,
                ..ProviderCapabilities::default()
            },
            eligibility: AccountEligibility::eligible(),
            last_verified_at: None,
            created_at: 1_000,
            updated_at: 1_000,
        }
    }

    fn token_secret() -> TokenSecret {
        TokenSecret::encrypt(
            "acct_1",
            "access-secret",
            Some("refresh-secret"),
            &TestKeyProvider::new("test-key-1", "local-key"),
            1_000,
        )
        .unwrap_or_else(|err| panic!("encrypt token secret: {err}"))
    }

    fn processing_job() -> PublishJob {
        PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            2_000,
            "user_1",
        )
        .schedule(2_000)
        .claim_for_upload(2_100)
        .processing("yt_video_1", 2_200)
    }
}
