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
use crate::edl::op::{BRollPosition, EdlOp};
use crate::edl::{
    AnchorContext, ApplyError, EdlParseError, apply as edl_apply, parse as edl_parse,
};
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};

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
    /// Optional editorial reasoning for this envelope. Free text;
    /// captures *why* the agent made these specific edits. Lands in
    /// the auto-commit body as the `Agent reasoning: …` block, so
    /// future "why did we cut this?" reads on the commit log have
    /// real context. Strongly encouraged for any non-trivial edit.
    /// `None` is permitted (the auto-header alone is still a valid
    /// audit entry) but the agent should pass something whenever it
    /// has any context — even one short sentence.
    #[serde(default)]
    reasoning: Option<String>,
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
                    },
                    "reasoning": {
                        "type": "string",
                        "description": "Optional: WHY you made these edits. Lands in the auto-commit body so 'why did we cut this?' reads on the commit log have real context. One short sentence is enough. Reference rules ('per rhythm-preservation rule'), user requests ('user asked for tighter pacing'), or trigger findings ('matched find_broll_opportunities at 12.4s')."
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

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        let edl = invocation
            .args
            .get("edl")
            .and_then(|v| v.as_str())
            .unwrap_or("<invalid>");
        vec![ApprovalKey::new(
            "apply_edl",
            format!(
                "edl:{}",
                short_sha256(normalize_edl_for_approval(edl).as_bytes())
            ),
        )]
    }

    fn editorial_decision_tags(&self, invocation: &ToolInvocation) -> Vec<String> {
        let Some(edl) = invocation.args.get("edl").and_then(|v| v.as_str()) else {
            return Vec::new();
        };
        edl_parse(edl)
            .map(|envelope| editorial_tags_for_envelope(&envelope))
            .unwrap_or_default()
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

        // Commit to disk (skip when dry_run).
        if !args.dry_run {
            let mut updated = project.clone();
            updated.timeline = new_timeline;
            updated.write(&ctx.project_root).map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "apply_edl: timeline written-validate ok but disk write failed: {e}"
                ))
            })?;

            // Phase B auto-commit. Snapshot the post-apply state
            // into vedit so every accepted envelope becomes a
            // structured commit. Best-effort: a vedit failure must
            // not unwind the disk write that already happened —
            // editing has higher stake than version control.
            //
            // The `reasoning` arg from the agent's apply_edl call
            // becomes the commit body — that's the audit trail's
            // *why*. The auto-header (built from AppliedOp
            // descriptions) is the *what*. Together they answer
            // "why did we cut this?" reads on the commit log.
            match crate::vc::open_or_init(&ctx.project_root) {
                Ok(repo) => {
                    let descriptions: Vec<String> = outcome
                        .applied
                        .iter()
                        .map(|a| a.description.clone())
                        .collect();
                    let action_metadata = action_metadata_for_applied(&outcome.applied);
                    if let Err(e) = crate::vc::auto_commit_apply_with_metadata(
                        &repo,
                        &descriptions,
                        args.reasoning.as_deref(),
                        Some(&action_metadata),
                    ) {
                        tracing::warn!(
                            error = %e,
                            "vedit auto-commit failed; apply_edl write succeeded but no commit landed"
                        );
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        "vedit repo unavailable; skipping auto-commit"
                    );
                }
            }

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
            summary.push_str(&format!(
                "\n  {}. {} ({})",
                op.index + 1,
                op.description,
                format_applied_metadata(&op.metadata)
            ));
        }
        let operation_metadata: Vec<_> = outcome.applied.iter().map(|op| &op.metadata).collect();
        if let Ok(json) = serde_json::to_string(&operation_metadata) {
            summary.push_str("\nmetadata: ");
            summary.push_str(&json);
        }
        if !args.dry_run && crate::tools::podcast_qc_report::is_podcast_project(&project) {
            summary.push_str(
                "\n\nRequired podcast follow-up before claiming done or rendering: run \
                 podcast_smooth_cut_boundaries for cleanup cuts when applicable, then \
                 podcast_post_draft_check, podcast_audio_polish, podcast_visual_polish, \
                 and podcast_qc_report after this apply_edl result.",
            );
        }
        Ok(ToolOutput::text(summary))
    }
}

fn action_metadata_for_applied(applied: &[crate::edl::AppliedOp]) -> crate::vc::ActionMetadata {
    crate::vc::ActionMetadata {
        source: Some("agent".to_string()),
        operations: applied.iter().map(|op| op.metadata.clone()).collect(),
    }
}

fn format_applied_metadata(metadata: &crate::edl::AppliedOpMetadata) -> String {
    let clip_ids = metadata.affected_clip_ids.join(",");
    let track_ids = metadata.affected_track_ids.join(",");
    let params = serde_json::to_string(&metadata.parameters).unwrap_or_else(|_| "{}".into());
    let source = metadata.source.as_deref().unwrap_or("unknown");
    format!(
        "kind={} affected_clip_ids=[{}] affected_track_ids=[{}] source={} params={}",
        metadata.kind, clip_ids, track_ids, source, params
    )
}

fn short_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(bytes);
    let mut out = String::with_capacity(16);
    for b in &hash[..8] {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn normalize_edl_for_approval(edl: &str) -> String {
    edl.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn editorial_tags_for_envelope(envelope: &crate::edl::EdlEnvelope) -> Vec<String> {
    let mut tags = Vec::new();
    let mut transition_count = 0_usize;
    for op in &envelope.ops {
        match op {
            EdlOp::SetCutIntent { spec, .. } => {
                push_tag_owned(&mut tags, "cut_type", serde_tag(&spec.cut_type));
                push_tag(&mut tags, "cut_intent", Some(spec.intent.as_str()));
                push_tag_owned(&mut tags, "audio_relation", serde_tag(&spec.audio_relation));
                if spec.intent == "thematic_montage" {
                    tags.push("broll_mode:thematic_montage".into());
                }
            }
            EdlOp::InsertTransition { kind, spec, .. } => {
                transition_count += 1;
                tags.push(format!("transition_id:{}", normalize_tag_value(kind)));
                if let Some(family) = spec
                    .as_ref()
                    .and_then(|spec| spec.family.as_deref())
                    .or_else(|| {
                        awidat_proto::transitions::lookup_builtin_transition(kind)
                            .map(|transition| transition.family)
                    })
                {
                    push_tag(&mut tags, "transition_family", Some(family));
                }
                if let Some(intent) = spec.as_ref().and_then(|spec| spec.intent.as_deref()) {
                    push_tag(&mut tags, "transition_intent", Some(intent));
                }
            }
            EdlOp::SetAudioLead { lead_s, .. } => {
                tags.push("split_edit:j_cut".into());
                tags.push(format!("audio_lead_range:{}", split_lead_bucket(*lead_s)));
            }
            EdlOp::SetAudioTrail { trail_s, .. } => {
                tags.push("split_edit:l_cut".into());
                tags.push(format!(
                    "audio_trail_range:{}",
                    split_trail_bucket(*trail_s)
                ));
            }
            EdlOp::InsertBRoll { position, .. } => {
                tags.push("broll_mode:literal_cover".into());
                tags.push(format!(
                    "broll_position:{}",
                    match position {
                        BRollPosition::Overlay => "overlay",
                        BRollPosition::Replace => "replace",
                    }
                ));
            }
            EdlOp::SetOutputFormat {
                aspect_ratio,
                platform,
                safe_area,
            } => {
                push_tag(&mut tags, "format_aspect", Some(aspect_ratio.as_str()));
                push_tag(&mut tags, "format_platform", platform.as_deref());
                push_tag(&mut tags, "format_safe_area", safe_area.as_deref());
            }
            _ => {}
        }
    }
    if transition_count > 0 {
        tags.push(format!(
            "transition_density:{}",
            transition_density_bucket(transition_count)
        ));
    }
    tags.sort();
    tags.dedup();
    if tags.contains(&"broll_mode:thematic_montage".to_string()) {
        tags.retain(|tag| tag != "broll_mode:literal_cover");
    }
    tags
}

fn transition_density_bucket(transition_count: usize) -> &'static str {
    match transition_count {
        0 => "none",
        1 => "single",
        2 => "moderate",
        _ => "high",
    }
}

fn push_tag(tags: &mut Vec<String>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        let value = normalize_tag_value(value);
        if !value.is_empty() {
            tags.push(format!("{key}:{value}"));
        }
    }
}

fn push_tag_owned(tags: &mut Vec<String>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        push_tag(tags, key, Some(&value));
    }
}

fn serde_tag<T: serde::Serialize>(value: &T) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
}

fn normalize_tag_value(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c == ':' { 'x' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .collect()
}

fn split_lead_bucket(seconds: f64) -> &'static str {
    if seconds < 0.25 {
        "under_0_25"
    } else if seconds <= 0.60 {
        "0_25_to_0_60"
    } else {
        "over_0_60"
    }
}

fn split_trail_bucket(seconds: f64) -> &'static str {
    if seconds < 0.25 {
        "under_0_25"
    } else if seconds <= 0.80 {
        "0_25_to_0_80"
    } else {
        "over_0_80"
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
            "asset" => Some(
                "Insert Clip / Insert BRoll / Insert PiP needs `+ asset: <project-relative path>`.",
            ),
            "track" => Some(
                "Insert Clip needs `+ track: <track name>`. The track is created \
                 if it doesn't exist (Video kind). Common default: `V1`.",
            ),
            "duration_s" => Some("Insert BRoll / Insert PiP needs `+ duration_s: <seconds>`."),
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
         `*** Begin EDL` and end with `*** End EDL`. Clip ops: \
         `*** Trim Clip | Untrim Clip | Delete Clip | Split Clip | \
         Insert Clip | Insert BRoll | Insert PiP | Move Clip | \
         Ripple Move | Ripple Delete | Ripple Trim`. Gap ops: \
         `*** Delete Gap` (with `+ side: before|after` against a real \
         clip's anchor) and `*** Trim Track Tail` (with `+ track: V1`). \
         Track ops: `*** Insert Track` and `*** Delete Track` (both \
         use `+ name: <track>`; Delete Track refuses populated tracks \
         unless `+ force: true`). Transition ops: `*** Insert \
         Transition | Delete Transition`. Anchors look like `@@ \
         anchor: transcript_snippet=\"...\"` or `@@ anchor: \
         clip_uuid=clip-0`. Insert Clip and the Track ops skip the \
         `@@ anchor:` line — they don't anchor against an existing clip."
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
This is the graph-native editing path. Use it for timeline changes \
instead of rewriting project.otio.json by hand or producing edited \
media directly with bash/FFmpeg. \
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
<seconds>` (required). Optional `+ alignment: <center|start_at_cut|\
end_at_cut>` (default `center`), or explicit `+ in_offset_s: <seconds>` \
and/or `+ out_offset_s: <seconds>` for asymmetric placement. Anchored via `@@ between: ANCHOR_A and \
ANCHOR_B` where the two anchors identify ADJACENT clips on the \
same track. The transition is centered on the cut: half of \
`duration_s` uses incoming pre-roll before the cut, half uses outgoing \
post-roll after the cut. In OTIO terms, `in_offset_s` is incoming pre-roll before \
the cut and `out_offset_s` is outgoing post-roll after the cut. Awidat \
validates source handles before inserting. New EDLs must use a registered \
`awidat.*` transition id or `SMPTE_Dissolve`; raw FFmpeg transition names \
are render-legacy only. Optional `+ composition_json: {...}` stores a \
data-only transition recipe over stable primitives (`opacity`, `push`, \
`wipe`, `zoom`, `blur`, `flash`, `shake`, `chromatic_split`, `pixelize`, \
or stable `atomic` awidat ids). Do not put raw FFmpeg graphs, GLSL, shell, \
or plugin code in transition metadata. Use `awidat.composite` for a \
custom on-the-spot recipe whose `composition_json` defines the actual feel. \
Prefer `awidat.cross_dissolve` or `SMPTE_Dissolve` \
for interview/dialogue smoothing. Use `awidat.fade_black` only for \
intentional chapter resets or endings; it is a fade-through-black, \
not a normal dissolve. Do not use legacy `awidat.fade_in` / \
`awidat.fade_out` between ordinary adjacent clips unless the user \
explicitly asks for a black dip. Transitions may be chained across \
adjacent clips; transitions still cannot cross gaps or non-adjacent \
anchors.\
\n  - **Delete Transition**: no fields. Anchored via `@@ between: \
ANCHOR_A and ANCHOR_B` where the two anchors identify the clips \
immediately before and after an existing transition on the same track.\
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
\n  - **Insert PiP**: `+ asset: <path>` and `+ duration_s: \
<seconds>` required. Optional `+ source_start_s: <seconds>` \
(default 0), `+ corner: <top_left|top_right|bottom_left|bottom_right>` \
(default bottom_right), `+ scale: <0.10..0.60>` (default 0.28), \
and `+ margin_pct: <0.0..0.15>` (default 0.035). Anchored to a clip; \
lands on an upper video track with `awidat.video_overlay` metadata. \
PiP overlay audio is muted in v1.\
\n  - **Set Volume**: `+ value: <gain>` (required). Linear gain \
multiplier on the clip's audio: `0.0` mutes, `1.0` is unity (no \
change — the default for clips with no Set Volume), values above \
`1.0` amplify (clipping risk). Stamps an `awidat.volume` Effect \
on the clip; re-applying replaces the existing effect rather than \
stacking. Render emits `volume=<value>` on this segment's audio \
stream before concat / xfade.\
\n  - **Set Effect**: generic registered clip effect. Required \
fields: `+ effect: <awidat.effect_id>` and optional `+ params_json: \
{...}` plus optional `+ rationale: <why>`. The effect id must exist \
in the in-tree `awidat-effects` registry. Parameters are validated \
against the registry before writing, then stamped as OTIO Effect \
metadata. Same-id effects replace by default unless the registry \
marks them stackable. Use this when an operation is easier to express \
as a semantic graph-native effect than as a dedicated EDL op.\
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
\n  - **Set Time Remap**: `+ curve_json: [...]` (required). Anchored \
to a clip. Stamps an `awidat.time_remap` Effect; re-applying replaces \
the existing time-remap effect rather than stacking. Curve points should \
be ordered objects such as `{\\\"source_time_s\\\":0,\\\"timeline_time_s\\\":0}` \
and can include planner metadata for beat-sync or ramp intent. Use this \
for variable speed ramps; use Set Speed for one constant multiplier.\
\n  - **Set Freeze**: `+ freeze_at_source_s: <seconds>` and \
`+ duration_s: <seconds>` required. Optional `+ freeze_position: at` \
and `+ audio_behavior: silence`. Anchored to a clip. Stamps an \
`awidat.freeze` Effect; re-applying replaces the existing freeze effect. \
Render inserts a held video frame and generated silence for the hold.\
\n  - **Set Color Correction**: optional fields `+ exposure_ev`, \
`+ contrast`, `+ saturation`, `+ temperature`, `+ tint`, `+ shadows`, \
`+ highlights` (at least one required). Anchored to a clip. Stamps \
an `awidat.color_correction` Effect; re-applying replaces the existing \
correction rather than stacking. Render maps these controls to FFmpeg \
color filters before speed, concat, transitions, and title overlays. \
Ranges: exposure_ev `[-4,4]`, contrast/saturation `[0,3]`, all other \
controls `[-1,1]`.\
\n  - **Apply LUT**: `+ lut_path: <project-relative-path>` (required), \
optional `+ interpolation: <nearest|trilinear|tetrahedral|pyramid|prism>`, \
optional `+ strength: <0.0..=1.0>` (default 1.0 = full LUT; values below \
1.0 mix the LUT with the un-graded source via FFmpeg's `split → lut3d → \
blend=all_opacity=<strength>` pattern, where `strength=0.6` means 60% LUT \
+ 40% original). Anchored to a clip. Stamps an `awidat.lut` Effect with a \
project-relative LUT path; paths must be non-empty, non-absolute, must \
not contain `.`, `..`, or backslashes, and must end in `.3dl`, `.cube`, \
`.dat`, `.m3d`, or `.csp`. `.cube` and `.3dl` files are parsed at apply \
time and a structured error is returned if the LUT is malformed. \
Re-applying replaces the prior LUT. Render emits `lut3d` for this segment \
before speed, concat, transitions, and title overlays.\
\n  - **Remove LUT**: no fields besides the anchor. Clears `awidat.lut` \
from the clip while leaving color correction, speed, audio, and title \
effects intact.\
\n  - **Insert Title**: `+ start_s: <seconds>`, `+ end_s: <seconds>`, \
`+ text: \"<string>\"` (required). Optional `+ position: <top|center\
|bottom>` (default `center`), `+ font_size: <px>` (default 64), \
`+ color: <#RRGGBB>` (default `#FFFFFF`), `+ font_weight: <normal|\
bold>` (default `normal`), `+ animation: <none|fade_in|fade_out|\
fade_in_out|slide_in|slide_out>` (default `none`). DOES NOT take \
an `@@ anchor:` line — it builds a new title overlay rather than \
locating one. The Titles track auto-creates on first call. Render \
emits an ffmpeg `drawtext=` filter per title with proportional \
y= positions (top: `h*0.05`, center: `(h-text_h)/2`, bottom: \
`h*0.85`); fade animations modulate `alpha=`, slides modulate \
`x=` or `y=`. start_s / end_s are master-timeline seconds.\
\n  - **Set Title**: anchored update of an existing title by its \
clip_uuid. ALL styling fields are optional — only non-None ones \
update. Same field set as `Insert Title` plus `start_s` / `end_s` \
to retime the overlay window. Errors when the anchored clip has \
no `awidat.title` effect.\
\n  - **Insert Caption**: `+ start_s: <seconds>`, `+ end_s: <seconds>`, \
`+ text: \"<string>\"` (required). Optional `+ position: <top|center|bottom>` \
(default `bottom`), `+ font_size: <px>` (default 52), `+ color: <#RRGGBB>` \
(default `#FFFFFF`), `+ safe_area: <profile>` (default `mobile`), \
`+ word_timings_json: [{\"text\":\"word\",\"start_s\":1.0,\"end_s\":1.2}]` \
(optional transcript word timings for per-word reveal). Captions are graph \
nodes on the Titles track with `role=\"caption\"`; do not burn captions by \
writing a separate render script.\
\n  - **Set Output Format**: `+ aspect_ratio: <16:9|9:16|1:1|4:5>` \
(required). Optional `+ platform: <name>` and `+ safe_area: <profile>`. \
Stores delivery format intent on `timeline.metadata.awidat.extra.output_format`.\
\n  - **Set Loudness Target**: `+ integrated_lufs: <negative number>` \
(required). Optional `+ true_peak_db: <negative ceiling>`. Stores audio \
delivery intent on the timeline graph.\
\n  - **Set Package Metadata**: optional `+ platform`, `+ title`, \
`+ description`, `+ tags` (comma-separated). Stores publishing-package \
intent on the timeline graph; at least one field is required.\
\n  - **Set Broadcast Overlay**: timeline-level overlay config for \
broadcast title cards, lower-thirds, host intro strips, ticker/topic \
badges, and chapter cards. Preferred form is `+ config_json: {...}` \
using the `awidat.broadcast_overlay` schema. Simple field form also \
accepts `+ enabled`, `+ episode_title`, `+ episode_subtitle`, \
`+ show_name`, `+ host_a_name`, `+ host_a_title`, `+ host_a_photo_path`, \
`+ host_b_name`, `+ host_b_title`, `+ host_b_photo_path`, `+ sponsors` \
(JSON array), `+ topics` (JSON array of `{time_seconds,text}`), \
`+ chapters` (same), `+ brand_logo_path`, and `+ style` (JSON object). \
Asset paths must be project-relative and must not contain `..`. \
Re-applying replaces the prior timeline overlay config.\
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
\n\n\
**Reasoning.** Pass `reasoning: \"<one short sentence>\"` whenever \
you have any context for the edit. It lands in the auto-commit \
body so future reads on the commit log have a real audit trail — \
e.g. \"User asked for tighter pacing\", \"Per rhythm-preservation \
rule (rule #5)\", \"Bundled with cross-dissolve because \
assess_continuity returned dirty\". One sentence is enough. \
Skipping it produces a commit with the structured op summary alone, \
which is still better than nothing but loses the *why*.\
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
            sandbox_mode: crate::tool::SandboxMode::Default,
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

    fn mark_project_as_podcast(root: &Path) {
        let mut project = Project::read(root).unwrap();
        project
            .timeline
            .metadata
            .awidat
            .as_mut()
            .unwrap()
            .extra
            .insert(
                "awidat_project_type".into(),
                serde_json::json!({"kind": "podcast"}),
            );
        project.write(root).unwrap();
    }

    #[test]
    fn description_declares_graph_native_edit_path() {
        assert!(DESCRIPTION.contains("graph-native editing path"));
        assert!(DESCRIPTION.contains("project.otio.json by hand"));
        assert!(DESCRIPTION.contains("bash/FFmpeg"));
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
    async fn podcast_apply_edl_output_names_required_follow_up_gates() {
        let dir = project_with_three_clips();
        mark_project_as_podcast(dir.path());
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

        assert!(out.content.contains("Required podcast follow-up"));
        assert!(out.content.contains("podcast_post_draft_check"));
        assert!(out.content.contains("podcast_audio_polish"));
        assert!(out.content.contains("podcast_visual_polish"));
        assert!(out.content.contains("podcast_qc_report"));
    }

    #[tokio::test]
    async fn apply_output_includes_command_style_metadata_without_changing_timeline() {
        let dir = project_with_three_clips();
        let before = Project::read(dir.path()).unwrap();
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
        assert!(out.content.contains("kind=trim_clip"), "{out:?}");
        assert!(
            out.content.contains("affected_clip_ids=[clip-1]"),
            "{out:?}"
        );
        assert!(out.content.contains("affected_track_ids=[V1]"), "{out:?}");
        assert!(out.content.contains("source=agent"), "{out:?}");
        assert!(out.content.contains("params={\"end\":3.0}"), "{out:?}");

        let after = Project::read(dir.path()).unwrap();
        let StackChild::Track(before_track) = &before.timeline.tracks.children[0] else {
            panic!()
        };
        let StackChild::Track(after_track) = &after.timeline.tracks.children[0] else {
            panic!()
        };
        assert_eq!(
            before_track.children.len(),
            after_track.children.len(),
            "metadata enrichment must not add/remove timeline children"
        );
        let TrackChild::Clip(c) = &after_track.children[1] else {
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
    async fn happy_path_auto_commits_to_vedit() {
        // Phase B: every accepted apply_edl envelope produces a vedit
        // commit with the auto-generated header. Best-effort, but on
        // a normal happy-path project it always lands.
        let dir = project_with_three_clips();
        let edl = "\
*** Begin EDL
*** Trim Clip
@@ anchor: transcript_snippet=\"bravo\"
+ end: 3.0
*** End EDL
";
        let _out = ApplyEdlTool
            .handle(invoke(serde_json::json!({"edl": edl})), ctx_at(dir.path()))
            .await
            .unwrap();

        // The repo exists, has at least one commit, and the head
        // commit's message header matches the auto-generator (uses
        // the AppliedOp.description text verbatim for a single op).
        let repo = crate::vc::open_or_init(dir.path()).unwrap();
        let entries = crate::vc::log(&repo, 5).unwrap();
        assert!(!entries.is_empty(), "no auto-commit landed");
        let head = &entries[0];
        // Description for trim is something like
        // "trimmed clip \"clip-1\" to source [0.0..3.0]". Don't pin
        // the exact phrasing — just confirm it landed and is non-empty.
        assert!(
            !head.header.is_empty(),
            "auto-commit header is empty: {head:?}"
        );
        assert!(
            head.header.contains("clip-1") || head.header.contains("trim"),
            "auto-commit header doesn't reflect the op: {}",
            head.header
        );
    }

    #[tokio::test]
    async fn apply_output_includes_command_history_metadata() {
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
                invoke(serde_json::json!({
                    "edl": edl,
                    "reasoning": "User asked for tighter pacing on the bravo segment."
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        assert!(out.content.contains("committed 1 op"));
        assert!(out.content.contains("\"kind\":\"trim_clip\""));
        assert!(out.content.contains("\"affected_clip_ids\":[\"clip-1\"]"));
        assert!(out.content.contains("\"affected_track_ids\":[\"V1\"]"));
        assert!(out.content.contains("\"source\":\"agent\""));
        assert!(out.content.contains("\"end\":3.0"));

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
    async fn vedit_auto_commit_preserves_history_and_adds_action_metadata() {
        let dir = project_with_three_clips();
        let edl = "\
*** Begin EDL
*** Trim Clip
@@ anchor: transcript_snippet=\"bravo\"
+ end: 3.0
*** End EDL
";
        ApplyEdlTool
            .handle(
                invoke(serde_json::json!({
                    "edl": edl,
                    "reasoning": "User asked for a shorter bravo beat."
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let repo = crate::vc::open_or_init(dir.path()).unwrap();
        let entries = crate::vc::log(&repo, 5).unwrap();
        assert_eq!(entries.len(), 1);
        let head = &entries[0];
        assert!(
            head.header.contains("clip-1") || head.header.contains("trim"),
            "old compact history header should remain useful: {}",
            head.header
        );
        assert!(
            head.full_message
                .contains("Agent reasoning: User asked for a shorter bravo beat."),
            "old reasoning body should remain present: {}",
            head.full_message
        );
        let metadata = head
            .action_metadata
            .as_ref()
            .expect("auto-commit should expose action metadata");
        assert_eq!(metadata.source.as_deref(), Some("agent"));
        assert_eq!(metadata.operations.len(), 1);
        let op = &metadata.operations[0];
        assert_eq!(op.kind, "trim_clip");
        assert_eq!(op.affected_clip_ids, vec!["clip-1"]);
        assert_eq!(op.affected_track_ids, vec!["V1"]);
        assert_eq!(
            op.parameters.get("end").and_then(serde_json::Value::as_f64),
            Some(3.0)
        );
    }

    #[tokio::test]
    async fn reasoning_arg_lands_in_auto_commit_body() {
        // Phase B follow-up: the agent passes a `reasoning` string,
        // which becomes the commit body so the audit trail captures
        // *why* the edit happened, not just *what*.
        let dir = project_with_three_clips();
        let edl = "\
*** Begin EDL
*** Trim Clip
@@ anchor: transcript_snippet=\"bravo\"
+ end: 3.0
*** End EDL
";
        ApplyEdlTool
            .handle(
                invoke(serde_json::json!({
                    "edl": edl,
                    "reasoning": "User asked for tighter pacing on the bravo segment."
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let repo = crate::vc::open_or_init(dir.path()).unwrap();
        let entries = crate::vc::log(&repo, 5).unwrap();
        assert!(!entries.is_empty());
        let head_msg = &entries[0].full_message;
        assert!(
            head_msg.contains("Agent reasoning: User asked for tighter pacing"),
            "auto-commit body missing reasoning: {head_msg}"
        );
    }

    #[tokio::test]
    async fn empty_reasoning_does_not_emit_a_blank_body() {
        // Whitespace-only reasoning shouldn't produce a `Agent
        // reasoning:` line with nothing after it. format_commit_message
        // already handles this; we sanity-check the integration.
        let dir = project_with_three_clips();
        let edl = "\
*** Begin EDL
*** Trim Clip
@@ anchor: transcript_snippet=\"bravo\"
+ end: 3.0
*** End EDL
";
        ApplyEdlTool
            .handle(
                invoke(serde_json::json!({
                    "edl": edl,
                    "reasoning": "   \n  "
                })),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let repo = crate::vc::open_or_init(dir.path()).unwrap();
        let entries = crate::vc::log(&repo, 5).unwrap();
        let head_msg = &entries[0].full_message;
        assert!(
            !head_msg.contains("Agent reasoning:"),
            "blank reasoning produced an empty body block: {head_msg}"
        );
    }

    #[tokio::test]
    async fn dry_run_does_not_auto_commit() {
        let dir = project_with_three_clips();
        let edl = "\
*** Begin EDL
*** Trim Clip
@@ anchor: transcript_snippet=\"bravo\"
+ end: 3.0
*** End EDL
";
        ApplyEdlTool
            .handle(
                invoke(serde_json::json!({"edl": edl, "dry_run": true})),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        // No vedit repo should exist at all (dry run never opens it).
        // open_or_init would create one if called; we want zero
        // commits in the freshly-created repo.
        let repo = crate::vc::open_or_init(dir.path()).unwrap();
        let entries = crate::vc::log(&repo, 5).unwrap();
        assert!(
            entries.is_empty(),
            "dry-run produced unexpected commits: {entries:?}"
        );
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

    #[test]
    fn approval_learning_tags_capture_editorial_dimensions() {
        let edl = "\
*** Begin EDL
*** Set Cut Intent
@@ between: clip_uuid=a and clip_uuid=b
+ cut_type: jump_cut
+ intent: thematic_montage
+ audio_relation: sync

*** Insert Transition
@@ between: clip_uuid=b and clip_uuid=c
+ kind: awidat.whip_pan_right
+ duration_s: 0.180
+ intent: hide_motion_jump

*** Set Audio Lead
@@ anchor: clip_uuid=c
+ lead_s: 0.450
+ reason: speaker handoff

*** Insert BRoll
@@ anchor: clip_uuid=c
+ asset: raw/montage/factory.mp4
+ duration_s: 2.000
+ position: overlay
*** End EDL
";
        let inv = invoke(serde_json::json!({ "edl": edl }));
        let tags = crate::tool::ToolHandler::editorial_decision_tags(&ApplyEdlTool, &inv);

        for expected in [
            "cut_type:jump_cut",
            "cut_intent:thematic_montage",
            "audio_relation:sync",
            "transition_family:motion_blur",
            "transition_intent:hide_motion_jump",
            "split_edit:j_cut",
            "audio_lead_range:0_25_to_0_60",
            "broll_mode:thematic_montage",
            "broll_position:overlay",
        ] {
            assert!(
                tags.iter().any(|tag| tag == expected),
                "missing {expected}: {tags:?}"
            );
        }
        assert!(!tags.iter().any(|tag| tag == "broll_mode:literal_cover"));
    }

    #[test]
    fn approval_learning_tags_capture_transition_density() {
        let edl = "\
*** Begin EDL
*** Insert Transition
@@ between: clip_uuid=a and clip_uuid=b
+ kind: awidat.flash_white
+ duration_s: 0.120
+ intent: beat_hit

*** Insert Transition
@@ between: clip_uuid=b and clip_uuid=c
+ kind: awidat.whip_pan_right
+ duration_s: 0.180
+ intent: hide_motion_jump

*** Insert Transition
@@ between: clip_uuid=c and clip_uuid=d
+ kind: awidat.zoom_in
+ duration_s: 0.160
+ intent: emphasis
*** End EDL
";
        let inv = invoke(serde_json::json!({ "edl": edl }));
        let tags = crate::tool::ToolHandler::editorial_decision_tags(&ApplyEdlTool, &inv);

        assert!(
            tags.iter().any(|tag| tag == "transition_density:high"),
            "missing transition density tag: {tags:?}"
        );
    }

    #[test]
    fn approval_learning_tags_capture_project_format_defaults() {
        let edl = "\
*** Begin EDL
*** Set Output Format
+ aspect_ratio: 9:16
+ platform: youtube_shorts
+ safe_area: mobile
*** End EDL
";
        let inv = invoke(serde_json::json!({ "edl": edl }));
        let tags = crate::tool::ToolHandler::editorial_decision_tags(&ApplyEdlTool, &inv);

        for expected in [
            "format_aspect:9x16",
            "format_platform:youtube_shorts",
            "format_safe_area:mobile",
        ] {
            assert!(
                tags.iter().any(|tag| tag == expected),
                "missing {expected}: {tags:?}"
            );
        }
    }
}
