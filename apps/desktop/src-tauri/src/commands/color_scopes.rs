//! Color-scopes IPC for the desktop UI.
//!
//! Thin wrapper around
//! [`awidat_core::tools::color_scopes::extract_and_compute_color_scopes`]
//! — the same pipeline the `color_scopes` agent tool calls — so the
//! human-facing scope dock and the agent's evidence reads stay in
//! lockstep.
//!
//! Path validation rule: the requested asset must live under the
//! currently-loaded project root. Proxies (which the media pane
//! plays) all live under `<project>/.awidat/proxies/`, so passing a
//! proxy path works without any special casing on the frontend.

use std::path::PathBuf;

use awidat_core::tools::color_scopes::{
    self, ColorScopeSnapshot, DEFAULT_SCOPE_BINS, DEFAULT_SCOPE_MAX_WIDTH,
};
use serde::Deserialize;
use tauri::State;

use crate::state::AwidatState;

/// Arguments for [`get_color_scopes`].
#[derive(Debug, Deserialize)]
pub struct GetColorScopesArgs {
    /// Absolute path or project-relative path to the video asset
    /// (typically a proxy mp4 path from `list_proxies`).
    pub path: String,
    /// Time in seconds to sample.
    pub t_s: f64,
    /// Scope resolution per axis. Defaults to
    /// [`DEFAULT_SCOPE_BINS`] when absent.
    #[serde(default)]
    pub bins: Option<usize>,
    /// Extracted frame width cap. Defaults to
    /// [`DEFAULT_SCOPE_MAX_WIDTH`] when absent.
    #[serde(default)]
    pub max_width: Option<usize>,
}

/// Compute waveform / vectorscope / RGB parade / luma & RGB histograms
/// for one frame of an asset already loaded into the project.
///
/// Returns the scope snapshot on success; on failure, returns a
/// human-readable error string the frontend logs and displays as
/// "scopes unavailable" without crashing the dock.
#[tauri::command]
pub async fn get_color_scopes(
    state: State<'_, AwidatState>,
    args: GetColorScopesArgs,
) -> Result<ColorScopeSnapshot, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    let candidate = PathBuf::from(&args.path);
    let abs = if candidate.is_absolute() {
        candidate
    } else {
        project_root.join(&candidate)
    };
    if !abs.is_file() {
        return Err(format!("asset not found on disk: {}", abs.display()));
    }

    // Defence in depth: refuse paths that resolve outside the project
    // root. Mirrors the `resolve_asset_path` guard in the core tool.
    let canonical_root = std::fs::canonicalize(&project_root)
        .map_err(|e| format!("canonicalize project root: {e}"))?;
    let canonical_asset =
        std::fs::canonicalize(&abs).map_err(|e| format!("canonicalize asset: {e}"))?;
    if !canonical_asset.starts_with(&canonical_root) {
        return Err(format!(
            "asset path '{}' must live under the project root",
            args.path
        ));
    }

    let bins = args.bins.unwrap_or(DEFAULT_SCOPE_BINS);
    let max_width = args.max_width.unwrap_or(DEFAULT_SCOPE_MAX_WIDTH);

    color_scopes::extract_and_compute_color_scopes(&canonical_asset, args.t_s, bins, max_width)
        .await
        .map_err(|e| format!("color_scopes: {e}"))
}
