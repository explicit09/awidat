use crate::model::{OwnerRef, Provider};
use crate::oauth::OAuthConnection;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthProviderConfig {
    pub client_id: String,
    pub redirect_uri: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthAuthorizeRequest {
    pub connection: OAuthConnection,
    pub authorization_url: String,
}

#[allow(clippy::too_many_arguments)]
pub fn begin_provider_oauth(
    id: impl Into<String>,
    owner: OwnerRef,
    provider: Provider,
    config: &OAuthProviderConfig,
    raw_state: &str,
    return_to: String,
    created_at: i64,
    expires_at: i64,
) -> OAuthAuthorizeRequest {
    let scopes = scopes_for(&provider);
    let connection = OAuthConnection::start(
        id,
        owner,
        provider.clone(),
        raw_state,
        scopes.iter().map(|scope| (*scope).to_string()).collect(),
        return_to,
        created_at,
        expires_at,
    );

    OAuthAuthorizeRequest {
        authorization_url: authorization_url(&provider, config, raw_state, &scopes),
        connection,
    }
}

fn scopes_for(provider: &Provider) -> Vec<&'static str> {
    match provider {
        Provider::YouTube => vec![
            "https://www.googleapis.com/auth/youtube.upload",
            "https://www.googleapis.com/auth/youtube.readonly",
        ],
        Provider::TikTok => vec!["user.info.basic", "video.publish"],
        Provider::Instagram => vec![
            "instagram_business_basic",
            "instagram_business_content_publish",
        ],
        Provider::TwitterX => vec!["users.read", "tweet.write", "media.write", "offline.access"],
    }
}

fn authorization_url(
    provider: &Provider,
    config: &OAuthProviderConfig,
    raw_state: &str,
    scopes: &[&str],
) -> String {
    let scope_separator = match provider {
        Provider::YouTube | Provider::TwitterX => " ",
        Provider::TikTok | Provider::Instagram => ",",
    };
    let scope = scopes.join(scope_separator);
    let base_url = match provider {
        Provider::YouTube => "https://accounts.google.com/o/oauth2/v2/auth",
        Provider::TikTok => "https://www.tiktok.com/v2/auth/authorize/",
        Provider::Instagram => "https://www.instagram.com/oauth/authorize",
        Provider::TwitterX => "https://twitter.com/i/oauth2/authorize",
    };
    let client_key = match provider {
        Provider::TikTok => "client_key",
        Provider::YouTube | Provider::Instagram | Provider::TwitterX => "client_id",
    };

    let mut params = vec![
        (client_key.to_string(), config.client_id.clone()),
        ("redirect_uri".to_string(), config.redirect_uri.clone()),
        ("response_type".to_string(), "code".to_string()),
        ("scope".to_string(), scope),
        ("state".to_string(), raw_state.to_string()),
    ];
    if provider == &Provider::YouTube {
        params.push(("access_type".to_string(), "offline".to_string()));
        params.push(("prompt".to_string(), "consent".to_string()));
    }
    if provider == &Provider::TwitterX {
        let code_challenge = pkce_s256_challenge(raw_state);
        params.push(("code_challenge".to_string(), code_challenge));
        params.push(("code_challenge_method".to_string(), "S256".to_string()));
    }
    if provider == &Provider::Instagram {
        params.push(("force_reauth".to_string(), "true".to_string()));
    }

    let query = params
        .into_iter()
        .map(|(key, value)| format!("{}={}", percent_encode(&key), percent_encode(&value)))
        .collect::<Vec<_>>()
        .join("&");

    format!("{base_url}?{query}")
}

fn pkce_s256_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => {
                encoded.push('%');
                encoded.push(hex_digit(byte >> 4));
                encoded.push(hex_digit(byte & 0x0f));
            }
        }
    }
    encoded
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        10..=15 => char::from(b'A' + value - 10),
        _ => '?',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> OAuthProviderConfig {
        OAuthProviderConfig {
            client_id: "client_123".into(),
            redirect_uri: "https://app.montage.test/social/oauth/callback".into(),
        }
    }

    #[test]
    fn youtube_authorize_url_uses_google_endpoint_and_upload_scope() {
        let request = begin_provider_oauth(
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::YouTube,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            200,
        );

        assert!(
            request
                .authorization_url
                .starts_with("https://accounts.google.com/o/oauth2/v2/auth?")
        );
        assert!(request.authorization_url.contains("client_id=client_123"));
        // Both upload and readonly scopes must be present (space-separated, percent-encoded).
        assert!(
            request
                .authorization_url
                .contains("https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fyoutube.upload")
        );
        assert!(
            request
                .authorization_url
                .contains("https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fyoutube.readonly")
        );
        assert!(request.authorization_url.contains("access_type=offline"));
        assert!(request.authorization_url.contains("prompt=consent"));
        assert!(request.authorization_url.contains("state=state-secret"));
        assert_ne!(request.connection.state_hash, "state-secret");
    }

    #[test]
    fn tiktok_authorize_url_uses_tiktok_endpoint_and_publish_scopes() {
        let request = begin_provider_oauth(
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::TikTok,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            200,
        );

        assert!(
            request
                .authorization_url
                .starts_with("https://www.tiktok.com/v2/auth/authorize/?")
        );
        assert!(
            request
                .authorization_url
                .contains("scope=user.info.basic%2Cvideo.publish")
        );
        assert!(request.authorization_url.contains("client_key=client_123"));
        assert!(!request.authorization_url.contains("client_id=client_123"));
        assert!(request.authorization_url.contains("response_type=code"));
        assert!(request.authorization_url.contains("state=state-secret"));
    }

    #[test]
    fn instagram_authorize_url_uses_meta_endpoint_and_business_publish_scope() {
        let request = begin_provider_oauth(
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::Instagram,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            200,
        );

        assert!(
            request
                .authorization_url
                .starts_with("https://www.instagram.com/oauth/authorize?")
        );
        assert!(
            request
                .authorization_url
                .contains("scope=instagram_business_basic%2Cinstagram_business_content_publish")
        );
        assert!(request.authorization_url.contains("force_reauth=true"));
        assert!(request.authorization_url.contains("client_id=client_123"));
        assert!(
            request.authorization_url.contains(
                "redirect_uri=https%3A%2F%2Fapp.montage.test%2Fsocial%2Foauth%2Fcallback"
            )
        );
        assert!(request.authorization_url.contains("state=state-secret"));
    }

    #[test]
    fn twitter_x_authorize_url_includes_pkce_and_write_scopes() {
        let request = begin_provider_oauth(
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::TwitterX,
            &config(),
            "state-secret",
            "/campaigns/campaign_1".into(),
            100,
            200,
        );

        assert!(
            request
                .authorization_url
                .starts_with("https://twitter.com/i/oauth2/authorize?")
        );
        assert!(request.authorization_url.contains("client_id=client_123"));
        assert!(request.authorization_url.contains("tweet.write"));
        assert!(request.authorization_url.contains("media.write"));
        assert!(request.authorization_url.contains("offline.access"));
        assert!(
            request
                .authorization_url
                .contains("code_challenge=GgsJFfpcaJK8VKK4W_HOym7EIymQim3GnS6vhWsz-fg")
        );
        assert!(
            request
                .authorization_url
                .contains("code_challenge_method=S256")
        );
        assert!(
            !request
                .authorization_url
                .contains("code_challenge=state-secret")
        );
    }
}
