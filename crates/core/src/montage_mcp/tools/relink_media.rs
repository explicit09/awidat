//! `relink_media` — apply a safe relink to timeline media references.
//! Ported from `crates/core/src/tools/relink_media.rs` to the in-process
//! MCP server. Mutating: rewrites `project.otio.json`.

use std::path::{Component, Path};

use montage_proto::otio::{Clip, MediaReference, Stack, StackChild, Track, TrackChild};
use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `relink_media`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct RelinkMediaArgs {
    /// Existing `ExternalReference.target_url` to replace.
    #[serde(default)]
    pub old_target_url: Option<String>,
    /// Optional clip id/name to relink. Matches montage clip_uuid when
    /// present, otherwise clip name.
    #[serde(default)]
    pub clip_id: Option<String>,
    /// New project-relative media target under the project root.
    pub new_target_url: String,
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

pub fn run(args: RelinkMediaArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.old_target_url.is_none() && args.clip_id.is_none() {
        return Err(
            "relink_media: provide old_target_url and/or clip_id so the relink target is scoped"
                .into(),
        );
    }
    validate_project_relative_media_path(&args.new_target_url)
        .map_err(|message| format!("relink_media: {message}"))?;
    if !ctx.project_root.join(&args.new_target_url).is_file() {
        return Err(format!(
            "relink_media: replacement does not exist under project root: {}",
            args.new_target_url
        ));
    }

    let _mutation = crate::vc::lock_timeline_mutation(&ctx.project_root)
        .map_err(|e| format!("relink_media: lock timeline mutation: {e}"))?;
    let mut project = Project::read(&ctx.project_root)
        .map_err(|e| format!("relink_media: unable to read project: {e}"))?;
    let mut affected_clips = Vec::new();
    relink_stack(
        &mut project.timeline.tracks,
        args.old_target_url.as_deref(),
        args.clip_id.as_deref(),
        &args.new_target_url,
        &mut affected_clips,
    );
    if affected_clips.is_empty() {
        return Err("relink_media: no matching timeline media references found".into());
    }
    project
        .write(&ctx.project_root)
        .map_err(|e| format!("relink_media: unable to write project: {e}"))?;

    serde_json::to_string(&RelinkMediaReport {
        status: "relinked",
        old_target_url: args.old_target_url,
        clip_id: args.clip_id,
        new_target_url: args.new_target_url,
        changed_count: affected_clips.len(),
        affected_clips,
    })
    .map_err(|e| format!("relink_media: serialization failed: {e}"))
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
        .montage
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

pub const DESCRIPTION: &str = "\
Apply a safe relink candidate to timeline media references. Use \
`diagnose_project_media` first to find missing/unsafe references and \
candidate project-relative replacements. Provide old_target_url and/or \
clip_id plus a new_target_url that already exists under the project root.";
