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

pub use manifest_io::{read_manifest, write_manifest};
pub use sha::asset_sha256;

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

    /// Counts: (skipped, wrote, failed).
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut s = 0;
        let mut w = 0;
        let mut f = 0;
        for o in &self.outcomes {
            match o {
                PairOutcome::Skipped { .. } => s += 1,
                PairOutcome::Wrote { .. } => w += 1,
                PairOutcome::Failed { .. } => f += 1,
            }
        }
        (s, w, f)
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

    // Build the (indexer × asset) work list.
    let mut work = Vec::new();
    for server in servers {
        for (id, path, sha) in &hashes {
            work.push(WorkItem {
                server: server.clone(),
                asset_id: id.clone(),
                asset_path: path.clone(),
                asset_sha: sha.clone(),
            });
        }
    }

    let inflight_cap = if max_concurrent == 0 {
        work.len().max(1)
    } else {
        max_concurrent
    };

    let mut inflight = FuturesUnordered::new();
    let mut work_iter = work.into_iter();

    // Prime the in-flight set up to the cap.
    for _ in 0..inflight_cap {
        if let Some(item) = work_iter.next() {
            inflight.push(spawn_pair(
                project_root.to_path_buf(),
                index_dir.clone(),
                item,
                client_info.clone(),
                manifest.clone(),
                report.clone(),
            ));
        }
    }
    while inflight.next().await.is_some() {
        if let Some(item) = work_iter.next() {
            inflight.push(spawn_pair(
                project_root.to_path_buf(),
                index_dir.clone(),
                item,
                client_info.clone(),
                manifest.clone(),
                report.clone(),
            ));
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

fn spawn_pair(
    project_root: PathBuf,
    index_dir: PathBuf,
    item: WorkItem,
    client_info: ClientInfo,
    manifest: Arc<Mutex<Manifest>>,
    report: Arc<Mutex<IndexReport>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let outcome = run_pair(&project_root, &index_dir, &item, client_info).await;
        // Update manifest on success.
        if let PairOutcome::Wrote { indexer, asset, .. } = &outcome {
            let mut m = manifest.lock().await;
            update_manifest(&mut m, indexer, asset, &item.server);
        }
        report.lock().await.outcomes.push(outcome);
    })
}

async fn run_pair(
    project_root: &Path,
    index_dir: &Path,
    item: &WorkItem,
    client_info: ClientInfo,
) -> PairOutcome {
    // Idempotency: if a sidecar already exists with a matching sha, skip.
    let sidecar_path = match sidecar_path(index_dir, &item.server.name, &item.asset_id) {
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

fn sidecar_path(
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
        assert_eq!(r.counts(), (1, 1, 1));
        assert!(r.has_failures());
        assert_eq!(r.failures().count(), 1);
    }
}
