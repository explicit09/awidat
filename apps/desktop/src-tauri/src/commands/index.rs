//! `index_project` command. Wraps `awidat_index::run` with the
//! progress callback added in commit dd15fe5, surfacing a single
//! [`Item::Job`] (kind = `Indexing`) that streams percent + status
//! as pairs complete.

use std::path::Path;
use std::sync::Arc;

use awidat_config::Config;
use awidat_desktop_protocol::{Id, JobKind};
use awidat_index::{AssetInput, IndexProgress, PairOutcome, ProgressCallback};
use awidat_mcp::ClientInfo;
use awidat_proto::index::AssetId;
use serde::Serialize;
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

#[derive(Debug, Clone, Serialize)]
pub struct IndexReadinessSnapshot {
    pub transcripts: bool,
    pub scenes: bool,
    pub audio: bool,
    pub face: bool,
    pub motion: bool,
    pub color: bool,
    pub silence: bool,
    pub speaker: bool,
    pub captions: bool,
    pub ready_count: usize,
    /// Total `shots` across every scenedetect sidecar. Distinct from
    /// `scenes` (the boolean "indexer has run"): a value of `0` means
    /// the indexer ran but found no cuts (e.g. a single continuous
    /// interview shot), while `scenes == false` means it hasn't run.
    pub scene_count: usize,
}

#[tauri::command]
pub async fn index_readiness(
    state: State<'_, AwidatState>,
) -> Result<IndexReadinessSnapshot, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    tokio::task::spawn_blocking(move || compute_index_readiness_at(&project_root))
        .await
        .map_err(|e| format!("index readiness join: {e}"))
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
        .unwrap_or(2);
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
            // Built-in passes (motion + silence). These are FFmpeg
            // helpers, not MCP indexers, so the dispatcher above
            // doesn't touch them. Run them here so a user clicking
            // "Run indexers" fills the sidecars for every asset —
            // same coverage they'd get from re-importing. Per-asset
            // calls are mtime-fresh-checked, so already-current
            // sidecars are no-ops.
            let mut motion_wrote = 0usize;
            let mut motion_failed: Vec<(String, String)> = Vec::new();
            let mut silence_wrote = 0usize;
            let mut silence_failed: Vec<(String, String)> = Vec::new();
            for asset in &assets {
                if cancel.is_cancelled() {
                    break;
                }
                match crate::commands::motion::generate_motion_for_asset_in_project(
                    app,
                    state,
                    &project_root,
                    &asset.path,
                )
                .await
                {
                    Ok(_) => motion_wrote += 1,
                    Err(e) => motion_failed.push((asset.id.to_string(), e)),
                }
                if cancel.is_cancelled() {
                    break;
                }
                match crate::commands::silence::generate_silences_for_asset_in_project(
                    app,
                    state,
                    &project_root,
                    &asset.path,
                )
                .await
                {
                    Ok(_) => silence_wrote += 1,
                    Err(e) => silence_failed.push((asset.id.to_string(), e)),
                }
            }
            let mut summary = summary;
            if motion_wrote > 0 || !motion_failed.is_empty() {
                summary = format!(
                    "{summary}; motion: {motion_wrote} ok, {} failed",
                    motion_failed.len(),
                );
            }
            if silence_wrote > 0 || !silence_failed.is_empty() {
                summary = format!(
                    "{summary}; silence: {silence_wrote} ok, {} failed",
                    silence_failed.len(),
                );
            }
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
            if report.has_failures() || !motion_failed.is_empty() || !silence_failed.is_empty() {
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
                        ..
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
                for (asset, message) in &motion_failed {
                    let first = message.lines().next().unwrap_or(message);
                    let truncated = if first.len() > 200 {
                        format!("{}…", &first[..200])
                    } else {
                        first.to_string()
                    };
                    detail.push_str(&format!("\n  ✗ motion · {asset}: {truncated}"));
                }
                for (asset, message) in &silence_failed {
                    let first = message.lines().next().unwrap_or(message);
                    let truncated = if first.len() > 200 {
                        format!("{}…", &first[..200])
                    } else {
                        first.to_string()
                    };
                    detail.push_str(&format!("\n  ✗ silence · {asset}: {truncated}"));
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
    let elapsed = compact_duration(o.telemetry().total);
    match o {
        PairOutcome::Wrote { indexer, asset, .. } => format!("✓ {indexer} · {asset} · {elapsed}"),
        PairOutcome::Skipped { indexer, asset, .. } => format!("· {indexer} · {asset} · {elapsed}"),
        PairOutcome::Failed { indexer, asset, .. } => format!("✗ {indexer} · {asset} · {elapsed}"),
        PairOutcome::SkippedDep { indexer, asset, .. } => {
            format!("⊘ {indexer} · {asset} · {elapsed}")
        }
    }
}

fn compact_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
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

fn compute_index_readiness_at(project_root: &Path) -> IndexReadinessSnapshot {
    let transcripts = any_json_file(&project_root.join("index").join("whisper"));
    let scenes = any_json_file(&project_root.join("index").join("scenedetect"));
    let audio = any_json_file(&project_root.join("index").join("audio-energy"));
    let face = any_json_file(&project_root.join("index").join("face"));
    let motion = any_json_file(&project_root.join(".awidat").join("motion"));
    let color = any_json_file(&project_root.join("index").join("color-analysis"));
    // Silence readiness is "do we have silence segments to feed
    // find_dead_air". audio-energy is RMS magnitude per frame, not
    // boundary segments — falling back to it produced a false-positive
    // "indexed" for silence. Only the real `.awidat/silences/` sidecar
    // counts.
    let silence = any_json_file(&project_root.join(".awidat").join("silences"));
    // Speaker labels live INSIDE the whisper sidecar's body when
    // diarization runs (`data.diarized: true` + `data.speakers: [...]`),
    // not in a separate `index/speaker/` directory. Check the whisper
    // sidecars for a diarized=true marker.
    let speaker = transcripts && any_whisper_sidecar_diarized(&project_root.join("index").join("whisper"));
    let captions = transcripts;
    let ready_count = [
        transcripts,
        scenes,
        audio,
        face,
        motion,
        color,
        silence,
        speaker,
        captions,
    ]
    .into_iter()
    .filter(|ready| *ready)
    .count();
    let scene_count = if scenes {
        count_scenes_in(&project_root.join("index").join("scenedetect"))
    } else {
        0
    };
    IndexReadinessSnapshot {
        transcripts,
        scenes,
        audio,
        face,
        motion,
        color,
        silence,
        speaker,
        captions,
        ready_count,
        scene_count,
    }
}

/// Walk every scenedetect sidecar under `dir` and sum the `data.shots`
/// array lengths. Returns 0 when nothing parses (worst case is an
/// underreported count; never crashes).
fn count_scenes_in(dir: &Path) -> usize {
    let mut total = 0_usize;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            total += count_scenes_in(&path);
        } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Ok(bytes) = std::fs::read(&path) {
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if let Some(shots) = value
                        .get("data")
                        .and_then(|d| d.get("shots"))
                        .and_then(|s| s.as_array())
                    {
                        total += shots.len();
                    }
                }
            }
        }
    }
    total
}

fn any_json_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if any_json_file(&path) {
                return true;
            }
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            return true;
        }
    }
    false
}

/// True iff any whisper sidecar under `dir` has `data.diarized: true`.
/// Used by the index-readiness check to flip the Speaker signal to
/// "Indexed" — diarization output lands inside the whisper sidecar,
/// not in a separate folder.
fn any_whisper_sidecar_diarized(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if any_whisper_sidecar_diarized(&path) {
                return true;
            }
            continue;
        }
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        {
            continue;
        }
        if sidecar_marks_diarized(&path) {
            return true;
        }
    }
    false
}

/// Returns true when a JSON file's `data.diarized` field is `true`.
/// Quietly returns false on any read/parse error — readiness is a
/// soft signal, not a load-bearing check.
fn sidecar_marks_diarized(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    json.get("data")
        .and_then(|data| data.get("diarized"))
        .and_then(|d| d.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_readiness_detects_existing_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("index/whisper/raw")).unwrap();
        std::fs::create_dir_all(dir.path().join("index/scenedetect/raw")).unwrap();
        std::fs::create_dir_all(dir.path().join("index/audio-energy/raw")).unwrap();
        std::fs::create_dir_all(dir.path().join("index/face/raw")).unwrap();
        std::fs::create_dir_all(dir.path().join("index/color-analysis/raw")).unwrap();
        std::fs::create_dir_all(dir.path().join(".awidat/motion")).unwrap();
        std::fs::create_dir_all(dir.path().join(".awidat/silences")).unwrap();

        std::fs::write(dir.path().join("index/whisper/raw/source.mov.json"), "{}").unwrap();
        std::fs::write(
            dir.path().join("index/scenedetect/raw/source.mov.json"),
            "{}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("index/audio-energy/raw/source.mov.json"),
            "{}",
        )
        .unwrap();
        std::fs::write(dir.path().join("index/face/raw/source.mov.json"), "{}").unwrap();
        std::fs::write(
            dir.path().join("index/color-analysis/raw/source.mov.json"),
            "{}",
        )
        .unwrap();
        std::fs::write(dir.path().join(".awidat/motion/source.json"), "{}").unwrap();
        std::fs::write(dir.path().join(".awidat/silences/source.json"), "{}").unwrap();

        let readiness = compute_index_readiness_at(dir.path());

        assert!(readiness.transcripts);
        assert!(readiness.scenes);
        assert!(readiness.audio);
        assert!(readiness.face);
        assert!(readiness.motion);
        assert!(readiness.color);
        assert!(readiness.silence);
        assert!(readiness.captions);
        // No diarized whisper sidecar in this fixture → speaker stays
        // false. The `ready_count` is 8 because every signal except
        // `speaker` is present.
        assert!(!readiness.speaker);
        assert_eq!(readiness.ready_count, 8);
    }

    #[test]
    fn speaker_readiness_flips_when_whisper_sidecar_has_diarized_true() {
        let dir = tempfile::tempdir().unwrap();
        let whisper_dir = dir.path().join("index/whisper/raw");
        std::fs::create_dir_all(&whisper_dir).unwrap();
        std::fs::write(
            whisper_dir.join("source.mov.json"),
            r#"{"data": {"diarized": true, "speakers": [{"id": "SPEAKER_00"}]}}"#,
        )
        .unwrap();

        let readiness = compute_index_readiness_at(dir.path());
        assert!(readiness.transcripts, "transcripts must be ready first");
        assert!(readiness.speaker, "diarized=true in whisper sidecar marks speaker ready");
    }

    #[test]
    fn speaker_readiness_stays_false_when_sidecar_has_diarized_false() {
        let dir = tempfile::tempdir().unwrap();
        let whisper_dir = dir.path().join("index/whisper/raw");
        std::fs::create_dir_all(&whisper_dir).unwrap();
        std::fs::write(
            whisper_dir.join("source.mov.json"),
            r#"{"data": {"diarized": false, "segments": []}}"#,
        )
        .unwrap();

        let readiness = compute_index_readiness_at(dir.path());
        assert!(readiness.transcripts);
        assert!(!readiness.speaker, "diarized=false leaves speaker unready");
    }
}
