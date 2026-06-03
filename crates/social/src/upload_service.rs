use crate::model::{
    ConnectedAccountStatus, PublishJob, PublishJobActorType, PublishJobEvent, PublishJobEventType,
    PublishJobStatus,
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
    /// Resolved visibility for this upload, derived upstream from the campaign
    /// target / account defaults. The worker no longer hard-codes `Private`.
    pub privacy: UploadPrivacy,
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
    /// The connected account is no longer usable for publishing — it was
    /// disconnected, made ineligible, or lacks the upload capability since the
    /// job was scheduled. Re-checked immediately before the provider call so a
    /// stale scheduled job can't publish from a since-revoked account.
    #[error("connected account is not eligible for upload: {0}")]
    AccountNotPublishable(String),
    /// The job was cancelled (by the user) while the provider upload was in
    /// flight. The provider result is discarded so the cancellation stands.
    #[error("publish job was cancelled during upload")]
    JobCancelledDuringUpload,
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
        // Re-check the account is still publishable right before we touch the
        // token / call the provider. A job scheduled while the account was
        // healthy may be claimed long after the user disconnected, revoked, or
        // lost eligibility for it — without this, a stale job would publish
        // from an account the user no longer controls.
        if account.status != ConnectedAccountStatus::Connected {
            return Err(UploadServiceError::AccountNotPublishable(format!(
                "status:{}",
                account_status_slug(&account.status)
            )));
        }
        if !account.eligibility.eligible {
            let reason = account
                .eligibility
                .reasons
                .first()
                .cloned()
                .unwrap_or_else(|| "ineligible".into());
            return Err(UploadServiceError::AccountNotPublishable(format!(
                "eligibility:{reason}"
            )));
        }
        if !account.capabilities.upload_video {
            return Err(UploadServiceError::AccountNotPublishable(
                "capability:upload_video".into(),
            ));
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
            privacy: input.privacy,
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
        let upload_in_progress = job.start_provider_processing(input.now);
        store.save_publish_job(upload_in_progress.clone())?;

        let result = adapter.upload(&request);
        // The provider call can take a while (multi-GB uploads). A user may
        // have cancelled the job in the meantime; the cancel writes
        // `Cancelled` to the store while we hold an older clone. Re-read the
        // persisted status before saving a terminal success so a slow upload
        // can't resurrect a cancelled job as `Published`.
        if result.is_ok() {
            let current = store.publish_job(&upload_in_progress.id)?;
            if current.status == PublishJobStatus::Cancelled {
                append_event(
                    store,
                    &upload_in_progress.id,
                    PublishJobEventType::Cancelled,
                    PublishJobActorType::Worker,
                    "provider upload completed after cancellation; result discarded",
                    serde_json::json!({"provider": upload_in_progress.provider.as_str()}),
                    input.now,
                )?;
                return Err(UploadServiceError::JobCancelledDuringUpload);
            }
        }
        let next = match result {
            Ok(result) => {
                let status = if result.processing {
                    "processing"
                } else {
                    "published"
                };
                let next = if result.processing {
                    upload_in_progress.processing(result.provider_post_id.clone(), input.now)
                } else {
                    upload_in_progress.publish(
                        result.provider_post_id.clone(),
                        result.provider_post_url.clone(),
                        input.now,
                    )
                };
                store.save_publish_job(next.clone())?;
                append_event(
                    store,
                    &next.id,
                    PublishJobEventType::Uploaded,
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
                let next = upload_in_progress.requires_action(reason.clone(), input.now);
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
                let next = upload_in_progress.fail(
                    reason.clone(),
                    "provider_error_ref_unavailable",
                    input.now,
                );
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
                let next =
                    upload_in_progress.fail("network_or_server_error", message.clone(), input.now);
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

fn account_status_slug(status: &ConnectedAccountStatus) -> &'static str {
    match status {
        ConnectedAccountStatus::Connected => "connected",
        ConnectedAccountStatus::NeedsReauth => "needs_reauth",
        ConnectedAccountStatus::MissingScope => "missing_scope",
        ConnectedAccountStatus::Ineligible => "ineligible",
        ConnectedAccountStatus::Disabled => "disabled",
        ConnectedAccountStatus::Revoked => "revoked",
    }
}

fn event_type_slug(event_type: &PublishJobEventType) -> &'static str {
    match event_type {
        PublishJobEventType::TargetBound => "target_bound",
        PublishJobEventType::Validated => "validated",
        PublishJobEventType::Scheduled => "scheduled",
        PublishJobEventType::Claimed => "claimed",
        PublishJobEventType::Uploaded => "uploaded",
        PublishJobEventType::StatusPolled => "status_polled",
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
        AccountEligibility, AccountKind, AccountPublishDefaults, CampaignVariantTarget,
        ConnectedAccount, ConnectedAccountStatus, OwnerRef, Provider, ProviderCapabilities,
        PublishJob, PublishJobActorType, PublishJobEvent, PublishJobEventType, PublishJobStatus,
        WorkspaceMemberRole,
    };
    use crate::oauth::{OAuthConnection, OAuthConnectionStatus};
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
                privacy: UploadPrivacy::Private,
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
        assert_eq!(events[1].event_type, PublishJobEventType::Uploaded);
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
                privacy: UploadPrivacy::Private,
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
                    privacy: UploadPrivacy::Private,
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
                privacy: UploadPrivacy::Private,
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

    #[test]
    fn execute_upload_persists_processing_before_provider_success_can_fail() {
        let mut store = FaultyStore::new(store_with_claimed_job(), 2);
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
                    privacy: UploadPrivacy::Private,
                    now: 2_000,
                },
            ),
            Err(UploadServiceError::Store(SocialStoreError::Storage(
                "publish save failed".into()
            )))
        );
        assert_eq!(adapter.call_count(), 1);
        let persisted = store
            .inner
            .publish_job("job_1")
            .unwrap_or_else(|err| panic!("load job: {err}"));
        assert_eq!(persisted.status, PublishJobStatus::Processing);
        assert_eq!(persisted.provider_post_id, None);
    }

    #[test]
    fn execute_upload_rejects_disconnected_account_before_provider_call() {
        for status in [
            ConnectedAccountStatus::Revoked,
            ConnectedAccountStatus::Disabled,
            ConnectedAccountStatus::NeedsReauth,
            ConnectedAccountStatus::Ineligible,
        ] {
            let mut store = store_with_claimed_job();
            let mut account = connected_account();
            account.status = status.clone();
            store
                .save_connected_account(account)
                .unwrap_or_else(|err| panic!("save account: {err}"));
            let adapter = RecordingUploadAdapter::published();

            let result = UploadService::execute_claimed_job(&mut store, &adapter, execute_input());

            assert!(
                matches!(result, Err(UploadServiceError::AccountNotPublishable(_))),
                "status {status:?} should block upload, got {result:?}"
            );
            assert_eq!(adapter.call_count(), 0, "adapter must not be called");
            // Nothing should have been persisted as in-flight.
            assert_eq!(
                store.publish_job("job_1").map(|job| job.status),
                Ok(PublishJobStatus::Uploading)
            );
            assert_eq!(
                store
                    .publish_job_events("job_1")
                    .unwrap_or_else(|err| panic!("load events: {err}")),
                Vec::new()
            );
        }
    }

    #[test]
    fn execute_upload_rejects_ineligible_account_before_provider_call() {
        let mut store = store_with_claimed_job();
        let mut account = connected_account();
        account.eligibility = AccountEligibility::blocked("monetization_review");
        store
            .save_connected_account(account)
            .unwrap_or_else(|err| panic!("save account: {err}"));
        let adapter = RecordingUploadAdapter::published();

        let result = UploadService::execute_claimed_job(&mut store, &adapter, execute_input());

        assert_eq!(
            result,
            Err(UploadServiceError::AccountNotPublishable(
                "eligibility:monetization_review".into()
            ))
        );
        assert_eq!(adapter.call_count(), 0);
    }

    #[test]
    fn execute_upload_rejects_account_without_upload_capability() {
        let mut store = store_with_claimed_job();
        let mut account = connected_account();
        account.capabilities.upload_video = false;
        store
            .save_connected_account(account)
            .unwrap_or_else(|err| panic!("save account: {err}"));
        let adapter = RecordingUploadAdapter::published();

        let result = UploadService::execute_claimed_job(&mut store, &adapter, execute_input());

        assert_eq!(
            result,
            Err(UploadServiceError::AccountNotPublishable(
                "capability:upload_video".into()
            ))
        );
        assert_eq!(adapter.call_count(), 0);
    }

    #[test]
    fn execute_upload_propagates_requested_privacy_to_adapter() {
        let mut store = store_with_claimed_job();
        let adapter = RecordingUploadAdapter::published();

        UploadService::execute_claimed_job(
            &mut store,
            &adapter,
            ExecuteUploadInput {
                privacy: UploadPrivacy::Public,
                ..execute_input()
            },
        )
        .unwrap_or_else(|err| panic!("execute upload: {err}"));

        let request = adapter
            .requests
            .borrow()
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("expected adapter request"));
        assert_eq!(request.privacy, UploadPrivacy::Public);
    }

    #[test]
    fn execute_upload_discards_result_when_job_cancelled_mid_upload() {
        // The store flips the job to Cancelled the moment the adapter is asked
        // to upload — modeling a user cancel landing while the provider call is
        // in flight. The worker must not overwrite that with Published.
        let mut store = CancelMidUploadStore::new(store_with_claimed_job());
        let adapter = RecordingUploadAdapter::published();

        let result = UploadService::execute_claimed_job(&mut store, &adapter, execute_input());

        assert_eq!(result, Err(UploadServiceError::JobCancelledDuringUpload));
        assert_eq!(adapter.call_count(), 1, "adapter ran before cancel landed");
        assert!(
            !store.published_after_cancel.get(),
            "worker must not persist Published/Processing after a cancel"
        );
        // A discard event was recorded for the audit trail.
        let events = store
            .inner
            .publish_job_events("job_1")
            .unwrap_or_else(|err| panic!("load events: {err}"));
        assert!(
            events
                .iter()
                .any(|event| event.event_type == PublishJobEventType::Cancelled
                    && event.actor_type == PublishJobActorType::Worker)
        );
    }

    fn execute_input() -> ExecuteUploadInput {
        ExecuteUploadInput {
            job_id: "job_1".into(),
            title: "Launch clip".into(),
            description: None,
            tags: Vec::new(),
            thumbnail_ref: None,
            privacy: UploadPrivacy::Private,
            now: 2_000,
        }
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

    struct FaultyStore {
        inner: InMemorySocialStore,
        fail_on_publish_save_call: usize,
        publish_save_calls: usize,
    }

    impl FaultyStore {
        fn new(inner: InMemorySocialStore, fail_on_publish_save_call: usize) -> Self {
            Self {
                inner,
                fail_on_publish_save_call,
                publish_save_calls: 0,
            }
        }
    }

    impl SocialStore for FaultyStore {
        fn save_oauth_connection(
            &mut self,
            connection: OAuthConnection,
        ) -> Result<(), SocialStoreError> {
            self.inner.save_oauth_connection(connection)
        }

        fn oauth_connection(&self, id: &str) -> Result<OAuthConnection, SocialStoreError> {
            self.inner.oauth_connection(id)
        }

        fn update_oauth_status(
            &mut self,
            id: &str,
            status: OAuthConnectionStatus,
        ) -> Result<OAuthConnection, SocialStoreError> {
            self.inner.update_oauth_status(id, status)
        }

        fn save_connected_account(
            &mut self,
            account: ConnectedAccount,
        ) -> Result<(), SocialStoreError> {
            self.inner.save_connected_account(account)
        }

        fn connected_account(&self, id: &str) -> Result<ConnectedAccount, SocialStoreError> {
            self.inner.connected_account(id)
        }

        fn connected_accounts_for_owner(
            &self,
            owner: &OwnerRef,
        ) -> Result<Vec<ConnectedAccount>, SocialStoreError> {
            self.inner.connected_accounts_for_owner(owner)
        }

        fn disable_connected_account(
            &mut self,
            id: &str,
            owner: &OwnerRef,
            now: i64,
        ) -> Result<ConnectedAccount, SocialStoreError> {
            self.inner.disable_connected_account(id, owner, now)
        }

        fn save_token_secret(&mut self, secret: TokenSecret) -> Result<(), SocialStoreError> {
            self.inner.save_token_secret(secret)
        }

        fn token_secret_for_account(
            &self,
            account_id: &str,
        ) -> Result<TokenSecret, SocialStoreError> {
            self.inner.token_secret_for_account(account_id)
        }

        fn save_campaign_variant_target(
            &mut self,
            target: CampaignVariantTarget,
        ) -> Result<(), SocialStoreError> {
            self.inner.save_campaign_variant_target(target)
        }

        fn campaign_variant_target(
            &self,
            id: &str,
        ) -> Result<CampaignVariantTarget, SocialStoreError> {
            self.inner.campaign_variant_target(id)
        }

        fn save_publish_job(&mut self, job: PublishJob) -> Result<(), SocialStoreError> {
            self.publish_save_calls += 1;
            if self.publish_save_calls == self.fail_on_publish_save_call {
                return Err(SocialStoreError::Storage("publish save failed".into()));
            }
            self.inner.save_publish_job(job)
        }

        fn publish_job(&self, id: &str) -> Result<PublishJob, SocialStoreError> {
            self.inner.publish_job(id)
        }

        fn publish_jobs_for_account(
            &self,
            connected_account_id: &str,
        ) -> Result<Vec<PublishJob>, SocialStoreError> {
            self.inner.publish_jobs_for_account(connected_account_id)
        }

        fn claim_due_publish_jobs(
            &mut self,
            now: i64,
            limit: usize,
        ) -> Result<Vec<PublishJob>, SocialStoreError> {
            self.inner.claim_due_publish_jobs(now, limit)
        }

        fn append_publish_job_event(
            &mut self,
            event: PublishJobEvent,
        ) -> Result<(), SocialStoreError> {
            self.inner.append_publish_job_event(event)
        }

        fn publish_job_events(
            &self,
            publish_job_id: &str,
        ) -> Result<Vec<PublishJobEvent>, SocialStoreError> {
            self.inner.publish_job_events(publish_job_id)
        }

        fn save_account_publish_defaults(
            &mut self,
            defaults: AccountPublishDefaults,
        ) -> Result<(), SocialStoreError> {
            self.inner.save_account_publish_defaults(defaults)
        }

        fn account_publish_defaults(
            &self,
            connected_account_id: &str,
        ) -> Result<AccountPublishDefaults, SocialStoreError> {
            self.inner.account_publish_defaults(connected_account_id)
        }

        fn save_workspace_member_role(
            &mut self,
            role: WorkspaceMemberRole,
        ) -> Result<(), SocialStoreError> {
            self.inner.save_workspace_member_role(role)
        }

        fn workspace_member_roles(
            &self,
            workspace_id: &str,
        ) -> Result<Vec<WorkspaceMemberRole>, SocialStoreError> {
            self.inner.workspace_member_roles(workspace_id)
        }

        fn token_secrets_due_refresh(
            &self,
            deadline: i64,
        ) -> Result<Vec<TokenSecret>, SocialStoreError> {
            self.inner.token_secrets_due_refresh(deadline)
        }
    }

    /// Store that simulates a user cancellation landing *during* the provider
    /// upload: the first `publish_job` read (the worker's initial load) returns
    /// the real claimed job, but the next read (the post-upload re-check)
    /// reports the job as `Cancelled` — and persists that into `inner` so the
    /// assertion can confirm the cancellation stood.
    struct CancelMidUploadStore {
        inner: InMemorySocialStore,
        reads: std::cell::Cell<usize>,
        /// Set if the worker ever tried to persist a `Published` status after
        /// the cancellation was observed — the bug this test guards against.
        published_after_cancel: std::cell::Cell<bool>,
    }

    impl CancelMidUploadStore {
        fn new(inner: InMemorySocialStore) -> Self {
            Self {
                inner,
                reads: std::cell::Cell::new(0),
                published_after_cancel: std::cell::Cell::new(false),
            }
        }
    }

    impl SocialStore for CancelMidUploadStore {
        fn save_oauth_connection(
            &mut self,
            connection: OAuthConnection,
        ) -> Result<(), SocialStoreError> {
            self.inner.save_oauth_connection(connection)
        }

        fn oauth_connection(&self, id: &str) -> Result<OAuthConnection, SocialStoreError> {
            self.inner.oauth_connection(id)
        }

        fn update_oauth_status(
            &mut self,
            id: &str,
            status: OAuthConnectionStatus,
        ) -> Result<OAuthConnection, SocialStoreError> {
            self.inner.update_oauth_status(id, status)
        }

        fn save_connected_account(
            &mut self,
            account: ConnectedAccount,
        ) -> Result<(), SocialStoreError> {
            self.inner.save_connected_account(account)
        }

        fn connected_account(&self, id: &str) -> Result<ConnectedAccount, SocialStoreError> {
            self.inner.connected_account(id)
        }

        fn connected_accounts_for_owner(
            &self,
            owner: &OwnerRef,
        ) -> Result<Vec<ConnectedAccount>, SocialStoreError> {
            self.inner.connected_accounts_for_owner(owner)
        }

        fn disable_connected_account(
            &mut self,
            id: &str,
            owner: &OwnerRef,
            now: i64,
        ) -> Result<ConnectedAccount, SocialStoreError> {
            self.inner.disable_connected_account(id, owner, now)
        }

        fn save_token_secret(&mut self, secret: TokenSecret) -> Result<(), SocialStoreError> {
            self.inner.save_token_secret(secret)
        }

        fn token_secret_for_account(
            &self,
            account_id: &str,
        ) -> Result<TokenSecret, SocialStoreError> {
            self.inner.token_secret_for_account(account_id)
        }

        fn save_campaign_variant_target(
            &mut self,
            target: CampaignVariantTarget,
        ) -> Result<(), SocialStoreError> {
            self.inner.save_campaign_variant_target(target)
        }

        fn campaign_variant_target(
            &self,
            id: &str,
        ) -> Result<CampaignVariantTarget, SocialStoreError> {
            self.inner.campaign_variant_target(id)
        }

        fn save_publish_job(&mut self, job: PublishJob) -> Result<(), SocialStoreError> {
            // The worker should never persist a terminal success once the
            // cancellation has been observed (reads >= 2).
            if self.reads.get() >= 2
                && matches!(
                    job.status,
                    PublishJobStatus::Published | PublishJobStatus::Processing
                )
            {
                self.published_after_cancel.set(true);
            }
            self.inner.save_publish_job(job)
        }

        fn publish_job(&self, id: &str) -> Result<PublishJob, SocialStoreError> {
            let count = self.reads.get() + 1;
            self.reads.set(count);
            let job = self.inner.publish_job(id)?;
            if count >= 2 {
                // Second read = the worker's post-upload re-check. Report the
                // job as cancelled, modeling a concurrent user cancel.
                return Ok(job.cancel(1_950));
            }
            Ok(job)
        }

        fn publish_jobs_for_account(
            &self,
            connected_account_id: &str,
        ) -> Result<Vec<PublishJob>, SocialStoreError> {
            self.inner.publish_jobs_for_account(connected_account_id)
        }

        fn claim_due_publish_jobs(
            &mut self,
            now: i64,
            limit: usize,
        ) -> Result<Vec<PublishJob>, SocialStoreError> {
            self.inner.claim_due_publish_jobs(now, limit)
        }

        fn append_publish_job_event(
            &mut self,
            event: PublishJobEvent,
        ) -> Result<(), SocialStoreError> {
            self.inner.append_publish_job_event(event)
        }

        fn publish_job_events(
            &self,
            publish_job_id: &str,
        ) -> Result<Vec<PublishJobEvent>, SocialStoreError> {
            self.inner.publish_job_events(publish_job_id)
        }

        fn save_account_publish_defaults(
            &mut self,
            defaults: AccountPublishDefaults,
        ) -> Result<(), SocialStoreError> {
            self.inner.save_account_publish_defaults(defaults)
        }

        fn account_publish_defaults(
            &self,
            connected_account_id: &str,
        ) -> Result<AccountPublishDefaults, SocialStoreError> {
            self.inner.account_publish_defaults(connected_account_id)
        }

        fn save_workspace_member_role(
            &mut self,
            role: WorkspaceMemberRole,
        ) -> Result<(), SocialStoreError> {
            self.inner.save_workspace_member_role(role)
        }

        fn workspace_member_roles(
            &self,
            workspace_id: &str,
        ) -> Result<Vec<WorkspaceMemberRole>, SocialStoreError> {
            self.inner.workspace_member_roles(workspace_id)
        }

        fn token_secrets_due_refresh(
            &self,
            deadline: i64,
        ) -> Result<Vec<TokenSecret>, SocialStoreError> {
            self.inner.token_secrets_due_refresh(deadline)
        }
    }
}
