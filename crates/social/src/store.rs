use crate::model::{ConnectedAccount, ConnectedAccountStatus, OwnerRef};
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef,
        Provider, ProviderCapabilities,
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
