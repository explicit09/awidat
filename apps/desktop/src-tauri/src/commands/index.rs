//! `index_project` command. Wraps `awidat_index::run` with the
//! progress callback added in commit dd15fe5, surfacing a single
//! [`Item::Job`] (kind = `Indexing`) that streams percent + status
//! as pairs complete.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use awidat_config::{Config, IndexerResourceClass, McpServer};
use awidat_desktop_protocol::{Id, JobKind};
use awidat_index::{
    AssetInput, IndexProgress, PairOutcome, ProgressCallback, media_files::collect_raw_media_inputs,
};
use awidat_mcp::ClientInfo;
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
    let mut servers: Vec<_> = config.indexers().cloned().collect();
    prepare_desktop_indexers(&mut servers);
    // Per-project overlay (Wave 4 T3) — drop any indexer the user has
    // disabled via the IndexersStrip popover. Reading the overlay file
    // server-side keeps the front-end IPC contract unchanged (no
    // `disabled: Vec<String>` arg threaded through every caller of
    // `index_project`); auto-chain runs from `import.rs` honor it too.
    // Fail-open: a missing or malformed file means "run everything",
    // same as the skill_config overlay.
    let disabled =
        crate::commands::indexer_config_overlay::load_disabled_indexers_sync(&project_root);
    if !disabled.is_empty() {
        servers.retain(|server| !disabled.iter().any(|name| name == &server.name));
    }
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
    emitter.progress(
        Some(0),
        format!(
            "preparing {} source media item(s) · hashing before indexers launch",
            assets.len()
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
        let mut active: std::collections::HashMap<String, (String, Instant)> =
            std::collections::HashMap::new();
        let started_at = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                maybe_evt = rx.recv() => {
                    let Some(evt) = maybe_evt else { break };
                    match evt {
                        IndexProgress::Started { total: t } => {
                            total = t;
                            emitter_for_task.progress(Some(0), format!("0 / {t} pairs"));
                        }
                        IndexProgress::PairStarted {
                            indexer,
                            asset,
                            completed,
                            total: t,
                        } => {
                            total = t;
                            let label = format!("{indexer} · {asset}");
                            active.insert(
                                pair_key(&indexer, &asset.to_string()),
                                (label.clone(), Instant::now()),
                            );
                            emitter_for_task.progress(
                                Some(last_percent),
                                format!("running {label} · {completed} / {t} pairs complete"),
                            );
                        }
                        IndexProgress::PairCompleted {
                            outcome,
                            completed,
                            total: t,
                        } => {
                            total = t;
                            active.remove(&outcome_pair_key(&outcome));
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
                _ = tick.tick() => {
                    if total > 0 {
                        emitter_for_task.progress(
                            Some(last_percent),
                            heartbeat_status(&active, started_at),
                        );
                    }
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

    // Built-in passes (motion + silence) live outside the MCP
    // dispatcher. By default they run *after* the dispatcher returns,
    // serializing what could be parallel. On machines with headroom
    // we spawn them concurrently with the MCP run instead — same
    // total CPU work, but the long-pole motion sampler doesn't sit
    // behind the python indexers anymore. The check is conservative
    // (≥8 cores AND load-avg under half of cores) so weaker
    // hardware doesn't get its other apps starved.
    let parallel = has_headroom_for_parallel_passes();
    let state_for_passes = state.inner();
    let passes_fut = async {
        if parallel {
            run_builtin_passes(app, state_for_passes, &project_root, &assets, &cancel).await
        } else {
            BuiltinPassReport::default()
        }
    };

    let (result, parallel_passes) = tokio::select! {
        _ = cancel.cancelled() => (Err("cancelled".into()), BuiltinPassReport::default()),
        r = async { tokio::join!(dispatch, passes_fut) } => {
            (r.0.map_err(|e| format!("dispatcher: {e}")), r.1)
        }
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
            // Built-in passes (motion + silence). Either ran in
            // parallel with the dispatcher (high-headroom machines)
            // or we run them now serially. Per-asset calls are
            // mtime-fresh-checked, so the second-pass invocation
            // skips any sidecar that landed during the parallel run.
            let post_passes = if parallel {
                BuiltinPassReport::default()
            } else {
                run_builtin_passes(app, state.inner(), &project_root, &assets, &cancel).await
            };
            let passes = BuiltinPassReport::merge(parallel_passes, post_passes);
            let mut summary = summary;
            if passes.motion_wrote > 0 || !passes.motion_failed.is_empty() {
                summary = format!(
                    "{summary}; motion: {} ok, {} failed",
                    passes.motion_wrote,
                    passes.motion_failed.len(),
                );
            }
            if passes.silence_wrote > 0 || !passes.silence_failed.is_empty() {
                summary = format!(
                    "{summary}; silence: {} ok, {} failed",
                    passes.silence_wrote,
                    passes.silence_failed.len(),
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
            if report.has_failures()
                || !passes.motion_failed.is_empty()
                || !passes.silence_failed.is_empty()
            {
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
                for (asset, message) in &passes.motion_failed {
                    let first = message.lines().next().unwrap_or(message);
                    let truncated = if first.len() > 200 {
                        format!("{}…", &first[..200])
                    } else {
                        first.to_string()
                    };
                    detail.push_str(&format!("\n  ✗ motion · {asset}: {truncated}"));
                }
                for (asset, message) in &passes.silence_failed {
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

fn pair_key(indexer: &str, asset: &str) -> String {
    format!("{indexer}\0{asset}")
}

fn outcome_pair_key(outcome: &PairOutcome) -> String {
    match outcome {
        PairOutcome::Skipped { indexer, asset, .. }
        | PairOutcome::Wrote { indexer, asset, .. }
        | PairOutcome::Failed { indexer, asset, .. }
        | PairOutcome::SkippedDep { indexer, asset, .. } => pair_key(indexer, &asset.to_string()),
    }
}

fn heartbeat_status(
    active: &std::collections::HashMap<String, (String, Instant)>,
    started_at: Instant,
) -> String {
    if let Some((label, at)) = active.values().min_by_key(|(_, at)| *at) {
        return format!(
            "still running {label} · {} elapsed",
            format_elapsed(at.elapsed())
        );
    }
    format!(
        "indexing still active · {} elapsed",
        format_elapsed(started_at.elapsed())
    )
}

fn format_elapsed(duration: Duration) -> String {
    let secs = duration.as_secs();
    let mins = secs / 60;
    let rem = secs % 60;
    if mins == 0 {
        format!("{rem}s")
    } else {
        format!("{mins}m {rem:02}s")
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

/// Outcome of one built-in-pass run (motion + silence). Accumulates
/// per-asset successes and failures so the caller can fold them into
/// the dispatcher's summary string and failure-detail block.
#[derive(Default)]
struct BuiltinPassReport {
    motion_wrote: usize,
    motion_failed: Vec<(String, String)>,
    silence_wrote: usize,
    silence_failed: Vec<(String, String)>,
}

impl BuiltinPassReport {
    /// Combine two reports (e.g. the parallel-during-dispatch result
    /// and the post-dispatch sweep). Counts add; failure lists
    /// concatenate.
    fn merge(mut a: Self, b: Self) -> Self {
        a.motion_wrote += b.motion_wrote;
        a.silence_wrote += b.silence_wrote;
        a.motion_failed.extend(b.motion_failed);
        a.silence_failed.extend(b.silence_failed);
        a
    }
}

/// Run motion + silence FFmpeg samplers for every asset, sequentially.
/// Each call is mtime-fresh-checked so re-invocation skips already-current
/// sidecars. Honors cancellation between every step.
async fn run_builtin_passes(
    app: &AppHandle,
    state: &AwidatState,
    project_root: &Path,
    assets: &[AssetInput],
    cancel: &CancellationToken,
) -> BuiltinPassReport {
    let mut report = BuiltinPassReport::default();
    for asset in assets {
        if cancel.is_cancelled() {
            break;
        }
        match crate::commands::motion::generate_motion_for_asset_in_project_inner(
            app,
            state,
            project_root,
            &asset.path,
        )
        .await
        {
            Ok(_) => report.motion_wrote += 1,
            Err(e) => report.motion_failed.push((asset.id.to_string(), e)),
        }
        if cancel.is_cancelled() {
            break;
        }
        match crate::commands::silence::generate_silences_for_asset_in_project_inner(
            app,
            state,
            project_root,
            &asset.path,
        )
        .await
        {
            Ok(_) => report.silence_wrote += 1,
            Err(e) => report.silence_failed.push((asset.id.to_string(), e)),
        }
    }
    report
}

/// Decide whether to run motion + silence in parallel with the MCP
/// dispatcher. Conservative: requires ≥8 physical cores AND a 1-min
/// load average under half the core count, so weak hardware (Intel
/// MacBooks, low-end laptops) gets the safe sequential path that
/// doesn't starve the user's other apps.
fn has_headroom_for_parallel_passes() -> bool {
    if std::env::var("AWIDAT_PARALLEL_PASSES").as_deref() == Ok("1") {
        return true;
    }
    if std::env::var("AWIDAT_PARALLEL_PASSES").as_deref() == Ok("0") {
        return false;
    }
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    if cores < 8 {
        return false;
    }
    match read_load_avg_1min() {
        Some(load) => load < (cores as f64) * 0.5,
        // No load-avg signal (unsupported platform). Conservative
        // default: don't parallelize — saving a few minutes isn't
        // worth pinning a user's laptop.
        None => false,
    }
}

/// Read the system 1-minute load average. macOS via sysctl,
/// Linux/BSD via /proc/loadavg. Returns None on Windows or any
/// failure — caller treats that as "no headroom signal".
fn read_load_avg_1min() -> Option<f64> {
    #[cfg(unix)]
    {
        if let Ok(contents) = std::fs::read_to_string("/proc/loadavg") {
            return contents
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok());
        }
        // macOS: shell out to sysctl. Avoids adding a libc dep just
        // for getloadavg(3).
        let output = std::process::Command::new("sysctl")
            .arg("-n")
            .arg("vm.loadavg")
            .output()
            .ok()?;
        let s = String::from_utf8(output.stdout).ok()?;
        // Output format: "{ 1.23 2.34 3.45 }"
        s.trim()
            .trim_start_matches('{')
            .trim_end_matches('}')
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
    }
    #[cfg(not(unix))]
    None
}

fn collect_assets(project_root: &std::path::Path) -> std::io::Result<Vec<AssetInput>> {
    collect_raw_media_inputs(project_root)
}

fn prepare_desktop_indexers(servers: &mut [McpServer]) {
    for server in servers.iter_mut() {
        if server.name == "whisper" && is_deepgram_whisper(server) {
            server.resource_class = IndexerResourceClass::Network;
        }
    }
    servers.sort_by_key(|server| desktop_indexer_priority(&server.name));
}

fn is_deepgram_whisper(server: &McpServer) -> bool {
    server
        .env
        .get("WHISPER_BACKEND")
        .is_some_and(|backend| backend.eq_ignore_ascii_case("deepgram"))
        || server.env.contains_key("DEEPGRAM_API_KEY")
}

fn desktop_indexer_priority(name: &str) -> u8 {
    match name {
        "whisper" => 0,
        "topic" => 1,
        "editorial-moments" => 2,
        "audio-energy" | "beats" => 3,
        "scenedetect" => 4,
        "frame-quality" | "color-analysis" => 5,
        "face" | "gaze" | "shot" | "clip" => 6,
        _ => 7,
    }
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
    let speaker =
        transcripts && any_whisper_sidecar_diarized(&project_root.join("index").join("whisper"));
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
    use std::collections::HashMap;

    fn test_server(name: &str) -> McpServer {
        McpServer {
            name: name.into(),
            command: "noop".into(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            kind: awidat_config::McpServerKind::Indexer,
            enabled: true,
            depends_on: vec![],
            resource_class: IndexerResourceClass::Light,
            indexer_group: None,
        }
    }

    #[test]
    fn desktop_indexers_start_deepgram_whisper_first_as_network_work() {
        let mut whisper = test_server("whisper");
        whisper.resource_class = IndexerResourceClass::Exclusive;
        whisper
            .env
            .insert("WHISPER_BACKEND".into(), "deepgram".into());
        let mut servers = vec![
            test_server("audio-energy"),
            test_server("beats"),
            test_server("scenedetect"),
            whisper,
            test_server("frame-quality"),
        ];

        prepare_desktop_indexers(&mut servers);

        assert_eq!(servers[0].name, "whisper");
        assert_eq!(servers[0].resource_class, IndexerResourceClass::Network);
        assert_eq!(servers[1].name, "audio-energy");
        assert_eq!(servers[2].name, "beats");
    }

    #[test]
    fn desktop_indexers_keep_local_whisper_exclusive_but_first() {
        let mut whisper = test_server("whisper");
        whisper.resource_class = IndexerResourceClass::Exclusive;
        let mut servers = vec![test_server("scenedetect"), whisper, test_server("beats")];

        prepare_desktop_indexers(&mut servers);

        assert_eq!(servers[0].name, "whisper");
        assert_eq!(servers[0].resource_class, IndexerResourceClass::Exclusive);
    }

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
        assert!(
            readiness.speaker,
            "diarized=true in whisper sidecar marks speaker ready"
        );
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
