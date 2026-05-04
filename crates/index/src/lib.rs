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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use awidat_config::McpServer;
use awidat_mcp::{Client, ClientInfo, ServerConfig};
use awidat_proto::index::{AssetId, IndexSidecar, IndexerEntry, Manifest};
use awidat_proto::project::files;
use chrono::Utc;
use futures::stream::{FuturesUnordered, StreamExt};
use thiserror::Error;
use tokio::sync::Mutex;
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
    },
    /// Sidecar written or refreshed.
    Wrote {
        /// Indexer name.
        indexer: String,
        /// Asset id.
        asset: AssetId,
        /// Path the sidecar was written to.
        path: PathBuf,
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
    },
}

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
#[allow(clippy::too_many_arguments)]
pub async fn run(
    project_root: &Path,
    servers: &[McpServer],
    assets: &[AssetInput],
    client_info: ClientInfo,
    max_concurrent: usize,
) -> Result<IndexReport, IndexError> {
    if !project_root.is_dir() {
        return Err(IndexError::BadRoot(project_root.display().to_string()));
    }
    let index_dir = project_root.join(files::INDEX_DIR);
    tokio::fs::create_dir_all(&index_dir).await.map_err(|e| {
        IndexError::SidecarIo {
            path: index_dir.display().to_string(),
            source: e,
        }
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
    let manifest = manifest_io::read_manifest(&manifest_path)?
        .unwrap_or_else(Manifest::empty);
    let manifest = Arc::new(Mutex::new(manifest));

    let report = Arc::new(Mutex::new(IndexReport::default()));

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
    let mut server_by_name: StdMap<String, &McpServer> = StdMap::new();
    for server in servers {
        server_by_name.insert(server.name.clone(), server);
        for (id, _path, _sha) in &hashes {
            state.insert((server.name.clone(), id.clone()), ItemState::Pending);
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
        tokio::task::JoinHandle<(ItemKey, ItemState)>,
    > = FuturesUnordered::new();

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
        for (key, st) in state.iter() {
            if *st != ItemState::Pending {
                continue;
            }
            let server = server_by_name.get(&key.0).expect("server registered");
            let mut all_deps_done = true;
            let mut failed_deps: Vec<String> = Vec::new();
            for dep_name in &server.depends_on {
                if !server_by_name.contains_key(dep_name) {
                    // Dep wasn't included in this run (e.g. user
                    // passed --indexer topic but not --indexer
                    // whisper). Treat as a failed dep so we skip
                    // the dependent with a clear message.
                    failed_deps.push(dep_name.clone());
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
            report.lock().await.outcomes.push(PairOutcome::SkippedDep {
                indexer: key.0.clone(),
                asset: key.1.clone(),
                missing,
            });
        }

        // Launch as many ready items as the cap permits.
        let slots = inflight_cap.saturating_sub(inflight.len());
        for key in ready.into_iter().take(slots) {
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
            let item = WorkItem {
                server,
                asset_id: key.1.clone(),
                asset_path,
                asset_sha,
            };
            let project_root_owned = project_root.to_path_buf();
            let index_dir_owned = index_dir.clone();
            let client_info_clone = client_info.clone();
            let manifest_clone = manifest.clone();
            let report_clone = report.clone();
            let key_for_task = key.clone();
            inflight.push(tokio::spawn(async move {
                let outcome =
                    run_pair(&project_root_owned, &index_dir_owned, &item, client_info_clone)
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
                report_clone.lock().await.outcomes.push(outcome);
                (key_for_task, result_state)
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
                    };
                    report.lock().await.outcomes.push(outcome);
                    *st = ItemState::Failed;
                }
            }
            break;
        }

        // Await one completion before re-checking ready set.
        if let Some(joined) = inflight.next().await {
            match joined {
                Ok((key, new_state)) => {
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
}

async fn run_pair(
    project_root: &Path,
    index_dir: &Path,
    item: &WorkItem,
    client_info: ClientInfo,
) -> PairOutcome {
    // Idempotency: if a sidecar already exists with a matching sha, skip.
    let sidecar_path = match sidecar_path_in_index_dir(index_dir, &item.server.name, &item.asset_id) {
        Ok(p) => p,
        Err(e) => {
            return PairOutcome::Failed {
                indexer: item.server.name.clone(),
                asset: item.asset_id.clone(),
                message: e.to_string(),
            };
        }
    };
    if let Ok(existing) = read_existing_sha(&sidecar_path)
        && existing == item.asset_sha
    {
        return PairOutcome::Skipped {
            indexer: item.server.name.clone(),
            asset: item.asset_id.clone(),
        };
    }

    // Launch the MCP server and call index_asset.
    let server_config = server_config_from(&item.server, project_root);
    let mut client = match Client::launch(server_config) {
        Ok(c) => c,
        Err(e) => {
            return PairOutcome::Failed {
                indexer: item.server.name.clone(),
                asset: item.asset_id.clone(),
                message: format!("failed to launch: {e}"),
            };
        }
    };

    if let Err(e) = client.initialize(client_info).await {
        return PairOutcome::Failed {
            indexer: item.server.name.clone(),
            asset: item.asset_id.clone(),
            message: format!("initialize failed: {e}"),
        };
    }

    let args = serde_json::json!({
        "asset_path": item.asset_path.to_string_lossy(),
        "asset_id": item.asset_id.as_str(),
        "asset_sha256": item.asset_sha,
    });
    // Generous timeout for indexer runs; whisper on a long episode is the
    // worst case.
    let result = client
        .call_tool_with_timeout("index_asset", args, Some(Duration::from_secs(60 * 60)))
        .await;
    let _ = client.shutdown().await;

    let result = match result {
        Ok(r) => r,
        Err(e) => {
            return PairOutcome::Failed {
                indexer: item.server.name.clone(),
                asset: item.asset_id.clone(),
                message: format!("index_asset call failed: {e}"),
            };
        }
    };
    if result.is_error {
        return PairOutcome::Failed {
            indexer: item.server.name.clone(),
            asset: item.asset_id.clone(),
            message: format!("indexer reported error: {result:?}"),
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
                    };
                }
            },
            None => {
                return PairOutcome::Failed {
                    indexer: item.server.name.clone(),
                    asset: item.asset_id.clone(),
                    message: "indexer returned no structured_content and no text".into(),
                };
            }
        },
    };

    // Validate header shape.
    let parsed: IndexSidecar<serde_json::Value> = match serde_json::from_value(sidecar_value.clone())
    {
        Ok(p) => p,
        Err(e) => {
            return PairOutcome::Failed {
                indexer: item.server.name.clone(),
                asset: item.asset_id.clone(),
                message: format!("indexer returned malformed sidecar: {e}"),
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

    if let Err(e) = write_sidecar(&sidecar_path, &sidecar_value).await {
        return PairOutcome::Failed {
            indexer: item.server.name.clone(),
            asset: item.asset_id.clone(),
            message: format!("write failed: {e}"),
        };
    }

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
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no asset_sha256 in sidecar"))
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

fn update_manifest(
    manifest: &mut Manifest,
    indexer: &str,
    asset: &AssetId,
    server: &McpServer,
) {
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

    #[test]
    fn report_counts() {
        let r = IndexReport {
            outcomes: vec![
                PairOutcome::Wrote {
                    indexer: "whisper".into(),
                    asset: AssetId::new("a"),
                    path: PathBuf::from("/x"),
                },
                PairOutcome::Skipped {
                    indexer: "whisper".into(),
                    asset: AssetId::new("b"),
                },
                PairOutcome::Failed {
                    indexer: "scenedetect".into(),
                    asset: AssetId::new("a"),
                    message: "boom".into(),
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
                },
                PairOutcome::Failed {
                    indexer: "whisper".into(),
                    asset: AssetId::new("b"),
                    message: "boom".into(),
                },
                PairOutcome::SkippedDep {
                    indexer: "topic".into(),
                    asset: AssetId::new("b"),
                    missing: vec!["whisper".into()],
                },
            ],
        };
        assert_eq!(r.counts(), (0, 1, 1, 1));
    }
}
