use crate::model::{
    AccountPublishDefaults, CampaignVariantTarget, ConnectedAccount, ConnectedAccountStatus,
    OwnerRef, PublishJob, PublishJobActorType, PublishJobEvent, PublishJobEventType,
    PublishJobStatus, ValidationState, WorkspaceMemberRole,
};
use crate::oauth::{OAuthConnection, OAuthConnectionStatus};
use crate::store::{SocialStore, SocialStoreError};
use crate::token::TokenSecret;
use rusqlite::{Connection, Error as RusqliteError, ErrorCode, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub struct SqliteSocialStore {
    connection: Connection,
}

impl SqliteSocialStore {
    pub fn new_in_memory() -> Result<Self, SocialStoreError> {
        let connection = Connection::open_in_memory().map_err(storage_error)?;
        let store = Self { connection };
        store.create_schema()?;
        Ok(store)
    }

    /// Opens (creating if absent) a file-backed store and ensures the schema.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, SocialStoreError> {
        let connection = Connection::open(path).map_err(storage_error)?;
        let store = Self { connection };
        store.create_schema()?;
        Ok(store)
    }

    fn create_schema(&self) -> Result<(), SocialStoreError> {
        self.connection
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS oauth_connections (
                    id TEXT PRIMARY KEY,
                    payload_json TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS connected_accounts (
                    id TEXT PRIMARY KEY,
                    owner_json TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    provider_account_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                );

                CREATE UNIQUE INDEX IF NOT EXISTS connected_accounts_owner_provider_account
                    ON connected_accounts (owner_json, provider, provider_account_id);

                CREATE TABLE IF NOT EXISTS oauth_token_secrets (
                    connected_account_id TEXT PRIMARY KEY,
                    payload_json TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS campaign_variant_targets (
                    id TEXT PRIMARY KEY,
                    campaign_id TEXT NOT NULL,
                    variant_id TEXT NOT NULL,
                    connected_account_id TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    validation_state TEXT NOT NULL,
                    scheduled_for INTEGER NOT NULL,
                    payload_json TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                );

                CREATE INDEX IF NOT EXISTS campaign_variant_targets_variant
                    ON campaign_variant_targets (campaign_id, variant_id);

                CREATE TABLE IF NOT EXISTS publish_jobs (
                    id TEXT PRIMARY KEY,
                    campaign_id TEXT NOT NULL,
                    variant_id TEXT NOT NULL,
                    connected_account_id TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    idempotency_key TEXT NOT NULL,
                    scheduled_for INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                );

                CREATE UNIQUE INDEX IF NOT EXISTS publish_jobs_idempotency_key
                    ON publish_jobs (idempotency_key);

                CREATE INDEX IF NOT EXISTS publish_jobs_due
                    ON publish_jobs (status, scheduled_for, id);

                CREATE TABLE IF NOT EXISTS publish_job_events (
                    id TEXT PRIMARY KEY,
                    publish_job_id TEXT NOT NULL,
                    event_type TEXT NOT NULL,
                    actor_type TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                );

                CREATE INDEX IF NOT EXISTS publish_job_events_job
                    ON publish_job_events (publish_job_id, created_at, id);

                CREATE TABLE IF NOT EXISTS account_publish_defaults (
                    connected_account_id TEXT PRIMARY KEY,
                    payload_json TEXT NOT NULL,
                    updated_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS workspace_member_roles (
                    workspace_id TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    PRIMARY KEY (workspace_id, user_id)
                );
                "#,
            )
            .map_err(storage_error)
    }
}

impl SocialStore for SqliteSocialStore {
    fn save_oauth_connection(
        &mut self,
        connection: OAuthConnection,
    ) -> Result<(), SocialStoreError> {
        let payload_json = oauth_connection_to_json(&connection)?;
        self.connection
            .execute(
                r#"
                INSERT INTO oauth_connections (id, payload_json)
                VALUES (?1, ?2)
                ON CONFLICT(id) DO UPDATE SET payload_json = excluded.payload_json
                "#,
                params![connection.id, payload_json],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn oauth_connection(&self, id: &str) -> Result<OAuthConnection, SocialStoreError> {
        self.connection
            .query_row(
                "SELECT payload_json FROM oauth_connections WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or(SocialStoreError::NotFound)
            .and_then(|payload_json| oauth_connection_from_json(&payload_json))
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
        let result = self.connection.execute(
            r#"
            INSERT INTO connected_accounts (
                id,
                owner_json,
                provider,
                provider_account_id,
                status,
                payload_json,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(id) DO UPDATE SET
                owner_json = excluded.owner_json,
                provider = excluded.provider,
                provider_account_id = excluded.provider_account_id,
                status = excluded.status,
                payload_json = excluded.payload_json,
                updated_at = excluded.updated_at
            "#,
            params![
                account.id,
                owner_json,
                account.provider.as_str(),
                account.provider_account_id,
                connected_account_status_as_str(&account.status),
                payload_json,
                account.updated_at,
            ],
        );

        match result {
            Ok(_) => Ok(()),
            Err(err) if is_constraint_error(&err) => {
                Err(SocialStoreError::DuplicateConnectedAccount)
            }
            Err(err) => Err(storage_error(err)),
        }
    }

    fn connected_account(&self, id: &str) -> Result<ConnectedAccount, SocialStoreError> {
        self.connection
            .query_row(
                "SELECT payload_json FROM connected_accounts WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or(SocialStoreError::NotFound)
            .and_then(|payload_json| from_json(&payload_json))
    }

    fn connected_accounts_for_owner(
        &self,
        owner: &OwnerRef,
    ) -> Result<Vec<ConnectedAccount>, SocialStoreError> {
        let owner_json = to_json(owner)?;
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT payload_json
                FROM connected_accounts
                WHERE owner_json = ?1
                ORDER BY id
                "#,
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![owner_json], |row| row.get::<_, String>(0))
            .map_err(storage_error)?;
        let mut accounts = Vec::new();
        for row in rows {
            let payload_json = row.map_err(storage_error)?;
            accounts.push(from_json(&payload_json)?);
        }
        Ok(accounts)
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
        self.connection
            .execute(
                r#"
                INSERT INTO oauth_token_secrets (connected_account_id, payload_json)
                VALUES (?1, ?2)
                ON CONFLICT(connected_account_id) DO UPDATE SET
                    payload_json = excluded.payload_json
                "#,
                params![secret.connected_account_id, payload_json],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn token_secret_for_account(&self, account_id: &str) -> Result<TokenSecret, SocialStoreError> {
        self.connection
            .query_row(
                "SELECT payload_json FROM oauth_token_secrets WHERE connected_account_id = ?1",
                params![account_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or(SocialStoreError::NotFound)
            .and_then(|payload_json| from_json(&payload_json))
    }

    fn save_campaign_variant_target(
        &mut self,
        target: CampaignVariantTarget,
    ) -> Result<(), SocialStoreError> {
        let payload_json = to_json(&target)?;
        self.connection
            .execute(
                r#"
                INSERT INTO campaign_variant_targets (
                    id,
                    campaign_id,
                    variant_id,
                    connected_account_id,
                    provider,
                    validation_state,
                    scheduled_for,
                    payload_json,
                    updated_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT(id) DO UPDATE SET
                    campaign_id = excluded.campaign_id,
                    variant_id = excluded.variant_id,
                    connected_account_id = excluded.connected_account_id,
                    provider = excluded.provider,
                    validation_state = excluded.validation_state,
                    scheduled_for = excluded.scheduled_for,
                    payload_json = excluded.payload_json,
                    updated_at = excluded.updated_at
                "#,
                params![
                    target.id,
                    target.campaign_id,
                    target.variant_id,
                    target.connected_account_id,
                    target.provider.as_str(),
                    validation_state_as_str(&target.validation_state),
                    target.scheduled_for,
                    payload_json,
                    target.updated_at,
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn campaign_variant_target(&self, id: &str) -> Result<CampaignVariantTarget, SocialStoreError> {
        self.connection
            .query_row(
                "SELECT payload_json FROM campaign_variant_targets WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or(SocialStoreError::NotFound)
            .and_then(|payload_json| from_json(&payload_json))
    }

    fn save_publish_job(&mut self, job: PublishJob) -> Result<(), SocialStoreError> {
        let payload_json = to_json(&job)?;
        let result = self.connection.execute(
            r#"
            INSERT INTO publish_jobs (
                id,
                campaign_id,
                variant_id,
                connected_account_id,
                provider,
                idempotency_key,
                scheduled_for,
                status,
                payload_json,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO UPDATE SET
                campaign_id = excluded.campaign_id,
                variant_id = excluded.variant_id,
                connected_account_id = excluded.connected_account_id,
                provider = excluded.provider,
                idempotency_key = excluded.idempotency_key,
                scheduled_for = excluded.scheduled_for,
                status = excluded.status,
                payload_json = excluded.payload_json,
                updated_at = excluded.updated_at
            "#,
            params![
                job.id,
                job.campaign_id,
                job.variant_id,
                job.connected_account_id,
                job.provider.as_str(),
                job.idempotency_key,
                job.scheduled_for,
                publish_job_status_as_str(&job.status),
                payload_json,
                job.updated_at,
            ],
        );

        match result {
            Ok(_) => Ok(()),
            Err(err) if is_constraint_error(&err) => Err(SocialStoreError::Storage(
                "duplicate publish job idempotency key".into(),
            )),
            Err(err) => Err(storage_error(err)),
        }
    }

    fn publish_job(&self, id: &str) -> Result<PublishJob, SocialStoreError> {
        self.connection
            .query_row(
                "SELECT payload_json FROM publish_jobs WHERE id = ?1",
                params![id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or(SocialStoreError::NotFound)
            .and_then(|payload_json| from_json(&payload_json))
    }

    fn publish_jobs_for_account(
        &self,
        connected_account_id: &str,
    ) -> Result<Vec<PublishJob>, SocialStoreError> {
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT payload_json
                FROM publish_jobs
                WHERE connected_account_id = ?1
                ORDER BY scheduled_for, id
                "#,
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![connected_account_id], |row| row.get::<_, String>(0))
            .map_err(storage_error)?;
        let mut jobs = Vec::new();
        for row in rows {
            let payload_json = row.map_err(storage_error)?;
            jobs.push(from_json(&payload_json)?);
        }
        Ok(jobs)
    }

    fn claim_due_publish_jobs(
        &mut self,
        now: i64,
        limit: usize,
    ) -> Result<Vec<PublishJob>, SocialStoreError> {
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT payload_json
                FROM publish_jobs
                WHERE status = ?1 AND scheduled_for <= ?2
                ORDER BY scheduled_for, id
                LIMIT ?3
                "#,
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![
                    publish_job_status_as_str(&PublishJobStatus::Scheduled),
                    now,
                    limit as i64
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(storage_error)?;
        let mut due_jobs = Vec::new();
        for row in rows {
            let payload_json = row.map_err(storage_error)?;
            due_jobs.push(from_json::<PublishJob>(&payload_json)?);
        }
        drop(statement);

        let mut claimed = Vec::with_capacity(due_jobs.len());
        for job in due_jobs {
            let updated = job.claim_for_upload(now);
            self.save_publish_job(updated.clone())?;
            claimed.push(updated);
        }

        Ok(claimed)
    }

    fn processing_publish_jobs(&self, limit: usize) -> Result<Vec<PublishJob>, SocialStoreError> {
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT payload_json
                FROM publish_jobs
                WHERE status = ?1
                ORDER BY updated_at, id
                LIMIT ?2
                "#,
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(
                params![
                    publish_job_status_as_str(&PublishJobStatus::Processing),
                    limit as i64
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(storage_error)?;
        let mut jobs = Vec::new();
        for row in rows {
            jobs.push(from_json::<PublishJob>(&row.map_err(storage_error)?)?);
        }
        Ok(jobs)
    }

    fn append_publish_job_event(&mut self, event: PublishJobEvent) -> Result<(), SocialStoreError> {
        let payload_json = to_json(&event)?;
        self.connection
            .execute(
                r#"
                INSERT INTO publish_job_events (
                    id,
                    publish_job_id,
                    event_type,
                    actor_type,
                    payload_json,
                    created_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    event.id,
                    event.publish_job_id,
                    publish_job_event_type_as_str(&event.event_type),
                    publish_job_actor_type_as_str(&event.actor_type),
                    payload_json,
                    event.created_at,
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn publish_job_events(
        &self,
        publish_job_id: &str,
    ) -> Result<Vec<PublishJobEvent>, SocialStoreError> {
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT payload_json
                FROM publish_job_events
                WHERE publish_job_id = ?1
                ORDER BY created_at, id
                "#,
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![publish_job_id], |row| row.get::<_, String>(0))
            .map_err(storage_error)?;
        let mut events = Vec::new();
        for row in rows {
            let payload_json = row.map_err(storage_error)?;
            events.push(from_json(&payload_json)?);
        }
        Ok(events)
    }

    fn save_account_publish_defaults(
        &mut self,
        defaults: AccountPublishDefaults,
    ) -> Result<(), SocialStoreError> {
        let payload_json = to_json(&defaults)?;
        self.connection
            .execute(
                r#"
                INSERT INTO account_publish_defaults (
                    connected_account_id,
                    payload_json,
                    updated_at
                )
                VALUES (?1, ?2, ?3)
                ON CONFLICT(connected_account_id) DO UPDATE SET
                    payload_json = excluded.payload_json,
                    updated_at = excluded.updated_at
                "#,
                params![
                    defaults.connected_account_id,
                    payload_json,
                    defaults.updated_at
                ],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn account_publish_defaults(
        &self,
        connected_account_id: &str,
    ) -> Result<AccountPublishDefaults, SocialStoreError> {
        self.connection
            .query_row(
                "SELECT payload_json FROM account_publish_defaults WHERE connected_account_id = ?1",
                params![connected_account_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?
            .ok_or(SocialStoreError::NotFound)
            .and_then(|payload_json| from_json(&payload_json))
    }

    fn save_workspace_member_role(
        &mut self,
        role: WorkspaceMemberRole,
    ) -> Result<(), SocialStoreError> {
        let payload_json = to_json(&role)?;
        self.connection
            .execute(
                r#"
                INSERT INTO workspace_member_roles (
                    workspace_id,
                    user_id,
                    payload_json
                )
                VALUES (?1, ?2, ?3)
                ON CONFLICT(workspace_id, user_id) DO UPDATE SET
                    payload_json = excluded.payload_json
                "#,
                params![role.workspace_id, role.user_id, payload_json],
            )
            .map_err(storage_error)?;
        Ok(())
    }

    fn workspace_member_roles(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<WorkspaceMemberRole>, SocialStoreError> {
        let mut statement = self
            .connection
            .prepare(
                r#"
                SELECT payload_json
                FROM workspace_member_roles
                WHERE workspace_id = ?1
                ORDER BY user_id
                "#,
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![workspace_id], |row| row.get::<_, String>(0))
            .map_err(storage_error)?;
        let mut roles = Vec::new();
        for row in rows {
            let payload_json = row.map_err(storage_error)?;
            roles.push(from_json(&payload_json)?);
        }
        Ok(roles)
    }

    fn token_secrets_due_refresh(
        &self,
        deadline: i64,
    ) -> Result<Vec<crate::token::TokenSecret>, SocialStoreError> {
        let mut stmt = self
            .connection
            .prepare(
                r#"
                SELECT payload_json
                FROM oauth_token_secrets
                WHERE access_token_expires_at IS NOT NULL
                  AND access_token_expires_at <= ?1
                "#,
            )
            .map_err(storage_error)?;
        let rows = stmt
            .query_map(params![deadline], |row| row.get::<_, String>(0))
            .map_err(storage_error)?;
        let mut secrets = Vec::new();
        for row in rows {
            secrets.push(from_json(&row.map_err(storage_error)?)?);
        }
        Ok(secrets)
    }

    fn youtube_upload_quota_today(&self, now: i64) -> Result<usize, SocialStoreError> {
        let day = now / 86_400;
        let count: i64 = self
            .connection
            .query_row(
                "SELECT COALESCE(count,0) FROM provider_upload_quota
                 WHERE project_key='default' AND day_epoch=?1",
                rusqlite::params![day],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(count as usize)
    }

    fn increment_youtube_quota(&mut self, now: i64) -> Result<(), SocialStoreError> {
        let day = now / 86_400;
        self.connection
            .execute(
                "INSERT INTO provider_upload_quota (project_key, day_epoch, count)
                 VALUES ('default', ?1, 1)
                 ON CONFLICT(project_key, day_epoch) DO UPDATE SET count = count + 1",
                rusqlite::params![day],
            )
            .map_err(storage_error)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct OAuthConnectionPayload {
    id: String,
    owner: OwnerRef,
    provider: crate::model::Provider,
    state_hash: String,
    requested_scopes: Vec<String>,
    return_to: String,
    status: String,
    created_at: i64,
    expires_at: i64,
}

fn oauth_connection_to_json(connection: &OAuthConnection) -> Result<String, SocialStoreError> {
    let payload = OAuthConnectionPayload {
        id: connection.id.clone(),
        owner: connection.owner.clone(),
        provider: connection.provider.clone(),
        state_hash: connection.state_hash.clone(),
        requested_scopes: connection.requested_scopes.clone(),
        return_to: connection.return_to.clone(),
        status: oauth_status_as_str(&connection.status).to_string(),
        created_at: connection.created_at,
        expires_at: connection.expires_at,
    };
    to_json(&payload)
}

fn oauth_connection_from_json(payload_json: &str) -> Result<OAuthConnection, SocialStoreError> {
    let payload: OAuthConnectionPayload = from_json(payload_json)?;
    Ok(OAuthConnection {
        id: payload.id,
        owner: payload.owner,
        provider: payload.provider,
        state_hash: payload.state_hash,
        requested_scopes: payload.requested_scopes,
        return_to: payload.return_to,
        status: oauth_status_from_str(&payload.status)?,
        created_at: payload.created_at,
        expires_at: payload.expires_at,
    })
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

fn publish_job_event_type_as_str(event_type: &PublishJobEventType) -> &'static str {
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

fn publish_job_actor_type_as_str(actor_type: &PublishJobActorType) -> &'static str {
    match actor_type {
        PublishJobActorType::User => "user",
        PublishJobActorType::System => "system",
        PublishJobActorType::Worker => "worker",
        PublishJobActorType::Provider => "provider",
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<String, SocialStoreError> {
    serde_json::to_string(value).map_err(|err| SocialStoreError::Storage(err.to_string()))
}

fn from_json<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T, SocialStoreError> {
    serde_json::from_str(value).map_err(|err| SocialStoreError::Storage(err.to_string()))
}

fn storage_error(err: RusqliteError) -> SocialStoreError {
    SocialStoreError::Storage(err.to_string())
}

fn is_constraint_error(err: &RusqliteError) -> bool {
    matches!(
        err,
        RusqliteError::SqliteFailure(error, _)
            if error.code == ErrorCode::ConstraintViolation
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountEligibility, AccountKind, AccountPublishDefaults, CampaignVariantTarget,
        ConnectedAccount, ConnectedAccountStatus, OwnerRef, Provider, ProviderCapabilities,
        PublishJob, PublishJobActorType, PublishJobEvent, PublishJobEventType, PublishJobStatus,
        TeamRole, ValidationState, WorkspaceMemberRole,
    };
    use crate::oauth::{OAuthConnection, OAuthConnectionStatus};
    use crate::store::{SocialStore, SocialStoreError};
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
            serde_json::json!({"privacy": "private"}),
            2_000,
            1_000,
        )
        .mark_validation(ValidationState::Valid, 1_100)
    }

    fn publish_job(id: &str, scheduled_for: i64) -> PublishJob {
        PublishJob::new(
            id,
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            format!("render://{id}"),
            scheduled_for,
            "user_1",
        )
        .schedule(1_000)
    }

    fn publish_job_event(id: &str, publish_job_id: &str, created_at: i64) -> PublishJobEvent {
        PublishJobEvent::new(
            id,
            publish_job_id,
            PublishJobEventType::Scheduled,
            PublishJobActorType::User,
            "scheduled",
            serde_json::json!({}),
            created_at,
        )
    }

    #[test]
    fn sqlite_round_trips_oauth_account_and_token_records() {
        let mut store = SqliteSocialStore::new_in_memory()
            .unwrap_or_else(|err| panic!("create sqlite social store: {err}"));
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
    fn sqlite_persists_targets_jobs_and_events() {
        let mut store = SqliteSocialStore::new_in_memory()
            .unwrap_or_else(|err| panic!("create sqlite social store: {err}"));
        let target = campaign_variant_target("target_1");
        let job = publish_job("job_1", 2_000);
        let event_1 = publish_job_event("event_1", "job_1", 1_000);
        let event_2 = publish_job_event("event_2", "job_1", 1_100);

        store
            .save_campaign_variant_target(target.clone())
            .unwrap_or_else(|err| panic!("save target: {err}"));
        store
            .save_publish_job(job.clone())
            .unwrap_or_else(|err| panic!("save publish job: {err}"));
        store
            .append_publish_job_event(event_2.clone())
            .unwrap_or_else(|err| panic!("append second event: {err}"));
        store
            .append_publish_job_event(event_1.clone())
            .unwrap_or_else(|err| panic!("append first event: {err}"));

        assert_eq!(store.campaign_variant_target("target_1"), Ok(target));
        assert_eq!(
            store.campaign_variant_target("missing_target"),
            Err(SocialStoreError::NotFound)
        );
        assert_eq!(store.publish_job("job_1"), Ok(job));
        assert_eq!(
            store.publish_job("missing_job"),
            Err(SocialStoreError::NotFound)
        );
        assert_eq!(
            store.publish_job_events("job_1"),
            Ok(vec![event_1.clone(), event_2.clone()])
        );
        let duplicate_event = PublishJobEvent::new(
            "event_1",
            "job_1",
            PublishJobEventType::RetryQueued,
            PublishJobActorType::Worker,
            "retry queued",
            serde_json::json!({"attempt": 2}),
            1_200,
        );
        assert!(store.append_publish_job_event(duplicate_event).is_err());
        assert_eq!(
            store.publish_job_events("job_1"),
            Ok(vec![event_1, event_2])
        );

        let duplicate_idempotency_key = PublishJob::new(
            "job_2",
            "campaign_1",
            "variant_1",
            "acct_1",
            Provider::YouTube,
            "render://job_1",
            2_000,
            "user_1",
        )
        .schedule(1_000);
        assert_eq!(
            store.save_publish_job(duplicate_idempotency_key),
            Err(SocialStoreError::Storage(
                "duplicate publish job idempotency key".into()
            ))
        );
    }

    #[test]
    fn sqlite_persists_account_defaults_roles_and_account_jobs() {
        let mut store = SqliteSocialStore::new_in_memory()
            .unwrap_or_else(|err| panic!("create sqlite social store: {err}"));
        let account = connected_account("acct_1");
        let defaults = AccountPublishDefaults {
            connected_account_id: "acct_1".into(),
            default_privacy: Some("unlisted".into()),
            default_tags: vec!["awidat".into()],
            title_prefix: Some("Awidat: ".into()),
            description_suffix: Some("Built with Awidat.".into()),
            updated_at: 2_000,
        };
        let role = WorkspaceMemberRole::new("workspace_1", "user_1", TeamRole::Publisher);
        let job = publish_job("job_1", 2_000);

        store
            .save_connected_account(account)
            .unwrap_or_else(|err| panic!("save account: {err}"));
        store
            .save_account_publish_defaults(defaults.clone())
            .unwrap_or_else(|err| panic!("save defaults: {err}"));
        store
            .save_workspace_member_role(role.clone())
            .unwrap_or_else(|err| panic!("save role: {err}"));
        store
            .save_publish_job(job.clone())
            .unwrap_or_else(|err| panic!("save job: {err}"));

        assert_eq!(store.account_publish_defaults("acct_1"), Ok(defaults));
        assert_eq!(store.workspace_member_roles("workspace_1"), Ok(vec![role]));
        assert_eq!(store.publish_jobs_for_account("acct_1"), Ok(vec![job]));
    }

    #[test]
    fn sqlite_claims_due_scheduled_jobs_once() {
        let mut store = SqliteSocialStore::new_in_memory()
            .unwrap_or_else(|err| panic!("create sqlite social store: {err}"));
        let later_by_schedule_first_by_id = publish_job("job_a", 1_900);
        let earlier_by_schedule_second_by_id = publish_job("job_b", 1_000);
        let future = publish_job("job_c", 2_600);

        for job in [
            later_by_schedule_first_by_id.clone(),
            earlier_by_schedule_second_by_id,
            future.clone(),
        ] {
            store
                .save_publish_job(job)
                .unwrap_or_else(|err| panic!("save publish job: {err}"));
        }

        let claimed = store
            .claim_due_publish_jobs(2_500, 1)
            .unwrap_or_else(|err| panic!("claim due jobs: {err}"));
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, "job_b");
        assert_eq!(claimed[0].status, PublishJobStatus::Uploading);
        assert_eq!(claimed[0].attempt_count, 1);
        assert_eq!(claimed[0].updated_at, 2_500);
        assert_eq!(store.publish_job("job_b"), Ok(claimed[0].clone()));
        assert_eq!(
            store.publish_job("job_a"),
            Ok(later_by_schedule_first_by_id)
        );
        assert_eq!(store.publish_job("job_c"), Ok(future));

        let claimed_again = store
            .claim_due_publish_jobs(2_500, 10)
            .unwrap_or_else(|err| panic!("claim due jobs again: {err}"));
        assert_eq!(claimed_again.len(), 1);
        assert_eq!(claimed_again[0].id, "job_a");

        let claimed_third = store
            .claim_due_publish_jobs(2_500, 10)
            .unwrap_or_else(|err| panic!("claim due jobs third time: {err}"));
        assert!(claimed_third.is_empty());
    }

    #[test]
    fn sqlite_rejects_duplicate_provider_account_for_same_owner() {
        let mut store = SqliteSocialStore::new_in_memory()
            .unwrap_or_else(|err| panic!("create sqlite social store: {err}"));
        let account = connected_account("acct_1");
        let mut duplicate = connected_account("acct_2");
        duplicate.provider_account_id = "channel_2".into();

        store
            .save_connected_account(account.clone())
            .unwrap_or_else(|err| panic!("save connected account: {err}"));
        store
            .save_connected_account(duplicate.clone())
            .unwrap_or_else(|err| panic!("save second connected account: {err}"));

        duplicate.provider_account_id = account.provider_account_id;

        assert_eq!(
            store.save_connected_account(duplicate.clone()),
            Err(SocialStoreError::DuplicateConnectedAccount)
        );

        duplicate.owner = other_owner();
        assert_eq!(store.save_connected_account(duplicate), Ok(()));
    }

    #[test]
    fn sqlite_account_listing_does_not_include_encrypted_token_material() {
        let mut store = SqliteSocialStore::new_in_memory()
            .unwrap_or_else(|err| panic!("create sqlite social store: {err}"));
        let account = connected_account("acct_1");
        let secret = token_secret("acct_1");

        store
            .save_connected_account(account)
            .unwrap_or_else(|err| panic!("save connected account: {err}"));
        store
            .save_token_secret(secret.clone())
            .unwrap_or_else(|err| panic!("save token secret: {err}"));

        let listed = store
            .connected_accounts_for_owner(&owner())
            .unwrap_or_else(|err| panic!("list accounts: {err}"));
        let listing_json = serde_json::to_string(&listed)
            .unwrap_or_else(|err| panic!("serialize listed accounts: {err}"));

        assert!(!listing_json.contains(&secret.encrypted_access_token));
        if let Some(refresh_token) = secret.encrypted_refresh_token {
            assert!(!listing_json.contains(&refresh_token));
        }
    }

    #[test]
    fn sqlite_disable_checks_owner_and_marks_account_disabled() {
        let mut store = SqliteSocialStore::new_in_memory()
            .unwrap_or_else(|err| panic!("create sqlite social store: {err}"));
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
    fn sqlite_oauth_status_update_persists_callback_result() {
        let mut store = SqliteSocialStore::new_in_memory()
            .unwrap_or_else(|err| panic!("create sqlite social store: {err}"));
        store
            .save_oauth_connection(oauth_connection("oauth_1"))
            .unwrap_or_else(|err| panic!("save oauth connection: {err}"));

        let updated = store
            .update_oauth_status("oauth_1", OAuthConnectionStatus::Completed)
            .unwrap_or_else(|err| panic!("update oauth status: {err}"));

        assert_eq!(updated.status, OAuthConnectionStatus::Completed);
        assert_eq!(store.oauth_connection("oauth_1"), Ok(updated));
    }

    #[test]
    fn open_persists_account_across_reopen() {
        let dir = std::env::temp_dir().join(format!("awidat_social_open_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("create temp dir: {err}"));
        let path = dir.join("social.sqlite");
        let _ = std::fs::remove_file(&path);

        {
            let mut store =
                SqliteSocialStore::open(&path).unwrap_or_else(|err| panic!("open store: {err}"));
            store
                .save_connected_account(connected_account("acct_open"))
                .unwrap_or_else(|err| panic!("save account: {err}"));
        }

        let reopened =
            SqliteSocialStore::open(&path).unwrap_or_else(|err| panic!("reopen store: {err}"));
        let loaded = reopened
            .connected_account("acct_open")
            .unwrap_or_else(|err| panic!("load account: {err}"));
        assert_eq!(loaded.id, "acct_open");

        let _ = std::fs::remove_file(&path);
    }
}
