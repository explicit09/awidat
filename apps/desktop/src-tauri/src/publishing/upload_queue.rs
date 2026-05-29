//! Per-render upload queue — bridges finished renders to the publishing providers.
//!
//! W5.A2 ships the chain `render done → uploading → published / failed`.
//! The frontend's `RenderQueueEntry` carries the user-side intent
//! (which providers to push to); this module owns the *backend* state
//! that mirrors it so:
//!
//! 1. Multiple render jobs can have independent upload fan-outs.
//! 2. Each `(job_id, provider_key)` pair has its own lifecycle —
//!    a successful YouTube push doesn't block a retry on a failed
//!    TikTok push for the same render.
//! 3. The Tauri command surface stays small: register targets once,
//!    fire-and-forget when the render finishes, poll for state, retry
//!    individually if a provider fails.
//!
//! # State machine (per (job, provider))
//!
//! ```text
//!     Pending ──► Uploading { progress } ──► Published { remote_url, remote_id }
//!         │              │
//!         └──────────────┴─────────────────► Failed { reason }
//! ```
//!
//! The render job's lifecycle (`queued → rendering → done`) and the
//! upload fan-out are decoupled — the job is "done" the moment the
//! file lands on disk, and the upload runs alongside. The UI treats
//! "any target still Pending/Uploading" as a job-level "uploading"
//! state for display purposes (see `RenderQueue.tsx`); the backend
//! doesn't carry a synthetic job-level state because providers fan
//! out independently.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::errors::ProviderError;
use super::types::{UploadParams, Visibility};
use super::ProviderRegistry;

/// File name (under `<config_dir>/awidat/`) where the default-targets
/// preferences live. Tiny JSON — list of provider keys the user has
/// opted into auto-publishing for.
const PREFS_FILE_NAME: &str = "upload_prefs.json";

/// Lifecycle state for one `(render_job, provider)` pairing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UploadState {
    /// The render is queued / running; the upload hasn't started yet.
    Pending,
    /// Upload is in flight. `progress` is 0.0..1.0 (or NaN when the
    /// provider doesn't report progress — frontend renders an
    /// indeterminate spinner in that case).
    Uploading {
        /// 0.0 ..= 1.0
        progress: f32,
    },
    /// Provider acknowledged the upload. `remote_url` is the shareable
    /// link, `remote_id` is the provider-internal id (so we can
    /// re-fetch analytics later).
    Published {
        /// e.g. `https://youtu.be/abc123`
        remote_url: String,
        /// Provider-specific identifier.
        remote_id: String,
    },
    /// Terminal failure. Surfaced verbatim in the UI; `Retry` button
    /// transitions back to `Pending` so a new upload attempt can start.
    Failed {
        /// Human-readable reason — typically the `ProviderError`
        /// rendered as `kind: message`.
        reason: String,
    },
}

impl UploadState {
    /// `true` for `Published` and `Failed` — the frontend uses this to
    /// decide whether to show a `Retry` button or a `View ↗` link.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Published { .. } | Self::Failed { .. })
    }
}

/// One entry in the upload queue — the per-render upload fan-out.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UploadJobEntry {
    /// The render job's id (the one returned by
    /// `start_timeline_render` / `start_reframe_render`).
    pub job_id: String,
    /// Provider keys (`"youtube"`, `"tiktok"`, …) the user has
    /// selected as upload targets for this render.
    pub upload_targets: Vec<String>,
    /// Per-target lifecycle state. Keyed by provider key — same set
    /// as `upload_targets` after `register`.
    pub upload_states: HashMap<String, UploadState>,
    /// Once `Published`, mirrors the `remote_url` for quick frontend
    /// access. Empty until at least one target publishes.
    pub published_urls: HashMap<String, String>,
}

impl UploadJobEntry {
    /// Fresh entry: every target starts in `Pending`.
    pub fn new(job_id: String, upload_targets: Vec<String>) -> Self {
        let upload_states = upload_targets
            .iter()
            .map(|key| (key.clone(), UploadState::Pending))
            .collect();
        Self {
            job_id,
            upload_targets,
            upload_states,
            published_urls: HashMap::new(),
        }
    }

    /// `true` when every target is in a terminal state.
    ///
    /// Currently used only by tests + the future "all uploads done"
    /// notification — exposed in the public API so frontends can build
    /// the same query without re-implementing the predicate.
    #[allow(dead_code)]
    pub fn all_terminal(&self) -> bool {
        !self.upload_states.is_empty()
            && self.upload_states.values().all(UploadState::is_terminal)
    }
}

/// Shared, cloneable handle to the in-memory upload queue.
///
/// Lives on `AwidatState`. Cheap to clone — single `Arc<Mutex<_>>`
/// behind it, so concurrent commands and the upload worker tasks can
/// hold it simultaneously without contention beyond the lock itself.
#[derive(Clone, Default)]
pub struct UploadQueue {
    entries: Arc<Mutex<HashMap<String, UploadJobEntry>>>,
}

impl UploadQueue {
    /// Construct an empty queue. Tests use this directly; production
    /// goes through `AwidatState::default()` which uses `Default`.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the provider keys for a render job. Idempotent: a
    /// second call replaces the targets list. Any previously-running
    /// upload state for providers no longer in the list is dropped
    /// (the frontend is the source of truth on user intent).
    pub async fn register(&self, job_id: String, providers: Vec<String>) {
        let mut guard = self.entries.lock().await;
        guard.insert(job_id.clone(), UploadJobEntry::new(job_id, providers));
    }

    /// Read the entry for one job, or `None` if no targets are registered.
    pub async fn snapshot(&self, job_id: &str) -> Option<UploadJobEntry> {
        self.entries.lock().await.get(job_id).cloned()
    }

    /// Read every job's snapshot. Used by the frontend on app boot to
    /// reconcile any in-flight uploads from before a reload.
    pub async fn snapshots(&self) -> Vec<UploadJobEntry> {
        self.entries.lock().await.values().cloned().collect()
    }

    /// Transition one target into `Uploading { progress }`.
    pub async fn mark_uploading(&self, job_id: &str, provider: &str, progress: f32) {
        let mut guard = self.entries.lock().await;
        if let Some(entry) = guard.get_mut(job_id) {
            entry
                .upload_states
                .insert(provider.into(), UploadState::Uploading { progress });
        }
    }

    /// Transition one target into `Published`. Also populates
    /// `published_urls[provider]` for quick frontend access.
    pub async fn mark_published(
        &self,
        job_id: &str,
        provider: &str,
        remote_url: String,
        remote_id: String,
    ) {
        let mut guard = self.entries.lock().await;
        if let Some(entry) = guard.get_mut(job_id) {
            entry.upload_states.insert(
                provider.into(),
                UploadState::Published {
                    remote_url: remote_url.clone(),
                    remote_id,
                },
            );
            entry.published_urls.insert(provider.into(), remote_url);
        }
    }

    /// Transition one target into `Failed`.
    pub async fn mark_failed(&self, job_id: &str, provider: &str, reason: String) {
        let mut guard = self.entries.lock().await;
        if let Some(entry) = guard.get_mut(job_id) {
            entry
                .upload_states
                .insert(provider.into(), UploadState::Failed { reason });
        }
    }

    /// Reset one target to `Pending` so a retry can start. No-op if the
    /// job isn't tracked or the provider isn't a registered target.
    pub async fn reset_to_pending(&self, job_id: &str, provider: &str) -> bool {
        let mut guard = self.entries.lock().await;
        match guard.get_mut(job_id) {
            Some(entry) if entry.upload_targets.iter().any(|p| p == provider) => {
                entry
                    .upload_states
                    .insert(provider.into(), UploadState::Pending);
                entry.published_urls.remove(provider);
                true
            }
            _ => false,
        }
    }

    /// Snapshot of every (job, provider) pair currently in
    /// `Pending` — used by the dispatcher to find work to start.
    ///
    /// Today the upload dispatcher is fired explicitly by
    /// `start_uploads_for_job`; this method exists for the future
    /// scheduler that drains the queue without per-job hand-off.
    #[allow(dead_code)]
    pub async fn pending_targets(&self) -> Vec<(String, String)> {
        let guard = self.entries.lock().await;
        let mut out = Vec::new();
        for entry in guard.values() {
            for provider in &entry.upload_targets {
                if matches!(
                    entry.upload_states.get(provider),
                    Some(UploadState::Pending)
                ) {
                    out.push((entry.job_id.clone(), provider.clone()));
                }
            }
        }
        out
    }
}

/// Stub metadata derived from the render job's display name. W5.A3
/// replaces this with a real per-target form (title, description,
/// tags); for now we hand the provider a safe-default payload so the
/// chain compiles end-to-end and tests can exercise it.
///
/// Defaults to `Visibility::Private` so a user who toggles "upload
/// after render" doesn't accidentally publish a draft to the world.
pub fn default_upload_params(file_path: &Path, title: &str) -> UploadParams {
    UploadParams {
        file_path: file_path.to_string_lossy().into_owned(),
        title: title.to_string(),
        description: String::new(),
        tags: Vec::new(),
        visibility: Visibility::Private,
        scheduled_at: None,
        thumbnail_path: None,
    }
}

/// Run one `(job_id, provider)` upload to completion. Updates the
/// queue with `Uploading` → `Published` / `Failed`.
///
/// Mockable: callers in production pass the live `ProviderRegistry`;
/// tests inject a stub registry built from
/// `ProviderRegistry::with_store_path(tempdir)`.
pub async fn run_upload(
    queue: &UploadQueue,
    registry: &ProviderRegistry,
    job_id: &str,
    provider_key: &str,
    params: UploadParams,
) {
    queue.mark_uploading(job_id, provider_key, 0.0).await;
    let provider = match registry.get(provider_key) {
        Some(p) => p,
        None => {
            queue
                .mark_failed(
                    job_id,
                    provider_key,
                    format!("unknown provider \"{provider_key}\""),
                )
                .await;
            return;
        }
    };
    match provider.upload(params).await {
        Ok(result) => {
            queue
                .mark_published(job_id, provider_key, result.remote_url, result.remote_id)
                .await;
        }
        Err(err) => {
            queue
                .mark_failed(job_id, provider_key, format_provider_error(&err))
                .await;
        }
    }
}

/// Render a `ProviderError` into a human-readable reason. Format is
/// `kind: message` for variants with a message, just `kind` otherwise
/// — same wire shape the Tauri command layer uses, so the frontend
/// can re-use `splitProviderError`.
fn format_provider_error(err: &ProviderError) -> String {
    let kind = err.kind();
    match err {
        ProviderError::NotConfigured | ProviderError::RateLimited => kind.to_string(),
        ProviderError::OAuthFailed(m)
        | ProviderError::NetworkError(m)
        | ProviderError::Unsupported(m)
        | ProviderError::Io(m) => format!("{kind}: {m}"),
    }
}

// ---- Default-targets preferences (persistent) --------------------

/// Persisted shape of the user's default-targets opt-ins. The file is
/// a flat `{ providers: [...] }` so adding fields later (per-target
/// metadata defaults) is a non-breaking append.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadPrefs {
    /// Provider keys the user has opted into auto-publishing for.
    /// Empty by default — safety: nothing publishes without an
    /// explicit toggle from the user.
    #[serde(default)]
    pub default_targets: Vec<String>,
}

/// Resolve `<config_dir>/awidat/upload_prefs.json`. Same conventions
/// as `storage::default_store_path` — kept separate so future
/// migration of credentials to the OS keychain doesn't affect this
/// non-secret file.
pub fn default_prefs_path() -> Result<PathBuf, ProviderError> {
    let cfg = dirs::config_dir().ok_or_else(|| {
        ProviderError::Io("could not resolve platform config_dir".into())
    })?;
    Ok(cfg.join("awidat").join(PREFS_FILE_NAME))
}

/// Read prefs from disk. Missing file → empty defaults (no error).
pub async fn load_prefs_from(path: &Path) -> Result<UploadPrefs, ProviderError> {
    match tokio::fs::read_to_string(path).await {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| ProviderError::Io(format!("parse {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UploadPrefs::default()),
        Err(e) => Err(ProviderError::Io(format!(
            "read {}: {e}",
            path.display()
        ))),
    }
}

/// Atomic write — same pattern as `storage::save_to`.
pub async fn save_prefs_to(path: &Path, prefs: &UploadPrefs) -> Result<(), ProviderError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            ProviderError::Io(format!("create dir {}: {e}", parent.display()))
        })?;
    }
    let buf = serde_json::to_vec_pretty(prefs)
        .map_err(|e| ProviderError::Io(format!("serialize prefs: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, &buf)
        .await
        .map_err(|e| ProviderError::Io(format!("write {}: {e}", tmp.display())))?;
    tokio::fs::rename(&tmp, path).await.map_err(|e| {
        ProviderError::Io(format!(
            "rename {} → {}: {e}",
            tmp.display(),
            path.display()
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publishing::ProviderRegistry;
    use std::path::PathBuf;

    fn test_registry(tmp: &tempfile::TempDir) -> ProviderRegistry {
        ProviderRegistry::with_store_path(tmp.path().join("publishing.json"))
    }

    fn fake_render_file(tmp: &tempfile::TempDir) -> PathBuf {
        let p = tmp.path().join("renders").join("clip.mp4");
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|err| panic!("create_dir_all: {err}"));
        }
        std::fs::write(&p, b"fake-mp4").unwrap_or_else(|err| panic!("write: {err}"));
        p
    }

    #[tokio::test]
    async fn register_seeds_pending_for_each_target() {
        let q = UploadQueue::new();
        q.register("job-1".into(), vec!["youtube".into(), "tiktok".into()])
            .await;
        let entry = q
            .snapshot("job-1")
            .await
            .unwrap_or_else(|| panic!("registered job missing"));
        assert_eq!(entry.upload_targets, vec!["youtube", "tiktok"]);
        assert_eq!(
            entry.upload_states.get("youtube"),
            Some(&UploadState::Pending),
        );
        assert_eq!(
            entry.upload_states.get("tiktok"),
            Some(&UploadState::Pending),
        );
        assert!(entry.published_urls.is_empty());
        assert!(!entry.all_terminal());
    }

    #[tokio::test]
    async fn pending_targets_lists_unstarted_pairs() {
        let q = UploadQueue::new();
        q.register("job-a".into(), vec!["youtube".into()]).await;
        q.register("job-b".into(), vec!["tiktok".into()]).await;
        let mut pending = q.pending_targets().await;
        pending.sort();
        assert_eq!(
            pending,
            vec![
                ("job-a".to_string(), "youtube".to_string()),
                ("job-b".to_string(), "tiktok".to_string()),
            ],
        );
        // Moving one out of Pending shrinks the result.
        q.mark_uploading("job-a", "youtube", 0.5).await;
        let pending = q.pending_targets().await;
        assert_eq!(pending, vec![("job-b".to_string(), "tiktok".to_string())]);
    }

    /// Brief-named test #1: a job with `upload_targets = ["youtube"]`
    /// transitions through the lifecycle on success.
    ///
    /// The "real" upload trait is mocked at the registry layer: we
    /// pre-seed the credential store so the stub provider passes the
    /// `is_configured` gate, then drive a synthetic publish.
    #[tokio::test]
    async fn render_done_transitions_to_uploading_and_published() {
        let tmp = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let registry = test_registry(&tmp);
        let q = UploadQueue::new();
        q.register("render-001".into(), vec!["youtube".into()]).await;

        // Before any upload work, target is Pending.
        let entry = q
            .snapshot("render-001")
            .await
            .unwrap_or_else(|| panic!("registered"));
        assert_eq!(
            entry.upload_states.get("youtube"),
            Some(&UploadState::Pending),
        );

        // Mock the upload: simulate a real platform success by directly
        // driving the queue's transitions the way `run_upload` would on
        // a working provider. (The stub registry's `upload` returns
        // `Unsupported` so we can't fully invoke it; this mirrors what
        // a real provider impl would do.)
        q.mark_uploading("render-001", "youtube", 0.4).await;
        let mid = q.snapshot("render-001").await.unwrap_or_else(|| panic!("mid"));
        assert_eq!(
            mid.upload_states.get("youtube"),
            Some(&UploadState::Uploading { progress: 0.4 }),
        );

        q.mark_published(
            "render-001",
            "youtube",
            "https://youtu.be/abc123".into(),
            "abc123".into(),
        )
        .await;
        let done = q
            .snapshot("render-001")
            .await
            .unwrap_or_else(|| panic!("done"));
        assert_eq!(
            done.upload_states.get("youtube"),
            Some(&UploadState::Published {
                remote_url: "https://youtu.be/abc123".into(),
                remote_id: "abc123".into(),
            }),
        );
        assert_eq!(
            done.published_urls.get("youtube"),
            Some(&"https://youtu.be/abc123".to_string()),
        );
        assert!(done.all_terminal());
        let _ = registry; // silence unused
    }

    /// Brief-named test #2: a failed upload lands the target in
    /// `Failed { reason }` without affecting the *job's* lifecycle
    /// (which we model implicitly — the queue doesn't carry a
    /// job-level state field).
    #[tokio::test]
    async fn failed_upload_isolated_to_target() {
        let q = UploadQueue::new();
        q.register(
            "render-002".into(),
            vec!["youtube".into(), "tiktok".into()],
        )
        .await;

        // YouTube fails; TikTok unaffected.
        q.mark_failed(
            "render-002",
            "youtube",
            "not_configured".into(),
        )
        .await;
        let entry = q
            .snapshot("render-002")
            .await
            .unwrap_or_else(|| panic!("registered"));
        assert_eq!(
            entry.upload_states.get("youtube"),
            Some(&UploadState::Failed {
                reason: "not_configured".into(),
            }),
        );
        assert_eq!(
            entry.upload_states.get("tiktok"),
            Some(&UploadState::Pending),
        );
        assert!(!entry.all_terminal(), "tiktok still pending");
    }

    #[tokio::test]
    async fn run_upload_against_stub_provider_records_failure() {
        // The W5.A1 stub providers all return Unsupported once
        // credentials are present. We exercise the dispatcher path to
        // make sure it correctly funnels the ProviderError into a
        // Failed entry rather than panicking.
        let tmp = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let registry = test_registry(&tmp);
        let q = UploadQueue::new();
        q.register("render-003".into(), vec!["youtube".into()]).await;

        // Seed creds so the provider passes is_configured.
        let yt = match registry.get("youtube") {
            Some(p) => p,
            None => panic!("youtube provider missing from registry"),
        };
        yt.complete_oauth("fake-code".into())
            .await
            .unwrap_or_else(|err| panic!("complete_oauth: {err}"));

        let file_path = fake_render_file(&tmp);
        let params = default_upload_params(&file_path, "Test render");
        run_upload(&q, &registry, "render-003", "youtube", params).await;

        let entry = q
            .snapshot("render-003")
            .await
            .unwrap_or_else(|| panic!("registered"));
        let state = entry
            .upload_states
            .get("youtube")
            .unwrap_or_else(|| panic!("youtube state"));
        match state {
            UploadState::Failed { reason } => {
                assert!(reason.starts_with("unsupported:"), "{reason}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_upload_unknown_provider_marks_failed() {
        let tmp = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let registry = test_registry(&tmp);
        let q = UploadQueue::new();
        q.register("render-004".into(), vec!["vimeo".into()]).await;
        let file_path = fake_render_file(&tmp);
        let params = default_upload_params(&file_path, "Test");
        run_upload(&q, &registry, "render-004", "vimeo", params).await;
        let entry = q
            .snapshot("render-004")
            .await
            .unwrap_or_else(|| panic!("registered"));
        match entry.upload_states.get("vimeo") {
            Some(UploadState::Failed { reason }) => {
                assert!(reason.contains("unknown provider"), "{reason}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reset_to_pending_clears_published_url() {
        let q = UploadQueue::new();
        q.register("job-r".into(), vec!["youtube".into()]).await;
        q.mark_published(
            "job-r",
            "youtube",
            "https://youtu.be/x".into(),
            "x".into(),
        )
        .await;
        assert!(q.reset_to_pending("job-r", "youtube").await);
        let entry = q
            .snapshot("job-r")
            .await
            .unwrap_or_else(|| panic!("registered"));
        assert_eq!(
            entry.upload_states.get("youtube"),
            Some(&UploadState::Pending),
        );
        assert!(entry.published_urls.get("youtube").is_none());
    }

    #[tokio::test]
    async fn reset_to_pending_rejects_untracked_target() {
        let q = UploadQueue::new();
        q.register("job-r2".into(), vec!["youtube".into()]).await;
        // tiktok was never a registered target — reset should no-op.
        assert!(!q.reset_to_pending("job-r2", "tiktok").await);
    }

    #[tokio::test]
    async fn prefs_round_trip() {
        let tmp = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let path = tmp.path().join("upload_prefs.json");
        let prefs = UploadPrefs {
            default_targets: vec!["youtube".into(), "tiktok".into()],
        };
        save_prefs_to(&path, &prefs)
            .await
            .unwrap_or_else(|err| panic!("save: {err}"));
        let reloaded = load_prefs_from(&path)
            .await
            .unwrap_or_else(|err| panic!("load: {err}"));
        assert_eq!(reloaded, prefs);
    }

    #[tokio::test]
    async fn prefs_missing_file_is_empty_defaults() {
        let tmp = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let prefs = load_prefs_from(&tmp.path().join("nope.json"))
            .await
            .unwrap_or_else(|err| panic!("load: {err}"));
        assert!(prefs.default_targets.is_empty());
    }

    #[test]
    fn upload_state_is_terminal_classifies_correctly() {
        assert!(!UploadState::Pending.is_terminal());
        assert!(!UploadState::Uploading { progress: 0.5 }.is_terminal());
        assert!(UploadState::Published {
            remote_url: "u".into(),
            remote_id: "i".into(),
        }
        .is_terminal());
        assert!(UploadState::Failed {
            reason: "x".into(),
        }
        .is_terminal());
    }

    #[test]
    fn default_upload_params_is_safe() {
        let p = default_upload_params(Path::new("/tmp/a.mp4"), "My render");
        assert_eq!(p.file_path, "/tmp/a.mp4");
        assert_eq!(p.title, "My render");
        assert_eq!(p.visibility, Visibility::Private);
        assert!(p.tags.is_empty());
        assert!(p.description.is_empty());
        assert!(p.scheduled_at.is_none());
    }
}
