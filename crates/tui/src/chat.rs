//! Chat-pane state and render.
//!
//! The pane is a scrollable list of `ChatItem`s. Each item knows how to
//! render itself into a sequence of `Line`s. We deliberately don't
//! implement Codex's commit-tick streaming controller (chunking +
//! catch-up modes, ~700 lines of policy code) — for v1 we append model
//! deltas to the trailing assistant item and re-render. If flicker
//! shows up under heavy streaming we'll port the queue-and-tick drain
//! pattern then.
//!
//! Auto-scroll behavior: stick to the bottom unless the user has
//! scrolled up. We track that via a single bool the app sets when it
//! sees a PageUp/Down.

use awidat_core::SessionEvent;
use awidat_core::tool::{ApprovalRequest, PlanItem, UserInputRequest};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};

/// One row in the chat scrollback. Each variant renders to one or more
/// lines depending on width.
#[derive(Debug, Clone)]
pub enum ChatItem {
    /// User-typed prompt.
    User(String),
    /// Streamed model text. Mutated in-place as deltas arrive.
    Assistant(String),
    /// Tool call the agent issued. Pending until the matching result
    /// arrives.
    ToolCall {
        /// Tool name.
        name: String,
        /// Tool call id (used to match the result).
        call_id: String,
        /// Args, if they parsed.
        args_summary: Option<String>,
        /// Set on the matching ToolResult.
        status: ToolStatus,
    },
    /// Plan snapshot from `update_plan`.
    Plan {
        /// Plan steps.
        items: Vec<PlanItem>,
        /// Optional one-sentence note from the agent.
        note: Option<String>,
    },
    /// Approval prompt for a mutating tool. Cleared when the user replies.
    ApprovalPending {
        /// Tool name (e.g. "apply_edl").
        tool_name: String,
        /// Args summary the loop pre-built.
        args_summary: String,
    },
    /// "user denied" / "user allowed" decision crumb.
    ApprovalResolved {
        /// Tool name.
        tool_name: String,
        /// Decision text ("allowed", "allowed for session", "denied").
        decision: String,
    },
    /// `request_user_input` prompt from the agent.
    UserInputPending {
        /// Question to display.
        question: String,
    },
    /// Turn-fatal error. Distinguished visually from a per-tool error.
    Error(String),
    /// Timeline diff produced after a successful apply_edl. Each string
    /// is one change line ("− clip-1 deleted", "~ clip-0 trimmed
    /// 10.00s → 5.00s", etc.).
    Diff(Vec<String>),
    /// "turn ended" crumb between turns.
    TurnEnded,
}

/// Lifecycle state of a tool call as we paint it.
#[derive(Debug, Clone)]
pub enum ToolStatus {
    /// Args streaming or running.
    Running,
    /// Tool returned `Ok(content)`. Content is preview-truncated to
    /// keep the chat pane readable.
    Done(String),
    /// Tool returned `Err(message)` (model sees `is_error: true`).
    Failed(String),
}

/// Chat pane state.
///
/// Two queues:
///
/// - `items` — **active** items currently rendered in the inline
///   viewport (a streaming Assistant message, a Running tool call).
///   These are *transient* — once they finish, they get drained into
///   `pending_history` and a new render cycle pushes them above the
///   viewport via `Terminal::insert_before`.
/// - `pending_history` — items the App's render loop has not yet
///   flushed to the terminal scrollback. Drained on each paint.
///
/// This matches the Codex pattern: short live region + scrollable
/// shell history above. The user's normal terminal scroll buffer is
/// the chat history.
#[derive(Debug, Default)]
pub struct Chat {
    items: Vec<ChatItem>,
    pending_history: Vec<ChatItem>,
    /// Animation phase for spinners (incremented on each Tick).
    spinner_phase: u8,
}

impl Chat {
    /// Empty chat.
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            pending_history: Vec::new(),
            spinner_phase: 0,
        }
    }

    /// Items currently *live* in the viewport (streaming text,
    /// in-flight tool calls).
    pub fn items(&self) -> &[ChatItem] {
        &self.items
    }

    /// Total number of rendered rows the live region needs. Used by
    /// the App's layout to size the chat region exactly to its
    /// content — so an empty live region collapses to zero rows
    /// instead of reserving blank space.
    pub fn rendered_height(&self) -> u16 {
        if self.items.is_empty() {
            return 0;
        }
        let mut total = 0usize;
        for (i, item) in self.items.iter().enumerate() {
            if i > 0 {
                total += 1; // blank separator row
            }
            total += render_item(item, 0).len();
        }
        u16::try_from(total).unwrap_or(u16::MAX)
    }

    /// Items waiting to be flushed into terminal scrollback.
    /// Returns a reference; callers should call [`Chat::take_history`]
    /// after rendering them via `Terminal::insert_before`.
    pub fn pending_history(&self) -> &[ChatItem] {
        &self.pending_history
    }

    /// Drain the pending-history queue. Caller is responsible for
    /// rendering each item via `Terminal::insert_before` *before*
    /// calling this — the queue is consumed.
    pub fn take_history(&mut self) -> Vec<ChatItem> {
        std::mem::take(&mut self.pending_history)
    }

    /// Move all pending-history items to the front of `items` so the
    /// chat pane renders them as ordered scrollback. Used in
    /// full-screen alt-screen mode where the chat pane owns its own
    /// scrollback (no terminal `insert_before`).
    pub fn fold_history_into_items(&mut self) {
        if self.pending_history.is_empty() {
            return;
        }
        let mut combined = std::mem::take(&mut self.pending_history);
        combined.extend(std::mem::take(&mut self.items));
        self.items = combined;
    }

    /// Append a user prompt — commits straight to history (it's frozen
    /// the moment it's submitted).
    pub fn push_user(&mut self, text: impl Into<String>) {
        self.commit_active_assistant();
        self.pending_history.push(ChatItem::User(text.into()));
    }

    /// Append a free-form error.
    pub fn push_error(&mut self, msg: impl Into<String>) {
        self.commit_active_assistant();
        self.pending_history.push(ChatItem::Error(msg.into()));
    }

    /// Append a diff crumb. Frozen on arrival.
    pub fn push_diff(&mut self, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        self.commit_active_assistant();
        self.pending_history.push(ChatItem::Diff(lines));
    }

    /// Append an approval prompt crumb. Frozen.
    pub fn push_approval_pending(&mut self, req: &ApprovalRequest) {
        self.commit_active_assistant();
        self.pending_history.push(ChatItem::ApprovalPending {
            tool_name: req.tool_name.clone(),
            args_summary: req.args_summary.clone(),
        });
    }

    /// Replace the most recent ApprovalPending crumb with a resolved
    /// decision crumb. Looks in pending_history (the more common case
    /// since approvals are usually still in the flush queue).
    pub fn resolve_approval(&mut self, tool_name: &str, decision: &str) {
        for item in self.pending_history.iter_mut().rev() {
            if let ChatItem::ApprovalPending { tool_name: t, .. } = item
                && t == tool_name
            {
                *item = ChatItem::ApprovalResolved {
                    tool_name: tool_name.into(),
                    decision: decision.into(),
                };
                return;
            }
        }
        self.pending_history.push(ChatItem::ApprovalResolved {
            tool_name: tool_name.into(),
            decision: decision.into(),
        });
    }

    /// Append a user-input prompt crumb.
    pub fn push_user_input_pending(&mut self, req: &UserInputRequest) {
        self.commit_active_assistant();
        self.pending_history.push(ChatItem::UserInputPending {
            question: req.question.clone(),
        });
    }

    /// If an Assistant item is currently streaming in `items`, freeze
    /// it: move into `pending_history`. Called whenever a non-text
    /// event arrives so the streaming text gets out of the way.
    fn commit_active_assistant(&mut self) {
        if let Some(idx) = self
            .items
            .iter()
            .position(|i| matches!(i, ChatItem::Assistant(_)))
        {
            self.pending_history.push(self.items.remove(idx));
        }
    }

    /// Apply a SessionEvent. Items that are already "frozen" (user
    /// prompts, finished tool calls, errors, plans, diffs) go directly
    /// into `pending_history` for the App to flush into terminal
    /// scrollback. Items that are still in-flight (streaming
    /// Assistant text, Running tool calls) live in `items`.
    pub fn apply_session_event(&mut self, ev: SessionEvent) {
        match ev {
            SessionEvent::TurnStart | SessionEvent::MessageStart { .. } => {}
            SessionEvent::TextDelta(s) => self.append_assistant_delta(&s),
            SessionEvent::ToolCallStart { id, name } => {
                // The arrival of a tool call ends the current Assistant
                // streaming chunk — commit that text.
                self.commit_active_assistant();
                self.items.push(ChatItem::ToolCall {
                    name,
                    call_id: id,
                    args_summary: None,
                    status: ToolStatus::Running,
                });
            }
            SessionEvent::ToolCallArgs { id, args, .. } => {
                if let Some(item) = self
                    .items
                    .iter_mut()
                    .rev()
                    .find(|i| matches!(i, ChatItem::ToolCall { call_id, .. } if call_id == &id))
                    && let ChatItem::ToolCall { args_summary, .. } = item
                {
                    *args_summary = Some(short_json(&args));
                }
            }
            SessionEvent::ToolResult { id, result, .. } => {
                // Find the matching Running tool call in `items`,
                // mark Done/Failed, and graduate it to history.
                if let Some(idx) = self
                    .items
                    .iter()
                    .position(|i| matches!(i, ChatItem::ToolCall { call_id, .. } if call_id == &id))
                {
                    let mut item = self.items.remove(idx);
                    if let ChatItem::ToolCall { status, .. } = &mut item {
                        *status = match result {
                            Ok(out) => ToolStatus::Done(preview(&out)),
                            Err(e) => ToolStatus::Failed(preview(&e)),
                        };
                    }
                    self.pending_history.push(item);
                }
            }
            SessionEvent::SamplingComplete { .. } => {}
            SessionEvent::TurnEnd => {
                // Commit any straggling assistant text, then drop a
                // turn-ended marker so users can see boundaries.
                self.commit_active_assistant();
                self.pending_history.push(ChatItem::TurnEnded);
            }
            SessionEvent::Error(msg) => self.push_error(msg),
            SessionEvent::EditPlanUpdate { items, note } => {
                self.commit_active_assistant();
                self.pending_history.push(ChatItem::Plan { items, note });
            }
            SessionEvent::AwaitingUserInput { question, .. } => {
                self.commit_active_assistant();
                self.pending_history
                    .push(ChatItem::UserInputPending { question });
            }
        }
    }

    /// Tick the spinner animation.
    pub fn tick(&mut self) {
        self.spinner_phase = self.spinner_phase.wrapping_add(1);
    }

    fn append_assistant_delta(&mut self, delta: &str) {
        if let Some(ChatItem::Assistant(buf)) = self.items.last_mut() {
            buf.push_str(delta);
        } else {
            self.items.push(ChatItem::Assistant(delta.to_string()));
        }
    }
}

/// Build the `Line`s for one chat item. Multi-line so heavy items
/// (plan, tool result) get their own block.
fn render_item(item: &ChatItem, spinner_phase: u8) -> Vec<Line<'static>> {
    match item {
        // Single-glyph role marks. No "you/awidat" labels — every line
        // gets too noisy. Magenta › for the user (matches the composer
        // glyph) and a soft white left-margin for the assistant.
        ChatItem::User(text) => vec![Line::from(vec![
            Span::styled("› ", Style::default().fg(Color::Magenta)),
            Span::raw(text.clone()),
        ])],
        ChatItem::Assistant(text) => vec![Line::from(vec![
            Span::raw("  "),
            Span::raw(text.clone()),
        ])],
        ChatItem::ToolCall {
            name,
            args_summary,
            status,
            ..
        } => {
            let mut lines = Vec::new();
            let glyph = match status {
                ToolStatus::Running => spinner_glyph(spinner_phase),
                ToolStatus::Done(_) => "✓",
                ToolStatus::Failed(_) => "✗",
            };
            let glyph_style = match status {
                ToolStatus::Running => Style::default().fg(Color::Yellow),
                ToolStatus::Done(_) => Style::default().fg(Color::Green),
                ToolStatus::Failed(_) => Style::default().fg(Color::Red),
            };
            let mut head = vec![
                Span::styled(format!("{glyph} "), glyph_style),
                Span::styled(name.clone(), Style::default().fg(Color::Cyan)),
            ];
            if let Some(args) = args_summary {
                head.push(Span::styled(
                    format!(" {args}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            lines.push(Line::from(head));
            match status {
                // Tool results: one-line summary derived from the
                // tool's output type. Anything richer goes in the
                // chat scrollback only on demand (future feature).
                ToolStatus::Done(out) => {
                    if let Some(summary) = summarize_tool_result(name, out) {
                        lines.push(Line::from(vec![
                            Span::raw("    "),
                            Span::styled(summary, Style::default().fg(Color::DarkGray)),
                        ]));
                    }
                }
                ToolStatus::Failed(err) => {
                    lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(
                            truncate_one_line(err, 120),
                            Style::default().fg(Color::Red),
                        ),
                    ]));
                }
                ToolStatus::Running => {}
            }
            lines
        }
        ChatItem::Plan { items, note } => {
            let mut lines = vec![Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    "plan",
                    Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
                ),
            ])];
            for it in items {
                let glyph = match it.status.as_str() {
                    "completed" => "✓",
                    "in_progress" => "▶",
                    _ => "·",
                };
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(format!("{glyph} "), Style::default().fg(Color::Blue)),
                    Span::raw(it.step.clone()),
                ]));
            }
            if let Some(n) = note {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(n.clone(), Style::default().fg(Color::DarkGray)),
                ]));
            }
            lines
        }
        // ApprovalPending exists in the scrollback as a quiet crumb;
        // the actual modal is what blocks the user. Don't shout.
        ChatItem::ApprovalPending {
            tool_name,
            args_summary,
        } => vec![Line::from(vec![
            Span::styled("? ", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("approve {tool_name}"),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(
                format!(" {}", truncate_one_line(args_summary, 80)),
                Style::default().fg(Color::DarkGray),
            ),
        ])],
        ChatItem::ApprovalResolved {
            tool_name,
            decision,
        } => vec![Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{tool_name}: {decision}"),
                Style::default().fg(Color::DarkGray),
            ),
        ])],
        ChatItem::UserInputPending { question } => vec![Line::from(vec![
            Span::styled("? ", Style::default().fg(Color::Cyan)),
            Span::raw(question.clone()),
        ])],
        ChatItem::Error(msg) => vec![Line::from(vec![
            Span::styled("✗ ", Style::default().fg(Color::Red)),
            Span::styled(msg.clone(), Style::default().fg(Color::Red)),
        ])],
        ChatItem::Diff(change_lines) => {
            let mut lines = Vec::new();
            for cl in change_lines {
                let style = if cl.starts_with('−') {
                    Style::default().fg(Color::Red)
                } else if cl.starts_with('+') {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
                };
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(cl.clone(), style),
                ]));
            }
            lines
        }
        ChatItem::TurnEnded => vec![Line::from(Span::styled(
            "─ end of turn ─",
            Style::default().fg(Color::DarkGray),
        ))],
    }
}

fn spinner_glyph(phase: u8) -> &'static str {
    const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
    FRAMES[(phase as usize) % FRAMES.len()]
}

fn preview(s: &str) -> String {
    const CAP: usize = 400;
    if s.chars().count() <= CAP {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(CAP).collect();
        format!("{truncated}…")
    }
}

fn truncate_one_line(s: &str, cap: usize) -> String {
    let single = s.replace('\n', " ");
    if single.chars().count() <= cap {
        single
    } else {
        let truncated: String = single.chars().take(cap).collect();
        format!("{truncated}…")
    }
}

/// Tool-call args as a short blurb shown next to the tool name. We
/// special-case `apply_edl` — its `edl` field is a multiline freeform
/// string and showing the JSON-escaped form of it is unreadable noise.
/// Everything else falls through to a compact JSON one-liner.
fn short_json(v: &serde_json::Value) -> String {
    const CAP: usize = 80;
    if let Some(edl) = v.get("edl").and_then(|e| e.as_str()) {
        // Pull the operator (the *** Verb Clip line) out of the EDL so
        // the reader sees "Delete Clip" not the whole envelope.
        let op = edl
            .lines()
            .find(|l| l.starts_with("*** ") && !l.contains("Begin EDL") && !l.contains("End EDL"))
            .map(|l| l.trim_start_matches("*** ").trim().to_string())
            .unwrap_or_else(|| "edit".into());
        return op;
    }
    let raw: String = v
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if raw == "{}" {
        return String::new();
    }
    if raw.chars().count() <= CAP {
        raw
    } else {
        let truncated: String = raw.chars().take(CAP).collect();
        format!("{truncated}…")
    }
}

/// One-line summary of a tool's text output. Each tool gets a hand-
/// picked extractor — the alternative is showing 200 chars of raw text
/// which clutters the scroll. Returns `None` to mean "nothing useful;
/// don't print a body line at all".
fn summarize_tool_result(name: &str, out: &str) -> Option<String> {
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return None;
    }
    match name {
        "view_timeline" => trimmed
            .lines()
            .next()
            .map(|l| truncate_one_line(l, 120)),
        "find_moment" => {
            // JSON: { "results": [...], "more_available": ... }.
            let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
            let n = v.get("results").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0);
            if n == 0 {
                Some("no matches".into())
            } else {
                let first_snippet = v
                    .get("results")
                    .and_then(|r| r.as_array())
                    .and_then(|a| a.first())
                    .and_then(|h| h.get("snippet"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                Some(truncate_one_line(
                    &format!("{n} match{} · \"{first_snippet}\"", if n == 1 { "" } else { "es" }),
                    120,
                ))
            }
        }
        "apply_edl" => trimmed
            .lines()
            .next()
            .map(|l| truncate_one_line(l, 120)),
        "list_assets" | "read_index" | "inspect_clip" => {
            Some(format!("{} char body", trimmed.chars().count()))
        }
        "view_frame" => Some("frame attached".into()),
        "start_render" | "poll_render" => trimmed
            .lines()
            .next()
            .map(|l| truncate_one_line(l, 120)),
        "bash" => trimmed
            .lines()
            .next()
            .map(|l| truncate_one_line(l, 120)),
        _ => Some(truncate_one_line(trimmed, 120)),
    }
}

impl Widget for &Chat {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Active items only — the live region. Frozen items have been
        // moved to `pending_history` and printed into terminal
        // scrollback already (above the inline viewport).
        if self.items.is_empty() {
            return;
        }
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (i, item) in self.items.iter().enumerate() {
            if i > 0 {
                lines.push(Line::from(""));
            }
            lines.extend(render_item(item, self.spinner_phase));
        }
        // Tail-bias: if there are more lines than the viewport can
        // hold, show the *last* viewport-height lines (most recent
        // streaming text + spinner).
        let total = lines.len() as u16;
        let scroll_y = total.saturating_sub(area.height);
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll_y, 0))
            .render(area, buf);
    }
}

/// Render one chat item to lines for `Terminal::insert_before` to
/// push into the terminal scrollback. Borrowed by the App on each
/// `flush_history` pass.
pub fn render_item_for_history(item: &ChatItem) -> Vec<Line<'static>> {
    // Spinner phase is irrelevant in scrollback (the item is frozen).
    render_item(item, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_deltas_concat() {
        let mut c = Chat::new();
        c.apply_session_event(SessionEvent::TextDelta("hello ".into()));
        c.apply_session_event(SessionEvent::TextDelta("world".into()));
        match c.items().last().unwrap() {
            ChatItem::Assistant(s) => assert_eq!(s, "hello world"),
            _ => panic!("expected Assistant item"),
        }
    }

    #[test]
    fn tool_lifecycle_running_to_done() {
        // After ToolResult arrives the call is graduated from `items`
        // (live region) into `pending_history` (ready for scrollback).
        let mut c = Chat::new();
        c.apply_session_event(SessionEvent::ToolCallStart {
            id: "c1".into(),
            name: "view_timeline".into(),
        });
        c.apply_session_event(SessionEvent::ToolCallArgs {
            id: "c1".into(),
            name: "view_timeline".into(),
            args: serde_json::json!({}),
        });
        // While Running, the call lives in items.
        assert!(matches!(
            c.items().last().unwrap(),
            ChatItem::ToolCall { status: ToolStatus::Running, .. }
        ));
        c.apply_session_event(SessionEvent::ToolResult {
            id: "c1".into(),
            name: "view_timeline".into(),
            result: Ok("clip-0\nclip-1".into()),
        });
        assert!(c.items().is_empty(), "tool call moves to history");
        match c.pending_history().last().unwrap() {
            ChatItem::ToolCall {
                status: ToolStatus::Done(_),
                ..
            } => {}
            _ => panic!("expected ToolCall in history"),
        }
    }

    #[test]
    fn tool_failure_records_error() {
        let mut c = Chat::new();
        c.apply_session_event(SessionEvent::ToolCallStart {
            id: "c1".into(),
            name: "bash".into(),
        });
        c.apply_session_event(SessionEvent::ToolResult {
            id: "c1".into(),
            name: "bash".into(),
            result: Err("denied".into()),
        });
        match c.pending_history().last().unwrap() {
            ChatItem::ToolCall {
                status: ToolStatus::Failed(e),
                ..
            } => assert_eq!(e, "denied"),
            _ => panic!("expected failed ToolCall in history"),
        }
    }

    #[test]
    fn user_input_pushes_to_history() {
        // Smoke: prove user prompts go straight to history.
        let mut c = Chat::new();
        c.push_user("hi");
        assert!(c.items().is_empty());
        assert_eq!(c.pending_history().len(), 1);
    }

    #[test]
    fn take_history_drains_queue() {
        let mut c = Chat::new();
        c.push_user("a");
        c.push_user("b");
        let drained = c.take_history();
        assert_eq!(drained.len(), 2);
        assert!(c.pending_history().is_empty(), "drained");
    }
}
