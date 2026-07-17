//! Performing the actual sign-in actions, all delegated to codex-login.

use std::path::Path;

use codex_login::{
    AuthKeyringBackendKind, LoginServer, ServerOptions, ShutdownHandle, login_with_api_key,
    logout_with_revoke, run_login_server,
};

use crate::env::{AuthEnv, ForcedMethod};
use crate::{AuthError, oauth_client_id, validate_api_key};

/// Validate `raw_key`, then persist it as the active credential, replacing any
/// existing auth. The running agent picks it up on its next read of `auth.json`.
///
/// Rejected on a managed install that forces ChatGPT login, mirroring codex's own
/// `forced_login_method` enforcement so the chooser can't switch a locked install
/// to API-key billing.
pub fn set_api_key(env: &AuthEnv, raw_key: &str) -> Result<(), AuthError> {
    if env.forced_login_method == Some(ForcedMethod::ChatGpt) {
        return Err(AuthError::ForbiddenByPolicy(
            "this install requires Sign in with ChatGPT; API keys are not allowed".to_string(),
        ));
    }
    let key = validate_api_key(raw_key)?;
    ensure_home(&env.codex_home)?;
    login_with_api_key(
        &env.codex_home,
        &key,
        env.store_mode,
        AuthKeyringBackendKind::default(),
    )
    .map_err(AuthError::Io)
}

/// Clear stored credentials. Returns whether a credential was present to remove.
///
/// Uses codex's *revoking* logout so a ChatGPT refresh-token grant is invalidated
/// server-side (matching `codex logout`), not just deleted locally — otherwise a
/// copied/restored `auth.json` would keep working. It still deletes locally even
/// if the network revoke fails, so offline sign-out keeps working.
pub async fn logout(env: &AuthEnv) -> Result<bool, AuthError> {
    logout_with_revoke(
        &env.codex_home,
        env.store_mode,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
    )
    .await
    .map_err(AuthError::Io)
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

    /// A cloneable handle to cancel this login from elsewhere — e.g. so a later
    /// API-key save / logout can shut the pending callback server down before its
    /// browser flow completes and silently overwrites the newer choice.
    pub fn cancel_handle(&self) -> ShutdownHandle {
        self.server.cancel_handle()
    }
}

/// Start "Sign in with ChatGPT": bind the local callback server and open the
/// consent page.
///
/// Must be called from within a tokio runtime — the callback server runs as a
/// tokio task. Await [`LoginHandle::wait`] to learn the outcome. The OAuth client
/// id comes from [`oauth_client_id`], so public builds fail clearly unless a
/// sanctioned OAuth client id is explicitly configured.
///
/// Rejected on a managed install that forces API-key login, and any configured
/// `forced_chatgpt_workspace_id` allow-list is passed to the callback so
/// credentials from other workspaces are refused — mirroring codex.
pub fn begin_chatgpt_login(env: &AuthEnv) -> Result<LoginHandle, AuthError> {
    if env.forced_login_method == Some(ForcedMethod::Api) {
        return Err(AuthError::ForbiddenByPolicy(
            "this install requires an API key; ChatGPT sign-in is not allowed".to_string(),
        ));
    }
    ensure_home(&env.codex_home)?;
    let options = ServerOptions::new(
        env.codex_home.clone(),
        oauth_client_id()?,
        env.forced_workspace_ids.clone(),
        env.store_mode,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
    );
    let server = run_login_server(options).map_err(AuthError::Io)?;
    Ok(LoginHandle {
        auth_url: server.auth_url.clone(),
        port: server.actual_port,
        server,
    })
}

/// Codex writes `auth.json` into `CODEX_HOME`; create it if a fresh user hasn't
/// run codex yet, so the first sign-in through montage doesn't fail on a missing
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

        let stored = load_auth_dot_json(
            &env.codex_home,
            env.store_mode,
            AuthKeyringBackendKind::default(),
        )
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
            load_auth_dot_json(
                &env.codex_home,
                env.store_mode,
                AuthKeyringBackendKind::default()
            )
            .unwrap()
            .is_none()
        );
    }

    #[tokio::test]
    async fn logout_reports_presence_then_absence() {
        let (_home, env) = temp_env();
        // API-key auth has no ChatGPT tokens, so the revoking logout makes no
        // network call here — the test stays offline.
        set_api_key(&env, "sk-proj-validkey0123456789abc").unwrap();
        assert!(logout(&env).await.unwrap(), "first logout removes the key");
        assert!(
            !logout(&env).await.unwrap(),
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

    #[test]
    fn set_api_key_blocked_when_chatgpt_is_forced() {
        let home = TempDir::new().unwrap();
        let mut env = AuthEnv::new(home.path().to_path_buf(), AuthCredentialsStoreMode::File);
        env.forced_login_method = Some(ForcedMethod::ChatGpt);
        let err = set_api_key(&env, "sk-proj-validkey0123456789abc").unwrap_err();
        assert!(matches!(err, AuthError::ForbiddenByPolicy(_)));
        assert!(
            load_auth_dot_json(
                &env.codex_home,
                env.store_mode,
                AuthKeyringBackendKind::default()
            )
            .unwrap()
            .is_none(),
            "no key should be written when policy forbids it"
        );
    }

    #[test]
    fn begin_chatgpt_blocked_when_api_is_forced() {
        let home = TempDir::new().unwrap();
        let mut env = AuthEnv::new(home.path().to_path_buf(), AuthCredentialsStoreMode::File);
        env.forced_login_method = Some(ForcedMethod::Api);
        // `LoginHandle` isn't `Debug`, so match instead of `unwrap_err()`.
        let result = begin_chatgpt_login(&env);
        assert!(matches!(result, Err(AuthError::ForbiddenByPolicy(_))));
    }
}
