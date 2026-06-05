use crate::model::{
    CampaignVariantTarget, ConnectedAccount, ConnectedAccountStatus, OwnerRef, PublishJob,
    PublishJobActorType, PublishJobEvent, PublishJobEventType, PublishJobStatus, ValidationState,
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
    #[error("publish job id is already used by a different target")]
    JobIdConflict,
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
        if let Some(existing) = existing_target(store, &input.id)? {
            let existing_account = store.connected_account(&existing.connected_account_id)?;
            if existing_account.owner != *owner {
                return Err(PublishServiceError::OwnerMismatch);
            }
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

        let (state, reasons) = target_validation_state(&target, &account, now);

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
        if account.provider != target.provider {
            return Err(PublishServiceError::ProviderMismatch);
        }
        if target.validation_state != ValidationState::Valid {
            return Err(PublishServiceError::TargetNotValid);
        }
        let (current_state, _) = target_validation_state(&target, &account, input.now);
        if current_state != ValidationState::Valid {
            store.save_campaign_variant_target(target.mark_validation(current_state, input.now))?;
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

        if let Some(existing) = existing_job(store, &job.id)? {
            let existing_account = store.connected_account(&existing.connected_account_id)?;
            if existing_account.owner != *owner {
                return Err(PublishServiceError::OwnerMismatch);
            }
            // Reusing a job id is only allowed as an idempotent retry of the
            // *same* publish — same campaign/variant/account/artifact. If the
            // id is reused for a different target, rejecting it protects an
            // existing (possibly already-published) job from being silently
            // overwritten by the upsert below.
            if existing.idempotency_key != job.idempotency_key {
                return Err(PublishServiceError::JobIdConflict);
            }
        }

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
        if !is_user_retryable(&job) {
            return Err(PublishServiceError::JobNotRetryable);
        }

        let retry = job.retry(now);
        let retry_event_id = retry_event_id(store, &retry.id)?;
        store.save_publish_job(retry.clone())?;
        store.append_publish_job_event(PublishJobEvent::new(
            retry_event_id,
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

fn is_user_retryable(job: &PublishJob) -> bool {
    matches!(
        job.status,
        PublishJobStatus::Failed | PublishJobStatus::RequiresAction
    ) || (matches!(
        job.status,
        PublishJobStatus::Uploading | PublishJobStatus::Processing
    ) && job.provider_post_id.is_none())
}

fn existing_target(
    store: &impl SocialStore,
    target_id: &str,
) -> Result<Option<CampaignVariantTarget>, PublishServiceError> {
    match store.campaign_variant_target(target_id) {
        Ok(target) => Ok(Some(target)),
        Err(SocialStoreError::NotFound) => Ok(None),
        Err(err) => Err(PublishServiceError::Store(err)),
    }
}

fn existing_job(
    store: &impl SocialStore,
    job_id: &str,
) -> Result<Option<PublishJob>, PublishServiceError> {
    match store.publish_job(job_id) {
        Ok(job) => Ok(Some(job)),
        Err(SocialStoreError::NotFound) => Ok(None),
        Err(err) => Err(PublishServiceError::Store(err)),
    }
}

fn target_validation_state(
    target: &CampaignVariantTarget,
    account: &ConnectedAccount,
    now: i64,
) -> (ValidationState, Vec<String>) {
    if account.status != ConnectedAccountStatus::Connected {
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
    }
}

fn retry_event_id(store: &impl SocialStore, job_id: &str) -> Result<String, PublishServiceError> {
    let retry_count = store
        .publish_job_events(job_id)?
        .into_iter()
        .filter(|event| event.event_type == PublishJobEventType::RetryQueued)
        .count();
    Ok(format!("event_{job_id}_retry_{}", retry_count + 1))
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
    fn bind_target_does_not_overwrite_existing_target_for_other_owner() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", owner(), true))
            .unwrap_or_else(|err| panic!("save owner account: {err}"));
        store
            .save_connected_account(connected_account("acct_2", other_owner(), true))
            .unwrap_or_else(|err| panic!("save other account: {err}"));
        let original = PublishService::bind_target(
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
        .unwrap_or_else(|err| panic!("bind original target: {err}"));

        assert_eq!(
            PublishService::bind_target(
                &mut store,
                &other_owner(),
                BindTargetInput {
                    id: "target_1".into(),
                    campaign_id: "campaign_2".into(),
                    variant_id: "variant_2".into(),
                    connected_account_id: "acct_2".into(),
                    platform_fields: serde_json::json!({"privacy": "public"}),
                    scheduled_for: 3_000,
                    now: 1_100,
                },
            ),
            Err(PublishServiceError::OwnerMismatch)
        );
        assert_eq!(store.campaign_variant_target("target_1"), Ok(original));
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
    fn schedule_target_does_not_overwrite_existing_job_for_other_owner() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", owner(), true))
            .unwrap_or_else(|err| panic!("save owner account: {err}"));
        store
            .save_connected_account(connected_account("acct_2", other_owner(), true))
            .unwrap_or_else(|err| panic!("save other account: {err}"));
        bind_target(&mut store);
        PublishService::validate_target(&mut store, &owner(), "target_1", 1_100)
            .unwrap_or_else(|err| panic!("validate owner target: {err}"));
        let original = PublishService::schedule_target(
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
        .unwrap_or_else(|err| panic!("schedule owner target: {err}"));
        PublishService::bind_target(
            &mut store,
            &other_owner(),
            BindTargetInput {
                id: "target_2".into(),
                campaign_id: "campaign_2".into(),
                variant_id: "variant_2".into(),
                connected_account_id: "acct_2".into(),
                platform_fields: serde_json::json!({"privacy": "private"}),
                scheduled_for: 2_500,
                now: 1_000,
            },
        )
        .unwrap_or_else(|err| panic!("bind other target: {err}"));
        PublishService::validate_target(&mut store, &other_owner(), "target_2", 1_100)
            .unwrap_or_else(|err| panic!("validate other target: {err}"));

        assert_eq!(
            PublishService::schedule_target(
                &mut store,
                &other_owner(),
                ScheduleTargetInput {
                    job_id: "job_1".into(),
                    target_id: "target_2".into(),
                    artifact_ref: "render://artifact_2".into(),
                    created_by: "user_2".into(),
                    now: 1_200,
                },
            ),
            Err(PublishServiceError::OwnerMismatch)
        );
        assert_eq!(store.publish_job("job_1"), Ok(original));
    }

    #[test]
    fn schedule_target_rejects_reused_job_id_for_different_target() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", owner(), true))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        bind_target(&mut store);
        PublishService::validate_target(&mut store, &owner(), "target_1", 1_100)
            .unwrap_or_else(|err| panic!("validate target_1: {err}"));
        let original = PublishService::schedule_target(
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
        .unwrap_or_else(|err| panic!("schedule target_1: {err}"));

        // A second target owned by the SAME owner, different campaign/variant.
        PublishService::bind_target(
            &mut store,
            &owner(),
            BindTargetInput {
                id: "target_2".into(),
                campaign_id: "campaign_2".into(),
                variant_id: "variant_2".into(),
                connected_account_id: "acct_1".into(),
                platform_fields: serde_json::json!({"privacy": "private"}),
                scheduled_for: 2_500,
                now: 1_000,
            },
        )
        .unwrap_or_else(|err| panic!("bind target_2: {err}"));
        PublishService::validate_target(&mut store, &owner(), "target_2", 1_100)
            .unwrap_or_else(|err| panic!("validate target_2: {err}"));

        // Reusing job_1 for the different target must be rejected, not upsert
        // over the existing job.
        assert_eq!(
            PublishService::schedule_target(
                &mut store,
                &owner(),
                ScheduleTargetInput {
                    job_id: "job_1".into(),
                    target_id: "target_2".into(),
                    artifact_ref: "render://artifact_2".into(),
                    created_by: "user_1".into(),
                    now: 1_300,
                },
            ),
            Err(PublishServiceError::JobIdConflict)
        );
        // The original job is untouched.
        assert_eq!(store.publish_job("job_1"), Ok(original));
    }

    #[test]
    fn schedule_target_allows_idempotent_reschedule_of_same_target() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", owner(), true))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        bind_target(&mut store);
        PublishService::validate_target(&mut store, &owner(), "target_1", 1_100)
            .unwrap_or_else(|err| panic!("validate target: {err}"));
        let input = || ScheduleTargetInput {
            job_id: "job_1".into(),
            target_id: "target_1".into(),
            artifact_ref: "render://artifact_1".into(),
            created_by: "user_1".into(),
            now: 1_200,
        };
        PublishService::schedule_target(&mut store, &owner(), input())
            .unwrap_or_else(|err| panic!("schedule target: {err}"));

        // Same id + same target/artifact = idempotent retry, still allowed.
        let again = PublishService::schedule_target(&mut store, &owner(), input())
            .unwrap_or_else(|err| panic!("reschedule target: {err}"));
        assert_eq!(again.status, PublishJobStatus::Scheduled);
    }

    #[test]
    fn schedule_target_rechecks_current_account_publishability() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", owner(), true))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        bind_target(&mut store);
        PublishService::validate_target(&mut store, &owner(), "target_1", 1_100)
            .unwrap_or_else(|err| panic!("validate target: {err}"));
        let mut account = store
            .connected_account("acct_1")
            .unwrap_or_else(|err| panic!("account: {err}"));
        account.status = ConnectedAccountStatus::Disabled;
        store
            .save_connected_account(account)
            .unwrap_or_else(|err| panic!("disable account: {err}"));

        assert_eq!(
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
            ),
            Err(PublishServiceError::TargetNotValid)
        );
        assert_eq!(
            store
                .campaign_variant_target("target_1")
                .unwrap_or_else(|err| panic!("target: {err}"))
                .validation_state,
            ValidationState::RequiresAction
        );
        assert_eq!(store.publish_job("job_1"), Err(SocialStoreError::NotFound));
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

        let failed_again = store
            .publish_job("job_1")
            .unwrap_or_else(|err| panic!("job after retry: {err}"))
            .fail("provider_error_again", "errors/job_1_again.json", 2_350);
        store
            .save_publish_job(failed_again)
            .unwrap_or_else(|err| panic!("save failed job again: {err}"));
        PublishService::retry_job(&mut store, &owner(), "job_1", 2_300)
            .unwrap_or_else(|err| panic!("retry job again: {err}"));

        let events = store
            .publish_job_events("job_1")
            .unwrap_or_else(|err| panic!("events: {err}"));
        assert_eq!(events[1].id, "event_job_1_retry_1");
        assert_eq!(events[2].id, "event_job_1_retry_2");
    }

    #[test]
    fn retry_job_requeues_stranded_in_flight_job_without_provider_post_id() {
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
        let stranded = store
            .publish_job("job_1")
            .unwrap_or_else(|err| panic!("job: {err}"))
            .start_provider_processing(2_200);
        store
            .save_publish_job(stranded)
            .unwrap_or_else(|err| panic!("save stranded job: {err}"));

        let retry = PublishService::retry_job(&mut store, &owner(), "job_1", 2_300)
            .unwrap_or_else(|err| panic!("retry job: {err}"));
        assert_eq!(retry.status, PublishJobStatus::Scheduled);
        assert_eq!(retry.provider_post_id, None);

        let processing_with_post = retry.processing("yt_video_1", 2_400);
        store
            .save_publish_job(processing_with_post)
            .unwrap_or_else(|err| panic!("save processing job: {err}"));
        assert_eq!(
            PublishService::retry_job(&mut store, &owner(), "job_1", 2_500),
            Err(PublishServiceError::JobNotRetryable)
        );
    }

    fn owner() -> OwnerRef {
        OwnerRef::User("user_1".into())
    }

    fn other_owner() -> OwnerRef {
        OwnerRef::User("user_2".into())
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
