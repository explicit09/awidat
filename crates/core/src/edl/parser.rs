//! One-shot parser for the EDL envelope format. Hand-rolled line-driven
//! state machine that mirrors the grammar in
//! `crates/core/src/edl/EDL_GRAMMAR.md` (sister doc).
//!
//! Per the survey of `harnesses/codex/codex-rs/apply-patch/src/parser.rs`:
//! we don't run a generic Lark runtime. The grammar is the documentation;
//! the parser is line-oriented and commits at every `\n`.
//!
//! # Format (recap)
//!
//! ```text
//! *** Begin EDL
//! *** Trim Clip
//! @@ anchor: transcript_snippet="..."
//! - end: 80.4
//! + end: 78.9
//! *** Delete Clip
//! @@ anchor: transcript_snippet="..."
//! *** End EDL
//! ```
//!
//! Headings start with `*** `. Anchor lines start with `@@ anchor: `. Field
//! lines start with `+ ` (set) or `- ` (informational delta — the model
//! shows the old value alongside the new). For trim, both `- end: <old>`
//! and `+ end: <new>` are accepted; we use the `+` value as authoritative.

use awidat_proto::awidat_meta::{
    AudioRelation, BroadcastOverlayConfig, BroadcastOverlayStyle, BroadcastTimedEntry, CutType,
    SemanticCutSpec, SplitEditSpec,
};
use awidat_proto::professional::{
    AudioFinishingState, ColorFinishingState, CompositionGraph, DeliveryProfile,
    MotionGraphicsTemplate, MotionScene, ParameterAnimation, PipelineReadinessReport,
    PlannerPassContract, PreflightReport, ProposalPackage, ReframeSmoothing, SourceRange,
    SourceSelect, Stringout, TrackingPackage, WorkflowLens,
};
use thiserror::Error;

use super::op::{
    Anchor, AnnotationKind, AudioFxConfig, BRollPosition, CaptionWordTiming, EdlEnvelope, EdlOp,
    EqBand, InsertTrackKind, MotionTemplateAnimation, MulticamApplyPlan, PiPCorner,
    ProfessionalTimelineEdit, RichTextSegment, SnapOptions, SnapTargetKind, TitleAnimation,
    TitlePhases, TitlePosition, TitleWeight, TransitionAlignment, TransitionBetween,
    valid_graphic_color,
};

/// Parse errors. All are `RespondToModel`-shaped — the model gets the
/// string and re-emits a corrected envelope. Line numbers are 1-based.
#[derive(Debug, Error, PartialEq)]
pub enum EdlParseError {
    /// Missing `*** Begin EDL` line.
    #[error("EDL missing `*** Begin EDL` opener")]
    MissingBegin,
    /// Missing `*** End EDL` line.
    #[error("EDL missing `*** End EDL` closer")]
    MissingEnd,
    /// Heading line that doesn't match a known op.
    #[error(
        "line {line}: unknown op heading {heading:?}; expected one of: \
             Trim Clip, Delete Clip, Split Clip, Untrim Clip, Insert Clip, Insert BRoll, Insert PiP, Move Clip, Insert Transition, Delete Transition, Set Cut Intent, Set Volume, Mute Clip, Set Audio Fade, Set Audio Lead, Set Audio Trail, Set Track Audio, Set Ducking, Set Sync Group, Set Clip Audio FX, Set Track Audio FX, Set Effect, Set Speed, Set Time Remap, Set Freeze, Set Color Correction, Apply LUT, Remove LUT, Insert Title, Set Title, Insert Caption, Set Output Format, Set Loudness Target, Set Package Metadata, Set Broadcast Overlay, Author Subject Reframe From Track"
    )]
    UnknownOp {
        /// Line number.
        line: usize,
        /// Raw heading text.
        heading: String,
    },
    /// `@@ anchor: ...` line malformed.
    #[error("line {line}: malformed anchor: {message}")]
    BadAnchor {
        /// Line number.
        line: usize,
        /// Diagnostic.
        message: String,
    },
    /// A `+ key: value` or `- key: value` line was malformed.
    #[error("line {line}: malformed field {raw:?}: {message}")]
    BadField {
        /// Line number.
        line: usize,
        /// Raw line.
        raw: String,
        /// Diagnostic.
        message: String,
    },
    /// An op was missing a required field.
    #[error("line {line}: op missing required field '{field}'")]
    MissingField {
        /// Line number.
        line: usize,
        /// Field name.
        field: String,
    },
    /// Lines outside an op (between Begin and the first heading, or
    /// between End and EOF, or after End).
    #[error("line {line}: stray content {raw:?} (outside any op)")]
    StrayLine {
        /// Line number.
        line: usize,
        /// Raw line.
        raw: String,
    },
}

/// Parse the entire envelope. Whitespace-only lines are ignored.
pub fn parse(text: &str) -> Result<EdlEnvelope, EdlParseError> {
    let mut state = State::Outside;
    let mut envelope = EdlEnvelope::new();
    let mut current: Option<OpBuilder> = None;
    let mut saw_end = false;

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim_end();
        if line.trim().is_empty() {
            continue;
        }

        match state {
            State::Outside => {
                if line == "*** Begin EDL" {
                    state = State::InsideEnvelope;
                    continue;
                }
                return Err(EdlParseError::StrayLine {
                    line: line_no,
                    raw: line.to_string(),
                });
            }
            State::InsideEnvelope => {
                if line == "*** End EDL" {
                    if let Some(b) = current.take() {
                        envelope.ops.push(b.build()?);
                    }
                    saw_end = true;
                    state = State::Closed;
                    continue;
                }
                if let Some(rest) = line.strip_prefix("*** ") {
                    // Start a new op; flush the previous.
                    if let Some(b) = current.take() {
                        envelope.ops.push(b.build()?);
                    }
                    current = Some(OpBuilder::new_for(rest, line_no)?);
                    continue;
                }
                if let Some(rest) = line.strip_prefix("@@ anchor: ") {
                    let b = current.as_mut().ok_or_else(|| EdlParseError::StrayLine {
                        line: line_no,
                        raw: line.to_string(),
                    })?;
                    let anchor = parse_anchor(rest, line_no)?;
                    b.set_anchor(anchor);
                    continue;
                }
                if let Some(rest) = line.strip_prefix("@@ between: ") {
                    let b = current.as_mut().ok_or_else(|| EdlParseError::StrayLine {
                        line: line_no,
                        raw: line.to_string(),
                    })?;
                    b.set_between(parse_between(rest, line_no)?);
                    continue;
                }
                if let Some(field) = line.strip_prefix("+ ") {
                    let b = current.as_mut().ok_or_else(|| EdlParseError::StrayLine {
                        line: line_no,
                        raw: line.to_string(),
                    })?;
                    let (k, v) = parse_field(field, line_no, line)?;
                    b.set_field(&k, v, line_no)?;
                    continue;
                }
                if let Some(_field) = line.strip_prefix("- ") {
                    // `-` lines are informational ("old value"). The model
                    // emits them so a human reading the EDL sees the diff;
                    // the parser ignores them — `+` lines are authoritative.
                    continue;
                }
                return Err(EdlParseError::StrayLine {
                    line: line_no,
                    raw: line.to_string(),
                });
            }
            State::Closed => {
                return Err(EdlParseError::StrayLine {
                    line: line_no,
                    raw: line.to_string(),
                });
            }
        }
    }

    if matches!(state, State::Outside) {
        return Err(EdlParseError::MissingBegin);
    }
    if !saw_end {
        return Err(EdlParseError::MissingEnd);
    }
    Ok(envelope)
}

/// Parser FSM state.
#[derive(Debug, Clone, Copy)]
enum State {
    /// Before `*** Begin EDL`.
    Outside,
    /// Between `Begin EDL` and `End EDL`.
    InsideEnvelope,
    /// After `*** End EDL`.
    Closed,
}

/// Per-op assembly buffer. Built up across heading + anchor + field lines.
struct OpBuilder {
    head_line: usize,
    op_kind: OpKind,
    anchor: Option<Anchor>,
    between: Option<TransitionBetween>,
    fields: Vec<(String, FieldValue)>,
}

#[derive(Debug, Clone, Copy)]
enum OpKind {
    TrimClip,
    DeleteClip,
    SplitClip,
    UntrimClip,
    InsertClip,
    InsertBRoll,
    InsertPiP,
    MoveClip,
    RippleMove,
    RippleDelete,
    RippleTrim,
    DeleteGap,
    TrimTrackTail,
    InsertTrack,
    DeleteTrack,
    ApplyMulticamPlan,
    InsertTransition,
    DeleteTransition,
    SetCutIntent,
    SetVolume,
    MuteClip,
    SetAudioFade,
    SetAudioLead,
    SetAudioTrail,
    SetTrackAudio,
    SetDucking,
    SetSyncGroup,
    SetClipAudioFx,
    SetTrackAudioFx,
    SetEffect,
    SetSpeed,
    SetTimeRemap,
    SetFreeze,
    SetColorCorrection,
    ApplyLut,
    RemoveLut,
    InsertTitle,
    InsertRichTitle,
    InstantiateMotionTemplate,
    SetTitle,
    InsertCaption,
    InsertAnnotation,
    SetOutputFormat,
    SetLoudnessTarget,
    SetPackageMetadata,
    SetBroadcastOverlay,
    SetAssetCatalog,
    SetSourceReview,
    ProfessionalTimelineEdit,
    AddProposalPackage,
    SetParameterAnimation,
    SetMotionTemplate,
    SetMotionScene,
    AttachComposition,
    SetTrackingPackage,
    AuthorSubjectReframeFromTrack,
    SetColorFinishing,
    SetAudioFinishing,
    SelectDeliveryProfile,
    AddPreflightReport,
    SetWorkflowLens,
    SetPipelineReadiness,
}

impl OpBuilder {
    fn new_for(heading: &str, line: usize) -> Result<Self, EdlParseError> {
        let kind = match heading.trim() {
            "Trim Clip" => OpKind::TrimClip,
            "Delete Clip" => OpKind::DeleteClip,
            "Split Clip" => OpKind::SplitClip,
            "Untrim Clip" => OpKind::UntrimClip,
            "Insert Clip" => OpKind::InsertClip,
            "Insert BRoll" => OpKind::InsertBRoll,
            "Insert PiP" => OpKind::InsertPiP,
            "Move Clip" => OpKind::MoveClip,
            "Ripple Move" => OpKind::RippleMove,
            "Ripple Delete" => OpKind::RippleDelete,
            "Ripple Trim" => OpKind::RippleTrim,
            "Delete Gap" => OpKind::DeleteGap,
            "Trim Track Tail" => OpKind::TrimTrackTail,
            "Insert Track" => OpKind::InsertTrack,
            "Delete Track" => OpKind::DeleteTrack,
            "Apply Multicam Plan" => OpKind::ApplyMulticamPlan,
            "Insert Transition" => OpKind::InsertTransition,
            "Delete Transition" => OpKind::DeleteTransition,
            "Set Cut Intent" => OpKind::SetCutIntent,
            "Set Volume" => OpKind::SetVolume,
            "Mute Clip" => OpKind::MuteClip,
            "Set Audio Fade" => OpKind::SetAudioFade,
            "Set Audio Lead" => OpKind::SetAudioLead,
            "Set Audio Trail" => OpKind::SetAudioTrail,
            "Set Track Audio" => OpKind::SetTrackAudio,
            "Set Ducking" => OpKind::SetDucking,
            "Set Sync Group" => OpKind::SetSyncGroup,
            "Set Clip Audio FX" => OpKind::SetClipAudioFx,
            "Set Track Audio FX" => OpKind::SetTrackAudioFx,
            "Set Effect" => OpKind::SetEffect,
            "Set Speed" => OpKind::SetSpeed,
            "Set Time Remap" => OpKind::SetTimeRemap,
            "Set Freeze" => OpKind::SetFreeze,
            "Set Color Correction" => OpKind::SetColorCorrection,
            "Apply LUT" => OpKind::ApplyLut,
            "Remove LUT" => OpKind::RemoveLut,
            "Insert Title" => OpKind::InsertTitle,
            "Insert Rich Title" => OpKind::InsertRichTitle,
            "Instantiate Motion Template" | "Insert Motion Template" => {
                OpKind::InstantiateMotionTemplate
            }
            "Set Title" => OpKind::SetTitle,
            "Insert Caption" => OpKind::InsertCaption,
            "Insert Annotation" => OpKind::InsertAnnotation,
            "Set Output Format" => OpKind::SetOutputFormat,
            "Set Loudness Target" => OpKind::SetLoudnessTarget,
            "Set Package Metadata" => OpKind::SetPackageMetadata,
            "Set Broadcast Overlay" => OpKind::SetBroadcastOverlay,
            "Set Asset Catalog" => OpKind::SetAssetCatalog,
            "Set Source Review" => OpKind::SetSourceReview,
            "Professional Timeline Edit" => OpKind::ProfessionalTimelineEdit,
            "Add Proposal Package" => OpKind::AddProposalPackage,
            "Set Parameter Animation" => OpKind::SetParameterAnimation,
            "Set Motion Template" => OpKind::SetMotionTemplate,
            "Set Motion Scene" => OpKind::SetMotionScene,
            "Attach Composition" => OpKind::AttachComposition,
            "Set Tracking Package" => OpKind::SetTrackingPackage,
            "Author Subject Reframe From Track" => OpKind::AuthorSubjectReframeFromTrack,
            "Set Color Finishing" => OpKind::SetColorFinishing,
            "Set Audio Finishing" => OpKind::SetAudioFinishing,
            "Select Delivery Profile" => OpKind::SelectDeliveryProfile,
            "Add Preflight Report" => OpKind::AddPreflightReport,
            "Set Workflow Lens" => OpKind::SetWorkflowLens,
            "Set Pipeline Readiness" => OpKind::SetPipelineReadiness,
            other => {
                return Err(EdlParseError::UnknownOp {
                    line,
                    heading: other.to_string(),
                });
            }
        };
        Ok(Self {
            head_line: line,
            op_kind: kind,
            anchor: None,
            between: None,
            fields: Vec::new(),
        })
    }

    fn set_anchor(&mut self, anchor: Anchor) {
        self.anchor = Some(anchor);
    }

    fn set_between(&mut self, between: TransitionBetween) {
        self.between = Some(between);
    }

    fn set_field(
        &mut self,
        key: &str,
        value: FieldValue,
        _line: usize,
    ) -> Result<(), EdlParseError> {
        self.fields.push((key.to_string(), value));
        Ok(())
    }

    fn build(self) -> Result<EdlOp, EdlParseError> {
        let head = self.head_line;
        let mut fields = self.fields;
        match self.op_kind {
            OpKind::TrimClip => Ok(EdlOp::TrimClip {
                anchor: self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?,
                start: take_field_f64(&mut fields, "start"),
                end: take_field_f64(&mut fields, "end"),
            }),
            OpKind::DeleteClip => Ok(EdlOp::DeleteClip {
                anchor: self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?,
            }),
            OpKind::SplitClip => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let at_s = take_field_f64(&mut fields, "at_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "at_s".into(),
                    }
                })?;
                let snap = parse_snap_options(&mut fields, head)?;
                Ok(EdlOp::SplitClip { anchor, at_s, snap })
            }
            OpKind::UntrimClip => Ok(EdlOp::UntrimClip {
                anchor: self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?,
                start: take_field_f64(&mut fields, "start"),
                end: take_field_f64(&mut fields, "end"),
            }),
            OpKind::InsertClip => {
                let asset = take_field_string(&mut fields, "asset").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "asset".into(),
                    }
                })?;
                let track = take_field_string(&mut fields, "track").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "track".into(),
                    }
                })?;
                let track_kind = take_field_string(&mut fields, "track_kind")
                    .as_deref()
                    .map(|raw| parse_insert_track_kind(raw, head))
                    .transpose()?;
                let at_position = take_field_usize(&mut fields, "at_position");
                let at_s = take_field_f64(&mut fields, "at_s");
                let start = take_field_f64(&mut fields, "start");
                let end = take_field_f64(&mut fields, "end");
                let name = take_field_string(&mut fields, "name");
                let link_group_id = take_field_string(&mut fields, "link_group_id");
                let snap = parse_snap_options(&mut fields, head)?;
                Ok(EdlOp::InsertClip {
                    asset,
                    track,
                    track_kind,
                    at_position,
                    at_s,
                    start,
                    end,
                    name,
                    link_group_id,
                    snap,
                })
            }
            OpKind::InsertBRoll => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let asset = take_field_string(&mut fields, "asset").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "asset".into(),
                    }
                })?;
                let duration_s = take_field_f64(&mut fields, "duration_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "duration_s".into(),
                    }
                })?;
                let position = match take_field_string(&mut fields, "position").as_deref() {
                    Some("overlay") | None => BRollPosition::Overlay,
                    Some("replace") => BRollPosition::Replace,
                    Some(other) => {
                        return Err(EdlParseError::BadField {
                            line: head,
                            raw: format!("position: {other}"),
                            message: "must be 'overlay' or 'replace'".into(),
                        });
                    }
                };
                Ok(EdlOp::InsertBRoll {
                    anchor,
                    asset,
                    duration_s,
                    position,
                })
            }
            OpKind::InsertPiP => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let asset = take_field_string(&mut fields, "asset").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "asset".into(),
                    }
                })?;
                let duration_s = take_field_f64(&mut fields, "duration_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "duration_s".into(),
                    }
                })?;
                let source_start_s = take_field_f64(&mut fields, "source_start_s").unwrap_or(0.0);
                let corner = parse_pip_corner(
                    take_field_string(&mut fields, "corner")
                        .as_deref()
                        .unwrap_or("bottom_right"),
                    head,
                )?;
                let scale = take_field_f64(&mut fields, "scale").unwrap_or(0.28);
                let margin_pct = take_field_f64(&mut fields, "margin_pct").unwrap_or(0.035);
                validate_pip_number(head, "scale", scale, 0.10, 0.60)?;
                validate_pip_number(head, "margin_pct", margin_pct, 0.0, 0.15)?;
                Ok(EdlOp::InsertPiP {
                    anchor,
                    asset,
                    duration_s,
                    source_start_s,
                    corner,
                    scale,
                    margin_pct,
                })
            }
            OpKind::MoveClip => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let at_s = take_field_f64(&mut fields, "at_s");
                let to_position = take_field_usize(&mut fields, "to_position")
                    .or_else(|| at_s.map(|_| 0))
                    .ok_or_else(|| EdlParseError::MissingField {
                        line: head,
                        field: "to_position or at_s".into(),
                    })?;
                Ok(EdlOp::MoveClip {
                    anchor,
                    to_position,
                    at_s,
                    snap: parse_snap_options(&mut fields, head)?,
                })
            }
            OpKind::RippleMove => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let delta_s = take_field_f64(&mut fields, "delta_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "delta_s".into(),
                    }
                })?;
                Ok(EdlOp::RippleMove {
                    anchor,
                    delta_s,
                    snap: parse_snap_options(&mut fields, head)?,
                })
            }
            OpKind::RippleDelete => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                Ok(EdlOp::RippleDelete { anchor })
            }
            OpKind::RippleTrim => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let edge_raw = take_field_string(&mut fields, "edge").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "edge".into(),
                    }
                })?;
                let edge = match edge_raw.as_str() {
                    "start" => crate::edl::op::RippleTrimEdge::Start,
                    "end" => crate::edl::op::RippleTrimEdge::End,
                    other => {
                        return Err(EdlParseError::BadField {
                            line: head,
                            raw: format!("+ edge: {other}"),
                            message: format!(
                                "ripple_trim: edge must be 'start' or 'end', got {other:?}"
                            ),
                        });
                    }
                };
                let value_s = take_field_f64(&mut fields, "value_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "value_s".into(),
                    }
                })?;
                Ok(EdlOp::RippleTrim {
                    anchor,
                    edge,
                    value_s,
                })
            }
            OpKind::DeleteGap => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let side_raw = take_field_string(&mut fields, "side").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "side".into(),
                    }
                })?;
                let side = match side_raw.as_str() {
                    "before" => crate::edl::op::GapSide::Before,
                    "after" => crate::edl::op::GapSide::After,
                    other => {
                        return Err(EdlParseError::BadField {
                            line: head,
                            raw: format!("+ side: {other}"),
                            message: format!(
                                "delete_gap: side must be 'before' or 'after', got {other:?}"
                            ),
                        });
                    }
                };
                Ok(EdlOp::DeleteGap { anchor, side })
            }
            OpKind::TrimTrackTail => {
                let track = take_field_string(&mut fields, "track").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "track".into(),
                    }
                })?;
                Ok(EdlOp::TrimTrackTail { track })
            }
            OpKind::InsertTrack => {
                let name = take_field_string(&mut fields, "name").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "name".into(),
                    }
                })?;
                let kind_raw = take_field_string(&mut fields, "kind").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "kind".into(),
                    }
                })?;
                let kind = parse_insert_track_kind(&kind_raw, head)?;
                let at_position = take_field_string(&mut fields, "at_position")
                    .map(|s| {
                        s.parse::<usize>().map_err(|e| EdlParseError::BadField {
                            line: head,
                            raw: format!("+ at_position: {s}"),
                            message: format!("at_position must be a non-negative integer: {e}"),
                        })
                    })
                    .transpose()?;
                Ok(EdlOp::InsertTrack {
                    name,
                    kind,
                    at_position,
                })
            }
            OpKind::DeleteTrack => {
                // Accept either `+ name:` or `+ track:` for symmetry
                // with TrimTrackTail (which uses `track`). Both work;
                // the agent has been observed using either.
                let name = take_field_string(&mut fields, "name")
                    .or_else(|| take_field_string(&mut fields, "track"))
                    .ok_or_else(|| EdlParseError::MissingField {
                        line: head,
                        field: "name (or track)".into(),
                    })?;
                // `force` defaults to false — protects against typos
                // that would silently nuke a populated track.
                let force = match take_field_string(&mut fields, "force").as_deref() {
                    Some("true") => true,
                    Some("false") | None => false,
                    Some(other) => {
                        return Err(EdlParseError::BadField {
                            line: head,
                            raw: format!("+ force: {other}"),
                            message: format!(
                                "delete_track: force must be 'true' or 'false', got {other:?}"
                            ),
                        });
                    }
                };
                Ok(EdlOp::DeleteTrack { name, force })
            }
            OpKind::ApplyMulticamPlan => Ok(EdlOp::ApplyMulticamPlan {
                plan: take_required_json::<MulticamApplyPlan>(&mut fields, "plan_json", head)?,
            }),
            OpKind::InsertTransition => {
                let between = self.between.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "between".into(),
                })?;
                let transition_id = take_field_string(&mut fields, "id");
                let kind = take_field_string(&mut fields, "kind")
                    .or_else(|| transition_id.clone())
                    .ok_or_else(|| EdlParseError::MissingField {
                        line: head,
                        field: "kind".into(),
                    })?;
                let duration_s = take_field_f64(&mut fields, "duration_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "duration_s".into(),
                    }
                })?;
                let alignment = take_field_string(&mut fields, "alignment")
                    .as_deref()
                    .map(|raw| parse_transition_alignment(raw, head))
                    .transpose()?;
                let in_offset_s = take_field_f64(&mut fields, "in_offset_s");
                let out_offset_s = take_field_f64(&mut fields, "out_offset_s");
                let family = take_field_string(&mut fields, "family");
                let intent = take_field_string(&mut fields, "intent");
                let energy = take_field_f64(&mut fields, "energy");
                let direction = take_field_string(&mut fields, "direction");
                let params = take_field_json_map(&mut fields, "params_json", head)?;
                let composition = take_field_json::<
                    awidat_proto::transitions::TransitionComposition,
                >(&mut fields, "composition_json", head)?;
                let has_semantic_fields = family.is_some()
                    || intent.is_some()
                    || energy.is_some()
                    || direction.is_some()
                    || !params.is_empty()
                    || composition.is_some();
                let semantic_id =
                    transition_id.or_else(|| has_semantic_fields.then(|| kind.clone()));
                let spec =
                    semantic_id.map(|id| awidat_proto::transitions::SemanticTransitionSpec {
                        id,
                        family,
                        intent,
                        energy,
                        direction,
                        params,
                        composition,
                    });
                Ok(EdlOp::InsertTransition {
                    between,
                    kind,
                    duration_s,
                    alignment,
                    in_offset_s,
                    out_offset_s,
                    spec,
                })
            }
            OpKind::DeleteTransition => {
                let between = self.between.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "between".into(),
                })?;
                Ok(EdlOp::DeleteTransition { between })
            }
            OpKind::SetCutIntent => {
                let between = self.between.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "between".into(),
                })?;
                let cut_type = take_field_string(&mut fields, "cut_type")
                    .as_deref()
                    .map(|raw| parse_cut_type(raw, head))
                    .transpose()?
                    .ok_or_else(|| EdlParseError::MissingField {
                        line: head,
                        field: "cut_type".into(),
                    })?;
                let intent = take_field_string(&mut fields, "intent").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "intent".into(),
                    }
                })?;
                let audio_relation = take_field_string(&mut fields, "audio_relation")
                    .as_deref()
                    .map(|raw| parse_audio_relation(raw, head))
                    .transpose()?
                    .unwrap_or_default();
                Ok(EdlOp::SetCutIntent {
                    between,
                    spec: SemanticCutSpec {
                        cut_type,
                        intent,
                        energy: take_field_f64(&mut fields, "energy"),
                        audio_relation,
                        confidence: take_field_f64(&mut fields, "confidence"),
                        reason: take_field_string(&mut fields, "reason"),
                        extra: Default::default(),
                    },
                })
            }
            OpKind::SetVolume => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let value = take_field_f64(&mut fields, "value").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "value".into(),
                    }
                })?;
                Ok(EdlOp::SetVolume { anchor, value })
            }
            OpKind::MuteClip => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                // `muted` defaults to true so `*** Mute Clip` with only an
                // anchor mutes; pass `+ muted: false` to unmute.
                let muted = take_field_bool(&mut fields, "muted").unwrap_or(true);
                Ok(EdlOp::MuteClip { anchor, muted })
            }
            OpKind::SetAudioFade => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                Ok(EdlOp::SetAudioFade {
                    anchor,
                    fade_in_s: take_field_f64(&mut fields, "fade_in_s"),
                    fade_out_s: take_field_f64(&mut fields, "fade_out_s"),
                })
            }
            OpKind::SetAudioLead => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let lead_s = take_field_f64(&mut fields, "lead_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "lead_s".into(),
                    }
                })?;
                let reason = take_field_string(&mut fields, "reason");
                let confidence = take_field_f64(&mut fields, "confidence");
                Ok(EdlOp::SetAudioLead {
                    anchor,
                    lead_s,
                    split_edit: Some(SplitEditSpec {
                        audio_lead_s: Some(lead_s),
                        audio_lead_from_clip_id: None,
                        audio_trail_s: None,
                        audio_trail_to_clip_id: None,
                        reason,
                        confidence,
                    }),
                })
            }
            OpKind::SetAudioTrail => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let trail_s = take_field_f64(&mut fields, "trail_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "trail_s".into(),
                    }
                })?;
                let reason = take_field_string(&mut fields, "reason");
                let confidence = take_field_f64(&mut fields, "confidence");
                Ok(EdlOp::SetAudioTrail {
                    anchor,
                    trail_s,
                    split_edit: Some(SplitEditSpec {
                        audio_lead_s: None,
                        audio_lead_from_clip_id: None,
                        audio_trail_s: Some(trail_s),
                        audio_trail_to_clip_id: None,
                        reason,
                        confidence,
                    }),
                })
            }
            OpKind::SetTrackAudio => {
                let track = take_field_string(&mut fields, "track").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "track".into(),
                    }
                })?;
                Ok(EdlOp::SetTrackAudio {
                    track,
                    role: take_field_string(&mut fields, "role"),
                    volume: take_field_f64(&mut fields, "volume"),
                    muted: take_field_bool(&mut fields, "muted"),
                    solo: take_field_bool(&mut fields, "solo"),
                })
            }
            OpKind::SetDucking => {
                let track = take_field_string(&mut fields, "track").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "track".into(),
                    }
                })?;
                Ok(EdlOp::SetDucking {
                    track,
                    enabled: take_field_bool(&mut fields, "enabled"),
                    amount_db: take_field_f64(&mut fields, "amount_db"),
                    attack_ms: take_field_f64(&mut fields, "attack_ms"),
                    release_ms: take_field_f64(&mut fields, "release_ms"),
                })
            }
            OpKind::SetSyncGroup => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let sync_group_id =
                    take_field_string(&mut fields, "sync_group_id").ok_or_else(|| {
                        EdlParseError::MissingField {
                            line: head,
                            field: "sync_group_id".into(),
                        }
                    })?;
                let offset_s = take_field_f64(&mut fields, "offset_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "offset_s".into(),
                    }
                })?;
                Ok(EdlOp::SetSyncGroup {
                    anchor,
                    sync_group_id,
                    offset_s,
                    speed_factor: take_field_f64(&mut fields, "speed_factor"),
                    confidence: take_field_f64(&mut fields, "confidence"),
                })
            }
            OpKind::SetClipAudioFx => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                Ok(EdlOp::SetClipAudioFx {
                    anchor,
                    fx: parse_audio_fx_config(&mut fields, head)?,
                })
            }
            OpKind::SetTrackAudioFx => {
                let track = take_field_string(&mut fields, "track").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "track".into(),
                    }
                })?;
                Ok(EdlOp::SetTrackAudioFx {
                    track,
                    fx: parse_audio_fx_config(&mut fields, head)?,
                })
            }
            OpKind::SetEffect => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let effect = take_field_string(&mut fields, "effect").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "effect".into(),
                    }
                })?;
                let params = take_field_json_map(&mut fields, "params_json", head)?;
                Ok(EdlOp::SetEffect {
                    anchor,
                    effect,
                    params,
                    rationale: take_field_string(&mut fields, "rationale"),
                })
            }
            OpKind::SetSpeed => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let factor = take_field_f64(&mut fields, "factor").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "factor".into(),
                    }
                })?;
                Ok(EdlOp::SetSpeed { anchor, factor })
            }
            OpKind::SetTimeRemap => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let curve =
                    take_required_json::<Vec<serde_json::Value>>(&mut fields, "curve_json", head)?;
                Ok(EdlOp::SetTimeRemap { anchor, curve })
            }
            OpKind::SetFreeze => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let freeze_at_source_s = take_field_f64(&mut fields, "freeze_at_source_s")
                    .ok_or_else(|| EdlParseError::MissingField {
                        line: head,
                        field: "freeze_at_source_s".into(),
                    })?;
                let duration_s = take_field_f64(&mut fields, "duration_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "duration_s".into(),
                    }
                })?;
                Ok(EdlOp::SetFreeze {
                    anchor,
                    freeze_at_source_s,
                    duration_s,
                    freeze_position: take_field_string(&mut fields, "freeze_position"),
                    audio_behavior: take_field_string(&mut fields, "audio_behavior"),
                })
            }
            OpKind::SetColorCorrection => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                Ok(EdlOp::SetColorCorrection {
                    anchor,
                    exposure_ev: take_field_f64(&mut fields, "exposure_ev"),
                    contrast: take_field_f64(&mut fields, "contrast"),
                    saturation: take_field_f64(&mut fields, "saturation"),
                    temperature: take_field_f64(&mut fields, "temperature"),
                    tint: take_field_f64(&mut fields, "tint"),
                    shadows: take_field_f64(&mut fields, "shadows"),
                    highlights: take_field_f64(&mut fields, "highlights"),
                })
            }
            OpKind::ApplyLut => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let lut_path = take_field_string(&mut fields, "lut_path").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "lut_path".into(),
                    }
                })?;
                let interpolation = take_field_string(&mut fields, "interpolation");
                let strength = take_field_f64(&mut fields, "strength");
                Ok(EdlOp::ApplyLut {
                    anchor,
                    lut_path,
                    interpolation,
                    strength,
                })
            }
            OpKind::RemoveLut => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                Ok(EdlOp::RemoveLut { anchor })
            }
            OpKind::InsertTitle => {
                let start_s = take_field_f64(&mut fields, "start_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "start_s".into(),
                    }
                })?;
                let end_s = take_field_f64(&mut fields, "end_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "end_s".into(),
                    }
                })?;
                let text = take_field_string(&mut fields, "text").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "text".into(),
                    }
                })?;
                let position = parse_title_position(
                    take_field_string(&mut fields, "position").as_deref(),
                    head,
                )?
                .unwrap_or(TitlePosition::Center);
                let font_size = take_field_usize(&mut fields, "font_size")
                    .map(|n| n as u32)
                    .unwrap_or(64);
                let color = take_field_string(&mut fields, "color")
                    .unwrap_or_else(|| "#FFFFFF".to_string());
                let font_weight = parse_title_weight(
                    take_field_string(&mut fields, "font_weight").as_deref(),
                    head,
                )?
                .unwrap_or(TitleWeight::Normal);
                let animation = parse_title_animation(
                    take_field_string(&mut fields, "animation").as_deref(),
                    head,
                )?
                .unwrap_or(TitleAnimation::None);
                let phases = take_field_json::<TitlePhases>(&mut fields, "phases_json", head)?;
                Ok(EdlOp::InsertTitle {
                    start_s,
                    end_s,
                    text,
                    position,
                    font_size,
                    color,
                    font_weight,
                    animation,
                    phases,
                })
            }
            OpKind::InsertRichTitle => {
                let start_s = take_field_f64(&mut fields, "start_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "start_s".into(),
                    }
                })?;
                let end_s = take_field_f64(&mut fields, "end_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "end_s".into(),
                    }
                })?;
                let segments =
                    take_required_json::<Vec<RichTextSegment>>(&mut fields, "segments_json", head)?;
                let position = parse_title_position(
                    take_field_string(&mut fields, "position").as_deref(),
                    head,
                )?
                .unwrap_or(TitlePosition::Center);
                let font_size = take_field_usize(&mut fields, "font_size")
                    .map(|n| n as u32)
                    .unwrap_or(64);
                let animation = parse_title_animation(
                    take_field_string(&mut fields, "animation").as_deref(),
                    head,
                )?
                .unwrap_or(TitleAnimation::None);
                let phases = take_field_json::<TitlePhases>(&mut fields, "phases_json", head)?;
                Ok(EdlOp::InsertRichTitle {
                    start_s,
                    end_s,
                    segments,
                    position,
                    font_size,
                    animation,
                    phases,
                })
            }
            OpKind::InstantiateMotionTemplate => {
                let template_id =
                    take_field_string(&mut fields, "template_id").ok_or_else(|| {
                        EdlParseError::MissingField {
                            line: head,
                            field: "template_id".into(),
                        }
                    })?;
                let start_s = take_field_f64(&mut fields, "start_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "start_s".into(),
                    }
                })?;
                let end_s = take_field_f64(&mut fields, "end_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "end_s".into(),
                    }
                })?;
                let animation = parse_motion_template_animation(
                    take_field_string(&mut fields, "animation").as_deref(),
                    head,
                )?
                .unwrap_or(MotionTemplateAnimation::None);
                Ok(EdlOp::InstantiateMotionTemplate {
                    template_id,
                    start_s,
                    end_s,
                    animation,
                    slot_values: take_required_json(&mut fields, "slot_values_json", head)?,
                })
            }
            OpKind::SetTitle => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let start_s = take_field_f64(&mut fields, "start_s");
                let end_s = take_field_f64(&mut fields, "end_s");
                let text = take_field_string(&mut fields, "text");
                let position = parse_title_position(
                    take_field_string(&mut fields, "position").as_deref(),
                    head,
                )?;
                let font_size = take_field_usize(&mut fields, "font_size").map(|n| n as u32);
                let color = take_field_string(&mut fields, "color");
                let font_weight = parse_title_weight(
                    take_field_string(&mut fields, "font_weight").as_deref(),
                    head,
                )?;
                let animation = parse_title_animation(
                    take_field_string(&mut fields, "animation").as_deref(),
                    head,
                )?;
                let phases = take_field_json::<TitlePhases>(&mut fields, "phases_json", head)?;
                Ok(EdlOp::SetTitle {
                    anchor,
                    start_s,
                    end_s,
                    text,
                    position,
                    font_size,
                    color,
                    font_weight,
                    animation,
                    phases,
                })
            }
            OpKind::InsertCaption => {
                let start_s = take_field_f64(&mut fields, "start_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "start_s".into(),
                    }
                })?;
                let end_s = take_field_f64(&mut fields, "end_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "end_s".into(),
                    }
                })?;
                let text = take_field_string(&mut fields, "text").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "text".into(),
                    }
                })?;
                let position = parse_title_position(
                    take_field_string(&mut fields, "position").as_deref(),
                    head,
                )?
                .unwrap_or(TitlePosition::Bottom);
                let font_size = take_field_usize(&mut fields, "font_size")
                    .map(|n| n as u32)
                    .unwrap_or(52);
                let color = take_field_string(&mut fields, "color")
                    .unwrap_or_else(|| "#FFFFFF".to_string());
                let safe_area =
                    take_field_string(&mut fields, "safe_area").unwrap_or_else(|| "mobile".into());
                let word_timings = take_field_json::<Vec<CaptionWordTiming>>(
                    &mut fields,
                    "word_timings_json",
                    head,
                )?
                .unwrap_or_default();
                Ok(EdlOp::InsertCaption {
                    start_s,
                    end_s,
                    text,
                    position,
                    font_size,
                    color,
                    safe_area,
                    word_timings,
                })
            }
            OpKind::InsertAnnotation => {
                let start_s = take_field_f64(&mut fields, "start_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "start_s".into(),
                    }
                })?;
                let end_s = take_field_f64(&mut fields, "end_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "end_s".into(),
                    }
                })?;
                let kind = parse_annotation_kind(
                    take_field_string(&mut fields, "kind")
                        .as_deref()
                        .ok_or_else(|| EdlParseError::MissingField {
                            line: head,
                            field: "kind".into(),
                        })?,
                    head,
                )?;
                let x = take_field_f64(&mut fields, "x").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "x".into(),
                    }
                })?;
                let y = take_field_f64(&mut fields, "y").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "y".into(),
                    }
                })?;
                let width = take_field_f64(&mut fields, "width").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "width".into(),
                    }
                })?;
                let height = take_field_f64(&mut fields, "height").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "height".into(),
                    }
                })?;
                let color = parse_annotation_color(
                    take_field_string(&mut fields, "color")
                        .unwrap_or_else(|| "#FFCC00".into())
                        .as_str(),
                    head,
                )?;
                let stroke_width = take_field_usize(&mut fields, "stroke_width")
                    .map(|n| n as u32)
                    .unwrap_or(4);
                let label = take_field_string(&mut fields, "label");
                Ok(EdlOp::InsertAnnotation {
                    start_s,
                    end_s,
                    kind,
                    x,
                    y,
                    width,
                    height,
                    color,
                    stroke_width,
                    label,
                })
            }
            OpKind::SetOutputFormat => {
                let aspect_ratio =
                    take_field_string(&mut fields, "aspect_ratio").ok_or_else(|| {
                        EdlParseError::MissingField {
                            line: head,
                            field: "aspect_ratio".into(),
                        }
                    })?;
                Ok(EdlOp::SetOutputFormat {
                    aspect_ratio,
                    platform: take_field_string(&mut fields, "platform"),
                    safe_area: take_field_string(&mut fields, "safe_area"),
                })
            }
            OpKind::SetLoudnessTarget => {
                let integrated_lufs =
                    take_field_f64(&mut fields, "integrated_lufs").ok_or_else(|| {
                        EdlParseError::MissingField {
                            line: head,
                            field: "integrated_lufs".into(),
                        }
                    })?;
                Ok(EdlOp::SetLoudnessTarget {
                    integrated_lufs,
                    true_peak_db: take_field_f64(&mut fields, "true_peak_db"),
                })
            }
            OpKind::SetPackageMetadata => Ok(EdlOp::SetPackageMetadata {
                platform: take_field_string(&mut fields, "platform"),
                title: take_field_string(&mut fields, "title"),
                description: take_field_string(&mut fields, "description"),
                tags: take_field_string(&mut fields, "tags"),
            }),
            OpKind::SetBroadcastOverlay => {
                let config = parse_broadcast_overlay_config(&mut fields, head)?;
                Ok(EdlOp::SetBroadcastOverlay { config })
            }
            OpKind::SetAssetCatalog => Ok(EdlOp::SetAssetCatalog {
                catalog: take_required_json(&mut fields, "catalog_json", head)?,
            }),
            OpKind::SetSourceReview => Ok(EdlOp::SetSourceReview {
                selects: take_required_json::<Vec<SourceSelect>>(
                    &mut fields,
                    "selects_json",
                    head,
                )?,
                stringouts: take_field_json::<Vec<Stringout>>(
                    &mut fields,
                    "stringouts_json",
                    head,
                )?
                .unwrap_or_default(),
            }),
            OpKind::ProfessionalTimelineEdit => Ok(EdlOp::ProfessionalTimelineEdit {
                edit: take_required_json::<ProfessionalTimelineEdit>(
                    &mut fields,
                    "edit_json",
                    head,
                )?,
            }),
            OpKind::AddProposalPackage => Ok(EdlOp::AddProposalPackage {
                package: take_required_json::<ProposalPackage>(&mut fields, "package_json", head)?,
            }),
            OpKind::SetParameterAnimation => Ok(EdlOp::SetParameterAnimation {
                animation: take_required_json::<ParameterAnimation>(
                    &mut fields,
                    "animation_json",
                    head,
                )?,
            }),
            OpKind::SetMotionTemplate => Ok(EdlOp::SetMotionTemplate {
                template: take_required_json::<MotionGraphicsTemplate>(
                    &mut fields,
                    "template_json",
                    head,
                )?,
            }),
            OpKind::SetMotionScene => Ok(EdlOp::SetMotionScene {
                scene: take_required_json::<MotionScene>(&mut fields, "scene_json", head)?,
            }),
            OpKind::AttachComposition => Ok(EdlOp::AttachComposition {
                graph: take_required_json::<CompositionGraph>(&mut fields, "graph_json", head)?,
                attach_to: take_field_json::<SourceRange>(&mut fields, "attach_to_json", head)?,
            }),
            OpKind::SetTrackingPackage => Ok(EdlOp::SetTrackingPackage {
                package: take_required_json::<TrackingPackage>(&mut fields, "package_json", head)?,
            }),
            OpKind::AuthorSubjectReframeFromTrack => {
                let source_width =
                    take_field_u32(&mut fields, "source_width").ok_or_else(|| {
                        EdlParseError::MissingField {
                            line: head,
                            field: "source_width".into(),
                        }
                    })?;
                let source_height =
                    take_field_u32(&mut fields, "source_height").ok_or_else(|| {
                        EdlParseError::MissingField {
                            line: head,
                            field: "source_height".into(),
                        }
                    })?;
                let target_width =
                    take_field_u32(&mut fields, "target_width").ok_or_else(|| {
                        EdlParseError::MissingField {
                            line: head,
                            field: "target_width".into(),
                        }
                    })?;
                let target_height =
                    take_field_u32(&mut fields, "target_height").ok_or_else(|| {
                        EdlParseError::MissingField {
                            line: head,
                            field: "target_height".into(),
                        }
                    })?;
                let frame_rate = take_field_f64(&mut fields, "frame_rate").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "frame_rate".into(),
                    }
                })?;
                Ok(EdlOp::AuthorSubjectReframeFromTrack {
                    track_id: take_field_string(&mut fields, "track_id").ok_or_else(|| {
                        EdlParseError::MissingField {
                            line: head,
                            field: "track_id".into(),
                        }
                    })?,
                    clip_id: take_field_string(&mut fields, "clip_id").ok_or_else(|| {
                        EdlParseError::MissingField {
                            line: head,
                            field: "clip_id".into(),
                        }
                    })?,
                    aspect_ratio: take_field_string(&mut fields, "aspect_ratio").ok_or_else(
                        || EdlParseError::MissingField {
                            line: head,
                            field: "aspect_ratio".into(),
                        },
                    )?,
                    source_width,
                    source_height,
                    target_width,
                    target_height,
                    frame_rate,
                    smoothing: take_field_string(&mut fields, "smoothing")
                        .as_deref()
                        .map(parse_reframe_smoothing)
                        .transpose()
                        .map_err(|message| EdlParseError::BadField {
                            line: head,
                            raw: "smoothing".into(),
                            message,
                        })?
                        .unwrap_or_default(),
                    safe_area: take_field_string(&mut fields, "safe_area"),
                })
            }
            OpKind::SetColorFinishing => Ok(EdlOp::SetColorFinishing {
                state: take_required_json::<ColorFinishingState>(&mut fields, "state_json", head)?,
            }),
            OpKind::SetAudioFinishing => Ok(EdlOp::SetAudioFinishing {
                state: take_required_json::<AudioFinishingState>(&mut fields, "state_json", head)?,
            }),
            OpKind::SelectDeliveryProfile => Ok(EdlOp::SelectDeliveryProfile {
                profile: take_required_json::<DeliveryProfile>(&mut fields, "profile_json", head)?,
            }),
            OpKind::AddPreflightReport => Ok(EdlOp::AddPreflightReport {
                report: take_required_json::<PreflightReport>(&mut fields, "report_json", head)?,
            }),
            OpKind::SetWorkflowLens => Ok(EdlOp::SetWorkflowLens {
                lens: parse_workflow_lens(
                    take_field_string(&mut fields, "lens")
                        .as_deref()
                        .ok_or_else(|| EdlParseError::MissingField {
                            line: head,
                            field: "lens".into(),
                        })?,
                    head,
                )?,
            }),
            OpKind::SetPipelineReadiness => Ok(EdlOp::SetPipelineReadiness {
                readiness: take_required_json::<PipelineReadinessReport>(
                    &mut fields,
                    "readiness_json",
                    head,
                )?,
                planner_passes: take_field_json::<Vec<PlannerPassContract>>(
                    &mut fields,
                    "planner_passes_json",
                    head,
                )?
                .unwrap_or_default(),
            }),
        }
    }
}

fn parse_broadcast_overlay_config(
    fields: &mut Vec<(String, FieldValue)>,
    line: usize,
) -> Result<BroadcastOverlayConfig, EdlParseError> {
    if let Some(raw) = take_field_string(fields, "config_json") {
        return serde_json::from_str::<BroadcastOverlayConfig>(&raw).map_err(|e| {
            EdlParseError::BadField {
                line,
                raw: "config_json".into(),
                message: format!("must be valid broadcast overlay JSON: {e}"),
            }
        });
    }

    let mut config = BroadcastOverlayConfig::default();
    if let Some(enabled) = take_field_string(fields, "enabled") {
        config.enabled = parse_bool_field(&enabled, line, "enabled")?;
    }
    config.template_name = take_field_string(fields, "template_name");
    if let Some(v) = take_field_string(fields, "episode_title") {
        config.episode_title = v;
    }
    if let Some(v) = take_field_string(fields, "episode_subtitle") {
        config.episode_subtitle = v;
    }
    if let Some(v) = take_field_string(fields, "show_name") {
        config.show_name = v;
    }
    if let Some(v) = take_field_string(fields, "host_a") {
        config.host_a = parse_json_value(&v, line, "host_a")?;
    }
    if let Some(v) = take_field_string(fields, "host_b") {
        config.host_b = parse_json_value(&v, line, "host_b")?;
    }
    if let Some(v) = take_field_string(fields, "sponsors") {
        config.sponsors = parse_json_value(&v, line, "sponsors")?;
    }
    if let Some(v) = take_field_string(fields, "topics") {
        config.topics = parse_json_value::<Vec<BroadcastTimedEntry>>(&v, line, "topics")?;
    }
    if let Some(v) = take_field_string(fields, "chapters") {
        config.chapters = parse_json_value::<Vec<BroadcastTimedEntry>>(&v, line, "chapters")?;
    }
    config.brand_logo_path = take_field_string(fields, "brand_logo_path");
    if let Some(v) = take_field_string(fields, "short_form_mode") {
        config.short_form_mode = parse_bool_field(&v, line, "short_form_mode")?;
    }
    if let Some(v) = take_field_string(fields, "style") {
        config.style = parse_json_value::<BroadcastOverlayStyle>(&v, line, "style")?;
    }
    // Convenience fields for simple EDLs that do not want to JSON-encode hosts.
    if let Some(v) = take_field_string(fields, "host_a_name") {
        config.host_a.name = v;
    }
    if let Some(v) = take_field_string(fields, "host_a_title") {
        config.host_a.title = v;
    }
    if let Some(v) = take_field_string(fields, "host_a_photo_path") {
        config.host_a.photo_path = Some(v);
    }
    if let Some(v) = take_field_string(fields, "host_b_name") {
        config.host_b.name = v;
    }
    if let Some(v) = take_field_string(fields, "host_b_title") {
        config.host_b.title = v;
    }
    if let Some(v) = take_field_string(fields, "host_b_photo_path") {
        config.host_b.photo_path = Some(v);
    }
    Ok(config)
}

fn parse_audio_fx_config(
    fields: &mut Vec<(String, FieldValue)>,
    line: usize,
) -> Result<AudioFxConfig, EdlParseError> {
    let mut fx = AudioFxConfig {
        high_pass_hz: take_field_f64(fields, "high_pass_hz"),
        low_pass_hz: take_field_f64(fields, "low_pass_hz"),
        pan: take_field_f64(fields, "pan"),
        balance: take_field_f64(fields, "balance"),
        adeclick: take_field_bool(fields, "adeclick"),
        adeclip: take_field_bool(fields, "adeclip"),
        arnndn_model: take_field_string(fields, "arnndn_model"),
        eq_bands: Vec::new(),
        compressor_threshold_db: take_field_f64(fields, "compressor_threshold_db"),
        compressor_ratio: take_field_f64(fields, "compressor_ratio"),
        limiter_limit_db: take_field_f64(fields, "limiter_limit_db"),
        noise_gate_threshold_db: take_field_f64(fields, "noise_gate_threshold_db"),
        hum_notch_hz: take_field_f64(fields, "hum_notch_hz"),
        de_ess_hz: take_field_f64(fields, "de_ess_hz"),
        de_ess_reduction_db: take_field_f64(fields, "de_ess_reduction_db"),
        loudnorm_i: take_field_f64(fields, "loudnorm_i"),
        loudnorm_tp: take_field_f64(fields, "loudnorm_tp"),
    };
    if let Some(raw) = take_field_string(fields, "eq_bands_json") {
        fx.eq_bands =
            serde_json::from_str::<Vec<EqBand>>(&raw).map_err(|e| EdlParseError::BadField {
                line,
                raw: format!("eq_bands_json: {raw}"),
                message: format!("must be valid JSON array of EQ bands: {e}"),
            })?;
    } else if let Some(freq_hz) = take_field_f64(fields, "eq_freq_hz") {
        fx.eq_bands.push(EqBand {
            freq_hz,
            gain_db: take_field_f64(fields, "eq_gain_db").unwrap_or(0.0),
            width_hz: take_field_f64(fields, "eq_width_hz"),
        });
    }
    Ok(fx)
}

fn parse_json_value<T: serde::de::DeserializeOwned>(
    raw: &str,
    line: usize,
    field: &str,
) -> Result<T, EdlParseError> {
    serde_json::from_str(raw).map_err(|e| EdlParseError::BadField {
        line,
        raw: field.into(),
        message: format!("must be valid JSON: {e}"),
    })
}

fn parse_bool_field(raw: &str, line: usize, field: &str) -> Result<bool, EdlParseError> {
    match raw {
        "true" | "yes" | "1" => Ok(true),
        "false" | "no" | "0" => Ok(false),
        other => Err(EdlParseError::BadField {
            line,
            raw: format!("{field}: {other}"),
            message: "must be true or false".into(),
        }),
    }
}

fn parse_title_position(
    raw: Option<&str>,
    line: usize,
) -> Result<Option<TitlePosition>, EdlParseError> {
    match raw {
        None => Ok(None),
        Some("top") => Ok(Some(TitlePosition::Top)),
        Some("center") => Ok(Some(TitlePosition::Center)),
        Some("bottom") => Ok(Some(TitlePosition::Bottom)),
        Some(other) => Err(EdlParseError::BadField {
            line,
            raw: format!("position: {other}"),
            message: "must be 'top', 'center', or 'bottom'".into(),
        }),
    }
}

fn parse_title_weight(
    raw: Option<&str>,
    line: usize,
) -> Result<Option<TitleWeight>, EdlParseError> {
    match raw {
        None => Ok(None),
        Some("normal") => Ok(Some(TitleWeight::Normal)),
        Some("bold") => Ok(Some(TitleWeight::Bold)),
        Some(other) => Err(EdlParseError::BadField {
            line,
            raw: format!("font_weight: {other}"),
            message: "must be 'normal' or 'bold'".into(),
        }),
    }
}

fn parse_title_animation(
    raw: Option<&str>,
    line: usize,
) -> Result<Option<TitleAnimation>, EdlParseError> {
    match raw {
        None => Ok(None),
        Some("none") => Ok(Some(TitleAnimation::None)),
        Some("fade_in") => Ok(Some(TitleAnimation::FadeIn)),
        Some("fade_out") => Ok(Some(TitleAnimation::FadeOut)),
        Some("fade_in_out") => Ok(Some(TitleAnimation::FadeInOut)),
        Some("slide_in") => Ok(Some(TitleAnimation::SlideIn)),
        Some("slide_out") => Ok(Some(TitleAnimation::SlideOut)),
        Some(other) => Err(EdlParseError::BadField {
            line,
            raw: format!("animation: {other}"),
            message:
                "must be 'none', 'fade_in', 'fade_out', 'fade_in_out', 'slide_in', or 'slide_out'"
                    .into(),
        }),
    }
}

fn parse_motion_template_animation(
    raw: Option<&str>,
    line: usize,
) -> Result<Option<MotionTemplateAnimation>, EdlParseError> {
    match raw {
        None => Ok(None),
        Some("none") => Ok(Some(MotionTemplateAnimation::None)),
        Some("opacity") => Ok(Some(MotionTemplateAnimation::Opacity)),
        Some("transform") => Ok(Some(MotionTemplateAnimation::Transform)),
        Some("text_reveal") => Ok(Some(MotionTemplateAnimation::TextReveal)),
        Some("write_on") => Ok(Some(MotionTemplateAnimation::WriteOn)),
        Some(other) => Err(EdlParseError::BadField {
            line,
            raw: format!("animation: {other}"),
            message: "must be 'none', 'opacity', 'transform', 'text_reveal', or 'write_on'".into(),
        }),
    }
}

fn parse_snap_options(
    fields: &mut Vec<(String, FieldValue)>,
    line: usize,
) -> Result<Option<SnapOptions>, EdlParseError> {
    let enabled = take_field_bool(fields, "snap");
    let tolerance_s = take_field_f64(fields, "snap_tolerance_s");
    let targets = take_field_string(fields, "snap_targets")
        .map(|raw| parse_snap_targets(&raw, line))
        .transpose()?;
    if enabled.is_none() && tolerance_s.is_none() && targets.is_none() {
        return Ok(None);
    }
    let tolerance_s = tolerance_s.unwrap_or(0.1);
    if !tolerance_s.is_finite() || tolerance_s < 0.0 {
        return Err(EdlParseError::BadField {
            line,
            raw: format!("snap_tolerance_s: {tolerance_s}"),
            message: "must be finite and >= 0".into(),
        });
    }
    Ok(Some(SnapOptions {
        enabled: enabled.unwrap_or(true),
        tolerance_s,
        targets: targets.unwrap_or_default(),
    }))
}

fn parse_snap_targets(raw: &str, line: usize) -> Result<Vec<SnapTargetKind>, EdlParseError> {
    let mut targets = Vec::new();
    for part in raw.split(',') {
        let target = match part.trim() {
            "" => continue,
            "clip_edge" | "clip_edges" => SnapTargetKind::ClipEdge,
            "marker" | "markers" => SnapTargetKind::Marker,
            "playhead" => SnapTargetKind::Playhead,
            other => {
                return Err(EdlParseError::BadField {
                    line,
                    raw: format!("snap_targets: {other}"),
                    message:
                        "must contain comma-separated values from: clip_edge, marker, playhead"
                            .into(),
                });
            }
        };
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    Ok(targets)
}

fn parse_annotation_kind(raw: &str, line: usize) -> Result<AnnotationKind, EdlParseError> {
    match raw {
        "rectangle" => Ok(AnnotationKind::Rectangle),
        "circle" => Ok(AnnotationKind::Circle),
        "arrow" => Ok(AnnotationKind::Arrow),
        "bracket" => Ok(AnnotationKind::Bracket),
        "blur" => Ok(AnnotationKind::Blur),
        other => Err(EdlParseError::BadField {
            line,
            raw: format!("kind: {other}"),
            message: "must be 'rectangle', 'circle', 'arrow', 'bracket', or 'blur'".into(),
        }),
    }
}

fn parse_annotation_color(raw: &str, line: usize) -> Result<String, EdlParseError> {
    if valid_graphic_color(raw) {
        return Ok(raw.to_string());
    }
    Err(EdlParseError::BadField {
        line,
        raw: format!("color: {raw}"),
        message:
            "annotation color must be #RGB/#RGBA/#RRGGBB/#RRGGBBAA hex or an alphanumeric color name"
                .into(),
    })
}

fn parse_transition_alignment(
    raw: &str,
    line: usize,
) -> Result<TransitionAlignment, EdlParseError> {
    match raw {
        "center" => Ok(TransitionAlignment::Center),
        "start_at_cut" => Ok(TransitionAlignment::StartAtCut),
        "end_at_cut" => Ok(TransitionAlignment::EndAtCut),
        other => Err(EdlParseError::BadField {
            line,
            raw: format!("alignment: {other}"),
            message: "must be 'center', 'start_at_cut', or 'end_at_cut'".into(),
        }),
    }
}

fn parse_cut_type(raw: &str, line: usize) -> Result<CutType, EdlParseError> {
    match raw {
        "hard_cut" => Ok(CutType::HardCut),
        "cut_on_action" => Ok(CutType::CutOnAction),
        "cutaway" => Ok(CutType::Cutaway),
        "insert" => Ok(CutType::Insert),
        "eyeline_match" => Ok(CutType::EyelineMatch),
        "shot_reverse_shot" => Ok(CutType::ShotReverseShot),
        "match_cut" => Ok(CutType::MatchCut),
        "smash_cut" => Ok(CutType::SmashCut),
        "jump_cut" => Ok(CutType::JumpCut),
        "cross_cut" => Ok(CutType::CrossCut),
        "j_cut" => Ok(CutType::JCut),
        "l_cut" => Ok(CutType::LCut),
        other => Err(EdlParseError::BadField {
            line,
            raw: format!("cut_type: {other}"),
            message: "must be one of: hard_cut, cut_on_action, cutaway, insert, eyeline_match, shot_reverse_shot, match_cut, smash_cut, jump_cut, cross_cut, j_cut, l_cut".into(),
        }),
    }
}

fn parse_audio_relation(raw: &str, line: usize) -> Result<AudioRelation, EdlParseError> {
    match raw {
        "sync" => Ok(AudioRelation::Sync),
        "audio_leads" => Ok(AudioRelation::AudioLeads),
        "audio_trails" => Ok(AudioRelation::AudioTrails),
        "overlap" => Ok(AudioRelation::Overlap),
        "audio_cut" => Ok(AudioRelation::AudioCut),
        other => Err(EdlParseError::BadField {
            line,
            raw: format!("audio_relation: {other}"),
            message: "must be one of: sync, audio_leads, audio_trails, overlap, audio_cut".into(),
        }),
    }
}

fn parse_pip_corner(raw: &str, line: usize) -> Result<PiPCorner, EdlParseError> {
    match raw {
        "top_left" => Ok(PiPCorner::TopLeft),
        "top_right" => Ok(PiPCorner::TopRight),
        "bottom_left" => Ok(PiPCorner::BottomLeft),
        "bottom_right" => Ok(PiPCorner::BottomRight),
        other => Err(EdlParseError::BadField {
            line,
            raw: format!("corner: {other}"),
            message: "must be 'top_left', 'top_right', 'bottom_left', or 'bottom_right'".into(),
        }),
    }
}

fn validate_pip_number(
    line: usize,
    field: &str,
    value: f64,
    min: f64,
    max: f64,
) -> Result<(), EdlParseError> {
    if value.is_finite() && (min..=max).contains(&value) {
        return Ok(());
    }
    Err(EdlParseError::BadField {
        line,
        raw: format!("{field}: {value}"),
        message: format!("must be finite and in range {min}..={max}"),
    })
}

/// One parsed `+ key: value` or `- key: value` field. We tag the value
/// type so the OpBuilder can pull strongly-typed coercions back out.
#[derive(Debug, Clone)]
enum FieldValue {
    String(String),
    Number(f64),
}

fn parse_field(
    rest: &str,
    line: usize,
    full_line: &str,
) -> Result<(String, FieldValue), EdlParseError> {
    let (key, raw_value) = rest
        .split_once(':')
        .ok_or_else(|| EdlParseError::BadField {
            line,
            raw: full_line.to_string(),
            message: "expected `key: value`".into(),
        })?;
    let key = key.trim().to_string();
    let raw = raw_value.trim();
    if let Ok(n) = raw.parse::<f64>() {
        return Ok((key, FieldValue::Number(n)));
    }
    // Strip optional surrounding quotes for string values.
    let s = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(raw);
    Ok((key, FieldValue::String(s.to_string())))
}

fn take_field_f64(fields: &mut Vec<(String, FieldValue)>, key: &str) -> Option<f64> {
    let pos = fields.iter().rposition(|(k, _)| k == key)?;
    let (_, v) = fields.remove(pos);
    match v {
        FieldValue::Number(n) => Some(n),
        FieldValue::String(s) => s.parse::<f64>().ok(),
    }
}

fn take_field_usize(fields: &mut Vec<(String, FieldValue)>, key: &str) -> Option<usize> {
    let pos = fields.iter().rposition(|(k, _)| k == key)?;
    let (_, v) = fields.remove(pos);
    match v {
        FieldValue::Number(n) if n >= 0.0 && n.fract() == 0.0 => Some(n as usize),
        FieldValue::String(s) => s.parse::<usize>().ok(),
        _ => None,
    }
}

fn take_field_u32(fields: &mut Vec<(String, FieldValue)>, key: &str) -> Option<u32> {
    let value = take_field_usize(fields, key)?;
    u32::try_from(value).ok()
}

fn take_field_string(fields: &mut Vec<(String, FieldValue)>, key: &str) -> Option<String> {
    let pos = fields.iter().rposition(|(k, _)| k == key)?;
    let (_, v) = fields.remove(pos);
    Some(match v {
        FieldValue::String(s) => s,
        FieldValue::Number(n) => n.to_string(),
    })
}

fn take_field_bool(fields: &mut Vec<(String, FieldValue)>, key: &str) -> Option<bool> {
    let pos = fields.iter().rposition(|(k, _)| k == key)?;
    let (_, v) = fields.remove(pos);
    match v {
        FieldValue::String(s) => match s.to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        },
        FieldValue::Number(n) if (n - 1.0).abs() < f64::EPSILON => Some(true),
        FieldValue::Number(n) if n.abs() < f64::EPSILON => Some(false),
        FieldValue::Number(_) => None,
    }
}

fn parse_reframe_smoothing(raw: &str) -> Result<ReframeSmoothing, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(ReframeSmoothing::None),
        "gentle" => Ok(ReframeSmoothing::Gentle),
        "moderate" => Ok(ReframeSmoothing::Moderate),
        "strong" => Ok(ReframeSmoothing::Strong),
        other => Err(format!(
            "unknown reframe smoothing {other:?}; expected none, gentle, moderate, or strong"
        )),
    }
}

fn take_field_json_map(
    fields: &mut Vec<(String, FieldValue)>,
    key: &str,
    line: usize,
) -> Result<serde_json::Map<String, serde_json::Value>, EdlParseError> {
    let Some(raw) = take_field_string(fields, key) else {
        return Ok(serde_json::Map::new());
    };
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| EdlParseError::BadField {
            line,
            raw: format!("{key}: {raw}"),
            message: format!("must be a JSON object: {e}"),
        })?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| EdlParseError::BadField {
            line,
            raw: format!("{key}: {raw}"),
            message: "must be a JSON object".into(),
        })
}

fn take_field_json<T: serde::de::DeserializeOwned>(
    fields: &mut Vec<(String, FieldValue)>,
    key: &str,
    line: usize,
) -> Result<Option<T>, EdlParseError> {
    let Some(raw) = take_field_string(fields, key) else {
        return Ok(None);
    };
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|e| EdlParseError::BadField {
            line,
            raw: format!("{key}: {raw}"),
            message: format!("must be valid JSON for {key}: {e}"),
        })
}

fn take_required_json<T: serde::de::DeserializeOwned>(
    fields: &mut Vec<(String, FieldValue)>,
    key: &str,
    line: usize,
) -> Result<T, EdlParseError> {
    take_field_json(fields, key, line)?.ok_or_else(|| EdlParseError::MissingField {
        line,
        field: key.into(),
    })
}

fn parse_workflow_lens(raw: &str, line: usize) -> Result<WorkflowLens, EdlParseError> {
    match raw {
        "media" => Ok(WorkflowLens::Media),
        "selects" => Ok(WorkflowLens::Selects),
        "assembly" => Ok(WorkflowLens::Assembly),
        "edit_review" => Ok(WorkflowLens::EditReview),
        "vfx" => Ok(WorkflowLens::Vfx),
        "color" => Ok(WorkflowLens::Color),
        "audio" => Ok(WorkflowLens::Audio),
        "delivery" => Ok(WorkflowLens::Delivery),
        "preflight" => Ok(WorkflowLens::Preflight),
        other => Err(EdlParseError::BadField {
            line,
            raw: format!("lens: {other}"),
            message: "must be one of: media, selects, assembly, edit_review, vfx, color, audio, delivery, preflight".into(),
        }),
    }
}

fn parse_insert_track_kind(raw: &str, line: usize) -> Result<InsertTrackKind, EdlParseError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "video" => Ok(InsertTrackKind::Video),
        "audio" => Ok(InsertTrackKind::Audio),
        "auto" => Ok(InsertTrackKind::Auto),
        other => Err(EdlParseError::BadField {
            line,
            raw: format!("track_kind: {other}"),
            message: "must be 'video', 'audio', or 'auto'".into(),
        }),
    }
}

/// Parse `transcript_snippet="..."` / `clip_uuid=...` / `scene_change_index=ASSET:N`.
///
/// Prefers `key=value`. Tolerates `key: value` (with the space) as
/// a fallback because agents sometimes drift to the colon syntax —
/// the space disambiguates from `scene_change_index`'s `ASSET:N`
/// value, which never has a space after the colon.
fn parse_anchor(rest: &str, line: usize) -> Result<Anchor, EdlParseError> {
    let (k, v) = if let Some((k, v)) = rest.split_once('=') {
        (k, v)
    } else if let Some((k, v)) = rest.split_once(": ") {
        (k, v)
    } else {
        return Err(EdlParseError::BadAnchor {
            line,
            message: format!("expected key=value, got {rest:?}"),
        });
    };
    let k = k.trim();
    let v = v.trim();
    match k {
        "transcript_snippet" => {
            let stripped = v.strip_prefix('"').and_then(|s| s.strip_suffix('"'));
            let text = stripped.unwrap_or(v).to_string();
            if text.is_empty() {
                return Err(EdlParseError::BadAnchor {
                    line,
                    message: "transcript_snippet must be non-empty".into(),
                });
            }
            Ok(Anchor::TranscriptSnippet { text })
        }
        "clip_uuid" => {
            if v.is_empty() {
                return Err(EdlParseError::BadAnchor {
                    line,
                    message: "clip_uuid must be non-empty".into(),
                });
            }
            Ok(Anchor::ClipUuid {
                uuid: v.to_string(),
            })
        }
        "scene_change_index" => {
            let (asset, idx) = v.rsplit_once(':').ok_or_else(|| EdlParseError::BadAnchor {
                line,
                message: "scene_change_index expects ASSET:INDEX".into(),
            })?;
            let index: u32 = idx.parse().map_err(|_| EdlParseError::BadAnchor {
                line,
                message: format!("scene_change_index: '{idx}' is not a number"),
            })?;
            Ok(Anchor::SceneChangeIndex {
                asset_id: asset.to_string(),
                index,
            })
        }
        other => Err(EdlParseError::BadAnchor {
            line,
            message: format!("unknown anchor kind '{other}'"),
        }),
    }
}

/// Parse a `between: ANCHOR_A and ANCHOR_B` for InsertTransition.
fn parse_between(rest: &str, line: usize) -> Result<TransitionBetween, EdlParseError> {
    let (a, b) = rest
        .split_once(" and ")
        .ok_or_else(|| EdlParseError::BadAnchor {
            line,
            message: "between: expects two anchors separated by ' and '".into(),
        })?;
    Ok(TransitionBetween {
        from: parse_anchor(a.trim(), line)?,
        to: parse_anchor(b.trim(), line)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_trim_envelope() {
        let text = "\
*** Begin EDL
*** Trim Clip
@@ anchor: transcript_snippet=\"hello world\"
- end: 80.4
+ end: 78.9
*** End EDL
";
        let env = parse(text).unwrap();
        assert_eq!(env.len(), 1);
        match &env.ops[0] {
            EdlOp::TrimClip { anchor, start, end } => {
                assert!(
                    matches!(anchor, Anchor::TranscriptSnippet { text } if text == "hello world")
                );
                assert_eq!(*start, None);
                assert_eq!(*end, Some(78.9));
            }
            other => panic!("want TrimClip, got {other:?}"),
        }
    }

    #[test]
    fn tolerates_colon_anchor_syntax() {
        // Agents sometimes emit "key: value" instead of canonical
        // "key=value". The parser accepts both. ": " (space-after-
        // colon) disambiguates from scene_change_index's
        // "ASSET:INDEX" value, which has no space.
        let text = "\
*** Begin EDL
*** Delete Clip
@@ anchor: clip_uuid: clip-0
*** End EDL
";
        let env = parse(text).unwrap();
        assert_eq!(env.len(), 1);
        match &env.ops[0] {
            EdlOp::DeleteClip { anchor } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-0"));
            }
            other => panic!("want DeleteClip, got {other:?}"),
        }
    }

    #[test]
    fn parses_author_subject_reframe_from_track() {
        let text = "\
*** Begin EDL
*** Author Subject Reframe From Track
+ track_id: speaker-track
+ clip_id: clip-1
+ aspect_ratio: 9:16
+ source_width: 1920
+ source_height: 1080
+ target_width: 1080
+ target_height: 1920
+ frame_rate: 30
+ smoothing: moderate
+ safe_area: mobile
*** End EDL
";
        let env = parse(text).unwrap();
        assert_eq!(env.len(), 1);
        match &env.ops[0] {
            EdlOp::AuthorSubjectReframeFromTrack {
                track_id,
                clip_id,
                aspect_ratio,
                source_width,
                source_height,
                target_width,
                target_height,
                frame_rate,
                smoothing,
                safe_area,
            } => {
                assert_eq!(track_id, "speaker-track");
                assert_eq!(clip_id, "clip-1");
                assert_eq!(aspect_ratio, "9:16");
                assert_eq!(*source_width, 1920);
                assert_eq!(*source_height, 1080);
                assert_eq!(*target_width, 1080);
                assert_eq!(*target_height, 1920);
                assert_eq!(*frame_rate, 30.0);
                assert_eq!(*smoothing, ReframeSmoothing::Moderate);
                assert_eq!(safe_area.as_deref(), Some("mobile"));
            }
            other => panic!("want AuthorSubjectReframeFromTrack, got {other:?}"),
        }
    }

    #[test]
    fn parses_delete_clip() {
        let text = "\
*** Begin EDL
*** Delete Clip
@@ anchor: clip_uuid=c-123
*** End EDL
";
        let env = parse(text).unwrap();
        assert_eq!(env.len(), 1);
        assert!(
            matches!(&env.ops[0], EdlOp::DeleteClip { anchor: Anchor::ClipUuid { uuid } } if uuid == "c-123")
        );
    }

    #[test]
    fn parses_multiple_ops_in_order() {
        let text = "\
*** Begin EDL
*** Trim Clip
@@ anchor: transcript_snippet=\"first\"
+ end: 10.0
*** Delete Clip
@@ anchor: transcript_snippet=\"second\"
*** End EDL
";
        let env = parse(text).unwrap();
        assert_eq!(env.len(), 2);
        assert!(matches!(env.ops[0], EdlOp::TrimClip { .. }));
        assert!(matches!(env.ops[1], EdlOp::DeleteClip { .. }));
    }

    #[test]
    fn parses_insert_broll_with_defaults() {
        let text = "\
*** Begin EDL
*** Insert BRoll
@@ anchor: transcript_snippet=\"the city skyline\"
+ asset: broll/skyline.mp4
+ duration_s: 2.4
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::InsertBRoll {
                asset,
                duration_s,
                position,
                ..
            } => {
                assert_eq!(asset, "broll/skyline.mp4");
                assert_eq!(*duration_s, 2.4);
                assert_eq!(*position, BRollPosition::Overlay);
            }
            _ => panic!("want InsertBRoll"),
        }
    }

    #[test]
    fn parses_insert_pip_with_defaults() {
        let text = "\
*** Begin EDL
*** Insert PiP
@@ anchor: clip_uuid=clip-1
+ asset: raw/broll/pip.mp4
+ duration_s: 4
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::InsertPiP {
                asset,
                duration_s,
                source_start_s,
                corner,
                scale,
                margin_pct,
                ..
            } => {
                assert_eq!(asset, "raw/broll/pip.mp4");
                assert_eq!(*duration_s, 4.0);
                assert_eq!(*source_start_s, 0.0);
                assert_eq!(*corner, PiPCorner::BottomRight);
                assert_eq!(*scale, 0.28);
                assert_eq!(*margin_pct, 0.035);
            }
            _ => panic!("want InsertPiP"),
        }
    }

    #[test]
    fn insert_pip_rejects_bad_layout_fields() {
        let text = "\
*** Begin EDL
*** Insert PiP
@@ anchor: clip_uuid=clip-1
+ asset: raw/broll/pip.mp4
+ duration_s: 4
+ corner: middle
*** End EDL
";
        let err = parse(text).unwrap_err();
        assert!(err.to_string().contains("corner"));
    }

    #[test]
    fn parses_move_clip() {
        let text = "\
*** Begin EDL
*** Move Clip
@@ anchor: clip_uuid=c-9f2
+ to_position: 4
*** End EDL
";
        let env = parse(text).unwrap();
        assert!(matches!(
            env.ops[0],
            EdlOp::MoveClip {
                to_position: 4,
                snap: None,
                ..
            }
        ));
    }

    #[test]
    fn parses_move_clip_at_timeline_time_with_snap_options() {
        let text = "\
*** Begin EDL
*** Move Clip
@@ anchor: clip_uuid=c-9f2
+ at_s: 12.5
+ snap: true
+ snap_tolerance_s: 0.2
+ snap_targets: clip_edge, marker
*** End EDL
";
        let env = parse(text).unwrap();
        let EdlOp::MoveClip { at_s, snap, .. } = &env.ops[0] else {
            panic!("expected MoveClip");
        };
        assert!(at_s.is_some_and(|v| (v - 12.5).abs() < 0.001));
        let snap = snap.as_ref().expect("snap options should parse");
        assert!(snap.enabled);
        assert!((snap.tolerance_s - 0.2).abs() < 0.001);
        assert_eq!(
            snap.targets,
            vec![SnapTargetKind::ClipEdge, SnapTargetKind::Marker]
        );
    }

    #[test]
    fn parses_apply_multicam_plan() {
        let text = "\
*** Begin EDL
*** Apply Multicam Plan
+ plan_json: {\"program_track\":\"Program Video\",\"decisions\":[{\"source_asset\":\"raw/cam-a.mov\",\"start_s\":0.0,\"end_s\":3.0,\"reason\":\"speaker A\"}]}
*** End EDL
";
        let env = parse(text).unwrap();
        let EdlOp::ApplyMulticamPlan { plan } = &env.ops[0] else {
            panic!("expected ApplyMulticamPlan");
        };

        assert_eq!(plan.program_track, "Program Video");
        assert_eq!(plan.decisions.len(), 1);
        assert_eq!(plan.decisions[0].source_asset, "raw/cam-a.mov");
        assert_eq!(plan.decisions[0].reason.as_deref(), Some("speaker A"));
    }

    #[test]
    fn parses_professional_nested_stack_edits() {
        let text = "\
*** Begin EDL
*** Professional Timeline Edit
+ edit_json: {\"edit\":\"wrap_as_nested\",\"track\":\"V1\",\"start_index\":0,\"end_index\":2,\"name\":\"Angle group\"}
*** Professional Timeline Edit
+ edit_json: {\"edit\":\"flatten_nested\",\"track\":\"V1\",\"child_index\":0}
*** End EDL
";
        let env = parse(text).unwrap();

        assert!(matches!(
            &env.ops[0],
            EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::WrapAsNested {
                    track,
                    start_index: 0,
                    end_index: 2,
                    name: Some(name),
                },
            } if track == "V1" && name == "Angle group"
        ));
        assert!(matches!(
            &env.ops[1],
            EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::FlattenNested {
                    track,
                    child_index: 0,
                },
            } if track == "V1"
        ));
    }

    #[test]
    fn parses_sync_group_and_audio_fx_ops() {
        let text = "\
*** Begin EDL
*** Set Sync Group
@@ anchor: clip_uuid=mic-a
+ sync_group_id: sync-a
+ offset_s: 1.25
+ speed_factor: 1.0002
+ confidence: 0.91
*** Set Clip Audio FX
@@ anchor: clip_uuid=mic-a
+ high_pass_hz: 80
+ hum_notch_hz: 60
+ pan: -0.25
+ balance: 0.40
+ adeclick: true
+ adeclip: true
+ arnndn_model: models/rnnoise.rnnn
+ compressor_threshold_db: -18
+ compressor_ratio: 2.5
+ eq_bands_json: [{\"freq_hz\":3000,\"gain_db\":-2,\"width_hz\":800}]
*** Set Track Audio FX
+ track: A1
+ loudnorm_i: -16
+ loudnorm_tp: -1.5
*** End EDL
";
        let env = parse(text).unwrap();
        assert!(matches!(
            &env.ops[0],
            EdlOp::SetSyncGroup {
                sync_group_id,
                offset_s,
                speed_factor: Some(_),
                confidence: Some(_),
                ..
            } if sync_group_id == "sync-a" && (*offset_s - 1.25).abs() < 0.001
        ));
        assert!(matches!(
            &env.ops[1],
            EdlOp::SetClipAudioFx { fx, .. }
                if fx.high_pass_hz == Some(80.0)
                    && fx.hum_notch_hz == Some(60.0)
                    && fx.pan == Some(-0.25)
                    && fx.balance == Some(0.40)
                    && fx.adeclick == Some(true)
                    && fx.adeclip == Some(true)
                    && fx.arnndn_model.as_deref() == Some("models/rnnoise.rnnn")
                    && fx.eq_bands.len() == 1
        ));
        assert!(matches!(
            &env.ops[2],
            EdlOp::SetTrackAudioFx { track, fx }
                if track == "A1" && fx.loudnorm_i == Some(-16.0)
        ));
    }

    #[test]
    fn parses_insert_transition() {
        let text = "\
*** Begin EDL
*** Insert Transition
@@ between: transcript_snippet=\"I realized\" and transcript_snippet=\"the truth was\"
+ kind: SMPTE_Dissolve
+ duration_s: 0.3
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::InsertTransition {
                between,
                kind,
                duration_s,
                spec,
                ..
            } => {
                assert_eq!(kind, "SMPTE_Dissolve");
                assert_eq!(*duration_s, 0.3);
                assert!(spec.is_none());
                assert!(
                    matches!(&between.from, Anchor::TranscriptSnippet { text } if text == "I realized")
                );
                assert!(
                    matches!(&between.to, Anchor::TranscriptSnippet { text } if text == "the truth was")
                );
            }
            _ => panic!("want InsertTransition"),
        }
    }

    #[test]
    fn parses_insert_transition_semantic_spec() {
        let text = "\
*** Begin EDL
*** Insert Transition
@@ between: clip_uuid=clip-a and clip_uuid=clip-b
+ id: awidat.slide_left
+ family: slide
+ intent: hide_motion_jump
+ energy: 0.7
+ direction: left
+ params_json: {\"blur\":0.25}
+ composition_json: {\"version\":1,\"primitives\":[{\"op\":\"push\",\"direction\":\"left\",\"distance\":0.9,\"start\":0.0,\"end\":1.0,\"easing\":\"ease_out_expo\"},{\"op\":\"blur\",\"amount\":0.65,\"direction\":\"left\",\"start\":0.1,\"end\":0.7}]}
+ duration_s: 0.28
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::InsertTransition {
                kind,
                duration_s,
                spec: Some(spec),
                ..
            } => {
                assert_eq!(kind, "awidat.slide_left");
                assert_eq!(*duration_s, 0.28);
                assert_eq!(spec.id, "awidat.slide_left");
                assert_eq!(spec.family.as_deref(), Some("slide"));
                assert_eq!(spec.intent.as_deref(), Some("hide_motion_jump"));
                assert_eq!(spec.energy, Some(0.7));
                assert_eq!(spec.direction.as_deref(), Some("left"));
                assert_eq!(spec.params.get("blur").and_then(|v| v.as_f64()), Some(0.25));
                let composition = spec.composition.as_ref().unwrap();
                assert_eq!(composition.primitives.len(), 2);
                assert!(matches!(
                    &composition.primitives[0].op,
                    awidat_proto::transitions::TransitionPrimitiveOp::Push {
                        direction,
                        distance
                    } if direction == "left" && (*distance - 0.9).abs() < 1e-9
                ));
            }
            other => panic!("want semantic InsertTransition, got {other:?}"),
        }
    }

    #[test]
    fn parses_insert_transition_semantic_fields_with_kind_id() {
        let text = "\
*** Begin EDL
*** Insert Transition
@@ between: clip_uuid=clip-a and clip_uuid=clip-b
+ kind: awidat.whip_pan_right
+ intent: hide_motion_jump
+ duration_s: 0.18
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::InsertTransition {
                kind,
                spec: Some(spec),
                ..
            } => {
                assert_eq!(kind, "awidat.whip_pan_right");
                assert_eq!(spec.id, "awidat.whip_pan_right");
                assert_eq!(spec.intent.as_deref(), Some("hide_motion_jump"));
            }
            other => panic!("want semantic InsertTransition, got {other:?}"),
        }
    }

    #[test]
    fn parses_insert_transition_alignment_and_offsets() {
        let text = "\
*** Begin EDL
*** Insert Transition
@@ between: clip_uuid=clip-a and clip_uuid=clip-b
+ kind: SMPTE_Dissolve
+ duration_s: 1.0
+ alignment: end_at_cut
+ in_offset_s: 0.8
+ out_offset_s: 0.2
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::InsertTransition {
                alignment,
                in_offset_s,
                out_offset_s,
                ..
            } => {
                assert_eq!(*alignment, Some(TransitionAlignment::EndAtCut));
                assert_eq!(*in_offset_s, Some(0.8));
                assert_eq!(*out_offset_s, Some(0.2));
            }
            other => panic!("want InsertTransition, got {other:?}"),
        }
    }

    #[test]
    fn parses_delete_transition() {
        let text = "\
*** Begin EDL
*** Delete Transition
@@ between: clip_uuid=clip-a and clip_uuid=clip-b
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::DeleteTransition { between } => {
                assert!(matches!(&between.from, Anchor::ClipUuid { uuid } if uuid == "clip-a"));
                assert!(matches!(&between.to, Anchor::ClipUuid { uuid } if uuid == "clip-b"));
            }
            other => panic!("want DeleteTransition, got {other:?}"),
        }
    }

    #[test]
    fn parses_set_cut_intent() {
        let text = "\
*** Begin EDL
*** Set Cut Intent
@@ between: clip_uuid=clip-a and clip_uuid=clip-b
+ cut_type: cut_on_action
+ intent: hide_action_continuity
+ energy: 0.72
+ audio_relation: sync
+ confidence: 0.91
+ reason: outgoing hand motion resolves into the next shot
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::SetCutIntent { between, spec } => {
                assert!(matches!(&between.from, Anchor::ClipUuid { uuid } if uuid == "clip-a"));
                assert!(matches!(&between.to, Anchor::ClipUuid { uuid } if uuid == "clip-b"));
                assert_eq!(spec.cut_type, CutType::CutOnAction);
                assert_eq!(spec.intent, "hide_action_continuity");
                assert_eq!(spec.energy, Some(0.72));
                assert_eq!(spec.audio_relation, AudioRelation::Sync);
                assert_eq!(spec.confidence, Some(0.91));
                assert_eq!(
                    spec.reason.as_deref(),
                    Some("outgoing hand motion resolves into the next shot")
                );
            }
            other => panic!("want SetCutIntent, got {other:?}"),
        }
    }

    #[test]
    fn parses_set_volume() {
        let text = "\
*** Begin EDL
*** Set Volume
@@ anchor: clip_uuid=clip-1
+ value: 0.5
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::SetVolume { anchor, value } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-1"));
                assert!((value - 0.5).abs() < 1e-9);
            }
            other => panic!("want SetVolume, got {other:?}"),
        }
    }

    #[test]
    fn parses_mute_clip_defaults_to_muted() {
        let text = "\
*** Begin EDL
*** Mute Clip
@@ anchor: clip_uuid=clip-1
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::MuteClip { anchor, muted } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-1"));
                assert!(*muted, "mute defaults to true when the field is omitted");
            }
            other => panic!("want MuteClip, got {other:?}"),
        }
    }

    #[test]
    fn parses_mute_clip_unmute() {
        let text = "\
*** Begin EDL
*** Mute Clip
@@ anchor: clip_uuid=clip-1
+ muted: false
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::MuteClip { muted, .. } => assert!(!*muted, "explicit muted:false unmutes"),
            other => panic!("want MuteClip, got {other:?}"),
        }
    }

    #[test]
    fn parses_audio_lead_and_trail() {
        let text = "\
*** Begin EDL
*** Set Audio Lead
@@ anchor: clip_uuid=clip-b
+ lead_s: 0.45
+ reason: let next speaker enter before picture
+ confidence: 0.87
*** Set Audio Trail
@@ anchor: clip_uuid=clip-a
+ trail_s: 0.60
+ reason: hold outgoing answer under reaction
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::SetAudioLead {
                anchor,
                lead_s,
                split_edit: Some(split_edit),
            } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-b"));
                assert_eq!(*lead_s, 0.45);
                assert_eq!(split_edit.audio_lead_s, Some(0.45));
                assert_eq!(
                    split_edit.reason.as_deref(),
                    Some("let next speaker enter before picture")
                );
                assert_eq!(split_edit.confidence, Some(0.87));
            }
            other => panic!("want SetAudioLead, got {other:?}"),
        }
        match &env.ops[1] {
            EdlOp::SetAudioTrail {
                anchor,
                trail_s,
                split_edit: Some(split_edit),
            } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-a"));
                assert_eq!(*trail_s, 0.60);
                assert_eq!(split_edit.audio_trail_s, Some(0.60));
                assert_eq!(
                    split_edit.reason.as_deref(),
                    Some("hold outgoing answer under reaction")
                );
            }
            other => panic!("want SetAudioTrail, got {other:?}"),
        }
    }

    #[test]
    fn parses_set_effect_with_params_and_rationale() {
        let text = "\
*** Begin EDL
*** Set Effect
@@ anchor: clip_uuid=clip-1
+ effect: awidat.color_correction
+ params_json: {\"contrast\":1.15,\"saturation\":0.9}
+ rationale: subtle correction for flat camera angle
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::SetEffect {
                anchor,
                effect,
                params,
                rationale,
            } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-1"));
                assert_eq!(effect, "awidat.color_correction");
                assert_eq!(params.get("contrast").and_then(|v| v.as_f64()), Some(1.15));
                assert_eq!(params.get("saturation").and_then(|v| v.as_f64()), Some(0.9));
                assert_eq!(
                    rationale.as_deref(),
                    Some("subtle correction for flat camera angle")
                );
            }
            other => panic!("want SetEffect, got {other:?}"),
        }
    }

    #[test]
    fn set_effect_missing_effect_is_error() {
        let text = "\
*** Begin EDL
*** Set Effect
@@ anchor: clip_uuid=clip-1
+ params_json: {\"contrast\":1.15}
*** End EDL
";
        let err = parse(text).unwrap_err();
        assert!(matches!(err, EdlParseError::MissingField { ref field, .. } if field == "effect"));
    }

    #[test]
    fn set_effect_rejects_invalid_params_json() {
        let text = "\
*** Begin EDL
*** Set Effect
@@ anchor: clip_uuid=clip-1
+ effect: awidat.color_correction
+ params_json: {\"contrast\":
*** End EDL
";
        let err = parse(text).unwrap_err();
        assert!(
            matches!(err, EdlParseError::BadField { ref message, .. } if message.contains("JSON object")),
            "got: {err:?}"
        );
    }

    #[test]
    fn parses_set_speed() {
        let text = "\
*** Begin EDL
*** Set Speed
@@ anchor: clip_uuid=clip-2
+ factor: 2.0
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::SetSpeed { anchor, factor } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-2"));
                assert!((factor - 2.0).abs() < 1e-9);
            }
            other => panic!("want SetSpeed, got {other:?}"),
        }
    }

    #[test]
    fn parses_set_time_remap() {
        let text = "\
*** Begin EDL
*** Set Time Remap
@@ anchor: clip_uuid=clip-2
+ curve_json: [{\"source_time_s\":0,\"timeline_time_s\":0},{\"source_time_s\":2,\"timeline_time_s\":3}]
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::SetTimeRemap { anchor, curve } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-2"));
                assert_eq!(curve.len(), 2);
                assert_eq!(curve[1]["source_time_s"], serde_json::json!(2));
                assert_eq!(curve[1]["timeline_time_s"], serde_json::json!(3));
            }
            other => panic!("want SetTimeRemap, got {other:?}"),
        }
    }

    #[test]
    fn parses_set_freeze() {
        let text = "\
*** Begin EDL
*** Set Freeze
@@ anchor: clip_uuid=clip-2
+ freeze_at_source_s: 1.2
+ duration_s: 0.8
+ freeze_position: at
+ audio_behavior: silence
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::SetFreeze {
                anchor,
                freeze_at_source_s,
                duration_s,
                freeze_position,
                audio_behavior,
            } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-2"));
                assert!((freeze_at_source_s - 1.2).abs() < 1e-9);
                assert!((duration_s - 0.8).abs() < 1e-9);
                assert_eq!(freeze_position.as_deref(), Some("at"));
                assert_eq!(audio_behavior.as_deref(), Some("silence"));
            }
            other => panic!("want SetFreeze, got {other:?}"),
        }
    }

    #[test]
    fn parses_set_color_correction_with_partial_fields() {
        let text = "\
*** Begin EDL
*** Set Color Correction
@@ anchor: clip_uuid=clip-3
+ exposure_ev: 0.35
+ saturation: 1.15
+ temperature: -0.2
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::SetColorCorrection {
                anchor,
                exposure_ev,
                contrast,
                saturation,
                temperature,
                tint,
                shadows,
                highlights,
            } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-3"));
                assert_eq!(*exposure_ev, Some(0.35));
                assert!(contrast.is_none());
                assert_eq!(*saturation, Some(1.15));
                assert_eq!(*temperature, Some(-0.2));
                assert!(tint.is_none());
                assert!(shadows.is_none());
                assert!(highlights.is_none());
            }
            other => panic!("want SetColorCorrection, got {other:?}"),
        }
    }

    #[test]
    fn parses_apply_lut() {
        let text = "\
*** Begin EDL
*** Apply LUT
@@ anchor: clip_uuid=clip-4
+ lut_path: luts/show-look.cube
+ interpolation: tetrahedral
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::ApplyLut {
                anchor,
                lut_path,
                interpolation,
                strength,
            } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-4"));
                assert_eq!(lut_path, "luts/show-look.cube");
                assert_eq!(interpolation.as_deref(), Some("tetrahedral"));
                assert_eq!(*strength, None);
            }
            other => panic!("want ApplyLut, got {other:?}"),
        }
    }

    #[test]
    fn parses_apply_lut_with_strength() {
        let text = "\
*** Begin EDL
*** Apply LUT
@@ anchor: clip_uuid=clip-4
+ lut_path: luts/show-look.cube
+ strength: 0.6
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::ApplyLut { strength, .. } => {
                assert_eq!(*strength, Some(0.6));
            }
            other => panic!("want ApplyLut, got {other:?}"),
        }
    }

    #[test]
    fn parses_remove_lut() {
        let text = "\
*** Begin EDL
*** Remove LUT
@@ anchor: clip_uuid=clip-4
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::RemoveLut { anchor } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "clip-4"));
            }
            other => panic!("want RemoveLut, got {other:?}"),
        }
    }

    #[test]
    fn parses_insert_title_with_full_styling() {
        let text = "\
*** Begin EDL
*** Insert Title
+ start_s: 0.0
+ end_s: 3.0
+ text: \"Welcome\"
+ position: top
+ font_size: 72
+ color: #FFAA00
+ font_weight: bold
+ animation: fade_in_out
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::InsertTitle {
                start_s,
                end_s,
                text,
                position,
                font_size,
                color,
                font_weight,
                animation,
                phases: _,
            } => {
                assert!((start_s - 0.0).abs() < 1e-9);
                assert!((end_s - 3.0).abs() < 1e-9);
                assert_eq!(text, "Welcome");
                assert_eq!(*position, TitlePosition::Top);
                assert_eq!(*font_size, 72);
                assert_eq!(color, "#FFAA00");
                assert_eq!(*font_weight, TitleWeight::Bold);
                assert_eq!(*animation, TitleAnimation::FadeInOut);
            }
            other => panic!("want InsertTitle, got {other:?}"),
        }
    }

    #[test]
    fn parses_insert_title_with_defaults() {
        // Only required fields present — position / font_size /
        // color / weight / animation should default.
        let text = "\
*** Begin EDL
*** Insert Title
+ start_s: 1.0
+ end_s: 2.5
+ text: hello
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::InsertTitle {
                position,
                font_size,
                color,
                font_weight,
                animation,
                ..
            } => {
                assert_eq!(*position, TitlePosition::Center);
                assert_eq!(*font_size, 64);
                assert_eq!(color, "#FFFFFF");
                assert_eq!(*font_weight, TitleWeight::Normal);
                assert_eq!(*animation, TitleAnimation::None);
            }
            other => panic!("want InsertTitle, got {other:?}"),
        }
    }

    #[test]
    fn parses_set_title_with_partial_fields() {
        let text = "\
*** Begin EDL
*** Set Title
@@ anchor: clip_uuid=title-uuid
+ text: \"Updated\"
+ animation: slide_in
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::SetTitle {
                anchor,
                text,
                animation,
                start_s,
                end_s,
                position,
                font_size,
                color,
                font_weight,
                phases: _,
            } => {
                assert!(matches!(anchor, Anchor::ClipUuid { uuid } if uuid == "title-uuid"));
                assert_eq!(text.as_deref(), Some("Updated"));
                assert_eq!(*animation, Some(TitleAnimation::SlideIn));
                // Untouched fields stay None.
                assert!(start_s.is_none());
                assert!(end_s.is_none());
                assert!(position.is_none());
                assert!(font_size.is_none());
                assert!(color.is_none());
                assert!(font_weight.is_none());
            }
            other => panic!("want SetTitle, got {other:?}"),
        }
    }

    #[test]
    fn parses_insert_caption_with_defaults() {
        let text = "\
*** Begin EDL
*** Insert Caption
+ start_s: 1.0
+ end_s: 2.4
+ text: \"This changed everything\"
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::InsertCaption {
                start_s,
                end_s,
                text,
                position,
                font_size,
                color,
                safe_area,
                word_timings,
            } => {
                assert!((start_s - 1.0).abs() < 1e-9);
                assert!((end_s - 2.4).abs() < 1e-9);
                assert_eq!(text, "This changed everything");
                assert_eq!(*position, TitlePosition::Bottom);
                assert_eq!(*font_size, 52);
                assert_eq!(color, "#FFFFFF");
                assert_eq!(safe_area, "mobile");
                assert!(word_timings.is_empty());
            }
            other => panic!("want InsertCaption, got {other:?}"),
        }
    }

    #[test]
    fn parses_insert_caption_with_word_timings_json() {
        let text = "\
*** Begin EDL
*** Insert Caption
+ start_s: 1.0
+ end_s: 2.4
+ text: \"This changed everything\"
+ word_timings_json: [{\"text\":\"This\",\"start_s\":1.0,\"end_s\":1.2},{\"text\":\"changed\",\"start_s\":1.24,\"end_s\":1.5}]
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::InsertCaption { word_timings, .. } => {
                assert_eq!(word_timings.len(), 2);
                assert_eq!(word_timings[0].text, "This");
                assert!((word_timings[1].start_s - 1.24).abs() < 1e-9);
            }
            other => panic!("want InsertCaption, got {other:?}"),
        }
    }

    #[test]
    fn parses_graph_output_metadata_ops() {
        let text = "\
*** Begin EDL
*** Set Output Format
+ aspect_ratio: 9:16
+ platform: youtube_shorts
+ safe_area: mobile
*** Set Loudness Target
+ integrated_lufs: -16
+ true_peak_db: -1
*** Set Package Metadata
+ platform: youtube_shorts
+ title: \"Launch Risk\"
+ description: \"A short clip about launch risk\"
+ tags: launch,risk,clip
*** End EDL
";
        let env = parse(text).unwrap();
        assert!(matches!(
            &env.ops[0],
            EdlOp::SetOutputFormat {
                aspect_ratio,
                platform: Some(platform),
                safe_area: Some(safe_area)
            } if aspect_ratio == "9:16" && platform == "youtube_shorts" && safe_area == "mobile"
        ));
        assert!(matches!(
            &env.ops[1],
            EdlOp::SetLoudnessTarget {
                integrated_lufs,
                true_peak_db: Some(true_peak_db)
            } if (*integrated_lufs + 16.0).abs() < 1e-9 && (*true_peak_db + 1.0).abs() < 1e-9
        ));
        assert!(matches!(
            &env.ops[2],
            EdlOp::SetPackageMetadata {
                platform: Some(platform),
                title: Some(title),
                ..
            } if platform == "youtube_shorts" && title == "Launch Risk"
        ));
    }

    #[test]
    fn set_broadcast_overlay_parses_config_json() {
        let text = r#"*** Begin EDL
*** Set Broadcast Overlay
+ config_json: {"enabled":true,"template_name":"technologia","episode_title":"Ben Adams","show_name":"TECHNOLOGIA TALKS","host_a":{"name":"Tadiwa Mbuwayesango","title":"Co-Host","photo_path":"branding/tadiwa.jpg"},"host_b":{"name":"Elvis Kimara","title":"Co-Host","photo_path":"branding/elvis.jpg"},"sponsors":["LEARN-X","Throwly"],"topics":[{"time_seconds":45.0,"text":"Custom drones"}],"chapters":[{"time_seconds":90.0,"text":"Hardware barriers"}]}
*** End EDL
"#;
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::SetBroadcastOverlay { config } => {
                assert!(config.enabled);
                assert_eq!(config.template_name.as_deref(), Some("technologia"));
                assert_eq!(config.episode_title, "Ben Adams");
                assert_eq!(
                    config.host_a.photo_path.as_deref(),
                    Some("branding/tadiwa.jpg")
                );
                assert_eq!(config.sponsors, vec!["LEARN-X", "Throwly"]);
                assert_eq!(config.topics[0].text, "Custom drones");
                assert_eq!(config.chapters[0].time_seconds, 90.0);
            }
            _ => panic!("want SetBroadcastOverlay"),
        }
    }

    #[test]
    fn set_broadcast_overlay_parses_convenience_fields() {
        let text = r#"*** Begin EDL
*** Set Broadcast Overlay
+ enabled: true
+ short_form_mode: true
+ episode_title: Ben Adams
+ show_name: Technologia Talks
+ host_a_name: Tadiwa Mbuwayesango
+ host_a_title: Co-Host
+ host_b_name: Elvis Kimara
+ host_b_title: Co-Host
+ sponsors: ["LEARN-X","Throwly"]
+ topics: [{"time_seconds":12,"text":"Opening"}]
*** End EDL
"#;
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::SetBroadcastOverlay { config } => {
                assert!(config.short_form_mode);
                assert_eq!(config.episode_title, "Ben Adams");
                assert_eq!(config.show_name, "Technologia Talks");
                assert_eq!(config.host_a.name, "Tadiwa Mbuwayesango");
                assert_eq!(config.host_b.title, "Co-Host");
                assert_eq!(config.sponsors.len(), 2);
                assert_eq!(config.topics[0].text, "Opening");
            }
            _ => panic!("want SetBroadcastOverlay"),
        }
    }

    #[test]
    fn insert_title_rejects_bad_position() {
        let text = "\
*** Begin EDL
*** Insert Title
+ start_s: 0.0
+ end_s: 3.0
+ text: hi
+ position: weird
*** End EDL
";
        let err = parse(text).unwrap_err();
        assert!(
            matches!(err, EdlParseError::BadField { ref message, .. } if message.contains("'top'"))
        );
    }

    #[test]
    fn set_volume_missing_value_is_error() {
        let text = "\
*** Begin EDL
*** Set Volume
@@ anchor: clip_uuid=clip-1
*** End EDL
";
        let err = parse(text).unwrap_err();
        assert!(matches!(err, EdlParseError::MissingField { ref field, .. } if field == "value"));
    }

    #[test]
    fn missing_begin_is_error() {
        let err = parse("*** Trim Clip\n*** End EDL").unwrap_err();
        // Stray content before Begin → StrayLine.
        assert!(matches!(err, EdlParseError::StrayLine { .. }));
    }

    #[test]
    fn missing_end_is_error() {
        let text = "\
*** Begin EDL
*** Trim Clip
@@ anchor: clip_uuid=c-1
+ end: 1.0
";
        let err = parse(text).unwrap_err();
        assert_eq!(err, EdlParseError::MissingEnd);
    }

    #[test]
    fn unknown_op_heading_is_error() {
        let text = "\
*** Begin EDL
*** Reverse Polarity
@@ anchor: clip_uuid=c-1
*** End EDL
";
        let err = parse(text).unwrap_err();
        assert!(
            matches!(err, EdlParseError::UnknownOp { heading, .. } if heading == "Reverse Polarity")
        );
    }

    #[test]
    fn missing_anchor_is_error() {
        let text = "\
*** Begin EDL
*** Trim Clip
+ end: 1.0
*** End EDL
";
        let err = parse(text).unwrap_err();
        assert!(matches!(err, EdlParseError::MissingField { field, .. } if field == "anchor"));
    }

    #[test]
    fn dash_lines_are_treated_as_informational() {
        let text = "\
*** Begin EDL
*** Trim Clip
@@ anchor: clip_uuid=c-1
- start: 0.0
- end: 100.0
+ end: 50.0
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::TrimClip { end, start, .. } => {
                // `-` lines ignored; only `+` end is taken.
                assert_eq!(*end, Some(50.0));
                assert_eq!(*start, None);
            }
            _ => panic!("want TrimClip"),
        }
    }

    #[test]
    fn empty_anchor_text_is_error() {
        let text = "\
*** Begin EDL
*** Trim Clip
@@ anchor: transcript_snippet=\"\"
+ end: 1.0
*** End EDL
";
        let err = parse(text).unwrap_err();
        assert!(matches!(err, EdlParseError::BadAnchor { .. }));
    }

    #[test]
    fn unknown_anchor_kind_is_error() {
        let text = "\
*** Begin EDL
*** Trim Clip
@@ anchor: hand_wave=foo
+ end: 1.0
*** End EDL
";
        let err = parse(text).unwrap_err();
        assert!(
            matches!(err, EdlParseError::BadAnchor { message, .. } if message.contains("hand_wave"))
        );
    }

    #[test]
    fn stray_content_after_end_is_error() {
        let text = "\
*** Begin EDL
*** Trim Clip
@@ anchor: clip_uuid=c-1
+ end: 1.0
*** End EDL
*** Trim Clip
";
        let err = parse(text).unwrap_err();
        assert!(matches!(err, EdlParseError::StrayLine { .. }));
    }

    #[test]
    fn blank_lines_are_tolerated() {
        let text = "\
*** Begin EDL

*** Trim Clip

@@ anchor: clip_uuid=c-1

+ end: 1.0

*** End EDL
";
        let env = parse(text).unwrap();
        assert_eq!(env.len(), 1);
    }

    #[test]
    fn trailing_whitespace_tolerated() {
        let text = "*** Begin EDL   \n*** Trim Clip   \n@@ anchor: clip_uuid=c-1   \n+ end: 1.0   \n*** End EDL   \n";
        parse(text).unwrap();
    }

    #[test]
    fn scene_change_index_anchor_parses() {
        let text = "\
*** Begin EDL
*** Delete Clip
@@ anchor: scene_change_index=raw/x.mp4:7
*** End EDL
";
        let env = parse(text).unwrap();
        match &env.ops[0] {
            EdlOp::DeleteClip {
                anchor: Anchor::SceneChangeIndex { asset_id, index },
            } => {
                assert_eq!(asset_id, "raw/x.mp4");
                assert_eq!(*index, 7);
            }
            _ => panic!("want SceneChangeIndex anchor"),
        }
    }
}
