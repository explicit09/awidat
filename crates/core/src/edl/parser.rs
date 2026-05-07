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

use thiserror::Error;

use super::op::{Anchor, BRollPosition, EdlEnvelope, EdlOp, TransitionBetween};

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
    #[error("line {line}: unknown op heading {heading:?}; expected one of: \
             Trim Clip, Delete Clip, Split Clip, Untrim Clip, Insert Clip, Insert BRoll, Move Clip, Insert Transition")]
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
    MoveClip,
    InsertTransition,
    SetVolume,
    SetSpeed,
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
            "Move Clip" => OpKind::MoveClip,
            "Insert Transition" => OpKind::InsertTransition,
            "Set Volume" => OpKind::SetVolume,
            "Set Speed" => OpKind::SetSpeed,
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
                Ok(EdlOp::SplitClip { anchor, at_s })
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
                Ok(EdlOp::InsertClip {
                    asset,
                    track,
                    at_position: take_field_usize(&mut fields, "at_position"),
                    start: take_field_f64(&mut fields, "start"),
                    end: take_field_f64(&mut fields, "end"),
                    name: take_field_string(&mut fields, "name"),
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
            OpKind::MoveClip => {
                let anchor = self.anchor.ok_or_else(|| EdlParseError::MissingField {
                    line: head,
                    field: "anchor".into(),
                })?;
                let to_position = take_field_usize(&mut fields, "to_position").ok_or_else(
                    || EdlParseError::MissingField {
                        line: head,
                        field: "to_position".into(),
                    },
                )?;
                Ok(EdlOp::MoveClip {
                    anchor,
                    to_position,
                })
            }
            OpKind::InsertTransition => {
                let between = self
                    .between
                    .ok_or_else(|| EdlParseError::MissingField {
                        line: head,
                        field: "between".into(),
                    })?;
                let kind = take_field_string(&mut fields, "kind").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "kind".into(),
                    }
                })?;
                let duration_s = take_field_f64(&mut fields, "duration_s").ok_or_else(|| {
                    EdlParseError::MissingField {
                        line: head,
                        field: "duration_s".into(),
                    }
                })?;
                Ok(EdlOp::InsertTransition {
                    between,
                    kind,
                    duration_s,
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
        }
    }
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
    let (key, raw_value) = rest.split_once(':').ok_or_else(|| EdlParseError::BadField {
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
    let s = raw.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(raw);
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

fn take_field_string(fields: &mut Vec<(String, FieldValue)>, key: &str) -> Option<String> {
    let pos = fields.iter().rposition(|(k, _)| k == key)?;
    let (_, v) = fields.remove(pos);
    Some(match v {
        FieldValue::String(s) => s,
        FieldValue::Number(n) => n.to_string(),
    })
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
    let (a, b) = rest.split_once(" and ").ok_or_else(|| EdlParseError::BadAnchor {
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
                assert!(matches!(anchor, Anchor::TranscriptSnippet { text } if text == "hello world"));
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
    fn parses_delete_clip() {
        let text = "\
*** Begin EDL
*** Delete Clip
@@ anchor: clip_uuid=c-123
*** End EDL
";
        let env = parse(text).unwrap();
        assert_eq!(env.len(), 1);
        assert!(matches!(&env.ops[0], EdlOp::DeleteClip { anchor: Anchor::ClipUuid { uuid } } if uuid == "c-123"));
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
            EdlOp::InsertBRoll { asset, duration_s, position, .. } => {
                assert_eq!(asset, "broll/skyline.mp4");
                assert_eq!(*duration_s, 2.4);
                assert_eq!(*position, BRollPosition::Overlay);
            }
            _ => panic!("want InsertBRoll"),
        }
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
        assert!(matches!(env.ops[0], EdlOp::MoveClip { to_position: 4, .. }));
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
            EdlOp::InsertTransition { between, kind, duration_s } => {
                assert_eq!(kind, "SMPTE_Dissolve");
                assert_eq!(*duration_s, 0.3);
                assert!(matches!(&between.from, Anchor::TranscriptSnippet { text } if text == "I realized"));
                assert!(matches!(&between.to, Anchor::TranscriptSnippet { text } if text == "the truth was"));
            }
            _ => panic!("want InsertTransition"),
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
        assert!(matches!(err, EdlParseError::UnknownOp { heading, .. } if heading == "Reverse Polarity"));
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
        assert!(matches!(err, EdlParseError::BadAnchor { message, .. } if message.contains("hand_wave")));
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
            EdlOp::DeleteClip { anchor: Anchor::SceneChangeIndex { asset_id, index } } => {
                assert_eq!(asset_id, "raw/x.mp4");
                assert_eq!(*index, 7);
            }
            _ => panic!("want SceneChangeIndex anchor"),
        }
    }
}
