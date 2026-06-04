use crate::model::Provider;
use crate::token_bundle::OAuthTokenResponse;

#[derive(Debug)]
pub struct TokenExchangeInput {
    pub provider: Provider,
    pub code: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenExchangeOutput {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_response: OAuthTokenResponse,
}

#[derive(Debug)]
pub enum OAuthExchangeError {
    Http(String),
    InvalidResponse(String),
    ChannelResolutionFailed(String),
}

impl std::fmt::Display for OAuthExchangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(msg) => write!(f, "HTTP error during token exchange: {msg}"),
            Self::InvalidResponse(msg) => write!(f, "unexpected token response: {msg}"),
            Self::ChannelResolutionFailed(msg) => {
                write!(f, "failed to resolve channel identity: {msg}")
            }
        }
    }
}

/// Async trait for exchanging an OAuth authorization code for tokens.
///
/// Each provider gets its own implementation. Tests use a mock implementation.
#[allow(async_fn_in_trait)]
pub trait OAuthTokenExchange {
    async fn exchange(
        &self,
        input: TokenExchangeInput,
    ) -> Result<TokenExchangeOutput, OAuthExchangeError>;
}

// ── Google / YouTube ──────────────────────────────────────────────────────────

pub struct GoogleOAuthExchangeConfig {
    pub client_id: String,
    pub client_secret: String,
}

/// Exchanges a Google OAuth code for tokens and resolves the YouTube channel ID.
///
/// Two round-trips:
/// 1. POST https://oauth2.googleapis.com/token — get access + refresh tokens.
/// 2. GET  https://www.googleapis.com/youtube/v3/channels?part=id&mine=true — resolve channel.
pub struct GoogleOAuthExchange {
    config: GoogleOAuthExchangeConfig,
    client: reqwest::Client,
}

impl GoogleOAuthExchange {
    pub fn new(config: GoogleOAuthExchangeConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }
}

impl OAuthTokenExchange for GoogleOAuthExchange {
    async fn exchange(
        &self,
        input: TokenExchangeInput,
    ) -> Result<TokenExchangeOutput, OAuthExchangeError> {
        // Step 1: exchange the code for tokens.
        let params = [
            ("code", input.code.as_str()),
            ("client_id", self.config.client_id.as_str()),
            ("client_secret", self.config.client_secret.as_str()),
            ("redirect_uri", input.redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ];
        let resp = self
            .client
            .post("https://oauth2.googleapis.com/token")
            .form(&params)
            .send()
            .await
            .map_err(|e| OAuthExchangeError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(OAuthExchangeError::Http(format!(
                "token endpoint {status}: {body}"
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| OAuthExchangeError::InvalidResponse(e.to_string()))?;

        let access_token = json["access_token"]
            .as_str()
            .ok_or_else(|| OAuthExchangeError::InvalidResponse("missing access_token".into()))?
            .to_string();
        let refresh_token = json["refresh_token"].as_str().map(ToOwned::to_owned);
        let expires_in = json["expires_in"]
            .as_i64()
            .ok_or_else(|| OAuthExchangeError::InvalidResponse("missing expires_in".into()))?;
        let scope = json["scope"].as_str().unwrap_or("").to_string();

        // Step 2: resolve the YouTube channel ID.
        let channel_resp = self
            .client
            .get("https://www.googleapis.com/youtube/v3/channels")
            .query(&[("part", "id"), ("mine", "true")])
            .bearer_auth(&access_token)
            .send()
            .await
            .map_err(|e| OAuthExchangeError::ChannelResolutionFailed(e.to_string()))?;

        if !channel_resp.status().is_success() {
            let status = channel_resp.status().as_u16();
            let body = channel_resp.text().await.unwrap_or_default();
            return Err(OAuthExchangeError::ChannelResolutionFailed(format!(
                "channels API {status}: {body}"
            )));
        }

        let channel_json: serde_json::Value = channel_resp
            .json()
            .await
            .map_err(|e| OAuthExchangeError::ChannelResolutionFailed(e.to_string()))?;

        let channel_id = channel_json["items"][0]["id"]
            .as_str()
            .ok_or_else(|| {
                OAuthExchangeError::ChannelResolutionFailed(
                    "no channel found for this account".into(),
                )
            })?
            .to_string();

        Ok(TokenExchangeOutput {
            access_token,
            refresh_token,
            token_response: OAuthTokenResponse {
                provider_account_id: channel_id,
                scopes: scope,
                expires_in,
                refresh_expires_in: None,
            },
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub mod tests {
    use super::*;

    /// Stub exchange for unit tests — returns a fixed output without network.
    pub struct MockOAuthExchange {
        pub output: Result<TokenExchangeOutput, String>,
    }

    impl OAuthTokenExchange for MockOAuthExchange {
        async fn exchange(
            &self,
            _input: TokenExchangeInput,
        ) -> Result<TokenExchangeOutput, OAuthExchangeError> {
            match &self.output {
                Ok(o) => Ok(o.clone()),
                Err(e) => Err(OAuthExchangeError::Http(e.clone())),
            }
        }
    }

    #[tokio::test]
    async fn mock_exchange_returns_configured_output() {
        let exchange = MockOAuthExchange {
            output: Ok(TokenExchangeOutput {
                access_token: "at-123".into(),
                refresh_token: Some("rt-456".into()),
                token_response: OAuthTokenResponse {
                    provider_account_id: "channel_abc".into(),
                    scopes: "https://www.googleapis.com/auth/youtube.upload https://www.googleapis.com/auth/youtube.readonly".into(),
                    expires_in: 3600,
                    refresh_expires_in: None,
                },
            }),
        };

        let result = exchange
            .exchange(TokenExchangeInput {
                provider: Provider::YouTube,
                code: "auth-code".into(),
                redirect_uri: "https://example.com/callback".into(),
            })
            .await
            .unwrap();

        assert_eq!(result.access_token, "at-123");
        assert_eq!(result.refresh_token.as_deref(), Some("rt-456"));
        assert_eq!(result.token_response.provider_account_id, "channel_abc");
    }

    #[tokio::test]
    async fn mock_exchange_propagates_error() {
        let exchange = MockOAuthExchange {
            output: Err("network timeout".into()),
        };

        let err = exchange
            .exchange(TokenExchangeInput {
                provider: Provider::YouTube,
                code: "code".into(),
                redirect_uri: "https://example.com/cb".into(),
            })
            .await
            .unwrap_err();

        assert!(err.to_string().contains("network timeout"));
    }
}
