//! Unified event type the app loop drains.
//!
//! Three sources are merged into one `AppEvent` queue:
//!
//! 1. Terminal input (crossterm `KeyEvent`, resize, mouse).
//! 2. Agent broadcast (`awidat_core::SessionEvent`) — model deltas,
//!    tool calls, tool results, end-of-turn.
//! 3. Approval requests (`awidat_core::ApprovalRequest`) — mutating-tool
//!    sign-off prompts the loop emits before dispatch.
//!
//! Merging at the channel layer keeps the render loop single-threaded
//! and means we never need a mutex around chat state. Pattern is lifted
//! from `harnesses/codex/codex-rs/tui/src/app_event.rs` (~900 lines)
//! but pruned to just what we use today.
//!
//! The `Tick` variant fires on a periodic timer (~33ms) so the chat
//! pane can advance spinners and drain the streaming queue smoothly.

use awidat_core::{SessionEvent, tool::ApprovalRequest, tool::UserInputRequest};
use crossterm::event::{KeyEvent, MouseEvent};

/// Everything the app loop reacts to.
#[derive(Debug)]
pub enum AppEvent {
    /// User pressed a key.
    Key(KeyEvent),
    /// User pasted a (possibly multi-line) string. Delivered as one
    /// event by terminals that support bracketed paste, instead of
    /// streaming Char + Enter keys (which would auto-submit at the
    /// first \n). Composer inserts the whole string at the cursor.
    Paste(String),
    /// User scrolled / clicked. Currently unused but plumbed so we can
    /// add scrollback navigation without rewiring the loop.
    Mouse(MouseEvent),
    /// Terminal resize. Triggers a full repaint.
    Resize {
        /// New width in cells.
        width: u16,
        /// New height in cells.
        height: u16,
    },
    /// Periodic tick (~33ms). Drives spinner animation and streaming
    /// drain.
    Tick,
    /// Agent emitted a SessionEvent.
    Session(SessionEvent),
    /// Agent wants approval to dispatch a mutating tool.
    Approval(ApprovalRequest),
    /// Agent wants the user to answer a question (`request_user_input`).
    UserInput(UserInputRequest),
    /// Crossterm event stream errored or closed. The app loop logs and
    /// keeps running — terminal events are best-effort.
    TerminalEventError(String),
    /// User asked the app to exit (Ctrl-C, Ctrl-D from an empty composer,
    /// or `:quit`).
    Quit,
}
