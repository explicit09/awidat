//! Project lifecycle commands: opening an existing project, creating
//! a new one, listing recent ones. Import + index commands live in
//! `import.rs` / `index.rs` (next commits).

use std::path::{Path, PathBuf};

use montage_proto::montage_meta::{EpisodeSpan, EpisodeSpanStatus};
use montage_proto::project::Project;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tokio::fs;

use crate::state::MontageState;

/// Reconfigure Tauri's asset-protocol scope so the webview can fetch
/// preview media and project-local broadcast assets via
/// `convertFileSrc()`. We allow only project-owned roots — the scope
/// is otherwise empty (set in tauri.conf.json) so the asset protocol
/// cannot be abused to read arbitrary files. Called from
/// `set_project_root` and `init_project`.
pub(crate) fn allow_project_asset_dirs(app: &AppHandle, project_root: &Path) {
    let scope = app.asset_protocol_scope();
    for sub in [".montage/proxies", ".montage/thumbnails", "branding"] {
        let dir = project_root.join(sub);
        // The dirs may not exist yet (first import will create them).
        // Allow them preemptively — the scope check is a glob, not a
        // stat.
        if let Err(e) = scope.allow_directory(&dir, true) {
            tracing::warn!(
                error = %e,
                path = %dir.display(),
                "failed to allow asset-protocol scope dir",
            );
        }
    }
}

/// Maximum number of recent project paths to remember.
const MAX_RECENTS: usize = 10;

/// Set / change the project root for subsequent turns. Resets any
/// existing `Session` so the next `start_turn` rebuilds against the
/// new root. Pushes the path to the recents list on success.
#[tauri::command]
pub async fn set_project_root(
    app: AppHandle,
    state: State<'_, MontageState>,
    path: String,
) -> Result<(), String> {
    ensure_project_switch_allowed(&state).await?;
    let buf = PathBuf::from(&path);
    if !buf.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    // Cheap sanity check that the directory looks like a project. We
    // don't fully read+validate the OTIO here — too expensive on every
    // open. Just verify the manifest sentinel exists. Empty dirs and
    // arbitrary unrelated dirs get rejected.
    let manifest = buf.join("project.otio.json");
    if !manifest.is_file() {
        return Err(format!(
            "not an montage project (no project.otio.json under {path})"
        ));
    }

    *state.project_root.lock().await = Some(buf.clone());
    // The codex bridge holds project_root + MCP env in its Config, so
    // a project change requires a fresh bridge. Tear down here; the
    // next `start_turn` will launch a new one (lazily) against `buf`.
    tear_down_codex_session(&state).await;
    spawn_generated_media_watcher(&state, &app, &buf).await;
    crate::commands::media::clear_media_server_files(&state)?;
    allow_project_asset_dirs(&app, &buf);

    // Best-effort: ignore failures so a corrupted recents file
    // doesn't block project opening.
    if let Err(e) = update_recents(&buf).await {
        tracing::warn!(error = %e, "failed to update recents file");
    }

    // Reconcile the proxy cache against the current schema. Prunes
    // orphaned proxies from previous encoder targets and backfills
    // any missing 1080p proxy in the background. Without this the
    // preview hangs on "Generating preview…" forever when a user
    // opens a project that was previously proxied under an older
    // filename schema (the auto-transcode otherwise only runs on
    // fresh imports).
    crate::commands::transcode::spawn_proxy_backfill_on_load(app.clone(), buf.clone());

    // Backfill thumbnail + waveform sidecars for any asset whose
    // post-import chain never completed (e.g. project was opened
    // from a previous schema, or import was interrupted). Without
    // this, the timeline canvas falls back to plain rects with no
    // filmstrip or amplitude line on otherwise-valid clips.
    crate::commands::transcode::spawn_sidecar_backfill_on_load(app.clone(), buf);

    Ok(())
}

/// Read the currently-configured project root, if any.
#[tauri::command]
pub async fn current_project_root(
    state: State<'_, MontageState>,
) -> Result<Option<String>, String> {
    Ok(state
        .project_root
        .lock()
        .await
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned()))
}

/// Close the current project without opening a replacement. Mirrors
/// `set_project_root`'s safety checks so we do not strand pending
/// proposals, turns, or import/index/render jobs against a project
/// the UI no longer shows.
#[tauri::command]
pub async fn close_project(state: State<'_, MontageState>) -> Result<(), String> {
    ensure_project_switch_allowed(&state).await?;
    *state.project_root.lock().await = None;
    tear_down_codex_session(&state).await;
    tear_down_generated_media_watcher(&state).await;
    crate::commands::media::clear_media_server_files(&state)?;
    Ok(())
}

/// Initialize a new montage project at `<parent_dir>/<name>` and load
/// it as the current project. Mirrors `montage new --no-md=false
/// --no-index` (init + starter AGENTS.md, no asset import). Asset
/// import is a separate step the frontend can chain after.
///
/// `project_type` is optional — when present, it's serialized into
/// the timeline's `metadata.montage.extra["montage_project_type"]`
/// slot so the agent can pick up the per-format defaults on session
/// start. When absent (e.g. older clients), the project type
/// defaults to `Other { description: "" }` which gets the neutral
/// system-prompt baseline.
#[tauri::command]
pub async fn init_project(
    app: AppHandle,
    state: State<'_, MontageState>,
    parent_dir: String,
    name: String,
    project_type: Option<montage_desktop_protocol::ProjectType>,
) -> Result<String, String> {
    ensure_project_switch_allowed(&state).await?;
    let parent = PathBuf::from(&parent_dir);
    if !parent.is_dir() {
        return Err(format!("parent is not a directory: {parent_dir}"));
    }
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(format!("invalid project name: {name:?}"));
    }
    let project_dir = parent.join(&name);
    if project_dir.exists() {
        // Match the CLI's behavior — refuse to clobber non-empty dirs.
        let mut entries = fs::read_dir(&project_dir)
            .await
            .map_err(|e| format!("inspect {}: {e}", project_dir.display()))?;
        if entries
            .next_entry()
            .await
            .map_err(|e| format!("inspect {}: {e}", project_dir.display()))?
            .is_some()
        {
            return Err(format!(
                "target directory exists and is not empty: {}",
                project_dir.display()
            ));
        }
    }

    let project_dir_for_init = project_dir.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        Project::init(&project_dir_for_init).map_err(|e| e.to_string())?;
        montage_core::lessons::apply_learned_project_format_defaults(&project_dir_for_init)
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| format!("init join: {e}"))?
    .map_err(|e| format!("init: {e}"))?;

    // Starter AGENTS.md so the project is "ready" with no extra
    // hand-holding from the user. The CLI's `--no-md` flag isn't
    // exposed here — desktop init always writes it. If the user
    // doesn't want it they delete the file.
    let md_path = project_dir.join("AGENTS.md");
    fs::write(&md_path, AGENTS_MD_TEMPLATE)
        .await
        .map_err(|e| format!("write AGENTS.md: {e}"))?;

    // Stamp project_type into the OTIO timeline's metadata.montage.extra
    // slot if the caller specified one. Done as a separate step (post-
    // Project::init) so the typed Project schema doesn't need to grow
    // a new field for what's essentially a forward-compat passthrough.
    if let Some(pt) = project_type {
        if let Err(e) = write_project_type_to_otio(&project_dir, &pt).await {
            tracing::warn!(error = %e, "failed to stamp project_type at init; using default");
        }
    }

    *state.project_root.lock().await = Some(project_dir.clone());
    // Drain any pre-existing bridge so the next start_turn picks up
    // the new project_root.
    tear_down_codex_session(&state).await;
    spawn_generated_media_watcher(&state, &app, &project_dir).await;
    allow_project_asset_dirs(&app, &project_dir);
    if let Err(e) = update_recents(&project_dir).await {
        tracing::warn!(error = %e, "failed to update recents file");
    }

    Ok(project_dir.to_string_lossy().into_owned())
}

/// Read the project type from the currently-loaded project, if any.
/// Returns `Other { description: "" }` when no project is loaded or
/// when the OTIO file's metadata.montage.extra has no
/// `montage_project_type` key (old projects, or projects created
/// before the picker landed).
#[tauri::command]
pub async fn get_project_type(
    state: State<'_, MontageState>,
) -> Result<montage_desktop_protocol::ProjectType, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => {
            return Ok(montage_desktop_protocol::ProjectType::Other {
                description: String::new(),
            });
        }
    };
    Ok(read_project_type_from_otio(&project_root).await.unwrap_or(
        montage_desktop_protocol::ProjectType::Other {
            description: String::new(),
        },
    ))
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectEpisodesSummary {
    pub total: usize,
    pub accepted: usize,
    pub review_needed: usize,
    pub rejected: usize,
    pub episodes: Vec<ProjectEpisodeSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectEpisodeSummary {
    pub id: String,
    pub name: String,
    pub order: u32,
    pub asset_id: String,
    pub start_s: f64,
    pub end_s: f64,
    pub duration_s: f64,
    pub confidence: f64,
    pub status: String,
    pub evidence_count: usize,
}

/// Return first-class episode spans stamped on the currently-loaded timeline.
#[tauri::command]
pub async fn get_project_episodes(
    state: State<'_, MontageState>,
) -> Result<ProjectEpisodesSummary, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    tokio::task::spawn_blocking(move || summarize_project_episodes(&project_root))
        .await
        .map_err(|e| format!("episodes join: {e}"))?
}

fn summarize_project_episodes(project_root: &Path) -> Result<ProjectEpisodesSummary, String> {
    let project = Project::read(project_root).map_err(|e| format!("read project: {e}"))?;
    let mut episodes: Vec<ProjectEpisodeSummary> = project
        .timeline
        .metadata
        .montage
        .as_ref()
        .map(|meta| {
            meta.episodes
                .iter()
                .map(project_episode_summary)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    episodes.sort_by(|a, b| {
        a.order
            .cmp(&b.order)
            .then_with(|| a.start_s.total_cmp(&b.start_s))
            .then_with(|| a.id.cmp(&b.id))
    });
    let accepted = episodes
        .iter()
        .filter(|episode| episode.status == "accepted")
        .count();
    let review_needed = episodes
        .iter()
        .filter(|episode| episode.status == "review_needed")
        .count();
    let rejected = episodes
        .iter()
        .filter(|episode| episode.status == "rejected")
        .count();
    Ok(ProjectEpisodesSummary {
        total: episodes.len(),
        accepted,
        review_needed,
        rejected,
        episodes,
    })
}

fn project_episode_summary(episode: &EpisodeSpan) -> ProjectEpisodeSummary {
    ProjectEpisodeSummary {
        id: episode.id.clone(),
        name: episode.name.clone().unwrap_or_default(),
        order: episode.order.unwrap_or(0),
        asset_id: episode.asset_id.clone(),
        start_s: episode.source_start_s,
        end_s: episode.source_end_s,
        duration_s: episode.duration_s(),
        confidence: episode.confidence.unwrap_or(0.0),
        status: episode_status_label(&episode.status).to_string(),
        evidence_count: episode.evidence.len(),
    }
}

fn episode_status_label(status: &EpisodeSpanStatus) -> &'static str {
    match status {
        EpisodeSpanStatus::ReviewNeeded => "review_needed",
        EpisodeSpanStatus::Accepted => "accepted",
        EpisodeSpanStatus::Rejected => "rejected",
    }
}

/// Update the project type on the currently-loaded project. Persists
/// to OTIO immediately so the next agent session-start picks it up.
#[tauri::command]
pub async fn set_project_type(
    state: State<'_, MontageState>,
    project_type: montage_desktop_protocol::ProjectType,
) -> Result<(), String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    write_project_type_to_otio(&project_root, &project_type).await
}

/// Slot key inside `Timeline.metadata.montage.extra` where the project
/// type lives. Kept as a constant so the agent-side reader can
/// reference the same key without copy-paste drift.
const PROJECT_TYPE_KEY: &str = "montage_project_type";

async fn write_project_type_to_otio(
    project_root: &Path,
    project_type: &montage_desktop_protocol::ProjectType,
) -> Result<(), String> {
    let otio_path = project_root.join(montage_proto::project::files::OTIO);
    let bytes = fs::read(&otio_path)
        .await
        .map_err(|e| format!("read otio: {e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("parse otio json: {e}"))?;
    // metadata.montage.extra is what we want — but `extra` is a
    // `#[serde(flatten)]` HashMap, which means at the JSON layer the
    // entries land directly inside `metadata.montage`. Walk to that
    // object and insert our key alongside the version field.
    let montage_meta = value
        .pointer_mut("/metadata/montage")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| "otio file missing metadata.montage".to_string())?;
    let pt_value =
        serde_json::to_value(project_type).map_err(|e| format!("serialize project_type: {e}"))?;
    montage_meta.insert(PROJECT_TYPE_KEY.to_string(), pt_value);
    let serialized =
        serde_json::to_vec_pretty(&value).map_err(|e| format!("re-serialize otio: {e}"))?;
    fs::write(&otio_path, serialized)
        .await
        .map_err(|e| format!("write otio: {e}"))?;
    Ok(())
}

async fn read_project_type_from_otio(
    project_root: &Path,
) -> Option<montage_desktop_protocol::ProjectType> {
    let otio_path = project_root.join(montage_proto::project::files::OTIO);
    let bytes = fs::read(&otio_path).await.ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let raw = value
        .pointer(&format!("/metadata/montage/{PROJECT_TYPE_KEY}"))?
        .clone();
    serde_json::from_value(raw).ok()
}

async fn ensure_project_switch_allowed(state: &State<'_, MontageState>) -> Result<(), String> {
    if state.turn.lock().await.is_some() {
        return Err("cannot change projects while a turn is running; stop it first".into());
    }
    if !state.pending_proposals.lock().await.is_empty() {
        return Err("cannot change projects while an edit proposal is pending".into());
    }
    if !state.jobs.lock().await.is_empty() {
        return Err("cannot change projects while an import/index/transcode job is running".into());
    }
    Ok(())
}

/// Drain any live codex bridge for the current project so the next
/// `start_turn` rebuilds against the new cwd + MCP env override.
/// Must be called AFTER `ensure_project_switch_allowed` (which refuses
/// a switch if a turn is in flight) so we don't kill a running turn's
/// pump task mid-event.
pub(super) async fn tear_down_codex_session(state: &State<'_, MontageState>) {
    if let Some(session) = state.codex.lock().await.take() {
        if let Err(e) = session.bridge.shutdown().await {
            tracing::warn!(error = %e, "codex bridge shutdown on project switch returned error");
        }
    }
}

/// Spawn the generated-media registry watcher for `project_root`.
/// Replaces any previously-running watcher (e.g. from a stale project
/// switch path).
pub(super) async fn spawn_generated_media_watcher(
    state: &State<'_, MontageState>,
    app: &AppHandle,
    project_root: &std::path::Path,
) {
    // Drop any prior watcher first so cancellation propagates on the
    // next tick.
    if let Some(prev) = state.generated_media_watcher.lock().await.take() {
        prev.cancel.cancel();
    }
    let handle = crate::generated_media_watcher::GeneratedMediaWatcher::spawn(
        app.clone(),
        project_root.to_path_buf(),
    );
    *state.generated_media_watcher.lock().await = Some(handle);
}

/// Cancel and drop the active generated-media watcher (if any).
pub(super) async fn tear_down_generated_media_watcher(state: &State<'_, MontageState>) {
    if let Some(watcher) = state.generated_media_watcher.lock().await.take() {
        watcher.cancel.cancel();
    }
}

/// Return the most recently opened project paths, newest first.
/// Stale paths (deleted directories, moved projects) are filtered
/// out so the UI never offers a path that won't open.
#[tauri::command]
pub async fn recent_projects() -> Result<Vec<String>, String> {
    let path = match recents_path() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };
    let bytes = match fs::read(&path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read recents: {e}")),
    };
    let raw: Vec<String> =
        serde_json::from_slice(&bytes).map_err(|e| format!("parse recents: {e}"))?;
    let mut out = Vec::with_capacity(raw.len());
    for p in raw {
        let path = PathBuf::from(&p);
        if path.is_dir() && path.join("project.otio.json").is_file() {
            out.push(p);
        }
    }
    Ok(out)
}

/// Push a path to the front of the recents list, dedup, cap to
/// [`MAX_RECENTS`]. Creates the recents file (and its parent dir)
/// if either is missing.
async fn update_recents(p: &std::path::Path) -> std::io::Result<()> {
    let Some(file) = recents_path() else {
        return Ok(());
    };
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut existing: Vec<String> = match fs::read(&file).await {
        Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    let p_str = p.to_string_lossy().into_owned();
    existing.retain(|x| x != &p_str);
    existing.insert(0, p_str);
    existing.truncate(MAX_RECENTS);
    let serialized = serde_json::to_vec_pretty(&existing).unwrap_or_else(|_| b"[]".to_vec());
    fs::write(&file, serialized).await?;
    Ok(())
}

/// Symmetric counterpart to [`update_recents`]: drop every entry whose
/// stored string equals `p`. Matching is byte-for-byte against what
/// `update_recents` originally wrote (no canonicalization), so the
/// caller must pass the same path string the recents UI is showing.
async fn remove_from_recents(p: &std::path::Path) -> std::io::Result<()> {
    let Some(file) = recents_path() else {
        return Ok(());
    };
    prune_recents_file(&file, p).await
}

/// Internal helper for [`remove_from_recents`] that takes the recents
/// file path explicitly. Split out so unit tests can target a tempdir
/// instead of mutating the user's real config dir.
async fn prune_recents_file(file: &std::path::Path, p: &std::path::Path) -> std::io::Result<()> {
    let existing_bytes = match fs::read(file).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut existing: Vec<String> = serde_json::from_slice(&existing_bytes).unwrap_or_default();
    let p_str = p.to_string_lossy();
    let before = existing.len();
    existing.retain(|x| x.as_str() != p_str.as_ref());
    if existing.len() == before {
        return Ok(());
    }
    let serialized = serde_json::to_vec_pretty(&existing).unwrap_or_else(|_| b"[]".to_vec());
    fs::write(file, serialized).await?;
    Ok(())
}

/// Compute the on-disk size of `path` recursively (files + dirs). Used
/// by the delete-project confirm modal so the user sees how much they
/// are about to free. Errors on individual entries (permission, broken
/// symlink) are skipped so the total is best-effort rather than
/// load-bearing — better to under-report than refuse to open the modal.
#[tauri::command]
pub async fn project_size_bytes(path: String) -> Result<u64, String> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    tokio::task::spawn_blocking(move || dir_size_recursive(&root))
        .await
        .map_err(|e| format!("size join: {e}"))
}

fn dir_size_recursive(dir: &Path) -> u64 {
    let mut total: u64 = 0;
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return 0,
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total = total.saturating_add(dir_size_recursive(&entry.path()));
        } else if meta.is_file() {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

/// Permanently delete `path` from disk and from the recents list. The
/// destructive shape requires extra care:
///
/// - `expected_basename` is a guard the frontend sends along with the
///   path it last saw; if the on-disk folder was renamed (Finder,
///   another process) between the modal opening and the user clicking
///   Delete, the basename mismatch refuses the request rather than
///   nuking an unrelated directory.
/// - We require a `project.otio.json` sentinel before removing, so a
///   stale recents entry pointing at an unrelated folder cannot lead
///   to a wrong `rm -rf`.
/// - When the path matches the currently-loaded project, we close it
///   first using the same `ensure_project_switch_allowed` guard that
///   guards every other project switch. That ensures no in-flight
///   turn / proposal / transcode is holding state against the doomed
///   directory.
/// - Recents prune happens BEFORE the `remove_dir_all` so a partial
///   delete still results in a consistent UI: `recent_projects`'
///   stale-path filter cleans up whatever's left.
#[tauri::command]
pub async fn delete_project(
    state: State<'_, MontageState>,
    path: String,
    expected_basename: String,
) -> Result<(), String> {
    let buf = validate_delete_target(&path, &expected_basename)?;

    let is_active = state
        .project_root
        .lock()
        .await
        .as_ref()
        .is_some_and(|current| current == &buf);
    if is_active {
        ensure_project_switch_allowed(&state).await?;
        *state.project_root.lock().await = None;
        tear_down_codex_session(&state).await;
        tear_down_generated_media_watcher(&state).await;
        crate::commands::media::clear_media_server_files(&state)?;
    }

    if let Err(e) = remove_from_recents(&buf).await {
        tracing::warn!(error = %e, path = %path, "failed to prune recents; continuing with delete");
    }

    tokio::fs::remove_dir_all(&buf)
        .await
        .map_err(|e| format!("delete: {e}"))?;
    Ok(())
}

/// Pure validation half of [`delete_project`], split out for testability.
/// Confirms the path is a real directory, that the on-disk basename
/// matches what the UI thought it was deleting (catches rename races),
/// and that the folder carries the Montage sentinel so we cannot be
/// tricked into `rm -rf`ing an unrelated directory.
fn validate_delete_target(path: &str, expected_basename: &str) -> Result<PathBuf, String> {
    let buf = PathBuf::from(path);
    if !buf.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    let basename = buf.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    if basename != expected_basename {
        return Err(format!(
            "project folder was renamed (expected {expected_basename:?}, found {basename:?}); refresh and try again"
        ));
    }
    if !buf.join("project.otio.json").is_file() {
        return Err(format!(
            "refusing to delete non-Montage directory: {path} (no project.otio.json sentinel)"
        ));
    }
    Ok(buf)
}

/// Cancel an in-flight long job (yt-dlp download, indexer run) by
/// its protocol-Item id. No-op if the id isn't currently running.
#[tauri::command]
pub async fn cancel_job(state: State<'_, MontageState>, job_id: String) -> Result<(), String> {
    if let Some(handle) = state.jobs.lock().await.get(&job_id) {
        handle.cancel.cancel();
    }
    Ok(())
}

/// Return job ids currently tracked by the live backend process.
#[tauri::command]
pub async fn running_job_ids(state: State<'_, MontageState>) -> Result<Vec<String>, String> {
    Ok(state.jobs.lock().await.keys().cloned().collect())
}

/// Path to the recents file.
///
/// macOS: `~/Library/Application Support/montage-desktop/recents.json`
/// Linux: `~/.config/montage-desktop/recents.json`
/// Windows: `%APPDATA%\montage-desktop\recents.json`
///
/// Returns `None` if the OS doesn't expose a config dir (we silently
/// drop recents in that case rather than failing).
fn recents_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("montage-desktop").join("recents.json"))
}

/// Starter AGENTS.md identical to the CLI's. Kept inline rather than
/// shared with new_cmd.rs because the desktop binary mustn't depend
/// on the CLI crate (would be circular: CLI bundles desktop in
/// release builds, eventually).
const AGENTS_MD_TEMPLATE: &str = "\
# Project conventions

This file is read by montage at session start and added to the agent's \
system prompt. Use it to record editorial conventions, ground rules, \
and per-episode constraints. Edit freely; remove sections you don't \
need. Subdirectories may also have their own `AGENTS.md` for narrower \
scope.

## Speakers

- Speaker A: <name / role>
- Speaker B: <name / role>

## Style

- Cut breath buffer: 200ms before / 100ms after each take
- Cross-talk: prefer the speaker who finishes their thought
- Filler removal: aggressive on um/uh, conservative on 'like'/'you know'

## Avoid

- Don't trim hooks below 5s
- Don't render with hard cuts mid-laugh
- Don't move the CTA out of the closing third

## Render targets

- Master: 1080p, ProRes 422 (or full-quality H.264 if no ProRes pipeline)
- Social: 1080×1920 (vertical) crops of standalone moments
";

#[cfg(test)]
mod tests {
    use super::*;

    fn make_fake_project(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("project.otio.json"), b"{}").unwrap();
        dir
    }

    #[test]
    fn validate_delete_target_accepts_real_montage_project() {
        let tmp = tempfile::tempdir().unwrap();
        let project = make_fake_project(tmp.path(), "demo");
        let buf = validate_delete_target(project.to_str().unwrap(), "demo").unwrap();
        assert_eq!(buf, project);
    }

    #[test]
    fn validate_delete_target_refuses_missing_path() {
        let err = validate_delete_target("/no/such/montage/project", "project").unwrap_err();
        assert!(err.contains("not a directory"), "got: {err}");
    }

    #[test]
    fn validate_delete_target_refuses_basename_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let project = make_fake_project(tmp.path(), "renamed-on-disk");
        let err =
            validate_delete_target(project.to_str().unwrap(), "what-the-ui-thought").unwrap_err();
        assert!(err.contains("renamed"), "got: {err}");
    }

    #[test]
    fn validate_delete_target_refuses_directory_without_otio_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("definitely-not-montage");
        std::fs::create_dir_all(&bogus).unwrap();
        std::fs::write(bogus.join("README.md"), b"not a project").unwrap();
        let err =
            validate_delete_target(bogus.to_str().unwrap(), "definitely-not-montage").unwrap_err();
        assert!(err.contains("non-Montage directory"), "got: {err}");
    }

    #[tokio::test]
    async fn prune_recents_file_drops_only_the_targeted_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let recents = tmp.path().join("recents.json");
        let initial = serde_json::to_vec(&vec![
            "/a/keep".to_string(),
            "/b/delete".to_string(),
            "/c/keep".to_string(),
        ])
        .unwrap();
        fs::write(&recents, initial).await.unwrap();

        prune_recents_file(&recents, std::path::Path::new("/b/delete"))
            .await
            .unwrap();

        let after: Vec<String> =
            serde_json::from_slice(&fs::read(&recents).await.unwrap()).unwrap();
        assert_eq!(after, vec!["/a/keep".to_string(), "/c/keep".to_string()]);
    }

    #[tokio::test]
    async fn prune_recents_file_is_noop_when_entry_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let recents = tmp.path().join("recents.json");
        let initial = serde_json::to_vec(&vec!["/a".to_string(), "/b".to_string()]).unwrap();
        fs::write(&recents, &initial).await.unwrap();

        prune_recents_file(&recents, std::path::Path::new("/missing"))
            .await
            .unwrap();

        assert_eq!(fs::read(&recents).await.unwrap(), initial);
    }

    #[tokio::test]
    async fn prune_recents_file_is_noop_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let recents = tmp.path().join("does-not-exist.json");
        prune_recents_file(&recents, std::path::Path::new("/anything"))
            .await
            .unwrap();
        assert!(!recents.exists());
    }

    #[tokio::test]
    async fn project_size_bytes_sums_files_recursively() {
        let tmp = tempfile::tempdir().unwrap();
        let project = make_fake_project(tmp.path(), "sized");
        std::fs::write(project.join("a.bin"), vec![0u8; 100]).unwrap();
        let nested = project.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("b.bin"), vec![0u8; 250]).unwrap();
        let total = project_size_bytes(project.to_string_lossy().into_owned())
            .await
            .unwrap();
        // 350 from our writes + 2 bytes for "{}" in project.otio.json.
        assert_eq!(total, 100 + 250 + 2);
    }

    #[tokio::test]
    async fn project_size_bytes_errors_on_non_dir() {
        let err = project_size_bytes("/no/such/place".into())
            .await
            .unwrap_err();
        assert!(err.contains("not a directory"), "got: {err}");
    }

    #[test]
    fn summarize_project_episodes_sorts_and_counts_statuses() {
        let tmp = tempfile::tempdir().unwrap();
        Project::init(tmp.path()).unwrap();
        let mut project = Project::read(tmp.path()).unwrap();
        project.timeline.metadata.montage.as_mut().unwrap().episodes = vec![
            EpisodeSpan {
                id: "needs-review".into(),
                name: Some("Needs Review".into()),
                order: Some(2),
                asset_id: "asset-a".into(),
                source_start_s: 240.0,
                source_end_s: 300.0,
                confidence: Some(0.74),
                status: EpisodeSpanStatus::ReviewNeeded,
                evidence: vec!["intro phrase".into()],
                extra: Default::default(),
            },
            EpisodeSpan {
                id: "accepted".into(),
                name: Some("Accepted".into()),
                order: Some(1),
                asset_id: "asset-a".into(),
                source_start_s: 20.0,
                source_end_s: 200.0,
                confidence: Some(0.91),
                status: EpisodeSpanStatus::Accepted,
                evidence: vec![],
                extra: Default::default(),
            },
            EpisodeSpan {
                id: "rejected".into(),
                name: Some("Rejected".into()),
                order: Some(3),
                asset_id: "asset-a".into(),
                source_start_s: 320.0,
                source_end_s: 360.0,
                confidence: Some(0.3),
                status: EpisodeSpanStatus::Rejected,
                evidence: vec![],
                extra: Default::default(),
            },
        ];
        project.write(tmp.path()).unwrap();

        let summary = summarize_project_episodes(tmp.path()).unwrap();

        assert_eq!(summary.total, 3);
        assert_eq!(summary.accepted, 1);
        assert_eq!(summary.review_needed, 1);
        assert_eq!(summary.rejected, 1);
        assert_eq!(summary.episodes[0].id, "accepted");
        assert_eq!(summary.episodes[0].duration_s, 180.0);
        assert_eq!(summary.episodes[1].evidence_count, 1);
    }
}
