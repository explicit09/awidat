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

use tauri::State;

use crate::publishing::{
    upload_queue::{
        self, default_prefs_path, load_prefs_from, run_upload, save_prefs_to,
        UploadJobEntry, UploadPrefs,
    },
    ConnectionStatus, OAuthChallenge, ProviderError, ProviderInfo, ProviderRegistry,
    UploadParams, UploadResult,
};
use crate::state::AwidatState;

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
#[tauri::command]
pub async fn begin_provider_oauth(key: String) -> Result<OAuthChallenge, String> {
    let reg = registry()?;
    let provider = provider_for(reg, &key)?;
    provider.begin_oauth().await.map_err(stringify_error)
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
    state: State<'_, AwidatState>,
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
    state: State<'_, AwidatState>,
    job_id: String,
) -> Result<Option<UploadJobEntry>, String> {
    Ok(state.upload_queue.snapshot(&job_id).await)
}

/// Every tracked render's upload state. Used by the frontend on app
/// boot to reconcile in-flight uploads after a reload.
#[tauri::command]
pub async fn list_upload_states(
    state: State<'_, AwidatState>,
) -> Result<Vec<UploadJobEntry>, String> {
    Ok(state.upload_queue.snapshots().await)
}

/// Kick off uploads for every target registered against `job_id`.
/// Called by the render-queue worker once a render lands at `done`.
///
/// Spawns one tokio task per target so providers fan out
/// independently — a slow YouTube push can't block a fast TikTok push.
///
/// Stub metadata (title from render label, no description / tags,
/// `private` visibility) is the contract for W5.A2; W5.A3 replaces it
/// with a per-target form.
#[tauri::command]
pub async fn start_uploads_for_job(
    state: State<'_, AwidatState>,
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
        let params = upload_queue::default_upload_params(&file_path_buf, &title);
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

/// Retry a single failed (or successful — caller's choice) upload.
/// Resets the target to `Pending` and spawns a fresh upload task.
/// Returns `Err` if the job or provider isn't tracked.
#[tauri::command]
pub async fn retry_upload(
    state: State<'_, AwidatState>,
    job_id: String,
    provider: String,
    file_path: String,
    title: String,
) -> Result<(), String> {
    if !state.upload_queue.reset_to_pending(&job_id, &provider).await {
        return Err(format!(
            "no upload target {provider:?} registered for job {job_id:?}",
        ));
    }
    let reg = registry()?.clone();
    let queue = state.upload_queue.clone();
    let file_path_buf = std::path::PathBuf::from(&file_path);
    let params = upload_queue::default_upload_params(&file_path_buf, &title);
    tokio::spawn(async move {
        run_upload(&queue, &reg, &job_id, &provider, params).await;
    });
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
        }
    }

    #[test]
    fn stringify_error_includes_kind() {
        assert_eq!(stringify_error(ProviderError::NotConfigured), "not_configured");
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
}
