use awidat_social::{
    oauth_exchange::{GoogleOAuthExchange, GoogleOAuthExchangeConfig, OAuthExchangeError},
    token::{Aead256Key, TokenSecret},
    token_refresh::{TokenRefreshError, TokenRefresher},
};

/// Production `TokenRefresher` for YouTube/Google.
///
/// Implements the synchronous domain `TokenRefresher` seam by bridging to the
/// async Google OAuth `refresh_access_token` call via the current tokio runtime
/// handle (`block_on`) — the same sync→async pattern the live upload client
/// uses. The confidential `client_secret` and the refresh token never leave the
/// server: the refresh token is decrypted here, exchanged, and the new access
/// token re-encrypted, all server-side.
pub struct ServerTokenRefresher {
    exchange: GoogleOAuthExchange,
    key: Aead256Key,
}

impl ServerTokenRefresher {
    pub fn new(client_id: String, client_secret: String, key: Aead256Key) -> Self {
        Self {
            exchange: GoogleOAuthExchange::new(GoogleOAuthExchangeConfig {
                client_id,
                client_secret,
            }),
            key,
        }
    }
}

impl TokenRefresher for ServerTokenRefresher {
    fn refresh(
        &self,
        _account_id: &str,
        secret: &TokenSecret,
        now: i64,
    ) -> Result<TokenSecret, TokenRefreshError> {
        // Decrypt the stored refresh token (server-side only).
        let refresh_token = secret
            .decrypt_refresh_token(&self.key)
            .map_err(|e| TokenRefreshError::Transient(format!("decrypt refresh token: {e}")))?
            .ok_or_else(|| {
                TokenRefreshError::InvalidGrant("no refresh token stored".to_string())
            })?;

        // Bridge sync → async: exchange the refresh token for a new access token.
        let refreshed = tokio::runtime::Handle::current()
            .block_on(self.exchange.refresh_access_token(&refresh_token))
            .map_err(|e| match e {
                OAuthExchangeError::InvalidGrant(msg) => TokenRefreshError::InvalidGrant(msg),
                other => TokenRefreshError::Transient(other.to_string()),
            })?;

        // Keep the existing refresh token unless Google rotated it.
        let new_refresh = refreshed.refresh_token.as_deref();
        // Re-encrypt the existing refresh token unchanged unless Google rotated
        // it. We decrypted it above, so re-supply the plaintext.
        let refresh_to_store = new_refresh.unwrap_or(refresh_token.as_str());
        let mut updated = TokenSecret::encrypt(
            &secret.connected_account_id,
            &refreshed.access_token,
            Some(refresh_to_store),
            &self.key,
            now,
        )
        .map_err(|e| TokenRefreshError::Transient(format!("encrypt refreshed token: {e}")))?;

        updated.access_token_expires_at = Some(now.saturating_add(refreshed.expires_in));
        // Preserve the refresh-token expiry; a rotated refresh token from Google
        // does not re-advertise its lifetime here, so keep the prior value.
        updated.refresh_token_expires_at = secret.refresh_token_expires_at;
        Ok(updated)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use awidat_social::token::TestKeyProvider;

    #[test]
    fn missing_refresh_token_is_invalid_grant() {
        let key = TestKeyProvider::new("k1", "server-refresher-key");
        // A secret with no refresh token.
        let secret = TokenSecret::encrypt("acct1", "access", None, &key, 0).unwrap();

        // We can't run the async exchange without a runtime/network, but the
        // missing-refresh-token branch short-circuits before any HTTP. Build the
        // refresher with a throwaway AEAD key matching the test provider.
        let aead = Aead256Key::from_bytes("k1", {
            let mut b = [0u8; 32];
            let src = b"server-refresher-key";
            b[..src.len()].copy_from_slice(src);
            b
        });
        let refresher = ServerTokenRefresher::new("cid".into(), "csecret".into(), aead);
        let err = refresher.refresh("acct1", &secret, 1_000).unwrap_err();
        assert!(matches!(err, TokenRefreshError::InvalidGrant(_)));
    }
}
