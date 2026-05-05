//! Media-pane Tauri commands. Currently just listing proxies; the
//! preview itself is served via Tauri's built-in `asset:` protocol
//! whose scope was opened to the project's proxies dir by
//! `set_project_root` / `init_project`.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::State;

use crate::state::AwidatState;

/// One playable proxy. The frontend feeds `proxy_path` into
/// `convertFileSrc()` to get a `<video>`-compatible URL.
#[derive(Debug, Clone, Serialize)]
pub struct ProxyEntry {
    /// Stem of the asset (no extension). Stable per asset; the
    /// frontend uses this as a label and as a stable React key.
    pub stem: String,
    /// Absolute path to the proxy mp4 on disk. Pass directly to
    /// `convertFileSrc()` on the frontend.
    pub proxy_path: String,
    /// File size in bytes. Useful for "is this proxy actually
    /// finished generating yet?" checks (zero or partial = still
    /// being written).
    pub size_bytes: u64,
}

/// Return every proxy currently sitting in
/// `<project>/.awidat/proxies/`. Empty list when no project is
/// loaded or the dir doesn't exist yet (first import will create
/// it). Sorted by stem so order is stable across calls.
#[tauri::command]
pub async fn list_proxies(state: State<'_, AwidatState>) -> Result<Vec<ProxyEntry>, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(Vec::new()),
    };
    let dir = project_root.join(".awidat").join("proxies");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut entries = tokio::fs::read_dir(&dir)
        .await
        .map_err(|e| format!("read proxies dir: {e}"))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| format!("read proxies entry: {e}"))?
    {
        let path = entry.path();
        if !is_proxy_file(&path) {
            continue;
        }
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if stem.is_empty() {
            continue;
        }
        out.push(ProxyEntry {
            stem,
            proxy_path: path.to_string_lossy().into_owned(),
            size_bytes: meta.len(),
        });
    }
    out.sort_by(|a, b| a.stem.cmp(&b.stem));
    Ok(out)
}

fn is_proxy_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("mp4"))
            .unwrap_or(false)
}

/// Tauri command that resolves the absolute path on disk for a
/// given asset stem, IF the proxy exists. Returns `None` otherwise.
/// Used by the frontend to ask "is the proxy for this asset ready?"
/// without listing every proxy.
#[tauri::command]
pub async fn proxy_path_for_stem(
    state: State<'_, AwidatState>,
    stem: String,
) -> Result<Option<String>, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(None),
    };
    let candidate: PathBuf = project_root
        .join(".awidat")
        .join("proxies")
        .join(format!("{stem}.mp4"));
    Ok(if candidate.is_file() {
        Some(candidate.to_string_lossy().into_owned())
    } else {
        None
    })
}
