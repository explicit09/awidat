use base64::Engine;
use chacha20poly1305::{
    AeadCore, ChaCha20Poly1305, KeyInit,
    aead::{Aead, OsRng},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const TOKEN_VERSION: u32 = 2;

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
    #[error("key material must be exactly 32 bytes for AEAD")]
    BadKeyLength,
    #[error("encrypted token payload is not valid base64")]
    InvalidEncoding,
    #[error("decrypted token payload is not valid UTF-8")]
    InvalidPlaintext,
    #[error("AEAD encryption failed")]
    EncryptionFailed,
    #[error("AEAD decryption failed — ciphertext is corrupted or key is wrong")]
    DecryptionFailed,
    #[error("token_version {0} is not supported; re-authenticate to generate a v2 token")]
    UnsupportedTokenVersion(u32),
}

/// Supplies raw key material for ChaCha20-Poly1305 envelope encryption.
///
/// Implementations must return exactly 32 bytes from `key_material`.
pub trait LocalTokenKeyProvider {
    fn key_id(&self) -> &str;
    fn key_material(&self) -> &[u8];
}

/// A key provider backed by a 32-byte secret loaded from an environment variable
/// or a secret manager at startup.
#[derive(Debug)]
pub struct Aead256Key {
    key_id: String,
    key_bytes: [u8; 32],
}

impl Aead256Key {
    pub fn from_bytes(key_id: impl Into<String>, bytes: [u8; 32]) -> Self {
        Self {
            key_id: key_id.into(),
            key_bytes: bytes,
        }
    }

    /// Decode a hex-encoded 32-byte key (64 hex chars).
    pub fn from_hex(key_id: impl Into<String>, hex: &str) -> Result<Self, TokenError> {
        if hex.len() != 64 {
            return Err(TokenError::BadKeyLength);
        }
        let mut bytes = [0u8; 32];
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let hi = hex_val(chunk[0]).ok_or(TokenError::BadKeyLength)?;
            let lo = hex_val(chunk[1]).ok_or(TokenError::BadKeyLength)?;
            bytes[i] = (hi << 4) | lo;
        }
        Ok(Self::from_bytes(key_id, bytes))
    }
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

impl LocalTokenKeyProvider for Aead256Key {
    fn key_id(&self) -> &str {
        &self.key_id
    }
    fn key_material(&self) -> &[u8] {
        &self.key_bytes
    }
}

/// Test-only key provider. Pads the provided string to 32 bytes (zero-padded).
/// An empty `key_material` string produces an empty slice so `EmptyKey` is still returned.
#[derive(Clone, Debug)]
pub struct TestKeyProvider {
    key_id: String,
    key_bytes: Vec<u8>,
}

impl TestKeyProvider {
    pub fn new(key_id: impl Into<String>, key_material: impl Into<String>) -> Self {
        let raw = key_material.into().into_bytes();
        let key_bytes = if raw.is_empty() {
            Vec::new()
        } else {
            // Pad or truncate to exactly 32 bytes so AEAD always sees a valid key length.
            let mut bytes = vec![0u8; 32];
            let len = raw.len().min(32);
            bytes[..len].copy_from_slice(&raw[..len]);
            bytes
        };
        Self {
            key_id: key_id.into(),
            key_bytes,
        }
    }
}

impl LocalTokenKeyProvider for TestKeyProvider {
    fn key_id(&self) -> &str {
        &self.key_id
    }
    fn key_material(&self) -> &[u8] {
        &self.key_bytes
    }
}

impl TokenSecret {
    pub fn encrypt(
        connected_account_id: impl Into<String>,
        access_token: &str,
        refresh_token: Option<&str>,
        key_provider: &impl LocalTokenKeyProvider,
        now: i64,
    ) -> Result<Self, TokenError> {
        Ok(Self {
            connected_account_id: connected_account_id.into(),
            encrypted_access_token: aead_seal(access_token, key_provider)?,
            encrypted_refresh_token: refresh_token
                .map(|token| aead_seal(token, key_provider))
                .transpose()?,
            access_token_expires_at: None,
            refresh_token_expires_at: None,
            token_version: TOKEN_VERSION,
            kms_key_id: key_provider.key_id().to_string(),
            last_refreshed_at: Some(now),
        })
    }

    pub fn decrypt_access_token(
        &self,
        key_provider: &impl LocalTokenKeyProvider,
    ) -> Result<String, TokenError> {
        if self.token_version != TOKEN_VERSION {
            return Err(TokenError::UnsupportedTokenVersion(self.token_version));
        }
        aead_open(&self.encrypted_access_token, key_provider)
    }

    pub fn decrypt_refresh_token(
        &self,
        key_provider: &impl LocalTokenKeyProvider,
    ) -> Result<Option<String>, TokenError> {
        if self.token_version != TOKEN_VERSION {
            return Err(TokenError::UnsupportedTokenVersion(self.token_version));
        }
        self.encrypted_refresh_token
            .as_deref()
            .map(|enc| aead_open(enc, key_provider))
            .transpose()
    }
}

/// Encrypt `plaintext` with ChaCha20-Poly1305.
/// Output format: base64(nonce || ciphertext_with_tag)
fn aead_seal(
    plaintext: &str,
    key_provider: &impl LocalTokenKeyProvider,
) -> Result<String, TokenError> {
    let key_bytes = key_provider.key_material();
    if key_bytes.is_empty() {
        return Err(TokenError::EmptyKey);
    }
    if key_bytes.len() != 32 {
        return Err(TokenError::BadKeyLength);
    }
    let cipher =
        ChaCha20Poly1305::new_from_slice(key_bytes).map_err(|_| TokenError::BadKeyLength)?;
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| TokenError::EncryptionFailed)?;
    let mut payload = nonce.to_vec();
    payload.extend_from_slice(&ciphertext);
    Ok(base64::engine::general_purpose::STANDARD.encode(payload))
}

/// Decrypt a value produced by `aead_seal`.
fn aead_open(
    encoded: &str,
    key_provider: &impl LocalTokenKeyProvider,
) -> Result<String, TokenError> {
    let key_bytes = key_provider.key_material();
    if key_bytes.is_empty() {
        return Err(TokenError::EmptyKey);
    }
    if key_bytes.len() != 32 {
        return Err(TokenError::BadKeyLength);
    }
    let payload = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| TokenError::InvalidEncoding)?;
    if payload.len() < 12 {
        return Err(TokenError::InvalidEncoding);
    }
    let (nonce_bytes, ciphertext) = payload.split_at(12);
    let nonce = chacha20poly1305::Nonce::from_slice(nonce_bytes);
    let cipher =
        ChaCha20Poly1305::new_from_slice(key_bytes).map_err(|_| TokenError::BadKeyLength)?;
    let plain = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| TokenError::DecryptionFailed)?;
    String::from_utf8(plain).map_err(|_| TokenError::InvalidPlaintext)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn provider() -> TestKeyProvider {
        TestKeyProvider::new("test-key-1", "local-key-for-testing")
    }

    #[test]
    fn token_secret_serialization_does_not_include_plaintext_tokens() {
        let secret = TokenSecret::encrypt(
            "acct_1",
            "access-secret",
            Some("refresh-secret"),
            &provider(),
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
        let p = provider();
        let secret = TokenSecret::encrypt("acct_1", "access-secret", None, &p, 100)
            .unwrap_or_else(|err| panic!("encrypt token secret: {err}"));
        let decrypted = secret
            .decrypt_access_token(&p)
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
        assert_eq!(result.unwrap_err(), TokenError::EmptyKey);
    }

    #[test]
    fn decrypt_rejects_invalid_base64() {
        let p = provider();
        let secret = TokenSecret {
            connected_account_id: "acct_1".to_string(),
            encrypted_access_token: "not base64!".to_string(),
            encrypted_refresh_token: None,
            access_token_expires_at: None,
            refresh_token_expires_at: None,
            token_version: TOKEN_VERSION,
            kms_key_id: "test-key-1".to_string(),
            last_refreshed_at: Some(100),
        };
        assert_eq!(
            secret.decrypt_access_token(&p),
            Err(TokenError::InvalidEncoding)
        );
    }

    #[test]
    fn decrypt_rejects_invalid_plaintext_utf8() {
        // Manufacture a valid AEAD blob that decrypts to non-UTF-8 bytes.
        // Encrypt raw bytes directly against the cipher.
        let p = provider();
        let cipher = ChaCha20Poly1305::new_from_slice(p.key_material()).unwrap();
        let nonce = chacha20poly1305::Nonce::from_slice(&[0u8; 12]);
        let bad_bytes: &[u8] = &[0xFF, 0xFE, 0xFD];
        let ct = cipher.encrypt(nonce, bad_bytes).unwrap();
        let mut payload = nonce.to_vec();
        payload.extend_from_slice(&ct);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&payload);

        let secret = TokenSecret {
            connected_account_id: "acct_1".to_string(),
            encrypted_access_token: encoded,
            encrypted_refresh_token: None,
            access_token_expires_at: None,
            refresh_token_expires_at: None,
            token_version: TOKEN_VERSION,
            kms_key_id: "test-key-1".to_string(),
            last_refreshed_at: Some(100),
        };
        assert_eq!(
            secret.decrypt_access_token(&p),
            Err(TokenError::InvalidPlaintext)
        );
    }

    #[test]
    fn aead_round_trip_access_and_refresh() {
        let p = provider();
        let secret = TokenSecret::encrypt("acct_1", "at-value", Some("rt-value"), &p, 0)
            .unwrap_or_else(|err| panic!("encrypt: {err}"));
        assert_eq!(secret.decrypt_access_token(&p).unwrap(), "at-value");
        assert_eq!(
            secret.decrypt_refresh_token(&p).unwrap(),
            Some("rt-value".to_string())
        );
    }

    #[test]
    fn tampered_ciphertext_returns_decryption_failed() {
        let p = provider();
        let mut secret = TokenSecret::encrypt("acct_1", "token-value", None, &p, 0).unwrap();
        // Flip a byte in the base64 payload.
        let mut bytes = base64::engine::general_purpose::STANDARD
            .decode(&secret.encrypted_access_token)
            .unwrap();
        bytes[20] ^= 0xFF;
        secret.encrypted_access_token = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert_eq!(
            secret.decrypt_access_token(&p),
            Err(TokenError::DecryptionFailed)
        );
    }

    #[test]
    fn two_encryptions_of_same_plaintext_differ() {
        let p = provider();
        let s1 = TokenSecret::encrypt("a", "token", None, &p, 0).unwrap();
        let s2 = TokenSecret::encrypt("a", "token", None, &p, 0).unwrap();
        // Random nonces mean ciphertexts must differ.
        assert_ne!(s1.encrypted_access_token, s2.encrypted_access_token);
    }

    #[test]
    fn v1_token_returns_unsupported_version() {
        let p = provider();
        let secret = TokenSecret {
            connected_account_id: "acct_1".to_string(),
            encrypted_access_token: "anything".to_string(),
            encrypted_refresh_token: None,
            access_token_expires_at: None,
            refresh_token_expires_at: None,
            token_version: 1,
            kms_key_id: "test-key-1".to_string(),
            last_refreshed_at: Some(0),
        };
        assert_eq!(
            secret.decrypt_access_token(&p),
            Err(TokenError::UnsupportedTokenVersion(1))
        );
        assert_eq!(
            secret.decrypt_refresh_token(&p),
            Err(TokenError::UnsupportedTokenVersion(1))
        );
    }

    #[test]
    fn aead256key_from_hex_round_trips() {
        let hex = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
        let key = Aead256Key::from_hex("k1", hex).unwrap();
        let secret = TokenSecret::encrypt("acct", "tok", None, &key, 0).unwrap();
        assert_eq!(secret.decrypt_access_token(&key).unwrap(), "tok");
    }

    #[test]
    fn aead256key_from_hex_rejects_wrong_length() {
        assert_eq!(
            Aead256Key::from_hex("k", "deadbeef").unwrap_err(),
            TokenError::BadKeyLength
        );
    }
}
