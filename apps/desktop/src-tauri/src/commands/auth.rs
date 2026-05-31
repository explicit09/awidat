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
    /// True when an `OPENAI_API_KEY` env var is overriding stored auth.
    pub via_env: bool,
}

impl From<AuthStatus> for AuthStatusDto {
    fn from(status: AuthStatus) -> Self {
        Self {
            mode: mode_tag(status.mode).to_string(),
            wallet_title: status.wallet.title,
            wallet_detail: status.wallet.detail,
            account_hint: status.account_hint,
            via_env: status.via_env,
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
    let env = AuthEnv::resolve().map_err(|e| e.to_string())?;
    awidat_auth::set_api_key(&env, &key).map_err(|e| e.to_string())?;
    refresh_agent_auth(&state).await;
    Ok(awidat_auth::status(&env).into())
}

/// Clear stored credentials, then return the new status.
#[tauri::command]
pub async fn auth_logout(state: State<'_, AwidatState>) -> Result<AuthStatusDto, String> {
    let env = AuthEnv::resolve().map_err(|e| e.to_string())?;
    awidat_auth::logout(&env).map_err(|e| e.to_string())?;
    refresh_agent_auth(&state).await;
    Ok(awidat_auth::status(&env).into())
}

/// Make a credential change take effect on the *running* agent.
///
/// The in-process codex app-server caches auth (codex's own login paths call
/// `AuthManager::reload()` after writing). We don't hold that manager, so we drop
/// the live session instead — `start_turn` lazily relaunches it, picking up the
/// new `auth.json`. Skipped while a turn is in flight so we don't kill its pump
/// task mid-event: the new credential is already persisted and applies on the
/// next session rebuild.
async fn refresh_agent_auth(state: &State<'_, AwidatState>) {
    if state.turn.lock().await.is_some() {
        tracing::info!(
            "auth changed during an active turn; new credentials apply on the next session rebuild"
        );
        return;
    }
    crate::commands::project::tear_down_codex_session(state).await;
}

/// Start "Sign in with ChatGPT". Returns the consent URL immediately; the
/// browser flow completes asynchronously, after which we emit
/// [`EVENT_AUTH_CHANGED`] so the UI refreshes its status.
#[tauri::command]
pub async fn auth_begin_chatgpt(app: AppHandle) -> Result<BeginChatgptDto, String> {
    let env = AuthEnv::resolve().map_err(|e| e.to_string())?;
    let handle = awidat_auth::begin_chatgpt_login(&env).map_err(|e| e.to_string())?;
    let dto = BeginChatgptDto {
        auth_url: handle.auth_url.clone(),
        port: handle.port,
    };

    // Wait for the callback off the command thread so the UI gets the URL now.
    tauri::async_runtime::spawn(async move {
        match handle.wait().await {
            Ok(()) => {
                // Drop the cached session so the new ChatGPT creds apply next turn.
                refresh_agent_auth(&app.state::<AwidatState>()).await;
                if let Err(err) = app.emit(EVENT_AUTH_CHANGED, ()) {
                    tracing::warn!(error = %err, "failed to emit auth-changed");
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "chatgpt login did not complete");
                let _ = app.emit(EVENT_AUTH_LOGIN_FAILED, err.to_string());
            }
        }
    });

    Ok(dto)
}
