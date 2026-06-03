use crate::model::{
    CampaignVariantTarget, ConnectedAccount, ConnectedAccountStatus, OwnerRef, PublishJob,
    PublishJobEvent,
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
        _target: CampaignVariantTarget,
    ) -> Result<(), SocialStoreError> {
        Err(publish_storage_pending_error())
    }

    fn campaign_variant_target(
        &self,
        _id: &str,
    ) -> Result<CampaignVariantTarget, SocialStoreError> {
        Err(publish_storage_pending_error())
    }

    fn save_publish_job(&mut self, _job: PublishJob) -> Result<(), SocialStoreError> {
        Err(publish_storage_pending_error())
    }

    fn publish_job(&self, _id: &str) -> Result<PublishJob, SocialStoreError> {
        Err(publish_storage_pending_error())
    }

    fn claim_due_publish_jobs(
        &mut self,
        _now: i64,
        _limit: usize,
    ) -> Result<Vec<PublishJob>, SocialStoreError> {
        Err(publish_storage_pending_error())
    }

    fn append_publish_job_event(
        &mut self,
        _event: PublishJobEvent,
    ) -> Result<(), SocialStoreError> {
        Err(publish_storage_pending_error())
    }

    fn publish_job_events(
        &self,
        _publish_job_id: &str,
    ) -> Result<Vec<PublishJobEvent>, SocialStoreError> {
        Err(publish_storage_pending_error())
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

fn publish_storage_pending_error() -> SocialStoreError {
    SocialStoreError::Storage("sqlite publish storage is pending Task 3".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef,
        Provider, ProviderCapabilities,
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
}
