//! Granular timeline agent tools.
//!
//! These tools intentionally lower to `apply_edl` envelopes instead of
//! mutating OTIO directly. They give agents a precise tool surface for common
//! timeline actions while preserving the existing validation, approval,
//! hook, write, and vedit audit path.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{
    ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput, ToolRegistry,
};
use crate::tool_schema::Tool as ToolSchema;
use crate::tools::apply_edl::ApplyEdlTool;

/// Register all granular timeline wrappers.
pub fn register_granular_timeline_tools(registry: &mut ToolRegistry) {
    registry.register(Arc::new(SplitClipTool));
    registry.register(Arc::new(TrimClipTool));
    registry.register(Arc::new(MoveClipTool));
    registry.register(Arc::new(DeleteClipsTool));
    registry.register(Arc::new(RollTrimTool));
    registry.register(Arc::new(SlipClipTool));
    registry.register(Arc::new(RippleTrimTool));
    registry.register(Arc::new(SetMarkerTool));
    registry.register(Arc::new(SetClipPropertyTool));
    registry.register(Arc::new(SetTrackPropertyTool));
}

/// Split a clip at a source-media timestamp.
pub struct SplitClipTool;
/// Trim a clip source range.
pub struct TrimClipTool;
/// Move a clip by index or timeline time.
pub struct MoveClipTool;
/// Delete one or more clips in one EDL envelope.
pub struct DeleteClipsTool;
/// Roll an edit boundary between adjacent clips.
pub struct RollTrimTool;
/// Slip a clip source range while preserving timeline placement.
pub struct SlipClipTool;
/// Ripple-trim a clip end and shift downstream material through EDL lowering.
pub struct RippleTrimTool;
/// Add or update a timeline marker through professional EDL lowering.
pub struct SetMarkerTool;
/// Set basic clip properties through existing EDL ops.
pub struct SetClipPropertyTool;
/// Set basic track properties through existing EDL ops.
pub struct SetTrackPropertyTool;

#[derive(Debug, Clone, Deserialize)]
struct AnchorArg {
    #[serde(default)]
    clip_uuid: Option<String>,
    #[serde(default)]
    transcript_snippet: Option<String>,
    #[serde(default)]
    asset_id: Option<String>,
    #[serde(default)]
    scene_change_index: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct SplitClipArgs {
    anchor: AnchorArg,
    at_s: f64,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TrimClipArgs {
    anchor: AnchorArg,
    #[serde(default)]
    start: Option<f64>,
    #[serde(default)]
    end: Option<f64>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MoveClipArgs {
    anchor: AnchorArg,
    #[serde(default)]
    to_position: Option<usize>,
    #[serde(default)]
    at_s: Option<f64>,
    #[serde(default)]
    snap: Option<bool>,
    #[serde(default)]
    snap_tolerance_s: Option<f64>,
    #[serde(default)]
    snap_targets: Vec<String>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeleteClipsArgs {
    anchors: Vec<AnchorArg>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RollTrimArgs {
    outgoing_anchor: AnchorArg,
    incoming_anchor: AnchorArg,
    delta_s: f64,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SlipClipArgs {
    anchor: AnchorArg,
    delta_s: f64,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RippleTrimArgs {
    anchor: AnchorArg,
    new_end_s: f64,
    #[serde(default)]
    ripple_tracks: Vec<String>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SetMarkerArgs {
    #[serde(default)]
    marker_id: Option<String>,
    #[serde(default)]
    anchor: Option<AnchorArg>,
    #[serde(default)]
    at_s: Option<f64>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    duration_s: Option<f64>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SetClipPropertyArgs {
    anchor: AnchorArg,
    #[serde(default)]
    volume: Option<f64>,
    #[serde(default)]
    speed: Option<f64>,
    #[serde(default)]
    effect: Option<String>,
    #[serde(default)]
    params: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    rationale: Option<String>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SetTrackPropertyArgs {
    track: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    volume: Option<f64>,
    #[serde(default)]
    muted: Option<bool>,
    #[serde(default)]
    solo: Option<bool>,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    reasoning: Option<String>,
}

trait GranularTimelineTool: ToolHandler {
    fn build_edl(&self, args: serde_json::Value) -> Result<DelegatedEdl, FunctionCallError>;
}

#[derive(Debug)]
struct DelegatedEdl {
    edl: String,
    dry_run: bool,
    reasoning: Option<String>,
}

macro_rules! impl_granular_tool {
    ($tool:ty, $name:literal, $description:literal, $schema:expr) => {
        #[async_trait]
        impl ToolHandler for $tool {
            fn name(&self) -> &'static str {
                $name
            }

            fn schema(&self) -> ToolSchema {
                ToolSchema {
                    name: $name.into(),
                    description: $description.into(),
                    input_schema: $schema,
                    ..Default::default()
                }
            }

            fn is_mutating(&self, invocation: &ToolInvocation) -> bool {
                !invocation
                    .args
                    .get("dry_run")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            }

            fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
                match self.build_edl(invocation.args.clone()) {
                    Ok(delegated) => ApplyEdlTool.approval_keys(&ToolInvocation {
                        call_id: invocation.call_id.clone(),
                        name: "apply_edl".into(),
                        args: apply_args(delegated),
                    }),
                    Err(_) => vec![ApprovalKey::for_invocation(invocation)],
                }
            }

            async fn handle(
                &self,
                invocation: ToolInvocation,
                ctx: ToolContext,
            ) -> Result<ToolOutput, FunctionCallError> {
                let delegated = self.build_edl(invocation.args)?;
                ApplyEdlTool
                    .handle(
                        ToolInvocation {
                            call_id: invocation.call_id,
                            name: "apply_edl".into(),
                            args: apply_args(delegated),
                        },
                        ctx,
                    )
                    .await
            }
        }
    };
}

impl_granular_tool!(
    SplitClipTool,
    "split_clip",
    "Split a clip by lowering to apply_edl's graph-native Split Clip operation; apply_edl remains the canonical mutation path.",
    serde_json::json!({
        "type": "object",
        "properties": {
            "anchor": anchor_schema(),
            "at_s": {"type": "number", "description": "Source-media time in seconds where the clip is split."},
            "dry_run": dry_run_schema(),
            "reasoning": reasoning_schema()
        },
        "required": ["anchor", "at_s"]
    })
);

impl GranularTimelineTool for SplitClipTool {
    fn build_edl(&self, args: serde_json::Value) -> Result<DelegatedEdl, FunctionCallError> {
        let args: SplitClipArgs = parse_args(self.name(), args)?;
        let op = format!(
            "*** Split Clip\n@@ anchor: {}\n+ at_s: {}",
            edl_anchor(&args.anchor)?,
            fmt_num(args.at_s)
        );
        Ok(delegated_edl(vec![op], args.dry_run, args.reasoning))
    }
}

impl_granular_tool!(
    TrimClipTool,
    "trim_clip",
    "Trim a clip by lowering to apply_edl's graph-native Trim Clip operation; apply_edl remains the canonical mutation path.",
    serde_json::json!({
        "type": "object",
        "properties": {
            "anchor": anchor_schema(),
            "start": {"type": "number", "description": "Optional new source start in seconds."},
            "end": {"type": "number", "description": "Optional new source end in seconds."},
            "dry_run": dry_run_schema(),
            "reasoning": reasoning_schema()
        },
        "required": ["anchor"]
    })
);

impl GranularTimelineTool for TrimClipTool {
    fn build_edl(&self, args: serde_json::Value) -> Result<DelegatedEdl, FunctionCallError> {
        let args: TrimClipArgs = parse_args(self.name(), args)?;
        let mut op = format!("*** Trim Clip\n@@ anchor: {}", edl_anchor(&args.anchor)?);
        push_opt_num(&mut op, "start", args.start);
        push_opt_num(&mut op, "end", args.end);
        Ok(delegated_edl(vec![op], args.dry_run, args.reasoning))
    }
}

impl_granular_tool!(
    MoveClipTool,
    "move_clip",
    "Move a clip by lowering to apply_edl's Move Clip operation; apply_edl remains the canonical mutation path.",
    serde_json::json!({
        "type": "object",
        "properties": {
            "anchor": anchor_schema(),
            "to_position": {"type": "integer", "description": "Optional destination child index on the current track."},
            "at_s": {"type": "number", "description": "Optional absolute timeline start time in seconds."},
            "snap": {"type": "boolean", "description": "Enable snapping for at_s moves."},
            "snap_tolerance_s": {"type": "number", "description": "Snap tolerance in seconds."},
            "snap_targets": {"type": "array", "items": {"type": "string", "enum": ["clip_edge", "marker", "playhead"]}},
            "dry_run": dry_run_schema(),
            "reasoning": reasoning_schema()
        },
        "required": ["anchor"]
    })
);

impl GranularTimelineTool for MoveClipTool {
    fn build_edl(&self, args: serde_json::Value) -> Result<DelegatedEdl, FunctionCallError> {
        let args: MoveClipArgs = parse_args(self.name(), args)?;
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
        Ok(delegated_edl(vec![op], args.dry_run, args.reasoning))
    }
}

impl_granular_tool!(
    DeleteClipsTool,
    "delete_clips",
    "Delete one or more clips in a single apply_edl envelope; apply_edl remains the canonical mutation path.",
    serde_json::json!({
        "type": "object",
        "properties": {
            "anchors": {"type": "array", "items": anchor_schema(), "minItems": 1},
            "dry_run": dry_run_schema(),
            "reasoning": reasoning_schema()
        },
        "required": ["anchors"]
    })
);

impl GranularTimelineTool for DeleteClipsTool {
    fn build_edl(&self, args: serde_json::Value) -> Result<DelegatedEdl, FunctionCallError> {
        let args: DeleteClipsArgs = parse_args(self.name(), args)?;
        if args.anchors.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "delete_clips: anchors must contain at least one clip anchor".into(),
            ));
        }
        let mut ops = Vec::with_capacity(args.anchors.len());
        for anchor in &args.anchors {
            ops.push(format!(
                "*** Delete Clip\n@@ anchor: {}",
                edl_anchor(anchor)?
            ));
        }
        Ok(delegated_edl(ops, args.dry_run, args.reasoning))
    }
}

impl_granular_tool!(
    RollTrimTool,
    "roll_trim",
    "Roll an edit point by lowering to apply_edl's Professional Timeline Edit contract; apply_edl remains the canonical mutation path.",
    serde_json::json!({
        "type": "object",
        "properties": {
            "outgoing_anchor": anchor_schema(),
            "incoming_anchor": anchor_schema(),
            "delta_s": {"type": "number", "description": "Roll delta in seconds."},
            "dry_run": dry_run_schema(),
            "reasoning": reasoning_schema()
        },
        "required": ["outgoing_anchor", "incoming_anchor", "delta_s"]
    })
);

impl GranularTimelineTool for RollTrimTool {
    fn build_edl(&self, args: serde_json::Value) -> Result<DelegatedEdl, FunctionCallError> {
        let args: RollTrimArgs = parse_args(self.name(), args)?;
        let edit = serde_json::json!({
            "edit": "roll_edit",
            "between": {
                "from": json_anchor(&args.outgoing_anchor)?,
                "to": json_anchor(&args.incoming_anchor)?,
            },
            "delta_s": args.delta_s
        });
        Ok(delegated_edl(
            vec![professional_edit_op(edit)],
            args.dry_run,
            args.reasoning,
        ))
    }
}

impl_granular_tool!(
    SlipClipTool,
    "slip_clip",
    "Slip a clip by lowering to apply_edl's Professional Timeline Edit contract; apply_edl remains the canonical mutation path.",
    serde_json::json!({
        "type": "object",
        "properties": {
            "anchor": anchor_schema(),
            "delta_s": {"type": "number", "description": "Source offset delta in seconds."},
            "dry_run": dry_run_schema(),
            "reasoning": reasoning_schema()
        },
        "required": ["anchor", "delta_s"]
    })
);

impl GranularTimelineTool for SlipClipTool {
    fn build_edl(&self, args: serde_json::Value) -> Result<DelegatedEdl, FunctionCallError> {
        let args: SlipClipArgs = parse_args(self.name(), args)?;
        let edit = serde_json::json!({
            "edit": "slip_clip",
            "anchor": json_anchor(&args.anchor)?,
            "delta_s": args.delta_s
        });
        Ok(delegated_edl(
            vec![professional_edit_op(edit)],
            args.dry_run,
            args.reasoning,
        ))
    }
}

impl_granular_tool!(
    RippleTrimTool,
    "ripple_trim",
    "Ripple-trim a clip by lowering to apply_edl's Professional Timeline Edit contract; apply_edl remains the canonical mutation path.",
    serde_json::json!({
        "type": "object",
        "properties": {
            "anchor": anchor_schema(),
            "new_end_s": {"type": "number", "description": "New clip end in source/timeline seconds."},
            "ripple_tracks": {"type": "array", "items": {"type": "string"}},
            "dry_run": dry_run_schema(),
            "reasoning": reasoning_schema()
        },
        "required": ["anchor", "new_end_s"]
    })
);

impl GranularTimelineTool for RippleTrimTool {
    fn build_edl(&self, args: serde_json::Value) -> Result<DelegatedEdl, FunctionCallError> {
        let args: RippleTrimArgs = parse_args(self.name(), args)?;
        let edit = serde_json::json!({
            "edit": "ripple_trim",
            "anchor": json_anchor(&args.anchor)?,
            "new_end_s": args.new_end_s,
            "ripple_tracks": args.ripple_tracks
        });
        Ok(delegated_edl(
            vec![professional_edit_op(edit)],
            args.dry_run,
            args.reasoning,
        ))
    }
}

impl_granular_tool!(
    SetMarkerTool,
    "set_marker",
    "Add or update a timeline marker by lowering to apply_edl's Professional Timeline Edit marker operations; apply_edl remains the canonical mutation path.",
    serde_json::json!({
        "type": "object",
        "properties": {
            "marker_id": {"type": "string", "description": "Optional existing marker id. When present, the marker is updated."},
            "anchor": anchor_schema(),
            "at_s": {"type": "number", "description": "Timeline marker time in seconds."},
            "label": {"type": "string"},
            "category": {"type": "string"},
            "note": {"type": "string"},
            "duration_s": {"type": "number"},
            "dry_run": dry_run_schema(),
            "reasoning": reasoning_schema()
        },
        "required": []
    })
);

impl GranularTimelineTool for SetMarkerTool {
    fn build_edl(&self, args: serde_json::Value) -> Result<DelegatedEdl, FunctionCallError> {
        let args: SetMarkerArgs = parse_args(self.name(), args)?;
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
            let at_s = args.at_s.ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "set_marker: at_s is required to add a marker".into(),
                )
            })?;
            let label = args.label.clone().ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "set_marker: label is required to add a marker".into(),
                )
            })?;
            serde_json::json!({
                "edit": "add_marker",
                "at_s": at_s,
                "label": label,
                "category": args.category
            })
        };
        Ok(delegated_edl(
            vec![professional_edit_op(edit)],
            args.dry_run,
            args.reasoning,
        ))
    }
}

impl_granular_tool!(
    SetClipPropertyTool,
    "set_clip_property",
    "Set basic clip properties by lowering to existing apply_edl ops such as Set Volume, Set Speed, or Set Effect; apply_edl remains the canonical mutation path.",
    serde_json::json!({
        "type": "object",
        "properties": {
            "anchor": anchor_schema(),
            "volume": {"type": "number", "description": "Linear audio gain."},
            "speed": {"type": "number", "description": "Playback speed factor."},
            "effect": {"type": "string", "description": "Optional awidat.* effect id."},
            "params": {"type": "object", "description": "Effect params for effect."},
            "rationale": {"type": "string"},
            "dry_run": dry_run_schema(),
            "reasoning": reasoning_schema()
        },
        "required": ["anchor"]
    })
);

impl GranularTimelineTool for SetClipPropertyTool {
    fn build_edl(&self, args: serde_json::Value) -> Result<DelegatedEdl, FunctionCallError> {
        let args: SetClipPropertyArgs = parse_args(self.name(), args)?;
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
            return Err(FunctionCallError::RespondToModel(
                "set_clip_property: provide at least one of volume, speed, or effect".into(),
            ));
        }
        Ok(delegated_edl(ops, args.dry_run, args.reasoning))
    }
}

impl_granular_tool!(
    SetTrackPropertyTool,
    "set_track_property",
    "Set basic track audio properties by lowering to apply_edl's Set Track Audio op; apply_edl remains the canonical mutation path.",
    serde_json::json!({
        "type": "object",
        "properties": {
            "track": {"type": "string"},
            "role": {"type": "string", "description": "Optional role such as dialogue, music, or sfx."},
            "volume": {"type": "number", "description": "Linear track gain."},
            "muted": {"type": "boolean"},
            "solo": {"type": "boolean"},
            "dry_run": dry_run_schema(),
            "reasoning": reasoning_schema()
        },
        "required": ["track"]
    })
);

impl GranularTimelineTool for SetTrackPropertyTool {
    fn build_edl(&self, args: serde_json::Value) -> Result<DelegatedEdl, FunctionCallError> {
        let args: SetTrackPropertyArgs = parse_args(self.name(), args)?;
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
        Ok(delegated_edl(vec![op], args.dry_run, args.reasoning))
    }
}

fn parse_args<T: serde::de::DeserializeOwned>(
    tool_name: &str,
    args: serde_json::Value,
) -> Result<T, FunctionCallError> {
    serde_json::from_value(args)
        .map_err(|e| FunctionCallError::RespondToModel(format!("{tool_name}: invalid args ({e})")))
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

fn apply_args(delegated: DelegatedEdl) -> serde_json::Value {
    serde_json::json!({
        "edl": delegated.edl,
        "dry_run": delegated.dry_run,
        "reasoning": delegated.reasoning
    })
}

fn professional_edit_op(edit: serde_json::Value) -> String {
    format!("*** Professional Timeline Edit\n+ edit_json: {edit}")
}

fn edl_anchor(anchor: &AnchorArg) -> Result<String, FunctionCallError> {
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
    Err(FunctionCallError::RespondToModel(
        "anchor must include clip_uuid, transcript_snippet, or asset_id + scene_change_index"
            .into(),
    ))
}

fn json_anchor(anchor: &AnchorArg) -> Result<serde_json::Value, FunctionCallError> {
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
    Err(FunctionCallError::RespondToModel(
        "anchor must include clip_uuid, transcript_snippet, or asset_id + scene_change_index"
            .into(),
    ))
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

fn anchor_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "clip_uuid": {"type": "string"},
            "transcript_snippet": {"type": "string"},
            "asset_id": {"type": "string"},
            "scene_change_index": {"type": "integer"}
        },
        "description": "Clip anchor. Prefer clip_uuid; transcript_snippet and asset_id + scene_change_index are also supported."
    })
}

fn dry_run_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "boolean",
        "description": "If true, validate through apply_edl without writing the timeline."
    })
}

fn reasoning_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "string",
        "description": "Optional editorial reason forwarded to apply_edl's vedit audit trail."
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use awidat_proto::awidat_meta::{Anchor as AwAnchor, AwidatClipMetadata};
    use awidat_proto::otio::{
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
        Track, TrackChild, TrackKind,
    };
    use awidat_proto::project::Project;
    use tempfile::TempDir;

    use super::*;
    use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolRegistry};

    fn invocation(name: &str, args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "call-1".into(),
            name: name.into(),
            args,
        }
    }

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tokio::sync::broadcast::channel(8).0,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn project_with_three_clips() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        let mut track = Track::empty("V1", TrackKind::Video);
        for (i, snip) in ["alpha snippet", "bravo snippet", "charlie snippet"]
            .iter()
            .enumerate()
        {
            let mut clip = Clip::empty(format!("clip-{i}"));
            clip.media_reference =
                MediaReference::External(ExternalReference::new(format!("raw/{i}.mp4")));
            clip.source_range = Some(TimeRange::new(
                RationalTime::new(0.0, 24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
            clip.metadata = ClipMetadata {
                awidat: Some(AwidatClipMetadata {
                    anchor: Some(AwAnchor {
                        transcript_snippet: Some((*snip).to_string()),
                        ..AwAnchor::default()
                    }),
                    ..AwidatClipMetadata::default()
                }),
                ..ClipMetadata::default()
            };
            track.children.push(TrackChild::Clip(clip));
        }
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(track));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(Track::empty("A1", TrackKind::Audio)));
        project.write(dir.path()).unwrap();
        dir
    }

    #[test]
    fn schemas_expose_granular_editor_operations() {
        let tools: Vec<Box<dyn ToolHandler>> = vec![
            Box::new(SplitClipTool),
            Box::new(TrimClipTool),
            Box::new(MoveClipTool),
            Box::new(DeleteClipsTool),
            Box::new(RollTrimTool),
            Box::new(SlipClipTool),
            Box::new(RippleTrimTool),
            Box::new(SetMarkerTool),
            Box::new(SetClipPropertyTool),
            Box::new(SetTrackPropertyTool),
        ];

        let names = tools.iter().map(|tool| tool.name()).collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "split_clip",
                "trim_clip",
                "move_clip",
                "delete_clips",
                "roll_trim",
                "slip_clip",
                "ripple_trim",
                "set_marker",
                "set_clip_property",
                "set_track_property",
            ]
        );

        for tool in tools {
            let schema = tool.schema();
            assert_eq!(schema.input_schema["type"], "object");
            assert!(
                schema.description.contains("apply_edl"),
                "{} schema should explain the canonical mutation path",
                schema.name
            );
        }
    }

    #[test]
    fn dry_run_is_read_only_for_granular_tools() {
        let inv = invocation(
            "split_clip",
            serde_json::json!({
                "anchor": {"clip_uuid": "clip-1"},
                "at_s": 2.0,
                "dry_run": true
            }),
        );
        assert!(!SplitClipTool.is_mutating(&inv));

        let inv = invocation(
            "split_clip",
            serde_json::json!({
                "anchor": {"clip_uuid": "clip-1"},
                "at_s": 2.0
            }),
        );
        assert!(SplitClipTool.is_mutating(&inv));
    }

    #[test]
    fn approval_keys_delegate_to_apply_edl_operation_hash() {
        let inv = invocation(
            "trim_clip",
            serde_json::json!({
                "anchor": {"clip_uuid": "clip-1"},
                "end": 3.0
            }),
        );
        let keys = TrimClipTool.approval_keys(&inv);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].tool_name, "apply_edl");
        assert!(keys[0].operation.starts_with("edl:"));
    }

    #[tokio::test]
    async fn split_clip_delegates_to_apply_edl_and_changes_timeline() {
        let dir = project_with_three_clips();
        let out = SplitClipTool
            .handle(
                invocation(
                    "split_clip",
                    serde_json::json!({
                        "anchor": {"clip_uuid": "clip-1"},
                        "at_s": 2.0,
                        "reasoning": "Split the bravo section for a later insert."
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        assert!(out.content.contains("committed 1 op"));
        assert!(out.content.contains("split clip"));
        let project = Project::read(dir.path()).unwrap();
        let StackChild::Track(track) = &project.timeline.tracks.children[0] else {
            panic!("expected track")
        };
        assert_eq!(track.children.len(), 4);
    }

    #[tokio::test]
    async fn delete_clips_builds_one_apply_edl_envelope_for_all_targets() {
        let dir = project_with_three_clips();
        let out = DeleteClipsTool
            .handle(
                invocation(
                    "delete_clips",
                    serde_json::json!({
                        "anchors": [
                            {"clip_uuid": "clip-0"},
                            {"clip_uuid": "clip-2"}
                        ],
                        "dry_run": true
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        assert!(out.content.contains("DRY RUN"));
        assert!(out.content.contains("Validated 2 op(s)"));
        let project = Project::read(dir.path()).unwrap();
        let StackChild::Track(track) = &project.timeline.tracks.children[0] else {
            panic!("expected track")
        };
        assert_eq!(track.children.len(), 3, "dry run must not write the graph");
    }

    #[tokio::test]
    async fn roll_slip_ripple_and_marker_use_professional_edl_lowering() {
        let dir = project_with_three_clips();

        SetMarkerTool
            .handle(
                invocation(
                    "set_marker",
                    serde_json::json!({
                        "at_s": 1.0,
                        "label": "Review beat",
                        "category": "review"
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        SlipClipTool
            .handle(
                invocation(
                    "slip_clip",
                    serde_json::json!({
                        "anchor": {"clip_uuid": "clip-1"},
                        "delta_s": 0.5
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let project = Project::read(dir.path()).unwrap();
        let StackChild::Track(track) = &project.timeline.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Clip(first) = &track.children[0] else {
            panic!("expected clip")
        };
        assert_eq!(first.markers.len(), 1);
        let TrackChild::Clip(slipped) = &track.children[1] else {
            panic!("expected clip")
        };
        assert!(
            (slipped
                .source_range
                .as_ref()
                .unwrap()
                .start_time
                .to_seconds()
                - 0.5)
                .abs()
                < 1e-9
        );
    }

    #[tokio::test]
    async fn basic_clip_and_track_property_wrappers_change_timeline() {
        let dir = project_with_three_clips();

        SetClipPropertyTool
            .handle(
                invocation(
                    "set_clip_property",
                    serde_json::json!({
                        "anchor": {"clip_uuid": "clip-1"},
                        "volume": 0.5,
                        "speed": 1.25
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        SetTrackPropertyTool
            .handle(
                invocation(
                    "set_track_property",
                    serde_json::json!({
                        "track": "A1",
                        "role": "dialogue",
                        "volume": 0.8,
                        "muted": false,
                        "solo": true
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let project = Project::read(dir.path()).unwrap();
        let StackChild::Track(video) = &project.timeline.tracks.children[0] else {
            panic!("expected video track")
        };
        let TrackChild::Clip(clip) = &video.children[1] else {
            panic!("expected clip")
        };
        let effect_names = clip
            .effects
            .iter()
            .map(|effect| effect.effect_name.as_str())
            .collect::<Vec<_>>();
        assert!(effect_names.contains(&"awidat.volume"));
        assert!(effect_names.contains(&"awidat.speed"));

        let StackChild::Track(audio) = &project.timeline.tracks.children[1] else {
            panic!("expected audio track")
        };
        let controls = audio
            .metadata
            .get("awidat_audio")
            .and_then(serde_json::Value::as_object)
            .expect("track audio controls should be written");
        assert_eq!(
            controls.get("role").and_then(serde_json::Value::as_str),
            Some("dialogue")
        );
        assert_eq!(
            controls.get("volume").and_then(serde_json::Value::as_f64),
            Some(0.8)
        );
        assert_eq!(
            controls.get("solo").and_then(serde_json::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn registry_can_contain_granular_timeline_tools() {
        let mut registry = ToolRegistry::new();
        register_granular_timeline_tools(&mut registry);
        assert!(registry.get("split_clip").is_some());
        assert!(registry.get("set_track_property").is_some());
    }
}
