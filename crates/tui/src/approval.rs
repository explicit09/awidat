//! Approval modal: Allow / Allow for Session / Deny.
//!
//! Renders as a centered overlay over the chat pane while an
//! `ApprovalRequest` is in flight. Keyboard:
//!
//! - `↑`/`↓` (or `j`/`k`): move selection
//! - `Enter`: confirm
//! - `Esc`: deny
//! - `1`/`2`/`3`: jump to choice
//!
//! Three-way matches `ApprovalDecision`. When the user picks, the app
//! sends the decision through the `oneshot::Sender` carried by the
//! request and clears the modal.

use awidat_core::tool::{ApprovalDecision, ApprovalRequest};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

/// Approval modal state.
pub struct ApprovalModal {
    request: ApprovalRequest,
    /// Index into [`CHOICES`].
    selected: usize,
}

const CHOICES: &[(ApprovalDecision, &str, &str)] = &[
    (ApprovalDecision::Allow, "Allow", "1"),
    (ApprovalDecision::AllowForSession, "Allow for Session", "2"),
    (ApprovalDecision::Deny, "Deny", "3"),
];

impl ApprovalModal {
    /// Build a modal around an in-flight approval request.
    ///
    /// **Default selection is Deny** — the safety choice. If a user
    /// happens to press Enter (e.g., from inertia after submitting their
    /// prompt) while the modal is opening, the agent's mutating call is
    /// blocked rather than rubber-stamped. To approve, the user has to
    /// move the selection up explicitly or press 1 / 2.
    pub fn new(request: ApprovalRequest) -> Self {
        Self {
            request,
            // Index 2 in CHOICES = Deny.
            selected: 2,
        }
    }

    /// Tool name being asked about (handy for the chat-pane crumb).
    pub fn tool_name(&self) -> &str {
        &self.request.tool_name
    }

    /// Apply a key event. Returns `Some((decision, text))` when the user
    /// has confirmed a decision; the caller is expected to consume the
    /// modal (sending the decision into the embedded oneshot).
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<(ApprovalDecision, &'static str)> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < CHOICES.len() {
                    self.selected += 1;
                }
                None
            }
            KeyCode::Char('1') => Some(decision_for(0)),
            KeyCode::Char('2') => Some(decision_for(1)),
            KeyCode::Char('3') => Some(decision_for(2)),
            KeyCode::Esc => Some(decision_for(2)),
            KeyCode::Enter => Some(decision_for(self.selected)),
            _ => None,
        }
    }

    /// Consume the modal and send the decision back through the embedded
    /// reply channel. Returns the decision-text crumb for the chat log.
    pub fn resolve(self, decision: ApprovalDecision) -> &'static str {
        let _ = self.request.reply.send(decision);
        match decision {
            ApprovalDecision::Allow => "allowed",
            ApprovalDecision::AllowForSession => "allowed for session",
            ApprovalDecision::Deny => "denied",
        }
    }
}

fn decision_for(i: usize) -> (ApprovalDecision, &'static str) {
    let (d, _, _) = CHOICES[i];
    (
        d,
        match d {
            ApprovalDecision::Allow => "allowed",
            ApprovalDecision::AllowForSession => "allowed for session",
            ApprovalDecision::Deny => "denied",
        },
    )
}

impl Widget for &ApprovalModal {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let modal = centered_rect(70, 11, area);
        Clear.render(modal, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(" approve mutating tool? ")
            .title_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );
        let inner = block.inner(modal);
        block.render(modal, buf);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // tool + args
                Constraint::Length(1), // spacer
                Constraint::Min(3),    // choices
                Constraint::Length(1), // hint
            ])
            .split(inner);

        let header = vec![
            Line::from(vec![
                Span::styled(
                    self.request.tool_name.clone(),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    self.request.args_summary.clone(),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            Line::from(""),
        ];
        Paragraph::new(header)
            .wrap(Wrap { trim: false })
            .render(chunks[0], buf);

        let choices: Vec<Line<'static>> = CHOICES
            .iter()
            .enumerate()
            .map(|(i, (_, label, hotkey))| {
                let marker = if i == self.selected { "▶" } else { " " };
                let style = if i == self.selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::from(vec![
                    Span::styled(format!("{marker} "), style),
                    Span::styled(format!("[{hotkey}] "), Style::default().fg(Color::DarkGray)),
                    Span::styled((*label).to_string(), style),
                ])
            })
            .collect();
        Paragraph::new(choices).render(chunks[2], buf);

        Paragraph::new(Line::from(Span::styled(
            "↑/↓ select  ⏎ confirm  esc deny",
            Style::default().fg(Color::DarkGray),
        )))
        .render(chunks[3], buf);
    }
}

/// Centered rect of given (width, height) inside `area`.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}
