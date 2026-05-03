//! Custom Ratatui Terminal that **never re-probes the cursor**.
//!
//! Lifted (and trimmed) from
//! `harnesses/codex/codex-rs/tui/src/custom_terminal.rs`. The headline
//! difference vs. ratatui's stock `Terminal::with_options` /
//! `Viewport::Inline`:
//!
//! - Stock ratatui's `try_draw` calls `autoresize()` which calls
//!   `resize()` which calls `compute_inline_size()` which calls
//!   `backend.get_cursor_position()` — a synchronous query that waits
//!   up to **two seconds** for the terminal's DSR-CPR reply.
//! - On macOS Terminal under load that reply sometimes never arrives
//!   and the TUI bails with "cursor position could not be read within
//!   a normal duration".
//!
//! We instead probe the cursor **once** (via [`crate::terminal_probe`]
//! with a 100ms timeout) and track it ourselves in
//! `last_known_cursor_pos`. `autoresize()` only checks `backend.size()`
//! (a non-blocking ioctl); cursor position is updated based on what we
//! know we just drew.
//!
//! The drawing primitives (`diff_buffers`, `draw`, `ModifierDiff`)
//! mirror ratatui upstream and Codex's port — keeping them in-tree
//! means we own the wire-protocol byte budget and can reason about
//! the diff strategy when we want to.
//!
//! Skipped from Codex's port (we may need them later):
//! - OSC8 hyperlink awareness in `display_width` (we use UnicodeWidth
//!   directly; no hyperlinks in our chat for now).
//! - `clear_scrollback*`, `clear_visible_screen`, `invalidate_viewport`.
//! - Snapshot-test backends.

use std::io;
use std::io::Write;

use crossterm::cursor::{MoveTo, SetCursorStyle};
use crossterm::queue;
use derive_more::IsVariant;
use crossterm::style::{
    Attribute as CAttribute, Colors, Print, SetAttribute, SetBackgroundColor, SetColors,
    SetForegroundColor,
};
use crossterm::terminal::{Clear, ClearType as CtClearType};
use ratatui::backend::{Backend, ClearType};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use ratatui::style::{Color, Modifier};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

/// One frame's worth of state: where the cursor should land after the
/// frame is flushed, the visible cursor style, the viewport rect, and
/// the buffer the render closure draws into.
pub struct Frame<'a> {
    pub(crate) cursor_position: Option<Position>,
    cursor_style: SetCursorStyle,
    pub(crate) viewport_area: Rect,
    pub(crate) buffer: &'a mut Buffer,
}

impl Frame<'_> {
    /// The viewport rect this frame paints into.
    pub const fn area(&self) -> Rect {
        self.viewport_area
    }

    /// Render a widget into `area`. Mirrors stock ratatui's
    /// `Frame::render_widget` — accepts any `Widget` (consuming) and
    /// passes our buffer through.
    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    /// After the frame is flushed, place the cursor at `position` and
    /// make it visible.
    pub fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        self.cursor_position = Some(position.into());
    }

    /// After the frame is flushed, set the visible cursor style.
    pub fn set_cursor_style(&mut self, style: SetCursorStyle) {
        self.cursor_style = style;
    }

    /// Mutable handle to the underlying buffer (escape hatch for
    /// custom widgets).
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffer
    }
}

/// Inline-viewport terminal. Owns a backend, two ratatui buffers (one
/// current, one previous) and the viewport rectangle.
pub struct Terminal<B>
where
    B: Backend + Write,
{
    backend: B,
    buffers: [Buffer; 2],
    current: usize,
    /// Whether the cursor is currently hidden.
    pub hidden_cursor: bool,
    /// Area of the inline viewport in screen coordinates.
    pub viewport_area: Rect,
    /// Last known size of the screen. Used by autoresize to detect
    /// terminal-resize events without probing the cursor.
    pub last_known_screen_size: Size,
    /// Last known position of the cursor in screen coordinates. Used
    /// by `insert_history_lines` to position scrollback inserts above
    /// the viewport, and by `set_cursor_position` to keep our state
    /// in sync with what we just told the terminal.
    pub last_known_cursor_pos: Position,
    /// How many rows of history we've already pushed above the
    /// viewport. Capped at the viewport's `top()` so we never claim
    /// to have written above row 0.
    visible_history_rows: u16,
}

impl<B> Drop for Terminal<B>
where
    B: Backend + Write,
{
    fn drop(&mut self) {
        // Best-effort cursor restore. If this fails (e.g. the backend
        // is already closed) there's nothing useful to do.
        let _ = self.reset_cursor_style();
        if self.hidden_cursor {
            let _ = self.show_cursor();
        }
    }
}

impl<B> Terminal<B>
where
    B: Backend + Write,
{
    /// Build a Terminal seeded with a known cursor position. Use this
    /// after a bounded probe — passing the fallback you want first
    /// render to honor (e.g. `Position::ORIGIN` if the probe timed out).
    pub fn with_options_and_cursor_position(
        backend: B,
        cursor_pos: Position,
    ) -> io::Result<Self> {
        let screen_size = backend.size()?;
        Ok(Self::with_screen_size_and_cursor_position(
            backend,
            screen_size,
            cursor_pos,
        ))
    }

    fn with_screen_size_and_cursor_position(
        backend: B,
        screen_size: Size,
        cursor_pos: Position,
    ) -> Self {
        Self {
            backend,
            buffers: [Buffer::empty(Rect::ZERO), Buffer::empty(Rect::ZERO)],
            current: 0,
            hidden_cursor: false,
            viewport_area: Rect::new(/*x*/ 0, cursor_pos.y, /*width*/ 0, /*height*/ 0),
            last_known_screen_size: screen_size,
            last_known_cursor_pos: cursor_pos,
            visible_history_rows: 0,
        }
    }

    /// Get a Frame to paint into.
    pub fn get_frame(&mut self) -> Frame<'_> {
        Frame {
            cursor_position: None,
            cursor_style: SetCursorStyle::DefaultUserShape,
            viewport_area: self.viewport_area,
            buffer: self.current_buffer_mut(),
        }
    }

    fn current_buffer(&self) -> &Buffer {
        &self.buffers[self.current]
    }

    fn current_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    fn previous_buffer(&self) -> &Buffer {
        &self.buffers[1 - self.current]
    }

    fn previous_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[1 - self.current]
    }

    /// Borrow the backend (read-only).
    pub const fn backend(&self) -> &B {
        &self.backend
    }

    /// Borrow the backend (mutable).
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Diff the previous and current buffers and write the deltas to
    /// the backend. Updates `last_known_cursor_pos` from the rightmost
    /// `Put` command's coordinates.
    pub fn flush(&mut self) -> io::Result<()> {
        let updates = diff_buffers(self.previous_buffer(), self.current_buffer());
        let last_put = updates.iter().rev().find(|c| c.is_put());
        if let Some(&DrawCommand::Put { x, y, .. }) = last_put {
            self.last_known_cursor_pos = Position { x, y };
        }
        draw_commands(&mut self.backend, updates.into_iter())
    }

    /// Update the *recorded* screen size. The viewport rectangle is
    /// left alone — callers that want to reposition or resize the
    /// inline viewport call `set_viewport_area` directly. This is the
    /// key contract that lets `autoresize` skip a cursor re-probe.
    pub fn resize(&mut self, screen_size: Size) -> io::Result<()> {
        self.last_known_screen_size = screen_size;
        Ok(())
    }

    /// Replace the viewport rect. Resizes both buffers to match. Caps
    /// the history-rows counter so we don't claim to have rolled out
    /// content above the screen.
    pub fn set_viewport_area(&mut self, area: Rect) {
        self.current_buffer_mut().resize(area);
        self.previous_buffer_mut().resize(area);
        self.viewport_area = area;
        self.visible_history_rows = self.visible_history_rows.min(area.top());
    }

    /// Re-read screen size and call `resize` if it changed. Cursor is
    /// **not** re-probed — that's the whole point.
    pub fn autoresize(&mut self) -> io::Result<()> {
        let screen_size = self.backend.size()?;
        if screen_size != self.last_known_screen_size {
            self.resize(screen_size)?;
        }
        Ok(())
    }

    /// Draw a frame: autoresize → render closure → flush diff → set
    /// cursor → swap buffers. The render closure can't fail; for
    /// fallible callers see [`Terminal::try_draw`].
    pub fn draw<F>(&mut self, render_callback: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame),
    {
        self.try_draw(|frame| {
            render_callback(frame);
            io::Result::Ok(())
        })
    }

    /// Fallible draw — render closure may return an `io::Error`.
    pub fn try_draw<F, E>(&mut self, render_callback: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame) -> Result<(), E>,
        E: Into<io::Error>,
    {
        self.autoresize()?;

        let mut frame = self.get_frame();
        render_callback(&mut frame).map_err(Into::into)?;

        let cursor_position = frame.cursor_position;
        let cursor_style = frame.cursor_style;

        self.flush()?;

        match cursor_position {
            None => self.hide_cursor()?,
            Some(position) => {
                self.set_cursor_style(cursor_style)?;
                self.show_cursor()?;
                self.set_cursor_position(position)?;
            }
        }

        self.swap_buffers();
        Backend::flush(&mut self.backend)?;
        Ok(())
    }

    /// Hide the cursor.
    pub fn hide_cursor(&mut self) -> io::Result<()> {
        self.backend.hide_cursor()?;
        self.hidden_cursor = true;
        Ok(())
    }

    /// Show the cursor.
    pub fn show_cursor(&mut self) -> io::Result<()> {
        self.backend.show_cursor()?;
        self.hidden_cursor = false;
        Ok(())
    }

    /// Set the visible cursor shape (block, bar, etc.).
    pub fn set_cursor_style(&mut self, style: SetCursorStyle) -> io::Result<()> {
        queue!(self.backend, style)
    }

    /// Restore the user's configured cursor shape.
    pub fn reset_cursor_style(&mut self) -> io::Result<()> {
        self.set_cursor_style(SetCursorStyle::DefaultUserShape)
    }

    /// Move the cursor and update our internal record of where it is.
    pub fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        self.backend.set_cursor_position(position)?;
        self.last_known_cursor_pos = position;
        Ok(())
    }

    /// Clear the viewport and force a full redraw next frame.
    pub fn clear(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        self.clear_after_position(self.viewport_area.as_position())
    }

    /// Clear from `position` to the end of the visible screen and
    /// force a full redraw. Building block for `clear()` and friends.
    pub(crate) fn clear_after_position(&mut self, position: Position) -> io::Result<()> {
        self.backend.set_cursor_position(position)?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    /// Force a full repaint on the next draw by resetting the diff
    /// buffer. Use after operations that move screen content outside
    /// of ratatui's knowledge — e.g. inserting history lines via raw
    /// scroll, or after a terminal-side `clear` triggered by the user.
    pub fn invalidate_viewport(&mut self) {
        self.previous_buffer_mut().reset();
    }

    /// Clear terminal scrollback (if the terminal supports it) and
    /// force a full redraw. We pair it with explicit cursor-home
    /// motion because Terminal.app gets squirrelly otherwise.
    pub fn clear_scrollback(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        let home = Position { x: 0, y: 0 };
        self.set_cursor_position(home)?;
        queue!(self.backend, Clear(CtClearType::Purge))?;
        self.set_cursor_position(home)?;
        std::io::Write::flush(&mut self.backend)?;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    /// Clear the entire visible screen (not just the viewport).
    pub fn clear_visible_screen(&mut self) -> io::Result<()> {
        let home = Position { x: 0, y: 0 };
        // ED2 + explicit cursor-home before/after, matching the
        // common shell `clear` sequence (`CSI 2J` + `CSI H`).
        self.set_cursor_position(home)?;
        self.backend.clear_region(ClearType::All)?;
        self.set_cursor_position(home)?;
        std::io::Write::flush(&mut self.backend)?;
        self.visible_history_rows = 0;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    /// Hard-reset scrollback + visible screen via a single ANSI
    /// sequence. Some terminals respect this where the separate
    /// commands above don't.
    pub fn clear_scrollback_and_visible_screen_ansi(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        // Reset scroll region + style, home cursor, clear screen,
        // purge scrollback. Mirrors `clear && printf '\e[3J'`.
        write!(self.backend, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H")?;
        std::io::Write::flush(&mut self.backend)?;
        self.last_known_cursor_pos = Position { x: 0, y: 0 };
        self.visible_history_rows = 0;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    /// Number of rows we've pushed into the terminal scrollback above
    /// the viewport via `insert_history_lines`. Capped at the
    /// viewport's top.
    pub fn visible_history_rows(&self) -> u16 {
        self.visible_history_rows
    }

    /// Mark `inserted_rows` of history as having been written above
    /// the viewport. Used by `insert_history` after it shifts content
    /// up to make room.
    pub fn note_history_rows_inserted(&mut self, inserted_rows: u16) {
        self.visible_history_rows = self
            .visible_history_rows
            .saturating_add(inserted_rows)
            .min(self.viewport_area.top());
    }

    /// Swap current and previous buffers; reset the new "previous"
    /// (drawn) buffer so future diffs start from a clean slate.
    pub fn swap_buffers(&mut self) {
        self.previous_buffer_mut().reset();
        self.current = 1 - self.current;
    }

    /// Query the backend for the live screen size.
    pub fn size(&self) -> io::Result<Size> {
        self.backend.size()
    }
}

#[derive(Debug, IsVariant)]
enum DrawCommand {
    Put { x: u16, y: u16, cell: Cell },
    ClearToEnd { x: u16, y: u16, bg: Color },
}

/// Display width of a cell symbol, ignoring OSC escape sequences.
///
/// OSC sequences (e.g. OSC 8 hyperlinks: `\x1B]8;;URL\x07`) are
/// terminal control codes that don't consume display columns.
/// `UnicodeWidthStr::width()` would incorrectly count their printable
/// payload bytes (`]`, `8`, `;`, URL characters). We strip OSC
/// sequences first so only visible characters contribute.
fn display_width(s: &str) -> usize {
    if !s.contains('\x1B') {
        return s.width();
    }
    let mut visible = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1B' && chars.clone().next() == Some(']') {
            // Consume `]` and everything up to and including BEL.
            chars.next();
            for c in chars.by_ref() {
                if c == '\x07' {
                    break;
                }
            }
            continue;
        }
        visible.push(ch);
    }
    visible.width()
}

/// Diff the previous and next buffers, returning the minimum command
/// list needed to bring the screen up to date.
fn diff_buffers(a: &Buffer, b: &Buffer) -> Vec<DrawCommand> {
    let previous_buffer = &a.content;
    let next_buffer = &b.content;

    let mut updates: Vec<DrawCommand> = vec![];
    let mut last_nonblank_columns = vec![0_u16; a.area.height as usize];
    for y in 0..a.area.height {
        let row_start = y as usize * a.area.width as usize;
        let row_end = row_start + a.area.width as usize;
        let row = &next_buffer[row_start..row_end];
        let bg = row.last().map_or(Color::Reset, |c| c.bg);

        let mut last_nonblank_column = 0_usize;
        let mut column = 0_usize;
        while column < row.len() {
            let cell = &row[column];
            let width = display_width(cell.symbol());
            if cell.symbol() != " " || cell.bg != bg || cell.modifier != Modifier::empty() {
                last_nonblank_column = column + width.saturating_sub(1);
            }
            column += width.max(1);
        }

        if last_nonblank_column + 1 < row.len() {
            let (x, y) = a.pos_of(row_start + last_nonblank_column + 1);
            updates.push(DrawCommand::ClearToEnd { x, y, bg });
        }

        last_nonblank_columns[y as usize] = last_nonblank_column as u16;
    }

    let mut invalidated: usize = 0;
    let mut to_skip: usize = 0;
    for (i, (current, previous)) in next_buffer.iter().zip(previous_buffer.iter()).enumerate() {
        if !current.skip && (current != previous || invalidated > 0) && to_skip == 0 {
            let (x, y) = a.pos_of(i);
            let row = i / a.area.width as usize;
            if x <= last_nonblank_columns[row] {
                updates.push(DrawCommand::Put {
                    x,
                    y,
                    cell: next_buffer[i].clone(),
                });
            }
        }

        to_skip = display_width(current.symbol()).saturating_sub(1);

        let affected_width = std::cmp::max(
            display_width(current.symbol()),
            display_width(previous.symbol()),
        );
        invalidated = std::cmp::max(affected_width, invalidated).saturating_sub(1);
    }
    updates
}

/// Walk the command list and write the corresponding ANSI sequences
/// to `writer`.
fn draw_commands<I>(writer: &mut impl Write, commands: I) -> io::Result<()>
where
    I: Iterator<Item = DrawCommand>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut modifier = Modifier::empty();
    let mut last_pos: Option<Position> = None;
    for command in commands {
        let (x, y) = match command {
            DrawCommand::Put { x, y, .. } | DrawCommand::ClearToEnd { x, y, .. } => (x, y),
        };
        if !matches!(last_pos, Some(p) if x == p.x + 1 && y == p.y) {
            queue!(writer, MoveTo(x, y))?;
        }
        last_pos = Some(Position { x, y });
        match command {
            DrawCommand::Put { cell, .. } => {
                if cell.modifier != modifier {
                    let diff = ModifierDiff {
                        from: modifier,
                        to: cell.modifier,
                    };
                    diff.queue(writer)?;
                    modifier = cell.modifier;
                }
                if cell.fg != fg || cell.bg != bg {
                    queue!(
                        writer,
                        SetColors(Colors::new(cell.fg.into(), cell.bg.into()))
                    )?;
                    fg = cell.fg;
                    bg = cell.bg;
                }
                queue!(writer, Print(cell.symbol()))?;
            }
            DrawCommand::ClearToEnd { bg: clear_bg, .. } => {
                queue!(writer, SetAttribute(CAttribute::Reset))?;
                modifier = Modifier::empty();
                queue!(writer, SetBackgroundColor(clear_bg.into()))?;
                bg = clear_bg;
                queue!(writer, Clear(CtClearType::UntilNewLine))?;
            }
        }
    }
    queue!(
        writer,
        SetForegroundColor(crossterm::style::Color::Reset),
        SetBackgroundColor(crossterm::style::Color::Reset),
        SetAttribute(CAttribute::Reset),
    )?;
    Ok(())
}

/// Translate a Modifier diff (added/removed bits) into ANSI SGR
/// commands. Lifted verbatim from ratatui upstream.
struct ModifierDiff {
    from: Modifier,
    to: Modifier,
}

impl ModifierDiff {
    fn queue<W: Write>(self, w: &mut W) -> io::Result<()> {
        let removed = self.from - self.to;
        if removed.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CAttribute::NoReverse))?;
        }
        if removed.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(CAttribute::NormalIntensity))?;
            if self.to.contains(Modifier::DIM) {
                queue!(w, SetAttribute(CAttribute::Dim))?;
            }
        }
        if removed.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CAttribute::NoItalic))?;
        }
        if removed.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CAttribute::NoUnderline))?;
        }
        if removed.contains(Modifier::DIM) {
            queue!(w, SetAttribute(CAttribute::NormalIntensity))?;
        }
        if removed.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CAttribute::NotCrossedOut))?;
        }
        if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CAttribute::NoBlink))?;
        }

        let added = self.to - self.from;
        if added.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CAttribute::Reverse))?;
        }
        if added.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(CAttribute::Bold))?;
        }
        if added.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CAttribute::Italic))?;
        }
        if added.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CAttribute::Underlined))?;
        }
        if added.contains(Modifier::DIM) {
            queue!(w, SetAttribute(CAttribute::Dim))?;
        }
        if added.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CAttribute::CrossedOut))?;
        }
        if added.contains(Modifier::SLOW_BLINK) {
            queue!(w, SetAttribute(CAttribute::SlowBlink))?;
        }
        if added.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CAttribute::RapidBlink))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    #[test]
    fn diff_buffers_emits_no_puts_for_two_empty_buffers() {
        // Two empty buffers may emit ClearToEnd (covering whitespace
        // tail), but never a Put — there's nothing visible to render.
        let area = Rect::new(0, 0, 4, 1);
        let a = Buffer::empty(area);
        let b = Buffer::empty(area);
        let cmds = diff_buffers(&a, &b);
        assert!(
            !cmds.iter().any(|c| c.is_put()),
            "no Put commands expected for empty=>empty; got {cmds:?}"
        );
    }

    #[test]
    fn diff_buffers_emits_put_for_changed_cell() {
        let area = Rect::new(0, 0, 3, 1);
        let prev = Buffer::empty(area);
        let mut next = Buffer::empty(area);
        next.set_string(0, 0, "hi", Style::default());
        let cmds = diff_buffers(&prev, &next);
        assert!(
            cmds.iter().any(|c| matches!(c, DrawCommand::Put { x: 0, .. })),
            "expected a Put for the new cell at x=0; got {cmds:?}"
        );
    }

    #[test]
    fn display_width_strips_osc_hyperlink_payload() {
        // OSC8 hyperlink: ESC ] 8 ; ; URL BEL VISIBLE ESC ] 8 ; ; BEL
        // `display_width` should ignore the URL bytes and only count
        // "VISIBLE".
        let s = "\x1B]8;;https://example.com\x07VISIBLE\x1B]8;;\x07";
        assert_eq!(display_width(s), "VISIBLE".width());
    }
}
