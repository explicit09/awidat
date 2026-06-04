use crate::model::{
    AccountPublishDefaults, CampaignVariantTarget, ConnectedAccount, ConnectedAccountStatus,
    OwnerRef, Provider, PublishJob, PublishJobActorType, PublishJobEvent, PublishJobEventType,
    PublishJobStatus, ValidationState, WorkspaceMemberRole,
};
use crate::oauth::{OAuthConnection, OAuthConnectionStatus};
use crate::store::{SocialStore, SocialStoreError};
use crate::token::TokenSecret;
use postgres::error::SqlState;
use r2d2::Pool;
use r2d2_postgres::PostgresConnectionManager;
use r2d2_postgres::postgres::NoTls;
use serde::{Deserialize, Serialize};

pub type PgPool = Pool<PostgresConnectionManager<NoTls>>;

pub struct PgSocialStore {
    pool: PgPool,
}

impl PgSocialStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Open a blocking connection pool to `database_url` (Supavisor session-pooler URL).
    /// Returns an error if the pool cannot be established or a test connection fails.
    pub fn connect(database_url: &str) -> Result<Self, SocialStoreError> {
        let manager = PostgresConnectionManager::new(
            database_url
                .parse()
                .map_err(|e: postgres::Error| SocialStoreError::Storage(e.to_string()))?,
            NoTls,
        );
        let pool = Pool::builder()
            .max_size(5)
            .build(manager)
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        Ok(Self { pool })
    }

    /// Apply all SQL migration files in-order from `migrations_dir`.
    ///
    /// Creates a `_migrations` tracking table so each migration runs exactly once.
    /// Suitable for boot-time application from the server binary.
    pub fn apply_migrations(
        &self,
        migrations_dir: &std::path::Path,
    ) -> Result<(), SocialStoreError> {
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;

        client
            .execute(
                "CREATE TABLE IF NOT EXISTS _migrations (
                    filename text PRIMARY KEY,
                    applied_at bigint NOT NULL
                )",
                &[],
            )
            .map_err(pg_error)?;

        let mut entries: Vec<_> = std::fs::read_dir(migrations_dir)
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?
            .filter_map(std::result::Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "sql"))
            .collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);

        for entry in entries {
            let filename = entry.file_name().to_string_lossy().into_owned();
            let row = client
                .query_opt(
                    "SELECT 1 FROM _migrations WHERE filename = $1",
                    &[&filename],
                )
                .map_err(pg_error)?;
            if row.is_some() {
                continue;
            }
            let sql = std::fs::read_to_string(entry.path())
                .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
            client.batch_execute(&sql).map_err(pg_error)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| SocialStoreError::Storage(e.to_string()))?
                .as_secs() as i64;
            client
                .execute(
                    "INSERT INTO _migrations (filename, applied_at) VALUES ($1, $2)
                     ON CONFLICT DO NOTHING",
                    &[&filename, &now],
                )
                .map_err(pg_error)?;
        }
        Ok(())
    }

    fn get(
        &self,
    ) -> Result<r2d2::PooledConnection<PostgresConnectionManager<NoTls>>, SocialStoreError> {
        self.pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))
    }
}

// ── Serde helpers (mirrors sqlite_store.rs) ──────────────────────────────────

fn to_json<T: Serialize>(value: &T) -> Result<String, SocialStoreError> {
    serde_json::to_string(value).map_err(|e| SocialStoreError::Storage(e.to_string()))
}

fn from_json<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T, SocialStoreError> {
    serde_json::from_str(s).map_err(|e| SocialStoreError::Storage(e.to_string()))
}

fn pg_error(e: postgres::Error) -> SocialStoreError {
    SocialStoreError::Storage(e.to_string())
}

// OAuthConnection is a struct without Serialize/Deserialize on OAuthConnectionStatus,
// so we use the same intermediate payload shape as sqlite_store.rs.
#[derive(Serialize, Deserialize)]
struct OAuthConnectionPayload {
    id: String,
    owner: OwnerRef,
    provider: Provider,
    state_hash: String,
    requested_scopes: Vec<String>,
    return_to: String,
    status: String,
    created_at: i64,
    expires_at: i64,
}

fn oauth_status_as_str(status: &OAuthConnectionStatus) -> &'static str {
    match status {
        OAuthConnectionStatus::Started => "started",
        OAuthConnectionStatus::Completed => "completed",
        OAuthConnectionStatus::Failed => "failed",
        OAuthConnectionStatus::Expired => "expired",
    }
}

fn oauth_status_from_str(value: &str) -> Result<OAuthConnectionStatus, SocialStoreError> {
    match value {
        "started" => Ok(OAuthConnectionStatus::Started),
        "completed" => Ok(OAuthConnectionStatus::Completed),
        "failed" => Ok(OAuthConnectionStatus::Failed),
        "expired" => Ok(OAuthConnectionStatus::Expired),
        other => Err(SocialStoreError::Storage(format!(
            "unknown oauth status: {other}"
        ))),
    }
}

fn oauth_connection_to_json(c: &OAuthConnection) -> Result<String, SocialStoreError> {
    to_json(&OAuthConnectionPayload {
        id: c.id.clone(),
        owner: c.owner.clone(),
        provider: c.provider.clone(),
        state_hash: c.state_hash.clone(),
        requested_scopes: c.requested_scopes.clone(),
        return_to: c.return_to.clone(),
        status: oauth_status_as_str(&c.status).to_string(),
        created_at: c.created_at,
        expires_at: c.expires_at,
    })
}

fn oauth_connection_from_json(json: &str) -> Result<OAuthConnection, SocialStoreError> {
    let p: OAuthConnectionPayload = from_json(json)?;
    Ok(OAuthConnection {
        id: p.id,
        owner: p.owner,
        provider: p.provider,
        state_hash: p.state_hash,
        requested_scopes: p.requested_scopes,
        return_to: p.return_to,
        status: oauth_status_from_str(&p.status)?,
        created_at: p.created_at,
        expires_at: p.expires_at,
    })
}

// ── Status string helpers (mirrors sqlite_store.rs) ──────────────────────────

fn connected_account_status_as_str(status: &ConnectedAccountStatus) -> &'static str {
    match status {
        ConnectedAccountStatus::Connected => "connected",
        ConnectedAccountStatus::NeedsReauth => "needs_reauth",
        ConnectedAccountStatus::MissingScope => "missing_scope",
        ConnectedAccountStatus::Ineligible => "ineligible",
        ConnectedAccountStatus::Disabled => "disabled",
        ConnectedAccountStatus::Revoked => "revoked",
    }
}

fn validation_state_as_str(state: &ValidationState) -> &'static str {
    match state {
        ValidationState::Pending => "pending",
        ValidationState::Valid => "valid",
        ValidationState::Invalid => "invalid",
        ValidationState::RequiresAction => "requires_action",
    }
}

fn publish_job_status_as_str(status: &PublishJobStatus) -> &'static str {
    match status {
        PublishJobStatus::Draft => "draft",
        PublishJobStatus::Validated => "validated",
        PublishJobStatus::Scheduled => "scheduled",
        PublishJobStatus::Uploading => "uploading",
        PublishJobStatus::Processing => "processing",
        PublishJobStatus::Published => "published",
        PublishJobStatus::Failed => "failed",
        PublishJobStatus::RequiresAction => "requires_action",
        PublishJobStatus::Cancelled => "cancelled",
    }
}

fn publish_job_event_type_as_str(t: &PublishJobEventType) -> &'static str {
    match t {
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

fn publish_job_actor_type_as_str(t: &PublishJobActorType) -> &'static str {
    match t {
        PublishJobActorType::User => "user",
        PublishJobActorType::System => "system",
        PublishJobActorType::Worker => "worker",
        PublishJobActorType::Provider => "provider",
    }
}

fn is_unique_violation(e: &postgres::Error) -> bool {
    e.code() == Some(&SqlState::UNIQUE_VIOLATION)
}

// ── SocialStore impl ─────────────────────────────────────────────────────────

impl SocialStore for PgSocialStore {
    fn save_oauth_connection(
        &mut self,
        connection: OAuthConnection,
    ) -> Result<(), SocialStoreError> {
        let payload_json = oauth_connection_to_json(&connection)?;
        let mut client = self.get()?;
        client
            .execute(
                "INSERT INTO oauth_connections (id, payload_json) VALUES ($1, $2)
                 ON CONFLICT (id) DO UPDATE SET payload_json = EXCLUDED.payload_json",
                &[&connection.id, &payload_json],
            )
            .map_err(pg_error)?;
        Ok(())
    }

    fn oauth_connection(&self, id: &str) -> Result<OAuthConnection, SocialStoreError> {
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let row = client
            .query_opt(
                "SELECT payload_json FROM oauth_connections WHERE id = $1",
                &[&id],
            )
            .map_err(pg_error)?
            .ok_or(SocialStoreError::NotFound)?;
        let payload_json: String = row.get(0);
        oauth_connection_from_json(&payload_json)
    }

    fn update_oauth_status(
        &mut self,
        id: &str,
        status: OAuthConnectionStatus,
    ) -> Result<OAuthConnection, SocialStoreError> {
        let mut connection = self.oauth_connection(id)?;
        connection.status = status;
        self.save_oauth_connection(connection.clone())?;
        Ok(connection)
    }

    fn save_connected_account(
        &mut self,
        account: ConnectedAccount,
    ) -> Result<(), SocialStoreError> {
        let owner_json = to_json(&account.owner)?;
        let payload_json = to_json(&account)?;
        let mut client = self.get()?;
        let result = client.execute(
            "INSERT INTO connected_accounts
                 (id, owner_json, provider, provider_account_id, status, payload_json, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (id) DO UPDATE SET
                 owner_json          = EXCLUDED.owner_json,
                 provider            = EXCLUDED.provider,
                 provider_account_id = EXCLUDED.provider_account_id,
                 status              = EXCLUDED.status,
                 payload_json        = EXCLUDED.payload_json,
                 updated_at          = EXCLUDED.updated_at",
            &[
                &account.id,
                &owner_json,
                &account.provider.as_str(),
                &account.provider_account_id,
                &connected_account_status_as_str(&account.status),
                &payload_json,
                &account.updated_at,
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(ref e) if is_unique_violation(e) => {
                Err(SocialStoreError::DuplicateConnectedAccount)
            }
            Err(e) => Err(pg_error(e)),
        }
    }

    fn connected_account(&self, id: &str) -> Result<ConnectedAccount, SocialStoreError> {
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let row = client
            .query_opt(
                "SELECT payload_json FROM connected_accounts WHERE id = $1",
                &[&id],
            )
            .map_err(pg_error)?
            .ok_or(SocialStoreError::NotFound)?;
        from_json(row.get(0))
    }

    fn connected_accounts_for_owner(
        &self,
        owner: &OwnerRef,
    ) -> Result<Vec<ConnectedAccount>, SocialStoreError> {
        let owner_json = to_json(owner)?;
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let rows = client
            .query(
                "SELECT payload_json FROM connected_accounts
                 WHERE owner_json = $1
                 ORDER BY id",
                &[&owner_json],
            )
            .map_err(pg_error)?;
        rows.iter().map(|row| from_json(row.get(0))).collect()
    }

    fn disable_connected_account(
        &mut self,
        id: &str,
        owner: &OwnerRef,
        now: i64,
    ) -> Result<ConnectedAccount, SocialStoreError> {
        let mut account = self.connected_account(id)?;
        if account.owner != *owner {
            return Err(SocialStoreError::OwnerMismatch);
        }
        account.status = ConnectedAccountStatus::Disabled;
        account.updated_at = now;
        self.save_connected_account(account.clone())?;
        Ok(account)
    }

    fn save_token_secret(&mut self, secret: TokenSecret) -> Result<(), SocialStoreError> {
        let payload_json = to_json(&secret)?;
        let mut client = self.get()?;
        client
            .execute(
                "INSERT INTO oauth_token_secrets (connected_account_id, payload_json)
                 VALUES ($1, $2)
                 ON CONFLICT (connected_account_id) DO UPDATE SET
                     payload_json = EXCLUDED.payload_json",
                &[&secret.connected_account_id, &payload_json],
            )
            .map_err(pg_error)?;
        Ok(())
    }

    fn token_secret_for_account(&self, account_id: &str) -> Result<TokenSecret, SocialStoreError> {
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let row = client
            .query_opt(
                "SELECT payload_json FROM oauth_token_secrets WHERE connected_account_id = $1",
                &[&account_id],
            )
            .map_err(pg_error)?
            .ok_or(SocialStoreError::NotFound)?;
        from_json(row.get(0))
    }

    fn save_campaign_variant_target(
        &mut self,
        target: CampaignVariantTarget,
    ) -> Result<(), SocialStoreError> {
        let payload_json = to_json(&target)?;
        let mut client = self.get()?;
        client
            .execute(
                "INSERT INTO campaign_variant_targets
                     (id, campaign_id, variant_id, connected_account_id, provider,
                      validation_state, scheduled_for, payload_json, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 ON CONFLICT (id) DO UPDATE SET
                     campaign_id          = EXCLUDED.campaign_id,
                     variant_id           = EXCLUDED.variant_id,
                     connected_account_id = EXCLUDED.connected_account_id,
                     provider             = EXCLUDED.provider,
                     validation_state     = EXCLUDED.validation_state,
                     scheduled_for        = EXCLUDED.scheduled_for,
                     payload_json         = EXCLUDED.payload_json,
                     updated_at           = EXCLUDED.updated_at",
                &[
                    &target.id,
                    &target.campaign_id,
                    &target.variant_id,
                    &target.connected_account_id,
                    &target.provider.as_str(),
                    &validation_state_as_str(&target.validation_state),
                    &target.scheduled_for,
                    &payload_json,
                    &target.updated_at,
                ],
            )
            .map_err(pg_error)?;
        Ok(())
    }

    fn campaign_variant_target(&self, id: &str) -> Result<CampaignVariantTarget, SocialStoreError> {
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let row = client
            .query_opt(
                "SELECT payload_json FROM campaign_variant_targets WHERE id = $1",
                &[&id],
            )
            .map_err(pg_error)?
            .ok_or(SocialStoreError::NotFound)?;
        from_json(row.get(0))
    }

    fn save_publish_job(&mut self, job: PublishJob) -> Result<(), SocialStoreError> {
        let payload_json = to_json(&job)?;
        let mut client = self.get()?;
        let result = client.execute(
            "INSERT INTO publish_jobs
                 (id, campaign_id, variant_id, connected_account_id, provider,
                  idempotency_key, scheduled_for, status, payload_json, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (id) DO UPDATE SET
                 campaign_id          = EXCLUDED.campaign_id,
                 variant_id           = EXCLUDED.variant_id,
                 connected_account_id = EXCLUDED.connected_account_id,
                 provider             = EXCLUDED.provider,
                 idempotency_key      = EXCLUDED.idempotency_key,
                 scheduled_for        = EXCLUDED.scheduled_for,
                 status               = EXCLUDED.status,
                 payload_json         = EXCLUDED.payload_json,
                 updated_at           = EXCLUDED.updated_at",
            &[
                &job.id,
                &job.campaign_id,
                &job.variant_id,
                &job.connected_account_id,
                &job.provider.as_str(),
                &job.idempotency_key,
                &job.scheduled_for,
                &publish_job_status_as_str(&job.status),
                &payload_json,
                &job.updated_at,
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(ref e) if is_unique_violation(e) => Err(SocialStoreError::Storage(
                "duplicate publish job idempotency key".into(),
            )),
            Err(e) => Err(pg_error(e)),
        }
    }

    fn publish_job(&self, id: &str) -> Result<PublishJob, SocialStoreError> {
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let row = client
            .query_opt(
                "SELECT payload_json FROM publish_jobs WHERE id = $1",
                &[&id],
            )
            .map_err(pg_error)?
            .ok_or(SocialStoreError::NotFound)?;
        from_json(row.get(0))
    }

    fn publish_jobs_for_account(
        &self,
        connected_account_id: &str,
    ) -> Result<Vec<PublishJob>, SocialStoreError> {
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let rows = client
            .query(
                "SELECT payload_json FROM publish_jobs
                 WHERE connected_account_id = $1
                 ORDER BY scheduled_for, id",
                &[&connected_account_id],
            )
            .map_err(pg_error)?;
        rows.iter().map(|row| from_json(row.get(0))).collect()
    }

    /// Atomically claim due scheduled jobs using `FOR UPDATE SKIP LOCKED`
    /// so concurrent ticks cannot double-claim the same job (G8).
    fn claim_due_publish_jobs(
        &mut self,
        now: i64,
        limit: usize,
    ) -> Result<Vec<PublishJob>, SocialStoreError> {
        let mut client = self.get()?;
        let mut tx = client.transaction().map_err(pg_error)?;

        let scheduled_str = publish_job_status_as_str(&PublishJobStatus::Scheduled);
        let rows = tx
            .query(
                "SELECT payload_json FROM publish_jobs
                 WHERE status = $1 AND scheduled_for <= $2
                 ORDER BY scheduled_for, id
                 LIMIT $3
                 FOR UPDATE SKIP LOCKED",
                &[&scheduled_str, &now, &(limit as i64)],
            )
            .map_err(pg_error)?;

        let due_jobs: Vec<PublishJob> = rows
            .iter()
            .map(|row| from_json(row.get(0)))
            .collect::<Result<_, _>>()?;

        let mut claimed = Vec::with_capacity(due_jobs.len());
        for job in due_jobs {
            let updated = job.claim_for_upload(now);
            let payload_json = to_json(&updated)?;
            tx.execute(
                "UPDATE publish_jobs SET
                     status       = $1,
                     payload_json = $2,
                     updated_at   = $3
                 WHERE id = $4",
                &[
                    &publish_job_status_as_str(&updated.status),
                    &payload_json,
                    &updated.updated_at,
                    &updated.id,
                ],
            )
            .map_err(pg_error)?;
            claimed.push(updated);
        }
        tx.commit().map_err(pg_error)?;
        Ok(claimed)
    }

    fn processing_publish_jobs(&self, limit: usize) -> Result<Vec<PublishJob>, SocialStoreError> {
        let mut client = self.get()?;
        let processing_str = publish_job_status_as_str(&PublishJobStatus::Processing);
        let rows = client
            .query(
                "SELECT payload_json FROM publish_jobs
                 WHERE status = $1
                 ORDER BY updated_at, id
                 LIMIT $2",
                &[&processing_str, &(limit as i64)],
            )
            .map_err(pg_error)?;
        rows.iter().map(|row| from_json(row.get(0))).collect()
    }

    fn append_publish_job_event(&mut self, event: PublishJobEvent) -> Result<(), SocialStoreError> {
        let payload_json = to_json(&event)?;
        let mut client = self.get()?;
        client
            .execute(
                "INSERT INTO publish_job_events
                     (id, publish_job_id, event_type, actor_type, payload_json, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6)",
                &[
                    &event.id,
                    &event.publish_job_id,
                    &publish_job_event_type_as_str(&event.event_type),
                    &publish_job_actor_type_as_str(&event.actor_type),
                    &payload_json,
                    &event.created_at,
                ],
            )
            .map_err(pg_error)?;
        Ok(())
    }

    fn publish_job_events(
        &self,
        publish_job_id: &str,
    ) -> Result<Vec<PublishJobEvent>, SocialStoreError> {
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let rows = client
            .query(
                "SELECT payload_json FROM publish_job_events
                 WHERE publish_job_id = $1
                 ORDER BY created_at, id",
                &[&publish_job_id],
            )
            .map_err(pg_error)?;
        rows.iter().map(|row| from_json(row.get(0))).collect()
    }

    fn save_account_publish_defaults(
        &mut self,
        defaults: AccountPublishDefaults,
    ) -> Result<(), SocialStoreError> {
        let payload_json = to_json(&defaults)?;
        let mut client = self.get()?;
        client
            .execute(
                "INSERT INTO account_publish_defaults (connected_account_id, payload_json, updated_at)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (connected_account_id) DO UPDATE SET
                     payload_json = EXCLUDED.payload_json,
                     updated_at   = EXCLUDED.updated_at",
                &[
                    &defaults.connected_account_id,
                    &payload_json,
                    &defaults.updated_at,
                ],
            )
            .map_err(pg_error)?;
        Ok(())
    }

    fn account_publish_defaults(
        &self,
        connected_account_id: &str,
    ) -> Result<AccountPublishDefaults, SocialStoreError> {
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let row = client
            .query_opt(
                "SELECT payload_json FROM account_publish_defaults WHERE connected_account_id = $1",
                &[&connected_account_id],
            )
            .map_err(pg_error)?
            .ok_or(SocialStoreError::NotFound)?;
        from_json(row.get(0))
    }

    fn save_workspace_member_role(
        &mut self,
        role: WorkspaceMemberRole,
    ) -> Result<(), SocialStoreError> {
        let payload_json = to_json(&role)?;
        let mut client = self.get()?;
        client
            .execute(
                "INSERT INTO workspace_member_roles (workspace_id, user_id, payload_json)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (workspace_id, user_id) DO UPDATE SET
                     payload_json = EXCLUDED.payload_json",
                &[&role.workspace_id, &role.user_id, &payload_json],
            )
            .map_err(pg_error)?;
        Ok(())
    }

    fn workspace_member_roles(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<WorkspaceMemberRole>, SocialStoreError> {
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let rows = client
            .query(
                "SELECT payload_json FROM workspace_member_roles
                 WHERE workspace_id = $1
                 ORDER BY user_id",
                &[&workspace_id],
            )
            .map_err(pg_error)?;
        rows.iter().map(|row| from_json(row.get(0))).collect()
    }

    fn workspace_member_roles_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<WorkspaceMemberRole>, SocialStoreError> {
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let rows = client
            .query(
                "SELECT payload_json FROM workspace_member_roles
                 WHERE user_id = $1
                 ORDER BY workspace_id",
                &[&user_id],
            )
            .map_err(pg_error)?;
        rows.iter().map(|row| from_json(row.get(0))).collect()
    }

    fn token_secrets_due_refresh(
        &self,
        deadline: i64,
    ) -> Result<Vec<crate::token::TokenSecret>, SocialStoreError> {
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let rows = client
            .query(
                "SELECT payload_json FROM oauth_token_secrets
                 WHERE access_token_expires_at IS NOT NULL
                   AND access_token_expires_at <= $1",
                &[&deadline],
            )
            .map_err(pg_error)?;
        rows.iter().map(|row| from_json(row.get(0))).collect()
    }

    fn youtube_upload_quota_today(&self, now: i64) -> Result<usize, SocialStoreError> {
        let day = now / 86_400;
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        let row = client
            .query_opt(
                "SELECT count FROM provider_upload_quota
                 WHERE project_key = 'default' AND day_epoch = $1",
                &[&day],
            )
            .map_err(pg_error)?;
        Ok(row.map(|r| r.get::<_, i32>(0) as usize).unwrap_or(0))
    }

    fn increment_youtube_quota(&mut self, now: i64) -> Result<(), SocialStoreError> {
        let day = now / 86_400;
        let mut client = self
            .pool
            .get()
            .map_err(|e| SocialStoreError::Storage(e.to_string()))?;
        client
            .execute(
                "INSERT INTO provider_upload_quota (project_key, day_epoch, count)
                 VALUES ('default', $1, 1)
                 ON CONFLICT (project_key, day_epoch) DO UPDATE SET count = provider_upload_quota.count + 1",
                &[&day],
            )
            .map_err(pg_error)?;
        Ok(())
    }
}
