//! Tool trait + registry. Lifted from
//! `harnesses/codex/codex-rs/core/src/tools/registry.rs:44-92`, narrowed
//! to the minimum-viable trait per the corpus survey.
//!
//! Codex's full trait carries 7 methods (kind, matches_kind, is_mutating,
//! pre_tool_use_payload, post_tool_use_payload, create_diff_consumer,
//! handle). We keep `is_mutating()` and `handle()`. The hooks land in
//! week 6+ when we wire skills; the diff consumer lands in week 4 with
//! `apply_edl`.
//!
//! # Mutating-vs-not gating
//!
//! Per the survey: **default `is_mutating()` to `true`**. Codex's default
//! is `false` and they override to `true` for ShellHandler unless the
//! command is on a known-safe list — which means a brand-new tool is
//! parallelizable by default, which is the wrong-direction default. We
//! flip it: tools that are genuinely read-only override to `false`.
//!
//! For week 3 with one tool (`bash`, mutating), the `is_mutating` bit
//! buys us nothing. Wiring the trait method now means week 4+ tools slot
//! in without a refactor.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::FunctionCallError;
use crate::anthropic::Tool as ToolSchema;

/// One in-flight tool call. Built by the agent loop from a parsed
/// `ToolCallEnd` event.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    /// Server-allocated id from the matching `tool_use` block. The agent
    /// loop echoes this back as `tool_use_id` on the result.
    pub call_id: String,
    /// Tool name dispatched.
    pub name: String,
    /// Args, JSON-shaped per the tool's input schema.
    pub args: serde_json::Value,
}

/// Tool result. Carries the call_id so the loop can build a matching
/// `tool_result` ContentBlock without bookkeeping.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// Text payload to feed back to the model.
    pub content: String,
}

/// One tool the agent can call. Object-safe via `BoxFuture` (`async_trait`).
#[async_trait]
pub trait ToolHandler: Send + Sync {
    /// Tool name. Stable string the model uses to dispatch.
    fn name(&self) -> &'static str;

    /// JSON Schema for the tool's args + a model-facing description.
    /// The agent loop builds an [`crate::anthropic::Tool`] from this for
    /// every request.
    fn schema(&self) -> ToolSchema;

    /// True iff invocation might mutate the environment. Defensive
    /// default: `true`. Override to `false` only for genuinely read-only
    /// tools so the future parallel-dispatch gate (week 5+) lets them run
    /// concurrently.
    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    /// Perform the call.
    async fn handle(
        &self,
        invocation: ToolInvocation,
    ) -> Result<ToolOutput, FunctionCallError>;
}

/// Name → handler lookup. Cheaply cloneable (handlers are `Arc<dyn>`).
#[derive(Clone, Default)]
pub struct ToolRegistry {
    handlers: HashMap<&'static str, Arc<dyn ToolHandler>>,
}

impl ToolRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler. Panics on duplicate name (tools are
    /// statically-known; a duplicate is a bug, not a runtime condition).
    #[allow(clippy::expect_used)]
    pub fn register(&mut self, handler: Arc<dyn ToolHandler>) {
        let name = handler.name();
        let prev = self.handlers.insert(name, handler);
        assert!(prev.is_none(), "duplicate tool registration: {name}");
    }

    /// Look up a handler by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.handlers.get(name).cloned()
    }

    /// Names of registered tools, in insertion order is NOT guaranteed
    /// (HashMap). Used for building the request's `tools` array — the
    /// model doesn't care about order.
    pub fn names(&self) -> impl Iterator<Item = &&'static str> {
        self.handlers.keys()
    }

    /// Build the schema list for an outgoing Messages request.
    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.handlers.values().map(|h| h.schema()).collect()
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Returns true iff no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;

    #[async_trait]
    impl ToolHandler for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn schema(&self) -> ToolSchema {
            ToolSchema {
                name: "fake".into(),
                description: "test".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }
        }
        async fn handle(&self, _i: ToolInvocation) -> Result<ToolOutput, FunctionCallError> {
            Ok(ToolOutput {
                content: "ok".into(),
            })
        }
    }

    #[test]
    fn register_and_lookup() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Fake));
        assert!(reg.get("fake").is_some());
        assert!(reg.get("missing").is_none());
        assert_eq!(reg.len(), 1);
        let names: Vec<&&str> = reg.names().collect();
        assert_eq!(names, vec![&"fake"]);
        let schemas = reg.schemas();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "fake");
    }

    #[test]
    #[should_panic(expected = "duplicate tool registration")]
    fn duplicate_registration_panics() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Fake));
        reg.register(Arc::new(Fake));
    }

    #[tokio::test]
    async fn handle_via_dyn() {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(Fake));
        let h = reg.get("fake").unwrap();
        let out = h
            .handle(ToolInvocation {
                call_id: "c".into(),
                name: "fake".into(),
                args: serde_json::json!({}),
            })
            .await
            .unwrap();
        assert_eq!(out.content, "ok");
    }

    #[test]
    fn defaults_to_mutating() {
        let f = Fake;
        let inv = ToolInvocation {
            call_id: "c".into(),
            name: "fake".into(),
            args: serde_json::json!({}),
        };
        assert!(f.is_mutating(&inv), "defensive default per survey");
    }
}
