use base64::Engine;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSecret {
    pub connected_account_id: String,
    pub encrypted_access_token: String,
    pub encrypted_refresh_token: Option<String>,
    pub access_token_expires_at: Option<i64>,
    pub refresh_token_expires_at: Option<i64>,
    pub token_version: u32,
    pub kms_key_id: String,
    pub last_refreshed_at: Option<i64>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TokenError {
    #[error("key provider returned an empty key")]
    EmptyKey,
    #[error("encrypted token payload is not valid base64")]
    InvalidEncoding,
}

pub trait TokenKeyProvider {
    fn key_id(&self) -> &str;
    fn key_material(&self) -> &[u8];
}

// Local/unit-test key provider boundary. Production code should supply a real
// envelope or KMS-backed provider through the same trait.
#[derive(Clone, Debug)]
pub struct TestKeyProvider {
    key_id: String,
    key_material: Vec<u8>,
}

impl TestKeyProvider {
    pub fn new(key_id: impl Into<String>, key_material: impl Into<String>) -> Self {
        Self {
            key_id: key_id.into(),
            key_material: key_material.into().into_bytes(),
        }
    }
}

impl TokenKeyProvider for TestKeyProvider {
    fn key_id(&self) -> &str {
        &self.key_id
    }

    fn key_material(&self) -> &[u8] {
        &self.key_material
    }
}

impl TokenSecret {
    pub fn encrypt(
        connected_account_id: impl Into<String>,
        access_token: &str,
        refresh_token: Option<&str>,
        key_provider: &impl TokenKeyProvider,
        now: i64,
    ) -> Result<Self, TokenError> {
        Ok(Self {
            connected_account_id: connected_account_id.into(),
            encrypted_access_token: encode_with_key(access_token, key_provider)?,
            encrypted_refresh_token: refresh_token
                .map(|token| encode_with_key(token, key_provider))
                .transpose()?,
            access_token_expires_at: None,
            refresh_token_expires_at: None,
            token_version: 1,
            kms_key_id: key_provider.key_id().to_string(),
            last_refreshed_at: Some(now),
        })
    }

    pub fn decrypt_access_token(
        &self,
        key_provider: &impl TokenKeyProvider,
    ) -> Result<String, TokenError> {
        decode_with_key(&self.encrypted_access_token, key_provider)
    }
}

fn encode_with_key(
    value: &str,
    key_provider: &impl TokenKeyProvider,
) -> Result<String, TokenError> {
    let bytes = xor(value.as_bytes(), key_provider.key_material())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn decode_with_key(
    value: &str,
    key_provider: &impl TokenKeyProvider,
) -> Result<String, TokenError> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value.as_bytes())
        .map_err(|_err| TokenError::InvalidEncoding)?;
    let plain = xor(&decoded, key_provider.key_material())?;
    Ok(String::from_utf8_lossy(&plain).into_owned())
}

fn xor(input: &[u8], key: &[u8]) -> Result<Vec<u8>, TokenError> {
    if key.is_empty() {
        return Err(TokenError::EmptyKey);
    }

    Ok(input
        .iter()
        .enumerate()
        .map(|(idx, byte)| byte ^ key[idx % key.len()])
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_secret_serialization_does_not_include_plaintext_tokens() {
        let secret = TokenSecret::encrypt(
            "acct_1",
            "access-secret",
            Some("refresh-secret"),
            &TestKeyProvider::new("test-key-1", "local-key"),
            100,
        )
        .unwrap_or_else(|err| panic!("encrypt token secret: {err}"));

        let json =
            serde_json::to_string(&secret).unwrap_or_else(|err| panic!("serialize secret: {err}"));
        assert!(!json.contains("access-secret"));
        assert!(!json.contains("refresh-secret"));
        assert_eq!(secret.kms_key_id, "test-key-1");
    }

    #[test]
    fn test_key_provider_round_trips_token_material() {
        let provider = TestKeyProvider::new("test-key-1", "local-key");
        let secret = TokenSecret::encrypt("acct_1", "access-secret", None, &provider, 100)
            .unwrap_or_else(|err| panic!("encrypt token secret: {err}"));
        let decrypted = secret
            .decrypt_access_token(&provider)
            .unwrap_or_else(|err| panic!("decrypt access token: {err}"));
        assert_eq!(decrypted, "access-secret");
    }

    #[test]
    fn encrypt_rejects_empty_key_material() {
        let result = TokenSecret::encrypt(
            "acct_1",
            "access-secret",
            None,
            &TestKeyProvider::new("test-key-1", ""),
            100,
        );

        let err = match result {
            Ok(secret) => panic!("expected empty key error, got secret: {secret:?}"),
            Err(err) => err,
        };

        assert_eq!(err, TokenError::EmptyKey);
    }

    #[test]
    fn decrypt_rejects_invalid_base64() {
        let provider = TestKeyProvider::new("test-key-1", "local-key");
        let secret = TokenSecret {
            connected_account_id: "acct_1".to_string(),
            encrypted_access_token: "not base64!".to_string(),
            encrypted_refresh_token: None,
            access_token_expires_at: None,
            refresh_token_expires_at: None,
            token_version: 1,
            kms_key_id: "test-key-1".to_string(),
            last_refreshed_at: Some(100),
        };

        assert_eq!(
            secret.decrypt_access_token(&provider),
            Err(TokenError::InvalidEncoding)
        );
    }
}
