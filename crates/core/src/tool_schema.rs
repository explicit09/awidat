//! Provider-agnostic tool-schema types used by every in-process tool.
//!
//! These types describe the *shape* a tool exposes to whatever LLM runtime
//! we plug in (codex, Anthropic, OpenAI). The 50+ files in `tools/` only
//! reach for [`Tool`] to declare their input schema — they don't care
//! about the Anthropic wire protocol that defined this shape originally.
//!
//! The struct shape is the JSON schema layout that every modern provider
//! (Anthropic, OpenAI, Gemini, Codex) accepts: `{ name, description,
//! input_schema }`. The optional `cache_control` field is a vendor-specific
//! prompt-cache breakpoint that round-trips when present and is otherwise
//! invisible on the wire (`skip_serializing_if = "Option::is_none"`).
//!
//! Lifted out of `crate::anthropic` in step 8e so the legacy harness can
//! be deleted without touching the in-process tool surface.

use serde::{Deserialize, Serialize};

/// One tool the model can call. Registered in `ToolRegistry` so the runtime
/// can advertise the surface to whichever LLM provider is driving the turn.
///
/// `cache_control` is set on the *last* tool of every request when
/// Anthropic prompt-caching is enabled — one breakpoint covers every
/// preceding tool plus the system prompt. Other providers ignore the field.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tool {
    /// Tool name, e.g. `"bash"`.
    pub name: String,
    /// Human-readable description for the model.
    pub description: String,
    /// JSON Schema for the inputs. The model is told to conform.
    pub input_schema: serde_json::Value,
    /// Prompt-cache breakpoint. Set on the last tool only; omitted
    /// otherwise.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_control: Option<CacheControl>,
}

/// Anthropic prompt-cache breakpoint marker. Attaching this to the
/// last tool (or system block) tells the API to cache the prefix up
/// through that point. Cache reads cost ~10% of normal input tokens
/// with a 5-minute TTL; writes cost 25% more than uncached. For any
/// session > ~3 turns the math is positive.
///
/// We only use the ephemeral variant — the 1-hour cache costs more
/// to write and is overkill for our usage pattern (one project, one
/// session).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    /// Always `"ephemeral"` for our usage.
    #[serde(rename = "type")]
    pub kind: String,
}

impl CacheControl {
    /// 5-minute ephemeral cache. The right default for an montage session.
    pub fn ephemeral() -> Self {
        Self {
            kind: "ephemeral".into(),
        }
    }
}
