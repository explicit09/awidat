use crate::model::{ConnectedAccount, OwnerRef, Provider};
use crate::oauth::OAuthConnectionStatus;
use crate::oauth_url::{OAuthAuthorizeRequest, OAuthProviderConfig, begin_provider_oauth};
use crate::store::{SocialStore, SocialStoreError};
use crate::token::{LocalTokenKeyProvider, TokenSecret};
use crate::token_bundle::ProviderTokenBundle;
use thiserror::Error;

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
    #[error(transparent)]
    Store(#[from] SocialStoreError),
    #[error("invalid oauth callback")]
    InvalidOAuthCallback,
}

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
            id, owner, provider, config, raw_state, return_to, created_at, expires_at,
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
        if connection
            .validate_callback(&input.raw_state, input.now)
            .is_err()
        {
            return Err(AccountServiceError::InvalidOAuthCallback);
        }

        let mut account = input.connected_account;
        account.scopes = input.token_bundle.scopes;
        account.last_verified_at = Some(input.now);
        account.updated_at = input.now;

        let mut secret = TokenSecret::encrypt(
            &account.id,
            &input.access_token,
            input.refresh_token.as_deref(),
            key_provider,
            input.now,
        )
        .map_err(|err| AccountServiceError::Store(SocialStoreError::Storage(err.to_string())))?;
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
        store
            .connected_accounts_for_owner(owner)
            .map_err(AccountServiceError::Store)
    }

    pub fn disconnect_account(
        store: &mut impl SocialStore,
        id: &str,
        owner: &OwnerRef,
        now: i64,
    ) -> Result<ConnectedAccount, AccountServiceError> {
        store
            .disable_connected_account(id, owner, now)
            .map_err(AccountServiceError::Store)
    }
}

#[cfg(test)]
mod tests {
    use crate::account_service::{AccountServiceError, CompleteOAuthInput, SocialAccountService};
    use crate::model::{
        AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef,
        Provider, ProviderCapabilities,
    };
    use crate::oauth::OAuthConnectionStatus;
    use crate::oauth_url::OAuthProviderConfig;
    use crate::store::{InMemorySocialStore, SocialStore, SocialStoreError};
    use crate::token::TestKeyProvider;
    use crate::token_bundle::ProviderTokenBundle;

    fn owner() -> OwnerRef {
        OwnerRef::User("user_1".into())
    }

    fn other_owner() -> OwnerRef {
        OwnerRef::Workspace("workspace_1".into())
    }

    fn config() -> OAuthProviderConfig {
        OAuthProviderConfig {
            client_id: "client_123".into(),
            redirect_uri: "https://app.awidat.test/social/oauth/callback".into(),
        }
    }

    fn connected_account(id: &str, account_owner: OwnerRef) -> ConnectedAccount {
        ConnectedAccount {
            id: id.into(),
            owner: account_owner,
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            display_name: "Awidat Channel".into(),
            handle: Some("@awidat".into()),
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["old.scope".into()],
            capabilities: ProviderCapabilities::default(),
            eligibility: AccountEligibility::eligible(),
            last_verified_at: None,
            created_at: 100,
            updated_at: 100,
        }
    }

    fn token_bundle() -> ProviderTokenBundle {
        ProviderTokenBundle {
            provider: Provider::YouTube,
            provider_account_id: "channel_1".into(),
            scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            access_token_expires_at: 4_700,
            refresh_token_expires_at: Some(8_700),
        }
    }

    fn complete_input(raw_state: &str) -> CompleteOAuthInput {
        CompleteOAuthInput {
            oauth_connection_id: "oauth_1".into(),
            raw_state: raw_state.into(),
            connected_account: connected_account("acct_1", owner()),
            token_bundle: token_bundle(),
            access_token: "access-secret".into(),
            refresh_token: Some("refresh-secret".into()),
            now: 1_000,
        }
    }

    fn start_oauth(store: &mut InMemorySocialStore) {
        SocialAccountService::start_oauth(
            store,
            "oauth_1",
            owner(),
            Provider::YouTube,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            2_000,
        )
        .unwrap_or_else(|err| panic!("start oauth: {err}"));
    }

    #[test]
    fn start_oauth_stores_connection_and_returns_provider_authorize_url() {
        let mut store = InMemorySocialStore::default();

        let request = SocialAccountService::start_oauth(
            &mut store,
            "oauth_1",
            owner(),
            Provider::YouTube,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            2_000,
        )
        .unwrap_or_else(|err| panic!("start oauth: {err}"));

        let stored = store
            .oauth_connection("oauth_1")
            .unwrap_or_else(|err| panic!("load oauth connection: {err}"));
        assert_eq!(stored, request.connection);
        assert!(
            request
                .authorization_url
                .starts_with("https://accounts.google.com/o/oauth2/v2/auth?")
        );
        assert!(request.authorization_url.contains("state=state-secret"));
    }

    #[test]
    fn complete_oauth_persists_account_and_token_without_exposing_raw_token() {
        let mut store = InMemorySocialStore::default();
        start_oauth(&mut store);
        let key_provider = TestKeyProvider::new("test-key-1", "local-key");

        let account = SocialAccountService::complete_oauth(
            &mut store,
            &key_provider,
            complete_input("state-secret"),
        )
        .unwrap_or_else(|err| panic!("complete oauth: {err}"));

        assert_eq!(account.scopes, token_bundle().scopes);
        assert_eq!(account.last_verified_at, Some(1_000));
        assert_eq!(account.updated_at, 1_000);

        let json = serde_json::to_string(&account)
            .unwrap_or_else(|err| panic!("serialize connected account: {err}"));
        assert!(!json.contains("access-secret"));
        assert!(!json.contains("refresh-secret"));

        let secret = store
            .token_secret_for_account("acct_1")
            .unwrap_or_else(|err| panic!("load token secret: {err}"));
        assert_ne!(secret.encrypted_access_token, "access-secret");
        assert_ne!(
            secret.encrypted_refresh_token.as_deref(),
            Some("refresh-secret")
        );
        assert_eq!(secret.access_token_expires_at, Some(4_700));
        assert_eq!(secret.refresh_token_expires_at, Some(8_700));
        assert_eq!(secret.last_refreshed_at, Some(1_000));

        let connection = store
            .oauth_connection("oauth_1")
            .unwrap_or_else(|err| panic!("load oauth connection: {err}"));
        assert_eq!(connection.status, OAuthConnectionStatus::Completed);
    }

    #[test]
    fn list_accounts_returns_only_owner_accounts() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", owner()))
            .unwrap_or_else(|err| panic!("save owner account: {err}"));
        store
            .save_connected_account(connected_account("acct_2", other_owner()))
            .unwrap_or_else(|err| panic!("save other account: {err}"));

        let accounts = SocialAccountService::list_accounts(&store, &owner())
            .unwrap_or_else(|err| panic!("list accounts: {err}"));

        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, "acct_1");
    }

    #[test]
    fn disconnect_checks_owner_and_maps_owner_mismatch_through_store_error() {
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(connected_account("acct_1", owner()))
            .unwrap_or_else(|err| panic!("save account: {err}"));

        let err = match SocialAccountService::disconnect_account(
            &mut store,
            "acct_1",
            &other_owner(),
            2_000,
        ) {
            Ok(account) => panic!("expected owner mismatch, got account: {account:?}"),
            Err(err) => err,
        };

        assert_eq!(
            err,
            AccountServiceError::Store(SocialStoreError::OwnerMismatch)
        );
    }

    #[test]
    fn invalid_raw_state_returns_invalid_oauth_callback() {
        let mut store = InMemorySocialStore::default();
        start_oauth(&mut store);

        let err = match SocialAccountService::complete_oauth(
            &mut store,
            &TestKeyProvider::new("test-key-1", "local-key"),
            complete_input("wrong-state"),
        ) {
            Ok(account) => panic!("expected invalid callback, got account: {account:?}"),
            Err(err) => err,
        };

        assert_eq!(err, AccountServiceError::InvalidOAuthCallback);
    }
}
