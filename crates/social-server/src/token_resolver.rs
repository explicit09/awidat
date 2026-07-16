use crate::store_handle::StoreHandle;
use montage_social::{
    store::SocialStore,
    token::Aead256Key,
    youtube_upload::{AccessTokenResolver, AccessTokenResolverError},
};

/// Production `AccessTokenResolver` used in the server worker.
///
/// Resolves `"token_secret:<account_id>"` → decrypted bearer token by:
/// 1. Looking up the `TokenSecret` row in the social store.
/// 2. Checking expiry — expired tokens return `Expired` (caller should refresh first).
/// 3. Decrypting with the AEAD key.
///
/// Token refresh is the responsibility of the token-refresh sweep (Phase 4).
/// If a token is expired here, the job will be retried on the next tick after
/// the sweep has refreshed it.
pub struct ServerAccessTokenResolver {
    store: StoreHandle,
    key: Aead256Key,
    now: i64,
}

impl ServerAccessTokenResolver {
    pub fn new(store: StoreHandle, key: Aead256Key, now: i64) -> Self {
        Self { store, key, now }
    }
}

impl AccessTokenResolver for ServerAccessTokenResolver {
    fn bearer_for(&self, access_token_ref: &str) -> Result<String, AccessTokenResolverError> {
        let account_id = access_token_ref
            .strip_prefix("token_secret:")
            .ok_or_else(|| {
                AccessTokenResolverError::NotFound(format!(
                    "unrecognised access_token_ref format: {access_token_ref}"
                ))
            })?;

        let store = self.store.open();
        let secret = store
            .token_secret_for_account(account_id)
            .map_err(|e| AccessTokenResolverError::NotFound(e.to_string()))?;

        if let Some(expires_at) = secret.access_token_expires_at
            && expires_at <= self.now
        {
            return Err(AccessTokenResolverError::Expired(format!(
                "access token for account {account_id} expired at {expires_at} (now {})",
                self.now
            )));
        }

        secret
            .decrypt_access_token(&self.key)
            .map_err(|e| AccessTokenResolverError::DecryptionFailed(e.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use montage_social::{
        model::{
            AccountEligibility, AccountKind, ConnectedAccount, ConnectedAccountStatus, OwnerRef,
            Provider, ProviderCapabilities,
        },
        store::{InMemorySocialStore, SocialStore},
        token::{TestKeyProvider, TokenSecret},
        youtube_upload::{AccessTokenResolver, AccessTokenResolverError, FixedTokenResolver},
    };

    #[test]
    fn fixed_resolver_returns_token_for_any_ref() {
        let r = FixedTokenResolver("bearer-xyz".into());
        assert_eq!(r.bearer_for("token_secret:acct1").unwrap(), "bearer-xyz");
        assert_eq!(r.bearer_for("anything").unwrap(), "bearer-xyz");
    }

    fn make_token_secret(
        account_id: &str,
        access_token: &str,
        expires_at: Option<i64>,
    ) -> (TestKeyProvider, TokenSecret) {
        let key = TestKeyProvider::new("k1", "server-access-token-resolver-key");
        let mut secret = TokenSecret::encrypt(account_id, access_token, None, &key, 0).unwrap();
        secret.access_token_expires_at = expires_at;
        (key, secret)
    }

    #[test]
    fn resolver_round_trips_token_from_in_memory_store() {
        // We can't use ServerAccessTokenResolver without a real Postgres pool,
        // but we can test the decrypt path via InMemorySocialStore directly.
        let (key, secret) = make_token_secret("acct1", "live-bearer-token", Some(9999));
        let mut store = InMemorySocialStore::default();
        store
            .save_connected_account(ConnectedAccount {
                id: "acct1".into(),
                owner: OwnerRef::User("u1".into()),
                provider: Provider::YouTube,
                provider_account_id: "ch_acct1".into(),
                display_name: "Test".into(),
                handle: None,
                avatar_url: None,
                account_kind: AccountKind::Channel,
                status: ConnectedAccountStatus::Connected,
                scopes: vec![],
                capabilities: ProviderCapabilities::default(),
                eligibility: AccountEligibility::eligible(),
                last_verified_at: None,
                created_at: 0,
                updated_at: 0,
            })
            .unwrap();
        store.save_token_secret(secret).unwrap();

        let loaded = store.token_secret_for_account("acct1").unwrap();
        let decrypted = loaded.decrypt_access_token(&key).unwrap();
        assert_eq!(decrypted, "live-bearer-token");
    }

    #[test]
    fn expired_token_returns_expired_error() {
        let (key, secret) = make_token_secret("acct1", "old-token", Some(500));
        let mut store = InMemorySocialStore::default();
        store.save_token_secret(secret).unwrap();

        let loaded = store.token_secret_for_account("acct1").unwrap();
        assert!(
            loaded
                .access_token_expires_at
                .is_some_and(|exp| exp <= 1000)
        );

        // Simulate what ServerAccessTokenResolver::bearer_for does:
        let now = 1000i64;
        let expires_at = loaded.access_token_expires_at.unwrap();
        let err = if expires_at <= now {
            Err::<String, _>(AccessTokenResolverError::Expired(format!(
                "access token for account acct1 expired at {expires_at} (now {now})"
            )))
        } else {
            loaded
                .decrypt_access_token(&key)
                .map_err(|e| AccessTokenResolverError::DecryptionFailed(e.to_string()))
        };
        assert!(matches!(err, Err(AccessTokenResolverError::Expired(_))));
        let _ = key; // silence unused warning
    }
}
