//! Performing the actual sign-in actions, all delegated to codex-login.

use std::path::Path;

use codex_login::{LoginServer, ServerOptions, login_with_api_key, run_login_server};

use crate::env::AuthEnv;
use crate::{AuthError, oauth_client_id, validate_api_key};

/// Validate `raw_key`, then persist it as the active credential, replacing any
/// existing auth. The running agent picks it up on its next read of `auth.json`.
pub fn set_api_key(env: &AuthEnv, raw_key: &str) -> Result<(), AuthError> {
    let key = validate_api_key(raw_key)?;
    ensure_home(&env.codex_home)?;
    login_with_api_key(&env.codex_home, &key, env.store_mode).map_err(AuthError::Io)
}

/// Clear stored credentials. Returns whether a credential was present to remove.
pub fn logout(env: &AuthEnv) -> Result<bool, AuthError> {
    codex_login::logout(&env.codex_home, env.store_mode).map_err(AuthError::Io)
}

/// A running "Sign in with ChatGPT" flow: the consent URL plus the local OAuth
/// callback server codex bound for it.
pub struct LoginHandle {
    /// The OpenAI consent URL. codex also opens this in the system browser, but
    /// we surface it so the UI can offer a manual "open this link" fallback when
    /// auto-open fails (e.g. headless / restricted environments).
    pub auth_url: String,
    /// Port the callback server bound — 1455, or the 1457 fallback.
    pub port: u16,
    server: LoginServer,
}

impl LoginHandle {
    /// Wait for the user to complete (or abandon) the browser flow. On success
    /// codex has already written the new ChatGPT credentials to `auth.json`.
    pub async fn wait(self) -> Result<(), AuthError> {
        self.server.block_until_done().await.map_err(AuthError::Io)
    }

    /// Cancel a pending login (e.g. the user closed the dialog).
    pub fn cancel(&self) {
        self.server.cancel();
    }
}

/// Start "Sign in with ChatGPT": bind the local callback server and open the
/// consent page.
///
/// Must be called from within a tokio runtime — the callback server runs as a
/// tokio task. Await [`LoginHandle::wait`] to learn the outcome. The OAuth client
/// id comes from [`oauth_client_id`], so the policy-sensitive reuse of codex's
/// first-party client stays centralized and env-overridable.
pub fn begin_chatgpt_login(env: &AuthEnv) -> Result<LoginHandle, AuthError> {
    ensure_home(&env.codex_home)?;
    let options = ServerOptions::new(
        env.codex_home.clone(),
        oauth_client_id(),
        /* forced_chatgpt_workspace_id */ None,
        env.store_mode,
    );
    let server = run_login_server(options).map_err(AuthError::Io)?;
    Ok(LoginHandle {
        auth_url: server.auth_url.clone(),
        port: server.actual_port,
        server,
    })
}

/// Codex writes `auth.json` into `CODEX_HOME`; create it if a fresh user hasn't
/// run codex yet, so the first sign-in through awidat doesn't fail on a missing
/// directory.
fn ensure_home(codex_home: &Path) -> Result<(), AuthError> {
    std::fs::create_dir_all(codex_home).map_err(AuthError::Io)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use codex_login::{AuthCredentialsStoreMode, load_auth_dot_json};
    use tempfile::TempDir;

    fn temp_env() -> (TempDir, AuthEnv) {
        let home = TempDir::new().unwrap();
        let env = AuthEnv::new(home.path().to_path_buf(), AuthCredentialsStoreMode::File);
        (home, env)
    }

    #[test]
    fn set_api_key_persists_a_valid_key() {
        let (_home, env) = temp_env();
        set_api_key(&env, "  sk-proj-validkey0123456789abc  ").unwrap();

        let stored = load_auth_dot_json(&env.codex_home, env.store_mode)
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.openai_api_key.as_deref(),
            Some("sk-proj-validkey0123456789abc")
        );
    }

    #[test]
    fn set_api_key_rejects_invalid_without_writing() {
        let (_home, env) = temp_env();
        let err = set_api_key(&env, "not-a-key").unwrap_err();
        assert!(matches!(err, AuthError::InvalidApiKey(_)));
        // Nothing should have been written.
        assert!(
            load_auth_dot_json(&env.codex_home, env.store_mode)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn logout_reports_presence_then_absence() {
        let (_home, env) = temp_env();
        set_api_key(&env, "sk-proj-validkey0123456789abc").unwrap();
        assert!(logout(&env).unwrap(), "first logout removes the key");
        assert!(
            !logout(&env).unwrap(),
            "second logout finds nothing to remove"
        );
    }

    #[test]
    fn set_api_key_creates_missing_codex_home() {
        let home = TempDir::new().unwrap();
        let nested = home.path().join("does/not/exist/yet");
        let env = AuthEnv::new(nested.clone(), AuthCredentialsStoreMode::File);
        set_api_key(&env, "sk-proj-validkey0123456789abc").unwrap();
        assert!(nested.join("auth.json").exists());
    }
}
