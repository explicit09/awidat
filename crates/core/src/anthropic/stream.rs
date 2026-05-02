//! Anthropic streaming-event parser.
//!
//! Anthropic's streaming SSE format interleaves multiple block types in
//! one response: zero or more text blocks (each emits `text_delta` per
//! chunk), zero or more `tool_use` blocks (each emits `input_json_delta`
//! per chunk — a partial JSON string that must be accumulated and parsed
//! only when the block ends).
//!
//! The wire shape (one frame per SSE `data:`):
//!
//! ```text
//! { "type": "message_start", "message": {...} }
//! { "type": "content_block_start", "index": 0,
//!   "content_block": { "type": "text", "text": "" } }
//! { "type": "content_block_delta", "index": 0,
//!   "delta": { "type": "text_delta", "text": "Hello" } }
//! { "type": "content_block_stop", "index": 0 }
//! { "type": "content_block_start", "index": 1,
//!   "content_block": { "type": "tool_use", "id": "toolu_1", "name": "bash", "input": {} } }
//! { "type": "content_block_delta", "index": 1,
//!   "delta": { "type": "input_json_delta", "partial_json": "{\"comm" } }
//! { "type": "content_block_delta", "index": 1,
//!   "delta": { "type": "input_json_delta", "partial_json": "and\":\"echo hi\"}" } }
//! { "type": "content_block_stop", "index": 1 }
//! { "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": {...} }
//! { "type": "message_stop" }
//! ```
//!
//! Our parser folds this into a flat [`StreamEvent`] enum the agent loop
//! consumes.
//!
//! # Pitfalls baked in
//!
//! 1. `input_json_delta` arrives as **string fragments**; concatenating
//!    them produces valid JSON only when the block is closed. We must NOT
//!    parse per-delta — accumulate, parse on `content_block_stop`. If
//!    parsing fails, emit a [`StreamEvent::ToolCallEnd`] with an `Err`
//!    so the agent loop can surface the malformed-args error to the model
//!    as `is_error: true` and let the model self-correct. Goose does this
//!    at `formats/anthropic.rs:720-742` and the corpus survey calls it
//!    out as load-bearing.
//!
//! 2. We index parser state by the `tool_use` block's **server-allocated
//!    id**, not by `content_block` index. Multiple parallel tool blocks
//!    may stream interleaved (in practice Anthropic serializes them, but
//!    indexing by id is robust against future ordering changes).

use std::collections::HashMap;

use serde::Deserialize;

use super::messages::{StopReason, Usage};

/// Parser-level errors. These are bugs in our parser or shape changes
/// from Anthropic, not stream-transport errors (those are
/// [`super::ClientError`]).
#[derive(Debug, thiserror::Error)]
pub enum StreamParseError {
    /// A frame's JSON didn't fit our schema.
    #[error("malformed streaming frame: {0}")]
    MalformedFrame(String),
}

/// One parsed event the agent loop consumes. Mirrors what an editorial
/// REPL or a TUI needs to render.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// The model is starting a response. Carries the message id so the
    /// caller can deduplicate or cancel.
    MessageStart {
        /// Server-allocated message id.
        message_id: String,
        /// Model that's responding (echo from `message.model`).
        model: String,
    },
    /// Incremental text delta. Print directly.
    TextDelta(String),
    /// A tool call starts. The agent loop reserves a slot but doesn't
    /// dispatch yet (args are still streaming).
    ToolCallStart {
        /// Server-allocated id; echo back in the matching `tool_result`.
        id: String,
        /// Tool name to dispatch.
        name: String,
    },
    /// A tool call ends. Args parsed; ready to dispatch.
    ///
    /// `result` is `Err(message)` if the model emitted invalid JSON for
    /// the args — surface as a tool_result with `is_error: true` so the
    /// model self-corrects (per [`crate::FunctionCallError::RespondToModel`]
    /// semantics).
    ToolCallEnd {
        /// Same id as the matching ToolCallStart.
        id: String,
        /// Tool name (echoed for caller convenience).
        name: String,
        /// Parsed args, or a parse error message.
        result: Result<serde_json::Value, String>,
    },
    /// The whole response has finished. Carries the stop reason and
    /// (best-effort) usage tally.
    Done {
        /// Why the model stopped.
        stop_reason: Option<StopReason>,
        /// Token usage (may be empty on errors).
        usage: Usage,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireEvent {
    MessageStart {
        message: WireMessage,
    },
    ContentBlockStart {
        content_block: WireContentBlock,
        #[allow(dead_code)]
        index: Option<u32>,
    },
    ContentBlockDelta {
        delta: WireDelta,
        #[allow(dead_code)]
        index: Option<u32>,
    },
    ContentBlockStop {
        #[allow(dead_code)]
        index: Option<u32>,
    },
    MessageDelta {
        delta: WireMessageDelta,
        #[serde(default)]
        usage: Option<Usage>,
    },
    MessageStop,
    /// Anthropic occasionally sends `ping` and `error` frames; we tolerate.
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct WireMessage {
    id: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireContentBlock {
    Text {
        #[allow(dead_code)]
        #[serde(default)]
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[allow(dead_code)]
        #[serde(default)]
        input: serde_json::Value,
    },
    /// Thinking, redacted_thinking, server_tool_use, ... — ignored at the
    /// parser; if we want them later we add variants.
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    /// Thinking deltas (extended thinking) — we'll consume later.
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct WireMessageDelta {
    #[serde(default)]
    stop_reason: Option<StopReason>,
}

/// Stateful parser. Feed each SSE `data:` payload via [`Self::push`];
/// drain emitted events from the returned iterator.
#[derive(Default)]
pub struct StreamParser {
    /// (tool_id) -> (name, accumulated_input_json_so_far).
    tool_calls: HashMap<String, (String, String)>,
    /// Stack of currently-open content blocks. We track names so
    /// `content_block_stop` knows what to emit. Indexed by block index.
    open_blocks: HashMap<u32, OpenBlock>,
    /// `MessageDelta`'s usage accumulates into here so we can emit a
    /// final usage tally on `MessageStop`.
    final_usage: Usage,
    final_stop_reason: Option<StopReason>,
}

#[derive(Debug, Clone)]
enum OpenBlock {
    Text,
    ToolUse { id: String },
    Other,
}

impl StreamParser {
    /// Construct an empty parser.
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse one SSE `data:` payload, returning zero or more events.
    pub fn push(&mut self, data: &str) -> Result<Vec<StreamEvent>, StreamParseError> {
        let trimmed = data.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let event: WireEvent = serde_json::from_str(trimmed).map_err(|e| {
            StreamParseError::MalformedFrame(format!("frame={trimmed} err={e}"))
        })?;

        let mut out = Vec::new();
        match event {
            WireEvent::MessageStart { message } => {
                if let Some(u) = message.usage {
                    self.merge_usage(u);
                }
                out.push(StreamEvent::MessageStart {
                    message_id: message.id,
                    model: message.model,
                });
            }
            WireEvent::ContentBlockStart {
                content_block,
                index,
            } => match content_block {
                WireContentBlock::Text { .. } => {
                    if let Some(idx) = index {
                        self.open_blocks.insert(idx, OpenBlock::Text);
                    }
                }
                WireContentBlock::ToolUse { id, name, .. } => {
                    self.tool_calls.insert(id.clone(), (name.clone(), String::new()));
                    if let Some(idx) = index {
                        self.open_blocks
                            .insert(idx, OpenBlock::ToolUse { id: id.clone() });
                    }
                    out.push(StreamEvent::ToolCallStart { id, name });
                }
                WireContentBlock::Other => {
                    if let Some(idx) = index {
                        self.open_blocks.insert(idx, OpenBlock::Other);
                    }
                }
            },
            WireEvent::ContentBlockDelta { delta, index } => match delta {
                WireDelta::TextDelta { text } => out.push(StreamEvent::TextDelta(text)),
                WireDelta::InputJsonDelta { partial_json } => {
                    if let Some(idx) = index
                        && let Some(OpenBlock::ToolUse { id }) = self.open_blocks.get(&idx)
                        && let Some((_, args)) = self.tool_calls.get_mut(id)
                    {
                        args.push_str(&partial_json);
                    }
                }
                WireDelta::Other => {}
            },
            WireEvent::ContentBlockStop { index } => {
                if let Some(idx) = index
                    && let Some(OpenBlock::ToolUse { id }) = self.open_blocks.remove(&idx)
                    && let Some((name, args)) = self.tool_calls.remove(&id)
                {
                    let result = if args.is_empty() {
                        Ok(serde_json::json!({}))
                    } else {
                        match serde_json::from_str::<serde_json::Value>(&args) {
                            Ok(v) => Ok(v),
                            Err(e) => {
                                Err(format!("could not parse tool arguments: {e}; raw: {args}"))
                            }
                        }
                    };
                    out.push(StreamEvent::ToolCallEnd { id, name, result });
                }
            }
            WireEvent::MessageDelta { delta, usage } => {
                if let Some(reason) = delta.stop_reason {
                    self.final_stop_reason = Some(reason);
                }
                if let Some(u) = usage {
                    self.merge_usage(u);
                }
            }
            WireEvent::MessageStop => {
                out.push(StreamEvent::Done {
                    stop_reason: self.final_stop_reason.take(),
                    usage: std::mem::take(&mut self.final_usage),
                });
            }
            WireEvent::Other => {
                // Ping, error frames the API may add — ignore at this layer.
                // Real `error` frames trigger a different SSE shape and would
                // be surfaced by the transport layer; tolerate here.
            }
        }
        Ok(out)
    }

    fn merge_usage(&mut self, u: Usage) {
        if u.input_tokens.is_some() {
            self.final_usage.input_tokens = u.input_tokens;
        }
        if u.output_tokens.is_some() {
            self.final_usage.output_tokens = u.output_tokens;
        }
        if u.cache_read_input_tokens.is_some() {
            self.final_usage.cache_read_input_tokens = u.cache_read_input_tokens;
        }
        if u.cache_creation_input_tokens.is_some() {
            self.final_usage.cache_creation_input_tokens = u.cache_creation_input_tokens;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(events: &[serde_json::Value]) -> Vec<String> {
        events.iter().map(serde_json::Value::to_string).collect()
    }

    fn collect_events(payloads: &[String]) -> Vec<StreamEvent> {
        let mut p = StreamParser::new();
        let mut out = Vec::new();
        for f in payloads {
            out.extend(p.push(f).expect("parse"));
        }
        out
    }

    #[test]
    fn text_only_response_yields_text_then_done() {
        let payloads = frames(&[
            serde_json::json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-haiku-4-5-20251001"}}),
            serde_json::json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "text", "text": ""}}),
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": "Hello"}}),
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": " world"}}),
            serde_json::json!({"type": "content_block_stop", "index": 0}),
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}),
            serde_json::json!({"type": "message_stop"}),
        ]);
        let evs = collect_events(&payloads);
        assert_eq!(evs.len(), 4, "{evs:?}");
        assert!(matches!(evs[0], StreamEvent::MessageStart { .. }));
        assert!(matches!(&evs[1], StreamEvent::TextDelta(t) if t == "Hello"));
        assert!(matches!(&evs[2], StreamEvent::TextDelta(t) if t == " world"));
        match &evs[3] {
            StreamEvent::Done { stop_reason, usage } => {
                assert_eq!(*stop_reason, Some(StopReason::EndTurn));
                assert_eq!(usage.output_tokens, Some(5));
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn tool_use_block_assembles_args_then_emits_end() {
        let payloads = frames(&[
            serde_json::json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-haiku-4-5-20251001"}}),
            serde_json::json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "tool_use", "id": "toolu_1", "name": "bash", "input": {}}}),
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{\"command\":"}}),
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": " \"echo hi\"}"}}),
            serde_json::json!({"type": "content_block_stop", "index": 0}),
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
            serde_json::json!({"type": "message_stop"}),
        ]);
        let evs = collect_events(&payloads);
        // expect: MessageStart, ToolCallStart, ToolCallEnd(Ok), Done.
        assert_eq!(evs.len(), 4, "{evs:?}");
        match &evs[1] {
            StreamEvent::ToolCallStart { id, name } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "bash");
            }
            other => panic!("want ToolCallStart, got {other:?}"),
        }
        match &evs[2] {
            StreamEvent::ToolCallEnd { id, name, result } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "bash");
                let v = result.as_ref().expect("args parsed");
                assert_eq!(v["command"], "echo hi");
            }
            other => panic!("want ToolCallEnd, got {other:?}"),
        }
        assert!(matches!(
            &evs[3],
            StreamEvent::Done { stop_reason: Some(StopReason::ToolUse), .. }
        ));
    }

    #[test]
    fn tool_use_with_invalid_json_yields_err_not_panic() {
        let payloads = frames(&[
            serde_json::json!({"type": "message_start", "message": {"id": "msg_1", "model": "x"}}),
            serde_json::json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "tool_use", "id": "toolu_1", "name": "bash", "input": {}}}),
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{not valid"}}),
            serde_json::json!({"type": "content_block_stop", "index": 0}),
            serde_json::json!({"type": "message_stop"}),
        ]);
        let evs = collect_events(&payloads);
        let end = evs
            .iter()
            .find(|e| matches!(e, StreamEvent::ToolCallEnd { .. }))
            .expect("ToolCallEnd present");
        match end {
            StreamEvent::ToolCallEnd { result, .. } => {
                assert!(result.is_err(), "must be Err on bad JSON, got {result:?}");
                assert!(result.as_ref().unwrap_err().contains("could not parse"));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn empty_input_block_parses_as_empty_object() {
        // Anthropic emits no input_json_delta frames for tools with no args.
        let payloads = frames(&[
            serde_json::json!({"type": "message_start", "message": {"id": "msg_1", "model": "x"}}),
            serde_json::json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "tool_use", "id": "toolu_1", "name": "now", "input": {}}}),
            serde_json::json!({"type": "content_block_stop", "index": 0}),
            serde_json::json!({"type": "message_stop"}),
        ]);
        let evs = collect_events(&payloads);
        let end = evs
            .iter()
            .find(|e| matches!(e, StreamEvent::ToolCallEnd { .. }))
            .expect("ToolCallEnd");
        match end {
            StreamEvent::ToolCallEnd { result, .. } => {
                assert_eq!(result.as_ref().unwrap(), &serde_json::json!({}));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn ping_frames_are_ignored() {
        let mut p = StreamParser::new();
        let evs = p.push(r#"{"type":"ping"}"#).unwrap();
        assert!(evs.is_empty());
    }

    #[test]
    fn empty_data_returns_no_events() {
        let mut p = StreamParser::new();
        assert!(p.push("").unwrap().is_empty());
        assert!(p.push("   ").unwrap().is_empty());
    }

    #[test]
    fn malformed_json_is_an_error() {
        let mut p = StreamParser::new();
        let err = p.push("not json").unwrap_err();
        assert!(matches!(err, StreamParseError::MalformedFrame(_)));
    }
}
