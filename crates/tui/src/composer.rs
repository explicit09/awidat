//! Input box the user types into.
//!
//! Single-line for v1 (Crush starts single-line and grows; we'll match
//! that trajectory). No history navigation, no slash-command popup —
//! those land in v1.5 once the rest is real. We do honor the basic
//! editing keys: backspace, delete, left/right arrow, home/end.
//!
//! The composer renders a `Paragraph` with a top border so the user
//! sees a clear "you type here" affordance below the chat scrollback.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

/// One-line composer state.
#[derive(Debug, Default)]
pub struct Composer {
    /// Current text the user has typed.
    text: String,
    /// Cursor position (chars from left, 0-indexed).
    cursor: usize,
    /// Visible hint when text is empty (e.g. "ask awidat anything…").
    placeholder: String,
}

impl Composer {
    /// Build a composer with the given placeholder text.
    pub fn new(placeholder: impl Into<String>) -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            placeholder: placeholder.into(),
        }
    }

    /// Current input.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the composer has any text.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Cursor column inside the composer render area.
    pub fn cursor_column(&self) -> u16 {
        let byte_idx = self.byte_idx_for_char(self.cursor);
        let input_width = self.text[..byte_idx].width();
        u16::try_from(2 + input_width).unwrap_or(u16::MAX)
    }

    /// Drain the current input and reset to empty. Returns `None` if the
    /// composer was empty.
    pub fn submit(&mut self) -> Option<String> {
        if self.text.is_empty() {
            return None;
        }
        let out = std::mem::take(&mut self.text);
        self.cursor = 0;
        Some(out)
    }

    /// Apply one key event. Returns `true` iff the input changed (so the
    /// app can decide whether to schedule a repaint).
    ///
    /// The composer does NOT handle Enter — the parent app intercepts
    /// Enter and decides whether to call `submit()` (we want submission
    /// to be explicit, not buried in a key handler).
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match (key.code, key.modifiers) {
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => {
                self.insert_char(c);
                true
            }
            (KeyCode::Backspace, _) => self.backspace(),
            (KeyCode::Delete, _) => self.delete_forward(),
            (KeyCode::Left, _) => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                false
            }
            (KeyCode::Right, _) => {
                if self.cursor < self.text.chars().count() {
                    self.cursor += 1;
                }
                false
            }
            (KeyCode::Home, _) => {
                self.cursor = 0;
                false
            }
            (KeyCode::End, _) => {
                self.cursor = self.text.chars().count();
                false
            }
            _ => false,
        }
    }

    fn insert_char(&mut self, c: char) {
        let byte_idx = self.byte_idx_for_char(self.cursor);
        self.text.insert(byte_idx, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let prev_char_byte = self.byte_idx_for_char(self.cursor - 1);
        let cur_byte = self.byte_idx_for_char(self.cursor);
        self.text.drain(prev_char_byte..cur_byte);
        self.cursor -= 1;
        true
    }

    fn delete_forward(&mut self) -> bool {
        let count = self.text.chars().count();
        if self.cursor >= count {
            return false;
        }
        let cur_byte = self.byte_idx_for_char(self.cursor);
        let next_byte = self.byte_idx_for_char(self.cursor + 1);
        self.text.drain(cur_byte..next_byte);
        true
    }

    fn byte_idx_for_char(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(b, _)| b)
    }
}

impl Widget for &Composer {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }
        let input_row = if area.height > 1 {
            area.y + area.height / 2
        } else {
            area.y
        };
        let input_area = Rect {
            x: area.x,
            y: input_row,
            width: area.width,
            height: 1,
        };
        Block::default()
            .style(Style::default().bg(Color::Rgb(45, 45, 45)))
            .render(input_area, buf);

        let line = if self.text.is_empty() {
            Line::from(vec![
                Span::styled(
                    "› ",
                    Style::default()
                        .fg(Color::Gray)
                        .bg(Color::Rgb(45, 45, 45))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    self.placeholder.clone(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .bg(Color::Rgb(45, 45, 45)),
                ),
            ])
        } else {
            Line::from(vec![
                Span::styled(
                    "› ",
                    Style::default()
                        .fg(Color::Magenta)
                        .bg(Color::Rgb(45, 45, 45))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    self.text.clone(),
                    Style::default().bg(Color::Rgb(45, 45, 45)),
                ),
            ])
        };
        Paragraph::new(line).render(input_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Press)
    }

    #[test]
    fn insert_and_submit() {
        let mut c = Composer::new("hint");
        for ch in "hello".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(c.text(), "hello");
        assert_eq!(c.submit(), Some("hello".into()));
        assert!(c.is_empty());
    }

    #[test]
    fn backspace_in_middle() {
        let mut c = Composer::new("hint");
        for ch in "abc".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        c.handle_key(key(KeyCode::Left));
        c.handle_key(key(KeyCode::Backspace));
        assert_eq!(c.text(), "ac");
    }

    #[test]
    fn submit_on_empty_returns_none() {
        let mut c = Composer::new("hint");
        assert_eq!(c.submit(), None);
    }

    #[test]
    fn unicode_handling() {
        let mut c = Composer::new("hint");
        for ch in "héllo".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        c.handle_key(key(KeyCode::Backspace));
        c.handle_key(key(KeyCode::Backspace));
        assert_eq!(c.text(), "hél");
    }

    #[test]
    fn cursor_column_accounts_for_prefix_and_text_width() {
        let mut c = Composer::new("hint");
        for ch in "hé".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(c.cursor_column(), 4);
    }
}
