use crate::model::{
    CampaignVariantTarget, Provider, PublishJob, PublishJobActorType, PublishJobEvent,
    PublishJobEventType, PublishJobStatus, ValidationState,
};
use base64::Engine;
use sha2::{Digest, Sha256};

impl CampaignVariantTarget {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        campaign_id: impl Into<String>,
        variant_id: impl Into<String>,
        connected_account_id: impl Into<String>,
        provider: Provider,
        platform_fields: serde_json::Value,
        scheduled_for: i64,
        now: i64,
    ) -> Self {
        Self {
            id: id.into(),
            campaign_id: campaign_id.into(),
            variant_id: variant_id.into(),
            connected_account_id: connected_account_id.into(),
            provider,
            platform_fields,
            scheduled_for,
            validation_state: ValidationState::Pending,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn mark_validation(mut self, state: ValidationState, now: i64) -> Self {
        self.validation_state = state;
        self.updated_at = now;
        self
    }
}

impl PublishJob {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        campaign_id: impl Into<String>,
        variant_id: impl Into<String>,
        connected_account_id: impl Into<String>,
        provider: Provider,
        artifact_ref: impl Into<String>,
        scheduled_for: i64,
        created_by: impl Into<String>,
    ) -> Self {
        let campaign_id = campaign_id.into();
        let variant_id = variant_id.into();
        let connected_account_id = connected_account_id.into();
        let artifact_ref = artifact_ref.into();
        let created_by = created_by.into();
        let idempotency_key = idempotency_key(
            &campaign_id,
            &variant_id,
            &connected_account_id,
            &artifact_ref,
        );

        Self {
            id: id.into(),
            campaign_id,
            variant_id,
            connected_account_id,
            provider,
            artifact_ref,
            idempotency_key,
            scheduled_for,
            status: PublishJobStatus::Draft,
            attempt_count: 0,
            provider_post_id: None,
            provider_post_url: None,
            normalized_error: None,
            raw_error_ref: None,
            requires_action_reason: None,
            created_by,
            // A freshly-built job has no real creation timestamp yet — the
            // caller stamps it via `schedule(now)` (the Draft -> Scheduled
            // transition). Until then `created_at` is a sentinel `0`, never the
            // future `scheduled_for`, which would otherwise corrupt
            // creation-time audit/sorting for jobs scheduled in the future.
            created_at: 0,
            updated_at: 0,
        }
    }

    pub fn requires_action(mut self, reason: impl Into<String>, now: i64) -> Self {
        self.status = PublishJobStatus::RequiresAction;
        self.requires_action_reason = Some(reason.into());
        self.updated_at = now;
        self
    }

    pub fn schedule(mut self, now: i64) -> Self {
        // The first schedule (from a freshly-built Draft) is also the job's
        // creation moment — stamp `created_at` with the real wall-clock `now`,
        // not the (possibly future) `scheduled_for`. Re-scheduling an existing
        // job leaves its original `created_at` intact.
        if self.created_at == 0 {
            self.created_at = now;
        }
        self.status = PublishJobStatus::Scheduled;
        self.updated_at = now;
        self
    }

    pub fn claim_for_upload(mut self, now: i64) -> Self {
        self.status = PublishJobStatus::Uploading;
        self.attempt_count = self.attempt_count.saturating_add(1);
        self.updated_at = now;
        self
    }

    pub fn processing(mut self, provider_post_id: impl Into<String>, now: i64) -> Self {
        self.status = PublishJobStatus::Processing;
        self.provider_post_id = Some(provider_post_id.into());
        self.updated_at = now;
        self
    }

    pub fn start_provider_processing(mut self, now: i64) -> Self {
        self.status = PublishJobStatus::Processing;
        self.updated_at = now;
        self
    }

    pub fn publish(
        mut self,
        provider_post_id: impl Into<String>,
        provider_post_url: impl Into<String>,
        now: i64,
    ) -> Self {
        self.status = PublishJobStatus::Published;
        self.provider_post_id = Some(provider_post_id.into());
        self.provider_post_url = Some(provider_post_url.into());
        self.normalized_error = None;
        self.raw_error_ref = None;
        self.requires_action_reason = None;
        self.updated_at = now;
        self
    }

    pub fn cancel(mut self, now: i64) -> Self {
        self.status = PublishJobStatus::Cancelled;
        self.updated_at = now;
        self
    }

    pub fn fail(
        mut self,
        normalized_error: impl Into<String>,
        raw_error_ref: impl Into<String>,
        now: i64,
    ) -> Self {
        self.status = PublishJobStatus::Failed;
        self.normalized_error = Some(normalized_error.into());
        self.raw_error_ref = Some(raw_error_ref.into());
        self.updated_at = now;
        self
    }

    pub fn retry(mut self, now: i64) -> Self {
        self.status = PublishJobStatus::Scheduled;
        self.normalized_error = None;
        self.raw_error_ref = None;
        self.requires_action_reason = None;
        self.updated_at = now;
        self
    }
}

impl PublishJobEvent {
    pub fn new(
        id: impl Into<String>,
        publish_job_id: impl Into<String>,
        event_type: PublishJobEventType,
        actor_type: PublishJobActorType,
        message: impl Into<String>,
        metadata: serde_json::Value,
        created_at: i64,
    ) -> Self {
        Self {
            id: id.into(),
            publish_job_id: publish_job_id.into(),
            event_type,
            actor_type,
            message: message.into(),
            metadata,
            created_at,
        }
    }
}

fn idempotency_key(
    campaign_id: &str,
    variant_id: &str,
    connected_account_id: &str,
    artifact_ref: &str,
) -> String {
    let mut hasher = Sha256::new();
    hash_part(&mut hasher, campaign_id);
    hash_part(&mut hasher, variant_id);
    hash_part(&mut hasher, connected_account_id);
    hash_part(&mut hasher, artifact_ref);
    let digest = hasher.finalize();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn hash_part(hasher: &mut Sha256, value: &str) {
    hasher.update(value.len().to_be_bytes());
    hasher.update(value.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_job_idempotency_key_is_stable_for_same_target() {
        let a = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            1_800,
            "user_1",
        );
        let b = PublishJob::new(
            "job_2",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            1_800,
            "user_1",
        );

        assert_eq!(a.idempotency_key, b.idempotency_key);
        assert_eq!(a.status, PublishJobStatus::Draft);
    }

    #[test]
    fn publish_job_idempotency_key_preserves_field_boundaries() {
        let a = PublishJob::new(
            "job_1",
            "campaign:1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            1_800,
            "user_1",
        );
        let b = PublishJob::new(
            "job_2",
            "campaign",
            "1:variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            1_800,
            "user_1",
        );

        assert_ne!(a.idempotency_key, b.idempotency_key);
    }

    #[test]
    fn publish_job_can_move_to_requires_action_with_reason() {
        let job = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::TikTok,
            "render://artifact_1",
            1_800,
            "user_1",
        )
        .requires_action("tiktok_direct_post_permission_required", 2_100);

        assert_eq!(job.status, PublishJobStatus::RequiresAction);
        assert_eq!(job.updated_at, 2_100);
        assert_eq!(
            job.requires_action_reason.as_deref(),
            Some("tiktok_direct_post_permission_required")
        );
    }

    #[test]
    fn campaign_variant_target_new_starts_pending_with_timestamps() {
        let target = CampaignVariantTarget::new(
            "target_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::Instagram,
            serde_json::json!({"caption": "Launch clip"}),
            2_400,
            2_000,
        );

        assert_eq!(target.validation_state, ValidationState::Pending);
        assert_eq!(target.created_at, 2_000);
        assert_eq!(target.updated_at, 2_000);
    }

    #[test]
    fn campaign_variant_target_can_mark_validation_state() {
        let target = CampaignVariantTarget::new(
            "target_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            serde_json::json!({}),
            2_400,
            2_000,
        )
        .mark_validation(ValidationState::Valid, 2_100);

        assert_eq!(target.validation_state, ValidationState::Valid);
        assert_eq!(target.created_at, 2_000);
        assert_eq!(target.updated_at, 2_100);
    }

    #[test]
    fn publish_job_schedule_claim_fail_and_retry_transitions() {
        let job = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::TikTok,
            "render://artifact_1",
            1_800,
            "user_1",
        )
        .schedule(2_000)
        .claim_for_upload(2_100)
        .fail("provider_rate_limited", "errors/job_1.json", 2_200)
        .requires_action("tiktok_reauth_required", 2_250)
        .retry(2_300);

        assert_eq!(job.status, PublishJobStatus::Scheduled);
        assert_eq!(job.attempt_count, 1);
        assert_eq!(job.updated_at, 2_300);
        assert_eq!(job.normalized_error, None);
        assert_eq!(job.raw_error_ref, None);
        assert_eq!(job.requires_action_reason, None);
    }

    #[test]
    fn publish_job_created_at_reflects_schedule_time_not_future_due_time() {
        // Job scheduled for the future (scheduled_for = 9_000) but created now
        // (schedule(now=2_000)). created_at must be the creation moment, not
        // the future due time, so creation-time audit/sorting stays correct.
        let job = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            9_000,
            "user_1",
        )
        .schedule(2_000);

        assert_eq!(job.scheduled_for, 9_000);
        assert_eq!(job.created_at, 2_000, "created_at = creation time");
        assert_eq!(job.updated_at, 2_000);

        // Re-scheduling (e.g. after a retry path) preserves the original
        // creation timestamp.
        let rescheduled = job.retry(2_500).schedule(2_600);
        assert_eq!(rescheduled.created_at, 2_000, "creation time is sticky");
        assert_eq!(rescheduled.updated_at, 2_600);
    }

    #[test]
    fn publish_job_can_move_to_processing_and_published() {
        let job = PublishJob::new(
            "job_1",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            1_800,
            "user_1",
        )
        .schedule(2_000)
        .claim_for_upload(2_100)
        .fail("temporary_provider_error", "errors/job_1.json", 2_150)
        .requires_action("youtube_reauth_required", 2_175)
        .claim_for_upload(2_190)
        .processing("video_123", 2_200);

        assert_eq!(job.status, PublishJobStatus::Processing);
        assert_eq!(job.provider_post_id.as_deref(), Some("video_123"));
        assert_eq!(job.provider_post_url, None);
        assert_eq!(
            job.normalized_error.as_deref(),
            Some("temporary_provider_error")
        );
        assert_eq!(job.raw_error_ref.as_deref(), Some("errors/job_1.json"));
        assert_eq!(
            job.requires_action_reason.as_deref(),
            Some("youtube_reauth_required")
        );
        assert_eq!(job.updated_at, 2_200);

        let job = job.publish("video_123", "https://youtube.com/watch?v=video_123", 2_300);

        assert_eq!(job.status, PublishJobStatus::Published);
        assert_eq!(job.provider_post_id.as_deref(), Some("video_123"));
        assert_eq!(
            job.provider_post_url.as_deref(),
            Some("https://youtube.com/watch?v=video_123")
        );
        assert_eq!(job.normalized_error, None);
        assert_eq!(job.raw_error_ref, None);
        assert_eq!(job.requires_action_reason, None);
        assert_eq!(job.updated_at, 2_300);
    }

    #[test]
    fn publish_job_event_serialization_includes_message_without_token_material() {
        let event = PublishJobEvent::new(
            "event_1",
            "job_1",
            PublishJobEventType::RetryQueued,
            PublishJobActorType::Worker,
            "retry queued after provider timeout",
            serde_json::json!({
                "safe_ref": "errors/job_1.json",
                "note": "token storage remains server side"
            }),
            2_400,
        );

        let json = serde_json::to_string(&event)
            .unwrap_or_else(|err| panic!("serialize publish job event: {err}"));

        assert!(json.contains("retry queued after provider timeout"));
        assert!(json.contains("\"event_type\":\"retry_queued\""));
        assert!(!json.contains("access_token"));
        assert!(!json.contains("refresh_token"));
    }
}
