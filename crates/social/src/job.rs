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
            created_at: scheduled_for,
            updated_at: scheduled_for,
        }
    }

    pub fn requires_action(mut self, reason: impl Into<String>) -> Self {
        self.status = PublishJobStatus::RequiresAction;
        self.requires_action_reason = Some(reason.into());
        self
    }

    pub fn schedule(mut self, now: i64) -> Self {
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

    pub fn cancel(mut self, now: i64) -> Self {
        self.status = PublishJobStatus::Cancelled;
        self.updated_at = now;
        self
    }

    pub fn fail(
        mut self,
        normalized_error: impl Into<String>,
        raw_error_ref: Option<String>,
        now: i64,
    ) -> Self {
        self.status = PublishJobStatus::Failed;
        self.normalized_error = Some(normalized_error.into());
        self.raw_error_ref = raw_error_ref;
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
        message: Option<String>,
        metadata: serde_json::Value,
        created_at: i64,
    ) -> Self {
        Self {
            id: id.into(),
            publish_job_id: publish_job_id.into(),
            event_type,
            actor_type,
            message,
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
        .requires_action("tiktok_direct_post_permission_required");

        assert_eq!(job.status, PublishJobStatus::RequiresAction);
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
        .fail(
            "provider_rate_limited",
            Some("errors/job_1.json".into()),
            2_200,
        )
        .requires_action("tiktok_reauth_required")
        .retry(2_300);

        assert_eq!(job.status, PublishJobStatus::Scheduled);
        assert_eq!(job.attempt_count, 1);
        assert_eq!(job.updated_at, 2_300);
        assert_eq!(job.normalized_error, None);
        assert_eq!(job.raw_error_ref, None);
        assert_eq!(job.requires_action_reason, None);
    }

    #[test]
    fn publish_job_event_serialization_includes_message_without_token_material() {
        let event = PublishJobEvent::new(
            "event_1",
            "job_1",
            PublishJobEventType::RetryQueued,
            PublishJobActorType::Worker,
            Some("retry queued after provider timeout".into()),
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
