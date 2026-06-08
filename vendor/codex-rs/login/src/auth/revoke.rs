//! Best-effort OAuth token revocation for managed auth cleanup.
//!
//! Managed ChatGPT auth stores OAuth tokens locally. Cleanup attempts to revoke
//! the refresh token, falling back to the access token when no refresh token is
//! available, and callers still complete their primary work if the revoke request
//! fails.

use serde::Serialize;
use std::time::Duration;

use codex_app_server_protocol::AuthMode as ApiAuthMode;
use codex_client::CodexHttpClient;

use super::manager::MONTAGE_OAUTH_CLIENT_ID_ENV_VAR;
use super::manager::REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR;
use super::manager::REVOKE_TOKEN_URL;
use super::manager::REVOKE_TOKEN_URL_OVERRIDE_ENV_VAR;
use super::manager::configured_montage_oauth_client_id;
use super::storage::AuthDotJson;
use super::util::try_parse_error_message;
use crate::default_client::create_client;
use crate::token_data::TokenData;

const REVOKE_HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RevokeTokenKind {
    Access,
    Refresh,
}

impl RevokeTokenKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Access => "access_token",
            Self::Refresh => "refresh_token",
        }
    }
}

#[derive(Serialize)]
struct RevokeTokenRequest {
    token: String,
    token_type_hint: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_id: Option<String>,
}

pub(crate) async fn revoke_auth_tokens(
    auth_dot_json: Option<&AuthDotJson>,
) -> Result<(), std::io::Error> {
    let Some((token, kind)) = auth_dot_json.and_then(revocable_token) else {
        return Ok(());
    };

    let client = create_client();
    let endpoint = revoke_token_endpoint();
    revoke_oauth_token(&client, endpoint.as_str(), token, kind, REVOKE_HTTP_TIMEOUT).await
}

pub(crate) fn should_revoke_auth_tokens(
    auth_dot_json: Option<&AuthDotJson>,
    replacement_auth: &AuthDotJson,
) -> bool {
    let Some((token, kind)) = auth_dot_json.and_then(revocable_token) else {
        return false;
    };
    let Some(replacement_tokens) = managed_chatgpt_tokens(replacement_auth) else {
        return true;
    };

    match kind {
        RevokeTokenKind::Access => replacement_tokens.access_token != token,
        RevokeTokenKind::Refresh => replacement_tokens.refresh_token != token,
    }
}

fn revocable_token(auth_dot_json: &AuthDotJson) -> Option<(&str, RevokeTokenKind)> {
    let tokens = managed_chatgpt_tokens(auth_dot_json)?;
    if !tokens.refresh_token.is_empty() {
        Some((tokens.refresh_token.as_str(), RevokeTokenKind::Refresh))
    } else if !tokens.access_token.is_empty() {
        Some((tokens.access_token.as_str(), RevokeTokenKind::Access))
    } else {
        None
    }
}

fn managed_chatgpt_tokens(auth_dot_json: &AuthDotJson) -> Option<&TokenData> {
    if resolved_auth_mode(auth_dot_json) == ApiAuthMode::Chatgpt {
        auth_dot_json.tokens.as_ref()
    } else {
        None
    }
}

fn resolved_auth_mode(auth_dot_json: &AuthDotJson) -> ApiAuthMode {
    if let Some(mode) = auth_dot_json.auth_mode {
        return mode;
    }
    if auth_dot_json.openai_api_key.is_some() {
        return ApiAuthMode::ApiKey;
    }
    ApiAuthMode::Chatgpt
}

async fn revoke_oauth_token(
    client: &CodexHttpClient,
    endpoint: &str,
    token: &str,
    kind: RevokeTokenKind,
    timeout: Duration,
) -> Result<(), std::io::Error> {
    let client_id = match kind {
        RevokeTokenKind::Access => None,
        RevokeTokenKind::Refresh => Some(configured_montage_oauth_client_id().ok_or_else(|| {
            std::io::Error::other(format!(
                "ChatGPT OAuth revoke is not configured. Set {MONTAGE_OAUTH_CLIENT_ID_ENV_VAR} to the sanctioned client id used for login."
            ))
        })?),
    };
    let request = RevokeTokenRequest {
        token: token.to_string(),
        token_type_hint: kind.as_str(),
        client_id,
    };

    let response = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .timeout(timeout)
        .json(&request)
        .send()
        .await
        .map_err(std::io::Error::other)?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }

    let body = response.text().await.unwrap_or_default();
    let message = try_parse_error_message(&body);
    Err(std::io::Error::other(format!(
        "failed to revoke {}: {}: {}",
        kind.as_str(),
        status,
        message
    )))
}

fn revoke_token_endpoint() -> String {
    if let Ok(endpoint) = std::env::var(REVOKE_TOKEN_URL_OVERRIDE_ENV_VAR) {
        return endpoint;
    }

    if let Ok(refresh_endpoint) = std::env::var(REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR)
        && let Some(endpoint) = derive_revoke_token_endpoint(&refresh_endpoint)
    {
        return endpoint;
    }

    REVOKE_TOKEN_URL.to_string()
}

fn derive_revoke_token_endpoint(refresh_endpoint: &str) -> Option<String> {
    let mut url = url::Url::parse(refresh_endpoint).ok()?;
    url.set_path("/oauth/revoke");
    url.set_query(None);
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_test_support::skip_if_no_network;
    use std::env;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    struct EnvVarGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = env::var_os(key);
            unsafe {
                env::set_var(key, value);
            }
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.original {
                    Some(value) => env::set_var(self.key, value),
                    None => env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn derives_revoke_url_from_refresh_token_override() {
        assert_eq!(
            derive_revoke_token_endpoint("http://127.0.0.1:1234/oauth/token?unified=true"),
            Some("http://127.0.0.1:1234/oauth/revoke".to_string())
        );
    }

    #[tokio::test]
    async fn revoke_request_times_out() {
        skip_if_no_network!();

        let server = MockServer::start().await;
        let _client_id_guard = EnvVarGuard::set(MONTAGE_OAUTH_CLIENT_ID_ENV_VAR, "app_sanctioned");
        Mock::given(method("POST"))
            .and(path("/oauth/revoke"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60)))
            .mount(&server)
            .await;

        let client = CodexHttpClient::new(reqwest::Client::new());
        let endpoint = format!("{}/oauth/revoke", server.uri());
        let error = revoke_oauth_token(
            &client,
            endpoint.as_str(),
            "refresh-token",
            RevokeTokenKind::Refresh,
            Duration::from_millis(20),
        )
        .await
        .expect_err("stalled revoke request should time out");

        let reqwest_error = error
            .get_ref()
            .and_then(|error| error.downcast_ref::<reqwest::Error>())
            .expect("timeout error should preserve reqwest error");
        assert!(reqwest_error.is_timeout());
    }

    #[tokio::test]
    #[serial_test::serial(codex_auth_env)]
    async fn revoke_refresh_request_uses_configured_montage_oauth_client_id() {
        let server = MockServer::start().await;
        let _client_id_guard = EnvVarGuard::set(MONTAGE_OAUTH_CLIENT_ID_ENV_VAR, "app_sanctioned");

        Mock::given(method("POST"))
            .and(path("/oauth/revoke"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let client = CodexHttpClient::new(reqwest::Client::new());
        let endpoint = format!("{}/oauth/revoke", server.uri());
        revoke_oauth_token(
            &client,
            endpoint.as_str(),
            "refresh-token",
            RevokeTokenKind::Refresh,
            REVOKE_HTTP_TIMEOUT,
        )
        .await
        .expect("configured OAuth client id should allow revoke");

        let requests = server
            .received_requests()
            .await
            .expect("received requests should be available");
        let body: serde_json::Value =
            serde_json::from_slice(&requests[0].body).expect("request body should be JSON");
        assert_eq!(body["client_id"], "app_sanctioned");
        assert_eq!(body["token"], "refresh-token");
        assert_eq!(body["token_type_hint"], "refresh_token");
    }
}
