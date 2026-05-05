//! Tauri event channel names + helpers for emitting protocol items.
//!
//! Channel names mirror the constants in
//! `apps/desktop/src/protocol/index.ts` — there's no runtime check,
//! changes have to land on both sides.

use awidat_desktop_protocol::Item;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::warn;

/// Carries one [`Item`] per event.
pub const ITEM_EVENT: &str = "awidat://item";

/// Fires once per turn when the run-loop returns.
pub const TURN_END_EVENT: &str = "awidat://turn-end";

/// Wraps an [`Item`] for transport over Tauri's event bus. Adds an
/// envelope so we can grow it (turn-id, thread-id correlation) later
/// without breaking subscribers.
#[derive(Debug, Clone, Serialize)]
pub struct ItemEvent {
    /// The protocol item.
    pub item: Item,
}

/// Payload emitted to `awidat://turn-end` when the run-loop returns.
#[derive(Debug, Clone, Serialize)]
pub struct TurnEndEvent {
    /// `Some(msg)` on session-level error, `None` on clean end.
    pub error: Option<String>,
}

/// Emit one item over `ITEM_EVENT`. Logs and swallows transport
/// errors — the frontend will surface the resulting absent state via
/// the running flag, and we don't want a single failed emit to crash
/// the agent loop.
pub fn emit_item(app: &AppHandle, item: Item) {
    if let Err(e) = app.emit(ITEM_EVENT, ItemEvent { item }) {
        warn!(error = %e, "emit item failed");
    }
}
