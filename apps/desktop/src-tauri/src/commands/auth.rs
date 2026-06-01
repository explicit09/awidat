//! OpenAI auth-mode commands: report status, sign in with ChatGPT, set an API
//! key, sign out.
//!
//! These are thin wrappers over [`awidat_auth`], which drives codex's own
//! `login` crate in-process. All correctness lives in `awidat-auth` (and its
//! unit tests); this layer only resolves the environment, maps errors to the
//! string form the frontend expects, and bridges the async ChatGPT flow to a
//! Tauri event.
//!
//! This auth (who powers the agent) is deliberately separate from the
//! publishing OAuth in [`crate::publishing`] (awidat acting on a user's behalf
//! toward YouTube/TikTok/IG). They share no token store.

use std::sync::atomic::Ordering;

use awidat_auth::{AuthEnv, AuthModeKind, AuthStatus};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::AwidatState;

/// Event emitted when stored credentials change out-of-band (i.e. a ChatGPT
/// login completed in the background). The frontend re-fetches `auth_status`.
const EVENT_AUTH_CHANGED: &str = "auth-changed";
/// Event emitted when a ChatGPT login fails or is abandoned.
const EVENT_AUTH_LOGIN_FAILED: &str = "auth-login-failed";

/// Frontend-facing auth snapshot (camelCase for JS).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatusDto {
    /// Stable mode tag: `chatgpt` | `api_key` | `agent_identity` | `none`.
    pub mode: String,
    /// Short "which wallet" title, e.g. "ChatGPT subscription".
    pub wallet_title: String,
    /// One-sentence explanation of what gets charged.
    pub wallet_detail: String,
    /// Masked credential hint (e.g. `sk-…wxyz`); never the full secret.
    pub account_hint: Option<String>,
    /// True when an environment variable is overriding stored auth.
    pub via_env: bool,
    /// When `via_env`, the name of the overriding variable (e.g. `CODEX_API_KEY`)
    /// so the UI can name the exact variable to unset.
    pub env_var: Option<String>,
}

impl From<AuthStatus> for AuthStatusDto {
    fn from(status: AuthStatus) -> Self {
        Self {
            mode: mode_tag(status.mode).to_string(),
            wallet_title: status.wallet.title,
            wallet_detail: status.wallet.detail,
            account_hint: status.account_hint,
            via_env: status.via_env,
            env_var: status.env_var,
        }
    }
}

fn mode_tag(mode: AuthModeKind) -> &'static str {
    match mode {
        AuthModeKind::ChatGpt => "chatgpt",
        AuthModeKind::ApiKey => "api_key",
        AuthModeKind::AgentIdentity => "agent_identity",
        AuthModeKind::None => "none",
    }
}

/// What the ChatGPT login flow returns to the UI immediately, before the user
/// has finished in the browser.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginChatgptDto {
    /// Consent URL — codex also auto-opens it, but the UI shows it as a manual
    /// fallback.
    pub auth_url: String,
    /// Local callback port codex bound (1455, or 1457 fallback).
    pub port: u16,
}

/// Report the credential the agent will use right now.
#[tauri::command]
pub async fn auth_status() -> Result<AuthStatusDto, String> {
    let env = AuthEnv::resolve().map_err(|e| e.to_string())?;
    Ok(awidat_auth::status(&env).into())
}

/// Validate and store an API key as the active credential, then return the new
/// status.
#[tauri::command]
pub async fn auth_set_api_key(
    state: State<'_, AwidatState>,
    key: String,
) -> Result<AuthStatusDto, String> {
    // Shut down any pending ChatGPT login first so its browser callback can't
    // land after this and overwrite the API key the user just chose.
    cancel_pending_oauth(&state).await;
    let env = AuthEnv::resolve().map_err(|e| e.to_string())?;
    awidat_auth::set_api_key(&env, &key).map_err(|e| e.to_string())?;
    refresh_agent_auth(&state).await;
    Ok(awidat_auth::status(&env).into())
}

/// Clear stored credentials, then return the new status.
#[tauri::command]
pub async fn auth_logout(state: State<'_, AwidatState>) -> Result<AuthStatusDto, String> {
    cancel_pending_oauth(&state).await;
    let env = AuthEnv::resolve().map_err(|e| e.to_string())?;
    awidat_auth::logout(&env).await.map_err(|e| e.to_string())?;
    refresh_agent_auth(&state).await;
    Ok(awidat_auth::status(&env).into())
}

/// Cancel any in-flight ChatGPT OAuth login and *wait for its task to finish*
/// before the caller writes new credentials.
///
/// Invalidating the id stops the task from acting on its result; awaiting the
/// task guarantees codex has finished (or abandoned) persisting `auth.json`, so
/// the caller's subsequent write is the last one — closing the race where a
/// completing browser callback overwrites a just-chosen API key.
async fn cancel_pending_oauth(state: &State<'_, AwidatState>) {
    state.current_oauth_id.store(0, Ordering::SeqCst);
    let pending = state.pending_oauth.lock().await.take();
    if let Some((cancel, task)) = pending {
        cancel.shutdown();
        let _ = task.await;
    }
}

/// Make a credential change take effect on the *running* agent.
///
/// The in-process codex app-server caches auth (codex's own login paths call
/// `AuthManager::reload()` after writing). We don't hold that manager, so we drop
/// the live session instead — `start_turn` lazily relaunches it, picking up the
/// new `auth.json`. If a turn is in flight we can't tear down mid-event, so we
/// set `auth_dirty`; `start_turn` honors it and rebuilds on the next turn.
async fn refresh_agent_auth(state: &State<'_, AwidatState>) {
    if state.turn.lock().await.is_some() {
        state.auth_dirty.store(true, Ordering::SeqCst);
        tracing::info!(
            "auth changed during an active turn; the session will rebuild on the next turn"
        );
        return;
    }
    crate::commands::project::tear_down_codex_session(state).await;
    state.auth_dirty.store(false, Ordering::SeqCst);
}

/// Start "Sign in with ChatGPT". Returns the consent URL immediately; the
/// browser flow completes asynchronously, after which we emit
/// [`EVENT_AUTH_CHANGED`] so the UI refreshes its status.
#[tauri::command]
pub async fn auth_begin_chatgpt(
    app: AppHandle,
    state: State<'_, AwidatState>,
) -> Result<BeginChatgptDto, String> {
    let env = AuthEnv::resolve().map_err(|e| e.to_string())?;
    let handle = awidat_auth::begin_chatgpt_login(&env).map_err(|e| e.to_string())?;
    let dto = BeginChatgptDto {
        auth_url: handle.auth_url.clone(),
        port: handle.port,
    };

    // This login becomes "the current choice"; shut down any prior pending one.
    let id = state.oauth_generation.fetch_add(1, Ordering::SeqCst) + 1;
    state.current_oauth_id.store(id, Ordering::SeqCst);
    if let Some((cancel, _prev_task)) = state.pending_oauth.lock().await.take() {
        cancel.shutdown();
    }
    let cancel = handle.cancel_handle();

    // Wait for the callback off the command thread so the UI gets the URL now.
    let app_for_task = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let result = handle.wait().await;
        let state = app_for_task.state::<AwidatState>();

        // Only act if we're still the current login: a later set-API-key / logout
        // / second sign-in bumps `current_oauth_id`, and we must not then tear the
        // session down or emit auth-changed for a choice the user moved on from.
        if state.current_oauth_id.load(Ordering::SeqCst) != id {
            return;
        }

        match result {
            Ok(()) => {
                // Drop the cached session so the new ChatGPT creds apply next turn.
                refresh_agent_auth(&state).await;
                if let Err(err) = app_for_task.emit(EVENT_AUTH_CHANGED, ()) {
                    tracing::warn!(error = %err, "failed to emit auth-changed");
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "chatgpt login did not complete");
                let _ = app_for_task.emit(EVENT_AUTH_LOGIN_FAILED, err.to_string());
            }
        }
    });
    *state.pending_oauth.lock().await = Some((cancel, task));

    Ok(dto)
}
