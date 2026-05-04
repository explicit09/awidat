//! The TUI event loop. Owns the terminal, the chat state, the
//! composer, the optional approval modal, and merges three async
//! sources (terminal events, agent broadcast, approval channel) into
//! one `AppEvent` queue.
//!
//! Lifecycle:
//!
//! 1. Caller hands us a `Session` and the receivers from
//!    `subscribe()` / `with_approval_channel`.
//! 2. We enter the alternate screen, build a custom full-screen
//!    `Terminal`, and drive the loop.
//! 3. On Quit (Ctrl-C, Ctrl-D, `:q`) we restore the terminal.

use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use awidat_core::tool::{ApprovalDecision, ApprovalRequest, UserInputRequest};
use awidat_core::{Session, SessionEvent};
use crossterm::event::{Event as CtEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::approval::ApprovalModal;
use crate::chat::Chat;
use crate::composer::Composer;
use crate::custom_terminal::Terminal;
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
    /// Optional pre-filled first user turn. When set, the composer
    /// starts with this text + the cursor at the end; the user can
    /// edit it or hit enter to submit. Used by `awidat skills run
    /// <name>` to stage a "use the X skill" prompt.
    pub initial_prompt: Option<String>,
}

/// The TUI app.
pub struct App {
    session: Arc<Session>,
    project_label: String,
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
    /// What we know about the project's indexer state at session
    /// start. Read once in `App::new`; only the welcome card consumes
    /// it (and the welcome card only renders before any prompt is
    /// submitted), so a fixed-at-startup snapshot is fine. Re-running
    /// indexers via shellout from inside the TUI doesn't refresh this
    /// — by then the user has been past the welcome screen for a
    /// while anyway.
    insights: crate::project_insights::ProjectInsights,
    quit: bool,
}

impl App {
    /// Build a fresh app from the config.
    pub fn new(cfg: &AppConfig) -> Self {
        let project_root = cfg.session.project_root().to_path_buf();
        let insights = crate::project_insights::ProjectInsights::gather(&project_root);
        Self {
            session: cfg.session.clone(),
            project_label: project_root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| project_root.to_str().unwrap_or("project"))
                .to_string(),
            chat: Chat::new(),
            timeline: Timeline::new(&project_root),
            composer: match cfg.initial_prompt.as_deref() {
                Some(text) => Composer::with_text("ask awidat anything…", text),
                None => Composer::new("ask awidat anything…"),
            },
            modal: None,
            pending_user_input: None,
            turn_cancel: None,
            turn_task: None,
            pending_apply_edl_snapshot: None,
            insights,
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
                    self.pending_apply_edl_snapshot = Some((id.clone(), self.timeline.snapshot()));
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
                    if let Some((snap_id, before)) = self.pending_apply_edl_snapshot.take()
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
                self.chat.push_error("turn cancelled".to_string());
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
        if let Ok(screen) = terminal.size() {
            let full_screen = Rect {
                x: 0,
                y: 0,
                width: screen.width,
                height: screen.height,
            };
            if terminal.viewport_area != full_screen {
                terminal.set_viewport_area(full_screen);
            }
        }
        self.chat.fold_history_into_items();

        terminal
            .draw(|f| {
                let area = f.area();
                let idle = self.chat.is_empty()
                    && self.turn_task.is_none()
                    && self.modal.is_none()
                    && self.pending_user_input.is_none();

                let composer_area = if idle && area.height >= 14 {
                    self.render_idle(f, area)
                } else {
                    let area = if area.height >= 10 {
                        self.render_active_header(f, area)
                    } else {
                        area
                    };
                    let (chat_area, composer_area, status_area) =
                        active_content_areas(area, self.chat.rendered_height());
                    f.render_widget(&self.chat, chat_area);
                    f.render_widget(self.status_line(), status_area);
                    composer_area
                };

                f.render_widget(&self.composer, composer_area);
                let composer_row = composer_area.y + composer_area.height / 2;
                let cursor_x = composer_area
                    .x
                    .saturating_add(self.composer.cursor_column())
                    .min(composer_area.right().saturating_sub(1));
                f.set_cursor_position(Position {
                    x: cursor_x,
                    y: composer_row,
                });
                if let Some(modal) = &self.modal {
                    f.render_widget(modal, area);
                }
            })
            .context("custom terminal draw")?;
        Ok(())
    }

    fn render_idle(&self, f: &mut crate::custom_terminal::Frame<'_>, area: Rect) -> Rect {
        let card_x = area.x + if area.width >= 80 { 2 } else { 1 };
        let card_y = area.y + top_gutter(area);
        let available_width = area.right().saturating_sub(card_x + 1);
        let card_width = available_width.min(96).max(44);
        // Card height = 2 (border) + content rows. Welcome card now
        // shows 5 rows per column: title, status, project, tools,
        // indexed-state. The right column adds an editorial-moments
        // summary when that index has run, otherwise mirrors with a
        // coachmark suggestion that exercises the live data.
        let card = Rect {
            x: card_x.min(area.right().saturating_sub(1)),
            y: card_y,
            width: card_width,
            height: 9,
        };

        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(198, 123, 88)))
            .title(format!(" Awidat TUI v{} ", crate::version()))
            .title_style(
                Style::default()
                    .fg(Color::Rgb(198, 123, 88))
                    .add_modifier(Modifier::BOLD),
            );
        let inner = card_block.inner(card);
        f.render_widget(card_block, card);

        // Vertical split: top 4 rows = two-column layout, bottom row =
        // full-width "indexed: ..." line. Pulling that out of the
        // narrow left column was needed because the indexer list runs
        // 60–80 chars (whisper + topic + editorial-moments + 5 vision
        // names + separators) — clipped at 32 chars in the column it
        // truncated to "edior" mid-word, which a real run surfaced.
        let card_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Length(1)])
            .split(inner);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(32), Constraint::Min(20)])
            .split(card_rows[0]);

        // Left column: identity + status (4 rows).
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("Awidat TUI v{}", crate::version()),
                        Style::default()
                            .fg(Color::Rgb(198, 123, 88))
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "ready to edit",
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled("project: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(self.project_label.clone(), Style::default().fg(Color::Gray)),
                ]),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled("tools: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        self.session.tool_count().to_string(),
                        Style::default().fg(Color::Cyan),
                    ),
                ]),
            ]),
            columns[0],
        );

        // Right column: editorial entry-points. The "moments:" line
        // surfaces the editorial-moments roll-up when present —
        // counts of hooks/punchlines/etc. — so the user knows
        // immediately what's worth asking about. When the moments
        // index hasn't run, we show a coachmark instead.
        //
        // Width-aware: the right column's width depends on the
        // terminal. We pass the available budget (column width
        // minus the "moments " prefix label, ~8 chars) to the
        // insights helper so it can degrade gracefully — drop
        // the "N other" suffix, then drop kind labels right-to-
        // left, until it fits. No mid-word truncation.
        let moments_budget = columns[1].width.saturating_sub(8);
        let moments_line = match self
            .insights
            .welcome_moments_line_for_width(moments_budget)
        {
            Some(line) => Line::from(vec![
                Span::styled("moments ", Style::default().fg(Color::DarkGray)),
                Span::styled(line, Style::default().fg(Color::Cyan)),
            ]),
            None => Line::from(vec![
                Span::styled("moments ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "(run editorial-moments indexer to populate)",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
        };
        // Right column: 4 rows. We dropped one example prompt to
        // make room for the editorial-moments summary, since the
        // moments roll-up actually changes per project (live data
        // signal) and the example prompts are static. Pick the
        // example based on what's been indexed: vision-tool prompt
        // when clip-mcp ran, beat prompt otherwise — both are
        // proven Cursor-moments from the real-video session.
        let example_prompt = if !self.insights.vision_indexers.is_empty() {
            "show frames where someone is holding a phone"
        } else if !self.insights.editorial_indexers.is_empty() {
            "find the strongest hooks and compose a 30s reel"
        } else {
            "drop a video under raw/ and run `awidat index`"
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![Span::styled(
                    "Start with a direct editing request",
                    Style::default()
                        .fg(Color::Rgb(198, 123, 88))
                        .add_modifier(Modifier::BOLD),
                )]),
                Line::from(vec![
                    Span::styled("timeline ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!(
                            "{} clip{} · {:.2}s",
                            self.timeline.clip_count(),
                            if self.timeline.clip_count() == 1 {
                                ""
                            } else {
                                "s"
                            },
                            self.timeline.total_duration_s()
                        ),
                        Style::default().fg(Color::Gray),
                    ),
                ]),
                moments_line,
                Line::from(vec![
                    Span::styled("try ", Style::default().fg(Color::DarkGray)),
                    Span::styled(example_prompt, Style::default().fg(Color::Gray)),
                ]),
            ]),
            columns[1],
        );

        // Full-width "indexed: ..." row. Pulled out of the narrow
        // left column so the indexer name list (whisper · topic ·
        // editorial-moments · clip · face · shot · gaze ·
        // frame-quality, ~80 chars) doesn't get truncated.
        let indexed_line = match self.insights.welcome_indexers_line() {
            Some(line) => Line::from(vec![
                Span::raw("  "),
                Span::styled("indexed: ", Style::default().fg(Color::DarkGray)),
                Span::styled(line, Style::default().fg(Color::Green)),
            ]),
            None => Line::from(vec![
                Span::raw("  "),
                Span::styled("indexed: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "(none yet — run `awidat index <project>` to build the brain)",
                    Style::default().fg(Color::Yellow),
                ),
            ]),
        };
        f.render_widget(Paragraph::new(indexed_line), card_rows[1]);

        let tip_y = card.bottom().saturating_add(2);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Tip: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    "mutating tools ask for approval; Ctrl-C cancels a running turn.",
                    Style::default().fg(Color::Gray),
                ),
            ])),
            Rect {
                x: card.x,
                y: tip_y,
                width: card.width,
                height: 1,
            },
        );

        let composer_y = tip_y.saturating_add(2);
        let composer = Rect {
            x: area.x + 1,
            y: composer_y.min(area.bottom().saturating_sub(4)),
            width: area.width.saturating_sub(2),
            height: 3,
        };
        let footer_y = composer
            .y
            .saturating_add(2)
            .min(area.bottom().saturating_sub(1));
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!(
                        "awidat · {} · {} clip{}",
                        self.project_label,
                        self.timeline.clip_count(),
                        if self.timeline.clip_count() == 1 {
                            ""
                        } else {
                            "s"
                        }
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ])),
            Rect {
                x: area.x + 1,
                y: footer_y,
                width: area.width.saturating_sub(2),
                height: 1,
            },
        );
        composer
    }

    fn render_active_header(&self, f: &mut crate::custom_terminal::Frame<'_>, area: Rect) -> Rect {
        let card_x = area.x + if area.width >= 80 { 2 } else { 1 };
        let card_y = area.y + top_gutter(area);
        let available_width = area.right().saturating_sub(card_x + 1);
        let card_width = available_width.min(72).max(38);
        let card_height = if area.width >= 72 { 6 } else { 7 };
        let card = Rect {
            x: card_x.min(area.right().saturating_sub(1)),
            y: card_y,
            width: card_width,
            height: card_height.min(area.height.saturating_sub(1)).max(1),
        };

        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(format!(" Awidat TUI v{} ", crate::version()))
            .title_style(
                Style::default()
                    .fg(Color::Rgb(198, 123, 88))
                    .add_modifier(Modifier::BOLD),
            );
        let inner = card_block.inner(card);
        f.render_widget(card_block, card);

        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("Awidat TUI v{}", crate::version()),
                        Style::default()
                            .fg(Color::Rgb(198, 123, 88))
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled("project: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        self.project_label.clone(),
                        Style::default()
                            .fg(Color::Gray)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled("timeline: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!(
                            "{} clip{} · {:.2}s",
                            self.timeline.clip_count(),
                            if self.timeline.clip_count() == 1 {
                                ""
                            } else {
                                "s"
                            },
                            self.timeline.total_duration_s()
                        ),
                        Style::default().fg(Color::Gray),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled("tools: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        self.session.tool_count().to_string(),
                        Style::default().fg(Color::Cyan),
                    ),
                ]),
            ]),
            inner,
        );

        let content_y = card.bottom().saturating_add(2);
        Rect {
            x: area.x + 1,
            y: content_y.min(area.bottom()),
            width: area.width.saturating_sub(2),
            height: area.bottom().saturating_sub(content_y),
        }
    }

    fn status_line(&self) -> Paragraph<'static> {
        let status = if self.modal.is_some() {
            "approval"
        } else if self.pending_user_input.is_some() {
            "waiting"
        } else if self.turn_task.is_some() {
            "running"
        } else {
            "ready"
        };
        Paragraph::new(Line::from(vec![
            Span::styled(
                " awidat ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{} ", self.project_label),
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                format!(
                    "| {} clip{} | {:.2}s | ",
                    self.timeline.clip_count(),
                    if self.timeline.clip_count() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    self.timeline.total_duration_s()
                ),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(status, Style::default().fg(Color::Yellow)),
        ]))
    }
}

fn top_gutter(area: Rect) -> u16 {
    if area.height >= 24 {
        3
    } else if area.height >= 14 {
        2
    } else {
        1
    }
}

fn active_content_areas(area: Rect, desired_chat_height: u16) -> (Rect, Rect, Rect) {
    const COMPOSER_HEIGHT: u16 = 3;
    const STATUS_HEIGHT: u16 = 1;

    let max_chat_height = area.height.saturating_sub(COMPOSER_HEIGHT + STATUS_HEIGHT);
    let chat_height = desired_chat_height.max(1).min(max_chat_height.max(1));
    let gap = if chat_height < max_chat_height
        && chat_height + 1 + COMPOSER_HEIGHT + STATUS_HEIGHT <= area.height
    {
        1
    } else {
        0
    };
    let composer_y = area.y + chat_height + gap;
    let composer_height = COMPOSER_HEIGHT.min(area.bottom().saturating_sub(composer_y));
    let status_y = composer_y + composer_height;
    let status_height = STATUS_HEIGHT.min(area.bottom().saturating_sub(status_y));

    (
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: chat_height.min(area.height),
        },
        Rect {
            x: area.x,
            y: composer_y,
            width: area.width,
            height: composer_height,
        },
        Rect {
            x: area.x,
            y: status_y,
            width: area.width,
            height: status_height,
        },
    )
}

fn enter_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    if let Err(e) = execute!(stdout, EnterAlternateScreen, Clear(ClearType::All)) {
        let _ = disable_raw_mode();
        return Err(e).context("enter alternate screen");
    }
    stdout.flush().context("flush alternate screen setup")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::with_options_and_cursor_position(backend, Position::ORIGIN)
        .context("create custom Terminal")?;

    let screen = terminal
        .size()
        .unwrap_or(ratatui::layout::Size::new(80, 24));
    terminal.set_viewport_area(Rect {
        x: 0,
        y: 0,
        width: screen.width,
        height: screen.height,
    });
    Ok(terminal)
}

fn leave_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let _ = terminal.show_cursor();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.backend_mut().flush();
    let _ = disable_raw_mode();
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
                    if tx
                        .send(AppEvent::Resize {
                            width: w,
                            height: h,
                        })
                        .is_err()
                    {
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
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    // Surface lag as a visible Error event rather
                    // than silently `continue`. Silent drops were
                    // the prime suspect for the "TUI silently went
                    // unresponsive" report on the 44-min real-video
                    // session: if the paint loop ever did fall behind
                    // by enough to drop deltas, the user would see
                    // a partial transcript and have no signal that
                    // anything went wrong. Now they get a tracing
                    // line + an inline error in the chat pane.
                    tracing::warn!(dropped = n, "TUI session pump lagged; some events lost");
                    let _ = tx.send(AppEvent::Session(SessionEvent::Error(format!(
                        "TUI fell behind by {n} event(s); some streaming output may be missing. \
                         Increase the broadcast buffer in session.rs if this persists."
                    ))));
                }
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
            project_label: "test".into(),
            chat: Chat::new(),
            timeline: Timeline::new(&project_root),
            composer: Composer::new("hint"),
            modal: None,
            pending_user_input: None,
            turn_cancel: None,
            turn_task: None,
            pending_apply_edl_snapshot: None,
            insights: crate::project_insights::ProjectInsights::gather(&project_root),
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
        let mutated = app.handle_event(AppEvent::Session(SessionEvent::TextDelta("hello".into())));
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
