# Social Persistence And OAuth Routes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Phase 3A of the server-backed social publishing pipeline: durable social account persistence plus route-shaped OAuth/account service methods for YouTube, TikTok, and Instagram.

**Architecture:** Keep implementation inside `montage-social` because this repository does not yet have a dedicated web server crate. Add a storage boundary, a SQLite-backed implementation using the workspace `rusqlite` dependency, and a framework-neutral service layer whose methods map directly to the planned HTTP APIs. This phase does not add live provider HTTP, upload execution, queue workers, desktop UI, or an Axum/HTTP server.

**Tech Stack:** Rust 2024 workspace crate, `serde`, `serde_json`, `thiserror`, `rusqlite`, deterministic unit tests, no live network calls in CI.

---

## Scope

This plan implements the persistence and account-route dependency that must exist before publish targeting can be reliable:

- Persist OAuth connection sessions.
- Persist connected accounts.
- Persist encrypted token secret records.
- List providers and connected accounts without returning token material.
- Start OAuth and store the short-lived connection.
- Complete OAuth using mocked provider/token/profile inputs and store the account plus token secret.
- Disconnect an account by owner and mark it disabled.

Do not implement upload adapters, scheduled queue claiming, live provider HTTP calls, frontend UI, or the full publish-job worker in this plan.

## File Structure

- Modify `crates/social/Cargo.toml`: add `rusqlite` workspace dependency.
- Modify `crates/social/src/lib.rs`: expose new modules.
- Create `crates/social/src/store.rs`: storage trait, in-memory test store, and store errors.
- Create `crates/social/src/sqlite_store.rs`: SQLite schema and `SqliteSocialStore` implementation.
- Create `crates/social/src/account_service.rs`: framework-neutral service methods matching provider/account/OAuth routes.

## Task 1: Storage Boundary And In-Memory Store

**Files:**
- Create: `crates/social/src/store.rs`
- Modify: `crates/social/src/lib.rs`

- [ ] **Step 1: Write failing store tests**

Create `crates/social/src/store.rs` with these tests first:

```rust
use crate::model::{ConnectedAccount, OwnerRef};
use crate::oauth::OAuthConnection;
use crate::token::TokenSecret;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SocialStoreError {
    #[error("record not found")]
    NotFound,
    #[error("record belongs to a different owner")]
    OwnerMismatch,
    #[error("duplicate connected account")]
    DuplicateConnectedAccount,
    #[error("storage error: {0}")]
    Storage(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountEligibility, AccountKind, ConnectedAccountStatus, Provider, ProviderCapabilities,
    };
    use crate::oauth::OAuthConnectionStatus;

    fn account(id: &str, owner: OwnerRef) -> ConnectedAccount {
        ConnectedAccount {
            id: id.into(),
            owner,
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            display_name: "Montage".into(),
            handle: Some("@montage".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            capabilities: ProviderCapabilities {
                native_scheduling: true,
                queue_scheduling: true,
                upload_video: true,
                upload_thumbnail: true,
                public_posting: true,
                requires_user_consent: false,
            },
            eligibility: AccountEligibility::eligible(),
            last_verified_at: Some(100),
            created_at: 100,
            updated_at: 100,
        }
    }

    fn oauth(id: &str) -> OAuthConnection {
        OAuthConnection::start(
            id,
            OwnerRef::User("user_1".into()),
            Provider::YouTube,
            "state-secret",
            vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            "/campaigns/campaign_1".into(),
            100,
            200,
        )
    }

    fn token(account_id: &str) -> TokenSecret {
        TokenSecret {
            connected_account_id: account_id.into(),
            encrypted_access_token: "encrypted-access".into(),
            encrypted_refresh_token: Some("encrypted-refresh".into()),
            access_token_expires_at: Some(3_700),
            refresh_token_expires_at: None,
            token_version: 1,
            kms_key_id: "test-key".into(),
            last_refreshed_at: Some(100),
        }
    }

    #[test]
    fn in_memory_store_persists_oauth_accounts_and_tokens() {
        let mut store = InMemorySocialStore::default();
        let connection = oauth("oauth_1");
        store.save_oauth_connection(connection.clone()).unwrap_or_else(|err| {
            panic!("save oauth connection: {err}");
        });
        assert_eq!(store.oauth_connection("oauth_1"), Ok(connection));

        let account = account("acct_1", OwnerRef::User("user_1".into()));
        store.save_connected_account(account.clone()).unwrap_or_else(|err| {
            panic!("save connected account: {err}");
        });
        store.save_token_secret(token("acct_1")).unwrap_or_else(|err| {
            panic!("save token secret: {err}");
        });

        assert_eq!(
            store.connected_accounts_for_owner(&OwnerRef::User("user_1".into())),
            Ok(vec![account])
        );
        assert_eq!(
            store.token_secret_for_account("acct_1")
                .unwrap_or_else(|err| panic!("load token secret: {err}"))
                .encrypted_access_token,
            "encrypted-access"
        );
    }

    #[test]
    fn in_memory_store_rejects_duplicate_provider_account_for_owner() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(account("acct_1", OwnerRef::User("user_1".into())))
            .unwrap_or_else(|err| panic!("save first account: {err}"));

        assert_eq!(
            store.save_connected_account(account("acct_2", OwnerRef::User("user_1".into()))),
            Err(SocialStoreError::DuplicateConnectedAccount)
        );
    }

    #[test]
    fn disable_connected_account_checks_owner() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(account("acct_1", OwnerRef::User("user_1".into())))
            .unwrap_or_else(|err| panic!("save account: {err}"));

        assert_eq!(
            store.disable_connected_account("acct_1", &OwnerRef::User("user_2".into()), 300),
            Err(SocialStoreError::OwnerMismatch)
        );

        let disabled = store
            .disable_connected_account("acct_1", &OwnerRef::User("user_1".into()), 300)
            .unwrap_or_else(|err| panic!("disable account: {err}"));
        assert_eq!(disabled.status, ConnectedAccountStatus::Disabled);
        assert_eq!(disabled.updated_at, 300);
    }

    #[test]
    fn update_oauth_status_persists_callback_result() {
        let mut store = InMemorySocialStore::default();
        store
            .save_oauth_connection(oauth("oauth_1"))
            .unwrap_or_else(|err| panic!("save oauth connection: {err}"));

        let connection = store
            .update_oauth_status("oauth_1", OAuthConnectionStatus::Completed)
            .unwrap_or_else(|err| panic!("update oauth status: {err}"));
        assert_eq!(connection.status, OAuthConnectionStatus::Completed);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p montage-social store::tests
```

Expected: FAIL with unresolved `InMemorySocialStore` and store methods.

- [ ] **Step 3: Implement the store trait and in-memory store**

Add this implementation above the tests in `crates/social/src/store.rs`:

```rust
use crate::model::{ConnectedAccount, ConnectedAccountStatus, OwnerRef};
use crate::oauth::{OAuthConnection, OAuthConnectionStatus};
use crate::token::TokenSecret;
use std::collections::BTreeMap;

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

    fn save_connected_account(
        &mut self,
        account: ConnectedAccount,
    ) -> Result<(), SocialStoreError>;

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
}

#[derive(Clone, Debug, Default)]
pub struct InMemorySocialStore {
    oauth_connections: BTreeMap<String, OAuthConnection>,
    connected_accounts: BTreeMap<String, ConnectedAccount>,
    token_secrets: BTreeMap<String, TokenSecret>,
}

impl SocialStore for InMemorySocialStore {
    fn save_oauth_connection(
        &mut self,
        connection: OAuthConnection,
    ) -> Result<(), SocialStoreError> {
        self.oauth_connections.insert(connection.id.clone(), connection);
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
            existing.owner == account.owner
                && existing.provider == account.provider
                && existing.provider_account_id == account.provider_account_id
                && existing.id != account.id
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
            .filter(|account| &account.owner == owner)
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
        if &account.owner != owner {
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
}
```

Update `crates/social/src/lib.rs`:

```rust
pub mod store;
```

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test -p montage-social store::tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/lib.rs crates/social/src/store.rs
git commit -m "feat(social): add social store boundary"
```

## Task 2: SQLite Social Store

**Files:**
- Create: `crates/social/src/sqlite_store.rs`
- Modify: `crates/social/src/lib.rs`
- Modify: `crates/social/Cargo.toml`

- [ ] **Step 1: Write failing SQLite store tests**

Create `crates/social/src/sqlite_store.rs` with tests that prove schema creation, round-trip persistence, duplicate owner/provider/account rejection, and token secrecy:

```rust
use crate::store::{SocialStore, SocialStoreError};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef,
        Provider, ProviderCapabilities,
    };
    use crate::oauth::OAuthConnection;
    use crate::token::TokenSecret;

    fn account(id: &str, owner: OwnerRef) -> ConnectedAccount {
        ConnectedAccount {
            id: id.into(),
            owner,
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            display_name: "Montage".into(),
            handle: Some("@montage".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            capabilities: ProviderCapabilities {
                native_scheduling: true,
                queue_scheduling: true,
                upload_video: true,
                upload_thumbnail: true,
                public_posting: true,
                requires_user_consent: false,
            },
            eligibility: AccountEligibility::eligible(),
            last_verified_at: Some(100),
            created_at: 100,
            updated_at: 100,
        }
    }

    fn oauth(id: &str) -> OAuthConnection {
        OAuthConnection::start(
            id,
            OwnerRef::User("user_1".into()),
            Provider::YouTube,
            "state-secret",
            vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            "/campaigns/campaign_1".into(),
            100,
            200,
        )
    }

    fn token(account_id: &str) -> TokenSecret {
        TokenSecret {
            connected_account_id: account_id.into(),
            encrypted_access_token: "encrypted-access".into(),
            encrypted_refresh_token: Some("encrypted-refresh".into()),
            access_token_expires_at: Some(3_700),
            refresh_token_expires_at: None,
            token_version: 1,
            kms_key_id: "test-key".into(),
            last_refreshed_at: Some(100),
        }
    }

    #[test]
    fn sqlite_store_round_trips_social_records() {
        let mut store = SqliteSocialStore::in_memory().unwrap_or_else(|err| {
            panic!("create sqlite store: {err}");
        });
        store.save_oauth_connection(oauth("oauth_1")).unwrap_or_else(|err| {
            panic!("save oauth: {err}");
        });
        store
            .save_connected_account(account("acct_1", OwnerRef::User("user_1".into())))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        store
            .save_token_secret(token("acct_1"))
            .unwrap_or_else(|err| panic!("save token: {err}"));

        assert_eq!(
            store
                .oauth_connection("oauth_1")
                .unwrap_or_else(|err| panic!("load oauth: {err}"))
                .id,
            "oauth_1"
        );
        let accounts = store
            .connected_accounts_for_owner(&OwnerRef::User("user_1".into()))
            .unwrap_or_else(|err| panic!("list accounts: {err}"));
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].display_name, "Montage");
        assert_eq!(
            store
                .token_secret_for_account("acct_1")
                .unwrap_or_else(|err| panic!("load token: {err}"))
                .encrypted_refresh_token,
            Some("encrypted-refresh".into())
        );
    }

    #[test]
    fn sqlite_store_rejects_duplicate_provider_account_for_owner() {
        let mut store = SqliteSocialStore::in_memory().unwrap_or_else(|err| {
            panic!("create sqlite store: {err}");
        });
        store
            .save_connected_account(account("acct_1", OwnerRef::User("user_1".into())))
            .unwrap_or_else(|err| panic!("save first: {err}"));

        assert_eq!(
            store.save_connected_account(account("acct_2", OwnerRef::User("user_1".into()))),
            Err(SocialStoreError::DuplicateConnectedAccount)
        );
    }

    #[test]
    fn sqlite_account_listing_does_not_include_token_material() {
        let mut store = SqliteSocialStore::in_memory().unwrap_or_else(|err| {
            panic!("create sqlite store: {err}");
        });
        store
            .save_connected_account(account("acct_1", OwnerRef::User("user_1".into())))
            .unwrap_or_else(|err| panic!("save account: {err}"));
        store
            .save_token_secret(token("acct_1"))
            .unwrap_or_else(|err| panic!("save token: {err}"));

        let accounts = store
            .connected_accounts_for_owner(&OwnerRef::User("user_1".into()))
            .unwrap_or_else(|err| panic!("list accounts: {err}"));
        let json = serde_json::to_string(&accounts)
            .unwrap_or_else(|err| panic!("serialize accounts: {err}"));
        assert!(!json.contains("encrypted-access"));
        assert!(!json.contains("encrypted-refresh"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p montage-social sqlite_store::tests
```

Expected: FAIL with unresolved `SqliteSocialStore`.

- [ ] **Step 3: Add dependency and module export**

Update `crates/social/Cargo.toml`:

```toml
rusqlite = { workspace = true }
```

Update `crates/social/src/lib.rs`:

```rust
pub mod sqlite_store;
```

- [ ] **Step 4: Implement schema and store**

Implement `SqliteSocialStore` with:

```rust
use crate::model::{
    ConnectedAccount, ConnectedAccountStatus, OwnerRef,
};
use crate::oauth::{OAuthConnection, OAuthConnectionStatus};
use crate::store::{SocialStore, SocialStoreError};
use crate::token::TokenSecret;
use rusqlite::{params, Connection, OptionalExtension};

pub struct SqliteSocialStore {
    connection: Connection,
}

impl SqliteSocialStore {
    pub fn in_memory() -> Result<Self, SocialStoreError> {
        let connection = Connection::open_in_memory()
            .map_err(|err| SocialStoreError::Storage(err.to_string()))?;
        let store = Self { connection };
        store.create_schema()?;
        Ok(store)
    }

    pub fn create_schema(&self) -> Result<(), SocialStoreError> {
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
                    ON connected_accounts(owner_json, provider, provider_account_id);
                CREATE TABLE IF NOT EXISTS oauth_token_secrets (
                    connected_account_id TEXT PRIMARY KEY,
                    payload_json TEXT NOT NULL
                );
                "#,
            )
            .map_err(|err| SocialStoreError::Storage(err.to_string()))
    }
}
```

Serialize full records as JSON payloads for this phase, while also storing indexed columns needed for uniqueness and lookup. Map SQLite unique constraint errors to `SocialStoreError::DuplicateConnectedAccount`, missing rows to `NotFound`, and serde/rusqlite failures to `Storage`.

- [ ] **Step 5: Run test to verify it passes**

Run:

```bash
cargo test -p montage-social sqlite_store::tests
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/social/Cargo.toml crates/social/src/lib.rs crates/social/src/sqlite_store.rs
git commit -m "feat(social): add sqlite social store"
```

## Task 3: Account Service Route Contracts

**Files:**
- Create: `crates/social/src/account_service.rs`
- Modify: `crates/social/src/lib.rs`

- [ ] **Step 1: Write failing service tests**

Create `crates/social/src/account_service.rs` with tests for route-shaped behavior:

```rust
use crate::model::{ConnectedAccount, OwnerRef, Provider};
use crate::oauth_url::{OAuthAuthorizeRequest, OAuthProviderConfig};
use crate::store::{SocialStore, SocialStoreError};
use crate::token::{LocalTokenKeyProvider, TokenSecret};
use crate::token_bundle::ProviderTokenBundle;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompleteOAuthInput {
    pub oauth_connection_id: String,
    pub raw_state: String,
    pub connected_account: ConnectedAccount,
    pub token_bundle: ProviderTokenBundle,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub now: i64,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AccountServiceError {
    #[error("store error: {0}")]
    Store(#[from] SocialStoreError),
    #[error("oauth callback validation failed")]
    InvalidOAuthCallback,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountEligibility, AccountKind, ConnectedAccountStatus, ProviderCapabilities,
    };
    use crate::store::InMemorySocialStore;
    use crate::token::TestKeyProvider;

    fn config() -> OAuthProviderConfig {
        OAuthProviderConfig {
            client_id: "client_123".into(),
            redirect_uri: "https://app.montage.test/social/oauth/callback".into(),
        }
    }

    fn account() -> ConnectedAccount {
        ConnectedAccount {
            id: "acct_1".into(),
            owner: OwnerRef::User("user_1".into()),
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            display_name: "Montage".into(),
            handle: Some("@montage".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            capabilities: ProviderCapabilities {
                native_scheduling: true,
                queue_scheduling: true,
                upload_video: true,
                upload_thumbnail: true,
                public_posting: true,
                requires_user_consent: false,
            },
            eligibility: AccountEligibility::eligible(),
            last_verified_at: Some(150),
            created_at: 150,
            updated_at: 150,
        }
    }

    fn bundle() -> ProviderTokenBundle {
        ProviderTokenBundle {
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            access_token_expires_at: 3_750,
            refresh_token_expires_at: None,
        }
    }

    #[test]
    fn start_oauth_stores_connection_and_returns_authorize_url() {
        let mut store = InMemorySocialStore::default();
        let request = SocialAccountService::start_oauth(
            &mut store,
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::YouTube,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            200,
        )
        .unwrap_or_else(|err| panic!("start oauth: {err}"));

        assert!(request.authorization_url.contains("client_id=client_123"));
        assert!(store.oauth_connection("oauth_1").is_ok());
    }

    #[test]
    fn complete_oauth_persists_account_and_token_without_exposing_token() {
        let mut store = InMemorySocialStore::default();
        SocialAccountService::start_oauth(
            &mut store,
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::YouTube,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            200,
        )
        .unwrap_or_else(|err| panic!("start oauth: {err}"));

        let completed = SocialAccountService::complete_oauth(
            &mut store,
            &TestKeyProvider::new("test-key", "local-key"),
            CompleteOAuthInput {
                oauth_connection_id: "oauth_1".into(),
                raw_state: "state-secret".into(),
                connected_account: account(),
                token_bundle: bundle(),
                access_token: "access-secret".into(),
                refresh_token: Some("refresh-secret".into()),
                now: 150,
            },
        )
        .unwrap_or_else(|err| panic!("complete oauth: {err}"));

        assert_eq!(completed.id, "acct_1");
        let json = serde_json::to_string(&completed)
            .unwrap_or_else(|err| panic!("serialize account: {err}"));
        assert!(!json.contains("access-secret"));
        assert!(!json.contains("refresh-secret"));
        assert!(store.token_secret_for_account("acct_1").is_ok());
    }

    #[test]
    fn list_accounts_returns_only_owner_accounts() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(account())
            .unwrap_or_else(|err| panic!("save account: {err}"));

        assert_eq!(
            SocialAccountService::list_accounts(&store, &OwnerRef::User("user_1".into()))
                .unwrap_or_else(|err| panic!("list accounts: {err}"))
                .len(),
            1
        );
        assert_eq!(
            SocialAccountService::list_accounts(&store, &OwnerRef::User("user_2".into()))
                .unwrap_or_else(|err| panic!("list accounts: {err}"))
                .len(),
            0
        );
    }

    #[test]
    fn disconnect_account_checks_owner() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(account())
            .unwrap_or_else(|err| panic!("save account: {err}"));

        assert_eq!(
            SocialAccountService::disconnect_account(
                &mut store,
                "acct_1",
                &OwnerRef::User("user_2".into()),
                300,
            ),
            Err(AccountServiceError::Store(SocialStoreError::OwnerMismatch))
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p montage-social account_service::tests
```

Expected: FAIL with unresolved `SocialAccountService`.

- [ ] **Step 3: Implement service methods**

Add this implementation above tests:

```rust
use crate::oauth::OAuthConnectionStatus;
use crate::oauth_url::begin_provider_oauth;

pub struct SocialAccountService;

impl SocialAccountService {
    #[allow(clippy::too_many_arguments)]
    pub fn start_oauth(
        store: &mut impl SocialStore,
        id: impl Into<String>,
        owner: OwnerRef,
        provider: Provider,
        config: &OAuthProviderConfig,
        raw_state: &str,
        return_to: String,
        created_at: i64,
        expires_at: i64,
    ) -> Result<OAuthAuthorizeRequest, AccountServiceError> {
        let request = begin_provider_oauth(
            id,
            owner,
            provider,
            config,
            raw_state,
            return_to,
            created_at,
            expires_at,
        );
        store.save_oauth_connection(request.connection.clone())?;
        Ok(request)
    }

    pub fn complete_oauth(
        store: &mut impl SocialStore,
        key_provider: &impl LocalTokenKeyProvider,
        input: CompleteOAuthInput,
    ) -> Result<ConnectedAccount, AccountServiceError> {
        let connection = store.oauth_connection(&input.oauth_connection_id)?;
        connection
            .validate_callback(&input.raw_state, input.now)
            .map_err(|_err| AccountServiceError::InvalidOAuthCallback)?;

        let mut account = input.connected_account;
        account.scopes = input.token_bundle.scopes;
        account.last_verified_at = Some(input.now);
        account.updated_at = input.now;

        let mut secret = TokenSecret::encrypt(
            account.id.clone(),
            &input.access_token,
            input.refresh_token.as_deref(),
            key_provider,
            input.now,
        )
        .map_err(|err| SocialStoreError::Storage(err.to_string()))?;
        secret.access_token_expires_at = Some(input.token_bundle.access_token_expires_at);
        secret.refresh_token_expires_at = input.token_bundle.refresh_token_expires_at;

        store.save_connected_account(account.clone())?;
        store.save_token_secret(secret)?;
        store.update_oauth_status(&input.oauth_connection_id, OAuthConnectionStatus::Completed)?;
        Ok(account)
    }

    pub fn list_accounts(
        store: &impl SocialStore,
        owner: &OwnerRef,
    ) -> Result<Vec<ConnectedAccount>, AccountServiceError> {
        Ok(store.connected_accounts_for_owner(owner)?)
    }

    pub fn disconnect_account(
        store: &mut impl SocialStore,
        id: &str,
        owner: &OwnerRef,
        now: i64,
    ) -> Result<ConnectedAccount, AccountServiceError> {
        Ok(store.disable_connected_account(id, owner, now)?)
    }
}
```

Update `crates/social/src/lib.rs`:

```rust
pub mod account_service;
```

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cargo test -p montage-social account_service::tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/social/src/lib.rs crates/social/src/account_service.rs
git commit -m "feat(social): add oauth account service"
```

## Task 4: Phase 3 Verification

**Files:**
- Verify all changed files.

- [ ] **Step 1: Run social crate tests**

Run:

```bash
cargo test -p montage-social
```

Expected: PASS.

- [ ] **Step 2: Run social crate clippy**

Run:

```bash
cargo clippy -p montage-social --all-targets -- -D warnings
```

Expected: PASS.

- [ ] **Step 3: Run formatting check**

Run:

```bash
cargo fmt --all -- --check
```

Expected: PASS. Existing stable-toolchain warnings about `imports_granularity = Item` are acceptable only if the command exits 0.

- [ ] **Step 4: Run whitespace diff check**

Run:

```bash
git diff --check
```

Expected: PASS with no output.

- [ ] **Step 5: Confirm branch status**

Run:

```bash
git status --short --branch
```

Expected: clean worktree on `codex/clip-campaign-engine`.

## Self-Review

- Spec coverage: This plan covers persistence for OAuth connections, connected accounts, and token secrets plus account-route service methods. It intentionally does not cover publish targeting, upload adapters, queue workers, live provider HTTP, or UI.
- Placeholder scan: No placeholder markers or unspecified test steps remain.
- Type consistency: Later tasks use types defined by earlier tasks or existing Phase 1/2 modules.
