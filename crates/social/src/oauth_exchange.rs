use crate::model::Provider;
use crate::token_bundle::OAuthTokenResponse;

#[derive(Debug)]
pub struct TokenExchangeInput {
    pub provider: Provider,
    pub code: String,
    pub redirect_uri: String,
    pub code_verifier: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenExchangeOutput {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_response: OAuthTokenResponse,
    pub display_name: Option<String>,
}

#[derive(Debug)]
pub enum OAuthExchangeError {
    Http(String),
    InvalidResponse(String),
    ChannelResolutionFailed(String),
    /// The refresh token was revoked or expired (`invalid_grant`). The account
    /// must be flipped to `NeedsReauth`; do not retry.
    InvalidGrant(String),
}

impl std::fmt::Display for OAuthExchangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(msg) => write!(f, "HTTP error during token exchange: {msg}"),
            Self::InvalidResponse(msg) => write!(f, "unexpected token response: {msg}"),
            Self::ChannelResolutionFailed(msg) => {
                write!(f, "failed to resolve channel identity: {msg}")
            }
            Self::InvalidGrant(msg) => write!(f, "refresh token rejected (invalid_grant): {msg}"),
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

    /// Exchange a stored refresh token for a new access token
    /// (`grant_type=refresh_token`). Used by the at-fire-time refresh and the
    /// token-refresh sweep — never resolves the channel again (the account id
    /// is already known). A new refresh token is returned only if Google
    /// rotates it; otherwise the caller keeps the existing one.
    ///
    /// Maps Google's `invalid_grant` (revoked / expired refresh token) to
    /// [`OAuthExchangeError::InvalidGrant`] so the caller can flip the account
    /// to `NeedsReauth` rather than retrying.
    pub async fn refresh_access_token(
        &self,
        refresh_token: &str,
    ) -> Result<RefreshedTokens, OAuthExchangeError> {
        let params = [
            ("refresh_token", refresh_token),
            ("client_id", self.config.client_id.as_str()),
            ("client_secret", self.config.client_secret.as_str()),
            ("grant_type", "refresh_token"),
        ];
        let resp = self
            .client
            .post("https://oauth2.googleapis.com/token")
            .form(&params)
            .send()
            .await
            .map_err(|e| OAuthExchangeError::Http(e.to_string()))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| OAuthExchangeError::InvalidResponse(e.to_string()))?;

        if !status.is_success() {
            // Google returns 400 with {"error":"invalid_grant"} when the refresh
            // token is revoked or expired.
            if body.contains("invalid_grant") {
                return Err(OAuthExchangeError::InvalidGrant(body));
            }
            return Err(OAuthExchangeError::Http(format!(
                "token endpoint {}: {body}",
                status.as_u16()
            )));
        }

        let json: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| OAuthExchangeError::InvalidResponse(e.to_string()))?;
        let access_token = json["access_token"]
            .as_str()
            .ok_or_else(|| OAuthExchangeError::InvalidResponse("missing access_token".into()))?
            .to_string();
        let expires_in = json["expires_in"]
            .as_i64()
            .ok_or_else(|| OAuthExchangeError::InvalidResponse("missing expires_in".into()))?;
        // Google usually omits a new refresh token; keep the old one if so.
        let refresh_token = json["refresh_token"].as_str().map(ToOwned::to_owned);

        Ok(RefreshedTokens {
            access_token,
            refresh_token,
            expires_in,
            refresh_expires_in: None,
        })
    }
}

/// Result of a `grant_type=refresh_token` exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshedTokens {
    pub access_token: String,
    /// Present only if the provider rotated the refresh token.
    pub refresh_token: Option<String>,
    pub expires_in: i64,
    /// Present only if the provider advertises a new refresh-token lifetime.
    pub refresh_expires_in: Option<i64>,
}

// ── TikTok / Instagram / Twitter/X ───────────────────────────────────────────

pub struct PlatformOAuthExchangeConfig {
    pub provider: Provider,
    pub client_id: String,
    pub client_secret: String,
    pub token_endpoint: String,
    pub profile_endpoint: Option<String>,
}

pub struct PlatformOAuthExchange {
    config: PlatformOAuthExchangeConfig,
    client: reqwest::Client,
}

impl PlatformOAuthExchange {
    pub fn new(config: PlatformOAuthExchangeConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    /// Exchange a stored platform refresh token for a fresh access token.
    ///
    /// TikTok and Twitter/X both use the same token endpoint as authorization
    /// code exchange with `grant_type=refresh_token`; parameter names differ
    /// only for TikTok's `client_key`.
    pub async fn refresh_access_token(
        &self,
        refresh_token: &str,
    ) -> Result<RefreshedTokens, OAuthExchangeError> {
        let params = match self.config.provider {
            Provider::TikTok => vec![
                ("client_key", self.config.client_id.clone()),
                ("client_secret", self.config.client_secret.clone()),
                ("grant_type", "refresh_token".to_string()),
                ("refresh_token", refresh_token.to_string()),
            ],
            Provider::TwitterX => vec![
                ("client_id", self.config.client_id.clone()),
                ("client_secret", self.config.client_secret.clone()),
                ("grant_type", "refresh_token".to_string()),
                ("refresh_token", refresh_token.to_string()),
            ],
            Provider::Instagram => {
                return Err(OAuthExchangeError::InvalidResponse(
                    "Instagram refresh token flow is not configured".into(),
                ));
            }
            Provider::YouTube => {
                return Err(OAuthExchangeError::InvalidResponse(
                    "use GoogleOAuthExchange for YouTube".into(),
                ));
            }
        };

        let resp = self
            .client
            .post(&self.config.token_endpoint)
            .form(&params)
            .send()
            .await
            .map_err(|e| OAuthExchangeError::Http(e.to_string()))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| OAuthExchangeError::InvalidResponse(e.to_string()))?;

        if !status.is_success() {
            if body.contains("invalid_grant") {
                return Err(OAuthExchangeError::InvalidGrant(body));
            }
            return Err(OAuthExchangeError::Http(format!(
                "token endpoint {}: {body}",
                status.as_u16()
            )));
        }

        let json: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| OAuthExchangeError::InvalidResponse(e.to_string()))?;
        if let Some(message) = oauth_error_message(&json) {
            if message.contains("invalid_grant") {
                return Err(OAuthExchangeError::InvalidGrant(message));
            }
            return Err(OAuthExchangeError::Http(format!(
                "token endpoint oauth error: {message}"
            )));
        }

        Ok(RefreshedTokens {
            access_token: string_field(&json, "access_token")?,
            refresh_token: json["refresh_token"].as_str().map(ToOwned::to_owned),
            expires_in: json["expires_in"]
                .as_i64()
                .ok_or_else(|| OAuthExchangeError::InvalidResponse("missing expires_in".into()))?,
            refresh_expires_in: json["refresh_expires_in"].as_i64(),
        })
    }
}

impl OAuthTokenExchange for PlatformOAuthExchange {
    async fn exchange(
        &self,
        input: TokenExchangeInput,
    ) -> Result<TokenExchangeOutput, OAuthExchangeError> {
        if input.provider != self.config.provider {
            return Err(OAuthExchangeError::InvalidResponse(format!(
                "exchange configured for {:?}, got {:?}",
                self.config.provider, input.provider
            )));
        }

        let mut params = match self.config.provider {
            Provider::TikTok => vec![
                ("client_key", self.config.client_id.clone()),
                ("client_secret", self.config.client_secret.clone()),
                ("code", input.code.clone()),
                ("grant_type", "authorization_code".to_string()),
                ("redirect_uri", input.redirect_uri.clone()),
            ],
            Provider::Instagram | Provider::TwitterX => vec![
                ("client_id", self.config.client_id.clone()),
                ("client_secret", self.config.client_secret.clone()),
                ("code", input.code.clone()),
                ("grant_type", "authorization_code".to_string()),
                ("redirect_uri", input.redirect_uri.clone()),
            ],
            Provider::YouTube => {
                return Err(OAuthExchangeError::InvalidResponse(
                    "use GoogleOAuthExchange for YouTube".into(),
                ));
            }
        };
        if let Some(verifier) = input.code_verifier {
            params.push(("code_verifier", verifier));
        }

        let resp = self
            .client
            .post(&self.config.token_endpoint)
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
        if let Some(message) = oauth_error_message(&json) {
            return Err(OAuthExchangeError::Http(format!(
                "token endpoint oauth error: {message}"
            )));
        }

        let mut access_token = string_field(&json, "access_token")?;
        let refresh_token = json["refresh_token"].as_str().map(ToOwned::to_owned);
        let mut expires_in = token_expires_in(&self.config.provider, &json)?;
        if self.config.provider == Provider::Instagram {
            let long_lived = exchange_instagram_long_lived_token(
                &self.client,
                &self.config.token_endpoint,
                &self.config.client_secret,
                &access_token,
            )
            .await?;
            access_token = long_lived.access_token;
            expires_in = long_lived.expires_in;
        }
        let refresh_expires_in = json["refresh_expires_in"].as_i64();
        let scopes = token_scopes(&self.config.provider, &json);
        let profile = provider_profile(
            &self.config.provider,
            &json,
            &self.client,
            self.config.profile_endpoint.as_deref(),
            &access_token,
        )
        .await?;

        Ok(TokenExchangeOutput {
            access_token,
            refresh_token,
            token_response: OAuthTokenResponse {
                provider_account_id: profile.provider_account_id,
                scopes,
                expires_in,
                refresh_expires_in,
            },
            display_name: profile.display_name,
        })
    }
}

struct InstagramLongLivedToken {
    access_token: String,
    expires_in: i64,
}

async fn exchange_instagram_long_lived_token(
    client: &reqwest::Client,
    token_endpoint: &str,
    client_secret: &str,
    short_lived_access_token: &str,
) -> Result<InstagramLongLivedToken, OAuthExchangeError> {
    let endpoint = instagram_long_lived_token_endpoint(token_endpoint)?;
    let resp = client
        .get(endpoint)
        .query(&[
            ("grant_type", "ig_exchange_token"),
            ("client_secret", client_secret),
            ("access_token", short_lived_access_token),
        ])
        .send()
        .await
        .map_err(|e| OAuthExchangeError::Http(e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(OAuthExchangeError::Http(format!(
            "instagram long-lived token endpoint {status}: {body}"
        )));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| OAuthExchangeError::InvalidResponse(e.to_string()))?;
    Ok(InstagramLongLivedToken {
        access_token: string_field(&json, "access_token")?,
        expires_in: json["expires_in"]
            .as_i64()
            .ok_or_else(|| OAuthExchangeError::InvalidResponse("missing expires_in".into()))?,
    })
}

fn instagram_long_lived_token_endpoint(
    token_endpoint: &str,
) -> Result<reqwest::Url, OAuthExchangeError> {
    let mut endpoint = reqwest::Url::parse(token_endpoint)
        .map_err(|e| OAuthExchangeError::InvalidResponse(e.to_string()))?;
    if endpoint.host_str() == Some("api.instagram.com") {
        endpoint
            .set_host(Some("graph.instagram.com"))
            .map_err(|_| OAuthExchangeError::InvalidResponse("invalid Instagram host".into()))?;
    }
    endpoint.set_path("/access_token");
    endpoint.set_query(None);
    Ok(endpoint)
}

fn string_field(json: &serde_json::Value, field: &str) -> Result<String, OAuthExchangeError> {
    json[field]
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| OAuthExchangeError::InvalidResponse(format!("missing {field}")))
}

fn oauth_error_message(json: &serde_json::Value) -> Option<String> {
    let error = json["error"]
        .as_str()
        .or_else(|| json["error_type"].as_str())
        .or_else(|| json["code"].as_str())?;
    let mut parts = vec![error.to_string()];
    if let Some(description) = json["error_description"]
        .as_str()
        .or_else(|| json["message"].as_str())
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(description.to_string());
    }
    if let Some(log_id) = json["log_id"]
        .as_str()
        .or_else(|| json["logid"].as_str())
        .filter(|value| !value.trim().is_empty())
    {
        parts.push(format!("log_id={log_id}"));
    }
    Some(parts.join(": "))
}

fn token_expires_in(
    provider: &Provider,
    json: &serde_json::Value,
) -> Result<i64, OAuthExchangeError> {
    if let Some(expires_in) = json["expires_in"].as_i64() {
        return Ok(expires_in);
    }
    if provider == &Provider::Instagram {
        return Ok(3_600);
    }
    Err(OAuthExchangeError::InvalidResponse(
        "missing expires_in".to_string(),
    ))
}

fn token_scopes(provider: &Provider, json: &serde_json::Value) -> String {
    if let Some(scope) = json["scope"].as_str() {
        return scope.to_string();
    }
    if provider == &Provider::Instagram {
        return json["permissions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|permission| permission.as_str())
            .collect::<Vec<_>>()
            .join(",");
    }
    String::new()
}

struct ProviderProfile {
    provider_account_id: String,
    display_name: Option<String>,
}

async fn provider_profile(
    provider: &Provider,
    token_json: &serde_json::Value,
    client: &reqwest::Client,
    profile_endpoint: Option<&str>,
    access_token: &str,
) -> Result<ProviderProfile, OAuthExchangeError> {
    match provider {
        Provider::TikTok => {
            let token_open_id = token_json["open_id"]
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| OAuthExchangeError::InvalidResponse("missing open_id".into()))?;
            let Some(endpoint) = profile_endpoint else {
                return Ok(ProviderProfile {
                    provider_account_id: token_open_id,
                    display_name: None,
                });
            };
            let profile = fetch_tiktok_profile_json(client, endpoint, access_token)
                .await
                .ok();
            let user = profile
                .as_ref()
                .and_then(|json| json["data"]["user"].as_object());
            let provider_account_id = user
                .and_then(|user| user.get("open_id"))
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned)
                .unwrap_or(token_open_id);
            let display_name = user
                .and_then(|user| user.get("display_name"))
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned);
            Ok(ProviderProfile {
                provider_account_id,
                display_name,
            })
        }
        Provider::Instagram => {
            if let Some(id) = token_json["user_id"].as_str() {
                return Ok(ProviderProfile {
                    provider_account_id: id.to_string(),
                    display_name: None,
                });
            }
            let profile = fetch_profile_json(client, profile_endpoint, access_token).await?;
            Ok(instagram_profile(&profile)?)
        }
        Provider::TwitterX => {
            let profile = fetch_profile_json(client, profile_endpoint, access_token).await?;
            let data = &profile["data"];
            let provider_account_id =
                data["id"].as_str().map(ToOwned::to_owned).ok_or_else(|| {
                    OAuthExchangeError::InvalidResponse("missing Twitter/X user id".into())
                })?;
            let display_name = data["username"]
                .as_str()
                .or_else(|| data["name"].as_str())
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned);
            Ok(ProviderProfile {
                provider_account_id,
                display_name,
            })
        }
        Provider::YouTube => Err(OAuthExchangeError::InvalidResponse(
            "use GoogleOAuthExchange for YouTube".into(),
        )),
    }
}

async fn fetch_tiktok_profile_json(
    client: &reqwest::Client,
    endpoint: &str,
    access_token: &str,
) -> Result<serde_json::Value, OAuthExchangeError> {
    let resp = client
        .get(endpoint)
        .query(&[("fields", "open_id,avatar_url,display_name")])
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| OAuthExchangeError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(OAuthExchangeError::Http(format!(
            "profile endpoint {status}: {body}"
        )));
    }
    resp.json()
        .await
        .map_err(|e| OAuthExchangeError::InvalidResponse(e.to_string()))
}

fn instagram_profile(profile: &serde_json::Value) -> Result<ProviderProfile, OAuthExchangeError> {
    profile["data"]
        .as_array()
        .and_then(|pages| {
            pages.iter().find_map(|page| {
                let account = &page["instagram_business_account"];
                let provider_account_id = account["id"]
                    .as_str()
                    .or_else(|| account["ig_id"].as_str())?;
                let display_name = account["username"]
                    .as_str()
                    .or_else(|| account["name"].as_str())
                    .map(ToOwned::to_owned);
                Some(ProviderProfile {
                    provider_account_id: provider_account_id.to_string(),
                    display_name,
                })
            })
        })
        .ok_or_else(|| {
            OAuthExchangeError::InvalidResponse("missing Instagram professional account id".into())
        })
}

async fn fetch_profile_json(
    client: &reqwest::Client,
    profile_endpoint: Option<&str>,
    access_token: &str,
) -> Result<serde_json::Value, OAuthExchangeError> {
    let endpoint = profile_endpoint
        .ok_or_else(|| OAuthExchangeError::InvalidResponse("missing profile endpoint".into()))?;
    let resp = client
        .get(endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| OAuthExchangeError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(OAuthExchangeError::Http(format!(
            "profile endpoint {status}: {body}"
        )));
    }
    resp.json()
        .await
        .map_err(|e| OAuthExchangeError::InvalidResponse(e.to_string()))
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

        // Step 2: resolve the YouTube channel ID and public channel title.
        let channel_resp = self
            .client
            .get("https://www.googleapis.com/youtube/v3/channels")
            .query(&[("part", "id,snippet"), ("mine", "true")])
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
        let display_name = channel_json["items"][0]["snippet"]["title"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .map(ToOwned::to_owned);

        Ok(TokenExchangeOutput {
            access_token,
            refresh_token,
            token_response: OAuthTokenResponse {
                provider_account_id: channel_id,
                scopes: scope,
                expires_in,
                refresh_expires_in: None,
            },
            display_name,
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
                display_name: Some("Montage Channel".into()),
            }),
        };

        let result = exchange
            .exchange(TokenExchangeInput {
                provider: Provider::YouTube,
                code: "auth-code".into(),
                redirect_uri: "https://example.com/callback".into(),
                code_verifier: None,
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
                code_verifier: None,
            })
            .await
            .unwrap_err();

        assert!(err.to_string().contains("network timeout"));
    }

    #[tokio::test]
    async fn platform_exchange_resolves_tiktok_open_id_from_token_response() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(body_string_contains("client_key=tiktok-key"))
            .and(body_string_contains("client_secret=tiktok-secret"))
            .and(body_string_contains("grant_type=authorization_code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "tt-access",
                "refresh_token": "tt-refresh",
                "open_id": "open_id_1",
                "scope": "user.info.basic,video.publish",
                "expires_in": 86400,
                "refresh_expires_in": 31536000
            })))
            .mount(&server)
            .await;

        let exchange = PlatformOAuthExchange::new(PlatformOAuthExchangeConfig {
            provider: Provider::TikTok,
            client_id: "tiktok-key".into(),
            client_secret: "tiktok-secret".into(),
            token_endpoint: format!("{}/oauth/token", server.uri()),
            profile_endpoint: None,
        });

        let output = exchange
            .exchange(TokenExchangeInput {
                provider: Provider::TikTok,
                code: "auth-code".into(),
                redirect_uri: "https://app.example/oauth/callback/tiktok".into(),
                code_verifier: None,
            })
            .await
            .unwrap();

        assert_eq!(output.access_token, "tt-access");
        assert_eq!(output.refresh_token.as_deref(), Some("tt-refresh"));
        assert_eq!(output.token_response.provider_account_id, "open_id_1");
        assert_eq!(
            output.token_response.scopes,
            "user.info.basic,video.publish"
        );
    }

    #[tokio::test]
    async fn platform_exchange_resolves_tiktok_display_name_from_user_info() {
        use wiremock::matchers::{bearer_token, body_string_contains, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(body_string_contains("client_key=tiktok-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "tt-access",
                "refresh_token": "tt-refresh",
                "open_id": "open_id_1",
                "scope": "user.info.basic,video.publish",
                "expires_in": 86400,
                "refresh_expires_in": 31536000
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/user/info/"))
            .and(query_param("fields", "open_id,avatar_url,display_name"))
            .and(bearer_token("tt-access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "user": {
                        "open_id": "open_id_1",
                        "display_name": "Montage Creator",
                        "avatar_url": "https://example.com/avatar.jpg"
                    }
                }
            })))
            .mount(&server)
            .await;

        let exchange = PlatformOAuthExchange::new(PlatformOAuthExchangeConfig {
            provider: Provider::TikTok,
            client_id: "tiktok-key".into(),
            client_secret: "tiktok-secret".into(),
            token_endpoint: format!("{}/oauth/token", server.uri()),
            profile_endpoint: Some(format!("{}/v2/user/info/", server.uri())),
        });

        let output = exchange
            .exchange(TokenExchangeInput {
                provider: Provider::TikTok,
                code: "auth-code".into(),
                redirect_uri: "https://app.example/oauth/callback/tiktok".into(),
                code_verifier: None,
            })
            .await
            .unwrap();

        assert_eq!(output.token_response.provider_account_id, "open_id_1");
        assert_eq!(output.display_name.as_deref(), Some("Montage Creator"));
    }

    #[tokio::test]
    async fn platform_exchange_reports_provider_oauth_error_before_missing_access_token() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(body_string_contains("client_key=tiktok-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "error": "invalid_client",
                "error_description": "client_key and client_secret do not match",
                "log_id": "20260607TOKEN"
            })))
            .mount(&server)
            .await;

        let exchange = PlatformOAuthExchange::new(PlatformOAuthExchangeConfig {
            provider: Provider::TikTok,
            client_id: "tiktok-key".into(),
            client_secret: "wrong-secret".into(),
            token_endpoint: format!("{}/oauth/token", server.uri()),
            profile_endpoint: None,
        });

        let err = exchange
            .exchange(TokenExchangeInput {
                provider: Provider::TikTok,
                code: "auth-code".into(),
                redirect_uri: "https://app.example/oauth/callback/tiktok".into(),
                code_verifier: None,
            })
            .await
            .unwrap_err();

        let message = err.to_string();
        assert!(
            message.contains("invalid_client"),
            "message should include provider error code, got {message}"
        );
        assert!(
            message.contains("client_key and client_secret do not match"),
            "message should include provider description, got {message}"
        );
        assert!(
            !message.contains("missing access_token"),
            "provider OAuth errors should not be hidden as parser errors: {message}"
        );
    }

    #[tokio::test]
    async fn platform_exchange_exchanges_instagram_login_token_for_long_lived_token() {
        use wiremock::matchers::{body_string_contains, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/access_token"))
            .and(body_string_contains("client_id=ig-client"))
            .and(body_string_contains("client_secret=ig-secret"))
            .and(body_string_contains("grant_type=authorization_code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "ig-short-access",
                "user_id": "ig_user_1",
                "permissions": [
                    "instagram_business_basic",
                    "instagram_business_content_publish"
                ]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/access_token"))
            .and(query_param("grant_type", "ig_exchange_token"))
            .and(query_param("client_secret", "ig-secret"))
            .and(query_param("access_token", "ig-short-access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "ig-long-access",
                "token_type": "bearer",
                "expires_in": 5_184_000
            })))
            .mount(&server)
            .await;

        let exchange = PlatformOAuthExchange::new(PlatformOAuthExchangeConfig {
            provider: Provider::Instagram,
            client_id: "ig-client".into(),
            client_secret: "ig-secret".into(),
            token_endpoint: format!("{}/oauth/access_token", server.uri()),
            profile_endpoint: None,
        });

        let output = exchange
            .exchange(TokenExchangeInput {
                provider: Provider::Instagram,
                code: "auth-code".into(),
                redirect_uri: "https://app.example/oauth/callback/instagram".into(),
                code_verifier: None,
            })
            .await
            .unwrap();

        assert_eq!(output.access_token, "ig-long-access");
        assert_eq!(output.token_response.provider_account_id, "ig_user_1");
        assert_eq!(
            output.token_response.scopes,
            "instagram_business_basic,instagram_business_content_publish"
        );
        assert_eq!(output.token_response.expires_in, 5_184_000);
    }

    #[tokio::test]
    async fn platform_exchange_sends_twitter_x_pkce_verifier_and_resolves_profile() {
        use wiremock::matchers::{bearer_token, body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/2/oauth2/token"))
            .and(body_string_contains("client_id=x-client"))
            .and(body_string_contains("client_secret=x-secret"))
            .and(body_string_contains("code_verifier=state-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "x-access",
                "refresh_token": "x-refresh",
                "scope": "users.read tweet.write media.write offline.access",
                "expires_in": 7200
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/2/users/me"))
            .and(bearer_token("x-access"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "id": "x_user_1",
                    "name": "Creator",
                    "username": "creator"
                }
            })))
            .mount(&server)
            .await;

        let exchange = PlatformOAuthExchange::new(PlatformOAuthExchangeConfig {
            provider: Provider::TwitterX,
            client_id: "x-client".into(),
            client_secret: "x-secret".into(),
            token_endpoint: format!("{}/2/oauth2/token", server.uri()),
            profile_endpoint: Some(format!("{}/2/users/me", server.uri())),
        });

        let output = exchange
            .exchange(TokenExchangeInput {
                provider: Provider::TwitterX,
                code: "auth-code".into(),
                redirect_uri: "https://app.example/oauth/callback/twitter_x".into(),
                code_verifier: Some("state-secret".into()),
            })
            .await
            .unwrap();

        assert_eq!(output.token_response.provider_account_id, "x_user_1");
        assert_eq!(
            output.token_response.scopes,
            "users.read tweet.write media.write offline.access"
        );
    }

    #[tokio::test]
    async fn platform_refresh_exchanges_tiktok_refresh_token() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .and(body_string_contains("client_key=tiktok-key"))
            .and(body_string_contains("client_secret=tiktok-secret"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=tt-refresh-old"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "tt-access-new",
                "refresh_token": "tt-refresh-new",
                "expires_in": 86400,
                "refresh_expires_in": 31536000
            })))
            .mount(&server)
            .await;

        let exchange = PlatformOAuthExchange::new(PlatformOAuthExchangeConfig {
            provider: Provider::TikTok,
            client_id: "tiktok-key".into(),
            client_secret: "tiktok-secret".into(),
            token_endpoint: format!("{}/oauth/token", server.uri()),
            profile_endpoint: None,
        });

        let refreshed = exchange
            .refresh_access_token("tt-refresh-old")
            .await
            .unwrap();

        assert_eq!(refreshed.access_token, "tt-access-new");
        assert_eq!(refreshed.refresh_token.as_deref(), Some("tt-refresh-new"));
        assert_eq!(refreshed.expires_in, 86400);
        assert_eq!(refreshed.refresh_expires_in, Some(31536000));
    }

    #[tokio::test]
    async fn platform_refresh_exchanges_twitter_x_refresh_token() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/2/oauth2/token"))
            .and(body_string_contains("client_id=x-client"))
            .and(body_string_contains("client_secret=x-secret"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=x-refresh-old"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "x-access-new",
                "refresh_token": "x-refresh-new",
                "expires_in": 7200
            })))
            .mount(&server)
            .await;

        let exchange = PlatformOAuthExchange::new(PlatformOAuthExchangeConfig {
            provider: Provider::TwitterX,
            client_id: "x-client".into(),
            client_secret: "x-secret".into(),
            token_endpoint: format!("{}/2/oauth2/token", server.uri()),
            profile_endpoint: None,
        });

        let refreshed = exchange
            .refresh_access_token("x-refresh-old")
            .await
            .unwrap();

        assert_eq!(refreshed.access_token, "x-access-new");
        assert_eq!(refreshed.refresh_token.as_deref(), Some("x-refresh-new"));
        assert_eq!(refreshed.expires_in, 7200);
        assert_eq!(refreshed.refresh_expires_in, None);
    }
}
