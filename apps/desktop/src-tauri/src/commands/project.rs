//! Project lifecycle commands: opening an existing project, creating
//! a new one, listing recent ones. Import + index commands live in
//! `import.rs` / `index.rs` (next commits).

use std::path::{Path, PathBuf};

use awidat_proto::project::Project;
use tauri::{AppHandle, Manager, State};
use tokio::fs;

use crate::state::AwidatState;

/// Reconfigure Tauri's asset-protocol scope so the webview can fetch
/// proxy mp4s out of `<project>/.awidat/proxies/` via
/// `convertFileSrc()`. We allow only that one directory at a time —
/// the scope is otherwise empty (set in tauri.conf.json) so the
/// asset protocol cannot be abused to read arbitrary files. Called
/// from `set_project_root` and `init_project`.
pub(crate) fn allow_proxies_dir(app: &AppHandle, project_root: &Path) {
    let proxies = project_root.join(".awidat").join("proxies");
    // The dir may not exist yet (first import will create it). Allow
    // it preemptively — the scope check is a glob, not a stat.
    let scope = app.asset_protocol_scope();
    if let Err(e) = scope.allow_directory(&proxies, true) {
        tracing::warn!(
            error = %e,
            path = %proxies.display(),
            "failed to allow proxies dir in asset-protocol scope",
        );
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
    state: State<'_, AwidatState>,
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
            "not an awidat project (no project.otio.json under {path})"
        ));
    }

    *state.project_root.lock().await = Some(buf.clone());
    *state.session.lock().await = None;
    allow_proxies_dir(&app, &buf);

    // Best-effort: ignore failures so a corrupted recents file
    // doesn't block project opening.
    if let Err(e) = update_recents(&buf).await {
        tracing::warn!(error = %e, "failed to update recents file");
    }

    Ok(())
}

/// Read the currently-configured project root, if any.
#[tauri::command]
pub async fn current_project_root(state: State<'_, AwidatState>) -> Result<Option<String>, String> {
    Ok(state
        .project_root
        .lock()
        .await
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned()))
}

/// Initialize a new awidat project at `<parent_dir>/<name>` and load
/// it as the current project. Mirrors `awidat new --no-md=false
/// --no-index` (init + starter AWIDAT.md, no asset import). Asset
/// import is a separate step the frontend can chain after.
#[tauri::command]
pub async fn init_project(
    app: AppHandle,
    state: State<'_, AwidatState>,
    parent_dir: String,
    name: String,
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
    tokio::task::spawn_blocking(move || Project::init(&project_dir_for_init))
        .await
        .map_err(|e| format!("init join: {e}"))?
        .map_err(|e| format!("init: {e}"))?;

    // Starter AWIDAT.md so the project is "ready" with no extra
    // hand-holding from the user. The CLI's `--no-md` flag isn't
    // exposed here — desktop init always writes it. If the user
    // doesn't want it they delete the file.
    let md_path = project_dir.join("AWIDAT.md");
    fs::write(&md_path, AWIDAT_MD_TEMPLATE)
        .await
        .map_err(|e| format!("write AWIDAT.md: {e}"))?;

    *state.project_root.lock().await = Some(project_dir.clone());
    *state.session.lock().await = None;
    allow_proxies_dir(&app, &project_dir);
    if let Err(e) = update_recents(&project_dir).await {
        tracing::warn!(error = %e, "failed to update recents file");
    }

    Ok(project_dir.to_string_lossy().into_owned())
}

async fn ensure_project_switch_allowed(state: &State<'_, AwidatState>) -> Result<(), String> {
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

/// Cancel an in-flight long job (yt-dlp download, indexer run) by
/// its protocol-Item id. No-op if the id isn't currently running.
#[tauri::command]
pub async fn cancel_job(state: State<'_, AwidatState>, job_id: String) -> Result<(), String> {
    if let Some(handle) = state.jobs.lock().await.get(&job_id) {
        handle.cancel.cancel();
    }
    Ok(())
}

/// Path to the recents file.
///
/// macOS: `~/Library/Application Support/awidat-desktop/recents.json`
/// Linux: `~/.config/awidat-desktop/recents.json`
/// Windows: `%APPDATA%\awidat-desktop\recents.json`
///
/// Returns `None` if the OS doesn't expose a config dir (we silently
/// drop recents in that case rather than failing).
fn recents_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("awidat-desktop").join("recents.json"))
}

/// Starter AWIDAT.md identical to the CLI's. Kept inline rather than
/// shared with new_cmd.rs because the desktop binary mustn't depend
/// on the CLI crate (would be circular: CLI bundles desktop in
/// release builds, eventually).
const AWIDAT_MD_TEMPLATE: &str = "\
# Project conventions

This file is read by awidat at session start and added to the agent's \
system prompt. Use it to record editorial conventions, ground rules, \
and per-episode constraints. Edit freely; remove sections you don't \
need. Subdirectories may also have their own `AWIDAT.md` for narrower \
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
