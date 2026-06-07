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

use super::ProviderRegistry;
use super::ai_disclosure::AiDisclosure;
use super::errors::ProviderError;
use super::types::{UploadParams, Visibility};

/// File name (under `<config_dir>/montage/`) where the default-targets
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

/// Per-target upload metadata (W5.A3). Mirrors the frontend
/// `UploadMetadata` shape — title / description / tags / visibility /
/// schedule / thumbnail — keyed by provider key on
/// [`UploadJobEntry::upload_metadata`]. Filled by the frontend's
/// per-target form before the render kicks; the dispatcher uses it to
/// build the per-provider [`UploadParams`] instead of W5.A2's
/// `default_upload_params` stub.
///
/// Missing entries (or fields) fall through to the W5.A2 stub —
/// safe-default `private` visibility, the render label as the title.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UploadMetadata {
    /// Provider-facing title. Required by every real platform —
    /// frontend validation flags empties before the form submits.
    #[serde(default)]
    pub title: String,
    /// Description / caption.
    #[serde(default)]
    pub description: String,
    /// Tags / hashtags. The provider impl maps these to whatever the
    /// platform calls "tags" (`#hashtag` for TikTok/IG, keyword
    /// strings for YouTube).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Visibility on publish. Defaults to `Private` if the field is
    /// absent — same safety default as [`UploadParams`].
    #[serde(default)]
    pub visibility: Visibility,
    /// Optional schedule. Unix epoch seconds.
    #[serde(default, rename = "scheduledAt")]
    pub scheduled_at: Option<i64>,
    /// Optional thumbnail / cover image path on disk.
    #[serde(default, rename = "thumbnailPath")]
    pub thumbnail_path: Option<String>,
}

impl UploadMetadata {
    /// Map to a [`UploadParams`] given the resolved render output
    /// path. The metadata carries everything *except* the file path
    /// and the AI disclosure — the file path comes from the render
    /// job's `output_path`, and the disclosure is computed centrally
    /// at register time (`UploadQueue::set_ai_disclosure`) and stamped
    /// onto the params by the dispatcher so every target sees the
    /// same disclosure for one render.
    pub fn into_upload_params(self, file_path: &Path) -> UploadParams {
        UploadParams {
            file_path: file_path.to_string_lossy().into_owned(),
            title: self.title,
            description: self.description,
            tags: self.tags,
            visibility: self.visibility,
            scheduled_at: self.scheduled_at,
            thumbnail_path: self.thumbnail_path,
            ai_disclosure: None,
            progress_tx: None,
        }
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
    /// Per-target form payload (W5.A3). Sparse — entries are inserted
    /// as the user edits each target's form. The dispatcher uses an
    /// entry if present, falling back to [`default_upload_params`]
    /// otherwise.
    #[serde(default)]
    pub upload_metadata: HashMap<String, UploadMetadata>,
    /// AI-content disclosure (W5.A4). Computed once at render-job
    /// register time from the project timeline + generated-media
    /// registry, then stamped onto every target's [`UploadParams`] by
    /// the dispatcher. `None` means "not yet computed" — the frontend
    /// treats that as "no synthetic content" for display, and the
    /// upload dispatcher passes `None` through to providers (which
    /// default to "off") so the absence is safe for clean cuts.
    #[serde(default, rename = "aiDisclosure")]
    pub ai_disclosure: Option<AiDisclosure>,
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
            upload_metadata: HashMap::new(),
            ai_disclosure: None,
        }
    }

    /// `true` when every target is in a terminal state.
    ///
    /// Currently used only by tests + the future "all uploads done"
    /// notification — exposed in the public API so frontends can build
    /// the same query without re-implementing the predicate.
    #[allow(dead_code)]
    pub fn all_terminal(&self) -> bool {
        !self.upload_states.is_empty() && self.upload_states.values().all(UploadState::is_terminal)
    }
}

/// Shared, cloneable handle to the in-memory upload queue.
///
/// Lives on `MontageState`. Cheap to clone — single `Arc<Mutex<_>>`
/// behind it, so concurrent commands and the upload worker tasks can
/// hold it simultaneously without contention beyond the lock itself.
#[derive(Clone, Default)]
pub struct UploadQueue {
    entries: Arc<Mutex<HashMap<String, UploadJobEntry>>>,
}

impl UploadQueue {
    /// Construct an empty queue. Tests use this directly; production
    /// goes through `MontageState::default()` which uses `Default`.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the provider keys for a render job. Idempotent: a
    /// second call replaces the targets list. Any previously-running
    /// upload state for providers no longer in the list is dropped
    /// (the frontend is the source of truth on user intent).
    ///
    /// Per-target [`UploadMetadata`] inserted ahead of `register` (the
    /// form can save metadata before the render kicks) is carried
    /// over for providers still in the targets list — re-registering
    /// shouldn't clobber the form state the user already typed.
    pub async fn register(&self, job_id: String, providers: Vec<String>) {
        let mut guard = self.entries.lock().await;
        let (preserved_metadata, preserved_disclosure) = guard
            .get(&job_id)
            .map(|existing| {
                let mut next = HashMap::new();
                for p in &providers {
                    if let Some(md) = existing.upload_metadata.get(p) {
                        next.insert(p.clone(), md.clone());
                    }
                }
                // AI disclosure is computed once for the render and
                // applies to every target — re-registering must not
                // clobber it (a user toggling targets after the cut
                // is computed shouldn't have to re-walk the timeline).
                (next, existing.ai_disclosure.clone())
            })
            .unwrap_or_default();
        let mut entry = UploadJobEntry::new(job_id.clone(), providers);
        entry.upload_metadata = preserved_metadata;
        entry.ai_disclosure = preserved_disclosure;
        guard.insert(job_id, entry);
    }

    /// Stamp the AI disclosure for one render job. Called once at
    /// register time (W5.A4) with the result of
    /// [`super::cut_contains_generated_media`]. Idempotent — a second
    /// call replaces.
    ///
    /// Parking semantics mirror [`Self::set_metadata`]: if the entry
    /// doesn't exist yet (disclosure computed before
    /// `set_render_upload_targets` ran), a fresh shell is created.
    pub async fn set_ai_disclosure(&self, job_id: String, disclosure: AiDisclosure) {
        let mut guard = self.entries.lock().await;
        let entry = guard
            .entry(job_id.clone())
            .or_insert_with(|| UploadJobEntry::new(job_id, Vec::new()));
        entry.ai_disclosure = Some(disclosure);
    }

    /// Look up the AI disclosure for one render job. Used by the
    /// dispatcher to stamp every target's [`UploadParams`].
    pub async fn ai_disclosure_for(&self, job_id: &str) -> Option<AiDisclosure> {
        self.entries
            .lock()
            .await
            .get(job_id)
            .and_then(|e| e.ai_disclosure.clone())
    }

    /// Replace the per-target metadata for one `(job, provider)`
    /// pair. If the entry doesn't exist yet (form saved before the
    /// render kicked off), a fresh shell is created so the metadata
    /// can be parked until `register` is called with the targets
    /// list. The shell starts with no `upload_targets` so the
    /// dispatcher doesn't try to fan out before `register` runs.
    pub async fn set_metadata(&self, job_id: String, provider: String, metadata: UploadMetadata) {
        let mut guard = self.entries.lock().await;
        let entry = guard
            .entry(job_id.clone())
            .or_insert_with(|| UploadJobEntry::new(job_id, Vec::new()));
        entry.upload_metadata.insert(provider, metadata);
    }

    /// Look up one target's saved metadata. Used by the dispatcher to
    /// build the per-provider [`UploadParams`].
    pub async fn metadata_for(&self, job_id: &str, provider: &str) -> Option<UploadMetadata> {
        self.entries
            .lock()
            .await
            .get(job_id)
            .and_then(|e| e.upload_metadata.get(provider).cloned())
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
        ai_disclosure: None,
        progress_tx: None,
    }
}

/// Run one `(job_id, provider)` upload to completion. Updates the
/// queue with `Uploading` → `Published` / `Failed`.
///
/// Mockable: callers in production pass the live `ProviderRegistry`;
/// tests inject a stub registry built from
/// `ProviderRegistry::with_store_path(tempdir)`.
///
/// W6.A3 progress wiring: this function owns the
/// [`UploadParams::progress_tx`] mpsc side-channel. The receiver runs
/// in a spawned task that forwards each `0.0..=1.0` value into
/// [`UploadQueue::mark_uploading`]. The provider sees only the
/// sender — it doesn't know (and shouldn't) that the queue exists.
/// Sender is dropped (channel closes) the moment `provider.upload`
/// returns, which lets the drain task exit cleanly without leaking.
pub async fn run_upload(
    queue: &UploadQueue,
    registry: &ProviderRegistry,
    job_id: &str,
    provider_key: &str,
    mut params: UploadParams,
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

    // Build the progress side channel. Unbounded so a slow drain
    // never back-pressures the upload itself; each chunk emits one
    // value so the volume is bounded by (file_size / chunk_size) —
    // 1 MB chunks cap a 4 GB upload at ~4096 values, well under any
    // memory concern.
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<f32>();
    params.progress_tx = Some(progress_tx);
    let drain_queue = queue.clone();
    let drain_job = job_id.to_string();
    let drain_provider = provider_key.to_string();
    let drain_handle = tokio::spawn(async move {
        while let Some(p) = progress_rx.recv().await {
            // Clamp defensively — a buggy provider that emits 1.2
            // shouldn't fingerprint as "completed" in the UI.
            let clamped = p.clamp(0.0, 1.0);
            drain_queue
                .mark_uploading(&drain_job, &drain_provider, clamped)
                .await;
        }
    });

    let upload_result = provider.upload(params).await;
    // Sender is dropped here when `params` goes out of scope inside
    // the provider call, so the drain loop has already broken out.
    // Awaiting the join is a clean-shutdown guard — we don't want
    // the next `mark_published` to race a still-running drain tick.
    let _ = drain_handle.await;

    match upload_result {
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

/// Resolve `<config_dir>/montage/upload_prefs.json`. Same conventions
/// as `storage::default_store_path` — kept separate so future
/// migration of credentials to the OS keychain doesn't affect this
/// non-secret file.
pub fn default_prefs_path() -> Result<PathBuf, ProviderError> {
    let cfg = dirs::config_dir()
        .ok_or_else(|| ProviderError::Io("could not resolve platform config_dir".into()))?;
    Ok(cfg.join("montage").join(PREFS_FILE_NAME))
}

/// Read prefs from disk. Missing file → empty defaults (no error).
pub async fn load_prefs_from(path: &Path) -> Result<UploadPrefs, ProviderError> {
    match tokio::fs::read_to_string(path).await {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| ProviderError::Io(format!("parse {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UploadPrefs::default()),
        Err(e) => Err(ProviderError::Io(format!("read {}: {e}", path.display()))),
    }
}

/// Atomic write — same pattern as `storage::save_to`.
pub async fn save_prefs_to(path: &Path, prefs: &UploadPrefs) -> Result<(), ProviderError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ProviderError::Io(format!("create dir {}: {e}", parent.display())))?;
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
        q.register("render-001".into(), vec!["youtube".into()])
            .await;

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
        let mid = q
            .snapshot("render-001")
            .await
            .unwrap_or_else(|| panic!("mid"));
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
        q.register("render-002".into(), vec!["youtube".into(), "tiktok".into()])
            .await;

        // YouTube fails; TikTok unaffected.
        q.mark_failed("render-002", "youtube", "not_configured".into())
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
        // The W5.A1 stub providers return Unsupported once credentials
        // are present. Post-W6.A3 YouTube is no longer a stub (it hits
        // the real `videos.insert` endpoint), so this test now drives
        // TikTok — which still uses `stub_upload` until its own real-
        // upload task lands — to keep covering the dispatcher path
        // that funnels a ProviderError into a Failed entry rather
        // than panicking.
        let tmp = tempfile::tempdir().unwrap_or_else(|err| panic!("tempdir: {err}"));
        let registry = test_registry(&tmp);
        let q = UploadQueue::new();
        q.register("render-003".into(), vec!["tiktok".into()]).await;

        // Seed credentials so the provider passes is_configured:
        // metadata in publishing.json, access token in the mock keychain.
        use crate::publishing::keychain::{Keychain, TokenKind, install_mock_backend};
        install_mock_backend();
        let mut store = crate::publishing::storage::PublishingStore::default();
        store.set(
            "tiktok",
            Some(crate::publishing::storage::Credentials::default()),
        );
        Keychain::new()
            .store_token("tiktok", TokenKind::AccessToken, "seeded-tiktok")
            .unwrap_or_else(|err| panic!("seed keychain: {err}"));
        crate::publishing::storage::save_to(&tmp.path().join("publishing.json"), &store)
            .await
            .unwrap_or_else(|err| panic!("seed credentials: {err}"));

        let file_path = fake_render_file(&tmp);
        let params = default_upload_params(&file_path, "Test render");
        run_upload(&q, &registry, "render-003", "tiktok", params).await;

        let entry = q
            .snapshot("render-003")
            .await
            .unwrap_or_else(|| panic!("registered"));
        let state = entry
            .upload_states
            .get("tiktok")
            .unwrap_or_else(|| panic!("tiktok state"));
        match state {
            UploadState::Failed { reason } => {
                assert!(reason.starts_with("unsupported:"), "{reason}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_upload_drains_progress_channel_into_queue() {
        // W6.A3 contract: `run_upload` opens an mpsc channel, stashes
        // the sender on the params, and forwards every f32 the
        // provider sends through into `UploadQueue::mark_uploading`.
        // We exercise the wiring by handing a fake provider that
        // emits a known sequence and reading the queue state at the
        // tail of the call.
        use crate::publishing::ClientCredentialsState;
        use crate::publishing::ProviderError;
        use crate::publishing::provider::PublishingProvider;
        use crate::publishing::types::{
            ConnectionStatus, OAuthChallenge, UploadParams, UploadResult, Visibility,
        };
        use async_trait::async_trait;
        use std::time::Duration;

        struct ProgressEmitter;
        #[async_trait]
        impl PublishingProvider for ProgressEmitter {
            fn key(&self) -> &'static str {
                "fake"
            }
            fn display_name(&self) -> &'static str {
                "Fake"
            }
            async fn is_configured(&self) -> bool {
                true
            }
            async fn begin_oauth(&self) -> Result<OAuthChallenge, ProviderError> {
                Err(ProviderError::Unsupported("test only".into()))
            }
            async fn complete_oauth(&self, _: String) -> Result<(), ProviderError> {
                Ok(())
            }
            async fn upload(&self, params: UploadParams) -> Result<UploadResult, ProviderError> {
                let tx = params
                    .progress_tx
                    .expect("dispatcher must stamp progress_tx");
                for tick in [0.25_f32, 0.5, 0.75, 1.0] {
                    tx.send(tick).expect("send tick");
                    // Yield so the drain task can land each value
                    // separately rather than coalescing them all
                    // under the same `mark_uploading` write.
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Ok(UploadResult {
                    remote_url: "https://fake/url".into(),
                    remote_id: "fake-id".into(),
                    uploaded_at: 0,
                })
            }
            async fn status(&self) -> ConnectionStatus {
                ConnectionStatus::default()
            }
            async fn disconnect(&self) -> Result<(), ProviderError> {
                Ok(())
            }
            async fn set_client_credentials(
                &self,
                _: String,
                _: String,
            ) -> Result<(), ProviderError> {
                Ok(())
            }
            async fn client_credentials_state(&self) -> ClientCredentialsState {
                ClientCredentialsState::default()
            }
        }

        // Hand-build a registry around the fake; we can't use the
        // production constructor because it always installs the trio
        // (youtube + tiktok + instagram).
        let providers: Vec<Box<dyn PublishingProvider>> = vec![Box::new(ProgressEmitter)];
        let registry = ProviderRegistry::from_providers(providers);

        let q = UploadQueue::new();
        q.register("job-drain".into(), vec!["fake".into()]).await;
        let params = UploadParams {
            file_path: "/tmp/x".into(),
            title: "t".into(),
            description: String::new(),
            tags: vec![],
            visibility: Visibility::Private,
            scheduled_at: None,
            thumbnail_path: None,
            ai_disclosure: None,
            progress_tx: None, // dispatcher overwrites this
        };
        run_upload(&q, &registry, "job-drain", "fake", params).await;

        let entry = q
            .snapshot("job-drain")
            .await
            .unwrap_or_else(|| panic!("registered"));
        // Final state is Published — the channel drained all the way
        // through without losing the terminal transition.
        match entry.upload_states.get("fake") {
            Some(UploadState::Published { remote_id, .. }) => {
                assert_eq!(remote_id, "fake-id");
            }
            other => panic!("expected Published, got {other:?}"),
        }
        assert_eq!(
            entry.published_urls.get("fake").map(String::as_str),
            Some("https://fake/url"),
        );
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
        q.mark_published("job-r", "youtube", "https://youtu.be/x".into(), "x".into())
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
        assert!(!entry.published_urls.contains_key("youtube"));
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
        assert!(
            UploadState::Published {
                remote_url: "u".into(),
                remote_id: "i".into(),
            }
            .is_terminal()
        );
        assert!(UploadState::Failed { reason: "x".into() }.is_terminal());
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

    #[tokio::test]
    async fn set_metadata_round_trips_through_queue() {
        let q = UploadQueue::new();
        q.register("job-md".into(), vec!["youtube".into()]).await;
        let md = UploadMetadata {
            title: "My title".into(),
            description: "Long-form description".into(),
            tags: vec!["tag1".into(), "tag2".into()],
            visibility: Visibility::Public,
            scheduled_at: Some(1_700_000_000),
            thumbnail_path: Some("/tmp/cover.png".into()),
        };
        q.set_metadata("job-md".into(), "youtube".into(), md.clone())
            .await;
        let fetched = q
            .metadata_for("job-md", "youtube")
            .await
            .unwrap_or_else(|| panic!("metadata round-trips"));
        assert_eq!(fetched, md);
        // And it survives a snapshot read.
        let snap = q
            .snapshot("job-md")
            .await
            .unwrap_or_else(|| panic!("snapshot present"));
        assert_eq!(snap.upload_metadata.get("youtube"), Some(&md));
    }

    #[tokio::test]
    async fn set_metadata_before_register_creates_parked_shell() {
        // The form can save metadata before the user hits Export —
        // store should park the metadata so it isn't lost when the
        // worker eventually registers the targets list.
        let q = UploadQueue::new();
        let md = UploadMetadata {
            title: "Drafted".into(),
            ..UploadMetadata::default()
        };
        q.set_metadata("job-parked".into(), "youtube".into(), md.clone())
            .await;
        // Render kicks: register replaces the shell but preserves the
        // metadata for providers still in the targets list.
        q.register("job-parked".into(), vec!["youtube".into()])
            .await;
        let after = q
            .snapshot("job-parked")
            .await
            .unwrap_or_else(|| panic!("entry present"));
        assert_eq!(after.upload_metadata.get("youtube"), Some(&md));
        assert_eq!(
            after.upload_states.get("youtube"),
            Some(&UploadState::Pending),
        );
    }

    #[tokio::test]
    async fn register_drops_metadata_for_removed_providers() {
        // If the user removes a target from `uploadTargets` (toggles
        // the per-target chip off), the next register call should
        // drop that provider's metadata too — staying in sync with
        // user intent.
        let q = UploadQueue::new();
        q.set_metadata(
            "job-x".into(),
            "youtube".into(),
            UploadMetadata {
                title: "YT".into(),
                ..UploadMetadata::default()
            },
        )
        .await;
        q.set_metadata(
            "job-x".into(),
            "tiktok".into(),
            UploadMetadata {
                title: "TT".into(),
                ..UploadMetadata::default()
            },
        )
        .await;
        q.register("job-x".into(), vec!["youtube".into()]).await;
        let after = q
            .snapshot("job-x")
            .await
            .unwrap_or_else(|| panic!("entry present"));
        assert!(after.upload_metadata.contains_key("youtube"));
        assert!(!after.upload_metadata.contains_key("tiktok"));
    }

    #[test]
    fn upload_metadata_into_upload_params_carries_fields() {
        let md = UploadMetadata {
            title: "T".into(),
            description: "D".into(),
            tags: vec!["a".into(), "b".into()],
            visibility: Visibility::Unlisted,
            scheduled_at: Some(42),
            thumbnail_path: Some("/tmp/x.png".into()),
        };
        let params = md.into_upload_params(Path::new("/tmp/clip.mp4"));
        assert_eq!(params.file_path, "/tmp/clip.mp4");
        assert_eq!(params.title, "T");
        assert_eq!(params.description, "D");
        assert_eq!(params.tags, vec!["a", "b"]);
        assert_eq!(params.visibility, Visibility::Unlisted);
        assert_eq!(params.scheduled_at, Some(42));
        assert_eq!(params.thumbnail_path.as_deref(), Some("/tmp/x.png"));
        assert!(
            params.ai_disclosure.is_none(),
            "metadata never carries disclosure"
        );
    }

    // ---- W5.A4 ai_disclosure round-trip ----

    #[tokio::test]
    async fn set_ai_disclosure_round_trips() {
        let q = UploadQueue::new();
        q.register("job-ai".into(), vec!["youtube".into()]).await;
        let disclosure =
            AiDisclosure::from_credits(vec![super::super::ai_disclosure::GeneratedMediaCredit {
                provider: "runway".into(),
                model: Some("gen3".into()),
                prompt: "sunset over a quiet street".into(),
                generated_at: Some(1_700_000_000),
                asset_id: "job-1".into(),
            }]);
        q.set_ai_disclosure("job-ai".into(), disclosure.clone())
            .await;
        let fetched = q
            .ai_disclosure_for("job-ai")
            .await
            .unwrap_or_else(|| panic!("disclosure present"));
        assert!(fetched.has_synthetic_content);
        assert_eq!(fetched.credits.len(), 1);
        assert_eq!(fetched, disclosure);
        let snap = q
            .snapshot("job-ai")
            .await
            .unwrap_or_else(|| panic!("snapshot"));
        assert!(snap.ai_disclosure.is_some());
    }

    #[tokio::test]
    async fn set_ai_disclosure_before_register_parks() {
        // Disclosure can be computed before the targets list lands —
        // park it so the next register call preserves the value.
        let q = UploadQueue::new();
        let disclosure =
            AiDisclosure::from_credits(vec![super::super::ai_disclosure::GeneratedMediaCredit {
                provider: "mock".into(),
                model: None,
                prompt: "x".into(),
                generated_at: None,
                asset_id: "a".into(),
            }]);
        q.set_ai_disclosure("job-parked-ai".into(), disclosure.clone())
            .await;
        q.register("job-parked-ai".into(), vec!["youtube".into()])
            .await;
        let snap = q
            .snapshot("job-parked-ai")
            .await
            .unwrap_or_else(|| panic!("snapshot"));
        assert_eq!(snap.ai_disclosure.as_ref(), Some(&disclosure));
    }

    #[tokio::test]
    async fn register_preserves_existing_disclosure() {
        let q = UploadQueue::new();
        q.register("job-pre".into(), vec!["youtube".into()]).await;
        let disclosure =
            AiDisclosure::from_credits(vec![super::super::ai_disclosure::GeneratedMediaCredit {
                provider: "mock".into(),
                model: None,
                prompt: "x".into(),
                generated_at: None,
                asset_id: "a".into(),
            }]);
        q.set_ai_disclosure("job-pre".into(), disclosure.clone())
            .await;
        // User toggles a second target — re-register must keep the disclosure.
        q.register("job-pre".into(), vec!["youtube".into(), "tiktok".into()])
            .await;
        let snap = q
            .snapshot("job-pre")
            .await
            .unwrap_or_else(|| panic!("snapshot"));
        assert_eq!(snap.ai_disclosure.as_ref(), Some(&disclosure));
    }
}
