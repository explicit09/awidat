//! Tauri event channel names + helpers for emitting protocol items.
//!
//! Channel names mirror the constants in
//! `apps/desktop/src/protocol/index.ts` — there's no runtime check,
//! changes have to land on both sides.

use std::path::Path;

use awidat_desktop_protocol::Item;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::warn;

/// Carries one [`Item`] per event.
pub const ITEM_EVENT: &str = "awidat://item";

/// Fires once per turn when the run-loop returns.
pub const TURN_END_EVENT: &str = "awidat://turn-end";

/// Fires when the backend mutates `project.otio.json` outside the
/// agent tool-result stream, e.g. first-import auto-insert.
pub const TIMELINE_CHANGED_EVENT: &str = "awidat://timeline-changed";

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
#[serde(rename_all = "camelCase")]
pub struct TurnEndEvent {
    /// Codex turn id this terminal signal belongs to.
    pub turn_id: String,
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

/// Emit a turn-end signal. `error: Some(msg)` indicates the turn
/// failed (codex error, child exit, mapping error); `None` is a clean
/// completion. Frontend uses this to clear the "thinking..." spinner.
pub fn emit_turn_end(app: &AppHandle, turn_id: String, error: Option<String>) {
    if let Err(e) = app.emit(TURN_END_EVENT, TurnEndEvent { turn_id, error }) {
        warn!(error = %e, "emit turn-end failed");
    }
}

/// Notify the frontend that one project's on-disk timeline changed.
pub fn emit_timeline_changed(app: &AppHandle, project_root: &Path) {
    let payload = project_root.to_string_lossy().into_owned();
    if let Err(e) = app.emit(TIMELINE_CHANGED_EVENT, payload) {
        warn!(error = %e, "emit timeline-changed failed");
    }
}

/// Helpers for emitting the [`Item::Job`] lifecycle without the
/// callers having to remember the lifecycle/upsert rules. Use one
/// `JobEmitter` per job: build with [`JobEmitter::start`], call
/// [`JobEmitter::progress`] zero or more times, finish with
/// [`JobEmitter::ok`] / [`JobEmitter::err`] / [`JobEmitter::cancelled`].
pub struct JobEmitter {
    app: AppHandle,
    id: awidat_desktop_protocol::Id,
    kind: awidat_desktop_protocol::JobKind,
}

impl JobEmitter {
    /// Emit a `Started` lifecycle event and return the emitter.
    pub fn start(
        app: AppHandle,
        id: awidat_desktop_protocol::Id,
        kind: awidat_desktop_protocol::JobKind,
        status: impl Into<String>,
    ) -> Self {
        emit_item(
            &app,
            Item::Job {
                id: id.clone(),
                phase: awidat_desktop_protocol::ItemLifecycle::Started,
                job_kind: kind,
                percent: None,
                status: status.into(),
                result: None,
                output_path: None,
            },
        );
        Self { app, id, kind }
    }

    /// Emit a `Delta` lifecycle event with current progress.
    pub fn progress(&self, percent: Option<u8>, status: impl Into<String>) {
        emit_item(
            &self.app,
            Item::Job {
                id: self.id.clone(),
                phase: awidat_desktop_protocol::ItemLifecycle::Delta,
                job_kind: self.kind,
                percent,
                status: status.into(),
                result: None,
                output_path: None,
            },
        );
    }

    /// Finish the job with success. `output_path` is the absolute
    /// path of the artifact produced (the rendered mp4, the proxy,
    /// etc.) so the frontend can show "Show in Finder."
    pub fn ok_with_path(self, summary: Option<String>, output_path: Option<String>) {
        emit_item(
            &self.app,
            Item::Job {
                id: self.id,
                phase: awidat_desktop_protocol::ItemLifecycle::Completed,
                job_kind: self.kind,
                percent: Some(100),
                status: summary.clone().unwrap_or_else(|| "done".into()),
                result: Some(awidat_desktop_protocol::JobResult::Ok { summary }),
                output_path,
            },
        );
    }

    /// Finish the job with success — no output artifact.
    pub fn ok(self, summary: Option<String>) {
        self.ok_with_path(summary, None);
    }

    /// Finish the job with an error.
    pub fn err(self, message: impl Into<String>) {
        let message = message.into();
        emit_item(
            &self.app,
            Item::Job {
                id: self.id,
                phase: awidat_desktop_protocol::ItemLifecycle::Completed,
                job_kind: self.kind,
                percent: None,
                status: message.clone(),
                result: Some(awidat_desktop_protocol::JobResult::Err { message }),
                output_path: None,
            },
        );
    }

    /// Finish the job because the user cancelled it.
    pub fn cancelled(self) {
        emit_item(
            &self.app,
            Item::Job {
                id: self.id,
                phase: awidat_desktop_protocol::ItemLifecycle::Completed,
                job_kind: self.kind,
                percent: None,
                status: "cancelled".into(),
                result: Some(awidat_desktop_protocol::JobResult::Cancelled),
                output_path: None,
            },
        );
    }
}
