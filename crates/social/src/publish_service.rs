use crate::model::{
    CampaignVariantTarget, ConnectedAccountStatus, OwnerRef, PublishJob, PublishJobActorType,
    PublishJobEvent, PublishJobEventType, PublishJobStatus, ValidationState,
};
use crate::store::{SocialStore, SocialStoreError};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindTargetInput {
    pub id: String,
    pub campaign_id: String,
    pub variant_id: String,
    pub connected_account_id: String,
    pub platform_fields: serde_json::Value,
    pub scheduled_for: i64,
    pub now: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleTargetInput {
    pub job_id: String,
    pub target_id: String,
    pub artifact_ref: String,
    pub created_by: String,
    pub now: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetValidationReport {
    pub state: ValidationState,
    pub reasons: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PublishServiceError {
    #[error(transparent)]
    Store(#[from] SocialStoreError),
    #[error("connected account belongs to a different owner")]
    OwnerMismatch,
    #[error("target provider does not match connected account")]
    ProviderMismatch,
    #[error("target is not valid for scheduling")]
    TargetNotValid,
    #[error("publish job cannot be cancelled from this state")]
    JobNotCancellable,
    #[error("publish job cannot be retried from this state")]
    JobNotRetryable,
}

pub struct PublishService;

impl PublishService {
    pub fn bind_target(
        store: &mut impl SocialStore,
        owner: &OwnerRef,
        input: BindTargetInput,
    ) -> Result<CampaignVariantTarget, PublishServiceError> {
        let account = store.connected_account(&input.connected_account_id)?;
        if account.owner != *owner {
            return Err(PublishServiceError::OwnerMismatch);
        }

        let target = CampaignVariantTarget::new(
            input.id,
            input.campaign_id,
            input.variant_id,
            account.id,
            account.provider,
            input.platform_fields,
            input.scheduled_for,
            input.now,
        );
        store.save_campaign_variant_target(target.clone())?;
        Ok(target)
    }

    pub fn validate_target(
        store: &mut impl SocialStore,
        owner: &OwnerRef,
        target_id: &str,
        now: i64,
    ) -> Result<TargetValidationReport, PublishServiceError> {
        let target = store.campaign_variant_target(target_id)?;
        let account = store.connected_account(&target.connected_account_id)?;
        if account.owner != *owner {
            return Err(PublishServiceError::OwnerMismatch);
        }
        if account.provider != target.provider {
            return Err(PublishServiceError::ProviderMismatch);
        }

        let (state, reasons) = if account.status != ConnectedAccountStatus::Connected {
            (
                ValidationState::RequiresAction,
                vec!["account_not_connected".to_string()],
            )
        } else if !account.eligibility.eligible {
            (
                ValidationState::RequiresAction,
                vec!["account_not_eligible".to_string()],
            )
        } else if !account.capabilities.upload_video || !account.capabilities.public_posting {
            (
                ValidationState::RequiresAction,
                vec!["missing_publish_capability".to_string()],
            )
        } else if target.scheduled_for <= now {
            (
                ValidationState::Invalid,
                vec!["scheduled_time_invalid".to_string()],
            )
        } else {
            (ValidationState::Valid, Vec::new())
        };

        store.save_campaign_variant_target(target.mark_validation(state.clone(), now))?;
        Ok(TargetValidationReport { state, reasons })
    }

    pub fn schedule_target(
        store: &mut impl SocialStore,
        owner: &OwnerRef,
        input: ScheduleTargetInput,
    ) -> Result<PublishJob, PublishServiceError> {
        let target = store.campaign_variant_target(&input.target_id)?;
        let account = store.connected_account(&target.connected_account_id)?;
        if account.owner != *owner {
            return Err(PublishServiceError::OwnerMismatch);
        }
        if target.validation_state != ValidationState::Valid {
            return Err(PublishServiceError::TargetNotValid);
        }

        let job = PublishJob::new(
            input.job_id,
            target.campaign_id,
            target.variant_id,
            target.connected_account_id,
            target.provider,
            input.artifact_ref,
            target.scheduled_for,
            input.created_by,
        )
        .schedule(input.now);
        store.save_publish_job(job.clone())?;
        store.append_publish_job_event(PublishJobEvent::new(
            format!("event_{}_scheduled", job.id),
            job.id.clone(),
            PublishJobEventType::Scheduled,
            PublishJobActorType::User,
            "publish job scheduled",
            serde_json::json!({"target_id": input.target_id}),
            input.now,
        ))?;
        Ok(job)
    }

    pub fn claim_due_jobs(
        store: &mut impl SocialStore,
        now: i64,
        limit: usize,
    ) -> Result<Vec<PublishJob>, PublishServiceError> {
        Ok(store.claim_due_publish_jobs(now, limit)?)
    }

    pub fn cancel_job(
        store: &mut impl SocialStore,
        owner: &OwnerRef,
        job_id: &str,
        now: i64,
    ) -> Result<PublishJob, PublishServiceError> {
        let job = store.publish_job(job_id)?;
        let account = store.connected_account(&job.connected_account_id)?;
        if account.owner != *owner {
            return Err(PublishServiceError::OwnerMismatch);
        }
        if matches!(
            job.status,
            PublishJobStatus::Published | PublishJobStatus::Cancelled
        ) {
            return Err(PublishServiceError::JobNotCancellable);
        }

        let cancelled = job.cancel(now);
        store.save_publish_job(cancelled.clone())?;
        store.append_publish_job_event(PublishJobEvent::new(
            format!("event_{}_cancelled", cancelled.id),
            cancelled.id.clone(),
            PublishJobEventType::Cancelled,
            PublishJobActorType::User,
            "publish job cancelled",
            serde_json::json!({}),
            now,
        ))?;
        Ok(cancelled)
    }

    pub fn retry_job(
        store: &mut impl SocialStore,
        owner: &OwnerRef,
        job_id: &str,
        now: i64,
    ) -> Result<PublishJob, PublishServiceError> {
        let job = store.publish_job(job_id)?;
        let account = store.connected_account(&job.connected_account_id)?;
        if account.owner != *owner {
            return Err(PublishServiceError::OwnerMismatch);
        }
        if !matches!(
            job.status,
            PublishJobStatus::Failed | PublishJobStatus::RequiresAction
        ) {
            return Err(PublishServiceError::JobNotRetryable);
        }

        let retry = job.retry(now);
        store.save_publish_job(retry.clone())?;
        store.append_publish_job_event(PublishJobEvent::new(
            format!("event_{}_retry", retry.id),
            retry.id.clone(),
            PublishJobEventType::RetryQueued,
            PublishJobActorType::User,
            "publish job retry queued",
            serde_json::json!({}),
            now,
        ))?;
        Ok(retry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef,
        Provider, ProviderCapabilities, PublishJobStatus, ValidationState,
    };
    use crate::store::{InMemorySocialStore, SocialStore};

    #[test]
    fn bind_target_checks_account_owner_and_saves_pending_target() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", owner(), true))
            .unwrap_or_else(|err| panic!("save account: {err}"));

        let target = PublishService::bind_target(
            &mut store,
            &owner(),
            BindTargetInput {
                id: "target_1".into(),
                campaign_id: "campaign_1".into(),
                variant_id: "variant_1".into(),
                connected_account_id: "acct_1".into(),
                platform_fields: serde_json::json!({"privacy": "private"}),
                scheduled_for: 2_000,
                now: 1_000,
            },
        )
        .unwrap_or_else(|err| panic!("bind target: {err}"));

        assert_eq!(target.validation_state, ValidationState::Pending);
        assert_eq!(store.campaign_variant_target("target_1"), Ok(target));
    }

    #[test]
    fn validate_target_marks_requires_action_for_ineligible_account() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", owner(), false))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        bind_target(&mut store);

        let report = PublishService::validate_target(&mut store, &owner(), "target_1", 1_100)
            .unwrap_or_else(|err| panic!("validate target: {err}"));

        assert_eq!(report.state, ValidationState::RequiresAction);
        assert_eq!(report.reasons, vec!["account_not_eligible"]);
        assert_eq!(
            store
                .campaign_variant_target("target_1")
                .unwrap_or_else(|err| panic!("target: {err}"))
                .validation_state,
            ValidationState::RequiresAction
        );
    }

    #[test]
    fn schedule_target_creates_scheduled_job_and_event() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", owner(), true))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        bind_target(&mut store);
        PublishService::validate_target(&mut store, &owner(), "target_1", 1_100)
            .unwrap_or_else(|err| panic!("validate target: {err}"));

        let job = PublishService::schedule_target(
            &mut store,
            &owner(),
            ScheduleTargetInput {
                job_id: "job_1".into(),
                target_id: "target_1".into(),
                artifact_ref: "render://artifact_1".into(),
                created_by: "user_1".into(),
                now: 1_200,
            },
        )
        .unwrap_or_else(|err| panic!("schedule target: {err}"));

        assert_eq!(job.status, PublishJobStatus::Scheduled);
        let events = store
            .publish_job_events("job_1")
            .unwrap_or_else(|err| panic!("events: {err}"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "event_job_1_scheduled");
        assert_eq!(
            events[0].metadata,
            serde_json::json!({"target_id": "target_1"})
        );
    }

    #[test]
    fn queue_contract_claim_cancel_and_retry_are_owner_checked() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", owner(), true))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        bind_target(&mut store);
        PublishService::validate_target(&mut store, &owner(), "target_1", 1_100)
            .unwrap_or_else(|err| panic!("validate target: {err}"));
        PublishService::schedule_target(
            &mut store,
            &owner(),
            ScheduleTargetInput {
                job_id: "job_1".into(),
                target_id: "target_1".into(),
                artifact_ref: "render://artifact_1".into(),
                created_by: "user_1".into(),
                now: 1_200,
            },
        )
        .unwrap_or_else(|err| panic!("schedule target: {err}"));

        let claimed = PublishService::claim_due_jobs(&mut store, 2_100, 10)
            .unwrap_or_else(|err| panic!("claim due jobs: {err}"));
        assert_eq!(claimed[0].status, PublishJobStatus::Uploading);

        assert_eq!(
            PublishService::cancel_job(
                &mut store,
                &OwnerRef::Workspace("workspace_1".into()),
                "job_1",
                2_200,
            ),
            Err(PublishServiceError::OwnerMismatch)
        );

        let cancelled = PublishService::cancel_job(&mut store, &owner(), "job_1", 2_200)
            .unwrap_or_else(|err| panic!("cancel job: {err}"));
        assert_eq!(cancelled.status, PublishJobStatus::Cancelled);

        assert_eq!(
            PublishService::retry_job(&mut store, &owner(), "job_1", 2_300),
            Err(PublishServiceError::JobNotRetryable)
        );
    }

    #[test]
    fn retry_job_requeues_failed_job_after_owner_check() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", owner(), true))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        bind_target(&mut store);
        PublishService::validate_target(&mut store, &owner(), "target_1", 1_100)
            .unwrap_or_else(|err| panic!("validate target: {err}"));
        PublishService::schedule_target(
            &mut store,
            &owner(),
            ScheduleTargetInput {
                job_id: "job_1".into(),
                target_id: "target_1".into(),
                artifact_ref: "render://artifact_1".into(),
                created_by: "user_1".into(),
                now: 1_200,
            },
        )
        .unwrap_or_else(|err| panic!("schedule target: {err}"));
        let failed = store
            .publish_job("job_1")
            .unwrap_or_else(|err| panic!("job: {err}"))
            .fail("provider_error", "errors/job_1.json", 2_200);
        store
            .save_publish_job(failed)
            .unwrap_or_else(|err| panic!("save failed job: {err}"));

        assert_eq!(
            PublishService::retry_job(
                &mut store,
                &OwnerRef::Workspace("workspace_1".into()),
                "job_1",
                2_300,
            ),
            Err(PublishServiceError::OwnerMismatch)
        );

        let retry = PublishService::retry_job(&mut store, &owner(), "job_1", 2_300)
            .unwrap_or_else(|err| panic!("retry job: {err}"));
        assert_eq!(retry.status, PublishJobStatus::Scheduled);
        assert_eq!(retry.normalized_error, None);
        assert_eq!(retry.raw_error_ref, None);

        let events = store
            .publish_job_events("job_1")
            .unwrap_or_else(|err| panic!("events: {err}"));
        assert_eq!(events[1].id, "event_job_1_retry");
    }

    fn owner() -> OwnerRef {
        OwnerRef::User("user_1".into())
    }

    fn connected_account(id: &str, account_owner: OwnerRef, eligible: bool) -> ConnectedAccount {
        ConnectedAccount {
            id: id.into(),
            owner: account_owner,
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            display_name: "Awidat Channel".into(),
            handle: Some("@awidat".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            capabilities: ProviderCapabilities {
                native_scheduling: true,
                queue_scheduling: true,
                upload_video: eligible,
                upload_thumbnail: true,
                public_posting: eligible,
                requires_user_consent: false,
            },
            eligibility: if eligible {
                AccountEligibility::eligible()
            } else {
                AccountEligibility::blocked("missing_youtube_upload_scope")
            },
            last_verified_at: Some(900),
            created_at: 900,
            updated_at: 900,
        }
    }

    fn bind_target(store: &mut InMemorySocialStore) {
        PublishService::bind_target(
            store,
            &owner(),
            BindTargetInput {
                id: "target_1".into(),
                campaign_id: "campaign_1".into(),
                variant_id: "variant_1".into(),
                connected_account_id: "acct_1".into(),
                platform_fields: serde_json::json!({"privacy": "private"}),
                scheduled_for: 2_000,
                now: 1_000,
            },
        )
        .unwrap_or_else(|err| panic!("bind target: {err}"));
    }
}
