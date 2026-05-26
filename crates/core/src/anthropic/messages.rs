//! Wire types for the Messages API.
//!
//! We type only what we use. Anthropic's actual schema has more fields
//! (vision images, citations, server-tool blocks, …); they round-trip as
//! `serde_json::Value` via `#[serde(flatten)]` extras so we don't lose
//! data and don't break when Anthropic adds a field.

use serde::{Deserialize, Serialize};

// `Tool` and `CacheControl` were lifted to `crate::tool_schema` in step 8e —
// they're provider-agnostic and used by 50+ in-process tool files, so they
// outlive the legacy Anthropic harness. Re-exported here so existing
// `use crate::anthropic::{Tool, ...}` call sites keep compiling (and so
// the wire-protocol types below — `SystemBlock`, `MessagesRequest` — can
// keep referring to them as if local).
pub use crate::tool_schema::{CacheControl, Tool};

/// Role of a message in the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The user.
    User,
    /// The assistant (Claude).
    Assistant,
}

/// One message in the conversation. Anthropic's API takes a sequence of
/// these in `messages`; we build a fresh sequence per `messages_stream`
/// call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Role.
    pub role: Role,
    /// Content blocks. The API accepts either a bare string (which it
    /// wraps as `[{type: "text", text: ...}]`) or an array of blocks; we
    /// always emit the array form because every interesting message has
    /// multiple block types (text + tool_use, tool_result, …).
    pub content: Vec<ContentBlock>,
}

impl Message {
    /// Convenience: a user-role message with a single text block.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::text(text)],
        }
    }

    /// Convenience: an assistant-role message with a single text block.
    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::text(text)],
        }
    }

    /// Set a cache breakpoint on the last `Text` block of this
    /// message. No-op if the message has no text blocks.
    ///
    /// Per Anthropic's docs, a cache breakpoint at any block N caches
    /// the entire prefix `[0..=N]` of the request (system + tools +
    /// all earlier messages + this block). Subsequent requests that
    /// share the same prefix bytes hit the cache. The breakpoint
    /// survives 5 minutes (ephemeral TTL).
    pub fn set_cache_breakpoint(&mut self) {
        if let Some(block) = self.content.iter_mut().rev().find(|b| b.is_text())
            && let ContentBlock::Text { cache_control, .. } = block
        {
            *cache_control = Some(CacheControl::ephemeral());
        }
    }

    /// Clear any cache breakpoint on every text block of this
    /// message. The walker calls this on every prior message before
    /// re-marking — Anthropic rejects requests with stale
    /// breakpoints all over history.
    pub fn clear_cache_breakpoint(&mut self) {
        for block in &mut self.content {
            if let ContentBlock::Text { cache_control, .. } = block {
                *cache_control = None;
            }
        }
    }
}

/// One block of content. The API discriminates on `type`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text. Optionally carries a cache breakpoint — when set,
    /// the API caches the prefix from the start of the request up
    /// through this block. We mark exactly one text block per request
    /// (the *moving* breakpoint on the most recent stable user
    /// message); the static breakpoint goes on the last tool. Walking
    /// breakpoint discipline mirrors aider's `chat_chunks.py` and
    /// swe-agent's `CacheControlHistoryProcessor` — see Session.
    Text {
        /// The text payload.
        text: String,
        /// Cache breakpoint (omitted when not marking).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        cache_control: Option<CacheControl>,
    },
    /// Model-emitted tool call. Inputs are JSON.
    ToolUse {
        /// Server-allocated id; must be echoed in the matching ToolResult.
        id: String,
        /// Tool name.
        name: String,
        /// Arguments. Object-shaped per Anthropic schema.
        input: serde_json::Value,
    },
    /// Image input (model→model only in v1). Carried for round-trip.
    Image {
        /// `{ type: "base64", media_type: "image/png", data: "..." }`
        /// or `{ type: "url", url: "https://..." }`.
        source: serde_json::Value,
    },
    /// Result we feed back for a previous tool call.
    ///
    /// `content` is intentionally typed as `serde_json::Value` because
    /// Anthropic accepts either a plain string OR an array of inner
    /// content blocks (text + image). We use the array form for tools
    /// that return images (`view_frame`); everything else emits a string.
    /// Construct via [`ToolResult::text`] / [`ToolResult::blocks`] for
    /// readability.
    ToolResult {
        /// Id from the matching ToolUse.
        tool_use_id: String,
        /// String OR array of inner blocks.
        content: serde_json::Value,
        /// True iff the tool failed (per `FunctionCallError::RespondToModel`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    /// Anything else (thinking, redacted_thinking, server tool use, ...) —
    /// preserved verbatim so we don't lose data on round-trip.
    #[serde(other)]
    Other,
}

impl ContentBlock {
    /// Construct a plain text block (no cache control).
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text {
            text: s.into(),
            cache_control: None,
        }
    }

    /// True iff this block is a Text. Used by the cache walker to find
    /// the last text block on a message.
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }
}

/// Helpers for constructing ToolResult content. Keeps the JSON-shape
/// implementation detail off the call site.
pub mod tool_result {
    use serde_json::{Value, json};

    /// Plain-text result. The most common case.
    pub fn text(s: impl Into<String>) -> Value {
        Value::String(s.into())
    }

    /// Multi-block result: text + images. `images` are
    /// `(media_type, base64_data)` tuples.
    pub fn text_and_images(text: impl Into<String>, images: &[(String, String)]) -> Value {
        let text = text.into();
        let mut blocks = Vec::with_capacity(images.len() + usize::from(!text.is_empty()));
        if !text.is_empty() {
            blocks.push(json!({"type": "text", "text": text}));
        }
        for (media_type, data) in images {
            blocks.push(json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": media_type,
                    "data": data,
                }
            }));
        }
        Value::Array(blocks)
    }
}

/// `tool_choice` field on the request — controls whether the model is
/// allowed / forced to call a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    /// Model decides whether to call a tool.
    Auto,
    /// Force the model to call any tool.
    Any,
    /// Force the model to call this specific tool.
    Tool {
        /// Tool name.
        name: String,
    },
    /// Disable tool calling.
    None,
}

/// One block of system prompt — text plus optional cache control.
/// Serialized as `{"type":"text","text":"...","cache_control":{...}}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemBlock {
    /// Always `"text"` for our usage.
    #[serde(rename = "type")]
    pub kind: String,
    /// The system-prompt body.
    pub text: String,
    /// Prompt-cache breakpoint; set on the last block when caching.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_control: Option<CacheControl>,
}

/// `system` field — the API accepts either a bare string or an array
/// of typed blocks. We use the block form when caching, the string
/// form otherwise (one fewer wire byte; matches what every API
/// example shows).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum System {
    /// Plain text (no cache control).
    Text(String),
    /// Block form. The Anthropic API requires this shape when
    /// `cache_control` is in play.
    Blocks(Vec<SystemBlock>),
}

/// One messages-API request. Narrow to what we use.
#[derive(Debug, Clone, Serialize)]
pub struct MessagesRequest {
    /// Model id.
    pub model: String,
    /// Max output tokens. Required by the API.
    pub max_tokens: u32,
    /// System prompt (top-level field; not a `Message` with role=System).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<System>,
    /// Conversation messages.
    pub messages: Vec<Message>,
    /// Available tools.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
    /// Tool-choice policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Sampling temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// True to enable streaming. Always `true` for our client; we never
    /// use the non-streaming path.
    pub stream: bool,
}

impl MessagesRequest {
    /// Build a streaming request with sensible defaults.
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            max_tokens: 4096,
            system: None,
            messages,
            tools: Vec::new(),
            tool_choice: None,
            temperature: None,
            stream: true,
        }
    }

    /// Add a system prompt as a plain string (no cache control).
    #[must_use]
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(System::Text(system.into()));
        self
    }

    /// Add a system prompt and mark it cacheable. Tier-1 of the
    /// two-tier cache strategy: system + tools rarely change across
    /// turns, so caching them gives a 90% input-token discount on
    /// every subsequent turn within the 5-minute TTL.
    #[must_use]
    pub fn with_system_cached(mut self, system: impl Into<String>) -> Self {
        self.system = Some(System::Blocks(vec![SystemBlock {
            kind: "text".into(),
            text: system.into(),
            cache_control: Some(CacheControl::ephemeral()),
        }]));
        self
    }

    /// Mark the last tool in `self.tools` with a cache breakpoint.
    /// No-op when `tools` is empty. The breakpoint covers the entire
    /// `tools` array AND the system block (when cached) — Anthropic
    /// applies cache prefixes top-down: system → tools → messages.
    pub fn cache_last_tool(&mut self) {
        if let Some(last) = self.tools.last_mut() {
            last.cache_control = Some(CacheControl::ephemeral());
        }
    }

    /// Set max tokens.
    #[must_use]
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = n;
        self
    }

    /// Set the available tools.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = tools;
        self
    }

    /// Set the tool_choice policy.
    #[must_use]
    pub fn with_tool_choice(mut self, choice: ToolChoice) -> Self {
        self.tool_choice = Some(choice);
        self
    }
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Natural completion.
    EndTurn,
    /// Hit `max_tokens`.
    MaxTokens,
    /// A stop sequence.
    StopSequence,
    /// The model emitted a `tool_use` block; the next turn must include
    /// `tool_result`s.
    ToolUse,
    /// Refusal.
    Refusal,
    /// Pause for a tool call we've not yet implemented.
    PauseTurn,
}

/// Token usage from the response. Surfaced via [`super::StreamEvent::Done`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Input token count.
    #[serde(default)]
    pub input_tokens: Option<u32>,
    /// Output token count.
    #[serde(default)]
    pub output_tokens: Option<u32>,
    /// Cached input tokens (cache read hit).
    #[serde(default)]
    pub cache_read_input_tokens: Option<u32>,
    /// Cached input tokens (cache write).
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_text_serializes_with_array_content() {
        let m = Message::user_text("hello");
        let s = serde_json::to_string(&m).unwrap();
        assert!(s.contains("\"role\":\"user\""));
        assert!(s.contains("\"type\":\"text\""));
        assert!(s.contains("\"text\":\"hello\""));
    }

    #[test]
    fn tool_result_includes_is_error_only_when_set() {
        let ok = ContentBlock::ToolResult {
            tool_use_id: "abc".into(),
            content: tool_result::text("ok"),
            is_error: None,
        };
        let s = serde_json::to_string(&ok).unwrap();
        assert!(
            !s.contains("is_error"),
            "is_error must be omitted when None: {s}"
        );

        let err = ContentBlock::ToolResult {
            tool_use_id: "abc".into(),
            content: tool_result::text("boom"),
            is_error: Some(true),
        };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("\"is_error\":true"));
    }

    #[test]
    fn tool_result_text_helper_emits_string() {
        let v = tool_result::text("hi");
        assert_eq!(v, serde_json::Value::String("hi".into()));
    }

    #[test]
    fn tool_result_text_and_images_helper_builds_array() {
        let v = tool_result::text_and_images(
            "frame at 12.4s",
            &[("image/png".into(), "BASE64".into())],
        );
        assert!(v.is_array());
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[0]["text"], "frame at 12.4s");
        assert_eq!(arr[1]["type"], "image");
        assert_eq!(arr[1]["source"]["type"], "base64");
        assert_eq!(arr[1]["source"]["media_type"], "image/png");
        assert_eq!(arr[1]["source"]["data"], "BASE64");
    }

    #[test]
    fn tool_result_text_only_no_images_omits_image_blocks() {
        let v = tool_result::text_and_images("just text", &[]);
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "text");
    }

    #[test]
    fn tool_result_image_only_omits_empty_text_block() {
        let v = tool_result::text_and_images("", &[("image/png".into(), "X".into())]);
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "image");
    }

    #[test]
    fn tool_use_round_trips() {
        let block = ContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: "bash".into(),
            input: serde_json::json!({"command": "echo hi"}),
        };
        let s = serde_json::to_string(&block).unwrap();
        let back: ContentBlock = serde_json::from_str(&s).unwrap();
        match back {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "bash");
                assert_eq!(input["command"], "echo hi");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn unknown_block_type_round_trips_as_other() {
        let json = serde_json::json!({"type": "thinking", "thinking": "..." });
        let block: ContentBlock = serde_json::from_value(json).unwrap();
        assert!(matches!(block, ContentBlock::Other));
    }

    #[test]
    fn request_omits_empty_collections() {
        let req = MessagesRequest::new("claude-haiku-4-5-20251001", vec![Message::user_text("hi")]);
        let s = serde_json::to_string(&req).unwrap();
        // Empty `tools` vec must not serialize.
        assert!(!s.contains("tools"));
        // Required fields are present.
        assert!(s.contains("\"max_tokens\":4096"));
        assert!(s.contains("\"stream\":true"));
    }
}
