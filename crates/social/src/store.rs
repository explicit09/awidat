use crate::model::{
    AccountPublishDefaults, CampaignVariantTarget, ConnectedAccount, ConnectedAccountStatus,
    OwnerRef, PublishJob, PublishJobEvent, PublishJobStatus, WorkspaceMemberRole,
};
use crate::oauth::{OAuthConnection, OAuthConnectionStatus};
use crate::token::TokenSecret;
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SocialStoreError {
    #[error("record not found")]
    NotFound,
    #[error("record owner does not match")]
    OwnerMismatch,
    #[error("connected account already exists for owner and provider account")]
    DuplicateConnectedAccount,
    #[error("storage error: {0}")]
    Storage(String),
}

pub trait SocialStore {
    fn save_oauth_connection(
        &mut self,
        connection: OAuthConnection,
    ) -> Result<(), SocialStoreError>;

    fn oauth_connection(&self, id: &str) -> Result<OAuthConnection, SocialStoreError>;

    fn update_oauth_status(
        &mut self,
        id: &str,
        status: OAuthConnectionStatus,
    ) -> Result<OAuthConnection, SocialStoreError>;

    fn save_connected_account(&mut self, account: ConnectedAccount)
    -> Result<(), SocialStoreError>;

    fn connected_account(&self, id: &str) -> Result<ConnectedAccount, SocialStoreError>;

    fn connected_accounts_for_owner(
        &self,
        owner: &OwnerRef,
    ) -> Result<Vec<ConnectedAccount>, SocialStoreError>;

    fn disable_connected_account(
        &mut self,
        id: &str,
        owner: &OwnerRef,
        now: i64,
    ) -> Result<ConnectedAccount, SocialStoreError>;

    fn save_token_secret(&mut self, secret: TokenSecret) -> Result<(), SocialStoreError>;

    fn token_secret_for_account(&self, account_id: &str) -> Result<TokenSecret, SocialStoreError>;

    fn save_campaign_variant_target(
        &mut self,
        target: CampaignVariantTarget,
    ) -> Result<(), SocialStoreError>;

    fn campaign_variant_target(&self, id: &str) -> Result<CampaignVariantTarget, SocialStoreError>;

    fn save_publish_job(&mut self, job: PublishJob) -> Result<(), SocialStoreError>;

    fn publish_job(&self, id: &str) -> Result<PublishJob, SocialStoreError>;

    fn publish_jobs_for_account(
        &self,
        connected_account_id: &str,
    ) -> Result<Vec<PublishJob>, SocialStoreError>;

    fn claim_due_publish_jobs(
        &mut self,
        now: i64,
        limit: usize,
    ) -> Result<Vec<PublishJob>, SocialStoreError>;

    fn append_publish_job_event(&mut self, event: PublishJobEvent) -> Result<(), SocialStoreError>;

    fn publish_job_events(
        &self,
        publish_job_id: &str,
    ) -> Result<Vec<PublishJobEvent>, SocialStoreError>;

    fn save_account_publish_defaults(
        &mut self,
        defaults: AccountPublishDefaults,
    ) -> Result<(), SocialStoreError>;

    fn account_publish_defaults(
        &self,
        connected_account_id: &str,
    ) -> Result<AccountPublishDefaults, SocialStoreError>;

    fn save_workspace_member_role(
        &mut self,
        role: WorkspaceMemberRole,
    ) -> Result<(), SocialStoreError>;

    fn workspace_member_roles(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<WorkspaceMemberRole>, SocialStoreError>;
}

#[derive(Clone, Debug, Default)]
pub struct InMemorySocialStore {
    oauth_connections: BTreeMap<String, OAuthConnection>,
    connected_accounts: BTreeMap<String, ConnectedAccount>,
    token_secrets: BTreeMap<String, TokenSecret>,
    campaign_variant_targets: BTreeMap<String, CampaignVariantTarget>,
    publish_jobs: BTreeMap<String, PublishJob>,
    publish_job_events: BTreeMap<String, Vec<PublishJobEvent>>,
    account_publish_defaults: BTreeMap<String, AccountPublishDefaults>,
    workspace_member_roles: BTreeMap<(String, String), WorkspaceMemberRole>,
}

impl SocialStore for InMemorySocialStore {
    fn save_oauth_connection(
        &mut self,
        connection: OAuthConnection,
    ) -> Result<(), SocialStoreError> {
        self.oauth_connections
            .insert(connection.id.clone(), connection);
        Ok(())
    }

    fn oauth_connection(&self, id: &str) -> Result<OAuthConnection, SocialStoreError> {
        self.oauth_connections
            .get(id)
            .cloned()
            .ok_or(SocialStoreError::NotFound)
    }

    fn update_oauth_status(
        &mut self,
        id: &str,
        status: OAuthConnectionStatus,
    ) -> Result<OAuthConnection, SocialStoreError> {
        let connection = self
            .oauth_connections
            .get_mut(id)
            .ok_or(SocialStoreError::NotFound)?;
        connection.status = status;
        Ok(connection.clone())
    }

    fn save_connected_account(
        &mut self,
        account: ConnectedAccount,
    ) -> Result<(), SocialStoreError> {
        if self.connected_accounts.values().any(|existing| {
            existing.id != account.id
                && existing.owner == account.owner
                && existing.provider == account.provider
                && existing.provider_account_id == account.provider_account_id
        }) {
            return Err(SocialStoreError::DuplicateConnectedAccount);
        }

        self.connected_accounts.insert(account.id.clone(), account);
        Ok(())
    }

    fn connected_account(&self, id: &str) -> Result<ConnectedAccount, SocialStoreError> {
        self.connected_accounts
            .get(id)
            .cloned()
            .ok_or(SocialStoreError::NotFound)
    }

    fn connected_accounts_for_owner(
        &self,
        owner: &OwnerRef,
    ) -> Result<Vec<ConnectedAccount>, SocialStoreError> {
        Ok(self
            .connected_accounts
            .values()
            .filter(|account| account.owner == *owner)
            .cloned()
            .collect())
    }

    fn disable_connected_account(
        &mut self,
        id: &str,
        owner: &OwnerRef,
        now: i64,
    ) -> Result<ConnectedAccount, SocialStoreError> {
        let account = self
            .connected_accounts
            .get_mut(id)
            .ok_or(SocialStoreError::NotFound)?;
        if account.owner != *owner {
            return Err(SocialStoreError::OwnerMismatch);
        }

        account.status = ConnectedAccountStatus::Disabled;
        account.updated_at = now;
        Ok(account.clone())
    }

    fn save_token_secret(&mut self, secret: TokenSecret) -> Result<(), SocialStoreError> {
        self.token_secrets
            .insert(secret.connected_account_id.clone(), secret);
        Ok(())
    }

    fn token_secret_for_account(&self, account_id: &str) -> Result<TokenSecret, SocialStoreError> {
        self.token_secrets
            .get(account_id)
            .cloned()
            .ok_or(SocialStoreError::NotFound)
    }

    fn save_campaign_variant_target(
        &mut self,
        target: CampaignVariantTarget,
    ) -> Result<(), SocialStoreError> {
        self.campaign_variant_targets
            .insert(target.id.clone(), target);
        Ok(())
    }

    fn campaign_variant_target(&self, id: &str) -> Result<CampaignVariantTarget, SocialStoreError> {
        self.campaign_variant_targets
            .get(id)
            .cloned()
            .ok_or(SocialStoreError::NotFound)
    }

    fn save_publish_job(&mut self, job: PublishJob) -> Result<(), SocialStoreError> {
        self.publish_jobs.insert(job.id.clone(), job);
        Ok(())
    }

    fn publish_job(&self, id: &str) -> Result<PublishJob, SocialStoreError> {
        self.publish_jobs
            .get(id)
            .cloned()
            .ok_or(SocialStoreError::NotFound)
    }

    fn publish_jobs_for_account(
        &self,
        connected_account_id: &str,
    ) -> Result<Vec<PublishJob>, SocialStoreError> {
        Ok(self
            .publish_jobs
            .values()
            .filter(|job| job.connected_account_id == connected_account_id)
            .cloned()
            .collect())
    }

    fn claim_due_publish_jobs(
        &mut self,
        now: i64,
        limit: usize,
    ) -> Result<Vec<PublishJob>, SocialStoreError> {
        let mut due_jobs: Vec<(i64, String)> = self
            .publish_jobs
            .iter()
            .filter(|(_, job)| {
                job.status == PublishJobStatus::Scheduled && job.scheduled_for <= now
            })
            .map(|(id, job)| (job.scheduled_for, id.clone()))
            .collect();
        due_jobs.sort();
        let due_ids: Vec<String> = due_jobs.into_iter().take(limit).map(|(_, id)| id).collect();

        let mut claimed = Vec::with_capacity(due_ids.len());
        for id in due_ids {
            let job = self
                .publish_jobs
                .get(&id)
                .cloned()
                .ok_or(SocialStoreError::NotFound)?
                .claim_for_upload(now);
            self.publish_jobs.insert(id, job.clone());
            claimed.push(job);
        }

        Ok(claimed)
    }

    fn append_publish_job_event(&mut self, event: PublishJobEvent) -> Result<(), SocialStoreError> {
        self.publish_job_events
            .entry(event.publish_job_id.clone())
            .or_default()
            .push(event);
        Ok(())
    }

    fn publish_job_events(
        &self,
        publish_job_id: &str,
    ) -> Result<Vec<PublishJobEvent>, SocialStoreError> {
        Ok(self
            .publish_job_events
            .get(publish_job_id)
            .cloned()
            .unwrap_or_default())
    }

    fn save_account_publish_defaults(
        &mut self,
        defaults: AccountPublishDefaults,
    ) -> Result<(), SocialStoreError> {
        self.account_publish_defaults
            .insert(defaults.connected_account_id.clone(), defaults);
        Ok(())
    }

    fn account_publish_defaults(
        &self,
        connected_account_id: &str,
    ) -> Result<AccountPublishDefaults, SocialStoreError> {
        self.account_publish_defaults
            .get(connected_account_id)
            .cloned()
            .ok_or(SocialStoreError::NotFound)
    }

    fn save_workspace_member_role(
        &mut self,
        role: WorkspaceMemberRole,
    ) -> Result<(), SocialStoreError> {
        self.workspace_member_roles
            .insert((role.workspace_id.clone(), role.user_id.clone()), role);
        Ok(())
    }

    fn workspace_member_roles(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<WorkspaceMemberRole>, SocialStoreError> {
        Ok(self
            .workspace_member_roles
            .values()
            .filter(|role| role.workspace_id == workspace_id)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CampaignVariantTarget;
    use crate::model::{
        AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef,
        Provider, ProviderCapabilities, PublishJob, PublishJobActorType, PublishJobEvent,
        PublishJobEventType, PublishJobStatus, ValidationState,
    };
    use crate::oauth::{OAuthConnection, OAuthConnectionStatus};
    use crate::token::{TestKeyProvider, TokenSecret};

    fn owner() -> OwnerRef {
        OwnerRef::User("user_1".into())
    }

    fn other_owner() -> OwnerRef {
        OwnerRef::Workspace("workspace_1".into())
    }

    fn oauth_connection(id: &str) -> OAuthConnection {
        OAuthConnection::start(
            id,
            owner(),
            Provider::YouTube,
            "state-secret",
            vec!["youtube.upload".into()],
            "/campaigns/campaign_1".into(),
            100,
            200,
        )
    }

    fn connected_account(id: &str) -> ConnectedAccount {
        ConnectedAccount {
            id: id.into(),
            owner: owner(),
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            display_name: "Awidat Channel".into(),
            handle: Some("@awidat".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["youtube.upload".into()],
            capabilities: ProviderCapabilities::default(),
            eligibility: AccountEligibility::eligible(),
            last_verified_at: None,
            created_at: 100,
            updated_at: 100,
        }
    }

    fn token_secret(account_id: &str) -> TokenSecret {
        TokenSecret::encrypt(
            account_id,
            "access-secret",
            Some("refresh-secret"),
            &TestKeyProvider::new("test-key-1", "local-key"),
            100,
        )
        .unwrap_or_else(|err| panic!("encrypt token secret: {err}"))
    }

    fn campaign_variant_target(id: &str) -> CampaignVariantTarget {
        CampaignVariantTarget::new(
            id,
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            serde_json::json!({"title": "Launch clip"}),
            1_800,
            100,
        )
        .mark_validation(ValidationState::Valid, 120)
    }

    fn publish_job(id: &str, scheduled_for: i64) -> PublishJob {
        PublishJob::new(
            id,
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://artifact_1",
            scheduled_for,
            "user_1",
        )
        .schedule(100)
    }

    fn publish_job_event(id: &str, publish_job_id: &str) -> PublishJobEvent {
        PublishJobEvent::new(
            id,
            publish_job_id,
            PublishJobEventType::Scheduled,
            PublishJobActorType::User,
            "Scheduled for upload",
            serde_json::json!({"source": "campaign"}),
            130,
        )
    }

    #[test]
    fn persists_oauth_account_and_token_records() {
        let mut store = InMemorySocialStore::default();
        let connection = oauth_connection("oauth_1");
        let account = connected_account("acct_1");
        let secret = token_secret("acct_1");

        store
            .save_oauth_connection(connection.clone())
            .unwrap_or_else(|err| panic!("save oauth connection: {err}"));
        store
            .save_connected_account(account.clone())
            .unwrap_or_else(|err| panic!("save connected account: {err}"));
        store
            .save_token_secret(secret.clone())
            .unwrap_or_else(|err| panic!("save token secret: {err}"));

        assert_eq!(store.oauth_connection("oauth_1"), Ok(connection));
        assert_eq!(store.connected_account("acct_1"), Ok(account.clone()));
        assert_eq!(
            store.connected_accounts_for_owner(&owner()),
            Ok(vec![account])
        );
        assert_eq!(store.token_secret_for_account("acct_1"), Ok(secret));
    }

    #[test]
    fn persists_campaign_targets_publish_jobs_and_events() {
        let mut store = InMemorySocialStore::default();
        let target = campaign_variant_target("target_1");
        let job = publish_job("job_1", 1_800);
        let event = publish_job_event("event_1", "job_1");

        store
            .save_campaign_variant_target(target.clone())
            .unwrap_or_else(|err| panic!("save campaign variant target: {err}"));
        store
            .save_publish_job(job.clone())
            .unwrap_or_else(|err| panic!("save publish job: {err}"));
        store
            .append_publish_job_event(event.clone())
            .unwrap_or_else(|err| panic!("append publish job event: {err}"));

        assert_eq!(store.campaign_variant_target("target_1"), Ok(target));
        assert_eq!(store.publish_job("job_1"), Ok(job));
        assert_eq!(store.publish_job_events("job_1"), Ok(vec![event]));
    }

    #[test]
    fn claims_due_scheduled_publish_jobs_once_in_schedule_order() {
        let mut store = InMemorySocialStore::default();
        let due_b = publish_job("job_b", 1_900);
        let due_a = publish_job("job_a", 1_000);
        let future = publish_job("job_c", 2_100);

        for job in [due_b, due_a, future.clone()] {
            store
                .save_publish_job(job)
                .unwrap_or_else(|err| panic!("save publish job: {err}"));
        }

        let claimed = store
            .claim_due_publish_jobs(2_000, 10)
            .unwrap_or_else(|err| panic!("claim due publish jobs: {err}"));

        assert_eq!(claimed.len(), 2);
        assert_eq!(claimed[0].id, "job_a");
        assert_eq!(claimed[1].id, "job_b");
        for job in claimed {
            assert_eq!(job.status, PublishJobStatus::Uploading);
            assert_eq!(job.attempt_count, 1);
            assert_eq!(job.updated_at, 2_000);
            assert_eq!(store.publish_job(&job.id), Ok(job));
        }

        assert_eq!(store.publish_job("job_c"), Ok(future));
        assert_eq!(
            store
                .claim_due_publish_jobs(2_000, 10)
                .unwrap_or_else(|err| panic!("claim due publish jobs again: {err}")),
            Vec::<PublishJob>::new()
        );
    }

    #[test]
    fn claim_limit_respects_schedule_before_job_id_order() {
        let mut store = InMemorySocialStore::default();
        let later_by_schedule_first_by_id = publish_job("job_a", 1_900);
        let earlier_by_schedule_second_by_id = publish_job("job_b", 1_000);

        for job in [
            later_by_schedule_first_by_id.clone(),
            earlier_by_schedule_second_by_id,
        ] {
            store
                .save_publish_job(job)
                .unwrap_or_else(|err| panic!("save publish job: {err}"));
        }

        let claimed = store
            .claim_due_publish_jobs(2_000, 1)
            .unwrap_or_else(|err| panic!("claim due publish jobs: {err}"));

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, "job_b");
        assert_eq!(
            store.publish_job("job_a"),
            Ok(later_by_schedule_first_by_id)
        );
    }

    #[test]
    fn rejects_duplicate_provider_account_for_same_owner() {
        let mut store = InMemorySocialStore::default();
        let account = connected_account("acct_1");
        let mut duplicate = connected_account("acct_2");

        store
            .save_connected_account(account)
            .unwrap_or_else(|err| panic!("save connected account: {err}"));

        assert_eq!(
            store.save_connected_account(duplicate.clone()),
            Err(SocialStoreError::DuplicateConnectedAccount)
        );

        duplicate.owner = other_owner();
        assert_eq!(store.save_connected_account(duplicate), Ok(()));
    }

    #[test]
    fn disable_connected_account_checks_owner_and_marks_disabled() {
        let mut store = InMemorySocialStore::default();
        let account = connected_account("acct_1");
        store
            .save_connected_account(account)
            .unwrap_or_else(|err| panic!("save connected account: {err}"));

        assert_eq!(
            store.disable_connected_account("acct_1", &other_owner(), 200),
            Err(SocialStoreError::OwnerMismatch)
        );

        let disabled = store
            .disable_connected_account("acct_1", &owner(), 200)
            .unwrap_or_else(|err| panic!("disable connected account: {err}"));

        assert_eq!(disabled.status, ConnectedAccountStatus::Disabled);
        assert_eq!(disabled.updated_at, 200);
        assert_eq!(store.connected_account("acct_1"), Ok(disabled));
    }

    #[test]
    fn oauth_status_update_persists_callback_result() {
        let mut store = InMemorySocialStore::default();
        store
            .save_oauth_connection(oauth_connection("oauth_1"))
            .unwrap_or_else(|err| panic!("save oauth connection: {err}"));

        let updated = store
            .update_oauth_status("oauth_1", OAuthConnectionStatus::Completed)
            .unwrap_or_else(|err| panic!("update oauth status: {err}"));

        assert_eq!(updated.status, OAuthConnectionStatus::Completed);
        assert_eq!(store.oauth_connection("oauth_1"), Ok(updated));
    }
}
