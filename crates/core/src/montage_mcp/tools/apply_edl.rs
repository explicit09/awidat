//! `apply_edl` — the load-bearing mutating tool. Ported from
//! `crates/core/src/tools/apply_edl.rs` to the in-process MCP server
//! in step 5 of the codex-harness migration.
//!
//! The handler body of the original tool also captured editorial
//! decision tags (via `editorial_decision_tags`) and ran approvals
//! through `ToolContext.approval_tx`. Both behaviors are intentionally
//! dropped in the port: codex performs the destructive-hint approval
//! before dispatching the call, and the agent loop that consumed
//! editorial tags is being removed in step 7.

use montage_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::edl::{
    AnchorContext, ApplyError, EdlParseError, apply as edl_apply, parse as edl_parse,
};
use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `apply_edl`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ApplyEdlArgs {
    /// The freeform envelope text. Begins with `*** Begin EDL` and
    /// ends with `*** End EDL`.
    pub edl: String,
    /// If true, parse + validate but don't write the new timeline to
    /// disk. The applied-op log is still returned. Default: false.
    #[serde(default)]
    pub dry_run: bool,
    /// Optional editorial reasoning for this envelope. Free text;
    /// captures *why* the agent made these specific edits. Lands in
    /// the auto-commit body as the `Agent reasoning: …` block.
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// Run `apply_edl` against the project resolved from [`McpToolCtx`].
/// Returns the response text as `Ok(String)`; parse / apply / write
/// errors return `Err(String)`.
pub fn run(args: ApplyEdlArgs, ctx: McpToolCtx) -> Result<String, String> {
    let envelope = edl_parse(&args.edl).map_err(|e| format_parse_error(&e))?;

    if envelope.is_empty() {
        return Ok("EDL parsed cleanly but contained zero ops; nothing applied.".to_string());
    }

    // Tier-1 verification: asset-existence check for Insert Clip ops.
    for (i, op) in envelope.ops.iter().enumerate() {
        if let crate::edl::op::EdlOp::InsertClip { asset, .. } = op {
            let abs = ctx.project_root.join(asset);
            if !abs.exists() {
                return Err(format!(
                    "apply_edl: op #{i} (Insert Clip) references {asset:?} \
                     but no such file at {}. Use `list_assets` to see what's \
                     actually under raw/ in this project, or fix the path.",
                    abs.display()
                ));
            }
        }
    }

    // pre_apply_edl hook.
    if let Ok(cfg) = montage_config::Config::load(Some(&ctx.project_root))
        && let Some(cmd) = cfg.hooks.pre_apply_edl.as_deref()
    {
        run_apply_edl_hook("pre_apply_edl", cmd, &args.edl, &ctx.project_root)?;
    }

    let project = Project::read(&ctx.project_root).map_err(|e| {
        format!(
            "apply_edl: failed to read project at {}: {e}",
            ctx.project_root.display()
        )
    })?;
    let anchor_ctx = AnchorContext::with_project_root(ctx.project_root.clone());
    let (new_timeline, outcome) =
        edl_apply(&project.timeline, &envelope, &anchor_ctx).map_err(|e| format_apply_error(&e))?;

    if !args.dry_run {
        let mut updated = project.clone();
        updated.timeline = new_timeline;
        updated.write(&ctx.project_root).map_err(|e| {
            format!("apply_edl: timeline written-validate ok but disk write failed: {e}")
        })?;

        // Phase B auto-commit — best-effort.
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

        // post_apply_edl hook (fire-and-forget).
        if let Ok(cfg) = montage_config::Config::load(Some(&ctx.project_root))
            && let Some(cmd) = cfg.hooks.post_apply_edl.as_deref()
        {
            let stdin_payload = serde_json::json!({
                "applied": outcome.applied.iter().map(|a| &a.description).collect::<Vec<_>>(),
            })
            .to_string();
            if let Err(e) = run_post_hook("post_apply_edl", cmd, &stdin_payload, &ctx.project_root)
            {
                tracing::warn!(error = %e, "post_apply_edl hook failed");
            }
        }
    }

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
    if !args.dry_run
        && outcome
            .applied
            .iter()
            .any(|op| op.metadata.kind == "insert_b_roll")
    {
        summary.push_str(
            "\n\nRequired B-roll verification before claiming B-roll is done: run \
             view_timeline and podcast_visual_polish after this apply_edl result; \
             verify each inserted b-roll asset matches its transcript anchor, verify \
             overlays are not accidentally clustered from append placement, and list \
             any skipped or failed stock/generated candidates explicitly.",
        );
    }
    if !args.dry_run && crate::tools::podcast_qc_report::is_podcast_project(&project) {
        summary.push_str(
            "\n\nRequired podcast follow-up before claiming done or rendering: run \
             podcast_smooth_cut_boundaries for cleanup cuts when applicable, then \
             podcast_post_draft_check, podcast_audio_polish, podcast_visual_polish, \
             and podcast_qc_report after this apply_edl result.",
        );
    }
    Ok(summary)
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

fn format_parse_error(e: &EdlParseError) -> String {
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

fn run_apply_edl_hook(
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
        .map_err(|e| {
            format!(
                "apply_edl: hook {name:?} failed to spawn ({e}). Check that the command \
                 is on PATH and the bash interpreter is available."
            )
        })?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_payload.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("apply_edl: hook {name:?} I/O error ({e})"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        return Err(format!(
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
        ));
    }
    Ok(())
}

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

pub const DESCRIPTION: &str = "\
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
Trim only NARROWS — to widen back out, use Untrim Clip.\
\n  - **Untrim Clip**: `+ start: <source_s>` and/or `+ end: <source_s>` \
(at least one). Widens a previously-trimmed clip back toward the \
original media bounds.\
\n  - **Delete Clip**: no fields.\
\n  - **Split Clip**: `+ at_s: <source_s>` (required). Cut point in \
source-media seconds; must lie strictly inside the clip's current \
source range.\
\n  - **Insert Clip**: `+ asset: <path>` and `+ track: <name>` \
(required). Optional `+ start`, `+ end`, `+ at_position`, `+ name`. \
Creates a new clip from a raw asset and inserts it on the named track. \
The ONLY op that doesn't take an `@@ anchor:` line.\
\n  - **Insert Transition**: `+ kind: <name>` and `+ duration_s: \
<seconds>` (required). Anchored via `@@ between: ANCHOR_A and \
ANCHOR_B`. New EDLs must use a registered `montage.*` transition id \
or `SMPTE_Dissolve`.\
\n  - **Delete Transition**: no fields. Anchored via `@@ between`.\
\n  - **Move Clip**: `+ to_position: <index>` (required).\
\n  - **Insert BRoll**: `+ asset` and `+ duration_s` (required). \
Optional `+ position: <replace|overlay>` (default overlay).\
\n  - **Insert PiP**: `+ asset` and `+ duration_s` required. Optional \
`+ corner`, `+ scale`, `+ margin_pct`.\
\n  - **Set Volume**: `+ value: <gain>`. Linear gain multiplier.\
\n  - **Set Effect**: `+ effect: <montage.effect_id>` plus optional \
`+ params_json: {...}` and `+ rationale: <why>`.\
\n  - **Set Speed**: `+ factor: <multiplier>`.\
\n  - **Set Time Remap**: `+ curve_json: [...]`.\
\n  - **Set Freeze**: `+ freeze_at_source_s` and `+ duration_s`.\
\n  - **Set Color Correction**: any of `+ exposure_ev`, `+ contrast`, \
`+ saturation`, `+ temperature`, `+ tint`, `+ shadows`, `+ highlights`.\
\n  - **Apply LUT**: `+ lut_path: <project-relative-path>` plus \
optional `+ interpolation`, `+ strength`.\
\n  - **Remove LUT**: no fields besides anchor.\
\n  - **Insert Title**: `+ start_s`, `+ end_s`, `+ text` required.\
\n  - **Set Title**: anchored update of an existing title.\
\n  - **Insert Caption**: `+ start_s`, `+ end_s`, `+ text` required.\
\n  - **Set Output Format**: `+ aspect_ratio: <16:9|9:16|1:1|4:5>`.\
\n  - **Set Loudness Target**: `+ integrated_lufs`.\
\n  - **Set Package Metadata**: any of `+ platform`, `+ title`, \
`+ description`, `+ tags`.\
\n  - **Set Broadcast Overlay**: timeline-level overlay config. \
Preferred form is `+ config_json: {...}`.\
\n\n\
**Anchors.** Each op identifies its target by content anchor — \
`transcript_snippet`, `clip_uuid`, `scene_change_index` — not \
absolute timestamps. For `clip_uuid=...`, use the clip anchor shown \
by `view_timeline`.\
\n\n\
**Time semantics.** All time fields are in seconds into the clip's \
source media. After a Trim, source-media seconds still count from \
the *original* media start (at offset 0).\
\n\n\
By default this commits. Set dry_run=true ONLY to validate the parse \
without writing. \
\n\n\
**Reasoning.** Pass `reasoning: \"<one short sentence>\"` whenever \
you have any context for the edit. It lands in the auto-commit body \
so future reads on the commit log have a real audit trail.\
";
