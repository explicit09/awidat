//! `relink_media` tool - apply safe timeline media relinks.

use std::path::{Component, Path};

use async_trait::async_trait;
use awidat_proto::otio::{Clip, MediaReference, Stack, StackChild, Track, TrackChild};
use awidat_proto::project::Project;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Apply a relink candidate to timeline media references.
pub struct RelinkMediaTool;

#[derive(Debug, Deserialize)]
struct RelinkMediaArgs {
    /// Existing `ExternalReference.target_url` to replace.
    #[serde(default)]
    old_target_url: Option<String>,
    /// Optional clip id/name to relink. Matches awidat clip_uuid when present, otherwise clip name.
    #[serde(default)]
    clip_id: Option<String>,
    /// New project-relative media target under the project root.
    new_target_url: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct RelinkMediaReport {
    status: &'static str,
    old_target_url: Option<String>,
    clip_id: Option<String>,
    new_target_url: String,
    changed_count: usize,
    affected_clips: Vec<String>,
}

#[async_trait]
impl ToolHandler for RelinkMediaTool {
    fn name(&self) -> &'static str {
        "relink_media"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "relink_media".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["new_target_url"],
                "properties": {
                    "old_target_url": {
                        "type": "string",
                        "description": "Existing ExternalReference target_url to replace."
                    },
                    "clip_id": {
                        "type": "string",
                        "description": "Optional clip id/name to relink. Matches awidat clip_uuid when present, otherwise clip name."
                    },
                    "new_target_url": {
                        "type": "string",
                        "description": "Project-relative replacement path. Must exist under the project root."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        vec![ApprovalKey::new(
            self.name(),
            format!(
                "{}->{}",
                invocation.args["old_target_url"].as_str().unwrap_or("*"),
                invocation.args["new_target_url"].as_str().unwrap_or("")
            ),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: RelinkMediaArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "relink_media: invalid args ({e}). Expected new_target_url plus old_target_url and/or clip_id."
            ))
        })?;
        if args.old_target_url.is_none() && args.clip_id.is_none() {
            return Err(FunctionCallError::RespondToModel(
                "relink_media: provide old_target_url and/or clip_id so the relink target is scoped"
                    .into(),
            ));
        }
        validate_project_relative_media_path(&args.new_target_url).map_err(|message| {
            FunctionCallError::RespondToModel(format!("relink_media: {message}"))
        })?;
        if !ctx.project_root.join(&args.new_target_url).is_file() {
            return Err(FunctionCallError::RespondToModel(format!(
                "relink_media: replacement does not exist under project root: {}",
                args.new_target_url
            )));
        }

        let mut project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!("relink_media: unable to read project: {e}"))
        })?;
        let mut affected_clips = Vec::new();
        relink_stack(
            &mut project.timeline.tracks,
            args.old_target_url.as_deref(),
            args.clip_id.as_deref(),
            &args.new_target_url,
            &mut affected_clips,
        );
        if affected_clips.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "relink_media: no matching timeline media references found".into(),
            ));
        }
        project.write(&ctx.project_root).map_err(|e| {
            FunctionCallError::Fatal(format!("relink_media: unable to write project: {e}"))
        })?;

        Ok(ToolOutput::text(
            serde_json::to_string(&RelinkMediaReport {
                status: "relinked",
                old_target_url: args.old_target_url,
                clip_id: args.clip_id,
                new_target_url: args.new_target_url,
                changed_count: affected_clips.len(),
                affected_clips,
            })
            .map_err(|e| {
                FunctionCallError::Fatal(format!("relink_media: serialization failed: {e}"))
            })?,
        ))
    }
}

fn relink_stack(
    stack: &mut Stack,
    old_target_url: Option<&str>,
    clip_id: Option<&str>,
    new_target_url: &str,
    affected_clips: &mut Vec<String>,
) {
    for child in &mut stack.children {
        match child {
            StackChild::Track(track) => {
                relink_track(
                    track,
                    old_target_url,
                    clip_id,
                    new_target_url,
                    affected_clips,
                );
            }
            StackChild::Stack(stack) => {
                relink_stack(
                    stack,
                    old_target_url,
                    clip_id,
                    new_target_url,
                    affected_clips,
                );
            }
            StackChild::Clip(clip) => {
                relink_clip(
                    clip,
                    old_target_url,
                    clip_id,
                    new_target_url,
                    affected_clips,
                );
            }
            StackChild::Gap(_) => {}
        }
    }
}

fn relink_track(
    track: &mut Track,
    old_target_url: Option<&str>,
    clip_id: Option<&str>,
    new_target_url: &str,
    affected_clips: &mut Vec<String>,
) {
    for child in &mut track.children {
        match child {
            TrackChild::Clip(clip) => {
                relink_clip(
                    clip,
                    old_target_url,
                    clip_id,
                    new_target_url,
                    affected_clips,
                );
            }
            TrackChild::Stack(stack) => {
                relink_stack(
                    stack,
                    old_target_url,
                    clip_id,
                    new_target_url,
                    affected_clips,
                );
            }
            TrackChild::Gap(_) | TrackChild::Transition(_) => {}
        }
    }
}

fn relink_clip(
    clip: &mut Clip,
    old_target_url: Option<&str>,
    clip_id: Option<&str>,
    new_target_url: &str,
    affected_clips: &mut Vec<String>,
) {
    let current_clip_id = clip_identifier(clip);
    if clip_id.is_some_and(|wanted| wanted != current_clip_id) {
        return;
    }
    let MediaReference::External(reference) = &mut clip.media_reference else {
        return;
    };
    if old_target_url.is_some_and(|wanted| wanted != reference.target_url) {
        return;
    }
    reference.target_url = new_target_url.to_string();
    affected_clips.push(current_clip_id);
}

fn clip_identifier(clip: &Clip) -> String {
    clip.metadata
        .awidat
        .as_ref()
        .and_then(|metadata| metadata.extra.get("clip_uuid"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(clip.name.as_str())
        .to_string()
}

fn validate_project_relative_media_path(target_url: &str) -> Result<(), &'static str> {
    if target_url.trim().is_empty() {
        return Err("new_target_url must not be empty");
    }
    if target_url.contains("://") {
        return Err("new_target_url must be project-relative, not a URL");
    }
    let path = Path::new(target_url);
    if path.is_absolute() {
        return Err("new_target_url must be project-relative, not absolute");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("new_target_url must not contain '..'");
    }
    Ok(())
}

const DESCRIPTION: &str = "\
Apply a safe relink candidate to timeline media references. Use \
`diagnose_project_media` first to find missing/unsafe references and \
candidate project-relative replacements. Provide old_target_url and/or \
clip_id plus a new_target_url that already exists under the project root.\
";

#[cfg(test)]
mod tests {
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, Stack, StackChild, Track, TrackChild, TrackKind,
    };
    use awidat_proto::project::Project;
    use tokio::sync::broadcast;

    use super::*;

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

    fn invocation(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "relink_media".into(),
            args,
        }
    }

    fn clip(name: &str, target_url: &str) -> Clip {
        let mut clip = Clip::empty(name);
        clip.media_reference = MediaReference::External(ExternalReference::new(target_url));
        clip
    }

    fn external_target(clip: &Clip) -> &str {
        let MediaReference::External(reference) = &clip.media_reference else {
            panic!("expected external reference");
        };
        &reference.target_url
    }

    #[test]
    fn rejects_unsafe_relink_targets() {
        assert!(validate_project_relative_media_path("../outside.mp4").is_err());
        assert!(validate_project_relative_media_path("/tmp/outside.mp4").is_err());
        assert!(validate_project_relative_media_path("https://example.com/a.mp4").is_err());
        assert!(validate_project_relative_media_path("raw/a.mp4").is_ok());
    }

    #[tokio::test]
    async fn relink_media_replaces_all_matching_targets_and_preserves_others() {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("raw")).unwrap();
        std::fs::write(dir.path().join("raw/new.mov"), b"media").unwrap();
        let mut track = Track::empty("V1", TrackKind::Video);
        track
            .children
            .push(TrackChild::Clip(clip("a", "raw/missing.mov")));
        track
            .children
            .push(TrackChild::Clip(clip("b", "raw/other.mov")));
        let mut nested = Stack::empty("nested");
        nested
            .children
            .push(StackChild::Clip(clip("c", "raw/missing.mov")));
        track.children.push(TrackChild::Stack(nested));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(track));
        project.write(dir.path()).unwrap();

        let output = RelinkMediaTool
            .handle(
                invocation(serde_json::json!({
                    "old_target_url": "raw/missing.mov",
                    "new_target_url": "raw/new.mov"
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let report: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(report["changed_count"], 2);
        assert_eq!(
            report["affected_clips"],
            serde_json::json!(["a".to_string(), "c".to_string()])
        );

        let project = Project::read(dir.path()).unwrap();
        let StackChild::Track(track) = &project.timeline.tracks.children[0] else {
            panic!("expected track");
        };
        let TrackChild::Clip(a) = &track.children[0] else {
            panic!("expected clip");
        };
        let TrackChild::Clip(b) = &track.children[1] else {
            panic!("expected clip");
        };
        let TrackChild::Stack(nested) = &track.children[2] else {
            panic!("expected stack");
        };
        let StackChild::Clip(c) = &nested.children[0] else {
            panic!("expected nested clip");
        };
        assert_eq!(external_target(a), "raw/new.mov");
        assert_eq!(external_target(b), "raw/other.mov");
        assert_eq!(external_target(c), "raw/new.mov");
    }

    #[tokio::test]
    async fn relink_media_rejects_missing_replacement_file() {
        let dir = tempfile::tempdir().unwrap();
        Project::init(dir.path()).unwrap();

        let err = RelinkMediaTool
            .handle(
                invocation(serde_json::json!({
                    "old_target_url": "raw/missing.mov",
                    "new_target_url": "raw/new.mov"
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();

        match err {
            FunctionCallError::RespondToModel(message) => {
                assert!(
                    message.contains("replacement does not exist"),
                    "got: {message}"
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
