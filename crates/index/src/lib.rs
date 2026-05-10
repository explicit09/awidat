//! Indexer dispatcher: launches MCP servers, calls `index_asset(asset)` on
//! each, writes sidecars + manifest. SHA-256 idempotency.
//!
//! Per `PLAN.md` §5.4 + `INDEX_SCHEMA.md`:
//! - One sidecar per `(indexer, asset)` at `index/<indexer>/<asset>.json`.
//! - Manifest at `index/manifest.json` is the registry.
//! - Re-running on an unchanged asset (matching `asset_sha256`) is a no-op.
//! - Failure of one `(indexer, asset)` pair doesn't kill the run; it logs
//!   and continues.
//!
//! The dispatcher is async and dispatches `(indexer × asset)` pairs in
//! parallel via `tokio::spawn` + `futures::stream::FuturesUnordered`. The
//! Rust engine never sees indexer-specific data — the body of every
//! sidecar is `serde_json::Value` to us.

#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use awidat_config::{IndexerResourceClass, McpServer};
use awidat_mcp::{Client, ClientInfo, ServerConfig};
use awidat_proto::index::{AssetId, IndexSidecar, IndexerEntry, Manifest};
use awidat_proto::project::files;
use chrono::Utc;
use futures::stream::{FuturesUnordered, StreamExt};
use thiserror::Error;
use tokio::sync::{Mutex, oneshot};
use tracing::{info, warn};

mod manifest_io;
mod sha;
pub mod sidecar_io;

pub use manifest_io::{read_manifest, write_manifest};
pub use sha::asset_sha256;
pub use sidecar_io::{SidecarError, read_sidecar, sidecar_path, walk_indexer};

/// Errors from the indexer dispatcher. Per-asset / per-indexer errors land
/// in [`IndexReport::failures`] instead.
#[derive(Debug, Error)]
pub enum IndexError {
    /// Project root doesn't exist or isn't a directory.
    #[error("project root '{0}' is not a directory")]
    BadRoot(String),
    /// Failed to read or write the manifest.
    #[error("manifest I/O at '{path}': {source}")]
    ManifestIo {
        /// File.
        path: String,
        /// Underlying.
        #[source]
        source: std::io::Error,
    },
    /// Failed to write a sidecar to disk.
    #[error("sidecar I/O at '{path}': {source}")]
    SidecarIo {
        /// File.
        path: String,
        /// Underlying.
        #[source]
        source: std::io::Error,
    },
    /// Asset id failed validation (path traversal, etc.). See
    /// [`AssetId::sidecar_relative_path`].
    #[error("invalid asset id '{asset}': {message}")]
    InvalidAssetId {
        /// Bad asset id.
        asset: String,
        /// Reason.
        message: String,
    },
}

/// Outcome of one `(indexer, asset)` pair.
#[derive(Debug, Clone)]
pub enum PairOutcome {
    /// Sidecar already up-to-date (matching `asset_sha256`); indexer not
    /// re-run.
    Skipped {
        /// Indexer that was skipped.
        indexer: String,
        /// Asset id.
        asset: AssetId,
        /// Timing / resource telemetry for this pair.
        telemetry: PairTelemetry,
    },
    /// Sidecar written or refreshed.
    Wrote {
        /// Indexer name.
        indexer: String,
        /// Asset id.
        asset: AssetId,
        /// Path the sidecar was written to.
        path: PathBuf,
        /// Timing / resource telemetry for this pair.
        telemetry: PairTelemetry,
    },
    /// Indexer failed for this asset. Run continues; manifest is not updated
    /// for this pair.
    Failed {
        /// Indexer name.
        indexer: String,
        /// Asset id.
        asset: AssetId,
        /// Diagnostic.
        message: String,
        /// Timing / resource telemetry for this pair.
        telemetry: PairTelemetry,
    },
    /// Indexer was not run because one of its declared `depends_on`
    /// indexers failed (or was itself skipped via this same path)
    /// for this asset. We skip rather than launch so the user sees
    /// one root-cause failure per asset, not a cascade of identical
    /// "missing prerequisite sidecar" errors.
    SkippedDep {
        /// Indexer name.
        indexer: String,
        /// Asset id.
        asset: AssetId,
        /// Names of failed prerequisite indexers (in declaration
        /// order, deduplicated).
        missing: Vec<String>,
        /// Timing / resource telemetry for this pair.
        telemetry: PairTelemetry,
    },
}

/// Per-pair indexing telemetry. Durations are wall-clock measurements
/// gathered by the dispatcher; RSS is best-effort and currently available
/// only where the platform lets us cheaply sample child process memory.
#[derive(Debug, Default, Clone, Copy)]
pub struct PairTelemetry {
    /// Time spent waiting in the scheduler after asset hashing completed.
    pub queued: Duration,
    /// Child process spawn + MCP initialize time.
    pub launch_init: Duration,
    /// `index_asset` tool runtime.
    pub tool: Duration,
    /// JSON serialization + sidecar write time.
    pub write: Duration,
    /// Total wall time from scheduler enqueue to terminal outcome.
    pub total: Duration,
    /// Peak resident set size for the child process, in bytes, when known.
    pub peak_rss_bytes: Option<u64>,
}

impl PairOutcome {
    /// Access the telemetry regardless of outcome variant.
    pub fn telemetry(&self) -> &PairTelemetry {
        match self {
            PairOutcome::Skipped { telemetry, .. }
            | PairOutcome::Wrote { telemetry, .. }
            | PairOutcome::Failed { telemetry, .. }
            | PairOutcome::SkippedDep { telemetry, .. } => telemetry,
        }
    }
}

/// One progress event emitted during a [`run`] invocation when the
/// caller supplies a [`ProgressCallback`]. The CLI ignores progress
/// (it prints the final report); the desktop translates these into
/// protocol items so the GUI can render a live progress bar.
///
/// Events are best-effort and unordered with respect to actual disk
/// I/O — they fire when the dispatcher loop observes state changes,
/// not when the underlying MCP server returns a chunk. Pair `total`
/// is `indexers.len() * assets.len()` modulo the dep-skipped subset
/// resolved synchronously up front; treat it as the upper bound.
#[derive(Debug, Clone)]
pub enum IndexProgress {
    /// Emitted once at the start of [`run`], after assets have been
    /// hashed but before any pair launches. Carries the total pair
    /// count the run will attempt to execute.
    Started {
        /// Number of `(indexer, asset)` pairs that will be attempted.
        total: usize,
    },
    /// Emitted as soon as a pair finishes (Wrote / Skipped / Failed /
    /// SkippedDep). `completed` is the running count of pairs that
    /// have reached any terminal state, including this one.
    PairCompleted {
        /// The terminal outcome.
        outcome: PairOutcome,
        /// Pairs completed so far (including this one).
        completed: usize,
        /// Total pairs in the run.
        total: usize,
    },
}

/// Caller-supplied progress sink for [`run`]. The dispatcher invokes
/// this synchronously from the loop task, so callbacks should be
/// quick (typically: forward to a channel, return immediately).
///
/// `Send + Sync` because the dispatcher loop runs on a tokio task
/// distinct from the caller's; `'static` because we move it into the
/// dispatcher state.
pub type ProgressCallback = Arc<dyn Fn(IndexProgress) + Send + Sync + 'static>;

/// Aggregate report from [`run`].
#[derive(Debug, Default, Clone)]
pub struct IndexReport {
    /// All per-pair outcomes in completion order.
    pub outcomes: Vec<PairOutcome>,
}

impl IndexReport {
    /// Just the failures, for the CLI to surface non-zero exit hints.
    pub fn failures(&self) -> impl Iterator<Item = &PairOutcome> {
        self.outcomes
            .iter()
            .filter(|o| matches!(o, PairOutcome::Failed { .. }))
    }

    /// True iff at least one pair failed.
    pub fn has_failures(&self) -> bool {
        self.failures().next().is_some()
    }

    /// Counts: (skipped, wrote, failed, dep-skipped). The two skipped
    /// counts are reported separately because they have different
    /// causes — `skipped` means the sidecar was already up-to-date
    /// (the idempotency win), `dep_skipped` means a prerequisite
    /// indexer failed for this asset so we elided the launch.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut s = 0;
        let mut w = 0;
        let mut f = 0;
        let mut d = 0;
        for o in &self.outcomes {
            match o {
                PairOutcome::Skipped { .. } => s += 1,
                PairOutcome::Wrote { .. } => w += 1,
                PairOutcome::Failed { .. } => f += 1,
                PairOutcome::SkippedDep { .. } => d += 1,
            }
        }
        (s, w, f, d)
    }
}

/// One asset to index. The dispatcher hashes `path` to derive
/// `asset_sha256`; the `id` is the AssetId stored in the manifest and
/// sidecar header.
#[derive(Debug, Clone)]
pub struct AssetInput {
    /// Logical id (project-relative path; see `AssetId`).
    pub id: AssetId,
    /// Absolute path on disk to the source file.
    pub path: PathBuf,
}

/// Dispatch every indexer in `servers` over every asset in `assets` in
/// parallel. Writes sidecars + updates the manifest in `<project>/index/`.
///
/// `client_info` is what we tell each MCP server about ourselves during
/// `initialize`. Typically `{ name: "awidat", version: env!("CARGO_PKG_VERSION") }`.
///
/// Concurrency: bounded by `max_concurrent` (typically the number of
/// physical cores). 0 disables the limit.
///
/// `progress` is an optional sink that fires on
/// [`IndexProgress::Started`] (once at the top of the run) and
/// [`IndexProgress::PairCompleted`] (one event per pair as it reaches
/// a terminal state). The CLI passes `None`; the desktop forwards
/// events to a protocol channel.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::expect_used)]
pub async fn run(
    project_root: &Path,
    servers: &[McpServer],
    assets: &[AssetInput],
    client_info: ClientInfo,
    max_concurrent: usize,
    progress: Option<ProgressCallback>,
) -> Result<IndexReport, IndexError> {
    if !project_root.is_dir() {
        return Err(IndexError::BadRoot(project_root.display().to_string()));
    }
    let index_dir = project_root.join(files::INDEX_DIR);
    tokio::fs::create_dir_all(&index_dir)
        .await
        .map_err(|e| IndexError::SidecarIo {
            path: index_dir.display().to_string(),
            source: e,
        })?;

    // Hash all assets up front, in parallel. SHA-256 of a 1h video on
    // M-series silicon: a few seconds. We do this synchronously per asset
    // (CPU-bound) but parallelize across assets via spawn_blocking.
    let mut hashes = Vec::with_capacity(assets.len());
    for asset in assets {
        let path = asset.path.clone();
        let id = asset.id.clone();
        let h = tokio::task::spawn_blocking(move || sha::asset_sha256(&path))
            .await
            .map_err(|e| IndexError::SidecarIo {
                path: asset.path.display().to_string(),
                source: std::io::Error::other(e.to_string()),
            })?
            .map_err(|e| IndexError::SidecarIo {
                path: asset.path.display().to_string(),
                source: e,
            })?;
        hashes.push((id, asset.path.clone(), h));
    }

    // Read existing manifest (if any) so idempotency works across runs.
    let manifest_path = index_dir.join(files::INDEX_MANIFEST);
    let manifest = manifest_io::read_manifest(&manifest_path)?.unwrap_or_else(Manifest::empty);
    let manifest = Arc::new(Mutex::new(manifest));

    let report = Arc::new(Mutex::new(IndexReport::default()));

    // Pair total = indexers × assets, computed up front. The
    // dep-skipped path resolves a subset synchronously before any
    // launch, but those still count toward `total` (they're real
    // pairs we visited). `completed` ticks up as each pair lands in
    // any terminal state.
    let total_pairs = servers.len() * hashes.len();
    let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    if let Some(cb) = progress.as_ref() {
        cb(IndexProgress::Started { total: total_pairs });
    }
    // Bump the completed counter and fire the callback if present.
    // Inlined here as a closure rather than a free fn so it can
    // close over `progress` and `completed` without threading.
    let emit_completed = {
        let progress = progress.clone();
        let completed = completed.clone();
        move |outcome: PairOutcome| {
            if let Some(cb) = progress.as_ref() {
                let n = completed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                cb(IndexProgress::PairCompleted {
                    outcome,
                    completed: n,
                    total: total_pairs,
                });
            }
        }
    };

    // Topo-aware scheduler.
    //
    // Per-(indexer, asset) state, keyed by `(server-name, asset-id)`.
    // An item starts `Pending`; the scheduler launches it once every
    // dep listed in `server.depends_on` has reached `Wrote` (or
    // pre-existing `Skipped` — already-on-disk sidecars satisfy
    // dependencies, since the producer's output is right where the
    // dependent expects it). If any dep ends up `Failed` or
    // `SkippedDep`, the item is recorded as `SkippedDep` and the
    // child indexer is never launched — the user sees one root-cause
    // failure per asset, not a cascade of identical "missing
    // prerequisite sidecar" errors.
    //
    // Cross-asset parallelism is preserved: asset A's `topic` waits
    // only on asset A's `whisper`, not asset B's. Within a layer,
    // the inflight cap caps total concurrent (indexer, asset)
    // launches.
    use std::collections::HashMap as StdMap;
    type ItemKey = (String, AssetId);
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum ItemState {
        Pending, // Not yet launched. Eligible for `ready` once deps are Done.
        Running, // Launched; result not yet observed. NOT eligible for re-launch.
        Done,    // Wrote or pre-existing Skipped — counts as a satisfied dep.
        Failed,  // Indexer reported error / launch failed / dep-skipped.
    }
    let mut state: StdMap<ItemKey, ItemState> = StdMap::new();
    let mut ordered_keys: Vec<ItemKey> = Vec::new();
    let mut server_by_name: StdMap<String, &McpServer> = StdMap::new();
    let queued_at = Instant::now();
    let mut queued_since: StdMap<ItemKey, Instant> = StdMap::new();
    for server in servers {
        server_by_name.insert(server.name.clone(), server);
        for (id, _path, _sha) in &hashes {
            let key = (server.name.clone(), id.clone());
            ordered_keys.push(key.clone());
            queued_since.insert(key.clone(), queued_at);
            state.insert(key, ItemState::Pending);
        }
    }

    let inflight_cap = if max_concurrent == 0 {
        state.len().max(1)
    } else {
        max_concurrent
    };

    // Loop: pick `Pending` items whose deps are all `Done`, launch
    // up to the inflight cap, await one, repeat. Items whose deps
    // contain a `Failed` flip to `SkippedDep` synchronously.
    let mut inflight: FuturesUnordered<
        tokio::task::JoinHandle<(ItemKey, ItemState, IndexerResourceClass)>,
    > = FuturesUnordered::new();
    let mut resources = ResourceUsage::default();

    // Per-asset hash lookup (path + sha) so we can build WorkItems
    // on the fly.
    let asset_index: StdMap<AssetId, (PathBuf, String)> = hashes
        .iter()
        .map(|(id, p, sha)| (id.clone(), (p.clone(), sha.clone())))
        .collect();

    loop {
        // Find ready-to-launch items.
        let mut ready: Vec<ItemKey> = Vec::new();
        let mut to_skip_dep: Vec<(ItemKey, Vec<String>)> = Vec::new();
        for key in &ordered_keys {
            let Some(st) = state.get(key) else {
                continue;
            };
            if *st != ItemState::Pending {
                continue;
            }
            let server = server_by_name.get(&key.0).expect("server registered");
            let mut all_deps_done = true;
            let mut failed_deps: Vec<String> = Vec::new();
            for dep_name in &server.depends_on {
                if !server_by_name.contains_key(dep_name) {
                    // Dep wasn't included in this run (e.g. user
                    // passed `--indexer topic` but not `--indexer
                    // whisper`). Check whether the dep's sidecar is
                    // already on disk for this asset — if it is,
                    // treat the dep as satisfied. This is the
                    // common case for re-running a single dependent
                    // indexer to re-tune its output (e.g. iterating
                    // on topic-mcp thresholds without paying for a
                    // 10-minute whisper re-run). Only when the
                    // sidecar truly doesn't exist do we mark the
                    // dependent as dep-blocked.
                    match sidecar_path_in_index_dir(&index_dir, dep_name, &key.1) {
                        Ok(p) if p.exists() => {
                            // Sidecar exists; producer satisfied.
                        }
                        _ => {
                            failed_deps.push(dep_name.clone());
                        }
                    }
                    continue;
                }
                let dep_key = (dep_name.clone(), key.1.clone());
                match state.get(&dep_key) {
                    Some(ItemState::Done) => {}
                    Some(ItemState::Failed) => failed_deps.push(dep_name.clone()),
                    // Pending or Running — dep not yet resolved; defer.
                    _ => {
                        all_deps_done = false;
                    }
                }
            }
            if !failed_deps.is_empty() {
                to_skip_dep.push((key.clone(), failed_deps));
            } else if all_deps_done {
                ready.push(key.clone());
            }
        }

        // Mark dep-skipped items synchronously; record the outcome.
        for (key, missing) in to_skip_dep {
            state.insert(key.clone(), ItemState::Failed);
            let outcome = PairOutcome::SkippedDep {
                indexer: key.0.clone(),
                asset: key.1.clone(),
                missing,
                telemetry: telemetry_for_terminal(
                    *queued_since.get(&key).unwrap_or(&queued_at),
                    Instant::now(),
                    Duration::ZERO,
                    Duration::ZERO,
                    Duration::ZERO,
                    None,
                ),
            };
            report.lock().await.outcomes.push(outcome.clone());
            emit_completed(outcome);
        }

        // Launch as many ready items as the cap permits.
        let slots = inflight_cap.saturating_sub(inflight.len());
        let mut launched = 0_usize;
        for key in ready {
            if launched >= slots {
                break;
            }
            let class = server_by_name
                .get(&key.0)
                .expect("server present")
                .resource_class;
            if !resources.can_launch(class) {
                continue;
            }
            // Single deep clone of the McpServer. The previous
            // `.clone().clone()` triggered clippy's
            // `suspicious_double_ref_op` because the outer `.clone()`
            // was operating on a `&&McpServer` (returned by `.get()`),
            // producing another reference rather than a deep clone.
            // `(*..).clone()` is the explicit form.
            let server = (*server_by_name.get(&key.0).expect("server present")).clone();
            let (asset_path, asset_sha) = asset_index
                .get(&key.1)
                .cloned()
                .expect("asset present in index");
            // Flip to Running so the next ready-scan doesn't re-pick
            // this same item; the spawned task overwrites with the
            // final state on completion. Without this we'd double-
            // dispatch the same indexer over and over while waiting
            // for the first launch to finish.
            state.insert(key.clone(), ItemState::Running);
            resources.add(class);
            launched += 1;
            let item = WorkItem {
                server,
                asset_id: key.1.clone(),
                asset_path,
                asset_sha,
                queued_at: *queued_since.get(&key).unwrap_or(&queued_at),
            };
            let project_root_owned = project_root.to_path_buf();
            let index_dir_owned = index_dir.clone();
            let client_info_clone = client_info.clone();
            let manifest_clone = manifest.clone();
            let report_clone = report.clone();
            let key_for_task = key.clone();
            let emit_completed = emit_completed.clone();
            inflight.push(tokio::spawn(async move {
                let outcome = run_pair(
                    &project_root_owned,
                    &index_dir_owned,
                    &item,
                    client_info_clone,
                )
                .await;
                let result_state = match &outcome {
                    PairOutcome::Wrote { .. } | PairOutcome::Skipped { .. } => ItemState::Done,
                    PairOutcome::Failed { .. } | PairOutcome::SkippedDep { .. } => {
                        ItemState::Failed
                    }
                };
                if let PairOutcome::Wrote { indexer, asset, .. } = &outcome {
                    let mut m = manifest_clone.lock().await;
                    update_manifest(&mut m, indexer, asset, &item.server);
                }
                report_clone.lock().await.outcomes.push(outcome.clone());
                emit_completed(outcome);
                (key_for_task, result_state, item.server.resource_class)
            }));
        }

        if inflight.is_empty() {
            // No tasks running and no ready work — loop is finished
            // when every state is non-Pending.
            let any_pending = state.values().any(|s| *s == ItemState::Pending);
            if !any_pending {
                break;
            }
            // We have Pending items but nothing's ready and nothing's
            // in flight. That's a cycle (or mis-declared deps); bail
            // by marking all remaining Pending as failed.
            for (key, st) in state.iter_mut() {
                if *st == ItemState::Pending {
                    let outcome = PairOutcome::SkippedDep {
                        indexer: key.0.clone(),
                        asset: key.1.clone(),
                        missing: vec!["<dependency cycle>".to_string()],
                        telemetry: telemetry_for_terminal(
                            *queued_since.get(key).unwrap_or(&queued_at),
                            Instant::now(),
                            Duration::ZERO,
                            Duration::ZERO,
                            Duration::ZERO,
                            None,
                        ),
                    };
                    report.lock().await.outcomes.push(outcome.clone());
                    emit_completed(outcome);
                    *st = ItemState::Failed;
                }
            }
            break;
        }

        // Await one completion before re-checking ready set.
        if let Some(joined) = inflight.next().await {
            match joined {
                Ok((key, new_state, class)) => {
                    resources.remove(class);
                    state.insert(key, new_state);
                }
                Err(e) => {
                    tracing::error!(error = %e, "indexer task join failed");
                }
            }
        }
    }

    // Persist manifest.
    let final_manifest = manifest.lock().await.clone();
    manifest_io::write_manifest(&manifest_path, &final_manifest)?;

    let final_report = std::mem::take(&mut *report.lock().await);
    Ok(final_report)
}

#[derive(Debug, Clone)]
struct WorkItem {
    server: McpServer,
    asset_id: AssetId,
    asset_path: PathBuf,
    asset_sha: String,
    queued_at: Instant,
}

#[derive(Debug, Default)]
struct ResourceUsage {
    exclusive: usize,
    vision: usize,
    non_network: usize,
}

impl ResourceUsage {
    fn can_launch(&self, class: IndexerResourceClass) -> bool {
        match class {
            // Exclusive indexers do the memory-heavy ML work. They may run
            // beside network-only work, but not beside CPU/video/model work.
            IndexerResourceClass::Exclusive => self.exclusive == 0 && self.non_network == 0,
            // Network work is intentionally allowed alongside CPU-heavy work.
            IndexerResourceClass::Network => true,
            // Non-network work should not start while an exclusive worker is
            // resident. Vision is additionally limited to one process.
            IndexerResourceClass::Vision => self.exclusive == 0 && self.vision == 0,
            IndexerResourceClass::Light | IndexerResourceClass::Embedding => self.exclusive == 0,
        }
    }

    fn add(&mut self, class: IndexerResourceClass) {
        match class {
            IndexerResourceClass::Exclusive => {
                self.exclusive += 1;
                self.non_network += 1;
            }
            IndexerResourceClass::Vision => {
                self.vision += 1;
                self.non_network += 1;
            }
            IndexerResourceClass::Light | IndexerResourceClass::Embedding => {
                self.non_network += 1;
            }
            IndexerResourceClass::Network => {}
        }
    }

    fn remove(&mut self, class: IndexerResourceClass) {
        match class {
            IndexerResourceClass::Exclusive => {
                self.exclusive = self.exclusive.saturating_sub(1);
                self.non_network = self.non_network.saturating_sub(1);
            }
            IndexerResourceClass::Vision => {
                self.vision = self.vision.saturating_sub(1);
                self.non_network = self.non_network.saturating_sub(1);
            }
            IndexerResourceClass::Light | IndexerResourceClass::Embedding => {
                self.non_network = self.non_network.saturating_sub(1);
            }
            IndexerResourceClass::Network => {}
        }
    }
}

async fn run_pair(
    project_root: &Path,
    index_dir: &Path,
    item: &WorkItem,
    client_info: ClientInfo,
) -> PairOutcome {
    let pair_started = Instant::now();
    let mut launch_init = Duration::ZERO;
    let mut tool = Duration::ZERO;
    let mut write = Duration::ZERO;
    let mut peak_rss_bytes = None;

    // Idempotency: if a sidecar already exists with a matching sha, skip.
    let sidecar_path = match sidecar_path_in_index_dir(index_dir, &item.server.name, &item.asset_id)
    {
        Ok(p) => p,
        Err(e) => {
            return PairOutcome::Failed {
                indexer: item.server.name.clone(),
                asset: item.asset_id.clone(),
                message: e.to_string(),
                telemetry: telemetry_for_terminal(
                    item.queued_at,
                    pair_started,
                    launch_init,
                    tool,
                    write,
                    peak_rss_bytes,
                ),
            };
        }
    };
    if let Ok(existing) = read_existing_sha(&sidecar_path)
        && existing == item.asset_sha
    {
        return PairOutcome::Skipped {
            indexer: item.server.name.clone(),
            asset: item.asset_id.clone(),
            telemetry: telemetry_for_terminal(
                item.queued_at,
                pair_started,
                launch_init,
                tool,
                write,
                peak_rss_bytes,
            ),
        };
    }

    // Launch the MCP server and call index_asset.
    let server_config = server_config_from(&item.server, project_root);
    let launch_started = Instant::now();
    let mut client = match Client::launch(server_config) {
        Ok(c) => c,
        Err(e) => {
            launch_init = launch_started.elapsed();
            return PairOutcome::Failed {
                indexer: item.server.name.clone(),
                asset: item.asset_id.clone(),
                message: format!("failed to launch: {e}"),
                telemetry: telemetry_for_terminal(
                    item.queued_at,
                    pair_started,
                    launch_init,
                    tool,
                    write,
                    peak_rss_bytes,
                ),
            };
        }
    };
    let mut rss_monitor = RssMonitor::start(client.child_pid());

    if let Err(e) = client.initialize(client_info).await {
        launch_init = launch_started.elapsed();
        peak_rss_bytes = stop_rss_monitor(&mut rss_monitor).await;
        return PairOutcome::Failed {
            indexer: item.server.name.clone(),
            asset: item.asset_id.clone(),
            message: format!("initialize failed: {e}"),
            telemetry: telemetry_for_terminal(
                item.queued_at,
                pair_started,
                launch_init,
                tool,
                write,
                peak_rss_bytes,
            ),
        };
    }
    launch_init = launch_started.elapsed();

    let args = serde_json::json!({
        "asset_path": item.asset_path.to_string_lossy(),
        "asset_id": item.asset_id.as_str(),
        "asset_sha256": item.asset_sha,
    });
    // Generous timeout for indexer runs; whisper on a long episode is the
    // worst case.
    let tool_started = Instant::now();
    let result = client
        .call_tool_with_timeout("index_asset", args, Some(Duration::from_secs(60 * 60)))
        .await;
    tool = tool_started.elapsed();
    let _ = client.shutdown().await;
    peak_rss_bytes = stop_rss_monitor(&mut rss_monitor).await;

    let result = match result {
        Ok(r) => r,
        Err(e) => {
            return PairOutcome::Failed {
                indexer: item.server.name.clone(),
                asset: item.asset_id.clone(),
                message: format!("index_asset call failed: {e}"),
                telemetry: telemetry_for_terminal(
                    item.queued_at,
                    pair_started,
                    launch_init,
                    tool,
                    write,
                    peak_rss_bytes,
                ),
            };
        }
    };
    if result.is_error {
        return PairOutcome::Failed {
            indexer: item.server.name.clone(),
            asset: item.asset_id.clone(),
            message: format!("indexer reported error: {result:?}"),
            telemetry: telemetry_for_terminal(
                item.queued_at,
                pair_started,
                launch_init,
                tool,
                write,
                peak_rss_bytes,
            ),
        };
    }

    // Indexer must return structured_content shaped as IndexSidecar<Value>.
    let sidecar_value = match result.structured_content {
        Some(v) => v,
        None => match result.single_text() {
            Some(text) => match serde_json::from_str::<serde_json::Value>(text) {
                Ok(v) => v,
                Err(e) => {
                    return PairOutcome::Failed {
                        indexer: item.server.name.clone(),
                        asset: item.asset_id.clone(),
                        message: format!("indexer returned non-JSON text: {e}"),
                        telemetry: telemetry_for_terminal(
                            item.queued_at,
                            pair_started,
                            launch_init,
                            tool,
                            write,
                            peak_rss_bytes,
                        ),
                    };
                }
            },
            None => {
                return PairOutcome::Failed {
                    indexer: item.server.name.clone(),
                    asset: item.asset_id.clone(),
                    message: "indexer returned no structured_content and no text".into(),
                    telemetry: telemetry_for_terminal(
                        item.queued_at,
                        pair_started,
                        launch_init,
                        tool,
                        write,
                        peak_rss_bytes,
                    ),
                };
            }
        },
    };

    // Validate header shape.
    let parsed: IndexSidecar<serde_json::Value> =
        match serde_json::from_value(sidecar_value.clone()) {
            Ok(p) => p,
            Err(e) => {
                return PairOutcome::Failed {
                    indexer: item.server.name.clone(),
                    asset: item.asset_id.clone(),
                    message: format!("indexer returned malformed sidecar: {e}"),
                    telemetry: telemetry_for_terminal(
                        item.queued_at,
                        pair_started,
                        launch_init,
                        tool,
                        write,
                        peak_rss_bytes,
                    ),
                };
            }
        };
    if parsed.header.indexer != item.server.name {
        warn!(
            indexer = %item.server.name,
            header_indexer = %parsed.header.indexer,
            "indexer name in sidecar header does not match server registration",
        );
    }

    let write_started = Instant::now();
    if let Err(e) = write_sidecar(&sidecar_path, &sidecar_value).await {
        write = write_started.elapsed();
        return PairOutcome::Failed {
            indexer: item.server.name.clone(),
            asset: item.asset_id.clone(),
            message: format!("write failed: {e}"),
            telemetry: telemetry_for_terminal(
                item.queued_at,
                pair_started,
                launch_init,
                tool,
                write,
                peak_rss_bytes,
            ),
        };
    }
    write = write_started.elapsed();

    info!(
        indexer = %item.server.name,
        asset = %item.asset_id,
        path = %sidecar_path.display(),
        "wrote sidecar",
    );
    PairOutcome::Wrote {
        indexer: item.server.name.clone(),
        asset: item.asset_id.clone(),
        path: sidecar_path,
        telemetry: telemetry_for_terminal(
            item.queued_at,
            pair_started,
            launch_init,
            tool,
            write,
            peak_rss_bytes,
        ),
    }
}

fn telemetry_for_terminal(
    queued_at: Instant,
    pair_started: Instant,
    launch_init: Duration,
    tool: Duration,
    write: Duration,
    peak_rss_bytes: Option<u64>,
) -> PairTelemetry {
    let total = queued_at.elapsed();
    PairTelemetry {
        queued: pair_started.saturating_duration_since(queued_at),
        launch_init,
        tool,
        write,
        total: if total.is_zero() {
            Duration::from_nanos(1)
        } else {
            total
        },
        peak_rss_bytes,
    }
}

fn server_config_from(server: &McpServer, project_root: &Path) -> ServerConfig {
    let cwd = server.cwd.as_ref().map(|c| {
        if c.is_absolute() {
            c.clone()
        } else {
            project_root.join(c)
        }
    });
    ServerConfig {
        name: server.name.clone(),
        command: server.command.clone(),
        args: server.args.clone(),
        env: server.env.clone(),
        cwd,
    }
}

struct RssMonitor {
    stop_tx: Option<oneshot::Sender<()>>,
    handle: tokio::task::JoinHandle<Option<u64>>,
}

impl RssMonitor {
    fn start(pid: Option<u32>) -> Option<Self> {
        let pid = pid?;
        let (stop_tx, mut stop_rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            let mut peak = sample_rss_bytes(pid).await;
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Some(rss) = sample_rss_bytes(pid).await {
                            peak = Some(peak.map_or(rss, |p| p.max(rss)));
                        }
                    }
                    _ = &mut stop_rx => {
                        if let Some(rss) = sample_rss_bytes(pid).await {
                            peak = Some(peak.map_or(rss, |p| p.max(rss)));
                        }
                        return peak;
                    }
                }
            }
        });
        Some(Self {
            stop_tx: Some(stop_tx),
            handle,
        })
    }

    async fn stop(mut self) -> Option<u64> {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        self.handle.await.ok().flatten()
    }
}

async fn stop_rss_monitor(monitor: &mut Option<RssMonitor>) -> Option<u64> {
    match monitor.take() {
        Some(m) => m.stop().await,
        None => None,
    }
}

#[cfg(target_os = "macos")]
async fn sample_rss_bytes(pid: u32) -> Option<u64> {
    let out = tokio::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let kib = text.trim().parse::<u64>().ok()?;
    Some(kib.saturating_mul(1024))
}

#[cfg(not(target_os = "macos"))]
async fn sample_rss_bytes(_pid: u32) -> Option<u64> {
    None
}

/// Resolve a sidecar path under `<project>/index/`. The dispatcher passes
/// `<project>/index` as `index_dir`; the public [`sidecar_io::sidecar_path`]
/// is the rooted-at-project-root variant the editorial tools use.
fn sidecar_path_in_index_dir(
    index_dir: &Path,
    indexer: &str,
    asset_id: &AssetId,
) -> Result<PathBuf, IndexError> {
    let rel = asset_id
        .sidecar_relative_path()
        .ok_or_else(|| IndexError::InvalidAssetId {
            asset: asset_id.to_string(),
            message: "asset id failed safe-path validation".into(),
        })?;
    Ok(index_dir.join(indexer).join(rel))
}

fn read_existing_sha(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let v: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    v.get("asset_sha256")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "no asset_sha256 in sidecar",
            )
        })
}

async fn write_sidecar(path: &Path, value: &serde_json::Value) -> Result<(), IndexError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| IndexError::SidecarIo {
                path: parent.display().to_string(),
                source: e,
            })?;
    }
    let body = serde_json::to_vec_pretty(value).map_err(|e| IndexError::SidecarIo {
        path: path.display().to_string(),
        source: std::io::Error::other(e),
    })?;
    tokio::fs::write(path, body)
        .await
        .map_err(|e| IndexError::SidecarIo {
            path: path.display().to_string(),
            source: e,
        })
}

fn update_manifest(manifest: &mut Manifest, indexer: &str, asset: &AssetId, server: &McpServer) {
    let now = Utc::now();
    if let Some(entry) = manifest.indexers.iter_mut().find(|e| e.name == indexer) {
        if !entry.assets.contains(asset) {
            entry.assets.push(asset.clone());
        }
        entry.last_run = now;
        // Note: indexer / schema versions come from the sidecar header, not
        // from server config. We keep whatever the previous run recorded.
        return;
    }
    let _ = server; // server config doesn't carry version info; placeholder.
    manifest.indexers.push(IndexerEntry {
        name: indexer.into(),
        version: String::from("unknown"),
        schema_version: String::from("1"),
        assets: vec![asset.clone()],
        last_run: now,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn server(
        name: &str,
        class: IndexerResourceClass,
        depends_on: Vec<String>,
        command: &str,
    ) -> McpServer {
        McpServer {
            name: name.into(),
            command: command.into(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            kind: awidat_config::McpServerKind::Indexer,
            enabled: true,
            depends_on,
            resource_class: class,
            indexer_group: None,
        }
    }

    #[test]
    fn report_counts() {
        let r = IndexReport {
            outcomes: vec![
                PairOutcome::Wrote {
                    indexer: "whisper".into(),
                    asset: AssetId::new("a"),
                    path: PathBuf::from("/x"),
                    telemetry: PairTelemetry::default(),
                },
                PairOutcome::Skipped {
                    indexer: "whisper".into(),
                    asset: AssetId::new("b"),
                    telemetry: PairTelemetry::default(),
                },
                PairOutcome::Failed {
                    indexer: "scenedetect".into(),
                    asset: AssetId::new("a"),
                    message: "boom".into(),
                    telemetry: PairTelemetry::default(),
                },
            ],
        };
        assert_eq!(r.counts(), (1, 1, 1, 0));
        assert!(r.has_failures());
        assert_eq!(r.failures().count(), 1);
    }

    #[test]
    fn report_counts_includes_dep_skipped() {
        let r = IndexReport {
            outcomes: vec![
                PairOutcome::Wrote {
                    indexer: "whisper".into(),
                    asset: AssetId::new("a"),
                    path: PathBuf::from("/x.json"),
                    telemetry: PairTelemetry::default(),
                },
                PairOutcome::Failed {
                    indexer: "whisper".into(),
                    asset: AssetId::new("b"),
                    message: "boom".into(),
                    telemetry: PairTelemetry::default(),
                },
                PairOutcome::SkippedDep {
                    indexer: "topic".into(),
                    asset: AssetId::new("b"),
                    missing: vec!["whisper".into()],
                    telemetry: PairTelemetry::default(),
                },
            ],
        };
        assert_eq!(r.counts(), (0, 1, 1, 1));
    }

    #[test]
    fn resource_usage_keeps_exclusive_apart() {
        let mut usage = ResourceUsage::default();
        assert!(usage.can_launch(IndexerResourceClass::Exclusive));
        usage.add(IndexerResourceClass::Exclusive);
        assert!(!usage.can_launch(IndexerResourceClass::Exclusive));
        assert!(!usage.can_launch(IndexerResourceClass::Vision));
        assert!(!usage.can_launch(IndexerResourceClass::Light));
        assert!(usage.can_launch(IndexerResourceClass::Network));
        usage.remove(IndexerResourceClass::Exclusive);
        assert!(usage.can_launch(IndexerResourceClass::Exclusive));
    }

    #[test]
    fn resource_usage_allows_light_beside_vision() {
        let mut usage = ResourceUsage::default();
        usage.add(IndexerResourceClass::Vision);
        assert!(!usage.can_launch(IndexerResourceClass::Vision));
        assert!(usage.can_launch(IndexerResourceClass::Light));
        assert!(usage.can_launch(IndexerResourceClass::Embedding));
        assert!(usage.can_launch(IndexerResourceClass::Network));
    }

    #[test]
    fn resource_usage_serial_cap_is_represented_by_inflight_cap_not_resources() {
        let usage = ResourceUsage::default();
        assert!(usage.can_launch(IndexerResourceClass::Light));
        assert!(usage.can_launch(IndexerResourceClass::Vision));
        // The run loop's inflight cap, not ResourceUsage, enforces
        // AWIDAT_INDEX_CONCURRENCY=1. ResourceUsage only models RAM classes.
    }

    #[tokio::test]
    async fn skipped_outcome_has_nonzero_telemetry_and_no_rss_requirement() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let raw = root.join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        let asset_path = raw.join("a.txt");
        std::fs::write(&asset_path, b"hello").unwrap();
        let sha = asset_sha256(&asset_path).unwrap();
        let sidecar = root.join("index/dummy/raw/a.txt.json");
        std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(
            &sidecar,
            serde_json::to_vec(&serde_json::json!({
                "indexer": "dummy",
                "indexer_version": "1",
                "schema_version": "1",
                "asset_id": "raw/a.txt",
                "asset_sha256": sha,
                "produced_at": "2026-05-02T12:00:00Z",
                "data": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let report = run(
            root,
            &[server(
                "dummy",
                IndexerResourceClass::Light,
                vec![],
                "/bin/false",
            )],
            &[AssetInput {
                id: AssetId::new("raw/a.txt"),
                path: asset_path,
            }],
            ClientInfo {
                name: "test".into(),
                version: "0".into(),
            },
            1,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(report.outcomes[0], PairOutcome::Skipped { .. }));
        assert!(!report.outcomes[0].telemetry().total.is_zero());
        // Unsupported RSS sampling should simply surface as None.
        let _ = report.outcomes[0].telemetry().peak_rss_bytes;
    }

    #[tokio::test]
    async fn failed_dependency_produces_dep_skip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let raw = root.join("raw");
        std::fs::create_dir_all(&raw).unwrap();
        let asset_path = raw.join("a.txt");
        std::fs::write(&asset_path, b"hello").unwrap();
        let assets = [AssetInput {
            id: AssetId::new("raw/a.txt"),
            path: asset_path,
        }];
        let servers = [
            server(
                "producer",
                IndexerResourceClass::Light,
                vec![],
                "/bin/false",
            ),
            server(
                "consumer",
                IndexerResourceClass::Light,
                vec!["producer".into()],
                "/bin/false",
            ),
        ];
        let report = run(
            root,
            &servers,
            &assets,
            ClientInfo {
                name: "test".into(),
                version: "0".into(),
            },
            2,
            None,
        )
        .await
        .unwrap();
        assert!(report.outcomes.iter().any(|o| matches!(
            o,
            PairOutcome::Failed { indexer, .. } if indexer == "producer"
        )));
        assert!(report.outcomes.iter().any(|o| matches!(
            o,
            PairOutcome::SkippedDep { indexer, missing, .. }
                if indexer == "consumer" && missing == &vec!["producer".to_string()]
        )));
    }
}
