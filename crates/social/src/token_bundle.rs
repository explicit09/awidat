use crate::model::Provider;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthTokenResponse {
    pub provider_account_id: String,
    pub scopes: String,
    pub expires_in: i64,
    pub refresh_expires_in: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderTokenBundle {
    pub provider: Provider,
    pub provider_account_id: String,
    pub scopes: Vec<String>,
    pub access_token_expires_at: i64,
    pub refresh_token_expires_at: Option<i64>,
}

impl ProviderTokenBundle {
    pub fn from_oauth_response(provider: Provider, response: OAuthTokenResponse, now: i64) -> Self {
        let scopes = split_scopes(&provider, &response.scopes);
        Self {
            provider,
            provider_account_id: response.provider_account_id,
            scopes,
            access_token_expires_at: now + response.expires_in,
            refresh_token_expires_at: response.refresh_expires_in.map(|ttl| now + ttl),
        }
    }
}

fn split_scopes(provider: &Provider, raw: &str) -> Vec<String> {
    let delimiter = match provider {
        Provider::YouTube => ' ',
        Provider::TikTok | Provider::Instagram => ',',
    };

    raw.split(delimiter)
        .map(str::trim)
        .filter(|scope| !scope.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_token_response_normalizes_channel_identity_and_expiry() {
        let response = OAuthTokenResponse {
            provider_account_id: "channel_1".into(),
            scopes: "https://www.googleapis.com/auth/youtube.upload".into(),
            expires_in: 3_600,
            refresh_expires_in: None,
        };

        let bundle = ProviderTokenBundle::from_oauth_response(Provider::YouTube, response, 100);
        assert_eq!(bundle.provider, Provider::YouTube);
        assert_eq!(bundle.provider_account_id, "channel_1");
        assert_eq!(
            bundle.scopes,
            vec!["https://www.googleapis.com/auth/youtube.upload"]
        );
        assert_eq!(bundle.access_token_expires_at, 3_700);
        assert_eq!(bundle.refresh_token_expires_at, None);
    }

    #[test]
    fn tiktok_token_response_splits_comma_scopes_and_refresh_expiry() {
        let response = OAuthTokenResponse {
            provider_account_id: "open_id_1".into(),
            scopes: "user.info.basic,video.publish".into(),
            expires_in: 86_400,
            refresh_expires_in: Some(31_536_000),
        };

        let bundle = ProviderTokenBundle::from_oauth_response(Provider::TikTok, response, 100);
        assert_eq!(bundle.provider, Provider::TikTok);
        assert_eq!(bundle.provider_account_id, "open_id_1");
        assert_eq!(bundle.scopes, vec!["user.info.basic", "video.publish"]);
        assert_eq!(bundle.access_token_expires_at, 86_500);
        assert_eq!(bundle.refresh_token_expires_at, Some(31_536_100));
    }

    #[test]
    fn instagram_token_response_splits_comma_scopes_and_trims_empty_entries() {
        let response = OAuthTokenResponse {
            provider_account_id: "ig_user_1".into(),
            scopes: "instagram_basic, instagram_content_publish,".into(),
            expires_in: 7_200,
            refresh_expires_in: Some(14_400),
        };

        let bundle = ProviderTokenBundle::from_oauth_response(Provider::Instagram, response, 100);
        assert_eq!(bundle.provider, Provider::Instagram);
        assert_eq!(bundle.provider_account_id, "ig_user_1");
        assert_eq!(
            bundle.scopes,
            vec!["instagram_basic", "instagram_content_publish"]
        );
        assert_eq!(bundle.access_token_expires_at, 7_300);
        assert_eq!(bundle.refresh_token_expires_at, Some(14_500));
    }
}
