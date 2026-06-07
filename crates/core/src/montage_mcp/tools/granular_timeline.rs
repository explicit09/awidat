//! `granular_timeline` — granular timeline mutator wrappers. Ported
//! from `crates/core/src/tools/granular_timeline.rs` to the in-process
//! MCP server in step 5 of the codex-harness migration.
//!
//! Each sub-tool lowers to an `apply_edl` envelope, then dispatches to
//! the ported [`super::apply_edl::run`]. The original tool surface
//! exposes 10 distinct tools to the model; the rmcp layer in
//! `montage_mcp::mod` registers a `#[tool]` wrapper per sub-tool, all
//! annotated `destructive_hint = true, read_only_hint = false`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::montage_mcp::tools::apply_edl::{self, ApplyEdlArgs};

// ------- Anchor -------

/// Clip anchor. At least one of `clip_uuid`, `transcript_snippet`,
/// or (`asset_id` + `scene_change_index`) must be set.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct AnchorArg {
    #[serde(default)]
    pub clip_uuid: Option<String>,
    #[serde(default)]
    pub transcript_snippet: Option<String>,
    #[serde(default)]
    pub asset_id: Option<String>,
    #[serde(default)]
    pub scene_change_index: Option<u32>,
}

// ------- Args -------

/// Arguments to `split_clip`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct SplitClipArgs {
    pub anchor: AnchorArg,
    pub at_s: f64,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// Arguments to `trim_clip`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct TrimClipArgs {
    pub anchor: AnchorArg,
    #[serde(default)]
    pub start: Option<f64>,
    #[serde(default)]
    pub end: Option<f64>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// Arguments to `move_clip`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct MoveClipArgs {
    pub anchor: AnchorArg,
    #[serde(default)]
    pub to_position: Option<usize>,
    #[serde(default)]
    pub at_s: Option<f64>,
    #[serde(default)]
    pub snap: Option<bool>,
    #[serde(default)]
    pub snap_tolerance_s: Option<f64>,
    #[serde(default)]
    pub snap_targets: Vec<String>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// Arguments to `delete_clips`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct DeleteClipsArgs {
    pub anchors: Vec<AnchorArg>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// Arguments to `roll_trim`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct RollTrimArgs {
    pub outgoing_anchor: AnchorArg,
    pub incoming_anchor: AnchorArg,
    pub delta_s: f64,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// Arguments to `slip_clip`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct SlipClipArgs {
    pub anchor: AnchorArg,
    pub delta_s: f64,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// Arguments to `ripple_trim`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct RippleTrimArgs {
    pub anchor: AnchorArg,
    pub new_end_s: f64,
    #[serde(default)]
    pub ripple_tracks: Vec<String>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// Arguments to `set_marker`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct SetMarkerArgs {
    #[serde(default)]
    pub marker_id: Option<String>,
    #[serde(default)]
    pub anchor: Option<AnchorArg>,
    #[serde(default)]
    pub at_s: Option<f64>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub duration_s: Option<f64>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// Arguments to `set_clip_property`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct SetClipPropertyArgs {
    pub anchor: AnchorArg,
    #[serde(default)]
    pub volume: Option<f64>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub effect: Option<String>,
    #[serde(default)]
    pub params: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub reasoning: Option<String>,
}

/// Arguments to `set_track_property`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct SetTrackPropertyArgs {
    pub track: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub volume: Option<f64>,
    #[serde(default)]
    pub muted: Option<bool>,
    #[serde(default)]
    pub solo: Option<bool>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub reasoning: Option<String>,
}

// ------- DelegatedEdl + helpers -------

struct DelegatedEdl {
    edl: String,
    dry_run: bool,
    reasoning: Option<String>,
}

fn delegated_edl(ops: Vec<String>, dry_run: bool, reasoning: Option<String>) -> DelegatedEdl {
    let mut edl = String::from("*** Begin EDL");
    for op in ops {
        edl.push('\n');
        edl.push_str(&op);
    }
    edl.push_str("\n*** End EDL\n");
    DelegatedEdl {
        edl,
        dry_run,
        reasoning,
    }
}

fn dispatch(delegated: DelegatedEdl, ctx: McpToolCtx) -> Result<String, String> {
    apply_edl::run(
        ApplyEdlArgs {
            edl: delegated.edl,
            dry_run: delegated.dry_run,
            reasoning: delegated.reasoning,
        },
        ctx,
    )
}

fn professional_edit_op(edit: serde_json::Value) -> String {
    format!("*** Professional Timeline Edit\n+ edit_json: {edit}")
}

fn edl_anchor(anchor: &AnchorArg) -> Result<String, String> {
    if let Some(uuid) = anchor.clip_uuid.as_deref() {
        return Ok(format!("clip_uuid={}", single_line(uuid)));
    }
    if let Some(text) = anchor.transcript_snippet.as_deref() {
        return Ok(format!("transcript_snippet={}", json_string(text)));
    }
    if let (Some(asset_id), Some(index)) = (anchor.asset_id.as_deref(), anchor.scene_change_index) {
        return Ok(format!(
            "scene_change_index={}:{}",
            single_line(asset_id),
            index
        ));
    }
    Err(
        "anchor must include clip_uuid, transcript_snippet, or asset_id + scene_change_index"
            .into(),
    )
}

fn json_anchor(anchor: &AnchorArg) -> Result<serde_json::Value, String> {
    if let Some(uuid) = anchor.clip_uuid.as_deref() {
        return Ok(serde_json::json!({"anchor_kind": "clip_uuid", "uuid": uuid}));
    }
    if let Some(text) = anchor.transcript_snippet.as_deref() {
        return Ok(serde_json::json!({"anchor_kind": "transcript_snippet", "text": text}));
    }
    if let (Some(asset_id), Some(index)) = (anchor.asset_id.as_deref(), anchor.scene_change_index) {
        return Ok(serde_json::json!({
            "anchor_kind": "scene_change_index",
            "asset_id": asset_id,
            "index": index
        }));
    }
    Err(
        "anchor must include clip_uuid, transcript_snippet, or asset_id + scene_change_index"
            .into(),
    )
}

fn push_opt_num(op: &mut String, key: &str, value: Option<f64>) {
    if let Some(value) = value {
        op.push_str(&format!("\n+ {key}: {}", fmt_num(value)));
    }
}

fn fmt_num(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

fn single_line(value: impl AsRef<str>) -> String {
    value.as_ref().replace(['\n', '\r'], " ").trim().to_string()
}

fn json_string(value: &str) -> String {
    serde_json::to_string(&single_line(value)).unwrap_or_else(|_| "\"\"".into())
}

// ------- Run functions -------

/// Run `split_clip`.
pub fn run_split_clip(args: SplitClipArgs, ctx: McpToolCtx) -> Result<String, String> {
    let op = format!(
        "*** Split Clip\n@@ anchor: {}\n+ at_s: {}",
        edl_anchor(&args.anchor)?,
        fmt_num(args.at_s)
    );
    dispatch(delegated_edl(vec![op], args.dry_run, args.reasoning), ctx)
}

/// Run `trim_clip`.
pub fn run_trim_clip(args: TrimClipArgs, ctx: McpToolCtx) -> Result<String, String> {
    let mut op = format!("*** Trim Clip\n@@ anchor: {}", edl_anchor(&args.anchor)?);
    push_opt_num(&mut op, "start", args.start);
    push_opt_num(&mut op, "end", args.end);
    dispatch(delegated_edl(vec![op], args.dry_run, args.reasoning), ctx)
}

/// Run `move_clip`.
pub fn run_move_clip(args: MoveClipArgs, ctx: McpToolCtx) -> Result<String, String> {
    let mut op = format!("*** Move Clip\n@@ anchor: {}", edl_anchor(&args.anchor)?);
    if let Some(to_position) = args.to_position {
        op.push_str(&format!("\n+ to_position: {to_position}"));
    }
    push_opt_num(&mut op, "at_s", args.at_s);
    if let Some(snap) = args.snap {
        op.push_str(&format!("\n+ snap: {snap}"));
    }
    push_opt_num(&mut op, "snap_tolerance_s", args.snap_tolerance_s);
    if !args.snap_targets.is_empty() {
        op.push_str(&format!(
            "\n+ snap_targets: {}",
            args.snap_targets.join(", ")
        ));
    }
    dispatch(delegated_edl(vec![op], args.dry_run, args.reasoning), ctx)
}

/// Run `delete_clips`.
pub fn run_delete_clips(args: DeleteClipsArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.anchors.is_empty() {
        return Err("delete_clips: anchors must contain at least one clip anchor".into());
    }
    let mut ops = Vec::with_capacity(args.anchors.len());
    for anchor in &args.anchors {
        ops.push(format!(
            "*** Delete Clip\n@@ anchor: {}",
            edl_anchor(anchor)?
        ));
    }
    dispatch(delegated_edl(ops, args.dry_run, args.reasoning), ctx)
}

/// Run `roll_trim`.
pub fn run_roll_trim(args: RollTrimArgs, ctx: McpToolCtx) -> Result<String, String> {
    let edit = serde_json::json!({
        "edit": "roll_edit",
        "between": {
            "from": json_anchor(&args.outgoing_anchor)?,
            "to": json_anchor(&args.incoming_anchor)?,
        },
        "delta_s": args.delta_s
    });
    dispatch(
        delegated_edl(
            vec![professional_edit_op(edit)],
            args.dry_run,
            args.reasoning,
        ),
        ctx,
    )
}

/// Run `slip_clip`.
pub fn run_slip_clip(args: SlipClipArgs, ctx: McpToolCtx) -> Result<String, String> {
    let edit = serde_json::json!({
        "edit": "slip_clip",
        "anchor": json_anchor(&args.anchor)?,
        "delta_s": args.delta_s
    });
    dispatch(
        delegated_edl(
            vec![professional_edit_op(edit)],
            args.dry_run,
            args.reasoning,
        ),
        ctx,
    )
}

/// Run `ripple_trim`.
pub fn run_ripple_trim(args: RippleTrimArgs, ctx: McpToolCtx) -> Result<String, String> {
    let edit = serde_json::json!({
        "edit": "ripple_trim",
        "anchor": json_anchor(&args.anchor)?,
        "new_end_s": args.new_end_s,
        "ripple_tracks": args.ripple_tracks
    });
    dispatch(
        delegated_edl(
            vec![professional_edit_op(edit)],
            args.dry_run,
            args.reasoning,
        ),
        ctx,
    )
}

/// Run `set_marker`.
pub fn run_set_marker(args: SetMarkerArgs, ctx: McpToolCtx) -> Result<String, String> {
    let edit = if args.marker_id.is_some() || args.anchor.is_some() {
        let mut selector = serde_json::Map::new();
        if let Some(marker_id) = args.marker_id {
            selector.insert("marker_id".into(), serde_json::Value::String(marker_id));
        }
        if let Some(anchor) = &args.anchor {
            selector.insert("anchor".into(), json_anchor(anchor)?);
        }
        if let Some(label) = &args.label {
            selector.insert("label".into(), serde_json::Value::String(label.clone()));
        }
        if let Some(at_s) = args.at_s {
            selector.insert("at_s".into(), serde_json::Value::from(at_s));
        }
        serde_json::json!({
            "edit": "update_marker",
            "selector": selector,
            "label": args.label,
            "category": args.category,
            "note": args.note,
            "at_s": args.at_s,
            "duration_s": args.duration_s,
        })
    } else {
        let at_s = args
            .at_s
            .ok_or_else(|| "set_marker: at_s is required to add a marker".to_string())?;
        let label = args
            .label
            .clone()
            .ok_or_else(|| "set_marker: label is required to add a marker".to_string())?;
        serde_json::json!({
            "edit": "add_marker",
            "at_s": at_s,
            "label": label,
            "category": args.category
        })
    };
    dispatch(
        delegated_edl(
            vec![professional_edit_op(edit)],
            args.dry_run,
            args.reasoning,
        ),
        ctx,
    )
}

/// Run `set_clip_property`.
pub fn run_set_clip_property(args: SetClipPropertyArgs, ctx: McpToolCtx) -> Result<String, String> {
    let anchor = edl_anchor(&args.anchor)?;
    let mut ops = Vec::new();
    if let Some(volume) = args.volume {
        ops.push(format!(
            "*** Set Volume\n@@ anchor: {anchor}\n+ value: {}",
            fmt_num(volume)
        ));
    }
    if let Some(speed) = args.speed {
        ops.push(format!(
            "*** Set Speed\n@@ anchor: {anchor}\n+ factor: {}",
            fmt_num(speed)
        ));
    }
    if let Some(effect) = args.effect {
        let mut op = format!(
            "*** Set Effect\n@@ anchor: {anchor}\n+ effect: {effect}\n+ params_json: {}",
            serde_json::Value::Object(args.params)
        );
        if let Some(rationale) = args.rationale {
            op.push_str(&format!("\n+ rationale: {}", single_line(rationale)));
        }
        ops.push(op);
    }
    if ops.is_empty() {
        return Err("set_clip_property: provide at least one of volume, speed, or effect".into());
    }
    dispatch(delegated_edl(ops, args.dry_run, args.reasoning), ctx)
}

/// Run `set_track_property`.
pub fn run_set_track_property(
    args: SetTrackPropertyArgs,
    ctx: McpToolCtx,
) -> Result<String, String> {
    let mut op = format!("*** Set Track Audio\n+ track: {}", single_line(args.track));
    if let Some(role) = args.role {
        op.push_str(&format!("\n+ role: {}", single_line(role)));
    }
    push_opt_num(&mut op, "volume", args.volume);
    if let Some(muted) = args.muted {
        op.push_str(&format!("\n+ muted: {muted}"));
    }
    if let Some(solo) = args.solo {
        op.push_str(&format!("\n+ solo: {solo}"));
    }
    dispatch(delegated_edl(vec![op], args.dry_run, args.reasoning), ctx)
}

// ------- Descriptions -------

pub const DESCRIPTION_SPLIT_CLIP: &str = "Split a clip by lowering to apply_edl's graph-native Split Clip operation; apply_edl remains the canonical mutation path.";

pub const DESCRIPTION_TRIM_CLIP: &str = "Trim a clip by lowering to apply_edl's graph-native Trim Clip operation; apply_edl remains the canonical mutation path.";

pub const DESCRIPTION_MOVE_CLIP: &str = "Move a clip by lowering to apply_edl's Move Clip operation; apply_edl remains the canonical mutation path.";

pub const DESCRIPTION_DELETE_CLIPS: &str = "Delete one or more clips in a single apply_edl envelope; apply_edl remains the canonical mutation path.";

pub const DESCRIPTION_ROLL_TRIM: &str = "Roll an edit point by lowering to apply_edl's Professional Timeline Edit contract; apply_edl remains the canonical mutation path.";

pub const DESCRIPTION_SLIP_CLIP: &str = "Slip a clip by lowering to apply_edl's Professional Timeline Edit contract; apply_edl remains the canonical mutation path.";

pub const DESCRIPTION_RIPPLE_TRIM: &str = "Ripple-trim a clip by lowering to apply_edl's Professional Timeline Edit contract; apply_edl remains the canonical mutation path.";

pub const DESCRIPTION_SET_MARKER: &str = "Add or update a timeline marker by lowering to apply_edl's Professional Timeline Edit marker operations; apply_edl remains the canonical mutation path.";

pub const DESCRIPTION_SET_CLIP_PROPERTY: &str = "Set basic clip properties by lowering to existing apply_edl ops such as Set Volume, Set Speed, or Set Effect; apply_edl remains the canonical mutation path.";

pub const DESCRIPTION_SET_TRACK_PROPERTY: &str = "Set basic track audio properties by lowering to apply_edl's Set Track Audio op; apply_edl remains the canonical mutation path.";
