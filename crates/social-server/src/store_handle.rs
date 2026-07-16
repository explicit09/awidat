//! Store injection seam.
//!
//! Production holds a Postgres pool and opens a fresh [`PgSocialStore`] per
//! blocking task, exactly as the handlers always have. Tests construct the
//! `InMemory` variant so route-level tests run hermetically (no DB server).
//!
//! `StoreHandle` is the cheap-to-clone handle stored in `AppState`;
//! [`StoreHandle::open`] yields a [`ServerStore`] that implements
//! [`SocialStore`] and is what handler closures use inside `spawn_blocking`.

use montage_social::model::{
    AccountPublishDefaults, CampaignVariantTarget, ConnectedAccount, OwnerRef, PublishJob,
    PublishJobEvent, WorkspaceMemberRole,
};
use montage_social::oauth::{OAuthConnection, OAuthConnectionStatus};
use montage_social::pg_store::PgSocialStore;
use montage_social::store::{InMemorySocialStore, SocialStore, SocialStoreError};
use montage_social::token::TokenSecret;
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use r2d2_postgres::postgres::NoTls;
use std::sync::{Arc, Mutex, PoisonError};

pub type PgPool = Pool<PostgresConnectionManager<NoTls>>;

/// Cheap-to-clone handle to the backing social store. Held by `AppState`.
#[derive(Clone)]
pub enum StoreHandle {
    /// Production: a Postgres pool; every `open()` builds a `PgSocialStore`
    /// over a clone of the pool (the pre-seam per-handler pattern).
    Pg(PgPool),
    /// Tests: a shared in-memory store. All `open()` calls see the same data.
    InMemory(Arc<Mutex<InMemorySocialStore>>),
}

impl StoreHandle {
    /// A fresh, empty in-memory store handle (hermetic tests).
    pub fn in_memory() -> Self {
        Self::InMemory(Arc::new(Mutex::new(InMemorySocialStore::default())))
    }

    /// Wrap an existing shared in-memory store (lets a test pre-seed data and
    /// keep its own reference for assertions).
    pub fn from_shared(store: Arc<Mutex<InMemorySocialStore>>) -> Self {
        Self::InMemory(store)
    }

    /// Open a store for use on the current (blocking) thread.
    pub fn open(&self) -> ServerStore {
        match self {
            StoreHandle::Pg(pool) => ServerStore::Pg(PgSocialStore::new(pool.clone())),
            StoreHandle::InMemory(shared) => ServerStore::InMemory(Arc::clone(shared)),
        }
    }
}

/// A concrete [`SocialStore`] backed by either Postgres or the shared
/// in-memory test store. Handlers use this exactly like they used
/// `PgSocialStore` before the seam.
pub enum ServerStore {
    Pg(PgSocialStore),
    InMemory(Arc<Mutex<InMemorySocialStore>>),
}

/// Run `$body` with `$store` bound to the underlying store. The in-memory arm
/// locks per call — the sync domain layer never holds the guard across awaits
/// (it is only ever used inside `spawn_blocking`).
macro_rules! with_store {
    ($self:expr, $store:ident => $body:expr) => {
        match $self {
            ServerStore::Pg($store) => $body,
            ServerStore::InMemory(shared) => {
                let mut guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
                let $store = &mut *guard;
                $body
            }
        }
    };
}

impl SocialStore for ServerStore {
    fn save_oauth_connection(
        &mut self,
        connection: OAuthConnection,
    ) -> Result<(), SocialStoreError> {
        with_store!(self, store => store.save_oauth_connection(connection))
    }

    fn oauth_connection(&self, id: &str) -> Result<OAuthConnection, SocialStoreError> {
        with_store!(self, store => store.oauth_connection(id))
    }

    fn update_oauth_status(
        &mut self,
        id: &str,
        status: OAuthConnectionStatus,
    ) -> Result<OAuthConnection, SocialStoreError> {
        with_store!(self, store => store.update_oauth_status(id, status))
    }

    fn save_connected_account(
        &mut self,
        account: ConnectedAccount,
    ) -> Result<(), SocialStoreError> {
        with_store!(self, store => store.save_connected_account(account))
    }

    fn connected_account(&self, id: &str) -> Result<ConnectedAccount, SocialStoreError> {
        with_store!(self, store => store.connected_account(id))
    }

    fn connected_accounts_for_owner(
        &self,
        owner: &OwnerRef,
    ) -> Result<Vec<ConnectedAccount>, SocialStoreError> {
        with_store!(self, store => store.connected_accounts_for_owner(owner))
    }

    fn disable_connected_account(
        &mut self,
        id: &str,
        owner: &OwnerRef,
        now: i64,
    ) -> Result<ConnectedAccount, SocialStoreError> {
        with_store!(self, store => store.disable_connected_account(id, owner, now))
    }

    fn save_token_secret(&mut self, secret: TokenSecret) -> Result<(), SocialStoreError> {
        with_store!(self, store => store.save_token_secret(secret))
    }

    fn token_secret_for_account(&self, account_id: &str) -> Result<TokenSecret, SocialStoreError> {
        with_store!(self, store => store.token_secret_for_account(account_id))
    }

    fn save_campaign_variant_target(
        &mut self,
        target: CampaignVariantTarget,
    ) -> Result<(), SocialStoreError> {
        with_store!(self, store => store.save_campaign_variant_target(target))
    }

    fn campaign_variant_target(&self, id: &str) -> Result<CampaignVariantTarget, SocialStoreError> {
        with_store!(self, store => store.campaign_variant_target(id))
    }

    fn save_publish_job(&mut self, job: PublishJob) -> Result<(), SocialStoreError> {
        with_store!(self, store => store.save_publish_job(job))
    }

    fn publish_job(&self, id: &str) -> Result<PublishJob, SocialStoreError> {
        with_store!(self, store => store.publish_job(id))
    }

    fn publish_jobs_for_account(
        &self,
        connected_account_id: &str,
    ) -> Result<Vec<PublishJob>, SocialStoreError> {
        with_store!(self, store => store.publish_jobs_for_account(connected_account_id))
    }

    fn claim_due_publish_jobs(
        &mut self,
        now: i64,
        limit: usize,
    ) -> Result<Vec<PublishJob>, SocialStoreError> {
        with_store!(self, store => store.claim_due_publish_jobs(now, limit))
    }

    // Delegate explicitly (rather than inheriting the trait default) so each
    // backend's own override — e.g. the transactional Postgres claim — is used.
    fn claim_due_publish_job(
        &mut self,
        id: &str,
        now: i64,
    ) -> Result<Option<PublishJob>, SocialStoreError> {
        with_store!(self, store => store.claim_due_publish_job(id, now))
    }

    fn processing_publish_jobs(&self, limit: usize) -> Result<Vec<PublishJob>, SocialStoreError> {
        with_store!(self, store => store.processing_publish_jobs(limit))
    }

    fn append_publish_job_event(&mut self, event: PublishJobEvent) -> Result<(), SocialStoreError> {
        with_store!(self, store => store.append_publish_job_event(event))
    }

    fn publish_job_events(
        &self,
        publish_job_id: &str,
    ) -> Result<Vec<PublishJobEvent>, SocialStoreError> {
        with_store!(self, store => store.publish_job_events(publish_job_id))
    }

    fn save_account_publish_defaults(
        &mut self,
        defaults: AccountPublishDefaults,
    ) -> Result<(), SocialStoreError> {
        with_store!(self, store => store.save_account_publish_defaults(defaults))
    }

    fn account_publish_defaults(
        &self,
        connected_account_id: &str,
    ) -> Result<AccountPublishDefaults, SocialStoreError> {
        with_store!(self, store => store.account_publish_defaults(connected_account_id))
    }

    fn save_workspace_member_role(
        &mut self,
        role: WorkspaceMemberRole,
    ) -> Result<(), SocialStoreError> {
        with_store!(self, store => store.save_workspace_member_role(role))
    }

    fn workspace_member_roles(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<WorkspaceMemberRole>, SocialStoreError> {
        with_store!(self, store => store.workspace_member_roles(workspace_id))
    }

    fn workspace_member_roles_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<WorkspaceMemberRole>, SocialStoreError> {
        with_store!(self, store => store.workspace_member_roles_for_user(user_id))
    }

    fn token_secrets_due_refresh(
        &self,
        deadline: i64,
    ) -> Result<Vec<TokenSecret>, SocialStoreError> {
        with_store!(self, store => store.token_secrets_due_refresh(deadline))
    }

    fn youtube_upload_quota_today(&self, now: i64) -> Result<usize, SocialStoreError> {
        with_store!(self, store => store.youtube_upload_quota_today(now))
    }

    fn increment_youtube_quota(&mut self, now: i64) -> Result<(), SocialStoreError> {
        with_store!(self, store => store.increment_youtube_quota(now))
    }
}
