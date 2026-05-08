//! `index_project` command. Wraps `awidat_index::run` with the
//! progress callback added in commit dd15fe5, surfacing a single
//! [`Item::Job`] (kind = `Indexing`) that streams percent + status
//! as pairs complete.

use std::sync::Arc;

use awidat_config::Config;
use awidat_desktop_protocol::{Id, JobKind};
use awidat_index::{AssetInput, IndexProgress, PairOutcome, ProgressCallback};
use awidat_mcp::ClientInfo;
use awidat_proto::index::AssetId;
use tauri::{AppHandle, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::events::JobEmitter;
use crate::state::{AwidatState, JobHandle};

/// Tauri-facing entrypoint. Thin wrapper over [`index_project_inner`]
/// so the auto-chain in `import.rs` can call the real implementation
/// directly without re-entering the command machinery.
#[tauri::command]
pub async fn index_project(app: AppHandle, state: State<'_, AwidatState>) -> Result<(), String> {
    index_project_inner(&app, &state).await
}

/// Run every configured indexer over every asset under the project's
/// `raw/` dir. Streams progress as an `Item::Job` (kind = `Indexing`).
/// Cancellable via `cancel_job(job_id)`.
pub async fn index_project_inner(
    app: &AppHandle,
    state: &State<'_, AwidatState>,
) -> Result<(), String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    index_project_at_root(app, state, project_root).await
}

/// Run every configured indexer over every asset under `project_root`.
/// The explicit root keeps background post-import chains tied to the
/// project that created them, even if the UI opens another project.
pub async fn index_project_at_root(
    app: &AppHandle,
    state: &State<'_, AwidatState>,
    project_root: std::path::PathBuf,
) -> Result<(), String> {
    // Load config (project-scoped overlays the global one). If no
    // indexers are configured the message mirrors the CLI's so a
    // user troubleshooting can match.
    let config = Config::load(Some(&project_root)).map_err(|e| format!("load config: {e}"))?;
    let servers: Vec<_> = config.indexers().cloned().collect();
    if servers.is_empty() {
        return Err(
            "no indexers configured. Add `[[mcp.servers]]` entries with kind = \"indexer\" \
                    to your project's `.awidat/config.toml` or `~/.config/awidat/config.toml`."
                .into(),
        );
    }

    // Discover assets under raw/ — same walk as the CLI's
    // `index_cmd::collect_assets` minus the explicit-paths branch.
    let assets = collect_assets(&project_root).map_err(|e| format!("scan raw/: {e}"))?;
    if assets.is_empty() {
        return Err(format!(
            "no assets to index. Drop source files under '{}/raw' or use Import.",
            project_root.display()
        ));
    }

    let job_id = Id::new(format!(
        "index-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let cancel = register_job(&state, &job_id).await;
    let emitter = JobEmitter::start(
        app.clone(),
        job_id.clone(),
        JobKind::Indexing,
        format!(
            "indexing {} asset(s) with {} indexer(s)",
            assets.len(),
            servers.len()
        ),
    );

    // Channel: `awidat_index::run`'s callback (sync, fires from the
    // dispatcher loop) → mpsc → a forwarder task that drives the
    // emitter. Going via mpsc keeps the callback non-blocking and
    // keeps the emitter calls on a single task.
    let (tx, mut rx) = mpsc::unbounded_channel::<IndexProgress>();
    let cb: ProgressCallback = Arc::new(move |evt| {
        let _ = tx.send(evt);
    });

    // Forwarder task — owns the JobEmitter for the duration. We
    // can't reuse the outer `emitter` variable because we move it
    // into the spawned task; we'll surface the final result via a
    // oneshot.
    let app_for_task = app.clone();
    let emitter_for_task = emitter; // move; outer scope uses `done_rx`
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<JobOutcome>();
    tokio::spawn(async move {
        let mut total: usize = 0;
        let mut last_percent: u8 = 0;
        while let Some(evt) = rx.recv().await {
            match evt {
                IndexProgress::Started { total: t } => {
                    total = t;
                    emitter_for_task.progress(Some(0), format!("0 / {t} pairs"));
                }
                IndexProgress::PairCompleted {
                    outcome,
                    completed,
                    total: t,
                } => {
                    total = t;
                    let pct = if t == 0 {
                        100
                    } else {
                        ((completed as f32 / t as f32) * 100.0).round() as u8
                    };
                    last_percent = pct;
                    let label = pair_label(&outcome);
                    emitter_for_task.progress(Some(pct), format!("{label} · {completed} / {t}"));
                }
            }
        }
        // Channel closed = run finished. The outer task delivers the
        // final Ok/Err/Cancelled via `done_tx` so this task only
        // mirrors progress.
        let _ = (last_percent, total, app_for_task);
        // Preserve the emitter for the outer task's final transition.
        let _ = done_tx.send(JobOutcome {
            emitter: emitter_for_task,
        });
    });

    // Drive the dispatcher.
    let client_info = ClientInfo {
        name: "awidat-desktop".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    };

    let concurrency = std::env::var("AWIDAT_INDEX_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1);
    let dispatch = awidat_index::run(
        &project_root,
        &servers,
        &assets,
        client_info,
        concurrency,
        Some(cb),
    );

    let result = tokio::select! {
        _ = cancel.cancelled() => Err("cancelled".into()),
        r = dispatch => r.map_err(|e| format!("dispatcher: {e}")),
    };

    unregister_job(&state, &job_id).await;

    // Pull the emitter back from the forwarder task. The forwarder
    // exits when the callback channel closes (which happens when the
    // dispatcher returns and drops the callback).
    let JobOutcome { emitter } = done_rx.await.map_err(|_| "emitter task crashed")?;

    match result {
        Ok(report) => {
            let (skipped, wrote, failed, dep_skipped) = report.counts();
            let summary = format!(
                "{wrote} wrote, {skipped} skipped, {failed} failed, {dep_skipped} dep-skipped"
            );
            // If a whisper sidecar was just written or refreshed,
            // drop the transcript-pane's parsed-Transcript cache so
            // the next read picks up the new shape. Cheap full-clear
            // — the cache rebuilds lazily on the next tab toggle.
            let whisper_wrote = report
                .outcomes
                .iter()
                .any(|o| matches!(o, PairOutcome::Wrote { indexer, .. } if indexer == "whisper"));
            if whisper_wrote {
                crate::commands::transcript::clear_transcript_cache(&state).await;
            }
            if report.has_failures() {
                // Pull each PairOutcome::Failed out so the user sees
                // *which* indexer failed on *which* asset *why*. With
                // 10+ indexers per asset, "1 failed" alone leaves the
                // user (or the agent) guessing.
                let mut detail = format!("indexing finished with failures: {summary}");
                for outcome in &report.outcomes {
                    if let PairOutcome::Failed {
                        indexer,
                        asset,
                        message,
                    } = outcome
                    {
                        // First line of the message is usually the
                        // root cause; trailing lines are stack
                        // context. Cap to first line + 200 chars so
                        // the card stays a card.
                        let first = message.lines().next().unwrap_or(message);
                        let truncated = if first.len() > 200 {
                            format!("{}…", &first[..200])
                        } else {
                            first.to_string()
                        };
                        detail.push_str(&format!("\n  ✗ {indexer} · {asset}: {truncated}"));
                    }
                }
                emitter.err(detail);
                Err(summary)
            } else {
                emitter.ok(Some(summary));
                Ok(())
            }
        }
        Err(e) if e == "cancelled" => {
            emitter.cancelled();
            Err(e)
        }
        Err(e) => {
            emitter.err(e.clone());
            Err(e)
        }
    }
}

/// Carries the `JobEmitter` from the forwarder task back to the
/// outer task once the callback channel closes.
struct JobOutcome {
    emitter: JobEmitter,
}

fn pair_label(o: &PairOutcome) -> String {
    match o {
        PairOutcome::Wrote { indexer, asset, .. } => format!("✓ {indexer} · {asset}"),
        PairOutcome::Skipped { indexer, asset } => format!("· {indexer} · {asset}"),
        PairOutcome::Failed { indexer, asset, .. } => format!("✗ {indexer} · {asset}"),
        PairOutcome::SkippedDep { indexer, asset, .. } => format!("⊘ {indexer} · {asset}"),
    }
}

fn collect_assets(project_root: &std::path::Path) -> std::io::Result<Vec<AssetInput>> {
    let raw_dir = project_root.join("raw");
    if !raw_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    walk(project_root, &raw_dir, &mut out)?;
    Ok(out)
}

fn walk(
    project_root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<AssetInput>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(project_root, &path, out)?;
        } else if path.is_file() {
            let id = path
                .strip_prefix(project_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(AssetInput {
                id: AssetId::new(id),
                path,
            });
        }
    }
    Ok(())
}

async fn register_job(state: &State<'_, AwidatState>, id: &Id) -> CancellationToken {
    let token = CancellationToken::new();
    state.jobs.lock().await.insert(
        id.0.clone(),
        JobHandle {
            cancel: token.clone(),
        },
    );
    token
}

async fn unregister_job(state: &State<'_, AwidatState>, id: &Id) {
    state.jobs.lock().await.remove(&id.0);
}
