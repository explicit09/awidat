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
use crate::edl::{
    AnchorContext, ApplyError, EdlParseError, apply as edl_apply, parse as edl_parse,
};
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
            ..Default::default()
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
        let envelope = edl_parse(&args.edl)
            .map_err(|e| FunctionCallError::RespondToModel(format_parse_error(&e)))?;

        if envelope.is_empty() {
            return Ok(ToolOutput::text(
                "EDL parsed cleanly but contained zero ops; nothing applied.",
            ));
        }

        // Tier-1 verification (PLAN.md §9.1): asset-existence check
        // for every Insert Clip op. Catches the common bug of
        // referencing a path that's been moved/deleted before it
        // hits the OTIO writer (where the error is more cryptic).
        // Other tier-1 checks (anchor resolution, frame-range
        // bounds, OTIO round-trip) are inside edl_apply already.
        for (i, op) in envelope.ops.iter().enumerate() {
            if let crate::edl::op::EdlOp::InsertClip { asset, .. } = op {
                let abs = ctx.project_root.join(asset);
                if !abs.exists() {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "apply_edl: op #{i} (Insert Clip) references {asset:?} \
                         but no such file at {}. Use `list_assets` to see what's \
                         actually under raw/ in this project, or fix the path.",
                        abs.display()
                    )));
                }
            }
        }

        // pre_apply_edl hook (PLAN.md §15 Week 7). Runs before
        // edl_apply; receives the raw EDL text on stdin. Non-zero
        // exit aborts the apply_edl call. Hook config is loaded
        // fresh per call to support live-edit during a session.
        if let Ok(cfg) = awidat_config::Config::load(Some(&ctx.project_root))
            && let Some(cmd) = cfg.hooks.pre_apply_edl.as_deref()
        {
            run_apply_edl_hook("pre_apply_edl", cmd, &args.edl, &ctx.project_root)?;
        }

        // 2-4. Apply against a clone of the current timeline.
        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "apply_edl: failed to read project at {}: {e}",
                ctx.project_root.display()
            ))
        })?;
        // Hand the resolver a context so it can search whisper
        // sidecars when the model anchors on a phrase that isn't
        // pre-seeded as clip metadata. Without this the agent can
        // only anchor on the short blurbs we seed at init time.
        let anchor_ctx = AnchorContext::with_project_root(ctx.project_root.clone());
        let (new_timeline, outcome) = edl_apply(&project.timeline, &envelope, &anchor_ctx)
            .map_err(|e| FunctionCallError::RespondToModel(format_apply_error(&e)))?;

        // 6. Commit to disk (skip when dry_run).
        if !args.dry_run {
            let mut updated = project.clone();
            updated.timeline = new_timeline;
            updated.write(&ctx.project_root).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "apply_edl: timeline written-validate ok but disk write failed: {e}"
                ))
            })?;

            // post_apply_edl hook (PLAN.md §15 Week 7). Fires
            // fire-and-forget after the disk write succeeds. Non-
            // zero exit is logged but doesn't fail the apply_edl —
            // we already committed; the agent shouldn't have to
            // un-commit on a side-effect's failure.
            if let Ok(cfg) = awidat_config::Config::load(Some(&ctx.project_root))
                && let Some(cmd) = cfg.hooks.post_apply_edl.as_deref()
            {
                let stdin_payload = serde_json::json!({
                    "applied": outcome.applied.iter().map(|a| &a.description).collect::<Vec<_>>(),
                })
                .to_string();
                if let Err(e) =
                    run_post_hook("post_apply_edl", cmd, &stdin_payload, &ctx.project_root)
                {
                    tracing::warn!(error = %e, "post_apply_edl hook failed");
                }
            }
        }

        // Build the response. Lead with a loud DRY RUN banner when
        // applicable — past traces showed agents quietly retrying a
        // dry-run "succeeded" call in a loop, never noticing nothing
        // landed on disk.
        let mut summary = if args.dry_run {
            format!(
                "DRY RUN — no disk write. Pass dry_run:false to commit. \
                 Validated {} op(s):",
                outcome.applied.len()
            )
        } else {
            format!(
                "committed {} op(s) to project.otio.json:",
                outcome.applied.len()
            )
        };
        for op in &outcome.applied {
            summary.push_str(&format!("\n  {}. {}", op.index + 1, op.description));
        }
        Ok(ToolOutput::text(summary))
    }
}

fn format_parse_error(e: &EdlParseError) -> String {
    // Tack on a per-field example for missing-field errors so the
    // model knows the wire syntax. Real-video runs showed agents
    // failing repeatedly on `at_s` / `start` / `duration_s` because
    // the bare error didn't tell them how to write the field line.
    let hint = match e {
        EdlParseError::MissingField { field, .. } => match field.as_str() {
            "at_s" => Some(
                "Split Clip needs a cut point. Add `+ at_s: <seconds>` \
                 (in source-media seconds) below the @@ anchor line.",
            ),
            "start" | "end" => Some(
                "Trim Clip / Untrim Clip needs at least one of \
                 `+ start: <seconds>` or `+ end: <seconds>` (in \
                 source-media seconds).",
            ),
            "asset" => Some("Insert Clip / Insert BRoll needs `+ asset: <project-relative path>`."),
            "track" => Some(
                "Insert Clip needs `+ track: <track name>`. The track is created \
                 if it doesn't exist (Video kind). Common default: `V1`.",
            ),
            "duration_s" => Some("Insert BRoll needs `+ duration_s: <seconds>`."),
            "anchor" => Some(
                "Every op needs an `@@ anchor: ...` line. Either \
                 transcript_snippet=\"...\" or clip_uuid=<clip name from view_timeline>.",
            ),
            _ => None,
        },
        _ => None,
    };
    let mut msg = format!(
        "apply_edl: parse failed — {e}. The envelope must begin with \
         `*** Begin EDL` and end with `*** End EDL`; ops are `*** Trim \
         Clip | Untrim Clip | Delete Clip | Split Clip | Insert Clip | \
         Insert BRoll | Move Clip | Insert Transition`. Anchors look \
         like `@@ anchor: transcript_snippet=\"...\"` or `@@ anchor: \
         clip_uuid=clip-0`. Insert Clip skips the `@@ anchor:` line — \
         it doesn't anchor against an existing clip."
    );
    if let Some(extra) = hint {
        msg.push_str("\n\nHint: ");
        msg.push_str(extra);
    }
    msg
}

fn format_apply_error(e: &ApplyError) -> String {
    format!("apply_edl: apply failed — {e}")
}

/// Run a pre-apply hook. Synchronous, blocking; non-zero exit
/// raises `RespondToModel` so the agent sees the hook's stderr +
/// can self-correct or retry.
fn run_apply_edl_hook(
    name: &str,
    command: &str,
    stdin_payload: &str,
    cwd: &std::path::Path,
) -> Result<(), FunctionCallError> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("bash")
        .arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "apply_edl: hook {name:?} failed to spawn ({e}). Check that the command \
                 is on PATH and the bash interpreter is available."
            ))
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_payload.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| {
        FunctionCallError::RespondToModel(format!("apply_edl: hook {name:?} I/O error ({e})"))
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        return Err(FunctionCallError::RespondToModel(format!(
            "apply_edl: pre-hook {name:?} rejected the call (exit {}). \
             stdout: {} \
             stderr: {} \
             Adjust the EDL or update the hook config under [hooks].",
            out.status.code().unwrap_or(-1),
            if stdout.is_empty() {
                "(empty)".into()
            } else {
                stdout
            },
            if stderr.is_empty() {
                "(empty)".into()
            } else {
                stderr
            }
        )));
    }
    Ok(())
}

/// Run a post-apply hook. Same shell semantics as
/// `run_apply_edl_hook` but failure is logged, not surfaced — the
/// edit already committed.
fn run_post_hook(
    name: &str,
    command: &str,
    stdin_payload: &str,
    cwd: &std::path::Path,
) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("bash")
        .arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("hook {name:?} failed to spawn: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_payload.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("hook {name:?} I/O: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "hook {name:?} exit {}: {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

const DESCRIPTION: &str = "\
Commit an Edit Decision List (EDL) to the project timeline — this \
WRITES project.otio.json. The EDL is a freeform envelope (NOT \
JSON-escaped multi-line content — pass the raw text). Begins with \
`*** Begin EDL` and ends with `*** End EDL`. \
\n\n\
Operations and their required `+ key: value` fields:\
\n  - **Trim Clip**: `+ start: <source_s>` and/or `+ end: <source_s>` \
(at least one). Times are seconds into the clip's source media. \
Trim only NARROWS — to widen back out, use Untrim Clip. Use \
`view_timeline` first: it shows each clip's current `source=[start..end]`. \
For \"trim the first N seconds\" of a current clip, set `start` to current \
source start + N; for \"trim the last N seconds\", set `end` to current \
source end - N.\
\n  - **Untrim Clip**: `+ start: <source_s>` and/or `+ end: <source_s>` \
(at least one). Widens a previously-trimmed clip back toward the \
original media bounds. Capped to the media reference's available \
range when known.\
\n  - **Delete Clip**: no fields.\
\n  - **Split Clip**: `+ at_s: <source_s>` (required). The cut \
point in source-media seconds; must lie strictly inside the clip's \
current source range.\
\n  - **Insert Clip**: `+ asset: <path>` and `+ track: <name>` \
(required). Optional `+ start: <source_s>`, `+ end: <source_s>`, \
`+ at_position: <index>`, `+ name: <clip_name>`. Creates a new \
clip from a raw asset and inserts it on the named track (track is \
created Video-kind if missing). The ONLY op that doesn't take an \
`@@ anchor:` line — it builds a clip rather than locating one.\
\n  - **Insert Transition**: `+ kind: <name>` and `+ duration_s: \
<seconds>` (required). Anchored via `@@ between: ANCHOR_A and \
ANCHOR_B` where the two anchors identify ADJACENT clips on the \
same track (the transition sits between them at the cut). Common \
kinds: `SMPTE_Dissolve` (cross-fade), `awidat.fade_in`, \
`awidat.fade_out`. The render pipeline maps these to ffmpeg's \
xfade transition names. duration_s applies symmetrically — half \
reaches into the outgoing clip, half into the incoming. v1 does \
not support transitions across gaps or chained transitions \
sharing a clip; if you ask for both, the second one is dropped \
at render time.\
\n  - **Move Clip**: `+ to_position: <index>` (required). Moves \
the anchored clip to a new position within its current track \
(no cross-track moves in v1). Index is the clip's slot in the \
post-move track, clamped to len-1.\
\n  - **Insert BRoll**: `+ asset: <path>` and `+ duration_s: \
<seconds>` (required). Optional `+ position: <replace|overlay>` \
(default `overlay`). Anchored to a clip on the timeline. \
`replace`: the leading duration_s of the anchor clip is swapped \
for the broll. The anchor's residual tail (if any) stays right \
after the broll on the same track. `overlay`: the broll lands on \
a higher video track (a fresh `V<N+1>` is created if no other \
video track exists), at the anchor clip's track-time start, with \
a leading Gap on the overlay track so it lines up under the \
anchor.\
\n  - **Set Volume**: `+ value: <gain>` (required). Linear gain \
multiplier on the clip's audio: `0.0` mutes, `1.0` is unity (no \
change — the default for clips with no Set Volume), values above \
`1.0` amplify (clipping risk). Stamps an `awidat.volume` Effect \
on the clip; re-applying replaces the existing effect rather than \
stacking. Render emits `volume=<value>` on this segment's audio \
stream before concat / xfade.\
\n  - **Set Speed**: `+ factor: <multiplier>` (required). Playback \
rate multiplier: `1.0` is unity (no change), `2.0` plays at double \
speed (half timeline length), `0.5` plays at half speed (double \
length). Stamps an `awidat.speed` Effect; replaces any existing \
one. The clip's contribution to the master timeline duration \
becomes `source_duration / factor`. Render uses \
`setpts=<1/factor>*PTS` on video and chained `atempo=` filters on \
audio (atempo's per-instance range is `[0.5, 2.0]`; factors \
outside chain — extreme values produce audible artifacts, so \
keep within `[0.25, 4.0]` unless the clip is silent).\
\n\n\
**Anchors.** Each op identifies its target by content anchor — \
`transcript_snippet`, `clip_uuid`, `scene_change_index` — not \
absolute timestamps; this lets edits survive prior changes in the \
same envelope. transcript_snippet matches against the clip's \
metadata first, then against the whisper sidecar's segment text \
when the project has been indexed. For `clip_uuid=...`, use the \
clip anchor shown by `view_timeline` (usually the clip name, e.g. \
`clip-0`). Do NOT use the asset filename, proxy stem, or raw media \
basename as the clip_uuid anchor. Call `view_timeline` first when \
you need the clip names.\
\n\n\
**Time semantics.** All time fields are in seconds into the clip's \
source media. After a Trim, the clip's source range narrows but \
source-media seconds still count from the *original* media start \
(at offset 0). Use `inspect_clip` to see the current source range \
before another edit.\
\n\n\
By default this commits. Set dry_run=true ONLY if you want to \
validate the parse without writing — in that case the response \
leads with the banner `DRY RUN — no disk write`. Don't pass \
dry_run if you intend to edit.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
    use awidat_proto::otio::{
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
        Track, TrackChild, TrackKind,
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
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
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
        assert!(out.content.contains("committed 1 op"));
        assert!(out.content.contains("trimmed clip \"clip-1\""));

        // Re-read project: the trim should be persisted.
        let p = Project::read(dir.path()).unwrap();
        let StackChild::Track(t) = &p.timeline.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &t.children[1] else {
            panic!()
        };
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
        assert!(out.content.contains("DRY RUN"));
        assert!(out.content.contains("dry_run:false"));

        // On-disk timeline unchanged.
        let p = Project::read(dir.path()).unwrap();
        let StackChild::Track(t) = &p.timeline.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &t.children[1] else {
            panic!()
        };
        assert!((c.source_range.as_ref().unwrap().duration.to_seconds() - 5.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn parse_error_is_respond_to_model_with_format_hint() {
        let dir = project_with_three_clips();
        let edl = "this is not an EDL";
        let err = ApplyEdlTool
            .handle(invoke(serde_json::json!({"edl": edl})), ctx_at(dir.path()))
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
