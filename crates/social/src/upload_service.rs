use crate::model::{
    PublishJob, PublishJobActorType, PublishJobEvent, PublishJobEventType, PublishJobStatus,
};
use crate::store::{SocialStore, SocialStoreError};
use crate::upload_adapter::{UploadAdapter, UploadAdapterError, UploadPrivacy, UploadRequest};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecuteUploadInput {
    pub job_id: String,
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub thumbnail_ref: Option<String>,
    pub now: i64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UploadServiceError {
    #[error(transparent)]
    Store(#[from] SocialStoreError),
    #[error("publish job is not claimed for upload")]
    JobNotUploading,
    #[error("upload provider does not match publish job")]
    ProviderMismatch,
}

pub struct UploadService;

impl UploadService {
    pub fn execute_claimed_job(
        store: &mut impl SocialStore,
        adapter: &impl UploadAdapter,
        input: ExecuteUploadInput,
    ) -> Result<PublishJob, UploadServiceError> {
        let job = store.publish_job(&input.job_id)?;
        if job.status != PublishJobStatus::Uploading {
            return Err(UploadServiceError::JobNotUploading);
        }
        if adapter.provider() != job.provider {
            return Err(UploadServiceError::ProviderMismatch);
        }

        let account = store.connected_account(&job.connected_account_id)?;
        if account.provider != job.provider {
            return Err(UploadServiceError::ProviderMismatch);
        }
        let _token_secret = store.token_secret_for_account(&account.id)?;
        let request = UploadRequest {
            job_id: job.id.clone(),
            provider: job.provider.clone(),
            connected_account_id: account.id.clone(),
            artifact_ref: job.artifact_ref.clone(),
            title: input.title,
            description: input.description,
            tags: input.tags,
            thumbnail_ref: input.thumbnail_ref,
            privacy: UploadPrivacy::Private,
            scheduled_for: Some(job.scheduled_for),
            access_token_ref: format!("token_secret:{}", account.id),
        };

        append_event(
            store,
            &job.id,
            PublishJobEventType::Claimed,
            PublishJobActorType::Worker,
            "upload job claimed by worker",
            serde_json::json!({"provider": job.provider.as_str()}),
            input.now,
        )?;

        let result = adapter.upload(&request);
        let next = match result {
            Ok(result) => {
                let status = if result.processing {
                    "processing"
                } else {
                    "published"
                };
                let next = if result.processing {
                    job.processing(result.provider_post_id.clone(), input.now)
                } else {
                    job.publish(
                        result.provider_post_id.clone(),
                        result.provider_post_url.clone(),
                        input.now,
                    )
                };
                store.save_publish_job(next.clone())?;
                append_event(
                    store,
                    &next.id,
                    PublishJobEventType::Claimed,
                    PublishJobActorType::Provider,
                    "provider upload completed",
                    serde_json::json!({
                        "provider": next.provider.as_str(),
                        "provider_post_id": result.provider_post_id,
                        "provider_post_url": result.provider_post_url,
                        "status": status,
                    }),
                    input.now,
                )?;
                next
            }
            Err(
                error @ (UploadAdapterError::RequiresAction { .. }
                | UploadAdapterError::MissingUploadToken),
            ) => {
                let reason = match error {
                    UploadAdapterError::RequiresAction { reason } => reason,
                    UploadAdapterError::MissingUploadToken => "missing_upload_token".into(),
                    _ => unreachable!("matched requires action upload errors"),
                };
                let next = job.requires_action(reason.clone(), input.now);
                store.save_publish_job(next.clone())?;
                append_event(
                    store,
                    &next.id,
                    PublishJobEventType::RequiresAction,
                    PublishJobActorType::Provider,
                    "provider upload requires action",
                    serde_json::json!({
                        "provider": next.provider.as_str(),
                        "reason": reason,
                    }),
                    input.now,
                )?;
                next
            }
            Err(UploadAdapterError::MediaConstraintFailed { reason }) => {
                let next = job.fail(reason.clone(), "provider_error_ref_unavailable", input.now);
                store.save_publish_job(next.clone())?;
                append_event(
                    store,
                    &next.id,
                    PublishJobEventType::Failed,
                    PublishJobActorType::Provider,
                    "provider upload failed media constraints",
                    serde_json::json!({
                        "provider": next.provider.as_str(),
                        "reason": reason,
                    }),
                    input.now,
                )?;
                next
            }
            Err(UploadAdapterError::NetworkOrServer { message }) => {
                let next = job.fail("network_or_server_error", message.clone(), input.now);
                store.save_publish_job(next.clone())?;
                append_event(
                    store,
                    &next.id,
                    PublishJobEventType::Failed,
                    PublishJobActorType::Provider,
                    "provider upload failed",
                    serde_json::json!({
                        "provider": next.provider.as_str(),
                        "message": message,
                    }),
                    input.now,
                )?;
                next
            }
            Err(UploadAdapterError::ProviderMismatch) => {
                return Err(UploadServiceError::ProviderMismatch);
            }
        };

        Ok(next)
    }
}

fn append_event(
    store: &mut impl SocialStore,
    job_id: &str,
    event_type: PublishJobEventType,
    actor_type: PublishJobActorType,
    message: &str,
    metadata: serde_json::Value,
    now: i64,
) -> Result<(), UploadServiceError> {
    let event_id = next_event_id(store, job_id, &event_type)?;
    store.append_publish_job_event(PublishJobEvent::new(
        event_id, job_id, event_type, actor_type, message, metadata, now,
    ))?;
    Ok(())
}

fn next_event_id(
    store: &impl SocialStore,
    job_id: &str,
    event_type: &PublishJobEventType,
) -> Result<String, UploadServiceError> {
    let sequence = store.publish_job_events(job_id)?.len() + 1;
    Ok(format!(
        "event_{}_{}_{}",
        job_id,
        event_type_slug(event_type),
        sequence
    ))
}

fn event_type_slug(event_type: &PublishJobEventType) -> &'static str {
    match event_type {
        PublishJobEventType::TargetBound => "target_bound",
        PublishJobEventType::Validated => "validated",
        PublishJobEventType::Scheduled => "scheduled",
        PublishJobEventType::Claimed => "claimed",
        PublishJobEventType::Cancelled => "cancelled",
        PublishJobEventType::RetryQueued => "retry_queued",
        PublishJobEventType::RequiresAction => "requires_action",
        PublishJobEventType::Failed => "failed",
    }
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
    use crate::upload_adapter::{UploadAdapter, UploadAdapterError, UploadRequest, UploadResult};
    use std::cell::RefCell;

    #[derive(Debug)]
    struct RecordingUploadAdapter {
        provider: Provider,
        result: Result<UploadResult, UploadAdapterError>,
        requests: RefCell<Vec<UploadRequest>>,
    }

    impl RecordingUploadAdapter {
        fn published() -> Self {
            Self {
                provider: Provider::YouTube,
                result: Ok(UploadResult {
                    provider_post_id: "yt_video_1".into(),
                    provider_post_url: "https://www.youtube.com/watch?v=yt_video_1".into(),
                    processing: false,
                }),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn failing(error: UploadAdapterError) -> Self {
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

    impl UploadAdapter for RecordingUploadAdapter {
        fn provider(&self) -> Provider {
            self.provider.clone()
        }

        fn upload(&self, request: &UploadRequest) -> Result<UploadResult, UploadAdapterError> {
            self.requests.borrow_mut().push(request.clone());
            self.result.clone()
        }
    }

    #[test]
    fn execute_upload_publishes_job_and_appends_provider_event() {
        let mut store = store_with_claimed_job();
        let adapter = RecordingUploadAdapter::published();

        let job = UploadService::execute_claimed_job(
            &mut store,
            &adapter,
            ExecuteUploadInput {
                job_id: "job_1".into(),
                title: "Launch clip".into(),
                description: Some("Description".into()),
                tags: vec!["awidat".into()],
                thumbnail_ref: Some("render://thumb_1".into()),
                now: 2_000,
            },
        )
        .unwrap_or_else(|err| panic!("execute upload: {err}"));

        assert_eq!(job.status, PublishJobStatus::Published);
        assert_eq!(job.provider_post_id.as_deref(), Some("yt_video_1"));
        assert_eq!(
            job.provider_post_url.as_deref(),
            Some("https://www.youtube.com/watch?v=yt_video_1")
        );
        assert_eq!(store.publish_job("job_1"), Ok(job));

        let request = adapter
            .requests
            .borrow()
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("expected adapter request"));
        assert_eq!(request.job_id, "job_1");
        assert_eq!(request.provider, Provider::YouTube);
        assert_eq!(request.connected_account_id, "acct_1");
        assert_eq!(request.artifact_ref, "render://artifact_1");
        assert_eq!(request.title, "Launch clip");
        assert_eq!(request.description.as_deref(), Some("Description"));
        assert_eq!(request.tags, vec!["awidat"]);
        assert_eq!(request.thumbnail_ref.as_deref(), Some("render://thumb_1"));
        assert_eq!(request.scheduled_for, Some(1_800));
        assert_eq!(request.access_token_ref, "token_secret:acct_1");

        let events = store
            .publish_job_events("job_1")
            .unwrap_or_else(|err| panic!("load events: {err}"));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, PublishJobEventType::Claimed);
        assert_eq!(events[0].actor_type, PublishJobActorType::Worker);
        assert_eq!(events[1].actor_type, PublishJobActorType::Provider);
        assert_eq!(events[1].metadata["provider_post_id"], "yt_video_1");
        assert_eq!(
            events[1].metadata["provider_post_url"],
            "https://www.youtube.com/watch?v=yt_video_1"
        );
        assert_eq!(events[1].metadata["status"], "published");
        assert_ne!(events[0].id, events[1].id);
    }

    #[test]
    fn execute_upload_requires_uploading_status_and_maps_requires_action() {
        let mut store = store_with_claimed_job();
        let adapter = RecordingUploadAdapter::failing(UploadAdapterError::RequiresAction {
            reason: "missing_scope".into(),
        });

        let job = UploadService::execute_claimed_job(
            &mut store,
            &adapter,
            ExecuteUploadInput {
                job_id: "job_1".into(),
                title: "Launch clip".into(),
                description: None,
                tags: Vec::new(),
                thumbnail_ref: None,
                now: 2_000,
            },
        )
        .unwrap_or_else(|err| panic!("execute upload: {err}"));

        assert_eq!(job.status, PublishJobStatus::RequiresAction);
        assert_eq!(job.requires_action_reason.as_deref(), Some("missing_scope"));
        assert_eq!(store.publish_job("job_1"), Ok(job));

        let events = store
            .publish_job_events("job_1")
            .unwrap_or_else(|err| panic!("load events: {err}"));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, PublishJobEventType::Claimed);
        assert_eq!(events[1].event_type, PublishJobEventType::RequiresAction);
        assert_eq!(events[1].actor_type, PublishJobActorType::Provider);
        assert_eq!(events[1].metadata["reason"], "missing_scope");
    }

    #[test]
    fn execute_upload_rejects_wrong_state_before_provider_call() {
        let mut store = store_with_claimed_job();
        let scheduled = store
            .publish_job("job_1")
            .unwrap_or_else(|err| panic!("load job: {err}"))
            .schedule(1_900);
        store
            .save_publish_job(scheduled)
            .unwrap_or_else(|err| panic!("save job: {err}"));
        let adapter = RecordingUploadAdapter::published();

        assert_eq!(
            UploadService::execute_claimed_job(
                &mut store,
                &adapter,
                ExecuteUploadInput {
                    job_id: "job_1".into(),
                    title: "Launch clip".into(),
                    description: None,
                    tags: Vec::new(),
                    thumbnail_ref: None,
                    now: 2_000,
                },
            ),
            Err(UploadServiceError::JobNotUploading)
        );
        assert_eq!(adapter.call_count(), 0);
        assert_eq!(
            store
                .publish_job_events("job_1")
                .unwrap_or_else(|err| panic!("load events: {err}")),
            Vec::new()
        );
    }

    #[test]
    fn execute_upload_request_and_event_metadata_do_not_include_raw_token_material() {
        let mut store = store_with_claimed_job();
        let adapter = RecordingUploadAdapter::failing(UploadAdapterError::MissingUploadToken);

        UploadService::execute_claimed_job(
            &mut store,
            &adapter,
            ExecuteUploadInput {
                job_id: "job_1".into(),
                title: "Launch clip".into(),
                description: None,
                tags: Vec::new(),
                thumbnail_ref: None,
                now: 2_000,
            },
        )
        .unwrap_or_else(|err| panic!("execute upload: {err}"));

        let request_json = serde_json::to_string(
            &adapter
                .requests
                .borrow()
                .first()
                .cloned()
                .unwrap_or_else(|| panic!("expected adapter request")),
        )
        .unwrap_or_else(|err| panic!("serialize request: {err}"));
        let events_json = serde_json::to_string(
            &store
                .publish_job_events("job_1")
                .unwrap_or_else(|err| panic!("load events: {err}")),
        )
        .unwrap_or_else(|err| panic!("serialize events: {err}"));

        assert!(request_json.contains("token_secret:acct_1"));
        assert!(!request_json.contains("access-secret"));
        assert!(!request_json.contains("refresh-secret"));
        assert!(!request_json.contains("access_token"));
        assert!(!request_json.contains("refresh_token"));
        assert!(!events_json.contains("access-secret"));
        assert!(!events_json.contains("refresh-secret"));
        assert!(!events_json.contains("access_token"));
        assert!(!events_json.contains("refresh_token"));
    }

    fn store_with_claimed_job() -> InMemorySocialStore {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account())
            .unwrap_or_else(|err| panic!("save connected account: {err}"));
        store
            .save_token_secret(token_secret())
            .unwrap_or_else(|err| panic!("save token secret: {err}"));
        store
            .save_publish_job(publish_job().claim_for_upload(1_900))
            .unwrap_or_else(|err| panic!("save publish job: {err}"));
        store
    }

    fn connected_account() -> ConnectedAccount {
        ConnectedAccount {
            id: "acct_1".into(),
            owner: OwnerRef::User("user_1".into()),
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            display_name: "Awidat Channel".into(),
            handle: Some("@awidat".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["youtube.upload".into()],
            capabilities: ProviderCapabilities {
                upload_video: true,
                upload_thumbnail: true,
                public_posting: true,
                ..ProviderCapabilities::default()
            },
            eligibility: AccountEligibility::eligible(),
            last_verified_at: None,
            created_at: 100,
            updated_at: 100,
        }
    }

    fn token_secret() -> TokenSecret {
        TokenSecret::encrypt(
            "acct_1",
            "access-secret",
            Some("refresh-secret"),
            &TestKeyProvider::new("test-key-1", "local-key"),
            100,
        )
        .unwrap_or_else(|err| panic!("encrypt token secret: {err}"))
    }

    fn publish_job() -> PublishJob {
        PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            1_800,
            "user_1",
        )
    }
}
