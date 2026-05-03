//! Push finalized chat lines into the terminal's scrollback above
//! the inline viewport.
//!
//! Lifted (and trimmed) from
//! `harnesses/codex/codex-rs/tui/src/insert_history.rs`. Codex's full
//! port also handles:
//!
//! - URL-aware wrapping (long lines that contain http URLs are kept
//!   intact so the terminal emulator can detect them as clickable
//!   links).
//! - Adaptive wrap of mixed prose-and-URL lines.
//! - A Zellij fallback path that uses `\n` instead of DECSTBM (Zellij
//!   silently mishandles scroll regions).
//!
//! We skip those for v1. Our lines are short, no URLs yet, and we
//! don't run inside Zellij. If we add markdown rendering or URL
//! callouts to the chat pane we'll port the wrapping module then.
//!
//! ## How it works
//!
//! The strategy is: lock the scroll region to "everything above the
//! viewport top", move the cursor to the row just above the viewport,
//! then `\r\n` for each new line. The terminal scrolls *only the
//! locked region*, so existing scrollback rolls upward and our new
//! lines land at the bottom of that region — i.e. directly above the
//! viewport. Released back to the full screen via `ResetScrollRegion`.
//!
//! When the viewport is already pinned at the bottom of the screen
//! we don't need to make room (we use the rows already above the
//! viewport). When the viewport has empty rows below it on the
//! screen, we first emit Reverse Index (`ESC M`) to scroll *into*
//! those rows, so we don't shift the user's existing scrollback by
//! more lines than necessary.

use std::fmt;
use std::io;
use std::io::Write;

use crossterm::{
    Command,
    cursor::MoveTo,
    queue,
    style::{
        Attribute as CAttribute, Color as CColor, Colors, Print, SetAttribute, SetBackgroundColor,
        SetColors, SetForegroundColor,
    },
    terminal::{Clear, ClearType},
};
use ratatui::{
    layout::Size,
    prelude::Backend,
    style::{Color, Modifier},
    text::{Line, Span},
};

use crate::custom_terminal::Terminal;

/// Push `lines` into terminal scrollback above the viewport.
pub fn insert_history_lines<B>(terminal: &mut Terminal<B>, lines: Vec<Line>) -> io::Result<()>
where
    B: Backend + Write,
{
    let screen_size = terminal.backend().size().unwrap_or(Size::new(0, 0));
    let mut area = terminal.viewport_area;
    let mut should_update_area = false;
    let last_cursor_pos = terminal.last_known_cursor_pos;
    let writer = terminal.backend_mut();

    // We don't pre-wrap; let the terminal soft-wrap visually if a
    // line is wider than the viewport. Each row of `lines` counts as
    // one row inserted.
    let inserted_rows = lines.len() as u16;
    if inserted_rows == 0 {
        return Ok(());
    }
    let wrap_width = area.width.max(1) as usize;

    // If the viewport doesn't reach the bottom of the screen, we
    // can shift it down to cover those rows instead of disturbing
    // existing scrollback. Reverse Index (ESC M) inside a scroll
    // region pulls content downward.
    let cursor_top = if area.bottom() < screen_size.height {
        let scroll_amount = inserted_rows.min(screen_size.height - area.bottom());
        let top_1based = area.top() + 1;
        queue!(writer, SetScrollRegion(top_1based..screen_size.height))?;
        queue!(writer, MoveTo(/*x*/ 0, area.top()))?;
        for _ in 0..scroll_amount {
            queue!(writer, Print("\x1bM"))?; // Reverse Index
        }
        queue!(writer, ResetScrollRegion)?;
        area.y += scroll_amount;
        should_update_area = true;
        area.top().saturating_sub(1)
    } else {
        area.top().saturating_sub(1)
    };

    // Clip the scroll region to "everything above the viewport". When
    // we now print `\r\n` the terminal scrolls only that region; new
    // text lands at the bottom of the region (immediately above the
    // viewport).
    queue!(writer, SetScrollRegion(1..area.top()))?;
    // MoveTo (not the Terminal's set_cursor_position) on purpose:
    // insert_history_lines must be cursor-position-neutral. We
    // explicitly restore at the end of the function.
    queue!(writer, MoveTo(/*x*/ 0, cursor_top))?;

    for line in &lines {
        queue!(writer, Print("\r\n"))?;
        write_history_line(writer, line, wrap_width)?;
    }

    queue!(writer, ResetScrollRegion)?;
    // Restore cursor position so subsequent draws aren't surprised.
    queue!(writer, MoveTo(last_cursor_pos.x, last_cursor_pos.y))?;

    if should_update_area {
        terminal.set_viewport_area(area);
    }
    terminal.note_history_rows_inserted(inserted_rows);
    Ok(())
}

/// Render one Line into the current cursor row. Caller is
/// responsible for any leading `\r\n` and cursor positioning.
fn write_history_line<W: Write>(writer: &mut W, line: &Line, _wrap_width: usize) -> io::Result<()> {
    queue!(
        writer,
        SetColors(Colors::new(
            line.style
                .fg
                .map(std::convert::Into::into)
                .unwrap_or(CColor::Reset),
            line.style
                .bg
                .map(std::convert::Into::into)
                .unwrap_or(CColor::Reset)
        ))
    )?;
    queue!(writer, Clear(ClearType::UntilNewLine))?;

    // Merge each span's style into the line's style so any
    // line-level color (e.g. on a Diff item) shows on every span.
    let merged_spans: Vec<Span> = line
        .spans
        .iter()
        .map(|s| Span {
            style: s.style.patch(line.style),
            content: s.content.clone(),
        })
        .collect();
    write_spans(writer, merged_spans.iter())
}

fn write_spans<'a, I>(mut writer: &mut impl Write, content: I) -> io::Result<()>
where
    I: IntoIterator<Item = &'a Span<'a>>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut last_modifier = Modifier::empty();
    for span in content {
        let mut modifier = Modifier::empty();
        modifier.insert(span.style.add_modifier);
        modifier.remove(span.style.sub_modifier);
        if modifier != last_modifier {
            ModifierDiff {
                from: last_modifier,
                to: modifier,
            }
            .queue(&mut writer)?;
            last_modifier = modifier;
        }
        let next_fg = span.style.fg.unwrap_or(Color::Reset);
        let next_bg = span.style.bg.unwrap_or(Color::Reset);
        if next_fg != fg || next_bg != bg {
            queue!(
                writer,
                SetColors(Colors::new(next_fg.into(), next_bg.into()))
            )?;
            fg = next_fg;
            bg = next_bg;
        }
        queue!(writer, Print(span.content.clone()))?;
    }
    queue!(
        writer,
        SetForegroundColor(CColor::Reset),
        SetBackgroundColor(CColor::Reset),
        SetAttribute(CAttribute::Reset),
    )
}

/// `CSI <top>;<bottom>r` — DECSTBM, set the scroll region in 1-based
/// inclusive coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetScrollRegion(pub std::ops::Range<u16>);

impl Command for SetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[{};{}r", self.0.start, self.0.end)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Err(std::io::Error::other("SetScrollRegion is ANSI-only"))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

/// `CSI r` — release any DECSTBM scroll region back to the full screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetScrollRegion;

impl Command for ResetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[r")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Err(std::io::Error::other("ResetScrollRegion is ANSI-only"))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

/// Translate a Modifier diff (added/removed bits) into ANSI SGR
/// commands. Mirrors the ratatui upstream version.
struct ModifierDiff {
    from: Modifier,
    to: Modifier,
}

impl ModifierDiff {
    fn queue<W: Write>(self, mut w: W) -> io::Result<()> {
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

    #[test]
    fn set_scroll_region_emits_decstbm() {
        let mut s = String::new();
        SetScrollRegion(1..24).write_ansi(&mut s).unwrap();
        assert_eq!(s, "\x1b[1;24r");
    }

    #[test]
    fn reset_scroll_region_emits_csi_r() {
        let mut s = String::new();
        ResetScrollRegion.write_ansi(&mut s).unwrap();
        assert_eq!(s, "\x1b[r");
    }

    #[test]
    fn write_spans_writes_plain_content() {
        let spans = ["hello", " world"].map(Into::into);
        let mut out: Vec<u8> = Vec::new();
        write_spans(&mut out, spans.iter()).unwrap();
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.contains("hello"), "got: {rendered:?}");
        assert!(rendered.contains("world"), "got: {rendered:?}");
        // Ensure we end with the SGR reset (modifier + colors).
        assert!(
            rendered.contains("\x1b[0m"),
            "missing SGR reset: {rendered:?}"
        );
    }
}
