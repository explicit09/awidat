//! `podcast_visual_polish` tool — forced visual/multicam planning pass.

use async_trait::async_trait;
use awidat_index::walk_indexer;
use awidat_proto::project::Project;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Build a podcast visual polish report from available indexes.
pub struct PodcastVisualPolishTool;

#[async_trait]
impl ToolHandler for PodcastVisualPolishTool {
    fn name(&self) -> &'static str {
        "podcast_visual_polish"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: "Check podcast visual polish readiness: multicam evidence, b-roll planning, chapters, lower thirds, and captions.".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        _invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "podcast_visual_polish: failed to read project: {e}"
            ))
        })?;
        let face_assets = indexed_assets(&ctx.project_root, "face");
        let shot_assets = indexed_assets(&ctx.project_root, "shot");
        let whisper_assets = indexed_assets(&ctx.project_root, "whisper");
        let topic_assets = indexed_assets(&ctx.project_root, "topic");
        let caption_summary = crate::captions::summarize_captions(&project);
        let has_broadcast_overlay = project
            .timeline
            .metadata
            .awidat
            .as_ref()
            .and_then(|meta| meta.broadcast_overlay.as_ref())
            .is_some();

        let mut issues = Vec::new();
        let mut recommendations = Vec::new();
        if whisper_assets.is_empty() {
            issues.push(serde_json::json!({
                "kind": "missing_transcript",
                "severity": "error",
                "message": "Visual polish needs transcript/speaker evidence for angle and chapter planning."
            }));
        }
        if face_assets.is_empty() || shot_assets.is_empty() {
            issues.push(serde_json::json!({
                "kind": "missing_multicam_evidence",
                "severity": "warning",
                "face_asset_count": face_assets.len(),
                "shot_asset_count": shot_assets.len(),
                "message": "Run face and shot indexing before trusting speaker angle/reaction planning."
            }));
        }
        if topic_assets.is_empty() {
            issues.push(serde_json::json!({
                "kind": "missing_chapter_evidence",
                "severity": "warning",
                "message": "No topic index found; chapter/title-card planning should use transcript/story-map evidence."
            }));
        }
        if !has_broadcast_overlay {
            recommendations.push("Plan lower thirds and chapter/title cards before final render.");
        }
        if caption_summary.caption_overlay_count > 0
            && caption_summary.missing_safe_area_caption_overlay_count > 0
        {
            issues.push(serde_json::json!({
                "kind": "caption_safe_area_missing",
                "severity": "warning",
                "missing_count": caption_summary.missing_safe_area_caption_overlay_count,
                "message": "Caption overlays are missing safe-area metadata."
            }));
        }
        recommendations.extend([
            "Run plan_multicam or produce an angle plan with minimum hold duration; avoid switching on short backchannels.",
            "Use broll_candidates/find_broll_opportunities for jump-cut cover and visual examples.",
            "Plan reaction shots around emotional peaks, jokes, and strong claims.",
        ]);
        let status = if issues.iter().any(|issue| issue["severity"] == "error") {
            "needs_fix"
        } else if issues.is_empty() {
            "ready"
        } else {
            "needs_review"
        };
        let body = serde_json::json!({
            "status": status,
            "summary_for_agent": format!("Visual polish status: {status}. {} issue(s), {} recommendation(s).", issues.len(), recommendations.len()),
            "evidence": {
                "whisper_asset_count": whisper_assets.len(),
                "face_asset_count": face_assets.len(),
                "shot_asset_count": shot_assets.len(),
                "topic_asset_count": topic_assets.len(),
                "caption_summary": caption_summary,
                "has_broadcast_overlay": has_broadcast_overlay
            },
            "issues": issues,
            "recommendations": recommendations,
            "required_before_render": true,
        });
        serde_json::to_string(&body)
            .map(ToolOutput::text)
            .map_err(|e| FunctionCallError::Fatal(format!("podcast_visual_polish serialize: {e}")))
    }
}

fn indexed_assets(project_root: &std::path::Path, indexer: &str) -> Vec<String> {
    walk_indexer(project_root, indexer)
        .map(|iter| iter.map(|(asset, _)| asset).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

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

    #[tokio::test]
    async fn reports_missing_multicam_evidence() {
        let dir = tempfile::tempdir().unwrap();
        Project::init(dir.path())
            .unwrap()
            .write(dir.path())
            .unwrap();
        let whisper_path = dir
            .path()
            .join("index")
            .join("whisper")
            .join("raw/episode.mov.json");
        std::fs::create_dir_all(whisper_path.parent().unwrap()).unwrap();
        std::fs::write(whisper_path, "{}").unwrap();
        let out = PodcastVisualPolishTool
            .handle(
                ToolInvocation {
                    call_id: "v1".into(),
                    name: "podcast_visual_polish".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(value["status"], "needs_review");
        assert!(
            value["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["kind"] == "missing_multicam_evidence")
        );
    }
}
