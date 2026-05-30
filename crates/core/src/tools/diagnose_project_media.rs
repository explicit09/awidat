//! `diagnose_project_media` tool - surface repair diagnostics for timeline media.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use awidat_index::media_files::{MediaScanOptions, collect_project_media_files};
use awidat_proto::project::Project;
use awidat_proto::validate::{ValidationWarning, validate_project};
use serde::Deserialize;
use serde::Serialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

const DEFAULT_MAX_CANDIDATES: usize = 5;
const MAX_CANDIDATES: usize = 25;
const MAX_SCAN_FILES: usize = 2_000;

/// The `diagnose_project_media` tool.
pub struct DiagnoseProjectMediaTool;

#[derive(Debug, Deserialize)]
struct DiagnoseProjectMediaArgs {
    /// Maximum relink candidates returned per missing media diagnostic.
    #[serde(default)]
    max_candidates: Option<usize>,
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

#[async_trait]
impl ToolHandler for DiagnoseProjectMediaTool {
    fn name(&self) -> &'static str {
        "diagnose_project_media"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "diagnose_project_media".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "max_candidates": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": MAX_CANDIDATES,
                        "description": "Maximum same-name/same-stem relink candidates per missing media item. Default 5."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: DiagnoseProjectMediaArgs =
            serde_json::from_value(invocation.args).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "diagnose_project_media: invalid args ({e}). Expected an optional max_candidates integer."
                ))
            })?;
        let max_candidates = args
            .max_candidates
            .unwrap_or(DEFAULT_MAX_CANDIDATES)
            .min(MAX_CANDIDATES);
        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "diagnose_project_media: unable to read project: {e}"
            ))
        })?;
        let validation = validate_project(&project).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "diagnose_project_media: unable to validate project: {e}"
            ))
        })?;
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
        let body = serde_json::to_string(&report).map_err(|e| {
            FunctionCallError::Fatal(format!("diagnose_project_media serialization failed: {e}"))
        })?;
        Ok(ToolOutput::text(body))
    }
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

const DESCRIPTION: &str = "\
Inspect the current project for missing or unsafe timeline media references. \
Returns structured repair diagnostics, including timeline paths, missing target \
paths, safe relink candidates found in the project, and non-mutating repair \
actions an agent can propose before rendering.";

#[cfg(test)]
mod tests {
    use tokio::sync::broadcast;

    use super::*;
    use crate::FunctionCallError;
    use crate::tool::{ToolContext, ToolHandler, ToolInvocation};

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invoke() -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "diagnose_project_media".into(),
            args: serde_json::json!({}),
        }
    }

    fn project_with_media_reference(
        root: &std::path::Path,
        target_url: &str,
    ) -> awidat_proto::project::Project {
        use awidat_proto::otio::{
            Clip, ExternalReference, MediaReference, StackChild, Track, TrackChild, TrackKind,
        };

        let mut project = awidat_proto::project::Project::init(root).unwrap();
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip");
        clip.media_reference = MediaReference::External(ExternalReference::new(target_url));
        track.children.push(TrackChild::Clip(clip));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(track));
        project.write(root).unwrap();
        project
    }

    #[tokio::test]
    async fn missing_media_reports_relink_candidate() {
        let dir = tempfile::tempdir().unwrap();
        project_with_media_reference(dir.path(), "raw/missing.mp4");
        let raw_dir = dir.path().join("raw").join("archive");
        std::fs::create_dir_all(&raw_dir).unwrap();
        std::fs::write(raw_dir.join("missing.mp4"), b"candidate").unwrap();

        let out = DiagnoseProjectMediaTool
            .handle(invoke(), ctx_at(dir.path()))
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();

        assert_eq!(body["status"], "needs_repair");
        assert_eq!(body["missing_count"], 1);
        assert_eq!(
            body["diagnostics"][0]["kind"],
            serde_json::json!("missing_media")
        );
        assert_eq!(body["diagnostics"][0]["target_url"], "raw/missing.mp4");
        assert_eq!(
            body["diagnostics"][0]["candidates"][0]["project_relative_path"],
            "raw/archive/missing.mp4"
        );
    }

    #[tokio::test]
    async fn unsafe_media_reference_reports_blocking_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        project_with_media_reference(dir.path(), "../outside.mp4");

        let out = DiagnoseProjectMediaTool
            .handle(invoke(), ctx_at(dir.path()))
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&out.content).unwrap();

        assert_eq!(body["status"], "needs_repair");
        assert_eq!(body["unsafe_count"], 1);
        assert_eq!(
            body["diagnostics"][0]["kind"],
            serde_json::json!("unsafe_media_reference")
        );
        assert_eq!(body["diagnostics"][0]["target_url"], "../outside.mp4");
    }

    #[tokio::test]
    async fn unreadable_project_is_respond_to_model() {
        let dir = tempfile::tempdir().unwrap();
        let err = DiagnoseProjectMediaTool
            .handle(invoke(), ctx_at(dir.path()))
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            FunctionCallError::RespondToModel(message)
                if message.contains("diagnose_project_media: unable to read project")
        ));
    }

    #[test]
    fn tool_is_read_only() {
        assert!(!DiagnoseProjectMediaTool.is_mutating(&invoke()));
    }
}
