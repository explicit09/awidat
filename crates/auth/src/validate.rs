//! API-key sanity checking — a gate, not authentication.

use crate::AuthError;

/// Validate and normalize a user-entered OpenAI API key.
///
/// Trims surrounding whitespace, then rejects the obvious mistakes (empty,
/// embedded whitespace, wrong prefix, implausibly short) before the key is ever
/// written to disk. This catches fat-finger errors early; only a real request
/// proves a key actually works.
pub fn validate_api_key(raw: &str) -> Result<String, AuthError> {
    let key = raw.trim();
    if key.is_empty() {
        return Err(AuthError::InvalidApiKey("the key is empty".to_string()));
    }
    if key.chars().any(char::is_whitespace) {
        return Err(AuthError::InvalidApiKey(
            "the key contains whitespace".to_string(),
        ));
    }
    if !key.starts_with("sk-") {
        return Err(AuthError::InvalidApiKey(
            "OpenAI API keys start with \"sk-\"".to_string(),
        ));
    }
    // Real keys are far longer; this only rules out obvious truncation.
    if key.len() < 20 {
        return Err(AuthError::InvalidApiKey(
            "the key looks too short".to_string(),
        ));
    }
    Ok(key.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const VALID: &str = "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789";

    #[test]
    fn accepts_a_well_formed_key() {
        assert_eq!(validate_api_key(VALID).unwrap(), VALID);
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let padded = format!("  {VALID}\n");
        assert_eq!(validate_api_key(&padded).unwrap(), VALID);
    }

    #[test]
    fn rejects_empty_and_whitespace_only() {
        assert!(matches!(
            validate_api_key(""),
            Err(AuthError::InvalidApiKey(_))
        ));
        assert!(matches!(
            validate_api_key("   \t"),
            Err(AuthError::InvalidApiKey(_))
        ));
    }

    #[test]
    fn rejects_embedded_whitespace() {
        let err = validate_api_key("sk-proj-abc def-ghijklmnopqrstuvwxyz").unwrap_err();
        assert!(matches!(err, AuthError::InvalidApiKey(_)));
    }

    #[test]
    fn rejects_wrong_prefix() {
        let err = validate_api_key("pk-abcdefghijklmnopqrstuvwxyz").unwrap_err();
        assert!(matches!(err, AuthError::InvalidApiKey(_)));
    }

    #[test]
    fn rejects_too_short() {
        let err = validate_api_key("sk-short").unwrap_err();
        assert!(matches!(err, AuthError::InvalidApiKey(_)));
    }
}
