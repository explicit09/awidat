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
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

    /// Build a composer pre-filled with `initial`. Used by
    /// `awidat skills run <name>` to stage the first user turn.
    /// Cursor is placed at the end so the user can edit or just
    /// hit enter to submit.
    pub fn with_text(placeholder: impl Into<String>, initial: impl Into<String>) -> Self {
        let text: String = initial.into();
        let cursor = text.chars().count();
        Self {
            text,
            cursor,
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

    /// Cursor column inside the composer render area, accounting for
    /// soft-wrap at `inner_width` (text-area width inside the
    /// composer's frame, NOT including the "› " prefix). Returns
    /// (column_in_text_area, row_offset_from_first_text_row).
    /// The 2-cell prefix is added by the caller.
    pub fn cursor_position(&self, inner_width: u16) -> (u16, u16) {
        let inner_width = inner_width.max(1) as usize;
        let byte_idx = self.byte_idx_for_char(self.cursor);
        let before = &self.text[..byte_idx];
        let mut col: usize = 0;
        let mut row: usize = 0;
        for ch in before.chars() {
            if ch == '\n' {
                row += 1;
                col = 0;
                continue;
            }
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if col + w > inner_width {
                row += 1;
                col = 0;
            }
            col += w;
        }
        (col as u16, row as u16)
    }

    /// Cursor position inside the visible composer area after the input
    /// has scrolled to keep the cursor in view.
    pub fn visible_cursor_position(&self, inner_width: u16, visible_height: u16) -> (u16, u16) {
        let (col, row) = self.cursor_position(inner_width);
        let scroll_y = self.scroll_y(inner_width, visible_height);
        (col, row.saturating_sub(scroll_y))
    }

    /// Legacy single-row cursor column used by the old call site
    /// (see TUI #2). Computes the column AS IF the input were one line.
    /// Kept so existing tests and callers that don't know about wrap
    /// keep compiling.
    pub fn cursor_column(&self) -> u16 {
        let byte_idx = self.byte_idx_for_char(self.cursor);
        let input_width = self.text[..byte_idx].width();
        u16::try_from(2 + input_width).unwrap_or(u16::MAX)
    }

    /// Number of rows this composer would occupy when rendered at
    /// `total_width` (border + padding included). Soft-wraps long lines
    /// and respects literal newlines from a multi-line paste. Capped
    /// at MAX_ROWS to keep the composer from eating the chat pane.
    pub fn desired_height(&self, total_width: u16) -> u16 {
        const MAX_ROWS: u16 = 6;
        const FRAME_OVERHEAD: u16 = 2; // border-ish padding
        // The "› " prefix occupies 2 cells of the first wrap segment.
        let inner_width = total_width.saturating_sub(2).max(1) as usize;
        let text = if self.text.is_empty() {
            self.placeholder.as_str()
        } else {
            self.text.as_str()
        };
        let mut row_count: u16 = 1;
        let mut col: usize = 0;
        for ch in text.chars() {
            if ch == '\n' {
                row_count = row_count.saturating_add(1);
                col = 0;
                continue;
            }
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            if col + w > inner_width {
                row_count = row_count.saturating_add(1);
                col = 0;
            }
            col += w;
        }
        (row_count + FRAME_OVERHEAD).min(MAX_ROWS)
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

    /// Insert a (possibly multi-line) string at the cursor without
    /// submitting. Called from the bracketed-paste path
    /// (`AppEvent::Paste`). Newlines are preserved in `self.text` so
    /// the user's pasted multi-line input stays intact and they
    /// submit explicitly with Enter.
    pub fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let byte_idx = self.byte_idx_for_char(self.cursor);
        self.text.insert_str(byte_idx, s);
        self.cursor += s.chars().count();
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

    fn scroll_y(&self, inner_width: u16, visible_height: u16) -> u16 {
        if visible_height == 0 {
            return 0;
        }
        let (_, row) = self.cursor_position(inner_width);
        row.saturating_add(1).saturating_sub(visible_height)
    }
}

impl Widget for &Composer {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        // Paint the composer background across the whole allotted area
        // so it reads as one box even when text wraps.
        Block::default()
            .style(Style::default().bg(Color::Rgb(45, 45, 45)))
            .render(area, buf);

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
        // `trim: false` preserves user-typed leading whitespace.
        Paragraph::new(line)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .scroll((self.scroll_y(area.width.saturating_sub(2), area.height), 0))
            .render(area, buf);
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

    #[test]
    fn insert_str_preserves_newlines_without_submitting() {
        // Regression for bug #1 (TUI #160): pasted multi-line text
        // used to fragment turns at every \n. Bracketed-paste path
        // calls insert_str, which keeps newlines as literal chars and
        // does NOT submit.
        let mut c = Composer::new("hint");
        c.insert_str("line one\nline two\nline three");
        assert_eq!(c.text(), "line one\nline two\nline three");
        assert!(!c.is_empty());
    }

    #[test]
    fn insert_str_at_cursor_position() {
        let mut c = Composer::new("hint");
        for ch in "ABCD".chars() {
            c.handle_key(key(KeyCode::Char(ch)));
        }
        c.handle_key(key(KeyCode::Left));
        c.handle_key(key(KeyCode::Left));
        // Cursor between B and C.
        c.insert_str("XYZ");
        assert_eq!(c.text(), "ABXYZCD");
    }

    #[test]
    fn insert_str_handles_unicode() {
        let mut c = Composer::new("hint");
        c.insert_str("café\n你好");
        assert_eq!(c.text(), "café\n你好");
    }

    #[test]
    fn desired_height_grows_with_explicit_newlines() {
        // 3 lines of pasted text → composer needs 3 text rows + frame
        // overhead, capped at MAX_ROWS=6.
        let mut c = Composer::new("hint");
        c.insert_str("a\nb\nc");
        // total_width=80 → 3 text rows → 5 with overhead
        assert_eq!(c.desired_height(80), 5);
    }

    #[test]
    fn desired_height_grows_when_text_overflows_width() {
        let mut c = Composer::new("hint");
        // Single 200-char line at width 50 wraps to 4 rows of text.
        c.insert_str(&"x".repeat(200));
        // inner_width = 50 - 2 = 48, 200/48 = 5 rows → 5 + 2 overhead
        // = 7, capped at MAX_ROWS=6
        assert_eq!(c.desired_height(50), 6);
    }

    #[test]
    fn desired_height_caps_at_max() {
        let mut c = Composer::new("hint");
        c.insert_str(&"line\n".repeat(20));
        assert!(c.desired_height(80) <= 6, "must respect MAX_ROWS cap");
    }

    #[test]
    fn cursor_position_tracks_wrap() {
        let mut c = Composer::new("hint");
        // 25 chars at inner_width=10 → wraps to row 2 col 5
        // (rows 0–9, 10–19 fill; chars 20..24 land on row 2).
        c.insert_str(&"x".repeat(25));
        let (col, row) = c.cursor_position(10);
        assert_eq!(row, 2);
        assert_eq!(col, 5);
    }

    #[test]
    fn cursor_position_respects_explicit_newlines() {
        let mut c = Composer::new("hint");
        c.insert_str("ab\ncd");
        let (col, row) = c.cursor_position(80);
        assert_eq!(row, 1);
        assert_eq!(col, 2);
    }

    #[test]
    fn visible_cursor_position_scrolls_capped_composer() {
        let mut c = Composer::new("hint");
        c.insert_str(&"x".repeat(55));
        let (col, row) = c.visible_cursor_position(10, 3);

        assert_eq!(col, 5);
        assert_eq!(row, 2);
        assert_eq!(c.scroll_y(10, 3), 3);
    }
}
