//! `view_episode` tool — re-fetch the compact episode-map.
//!
//! The same map is injected into the system prompt at session start
//! (see `Session::new`), so the agent already has it. This tool
//! exists for two cases:
//!
//! 1. After a compaction pass clears the system prompt's tier-1
//!    cache and the agent wants to re-orient.
//! 2. After a sequence of edits the timeline state has shifted; the
//!    map at session start no longer reflects "Editorial state:
//!    untrimmed, single clip" — the agent calls this to refresh.
//!
//! Read-only, no side effects, cheap.

use async_trait::async_trait;
use awidat_proto::project::Project;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `view_episode` tool.
pub struct ViewEpisodeTool;

#[async_trait]
impl ToolHandler for ViewEpisodeTool {
    fn name(&self) -> &'static str {
        "view_episode"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "view_episode".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
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
                "view_episode: failed to read project at {}: {e}",
                ctx.project_root.display()
            ))
        })?;
        let map = crate::episode_map::render(&project, &ctx.project_root);
        Ok(ToolOutput::text(map))
    }
}

const DESCRIPTION: &str = "\
Return a compact textual map of the episode: title, speaker count, \
topic list with timestamps, and the current editorial state (clip \
count, total duration, trimmed/untrimmed). The same map was injected \
into your system prompt at session start; call this tool to refresh \
it after edits or after a context compaction. No arguments. Cheap \
and read-only.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
        Track, TrackChild, TrackKind,
    };
    use tokio::sync::broadcast;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo { name: "test".into(), version: "0.0.0".into() }),
        }
    }

    fn invoke() -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "view_episode".into(),
            args: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn returns_minimal_map_on_fresh_project() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-0".to_string());
        clip.media_reference =
            MediaReference::External(ExternalReference::new("raw/x.mp4"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(10.0 * 24.0, 24.0),
        ));
        clip.metadata = ClipMetadata::default();
        track.children.push(TrackChild::Clip(clip));
        project.timeline.tracks.children.push(StackChild::Track(track));
        project.write(dir.path()).unwrap();

        let out = ViewEpisodeTool
            .handle(invoke(), ctx_at(dir.path()))
            .await
            .unwrap();
        assert!(out.content.contains("Episode:"));
        assert!(out.content.contains("1 clip(s)"));
        assert!(out.content.contains("10.00s total"));
    }
}
