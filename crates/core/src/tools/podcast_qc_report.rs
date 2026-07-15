//! `podcast_qc_report` tool — pre-render timeline QC gate.

use async_trait::async_trait;
use montage_proto::project::Project;

use crate::FunctionCallError;
use crate::podcast_analysis::build_podcast_qc_report;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Build a podcast timeline QC report before render.
pub struct PodcastQcReportTool;

#[async_trait]
impl ToolHandler for PodcastQcReportTool {
    fn name(&self) -> &'static str {
        "podcast_qc_report"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().into(),
            description: "Run pre-render podcast QC for gaps, missing media, captions, audio readiness, and suspicious timeline structure.".into(),
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
                "podcast_qc_report: failed to read project: {e}"
            ))
        })?;
        let body = build_podcast_qc_report(&ctx.project_root, &project);
        serde_json::to_string(&body)
            .map(ToolOutput::text)
            .map_err(|e| FunctionCallError::Fatal(format!("podcast_qc_report serialize: {e}")))
    }
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
            job_manager: montage_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(montage_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn write_project(root: &std::path::Path) {
        use montage_proto::otio::{
            Clip, ExternalReference, Gap, MediaReference, RationalTime, Stack, StackChild,
            TimeRange, Timeline, Track, TrackChild, TrackKind,
        };
        let mut project = Project::init(root).unwrap();
        let mut clip = Clip::empty("clip-1");
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/missing.mov"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));
        let mut gap = Gap::of_duration(2.0, 24.0);
        gap.name = "gap".into();
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        track.children.push(TrackChild::Gap(gap));
        let mut timeline = Timeline::empty("podcast");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        timeline.tracks = stack;
        project.timeline = timeline;
        project.write(root).unwrap();
    }

    #[tokio::test]
    async fn blocks_missing_media_and_reports_gaps() {
        let dir = tempfile::tempdir().unwrap();
        write_project(dir.path());
        let out = PodcastQcReportTool
            .handle(
                ToolInvocation {
                    call_id: "q1".into(),
                    name: "podcast_qc_report".into(),
                    args: serde_json::json!({}),
                },
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&out.content).unwrap();
        assert_eq!(value["status"], "blocked");
        assert!(
            value["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["kind"] == "missing_media")
        );
        assert!(
            value["issues"]
                .as_array()
                .unwrap()
                .iter()
                .any(|issue| issue["kind"] == "timeline_gap")
        );
    }
}
