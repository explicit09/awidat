//! Tauri commands for the publishing subsystem.
//!
//! Thin pass-throughs to the [`ProviderRegistry`] in
//! [`crate::publishing`]. The registry is built once per process
//! (lazily via a `OnceLock`) so commands don't pay the
//! `default_store_path` resolution cost on every invocation.
//!
//! Errors collapse to `String` at the IPC boundary because Tauri
//! commands serialise their `Err` arm via `serde_json::to_value`, and
//! we want a single shape (`{ kind, message }`) the frontend can
//! `JSON.parse` without sniffing for variant tags. We pre-flatten that
//! into one of two forms:
//!
//! - `"<kind>: <message>"` — for kinds that carry a message
//! - `"<kind>"` — for unit-kind errors (`not_configured`, `rate_limited`)
//!
//! The frontend's `splitProviderError(str)` helper (W5.A5) reverses
//! this back into `{ kind, message }`.

use std::sync::OnceLock;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::publishing::{
    AiDisclosure, ClientCredentialsState, ConnectionStatus, OAuthChallenge, ProviderError,
    ProviderInfo, ProviderRegistry, UploadParams, UploadResult, disclosure_for_project_root,
    oauth_listener::{self, OAuthListenerError},
    upload_queue::{
        self, UploadJobEntry, UploadMetadata, UploadPrefs, default_prefs_path, load_prefs_from,
        run_upload, save_prefs_to,
    },
};
use crate::state::MontageState;

/// Tauri event channel: fires once per OAuth flow when the 8419
/// listener (W6.A1) captures the redirect. Frontend subscribes after
/// invoking `begin_provider_oauth` and auto-calls
/// `complete_provider_oauth` with the captured code — replaces the
/// paste-the-code form for the happy path. The paste form stays as a
/// fallback when the listener can't bind / errors out.
///
/// Constant string (rather than `pub const`) to keep the wire shape
/// trivially greppable from the frontend.
pub const OAUTH_CALLBACK_EVENT: &str = "publishing://oauth-callback";

/// Payload of [`OAUTH_CALLBACK_EVENT`]. `provider` is the same key
/// the frontend used in `begin_provider_oauth` so multiple in-flight
/// flows (today: at most one — port 8419 is single-tenant) don't
/// cross-pollinate. `state` is included so the frontend can defence-
/// in-depth re-verify against its own `OAuthChallenge.state` if it
/// wants to. `error` carries the listener error kind (one of
/// `port_in_use` / `state_mismatch` / `invalid_request` / `timeout` /
/// `io`) when the capture failed — frontend uses this to fall back
/// to the paste form with the appropriate copy.
#[derive(Debug, Clone, Serialize)]
pub struct OAuthCallbackEvent {
    /// Provider key the flow was started for.
    pub provider: String,
    /// Captured `code` on success; empty string on error.
    pub code: String,
    /// Echoed-back state nonce; empty string on error.
    pub state: String,
    /// `None` on success; on failure the listener error kind so the
    /// frontend can pick the right fallback copy.
    pub error: Option<String>,
}

/// Process-wide registry. Resolved on first command invocation so
/// the path-resolution error (if `dirs::config_dir()` ever returns
/// `None`) surfaces to the user rather than crashing app startup.
static REGISTRY: OnceLock<Result<ProviderRegistry, String>> = OnceLock::new();

/// Get-or-init the registry. The OnceLock memoises both the success
/// and the failure case, so a transient `config_dir` failure doesn't
/// keep retrying on every IPC call.
fn registry() -> Result<&'static ProviderRegistry, String> {
    REGISTRY
        .get_or_init(|| ProviderRegistry::new().map_err(stringify_error))
        .as_ref()
        .map_err(|e| e.clone())
}

/// Format a `ProviderError` into the `"<kind>: <message>"` /
/// `"<kind>"` wire shape (see module docstring).
fn stringify_error(err: ProviderError) -> String {
    let kind = err.kind();
    match &err {
        ProviderError::NotConfigured | ProviderError::RateLimited => kind.to_string(),
        ProviderError::OAuthFailed(m)
        | ProviderError::NetworkError(m)
        | ProviderError::Unsupported(m)
        | ProviderError::Io(m) => format!("{kind}: {m}"),
    }
}

/// Look up a provider or stringify the "unknown key" failure mode.
fn provider_for<'a>(
    registry: &'a ProviderRegistry,
    key: &str,
) -> Result<&'a dyn crate::publishing::PublishingProvider, String> {
    registry.get(key).ok_or_else(|| {
        format!(
            "unsupported: unknown publishing provider \"{}\". Known: {}",
            key,
            registry
                .iter()
                .map(|p| p.key())
                .collect::<Vec<_>>()
                .join(", "),
        )
    })
}

/// `[{ key, display_name, configured }, …]` for each provider.
#[tauri::command]
pub async fn list_providers() -> Result<Vec<ProviderInfo>, String> {
    Ok(registry()?.list_info().await)
}

/// Start the OAuth flow for one provider. Frontend opens the
/// returned URL; user authorises; provider redirects back to the
/// loopback with `?code=…&state=…`.
///
/// W6.A1: this also kicks the 127.0.0.1:8419 listener in a background
/// task — when the redirect lands, an [`OAUTH_CALLBACK_EVENT`] fires
/// carrying the captured code. The frontend then calls
/// [`complete_provider_oauth`] without the user touching the paste
/// form. If the listener can't bind (another flow is in flight, or
/// the port is squatted) the event still fires with `error =
/// "port_in_use"` so the frontend can drop straight to the fallback
/// paste form instead of waiting on the timeout.
///
/// The listener runs in `tokio::spawn` so this command returns the
/// authorisation URL immediately — the browser handoff happens before
/// the redirect ever lands.
#[tauri::command]
pub async fn begin_provider_oauth(app: AppHandle, key: String) -> Result<OAuthChallenge, String> {
    let reg = registry()?;
    let provider = provider_for(reg, &key)?;
    let challenge = provider.begin_oauth().await.map_err(stringify_error)?;
    spawn_oauth_listener(app, key, challenge.state.clone());
    Ok(challenge)
}

/// Spawn the one-shot listener as a fire-and-forget Tokio task. The
/// only way the listener communicates with the rest of the app is via
/// [`OAUTH_CALLBACK_EVENT`] — emit-on-success / emit-on-failure both
/// land on the same channel with `error` discriminating the cases.
///
/// Split out from `begin_provider_oauth` to keep that command's body
/// readable and to make the listener lifecycle testable in isolation
/// (the listener itself is covered by
/// `publishing::oauth_listener::tests`).
fn spawn_oauth_listener(app: AppHandle, provider: String, expected_state: String) {
    tokio::spawn(async move {
        let result = oauth_listener::listen_for_callback(
            expected_state.clone(),
            oauth_listener::DEFAULT_TIMEOUT,
        )
        .await;
        let payload = match result {
            Ok(capture) => OAuthCallbackEvent {
                provider: provider.clone(),
                code: capture.code,
                state: capture.state,
                error: None,
            },
            Err(err) => {
                tracing::warn!(
                    provider = %provider,
                    kind = err.kind(),
                    error = %err,
                    "oauth listener failed; frontend will fall back to paste form",
                );
                OAuthCallbackEvent {
                    provider: provider.clone(),
                    code: String::new(),
                    state: String::new(),
                    error: Some(listener_error_kind(&err).to_string()),
                }
            }
        };
        if let Err(e) = app.emit(OAUTH_CALLBACK_EVENT, payload) {
            tracing::warn!(
                provider = %provider,
                error = %e,
                "failed to emit oauth-callback event",
            );
        }
    });
}

/// Map an [`OAuthListenerError`] to its stable wire kind. Centralised
/// here (rather than baked into `OAuthListenerError::kind`'s body) so
/// the frontend's switch statement only has to know about these
/// strings — and so a future listener-error variant is a compile-time
/// match-exhaustiveness reminder.
fn listener_error_kind(err: &OAuthListenerError) -> &'static str {
    err.kind()
}

/// Complete the OAuth flow with the authorisation code from the
/// redirect. Provider exchanges it for an access token and persists.
#[tauri::command]
pub async fn complete_provider_oauth(key: String, code: String) -> Result<(), String> {
    let reg = registry()?;
    let provider = provider_for(reg, &key)?;
    provider.complete_oauth(code).await.map_err(stringify_error)
}

/// Current connection status — account name + token expiry where
/// available.
#[tauri::command]
pub async fn get_provider_status(key: String) -> Result<ConnectionStatus, String> {
    let reg = registry()?;
    let provider = provider_for(reg, &key)?;
    Ok(provider.status().await)
}

/// Push a finished render to the platform. Returns
/// `"not_configured"` (kind only) when no credentials are on file —
/// the frontend matches on that to pop the connect-account sheet.
#[tauri::command]
pub async fn upload_via_provider(
    key: String,
    params: UploadParams,
) -> Result<UploadResult, String> {
    let reg = registry()?;
    let provider = provider_for(reg, &key)?;
    provider.upload(params).await.map_err(stringify_error)
}

// -------------------------------------------------------------------
// W5.A2 — per-render upload-queue commands.
// -------------------------------------------------------------------

/// Register the list of provider keys to publish to for one render
/// job. Idempotent — re-registering replaces the targets list and
/// resets per-target state to `Pending`. Empty `providers` clears the
/// fan-out (no auto-upload after render).
#[tauri::command]
pub async fn set_render_upload_targets(
    state: State<'_, MontageState>,
    job_id: String,
    providers: Vec<String>,
) -> Result<(), String> {
    state.upload_queue.register(job_id, providers).await;
    Ok(())
}

/// Snapshot of one render's per-target upload state. Returns `None`
/// (as `null`) when no targets were ever registered for that job —
/// the frontend treats that as "no auto-upload requested".
#[tauri::command]
pub async fn poll_upload_states(
    state: State<'_, MontageState>,
    job_id: String,
) -> Result<Option<UploadJobEntry>, String> {
    Ok(state.upload_queue.snapshot(&job_id).await)
}

/// Every tracked render's upload state. Used by the frontend on app
/// boot to reconcile in-flight uploads after a reload.
#[tauri::command]
pub async fn list_upload_states(
    state: State<'_, MontageState>,
) -> Result<Vec<UploadJobEntry>, String> {
    Ok(state.upload_queue.snapshots().await)
}

/// Kick off uploads for every target registered against `job_id`.
/// Called by the render-queue worker once a render lands at `done`.
///
/// Spawns one tokio task per target so providers fan out
/// independently — a slow YouTube push can't block a fast TikTok push.
///
/// Per-target metadata (W5.A3): if the user filled in the per-target
/// form, `set_upload_metadata` stored it on the [`UploadJobEntry`] —
/// the dispatcher reads from there. If a target has no saved
/// metadata, [`default_upload_params`] supplies the W5.A2 stub
/// (title from `title` arg, no description / tags, `private`).
#[tauri::command]
pub async fn start_uploads_for_job(
    state: State<'_, MontageState>,
    job_id: String,
    file_path: String,
    title: String,
) -> Result<(), String> {
    let Some(entry) = state.upload_queue.snapshot(&job_id).await else {
        // No targets registered — silent no-op so callers don't have to
        // gate on the queue's contents.
        return Ok(());
    };
    let reg = registry()?.clone();
    let queue = state.upload_queue.clone();
    let file_path_buf = std::path::PathBuf::from(&file_path);
    for provider_key in entry.upload_targets {
        // Skip already-terminal targets — re-fires would clobber
        // Published state. Retry path uses `retry_upload` instead.
        let snap = state.upload_queue.snapshot(&job_id).await;
        if let Some(snap) = snap {
            if let Some(st) = snap.upload_states.get(&provider_key) {
                if st.is_terminal() {
                    continue;
                }
            }
        }
        let params =
            build_upload_params(&queue, &job_id, &provider_key, &file_path_buf, &title).await;
        let reg_clone = reg.clone();
        let queue_clone = queue.clone();
        let job_id_clone = job_id.clone();
        let provider_clone = provider_key.clone();
        tokio::spawn(async move {
            run_upload(
                &queue_clone,
                &reg_clone,
                &job_id_clone,
                &provider_clone,
                params,
            )
            .await;
        });
    }
    Ok(())
}

/// Build the [`UploadParams`] for one `(job, provider)` upload —
/// preferring saved per-target metadata, falling back to the W5.A2
/// `default_upload_params` stub. Shared between the initial dispatch
/// (`start_uploads_for_job`) and the retry path so a retried upload
/// reuses the same metadata the original upload was handed.
async fn build_upload_params(
    queue: &crate::publishing::UploadQueue,
    job_id: &str,
    provider: &str,
    file_path: &std::path::Path,
    fallback_title: &str,
) -> UploadParams {
    let mut params = if let Some(metadata) = queue.metadata_for(job_id, provider).await {
        // The form treats title-required as a hard validation error,
        // but the dispatcher defends in depth: if the user somehow
        // submitted an empty title (e.g. saved metadata before the
        // validation lint shipped), fall back to the render label so
        // the provider doesn't reject the call outright.
        let mut params = metadata.into_upload_params(file_path);
        if params.title.trim().is_empty() {
            params.title = fallback_title.to_string();
        }
        params
    } else {
        upload_queue::default_upload_params(file_path, fallback_title)
    };
    // Stamp the per-job AI disclosure onto every target's params.
    // Disclosure is computed once at register time (W5.A4) and shared
    // across all providers — the same cut never lies to one platform
    // and not another about whether it contains generated content.
    params.ai_disclosure = queue.ai_disclosure_for(job_id).await;
    params
}

/// Retry a single failed (or successful — caller's choice) upload.
/// Resets the target to `Pending` and spawns a fresh upload task.
/// Returns `Err` if the job or provider isn't tracked.
///
/// Re-uses any per-target metadata the user configured for the
/// original upload — retry doesn't re-prompt the form.
#[tauri::command]
pub async fn retry_upload(
    state: State<'_, MontageState>,
    job_id: String,
    provider: String,
    file_path: String,
    title: String,
) -> Result<(), String> {
    if !state
        .upload_queue
        .reset_to_pending(&job_id, &provider)
        .await
    {
        return Err(format!(
            "no upload target {provider:?} registered for job {job_id:?}",
        ));
    }
    let reg = registry()?.clone();
    let queue = state.upload_queue.clone();
    let file_path_buf = std::path::PathBuf::from(&file_path);
    let params = build_upload_params(&queue, &job_id, &provider, &file_path_buf, &title).await;
    tokio::spawn(async move {
        run_upload(&queue, &reg, &job_id, &provider, params).await;
    });
    Ok(())
}

/// Persist per-target upload metadata (W5.A3). The frontend's form
/// calls this on every field edit (debounced); the dispatcher reads
/// it back via [`UploadQueue::metadata_for`] in
/// `start_uploads_for_job`. Idempotent — a second call replaces.
///
/// Storing metadata before the render kicks (and therefore before
/// `set_render_upload_targets` has registered the targets list) is
/// supported: [`UploadQueue::set_metadata`] creates a parked shell so
/// the metadata isn't lost.
#[tauri::command]
pub async fn set_upload_metadata(
    state: State<'_, MontageState>,
    job_id: String,
    provider: String,
    metadata: UploadMetadata,
) -> Result<(), String> {
    state
        .upload_queue
        .set_metadata(job_id, provider, metadata)
        .await;
    Ok(())
}

/// Read the user's persisted default upload targets. Empty by default
/// — opt-in only.
#[tauri::command]
pub async fn get_default_upload_targets() -> Result<UploadPrefs, String> {
    let path = default_prefs_path().map_err(stringify_error)?;
    load_prefs_from(&path).await.map_err(stringify_error)
}

/// Persist the user's default upload targets. Writes through the
/// atomic tempfile-rename path so a crash mid-write leaves either the
/// old prefs or the new ones — never a half-written file.
#[tauri::command]
pub async fn set_default_upload_targets(providers: Vec<String>) -> Result<(), String> {
    let path = default_prefs_path().map_err(stringify_error)?;
    let prefs = UploadPrefs {
        default_targets: providers,
    };
    save_prefs_to(&path, &prefs).await.map_err(stringify_error)
}

// -------------------------------------------------------------------
// W5.A4 — AI-disclosure compliance.
// -------------------------------------------------------------------

/// Compute the AI disclosure for the currently-loaded project by
/// walking its OTIO timeline against the generated-media registry.
///
/// Returns the computed disclosure so the frontend can show the
/// banner (and the credits list) right next to the per-target form
/// without a second IPC round trip.
///
/// `auto_disclose` is the user's "Auto-disclose AI content" preference
/// (W5.A4 — default ON, FTC / EU AI Act mandate). When true (default)
/// the computed disclosure is parked on the upload-queue entry so
/// every target's `UploadParams` carries it when the dispatcher runs.
/// When false (power-user override) the disclosure is computed for
/// display only — the parked copy on the queue stays None so the
/// dispatcher passes None to providers, which leaves each platform's
/// flag in its default off state. The banner still surfaces what
/// would have been claimed so the user can manually flag on-platform.
///
/// No project loaded → empty disclosure (no synthetic content). The
/// frontend treats that the same as "clean cut" and renders nothing.
#[tauri::command]
pub async fn compute_ai_disclosure(
    state: State<'_, MontageState>,
    job_id: String,
    auto_disclose: Option<bool>,
) -> Result<AiDisclosure, String> {
    let project_root = state.project_root.lock().await.clone();
    let disclosure = match project_root {
        Some(root) => disclosure_for_project_root(&root),
        None => AiDisclosure::empty(),
    };
    // Default ON — the safe choice and the contract: a missing
    // toggle (older frontend, malformed call) must auto-flag.
    let auto_disclose = auto_disclose.unwrap_or(true);
    let parked = if auto_disclose {
        disclosure.clone()
    } else {
        // Toggle off → park an empty disclosure so the dispatcher
        // doesn't auto-flag, but return the real computed one so
        // the UI can still show the credits + warning banner.
        AiDisclosure::empty()
    };
    state.upload_queue.set_ai_disclosure(job_id, parked).await;
    Ok(disclosure)
}

// -------------------------------------------------------------------
// W5.A5 — Settings → Publishing: disconnect + BYO client credentials.
// -------------------------------------------------------------------

/// Clear OAuth-issued tokens for one provider. Preserves the user's
/// BYO `client_credentials` so the next Connect step doesn't require
/// re-pasting them.
#[tauri::command]
pub async fn disconnect_provider(key: String) -> Result<(), String> {
    let reg = registry()?;
    let provider = provider_for(reg, &key)?;
    provider.disconnect().await.map_err(stringify_error)
}

/// Persist BYO OAuth-app credentials for one provider.
///
/// Currently disabled until the publishing layer stores client secrets
/// in the OS keychain instead of plaintext config.
///
/// Empty strings for either field are rejected: BYO is all-or-nothing,
/// and a partial state would silently fall back to the placeholder
/// `client_id` mid-OAuth (the worst failure mode — the user thinks
/// they're using their own app and isn't).
#[tauri::command]
pub async fn set_provider_client_credentials(
    key: String,
    client_id: String,
    client_secret: String,
) -> Result<(), String> {
    let reg = registry()?;
    let provider = provider_for(reg, &key)?;
    provider
        .set_client_credentials(client_id, client_secret)
        .await
        .map_err(stringify_error)
}

/// Read the *presence* of BYO credentials — booleans only, never the
/// actual `client_secret`. Used to render the "✓ Configured" pip in
/// Settings without round-tripping the secret through IPC.
#[tauri::command]
pub async fn get_provider_client_credentials(
    key: String,
) -> Result<ClientCredentialsState, String> {
    let reg = registry()?;
    let provider = provider_for(reg, &key)?;
    Ok(provider.client_credentials_state().await)
}

/// Resolve the absolute path to `publishing.json` for the Settings
/// "Open folder" affordance + the BYO-credentials warning copy.
#[tauri::command]
pub async fn get_publishing_credentials_path() -> Result<String, String> {
    crate::publishing::storage::default_store_path()
        .map(|p| p.display().to_string())
        .map_err(stringify_error)
}

/// Read the parked AI disclosure for one render job. Used by the
/// frontend on app boot to re-hydrate the disclosure banner without
/// re-walking the timeline. `None` means no disclosure has been
/// computed yet (e.g. the render hasn't started, or the queue was
/// cleared after a reload).
#[tauri::command]
pub async fn get_ai_disclosure_for_job(
    state: State<'_, MontageState>,
    job_id: String,
) -> Result<Option<AiDisclosure>, String> {
    Ok(state.upload_queue.ai_disclosure_for(&job_id).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publishing::Visibility;

    fn stub_params() -> UploadParams {
        UploadParams {
            file_path: "/tmp/fake.mp4".into(),
            title: "t".into(),
            description: String::new(),
            tags: vec![],
            visibility: Visibility::Private,
            scheduled_at: None,
            thumbnail_path: None,
            ai_disclosure: None,
            progress_tx: None,
        }
    }

    #[test]
    fn stringify_error_includes_kind() {
        assert_eq!(
            stringify_error(ProviderError::NotConfigured),
            "not_configured"
        );
        assert_eq!(stringify_error(ProviderError::RateLimited), "rate_limited");
        assert_eq!(
            stringify_error(ProviderError::OAuthFailed("bad code".into())),
            "oauth_failed: bad code",
        );
        assert_eq!(
            stringify_error(ProviderError::Unsupported("real upload not yet".into())),
            "unsupported: real upload not yet",
        );
        assert_eq!(
            stringify_error(ProviderError::NetworkError("dns".into())),
            "network_error: dns",
        );
        assert_eq!(
            stringify_error(ProviderError::Io("disk".into())),
            "io: disk",
        );
    }

    #[tokio::test]
    async fn provider_for_known_key_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = ProviderRegistry::with_store_path(tmp.path().join("publishing.json"));
        // `dyn PublishingProvider` is not Debug, so we can't use
        // `.unwrap()` here without tripping the deny-lint for missing
        // Debug bounds — match-and-assert instead.
        match provider_for(&reg, "youtube") {
            Ok(yt) => assert_eq!(yt.key(), "youtube"),
            Err(e) => panic!("expected youtube provider, got: {e}"),
        }
    }

    #[tokio::test]
    async fn provider_for_unknown_key_lists_options() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = ProviderRegistry::with_store_path(tmp.path().join("publishing.json"));
        let err = match provider_for(&reg, "vimeo") {
            Ok(_) => panic!("vimeo should be unknown"),
            Err(e) => e,
        };
        // The error string surfaces the known providers so the
        // frontend / user can see what they got wrong.
        assert!(err.contains("vimeo"), "{err}");
        assert!(err.contains("youtube"), "{err}");
        assert!(err.contains("tiktok"), "{err}");
        assert!(err.contains("instagram"), "{err}");
    }

    #[tokio::test]
    async fn upload_without_creds_yields_not_configured_kind() {
        // The wire shape the frontend matches on — bare "not_configured"
        // with no colon, because that variant has no message.
        let tmp = tempfile::tempdir().unwrap();
        let reg = ProviderRegistry::with_store_path(tmp.path().join("publishing.json"));
        let yt = match provider_for(&reg, "youtube") {
            Ok(p) => p,
            Err(e) => panic!("expected youtube provider, got: {e}"),
        };
        let err = yt.upload(stub_params()).await.unwrap_err();
        assert_eq!(stringify_error(err), "not_configured");
    }

    // ---- W5.A4: disclosure stamping + provider hint ----

    #[tokio::test]
    async fn build_upload_params_stamps_ai_disclosure() {
        use crate::publishing::{GeneratedMediaCredit, UploadQueue};
        let queue = UploadQueue::new();
        queue
            .register("job-disc".into(), vec!["youtube".into()])
            .await;
        let disclosure = AiDisclosure {
            has_synthetic_content: true,
            credits: vec![GeneratedMediaCredit {
                provider: "runway".into(),
                model: Some("gen3".into()),
                prompt: "neon city".into(),
                generated_at: Some(1),
                asset_id: "a".into(),
            }],
        };
        queue
            .set_ai_disclosure("job-disc".into(), disclosure.clone())
            .await;
        let params = build_upload_params(
            &queue,
            "job-disc",
            "youtube",
            std::path::Path::new("/tmp/clip.mp4"),
            "Fallback",
        )
        .await;
        assert_eq!(params.ai_disclosure.as_ref(), Some(&disclosure));
    }

    #[tokio::test]
    async fn build_upload_params_without_disclosure_passes_none() {
        use crate::publishing::UploadQueue;
        let queue = UploadQueue::new();
        queue
            .register("job-clean".into(), vec!["youtube".into()])
            .await;
        let params = build_upload_params(
            &queue,
            "job-clean",
            "youtube",
            std::path::Path::new("/tmp/clip.mp4"),
            "Fallback",
        )
        .await;
        assert!(params.ai_disclosure.is_none());
    }

    #[tokio::test]
    async fn provider_stub_upload_folds_disclosure_into_unsupported_message() {
        // Brief-named test: with credentials seeded + disclosure flagged,
        // each *stub* provider surfaces the AI flag intent so the user
        // can see what *would* have been claimed on the real call.
        //
        // Post-W6.A3 YouTube is no longer a stub — it short-circuits on
        // the BYO `client_id` check and never reaches the disclosure
        // log line. The YouTube path's disclosure wiring is covered by
        // `youtube_upload::build_metadata_body_sets_synthetic_flag_when_disclosed`
        // instead. TikTok + Instagram remain stubs until their own
        // real-upload tasks ship, so they continue to drive this test.
        use crate::publishing::GeneratedMediaCredit;
        use crate::publishing::keychain::{Keychain, TokenKind, install_mock_backend};
        install_mock_backend();
        let tmp = tempfile::tempdir().unwrap();
        let reg = ProviderRegistry::with_store_path(tmp.path().join("publishing.json"));
        // Seed credentials so the stub providers pass their is_configured
        // gate: metadata in publishing.json, secrets in the (mock) keychain.
        let mut store = crate::publishing::storage::PublishingStore::default();
        let kc = Keychain::new();
        for key in ["tiktok", "instagram"] {
            store.set(
                key,
                Some(crate::publishing::storage::Credentials::default()),
            );
            kc.store_token(key, TokenKind::AccessToken, &format!("seeded-{key}"))
                .unwrap();
        }
        crate::publishing::storage::save_to(&tmp.path().join("publishing.json"), &store)
            .await
            .unwrap();
        let disclosure = AiDisclosure {
            has_synthetic_content: true,
            credits: vec![GeneratedMediaCredit {
                provider: "runway".into(),
                model: Some("gen3".into()),
                prompt: "neon city".into(),
                generated_at: Some(0),
                asset_id: "a".into(),
            }],
        };
        let mut params = stub_params();
        params.ai_disclosure = Some(disclosure);
        // Each stub provider's flag is folded into the Unsupported message.
        let expected = [
            ("tiktok", "aigc_label=true"),
            ("instagram", "ai_label=true"),
        ];
        for (key, hint) in expected {
            let provider = match reg.get(key) {
                Some(p) => p,
                None => panic!("provider {key} missing"),
            };
            let err = provider.upload(params.clone()).await.unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains(hint),
                "{key} should fold {hint} into its Unsupported message; got: {msg}",
            );
            assert!(
                msg.contains("generated"),
                "{key} should mention generated clips; got: {msg}",
            );
        }
    }

    #[tokio::test]
    async fn provider_stub_upload_clean_cut_omits_disclosure_hint() {
        // Same provider-swap rationale as
        // `provider_stub_upload_folds_disclosure_into_unsupported_message`
        // above: YouTube post-W6.A3 doesn't ride the stub path, so we
        // probe TikTok (a remaining stub) for the "no disclosure →
        // baseline message" case.
        use crate::publishing::keychain::{Keychain, TokenKind, install_mock_backend};
        install_mock_backend();
        let tmp = tempfile::tempdir().unwrap();
        let reg = ProviderRegistry::with_store_path(tmp.path().join("publishing.json"));
        let tk = match reg.get("tiktok") {
            Some(p) => p,
            None => panic!("tiktok provider missing"),
        };
        let mut store = crate::publishing::storage::PublishingStore::default();
        store.set(
            "tiktok",
            Some(crate::publishing::storage::Credentials::default()),
        );
        Keychain::new()
            .store_token("tiktok", TokenKind::AccessToken, "seeded-tiktok")
            .unwrap();
        crate::publishing::storage::save_to(&tmp.path().join("publishing.json"), &store)
            .await
            .unwrap();
        // No disclosure → message is the W5.A2 baseline (no AI hint).
        let err = tk.upload(stub_params()).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("aigc_label"),
            "clean cut must not claim disclosure: {msg}"
        );
        assert!(
            !msg.contains("generated clip"),
            "clean cut must not mention generated clips: {msg}"
        );
    }
}
