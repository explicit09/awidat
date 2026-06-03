use crate::model::{Provider, PublishJob, PublishJobStatus};
use base64::Engine;
use sha2::{Digest, Sha256};

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
}

fn idempotency_key(
    campaign_id: &str,
    variant_id: &str,
    connected_account_id: &str,
    artifact_ref: &str,
) -> String {
    let raw = format!("{campaign_id}:{variant_id}:{connected_account_id}:{artifact_ref}");
    let digest = Sha256::digest(raw.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
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
}
