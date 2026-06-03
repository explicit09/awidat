use crate::model::{OwnerRef, Provider};
use base64::Engine;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthConnection {
    pub id: String,
    pub owner: OwnerRef,
    pub provider: Provider,
    pub state_hash: String,
    pub requested_scopes: Vec<String>,
    pub return_to: String,
    pub status: OAuthConnectionStatus,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthConnectionStatus {
    Started,
    Completed,
    Failed,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OAuthCallbackError {
    Expired,
    StateMismatch,
    AlreadyCompleted,
}

impl OAuthConnection {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        id: impl Into<String>,
        owner: OwnerRef,
        provider: Provider,
        raw_state: &str,
        requested_scopes: Vec<String>,
        return_to: String,
        created_at: i64,
        expires_at: i64,
    ) -> Self {
        Self {
            id: id.into(),
            owner,
            provider,
            state_hash: hash_state(raw_state),
            requested_scopes,
            return_to,
            status: OAuthConnectionStatus::Started,
            created_at,
            expires_at,
        }
    }

    pub fn matches_state(&self, raw_state: &str) -> bool {
        self.state_hash == hash_state(raw_state)
    }

    pub fn validate_callback(&self, raw_state: &str, now: i64) -> Result<(), OAuthCallbackError> {
        if self.status != OAuthConnectionStatus::Started {
            return Err(OAuthCallbackError::AlreadyCompleted);
        }
        if now > self.expires_at {
            return Err(OAuthCallbackError::Expired);
        }
        if !self.matches_state(raw_state) {
            return Err(OAuthCallbackError::StateMismatch);
        }
        Ok(())
    }
}

fn hash_state(raw_state: &str) -> String {
    let digest = Sha256::digest(raw_state.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> OAuthConnection {
        OAuthConnection::start(
            "oauth_1",
            OwnerRef::User("user_1".into()),
            Provider::YouTube,
            "state-secret",
            vec!["youtube.upload".into()],
            "/campaigns/campaign_1".into(),
            100,
            200,
        )
    }

    #[test]
    fn start_hashes_state_instead_of_storing_raw_state() {
        let conn = connection();
        assert_ne!(conn.state_hash, "state-secret");
        assert!(conn.matches_state("state-secret"));
        assert!(!conn.matches_state("wrong"));
    }

    #[test]
    fn validate_callback_rejects_expired_sessions() {
        let conn = connection();
        let err = match conn.validate_callback("state-secret", 201) {
            Ok(()) => panic!("expected expired callback to fail"),
            Err(err) => err,
        };
        assert_eq!(err, OAuthCallbackError::Expired);
    }

    #[test]
    fn validate_callback_rejects_state_mismatch() {
        let conn = connection();
        let err = match conn.validate_callback("wrong", 150) {
            Ok(()) => panic!("expected state mismatch to fail"),
            Err(err) => err,
        };
        assert_eq!(err, OAuthCallbackError::StateMismatch);
    }

    #[test]
    fn validate_callback_rejects_completed_sessions() {
        let mut conn = connection();
        conn.status = OAuthConnectionStatus::Completed;
        let err = match conn.validate_callback("state-secret", 150) {
            Ok(()) => panic!("expected completed callback to fail"),
            Err(err) => err,
        };
        assert_eq!(err, OAuthCallbackError::AlreadyCompleted);
    }
}
