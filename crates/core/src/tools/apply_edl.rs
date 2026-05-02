//! `apply_edl` tool — the load-bearing one.
//!
//! Per `PLAN.md` §6.2:
//!   1. Lark parse → structured EdlChange set.
//!   2. Anchor resolution.
//!   3. Schema validation (range, paths).
//!   4. OTIO round-trip — apply to a clone, validate.
//!   5. Hooks (deferred).
//!   6. Commit to disk; emit `TimelineDiff` event.
//!
//! Failures route as `RespondToModel` with actionable strings — anchor
//! misses include "did you mean?" candidates so the model can self-
//! correct in the same turn.
//!
//! The argument shape: `{ "edl": "<freeform-Lark text>" }`. JSON-
//! escaping the EDL is necessary at the wire level (Anthropic's tool-
//! use protocol takes JSON args) but the *content* is the freeform
//! envelope — the `Lark` discipline lives in the content, not the
//! wrapper.

use async_trait::async_trait;
use awidat_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;
use crate::edl::{ApplyError, EdlParseError, apply as edl_apply, parse as edl_parse};
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// The `apply_edl` tool.
pub struct ApplyEdlTool;

#[derive(Debug, Deserialize)]
struct ApplyEdlArgs {
    /// The freeform envelope text. See `crates/core/src/edl/parser.rs`
    /// module docs for the format.
    edl: String,
    /// If true, parse + validate but don't write the new timeline to
    /// disk. The applied-op log is still returned. Default: false.
    #[serde(default)]
    dry_run: bool,
}

#[async_trait]
impl ToolHandler for ApplyEdlTool {
    fn name(&self) -> &'static str {
        "apply_edl"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "apply_edl".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "edl": {
                        "type": "string",
                        "description": "Freeform EDL envelope. Begins with `*** Begin EDL` and ends with `*** End EDL`. Each op is a `*** <Op>` heading followed by `@@ anchor: ...` and `+ key: value` field lines."
                    },
                    "dry_run": {
                        "type": "boolean",
                        "description": "If true, validate without committing. Returns the same applied-op log; the timeline file isn't touched."
                    }
                },
                "required": ["edl"]
            }),
        }
    }

    fn is_mutating(&self, invocation: &ToolInvocation) -> bool {
        // dry_run=true is read-only, but the safe default is true; the
        // session's parallel-dispatch gate (week 5+) will use this.
        let dry_run = invocation
            .args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        !dry_run
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: ApplyEdlArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "apply_edl: invalid args ({e}). Required: {{ \"edl\": <envelope text> }}."
            ))
        })?;

        // 1. Parse.
        let envelope = edl_parse(&args.edl).map_err(|e| {
            FunctionCallError::RespondToModel(format_parse_error(&e))
        })?;

        if envelope.is_empty() {
            return Ok(ToolOutput::text(
                "EDL parsed cleanly but contained zero ops; nothing applied.",
            ));
        }

        // 2-4. Apply against a clone of the current timeline.
        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "apply_edl: failed to read project at {}: {e}",
                ctx.project_root.display()
            ))
        })?;
        let (new_timeline, outcome) = edl_apply(&project.timeline, &envelope).map_err(|e| {
            FunctionCallError::RespondToModel(format_apply_error(&e))
        })?;

        // 6. Commit to disk (skip when dry_run).
        if !args.dry_run {
            let mut updated = project.clone();
            updated.timeline = new_timeline;
            updated.write(&ctx.project_root).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "apply_edl: timeline written-validate ok but disk write failed: {e}"
                ))
            })?;
        }

        // Build the response. The model gets a one-line-per-op log so it
        // can confirm what landed; the TUI subscribes to a future
        // EditPlanUpdate for richer rendering.
        let mut summary = format!(
            "applied {} op(s){}:",
            outcome.applied.len(),
            if args.dry_run { " (dry-run, NOT committed)" } else { "" }
        );
        for op in &outcome.applied {
            summary.push_str(&format!("\n  {}. {}", op.index + 1, op.description));
        }
        Ok(ToolOutput::text(summary))
    }
}

fn format_parse_error(e: &EdlParseError) -> String {
    format!(
        "apply_edl: parse failed — {e}. The envelope must begin with `*** Begin EDL` and \
         end with `*** End EDL`; ops are `*** Trim Clip | Delete Clip | Insert BRoll | \
         Move Clip | Insert Transition`. Anchors look like `@@ anchor: \
         transcript_snippet=\"...\"` or `@@ anchor: clip_uuid=...`."
    )
}

fn format_apply_error(e: &ApplyError) -> String {
    format!("apply_edl: apply failed — {e}")
}

const DESCRIPTION: &str = "\
Apply an Edit Decision List (EDL) to the project timeline. The EDL is a \
freeform envelope (NOT JSON-escaped multi-line content — pass the raw \
text). Begins with `*** Begin EDL` and ends with `*** End EDL`. \
Operations: Trim Clip, Delete Clip (Insert BRoll / Move Clip / Insert \
Transition land in a future batch). Each op identifies its target by \
*content anchor* — transcript_snippet, clip_uuid, scene_change_index — \
not absolute timestamps; this lets edits survive prior changes in the \
same envelope. Set dry_run=true to validate without committing.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
    use awidat_proto::otio::{
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild,
        TimeRange, Track, TrackChild, TrackKind,
    };
    use std::path::Path;
    use tokio::sync::broadcast;

    fn ctx_at(root: &Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),

            approval_tx: None,
        }
    }

    fn invoke(args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: "apply_edl".into(),
            args,
        }
    }

    fn project_with_three_clips() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let mut track = Track::empty("V1", TrackKind::Video);
        for (i, snip) in ["alpha snippet", "bravo snippet", "charlie snippet"]
            .iter()
            .enumerate()
        {
            let mut c = Clip::empty(format!("clip-{i}"));
            c.media_reference =
                MediaReference::External(ExternalReference::new(format!("raw/{i}.mp4")));
            c.source_range = Some(TimeRange::new(
                RationalTime::new(0.0, 24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
            c.metadata = ClipMetadata {
                awidat: Some(AwidatClipMetadata {
                    anchor: Some(AwAnchor {
                        transcript_snippet: Some((*snip).to_string()),
                        ..AwAnchor::default()
                    }),
                    ..AwidatClipMetadata::default()
                }),
                ..ClipMetadata::default()
            };
            track.children.push(TrackChild::Clip(c));
        }
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(track));
        project.write(dir.path()).unwrap();
        dir
    }

    #[tokio::test]
    async fn happy_path_trim_commits_to_disk() {
        let dir = project_with_three_clips();
        let edl = "\
*** Begin EDL
*** Trim Clip
@@ anchor: transcript_snippet=\"bravo\"
+ end: 3.0
*** End EDL
";
        let out = ApplyEdlTool
            .handle(invoke(serde_json::json!({"edl": edl})), ctx_at(dir.path()))
            .await
            .unwrap();
        assert!(out.content.contains("applied 1 op"));
        assert!(out.content.contains("trimmed clip \"clip-1\""));

        // Re-read project: the trim should be persisted.
        let p = Project::read(dir.path()).unwrap();
        let StackChild::Track(t) = &p.timeline.tracks.children[0] else { panic!() };
        let TrackChild::Clip(c) = &t.children[1] else { panic!() };
        assert!((c.source_range.as_ref().unwrap().duration.to_seconds() - 3.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn dry_run_does_not_commit() {
        let dir = project_with_three_clips();
        let edl = "\
*** Begin EDL
*** Trim Clip
@@ anchor: transcript_snippet=\"bravo\"
+ end: 3.0
*** End EDL
";
        let out = ApplyEdlTool
            .handle(
                invoke(serde_json::json!({"edl": edl, "dry_run": true})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        assert!(out.content.contains("dry-run, NOT committed"));

        // On-disk timeline unchanged.
        let p = Project::read(dir.path()).unwrap();
        let StackChild::Track(t) = &p.timeline.tracks.children[0] else { panic!() };
        let TrackChild::Clip(c) = &t.children[1] else { panic!() };
        assert!((c.source_range.as_ref().unwrap().duration.to_seconds() - 5.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn parse_error_is_respond_to_model_with_format_hint() {
        let dir = project_with_three_clips();
        let edl = "this is not an EDL";
        let err = ApplyEdlTool
            .handle(
                invoke(serde_json::json!({"edl": edl})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("parse failed"));
                assert!(msg.contains("`*** Begin EDL`"));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn anchor_miss_includes_did_you_mean_candidates() {
        let dir = project_with_three_clips();
        let edl = "\
*** Begin EDL
*** Delete Clip
@@ anchor: transcript_snippet=\"no such clip exists\"
*** End EDL
";
        let err = ApplyEdlTool
            .handle(invoke(serde_json::json!({"edl": edl})), ctx_at(dir.path()))
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("apply failed"));
                assert!(msg.contains("Did you mean"));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unimplemented_op_surfaces_clear_error() {
        let dir = project_with_three_clips();
        let edl = "\
*** Begin EDL
*** Move Clip
@@ anchor: clip_uuid=clip-0
+ to_position: 2
*** End EDL
";
        let err = ApplyEdlTool
            .handle(invoke(serde_json::json!({"edl": edl})), ctx_at(dir.path()))
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("Move Clip"));
                assert!(msg.contains("not yet implemented"));
            }
            other => panic!("want RespondToModel, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_envelope_does_not_error() {
        let dir = project_with_three_clips();
        let edl = "*** Begin EDL\n*** End EDL\n";
        let out = ApplyEdlTool
            .handle(invoke(serde_json::json!({"edl": edl})), ctx_at(dir.path()))
            .await
            .unwrap();
        assert!(out.content.contains("zero ops"));
    }

    #[test]
    fn dry_run_is_not_mutating() {
        let inv = invoke(serde_json::json!({
            "edl": "*** Begin EDL\n*** End EDL\n",
            "dry_run": true,
        }));
        assert!(!ApplyEdlTool.is_mutating(&inv));

        let inv = invoke(serde_json::json!({
            "edl": "*** Begin EDL\n*** End EDL\n",
        }));
        assert!(ApplyEdlTool.is_mutating(&inv));
    }
}
