use crate::{
    model::Provider,
    oauth_exchange::{OAuthExchangeError, OAuthTokenExchange, TokenExchangeInput},
    store::{SocialStore, SocialStoreError},
    token::{LocalTokenKeyProvider, TOKEN_VERSION, TokenSecret},
    token_bundle::{OAuthTokenResponse, ProviderTokenBundle},
};

#[derive(Debug)]
pub enum RefreshError {
    Store(SocialStoreError),
    NoRefreshToken,
    Exchange(OAuthExchangeError),
    TokenBundle(String),
    Encrypt(String),
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(e) => write!(f, "store error: {e}"),
            Self::NoRefreshToken => write!(f, "account has no refresh token"),
            Self::Exchange(e) => write!(f, "token exchange: {e}"),
            Self::TokenBundle(e) => write!(f, "token bundle: {e}"),
            Self::Encrypt(e) => write!(f, "encrypt: {e}"),
        }
    }
}

pub struct TokenRefreshService;

impl TokenRefreshService {
    /// Ensure the access token for `account_id` is fresh.
    ///
    /// If the token expires before `deadline` (Unix seconds), refresh it via the
    /// provider's OAuth endpoint and write the updated `TokenSecret` back to the
    /// store.  Returns `Ok(true)` if a refresh was performed, `Ok(false)` if the
    /// token was still fresh.
    pub async fn ensure_fresh_access_token<E: OAuthTokenExchange>(
        store: &mut impl SocialStore,
        key_provider: &impl LocalTokenKeyProvider,
        exchange: &E,
        account_id: &str,
        provider: Provider,
        redirect_uri: String,
        deadline: i64,
        now: i64,
    ) -> Result<bool, RefreshError> {
        let secret = store
            .token_secret_for_account(account_id)
            .map_err(RefreshError::Store)?;

        let expires_at = secret.access_token_expires_at.unwrap_or(i64::MAX);
        if expires_at > deadline {
            return Ok(false);
        }

        let refresh_token = secret
            .decrypt_refresh_token(key_provider)
            .map_err(|e| RefreshError::Encrypt(e.to_string()))?
            .ok_or(RefreshError::NoRefreshToken)?;

        let output = exchange
            .exchange(TokenExchangeInput {
                provider: provider.clone(),
                code: refresh_token,
                redirect_uri,
            })
            .await
            .map_err(RefreshError::Exchange)?;

        let bundle = ProviderTokenBundle::from_oauth_response(
            provider,
            OAuthTokenResponse {
                provider_account_id: output.token_response.provider_account_id,
                scopes: output.token_response.scopes,
                expires_in: output.token_response.expires_in,
                refresh_expires_in: output.token_response.refresh_expires_in,
            },
            now,
        )
        .map_err(|e| RefreshError::TokenBundle(format!("{e:?}")))?;

        let new_refresh = output.refresh_token.as_deref().or(Some(""));
        let mut new_secret = TokenSecret::encrypt(
            account_id,
            &output.access_token,
            new_refresh.filter(|s| !s.is_empty()),
            key_provider,
            now,
        )
        .map_err(|e| RefreshError::Encrypt(e.to_string()))?;
        new_secret.access_token_expires_at = Some(bundle.access_token_expires_at);
        new_secret.refresh_token_expires_at = bundle.refresh_token_expires_at;

        store
            .save_token_secret(new_secret)
            .map_err(RefreshError::Store)?;
        Ok(true)
    }

    /// Sweep all token secrets that expire before `deadline` and refresh them.
    ///
    /// Errors for individual accounts are logged and skipped; the sweep continues.
    /// Returns the number of accounts successfully refreshed.
    pub async fn refresh_due_secrets<E: OAuthTokenExchange>(
        store: &mut impl SocialStore,
        key_provider: &impl LocalTokenKeyProvider,
        exchange: &E,
        provider: Provider,
        redirect_uri: String,
        deadline: i64,
        now: i64,
    ) -> usize {
        let secrets = match store.token_secrets_due_refresh(deadline) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("refresh_due_secrets: failed to query due secrets: {e}");
                return 0;
            }
        };

        let mut refreshed = 0usize;
        for secret in secrets {
            if secret.token_version != TOKEN_VERSION {
                tracing::warn!(
                    account_id = %secret.connected_account_id,
                    version = secret.token_version,
                    "skipping account with unsupported token version"
                );
                continue;
            }
            let account_id = secret.connected_account_id.clone();
            match Self::ensure_fresh_access_token(
                store,
                key_provider,
                exchange,
                &account_id,
                provider.clone(),
                redirect_uri.clone(),
                deadline,
                now,
            )
            .await
            {
                Ok(true) => {
                    refreshed += 1;
                    tracing::info!(account_id, "access token refreshed");
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(account_id, "refresh failed: {e}");
                }
            }
        }
        refreshed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{
            AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef,
            ProviderCapabilities,
        },
        oauth_exchange::{TokenExchangeOutput, tests::MockOAuthExchange},
        store::InMemorySocialStore,
        token::TestKeyProvider,
    };

    fn key() -> TestKeyProvider {
        TestKeyProvider::new("k1", "test-key-for-refresh-service-32b")
    }

    fn account(id: &str) -> ConnectedAccount {
        ConnectedAccount {
            id: id.to_string(),
            owner: OwnerRef::User("user_1".into()),
            provider: Provider::YouTube,
            provider_account_id: format!("channel_{id}"),
            display_name: "Test Channel".into(),
            handle: None,
            avatar_url: None,
            account_kind: AccountKind::Channel,
            status: ConnectedAccountStatus::Connected,
            scopes: vec!["https://www.googleapis.com/auth/youtube.upload".into()],
            capabilities: ProviderCapabilities::default(),
            eligibility: AccountEligibility::eligible(),
            last_verified_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn store_with_secret(
        account_id: &str,
        access_expires_at: i64,
        access_token: &str,
        refresh_token: &str,
    ) -> InMemorySocialStore {
        let k = key();
        let mut secret =
            TokenSecret::encrypt(account_id, access_token, Some(refresh_token), &k, 0).unwrap();
        secret.access_token_expires_at = Some(access_expires_at);
        let mut store = InMemorySocialStore::default();
        store.save_connected_account(account(account_id)).unwrap();
        store.save_token_secret(secret).unwrap();
        store
    }

    #[tokio::test]
    async fn fresh_token_skips_refresh() {
        let mut store = store_with_secret("acct1", 9999, "at-old", "rt-old");
        let exchange = MockOAuthExchange {
            output: Err("should not be called".into()),
        };
        let refreshed = TokenRefreshService::ensure_fresh_access_token(
            &mut store,
            &key(),
            &exchange,
            "acct1",
            Provider::YouTube,
            "https://example.com/cb".into(),
            5000,
            1000,
        )
        .await
        .unwrap();
        assert!(!refreshed);
    }

    #[tokio::test]
    async fn expired_token_triggers_refresh_and_stores_new_secret() {
        let mut store = store_with_secret("acct1", 500, "at-old", "rt-old");
        let exchange = MockOAuthExchange {
            output: Ok(TokenExchangeOutput {
                access_token: "at-new".into(),
                refresh_token: Some("rt-new".into()),
                token_response: OAuthTokenResponse {
                    provider_account_id: "channel_acct1".into(),
                    scopes: "https://www.googleapis.com/auth/youtube.upload".into(),
                    expires_in: 3600,
                    refresh_expires_in: None,
                },
            }),
        };
        let refreshed = TokenRefreshService::ensure_fresh_access_token(
            &mut store,
            &key(),
            &exchange,
            "acct1",
            Provider::YouTube,
            "https://example.com/cb".into(),
            1000,
            1000,
        )
        .await
        .unwrap();
        assert!(refreshed);
        let new_secret = store.token_secret_for_account("acct1").unwrap();
        let k = key();
        assert_eq!(new_secret.decrypt_access_token(&k).unwrap(), "at-new");
        assert_eq!(
            new_secret.decrypt_refresh_token(&k).unwrap().as_deref(),
            Some("rt-new")
        );
        assert_eq!(new_secret.access_token_expires_at, Some(4600));
    }

    #[tokio::test]
    async fn missing_refresh_token_returns_no_refresh_token_error() {
        let k = key();
        let mut secret = TokenSecret::encrypt("acct1", "at", None, &k, 0).unwrap();
        secret.access_token_expires_at = Some(0);
        let mut store = InMemorySocialStore::default();
        store.save_connected_account(account("acct1")).unwrap();
        store.save_token_secret(secret).unwrap();

        let exchange = MockOAuthExchange {
            output: Err("irrelevant".into()),
        };
        let err = TokenRefreshService::ensure_fresh_access_token(
            &mut store,
            &k,
            &exchange,
            "acct1",
            Provider::YouTube,
            "https://example.com/cb".into(),
            1000,
            1000,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, RefreshError::NoRefreshToken));
    }

    #[tokio::test]
    async fn refresh_due_secrets_counts_refreshed() {
        let mut store = store_with_secret("acct1", 100, "at-old", "rt-old");
        store.save_connected_account(account("acct2")).unwrap();
        let k = key();
        let mut s2 = TokenSecret::encrypt("acct2", "at2", Some("rt2"), &k, 0).unwrap();
        s2.access_token_expires_at = Some(100);
        store.save_token_secret(s2).unwrap();

        let exchange = MockOAuthExchange {
            output: Ok(TokenExchangeOutput {
                access_token: "at-refreshed".into(),
                refresh_token: None,
                token_response: OAuthTokenResponse {
                    provider_account_id: "channel_any".into(),
                    scopes: "scope".into(),
                    expires_in: 3600,
                    refresh_expires_in: None,
                },
            }),
        };
        let count = TokenRefreshService::refresh_due_secrets(
            &mut store,
            &k,
            &exchange,
            Provider::YouTube,
            "https://example.com/cb".into(),
            500,
            1000,
        )
        .await;
        assert_eq!(count, 2);
    }
}
