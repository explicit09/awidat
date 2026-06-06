//! Supabase Auth JWT verification (Phase 7, server-only).
//!
//! Implements the domain `JwtVerifier` trait so the `/social/*` routes can turn
//! a desktop-forwarded `Authorization: Bearer <supabase-jwt>` into trusted
//! `AuthClaims` (→ `ApiActor` via `auth_context::build_actor`). The JWT secret
//! is read from server env only (`SUPABASE_JWT_SECRET`) and is never shipped to
//! the desktop.
//!
//! Mode: HS256 with the project's JWT secret (Supabase's classic shared-secret
//! signing). This is fully testable with a locally-minted token. Asymmetric
//! (JWKS RS256/ES256) verification is a documented follow-up — it needs a
//! network fetch of the project JWKS and a cache, and is only required once a
//! project opts into asymmetric signing keys.

use awidat_social::auth_context::{AuthClaims, AuthContextError, JwtVerifier};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::Deserialize;

/// The subset of a Supabase Auth JWT we trust after verification.
#[derive(Debug, Deserialize)]
struct SupabaseClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    exp: i64,
}

/// HS256 verifier backed by the project's `SUPABASE_JWT_SECRET`.
pub struct SupabaseJwtVerifier {
    key: DecodingKey,
    validation: Validation,
}

impl SupabaseJwtVerifier {
    /// Build from the raw HS256 secret. Validates `exp` and `aud=authenticated`
    /// (Supabase's audience for signed-in users).
    pub fn new_hs256(secret: &str) -> Self {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_audience(&["authenticated"]);
        // We validate `exp` ourselves against the caller-supplied `now` (so the
        // check is deterministic and testable) rather than the library's wall
        // clock. The signature + audience checks still run.
        validation.validate_exp = false;
        Self {
            key: DecodingKey::from_secret(secret.as_bytes()),
            validation,
        }
    }
}

impl JwtVerifier for SupabaseJwtVerifier {
    fn verify(&self, bearer: &str, now: i64) -> Result<AuthClaims, AuthContextError> {
        let token = bearer.trim();
        if token.is_empty() {
            return Err(AuthContextError::Missing);
        }
        let data = decode::<SupabaseClaims>(token, &self.key, &self.validation).map_err(|e| {
            use jsonwebtoken::errors::ErrorKind;
            match e.kind() {
                ErrorKind::ExpiredSignature => AuthContextError::Expired,
                ErrorKind::InvalidSignature
                | ErrorKind::InvalidAlgorithm
                | ErrorKind::InvalidAlgorithmName => AuthContextError::InvalidSignature,
                _ => AuthContextError::MalformedClaims,
            }
        })?;

        let claims = data.claims;
        // Defense-in-depth explicit expiry check against the supplied clock.
        if claims.exp <= now {
            return Err(AuthContextError::Expired);
        }
        if claims.sub.trim().is_empty() {
            return Err(AuthContextError::MalformedClaims);
        }
        Ok(AuthClaims {
            user_id: claims.sub,
            email: claims.email,
            expires_at: claims.exp,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde::Serialize;

    const SECRET: &str = "test-supabase-jwt-secret";

    #[derive(Serialize)]
    struct TestClaims {
        sub: String,
        email: Option<String>,
        aud: String,
        exp: i64,
    }

    fn mint(sub: &str, aud: &str, exp: i64) -> String {
        let claims = TestClaims {
            sub: sub.into(),
            email: Some("u@example.com".into()),
            aud: aud.into(),
            exp,
        };
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap()
    }

    #[test]
    fn valid_token_verifies_to_claims() {
        let v = SupabaseJwtVerifier::new_hs256(SECRET);
        let token = mint("user_abc", "authenticated", 10_000);
        let claims = v.verify(&token, 1_000).unwrap();
        assert_eq!(claims.user_id, "user_abc");
        assert_eq!(claims.email.as_deref(), Some("u@example.com"));
        assert_eq!(claims.expires_at, 10_000);
    }

    #[test]
    fn wrong_secret_is_invalid_signature() {
        let other = SupabaseJwtVerifier::new_hs256("a-different-secret");
        let token = mint("user_abc", "authenticated", 10_000);
        assert_eq!(
            other.verify(&token, 1_000),
            Err(AuthContextError::InvalidSignature)
        );
    }

    #[test]
    fn expired_token_is_rejected() {
        let v = SupabaseJwtVerifier::new_hs256(SECRET);
        // exp in the past relative to `now` and to the wire clock.
        let token = mint("user_abc", "authenticated", 500);
        assert_eq!(v.verify(&token, 1_000), Err(AuthContextError::Expired));
    }

    #[test]
    fn wrong_audience_is_rejected() {
        let v = SupabaseJwtVerifier::new_hs256(SECRET);
        let token = mint("user_abc", "anon", 10_000);
        // Wrong audience fails validation → mapped to MalformedClaims.
        assert!(v.verify(&token, 1_000).is_err());
    }

    #[test]
    fn empty_bearer_is_missing() {
        let v = SupabaseJwtVerifier::new_hs256(SECRET);
        assert_eq!(v.verify("   ", 1_000), Err(AuthContextError::Missing));
    }
}
