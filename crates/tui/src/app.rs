//! The TUI event loop. Owns the terminal, the chat state, the
//! composer, the optional approval modal, and merges three async
//! sources (terminal events, agent broadcast, approval channel) into
//! one `AppEvent` queue.
//!
//! Lifecycle:
//!
//! 1. Caller hands us a `Session` and the receivers from
//!    `subscribe()` / `with_approval_channel`.
//! 2. We enter the alt-screen, drive the loop, painting on every Tick
//!    or AppEvent that mutates state.
//! 3. On Quit (Ctrl-C, Ctrl-D, `:q`) we restore the terminal.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use awidat_core::tool::{ApprovalDecision, ApprovalRequest, UserInputRequest};
use awidat_core::{Session, SessionEvent};
use crossterm::event::{Event as CtEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::{TerminalOptions, Viewport};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::approval::ApprovalModal;
use crate::chat::Chat;
use crate::composer::Composer;
use crate::event::AppEvent;
use crate::timeline::Timeline;

/// One-shot configuration the caller hands `App::new`.
pub struct AppConfig {
    /// The session the TUI drives. Must already have been built with
    /// `with_approval_channel(...)` if approvals are wanted.
    pub session: Arc<Session>,
    /// Receiver for approval requests. Pair with the matching sender on
    /// the Session.
    pub approval_rx: mpsc::Receiver<ApprovalRequest>,
    /// Receiver for user-input requests (`request_user_input` tool).
    /// Optional — None disables that tool.
    pub user_input_rx: Option<mpsc::Receiver<UserInputRequest>>,
}

/// The TUI app.
pub struct App {
    session: Arc<Session>,
    chat: Chat,
    timeline: Timeline,
    composer: Composer,
    modal: Option<ApprovalModal>,
    /// In-flight user-input request (we don't render a separate modal
    /// for v1 — the next composer submission feeds the reply oneshot).
    pending_user_input: Option<UserInputRequest>,
    /// Token cancelling the in-flight turn (Ctrl-C while turning).
    turn_cancel: Option<CancellationToken>,
    /// Handle of the in-flight turn task.
    turn_task: Option<JoinHandle<()>>,
    /// Timeline snapshot captured at `apply_edl` ToolCallStart, used to
    /// produce a before/after diff crumb when its ToolResult arrives.
    /// Keyed on call_id so out-of-order or interleaved tool calls don't
    /// corrupt the snapshot.
    pending_apply_edl_snapshot: Option<(String, Vec<crate::timeline::Row>)>,
    quit: bool,
}

impl App {
    /// Build a fresh app from the config.
    pub fn new(cfg: &AppConfig) -> Self {
        let project_root = cfg.session.project_root().to_path_buf();
        Self {
            session: cfg.session.clone(),
            chat: Chat::new(),
            timeline: Timeline::new(&project_root),
            composer: Composer::new("ask awidat anything…"),
            modal: None,
            pending_user_input: None,
            turn_cancel: None,
            turn_task: None,
            pending_apply_edl_snapshot: None,
            quit: false,
        }
    }

    /// Run the loop until the user quits.
    pub async fn run(mut self, cfg: AppConfig) -> Result<()> {
        let mut terminal = enter_terminal()?;
        let result = self.event_loop(&mut terminal, cfg).await;
        let _ = leave_terminal(&mut terminal);
        result
    }

    async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        cfg: AppConfig,
    ) -> Result<()> {
        let (app_tx, mut app_rx) = mpsc::unbounded_channel::<AppEvent>();

        // Spawn the input/event pumps. Each translates its source into
        // AppEvent and feeds the unified queue.
        let _terminal_pump = spawn_terminal_pump(app_tx.clone());
        let _session_pump = spawn_session_pump(self.session.subscribe(), app_tx.clone());
        let _approval_pump = spawn_approval_pump(cfg.approval_rx, app_tx.clone());
        if let Some(rx) = cfg.user_input_rx {
            let _ = spawn_user_input_pump(rx, app_tx.clone());
        }
        let _tick_pump = spawn_tick_pump(app_tx.clone());

        // Initial paint.
        self.paint(terminal)?;

        while !self.quit
            && let Some(event) = app_rx.recv().await
        {
            let mutated = self.handle_event(event);
            if mutated {
                self.paint(terminal)?;
            }
        }
        Ok(())
    }

    fn handle_event(&mut self, event: AppEvent) -> bool {
        match event {
            AppEvent::Tick => {
                self.chat.tick();
                true
            }
            AppEvent::Resize { .. } => true,
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Mouse(_) => false,
            AppEvent::Session(ev) => {
                // Turn lifecycle signals — clear in-flight state when
                // the session reports the turn is over, so the next
                // composer submission can start a new one.
                if matches!(&ev, SessionEvent::TurnEnd | SessionEvent::Error(_)) {
                    self.turn_task = None;
                    self.turn_cancel = None;
                }
                // Snapshot the timeline at the start of an apply_edl call
                // so we can produce a diff crumb after it commits.
                if let SessionEvent::ToolCallStart { id, name } = &ev
                    && name == "apply_edl"
                {
                    self.pending_apply_edl_snapshot =
                        Some((id.clone(), self.timeline.snapshot()));
                }
                // On a successful apply_edl ToolResult, refresh the
                // timeline pane and emit a chat-pane diff crumb.
                if let SessionEvent::ToolResult {
                    id,
                    name,
                    result: Ok(_),
                } = &ev
                    && name == "apply_edl"
                {
                    self.timeline.refresh();
                    if let Some((snap_id, before)) =
                        self.pending_apply_edl_snapshot.take()
                        && &snap_id == id
                    {
                        let diff =
                            crate::timeline::diff_snapshots(&before, &self.timeline.snapshot());
                        self.chat.push_diff(diff);
                    }
                }
                // On a failed apply_edl ToolResult, drop the snapshot so
                // the next call doesn't reuse stale state.
                if let SessionEvent::ToolResult {
                    id,
                    name,
                    result: Err(_),
                } = &ev
                    && name == "apply_edl"
                    && let Some((snap_id, _)) = &self.pending_apply_edl_snapshot
                    && snap_id == id
                {
                    self.pending_apply_edl_snapshot = None;
                }
                self.chat.apply_session_event(ev);
                true
            }
            AppEvent::Approval(req) => {
                self.chat.push_approval_pending(&req);
                self.modal = Some(ApprovalModal::new(req));
                true
            }
            AppEvent::UserInput(req) => {
                self.chat.push_user_input_pending(&req);
                self.pending_user_input = Some(req);
                true
            }
            AppEvent::TerminalEventError(msg) => {
                warn!("terminal event error: {msg}");
                false
            }
            AppEvent::Quit => {
                self.quit = true;
                false
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Global: Ctrl-C cancels in-flight turn or quits when idle.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(cancel) = self.turn_cancel.take() {
                cancel.cancel();
                if let Some(t) = self.turn_task.take() {
                    t.abort();
                }
                self.chat
                    .push_error("turn cancelled".to_string());
                return true;
            }
            self.quit = true;
            return false;
        }
        if key.code == KeyCode::Char('d')
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && self.composer.is_empty()
        {
            self.quit = true;
            return false;
        }

        // Modal capture: when an approval modal is open, all keys go to it.
        if let Some(modal) = self.modal.as_mut()
            && let Some((decision, crumb)) = modal.handle_key(key)
        {
            let modal = self.modal.take().expect("just borrowed");
            let tool_name = modal.tool_name().to_string();
            let _ = modal.resolve(decision);
            self.chat.resolve_approval(&tool_name, crumb);
            return true;
        }
        if self.modal.is_some() {
            return true;
        }

        // Composer routing for Enter / arrows.
        if key.code == KeyCode::Enter {
            if let Some(text) = self.composer.submit() {
                // If the agent is waiting on user-input, this submission
                // answers that oneshot rather than starting a new turn.
                if let Some(req) = self.pending_user_input.take() {
                    self.chat.push_user(text.clone());
                    let _ = req.reply.send(text);
                    return true;
                }
                self.start_turn(text);
                return true;
            }
            return false;
        }

        self.composer.handle_key(key)
    }

    fn start_turn(&mut self, prompt: String) {
        // If a turn is already running, silently drop the new submission.
        // Spamming an error crumb on every keystroke during a slow turn
        // is worse than the implicit "your input was ignored" feel —
        // the composer text was already cleared by submit().
        if self.turn_task.is_some() {
            return;
        }
        self.chat.push_user(prompt.clone());
        let cancel = CancellationToken::new();
        self.turn_cancel = Some(cancel.clone());
        let session = self.session.clone();
        // When the spawned turn finishes, the session emits TurnEnd
        // through the broadcast — that's the App's signal to clear
        // turn_task so the next prompt can start a new turn. We do
        // that in handle_event below.
        self.turn_task = Some(tokio::spawn(async move {
            if let Err(e) = session.run_turn(prompt, cancel).await {
                debug!("turn ended with error: {e}");
            }
        }));
    }

    fn paint(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        // Step 1: flush any frozen chat items into scrollback above
        // the inline viewport. Each call to `insert_before` shifts the
        // viewport down on the user's terminal and prints the item to
        // the rolled-out region above.
        let history = self.chat.take_history();
        for item in history {
            let lines = crate::chat::render_item_for_history(&item);
            let height = lines.len() as u16;
            if height == 0 {
                continue;
            }
            // The width on insert_before's buffer matches the
            // terminal, so use that.
            let term_width = terminal.size().map(|s| s.width).unwrap_or(80);
            let _ = terminal.insert_before(height, |buf| {
                for (i, line) in lines.into_iter().enumerate() {
                    buf.set_line(0, i as u16, &line, term_width);
                }
            });
        }

        // Step 2: paint the live region (inline viewport).
        terminal
            .draw(|f| {
                let area = f.area();
                let timeline_height =
                    pick_timeline_height(area.height, self.timeline.clip_count());
                // chat (live items) → timeline → composer
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(0),                  // active items
                        Constraint::Length(timeline_height), // timeline
                        Constraint::Length(1),               // composer
                    ])
                    .split(area);
                f.render_widget(&self.chat, chunks[0]);
                if timeline_height > 0 {
                    f.render_widget(&self.timeline, chunks[1]);
                }
                f.render_widget(&self.composer, chunks[2]);
                if let Some(modal) = &self.modal {
                    f.render_widget(modal, area);
                }
            })
            .context("ratatui draw")?;
        Ok(())
    }
}

/// Pick a timeline height. The pane wants `1 header + clip_count`
/// rows. We give the timeline up to ~half the inline viewport,
/// reserving the rest for the live chat region + composer.
/// Returns 0 if there are no clips or no room.
fn pick_timeline_height(total_rows: u16, clip_count: usize) -> u16 {
    if total_rows < 6 || clip_count == 0 {
        return 0;
    }
    let want = clip_count.saturating_add(1) as u16; // header + clips
    // Leave at least 3 rows for the live chat region + 1 for composer.
    let max_allowed = total_rows.saturating_sub(4);
    want.min(max_allowed).max(2)
}

/// Initial inline-viewport height. Big enough for:
///   - composer (1 row)
///   - timeline header + ~6 clip rows (7 rows)
///   - a few rows for streaming text + spinner above
/// Total ≈ 14 rows. The viewport is anchored to the bottom of the
/// terminal so anything in the user's shell history above stays
/// visible — they scroll up with native terminal scroll.
const DEFAULT_VIEWPORT_ROWS: u16 = 14;

fn enter_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    // Inline-viewport mode (Codex pattern). We do NOT take over the
    // alt-screen; instead we own a small region at the bottom of the
    // user's existing terminal, and chat history is `insert_before`d
    // into the normal scrollback. Empty space above us belongs to
    // whatever shell history was already there — and the user can
    // scroll up with their terminal's native scroll buffer to see it.
    enable_raw_mode().context("enable raw mode")?;
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(DEFAULT_VIEWPORT_ROWS),
        },
    )
    .context("create ratatui Terminal")?;
    Ok(terminal)
}

fn leave_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    // Move the cursor below the viewport so the user's next shell
    // prompt doesn't overwrite our last frame.
    let _ = terminal.clear();
    let _ = disable_raw_mode();
    println!();
    Ok(())
}

fn spawn_terminal_pump(tx: mpsc::UnboundedSender<AppEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut stream = EventStream::new();
        while let Some(next) = stream.next().await {
            match next {
                Ok(CtEvent::Key(k)) => {
                    if tx.send(AppEvent::Key(k)).is_err() {
                        break;
                    }
                }
                Ok(CtEvent::Mouse(m)) => {
                    if tx.send(AppEvent::Mouse(m)).is_err() {
                        break;
                    }
                }
                Ok(CtEvent::Resize(w, h)) => {
                    if tx.send(AppEvent::Resize { width: w, height: h }).is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    let _ = tx.send(AppEvent::TerminalEventError(e.to_string()));
                }
            }
        }
    })
}

fn spawn_session_pump(
    mut rx: tokio::sync::broadcast::Receiver<SessionEvent>,
    tx: mpsc::UnboundedSender<AppEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if tx.send(AppEvent::Session(ev)).is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    })
}

fn spawn_approval_pump(
    mut rx: mpsc::Receiver<ApprovalRequest>,
    tx: mpsc::UnboundedSender<AppEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            if tx.send(AppEvent::Approval(req)).is_err() {
                return;
            }
        }
    })
}

fn spawn_user_input_pump(
    mut rx: mpsc::Receiver<UserInputRequest>,
    tx: mpsc::UnboundedSender<AppEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            if tx.send(AppEvent::UserInput(req)).is_err() {
                return;
            }
        }
    })
}

fn spawn_tick_pump(tx: mpsc::UnboundedSender<AppEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            interval.tick().await;
            if tx.send(AppEvent::Tick).is_err() {
                return;
            }
        }
    })
}

/// Convenience: a one-line "denied" decision so `ApprovalDecision::Deny`
/// stays linked even if other places stop using it.
#[allow(dead_code)]
const _DEFAULT_DENY: ApprovalDecision = ApprovalDecision::Deny;

#[cfg(test)]
mod tests {
    //! App-level event-handling tests.
    //!
    //! These bypass the full `run()` loop (which needs a real TTY) and
    //! drive `handle_event` directly. The Session field is constructed
    //! against a bogus API key — we never call `start_turn`, so the
    //! client is never used.
    use super::*;
    use awidat_core::ToolRegistry;
    use awidat_core::anthropic::{Client, ClientConfig};
    use awidat_core::tool::{ApprovalDecision, ApprovalRequest, UserInputRequest};
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use tokio::sync::oneshot;

    fn make_app() -> App {
        let client = Client::new("test-key", ClientConfig::default()).expect("client");
        let project_root = std::env::temp_dir();
        let session = Arc::new(Session::new(
            client,
            ToolRegistry::new(),
            "claude-haiku-4-5-20251001",
            None,
            project_root.clone(),
        ));
        App {
            session,
            chat: Chat::new(),
            timeline: Timeline::new(&project_root),
            composer: Composer::new("hint"),
            modal: None,
            pending_user_input: None,
            turn_cancel: None,
            turn_task: None,
            pending_apply_edl_snapshot: None,
            quit: false,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::CONTROL, KeyEventKind::Press)
    }

    #[test]
    fn approval_request_opens_modal_and_chat_crumb() {
        let mut app = make_app();
        let (tx, _rx) = oneshot::channel::<ApprovalDecision>();
        let req = ApprovalRequest {
            call_id: "c1".into(),
            tool_name: "apply_edl".into(),
            args_summary: "{\"edl\":\"...\"}".into(),
            reply: tx,
        };
        let mutated = app.handle_event(AppEvent::Approval(req));
        assert!(mutated);
        assert!(app.modal.is_some());
        // ApprovalPending crumbs live in pending_history, ready to be
        // flushed into terminal scrollback by the next paint.
        assert!(matches!(
            app.chat.pending_history().last().unwrap(),
            crate::chat::ChatItem::ApprovalPending { .. }
        ));
    }

    #[tokio::test]
    async fn approval_modal_default_selection_is_deny_for_safety() {
        let mut app = make_app();
        let (tx, rx) = oneshot::channel::<ApprovalDecision>();
        let req = ApprovalRequest {
            call_id: "c1".into(),
            tool_name: "apply_edl".into(),
            args_summary: "summary".into(),
            reply: tx,
        };
        app.handle_event(AppEvent::Approval(req));
        assert!(app.modal.is_some());
        // A bare Enter (e.g., user inertia) confirms the default. The
        // safe default is Deny — the user must explicitly arrow up or
        // press 1/2 to approve.
        app.handle_event(AppEvent::Key(key(KeyCode::Enter)));
        assert!(app.modal.is_none(), "modal should clear after decision");
        assert_eq!(rx.await.unwrap(), ApprovalDecision::Deny);
    }

    #[tokio::test]
    async fn approval_modal_arrow_up_then_enter_allows() {
        let mut app = make_app();
        let (tx, rx) = oneshot::channel::<ApprovalDecision>();
        let req = ApprovalRequest {
            call_id: "c1".into(),
            tool_name: "apply_edl".into(),
            args_summary: "x".into(),
            reply: tx,
        };
        app.handle_event(AppEvent::Approval(req));
        // From Deny (idx 2): up → AllowForSession (1), up → Allow (0).
        app.handle_event(AppEvent::Key(key(KeyCode::Up)));
        app.handle_event(AppEvent::Key(key(KeyCode::Up)));
        app.handle_event(AppEvent::Key(key(KeyCode::Enter)));
        assert_eq!(rx.await.unwrap(), ApprovalDecision::Allow);
    }

    #[tokio::test]
    async fn approval_modal_esc_routes_to_deny() {
        let mut app = make_app();
        let (tx, rx) = oneshot::channel::<ApprovalDecision>();
        let req = ApprovalRequest {
            call_id: "c1".into(),
            tool_name: "bash".into(),
            args_summary: "rm -rf /".into(),
            reply: tx,
        };
        app.handle_event(AppEvent::Approval(req));
        app.handle_event(AppEvent::Key(key(KeyCode::Esc)));
        assert!(app.modal.is_none());
        assert_eq!(rx.await.unwrap(), ApprovalDecision::Deny);
    }

    #[tokio::test]
    async fn approval_modal_2_routes_to_session_allow() {
        let mut app = make_app();
        let (tx, rx) = oneshot::channel::<ApprovalDecision>();
        let req = ApprovalRequest {
            call_id: "c1".into(),
            tool_name: "start_render".into(),
            args_summary: "scope=preview".into(),
            reply: tx,
        };
        app.handle_event(AppEvent::Approval(req));
        app.handle_event(AppEvent::Key(key(KeyCode::Char('2'))));
        assert!(app.modal.is_none());
        assert_eq!(rx.await.unwrap(), ApprovalDecision::AllowForSession);
    }

    #[tokio::test]
    async fn user_input_request_routes_next_enter_to_oneshot() {
        let mut app = make_app();
        let (tx, rx) = oneshot::channel::<String>();
        let req = UserInputRequest {
            call_id: "c1".into(),
            question: "trim or delete?".into(),
            options: None,
            default: None,
            reply: tx,
        };
        app.handle_event(AppEvent::UserInput(req));
        assert!(app.pending_user_input.is_some());
        // Type "delete" + Enter.
        for ch in "delete".chars() {
            app.handle_event(AppEvent::Key(key(KeyCode::Char(ch))));
        }
        app.handle_event(AppEvent::Key(key(KeyCode::Enter)));
        assert!(app.pending_user_input.is_none());
        assert_eq!(rx.await.unwrap(), "delete");
        // Should NOT have started a turn — no in-flight task.
        assert!(app.turn_task.is_none());
    }

    #[test]
    fn ctrl_d_on_empty_composer_quits() {
        let mut app = make_app();
        app.handle_event(AppEvent::Key(ctrl(KeyCode::Char('d'))));
        assert!(app.quit);
    }

    #[test]
    fn ctrl_d_with_text_does_not_quit() {
        let mut app = make_app();
        app.handle_event(AppEvent::Key(key(KeyCode::Char('a'))));
        app.handle_event(AppEvent::Key(ctrl(KeyCode::Char('d'))));
        assert!(!app.quit);
    }

    #[test]
    fn ctrl_c_with_no_turn_quits() {
        let mut app = make_app();
        app.handle_event(AppEvent::Key(ctrl(KeyCode::Char('c'))));
        assert!(app.quit);
    }

    #[test]
    fn session_event_renders_into_chat() {
        let mut app = make_app();
        let mutated = app.handle_event(AppEvent::Session(SessionEvent::TextDelta(
            "hello".into(),
        )));
        assert!(mutated);
        assert!(matches!(
            app.chat.items().last().unwrap(),
            crate::chat::ChatItem::Assistant(_)
        ));
    }

    #[test]
    fn apply_edl_call_snapshots_timeline_before_dispatch() {
        let mut app = make_app();
        app.handle_event(AppEvent::Session(SessionEvent::ToolCallStart {
            id: "c1".into(),
            name: "apply_edl".into(),
        }));
        assert!(
            app.pending_apply_edl_snapshot.is_some(),
            "apply_edl ToolCallStart should snapshot the timeline"
        );
        let (id, _rows) = app.pending_apply_edl_snapshot.as_ref().unwrap();
        assert_eq!(id, "c1");
    }

    #[test]
    fn apply_edl_failure_clears_snapshot_no_diff_emitted() {
        let mut app = make_app();
        app.handle_event(AppEvent::Session(SessionEvent::ToolCallStart {
            id: "c1".into(),
            name: "apply_edl".into(),
        }));
        app.handle_event(AppEvent::Session(SessionEvent::ToolResult {
            id: "c1".into(),
            name: "apply_edl".into(),
            result: Err("anchor miss".into()),
        }));
        assert!(app.pending_apply_edl_snapshot.is_none(), "snapshot cleared");
        // No Diff item in chat — only a ToolCall whose status is Failed.
        let has_diff = app
            .chat
            .items()
            .iter()
            .any(|i| matches!(i, crate::chat::ChatItem::Diff(_)));
        assert!(!has_diff, "no diff crumb on failure");
    }

    #[test]
    fn non_apply_edl_tool_does_not_snapshot() {
        let mut app = make_app();
        app.handle_event(AppEvent::Session(SessionEvent::ToolCallStart {
            id: "c1".into(),
            name: "view_timeline".into(),
        }));
        assert!(
            app.pending_apply_edl_snapshot.is_none(),
            "view_timeline should not snapshot"
        );
    }
}
