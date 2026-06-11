//! Read and parse a project LUT for the preview's WebGL grade pass.
//!
//! The frontend cannot parse `.cube` itself without duplicating the
//! validation rules in `montage-lut` — and preview/export fidelity
//! depends on both sides agreeing on the table. This command parses
//! with the exact crate the render engine uses and ships the raw
//! table to the webview, which uploads it as a 3D texture.

use montage_lut::{Lut, parse_cube};
use serde::Serialize;
use tauri::State;

use crate::state::MontageState;

/// Parsed 3D LUT payload for the preview shader. The table is the
/// crate's canonical layout: R-fastest, G-middle, B-slowest —
/// uploaded as a (size × size × size) texture with x=R, y=G, z=B.
#[derive(Debug, Clone, Serialize)]
pub struct PreviewLut {
    pub size: usize,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub table: Vec<f32>,
}

/// Enumerate project-relative `.cube` LUTs for the Inspector's
/// dropdown. Shallow recursive walk from the project root (depth 4),
/// skipping hidden/derived directories. Paths come back sorted and
/// ready to feed `apply_lut` / `read_preview_lut` verbatim.
#[tauri::command]
pub async fn list_preview_luts(state: State<'_, MontageState>) -> Result<Vec<String>, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    tokio::task::spawn_blocking(move || {
        let mut found = Vec::new();
        let mut stack = vec![(project_root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') || name == "node_modules" {
                    continue;
                }
                if path.is_dir() {
                    if depth < 4 {
                        stack.push((path, depth + 1));
                    }
                } else if name.to_ascii_lowercase().ends_with(".cube")
                    && let Ok(rel) = path.strip_prefix(&project_root)
                {
                    found.push(rel.to_string_lossy().into_owned());
                }
            }
        }
        found.sort();
        Ok(found)
    })
    .await
    .map_err(|e| format!("lut scan join: {e}"))?
}

/// Resolve and parse a project-relative `.cube` LUT. Path rules
/// mirror the Inspector's input validation: relative, no traversal,
/// `.cube` only (the same scope the EDL `Apply LUT` op accepts).
#[tauri::command]
pub async fn read_preview_lut(
    state: State<'_, MontageState>,
    lut_path: String,
) -> Result<PreviewLut, String> {
    let trimmed = lut_path.trim();
    if trimmed.is_empty() {
        return Err("lut_path must not be empty".into());
    }
    if trimmed.starts_with('/') || trimmed.contains('\\') {
        return Err("lut_path must be project-relative".into());
    }
    if trimmed
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("lut_path must not contain traversal segments".into());
    }
    if !trimmed.to_ascii_lowercase().ends_with(".cube") {
        return Err("preview LUTs support .cube only".into());
    }

    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let path = project_root.join(trimmed);

    let src = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("read LUT {trimmed}: {e}"))?;
    let parsed = tokio::task::spawn_blocking(move || parse_cube(&src).map_err(|e| e.to_string()))
        .await
        .map_err(|e| format!("LUT parse join: {e}"))??;

    match parsed {
        Lut::Three(lut) => Ok(PreviewLut {
            size: lut.size,
            domain_min: lut.domain_min,
            domain_max: lut.domain_max,
            table: lut.table,
        }),
        Lut::One(_) => Err("1D LUTs are not supported in preview yet".into()),
    }
}
