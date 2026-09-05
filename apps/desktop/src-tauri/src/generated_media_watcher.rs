//! Generated-media registry watcher.
//!
//! The agent submits external video-gen jobs (OpenRouter+SeeDance,
//! etc.) via the MCP server, which writes to
//! `<project>/.montage/generated-media/registry.json`. The user can
//! see that state by hand but the desktop UI never surfaced it —
//! you'd watch the agent call `poll_generated_media_job` 20 times
//! in a row and have no idea whether anything was actually happening.
//!
//! This module spawns one watcher task per opened project. It diffs
//! the registry on each tick and emits `Item::Job { kind:
//! GeneratedMedia, … }` lifecycle events so the chat surface renders
//! progress in the regular [`crate::events::JobEmitter`] flow.
//! Frontend gets `Started` once per fresh queued/running record,
//! `Delta`s while a record is non-terminal, and a terminal
//! `Completed` when it succeeds/fails/cancels.
//!
//! The watcher is best-effort: registry-read failures are logged
//! and retried next tick.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use montage_core::generated_media::registry::{GeneratedMediaState, Registry};
use montage_desktop_protocol::{Id, Item, ItemLifecycle, JobKind, JobResult};
use tauri::AppHandle;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::events::emit_item;

/// Default tick interval. Three seconds is a fine balance for video
/// generation: tight enough that "queued → completed" feels live in
/// the UI, loose enough to not hammer the filesystem.
const TICK_INTERVAL: Duration = Duration::from_secs(3);

/// Handle returned to [`crate::state::MontageState`]; dropping cancels
/// the spawned watcher loop on the next tick.
#[allow(dead_code)]
pub struct GeneratedMediaWatcher {
    pub project_root: PathBuf,
    pub cancel: CancellationToken,
    pub wake: Arc<Notify>,
}

impl GeneratedMediaWatcher {
    /// Spawn the watcher. Returns a handle the caller stores in
    /// state; `cancel.cancel()` stops the loop.
    pub fn spawn(app: AppHandle, project_root: PathBuf) -> Self {
        let cancel = CancellationToken::new();
        let wake = Arc::new(Notify::new());
        let task_cancel = cancel.clone();
        let task_wake = Arc::clone(&wake);
        let task_root = project_root.clone();
        tokio::spawn(async move {
            run_loop(app, task_root, task_cancel, task_wake).await;
        });
        Self {
            project_root,
            cancel,
            wake,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Phase {
    Active,
    Terminal,
}

async fn run_loop(
    app: AppHandle,
    project_root: PathBuf,
    cancel: CancellationToken,
    wake: Arc<Notify>,
) {
    // Track records we've already announced + their last-known phase.
    // Cuts down redundant emissions and lets us send a `Completed`
    // exactly once per job. `montage-` prefix on item ids so the chat
    // stream renderer doesn't collide them with any other Items.
    let mut seen: HashMap<String, Phase> = HashMap::new();

    loop {
        match Registry::load_or_default(&project_root) {
            Ok(registry) => {
                let live_ids: HashSet<String> =
                    registry.records.iter().map(|r| r.job_id.clone()).collect();

                for record in &registry.records {
                    let phase = match record.state {
                        GeneratedMediaState::Succeeded
                        | GeneratedMediaState::Failed
                        | GeneratedMediaState::Cancelled => Phase::Terminal,
                        // Treat Draft as Active for UI purposes — it
                        // appears once start_generated_media_job
                        // creates the record but before submission
                        // lands; the user wants to see it.
                        _ => Phase::Active,
                    };
                    let id = Id::new(format!("montage-genmedia-{}", record.job_id));
                    let status = describe(record);
                    let output_path = record
                        .output_paths
                        .get("video")
                        .map(|rel| project_root.join(rel).display().to_string());

                    match (seen.get(&record.job_id), &phase) {
                        // First time we've seen this record: emit
                        // Started.
                        (None, _) => {
                            emit_item(
                                &app,
                                Item::Job {
                                    id: id.clone(),
                                    phase: ItemLifecycle::Started,
                                    job_kind: JobKind::GeneratedMedia,
                                    percent: None,
                                    status,
                                    result: None,
                                    output_path: output_path.clone(),
                                },
                            );
                        }
                        // Was active, still active: heartbeat as Delta
                        // so the renderer keeps the spinner alive.
                        (Some(Phase::Active), Phase::Active) => {
                            emit_item(
                                &app,
                                Item::Job {
                                    id: id.clone(),
                                    phase: ItemLifecycle::Delta,
                                    job_kind: JobKind::GeneratedMedia,
                                    percent: None,
                                    status,
                                    result: None,
                                    output_path: output_path.clone(),
                                },
                            );
                        }
                        // Was active, now terminal: emit one final
                        // Completed with the right JobResult.
                        (Some(Phase::Active), Phase::Terminal) => {
                            let summary = Some(describe(record));
                            let result = match record.state {
                                GeneratedMediaState::Succeeded => JobResult::Ok { summary },
                                GeneratedMediaState::Failed => JobResult::Err {
                                    message: record
                                        .failure_message
                                        .clone()
                                        .unwrap_or_else(|| "generation failed".into()),
                                },
                                GeneratedMediaState::Cancelled => JobResult::Cancelled,
                                _ => JobResult::Ok { summary },
                            };
                            emit_item(
                                &app,
                                Item::Job {
                                    id: id.clone(),
                                    phase: ItemLifecycle::Completed,
                                    job_kind: JobKind::GeneratedMedia,
                                    percent: Some(100),
                                    status: describe(record),
                                    result: Some(result),
                                    output_path,
                                },
                            );
                        }
                        // Already terminal in our seen-set; nothing to do.
                        (Some(Phase::Terminal), _) => {}
                    }

                    seen.insert(record.job_id.clone(), phase);
                }

                // Drop seen entries for records that were removed from
                // the registry (rare but possible if the user prunes
                // it manually).
                seen.retain(|id, _| live_ids.contains(id));
            }
            Err(err) => {
                tracing::warn!(error = %err, "generated-media watcher: registry load");
            }
        }

        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = wake.notified() => {}
            _ = tokio::time::sleep(TICK_INTERVAL) => {}
        }
    }
}

fn describe(record: &montage_core::generated_media::registry::GeneratedMediaRecord) -> String {
    let prompt = first_line(&record.prompt);
    match record.state {
        GeneratedMediaState::Draft => format!("draft · {prompt}"),
        GeneratedMediaState::Queued => format!("queued · {prompt}"),
        GeneratedMediaState::Running => format!("generating · {prompt}"),
        GeneratedMediaState::Succeeded => format!("ready · {prompt}"),
        GeneratedMediaState::Failed => {
            let why = record.failure_message.as_deref().unwrap_or("unknown error");
            format!("failed · {prompt} — {why}")
        }
        GeneratedMediaState::Cancelled => format!("cancelled · {prompt}"),
    }
}

fn first_line(s: &str) -> String {
    let trimmed = s.trim();
    let cap = 80;
    let head = trimmed.lines().next().unwrap_or(trimmed);
    if head.chars().count() > cap {
        let cut: String = head.chars().take(cap).collect();
        format!("{cut}…")
    } else {
        head.to_string()
    }
}
