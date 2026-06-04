//! Identity/auth seam for multi-user authorization (Phase 7).
//!
//! Pure and dependency-light: it defines how a verified Supabase Auth JWT
//! becomes the already-tested `ApiActor`/`ApiOwner` that `TeamPolicy` checks. It
//! pulls in NO web framework and NO JWT-crypto lib — the actual signature check
//! is behind the `JwtVerifier` trait so the domain crate stays pure and tests
//! inject a fake verifier (same trait-injection style as `LocalTokenKeyProvider`
//! / `UploadAdapter`). The concrete Supabase verifier lives in `supabase_jwt.rs`
//! (server-only).
//!
//! Phase 7 changes only how the actor is *constructed*; how it is *checked*
//! (`api.rs` authorize / `team_service.rs` TeamPolicy) is unchanged.

use crate::api::{ApiActor, ApiOwner};
use crate::model::WorkspaceMemberRole;

/// The verified subset of a Supabase Auth JWT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthClaims {
    /// The Supabase `sub` claim — the stable per-user id.
    pub user_id: String,
    /// The `email` claim, if present.
    pub email: Option<String>,
    /// The `exp` claim (Unix seconds).
    pub expires_at: i64,
}

/// Why a bearer token could not be turned into trusted claims. All variants map
/// to `SocialApiError::Unauthorized` at the HTTP boundary, preserving the
/// redaction/status contract from earlier phases.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AuthContextError {
    #[error("missing bearer token")]
    Missing,
    #[error("token expired")]
    Expired,
    #[error("invalid token signature")]
    InvalidSignature,
    #[error("malformed token claims")]
    MalformedClaims,
}

/// Verifies a raw bearer token into trusted [`AuthClaims`]. Abstracted so the
/// domain crate has no hard dependency on a JWT lib and tests inject a fake.
pub trait JwtVerifier {
    fn verify(&self, bearer: &str, now: i64) -> Result<AuthClaims, AuthContextError>;
}

/// The single seam where verified identity becomes an `ApiActor`: the user id
/// from `sub` plus the workspace roles loaded for that user.
pub fn build_actor(claims: &AuthClaims, roles: Vec<WorkspaceMemberRole>) -> ApiActor {
    ApiActor::new(claims.user_id.clone(), roles)
}

/// The owner representing the signed-in user acting on their own resources.
pub fn owner_for_user(claims: &AuthClaims) -> ApiOwner {
    ApiOwner::user(claims.user_id.clone())
}

/// The owner representing a shared workspace the actor targets.
pub fn owner_for_workspace(workspace_id: &str) -> ApiOwner {
    ApiOwner::workspace(workspace_id.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::model::{OwnerRef, TeamRole, WorkspaceMemberRole};

    /// Fake verifier: returns fixed claims for a known token, errors otherwise.
    struct FakeVerifier {
        result: Result<AuthClaims, AuthContextError>,
    }

    impl JwtVerifier for FakeVerifier {
        fn verify(&self, _bearer: &str, _now: i64) -> Result<AuthClaims, AuthContextError> {
            self.result.clone()
        }
    }

    fn claims() -> AuthClaims {
        AuthClaims {
            user_id: "user_abc".into(),
            email: Some("a@example.com".into()),
            expires_at: 9_999,
        }
    }

    #[test]
    fn build_actor_carries_user_id_and_roles() {
        let roles = vec![WorkspaceMemberRole {
            workspace_id: "ws_1".into(),
            user_id: "user_abc".into(),
            role: TeamRole::Publisher,
        }];
        let actor = build_actor(&claims(), roles.clone());
        assert_eq!(actor.user_id, "user_abc");
        assert_eq!(actor.workspace_roles, roles);
    }

    #[test]
    fn owner_for_user_wraps_the_subject() {
        assert_eq!(
            owner_for_user(&claims()).owner,
            OwnerRef::User("user_abc".into())
        );
    }

    #[test]
    fn owner_for_workspace_wraps_the_workspace_id() {
        assert_eq!(
            owner_for_workspace("ws_9").owner,
            OwnerRef::Workspace("ws_9".into())
        );
    }

    #[test]
    fn fake_verifier_round_trips_claims() {
        let v = FakeVerifier {
            result: Ok(claims()),
        };
        assert_eq!(v.verify("anything", 0).unwrap(), claims());
    }

    #[test]
    fn fake_verifier_surfaces_errors() {
        for err in [
            AuthContextError::Missing,
            AuthContextError::Expired,
            AuthContextError::InvalidSignature,
            AuthContextError::MalformedClaims,
        ] {
            let v = FakeVerifier {
                result: Err(err.clone()),
            };
            assert_eq!(v.verify("t", 0), Err(err));
        }
    }
}
