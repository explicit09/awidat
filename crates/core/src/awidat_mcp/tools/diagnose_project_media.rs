//! `diagnose_project_media` — surface repair diagnostics for timeline media.
//! Ported from `crates/core/src/tools/diagnose_project_media.rs` to the
//! in-process MCP server.

use std::path::{Path, PathBuf};

use awidat_index::media_files::{MediaScanOptions, collect_project_media_files};
use awidat_proto::project::Project;
use awidat_proto::validate::{ValidationWarning, validate_project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::awidat_mcp::context::McpToolCtx;

const DEFAULT_MAX_CANDIDATES: usize = 5;
const MAX_CANDIDATES: usize = 25;
const MAX_SCAN_FILES: usize = 2_000;

/// Arguments to `diagnose_project_media`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct DiagnoseProjectMediaArgs {
    /// Maximum relink candidates returned per missing media diagnostic.
    #[serde(default)]
    pub max_candidates: Option<usize>,
}

#[derive(Debug, Serialize)]
struct MediaDiagnosticsReport {
    status: &'static str,
    missing_count: usize,
    unsafe_count: usize,
    diagnostics: Vec<MediaDiagnostic>,
}

#[derive(Debug, Serialize)]
struct MediaDiagnostic {
    kind: &'static str,
    timeline_path: String,
    target_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_path: Option<String>,
    repair_actions: Vec<&'static str>,
    candidates: Vec<RelinkCandidate>,
}

#[derive(Debug, Serialize)]
struct RelinkCandidate {
    project_relative_path: String,
    reason: &'static str,
}

/// Run `diagnose_project_media` against the project resolved from
/// [`McpToolCtx`]. Returns the JSON body as `Ok(String)`; project-read
/// or validation errors return `Err(String)`.
pub fn run(args: DiagnoseProjectMediaArgs, ctx: McpToolCtx) -> Result<String, String> {
    let max_candidates = args
        .max_candidates
        .unwrap_or(DEFAULT_MAX_CANDIDATES)
        .min(MAX_CANDIDATES);
    let project = Project::read(&ctx.project_root)
        .map_err(|e| format!("diagnose_project_media: unable to read project: {e}"))?;
    let validation = validate_project(&project)
        .map_err(|e| format!("diagnose_project_media: unable to validate project: {e}"))?;
    let media_files = collect_media_files(&ctx.project_root);
    let mut missing_count = 0;
    let mut unsafe_count = 0;
    let mut diagnostics = Vec::new();

    for warning in validation.index_warnings {
        match warning {
            ValidationWarning::TimelineMediaMissing { path, target_url } => {
                missing_count += 1;
                let candidates = candidate_relinks(
                    &ctx.project_root,
                    &target_url,
                    &media_files,
                    max_candidates,
                );
                diagnostics.push(MediaDiagnostic {
                    kind: "missing_media",
                    timeline_path: path,
                    resolved_path: Some(
                        ctx.project_root.join(&target_url).display().to_string(),
                    ),
                    target_url,
                    repair_actions: vec![
                        "restore the missing file at target_url",
                        "relink the clip to one candidate path",
                        "remove or replace the affected timeline clip",
                    ],
                    candidates,
                });
            }
            ValidationWarning::UnsafeTimelineMediaReference { path, target_url } => {
                unsafe_count += 1;
                diagnostics.push(MediaDiagnostic {
                    kind: "unsafe_media_reference",
                    timeline_path: path,
                    target_url,
                    resolved_path: None,
                    repair_actions: vec![
                        "replace the reference with a safe project-relative media path",
                        "import the external media under the project before relinking",
                        "remove or replace the affected timeline clip",
                    ],
                    candidates: Vec::new(),
                });
            }
            _ => {}
        }
    }

    let report = MediaDiagnosticsReport {
        status: if diagnostics.is_empty() {
            "ok"
        } else {
            "needs_repair"
        },
        missing_count,
        unsafe_count,
        diagnostics,
    };
    serde_json::to_string(&report)
        .map_err(|e| format!("diagnose_project_media serialization failed: {e}"))
}

fn collect_media_files(project_root: &Path) -> Vec<PathBuf> {
    collect_project_media_files(
        project_root,
        MediaScanOptions {
            include_raw: true,
            include_renders: false,
            max_files: Some(MAX_SCAN_FILES),
        },
    )
    .map(|files| files.into_iter().map(|file| file.path).collect())
    .unwrap_or_default()
}

fn candidate_relinks(
    project_root: &Path,
    target_url: &str,
    media_files: &[PathBuf],
    max_candidates: usize,
) -> Vec<RelinkCandidate> {
    if max_candidates == 0 {
        return Vec::new();
    }
    let target = Path::new(target_url);
    let target_file_name = target.file_name();
    let target_stem = target.file_stem();
    let mut candidates = Vec::new();

    for media in media_files {
        let reason = if target_file_name.is_some() && media.file_name() == target_file_name {
            Some("same_file_name")
        } else if target_stem.is_some() && media.file_stem() == target_stem {
            Some("same_stem")
        } else {
            None
        };
        let Some(reason) = reason else {
            continue;
        };
        let Ok(relative) = media.strip_prefix(project_root) else {
            continue;
        };
        candidates.push(RelinkCandidate {
            project_relative_path: relative.to_string_lossy().replace('\\', "/"),
            reason,
        });
        if candidates.len() >= max_candidates {
            break;
        }
    }

    candidates
}

pub const DESCRIPTION: &str = "\
Inspect the current project for missing or unsafe timeline media references. \
Returns structured repair diagnostics, including timeline paths, missing target \
paths, safe relink candidates found in the project, and non-mutating repair \
actions an agent can propose before rendering.";
