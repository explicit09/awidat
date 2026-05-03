// Direct libc calls (dup, fcntl, poll, read) require `unsafe`. The
// rest of the workspace forbids unsafe; this module narrowly allows
// it. Each unsafe block carries a SAFETY: comment.
#![allow(unsafe_code)]

//! Short, best-effort terminal cursor probe for TUI startup.
//!
//! Lifted (and trimmed) from
//! `harnesses/codex/codex-rs/tui/src/terminal_probe.rs`. Codex's full
//! probe also queries OSC 10/11 colors and kitty keyboard support; we
//! only need the cursor position. The motivating bug:
//!
//! - ratatui's stock `Terminal` calls `backend.get_cursor_position()`
//!   on every `draw()` and `resize()` for inline viewport mode.
//! - Crossterm's `get_cursor_position` waits up to **two seconds** for
//!   the terminal's DSR-CPR (\x1B[6n) reply, blocking the redraw.
//! - On macOS Terminal under load the reply sometimes never arrives
//!   and the TUI bails with "cursor position could not be read within
//!   a normal duration". This kills the app.
//!
//! Our [`cursor_position`] uses a 100ms deadline and returns
//! `Ok(None)` if the terminal didn't answer in time. Callers fall back
//! to `Position::ORIGIN` and run with that — the only cost is that
//! the inline viewport may render at the top of the screen instead of
//! at the cursor, which is a far better failure mode than crashing.
//!
//! Unix-only. The Windows path returns `Ok(None)` so callers always
//! get the same fallback shape.

#[cfg(unix)]
mod imp {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Write};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::time::{Duration, Instant};

    use ratatui::layout::Position;

    /// Default wall-clock budget for the cursor probe.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(100);

    /// Owns a duplicated stdio file descriptor we put into nonblocking
    /// mode for the duration of a probe. Restores the original flags
    /// on drop.
    struct Tty {
        reader: File,
        writer: File,
        original_flags: libc::c_int,
    }

    impl Tty {
        fn open() -> io::Result<Self> {
            let stdio_reader = dup_file(libc::STDIN_FILENO);
            let stdio_writer = dup_file(libc::STDOUT_FILENO);
            match (stdio_reader, stdio_writer) {
                (Ok(reader), Ok(writer)) => Self::new(reader, writer),
                _ => {
                    // Fallback: open /dev/tty. Useful when stdio is a
                    // pipe (uncommon for a TUI but cheap to support).
                    let reader = OpenOptions::new().read(true).open("/dev/tty")?;
                    let writer = OpenOptions::new().write(true).open("/dev/tty")?;
                    Self::new(reader, writer)
                }
            }
        }

        fn new(reader: File, writer: File) -> io::Result<Self> {
            let fd = reader.as_raw_fd();
            // SAFETY: fd is a live FD owned by the just-opened File;
            // F_GETFL/F_SETFL on a valid FD is safe.
            let original_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if original_flags == -1 {
                return Err(io::Error::last_os_error());
            }
            if unsafe { libc::fcntl(fd, libc::F_SETFL, original_flags | libc::O_NONBLOCK) } == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                reader,
                writer,
                original_flags,
            })
        }

        fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.writer.write_all(bytes)?;
            self.writer.flush()
        }

        fn read_available(&mut self, buffer: &mut Vec<u8>) -> io::Result<()> {
            let mut chunk = [0_u8; 256];
            loop {
                // SAFETY: fd is owned by self.reader; chunk is a live
                // mutable buffer on the stack with `chunk.len()` bytes.
                let count = unsafe {
                    libc::read(
                        self.reader.as_raw_fd(),
                        chunk.as_mut_ptr().cast::<libc::c_void>(),
                        chunk.len(),
                    )
                };
                if count > 0 {
                    buffer.extend_from_slice(&chunk[..count as usize]);
                    continue;
                }
                if count == 0 {
                    return Ok(());
                }
                let err = io::Error::last_os_error();
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) {
                    return Ok(());
                }
                return Err(err);
            }
        }

        fn poll_readable(&self, timeout: Duration) -> io::Result<bool> {
            let mut fd = libc::pollfd {
                fd: self.reader.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let deadline = Instant::now() + timeout;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    return Ok(false);
                }
                let timeout_ms = deadline
                    .saturating_duration_since(now)
                    .as_millis()
                    .min(libc::c_int::MAX as u128) as libc::c_int;
                // SAFETY: fd is a stack-local pollfd with a valid FD.
                let result = unsafe { libc::poll(&mut fd, /*nfds*/ 1, timeout_ms) };
                if result > 0 {
                    return Ok((fd.revents & libc::POLLIN) != 0);
                }
                if result == 0 {
                    return Ok(false);
                }
                let err = io::Error::last_os_error();
                if err.kind() != io::ErrorKind::Interrupted {
                    return Err(err);
                }
            }
        }
    }

    impl Drop for Tty {
        fn drop(&mut self) {
            // Best-effort restore of the original O_NONBLOCK setting.
            // SAFETY: reader.as_raw_fd() is still live at drop time;
            // restoring fcntl flags can't make the program unsafe.
            let _ = unsafe {
                libc::fcntl(self.reader.as_raw_fd(), libc::F_SETFL, self.original_flags)
            };
        }
    }

    fn dup_file(fd: libc::c_int) -> io::Result<File> {
        // SAFETY: dup of a process-owned stdio FD. Returns -1 on
        // error; we map that to an io::Error before constructing File
        // so we never wrap an invalid FD.
        let duplicated = unsafe { libc::dup(fd) };
        if duplicated == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: just got a valid FD from dup; transferring ownership
        // to File via from_raw_fd is the documented way.
        Ok(unsafe { File::from_raw_fd(duplicated) })
    }

    /// Query the terminal for the current cursor position with a
    /// bounded timeout. Returns `Ok(None)` if the terminal didn't
    /// reply in time — caller should fall back to a known-safe
    /// position (typically `Position::ORIGIN`).
    pub fn cursor_position(timeout: Duration) -> io::Result<Option<Position>> {
        let mut tty = Tty::open()?;
        // ANSI DSR-CPR: request cursor position. The terminal replies
        // with `ESC [ <row> ; <col> R`.
        tty.write_all(b"\x1B[6n")?;
        let Some(response) = read_until(&mut tty, timeout, parse_cursor_position)? else {
            return Ok(None);
        };
        Ok(Some(response))
    }

    /// Read terminal bytes into a buffer, calling `parse` after each
    /// chunk until it produces `Some(_)` or the deadline expires.
    ///
    /// The accumulated buffer may include unrelated terminal input
    /// (e.g. keypresses arriving during the probe). We don't replay
    /// those — the probe runs only at startup, before the crossterm
    /// event stream is created.
    fn read_until<T>(
        tty: &mut Tty,
        timeout: Duration,
        mut parse: impl FnMut(&[u8]) -> Option<T>,
    ) -> io::Result<Option<T>> {
        let deadline = Instant::now() + timeout;
        let mut buffer = Vec::new();
        loop {
            tty.read_available(&mut buffer)?;
            if let Some(value) = parse(&buffer) {
                return Ok(Some(value));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            if !tty.poll_readable(deadline.saturating_duration_since(now))? {
                return Ok(None);
            }
        }
    }

    /// Parse a DSR-CPR response of the form `ESC [ <row> ; <col> R`.
    /// Both row and col are 1-based in the wire format; we translate
    /// to ratatui's 0-based [`Position`].
    fn parse_cursor_position(buffer: &[u8]) -> Option<Position> {
        for start in find_all_subslices(buffer, b"\x1B[") {
            let rest = &buffer[start + 2..];
            let end = rest.iter().position(|b| *b == b'R')?;
            let payload = std::str::from_utf8(&rest[..end]).ok()?;
            let (row, col) = payload.split_once(';')?;
            let row: u16 = row.parse().ok()?;
            let col: u16 = col.parse().ok()?;
            return Some(Position {
                x: col.saturating_sub(1),
                y: row.saturating_sub(1),
            });
        }
        None
    }

    fn find_all_subslices<'a>(
        haystack: &'a [u8],
        needle: &'a [u8],
    ) -> impl Iterator<Item = usize> + 'a {
        haystack
            .windows(needle.len())
            .enumerate()
            .filter_map(move |(idx, window)| (window == needle).then_some(idx))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_cursor_position_as_zero_based() {
            assert_eq!(
                parse_cursor_position(b"\x1B[20;10R"),
                Some(Position { x: 9, y: 19 })
            );
        }

        #[test]
        fn parses_cursor_position_with_leading_garbage() {
            assert_eq!(
                parse_cursor_position(b"garbage before\x1B[5;7R"),
                Some(Position { x: 6, y: 4 })
            );
        }

        #[test]
        fn returns_none_on_truncated_response() {
            assert_eq!(parse_cursor_position(b"\x1B[5;7"), None);
        }

        #[test]
        fn returns_none_on_empty_input() {
            assert_eq!(parse_cursor_position(b""), None);
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use std::io;
    use std::time::Duration;

    use ratatui::layout::Position;

    pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(100);

    pub fn cursor_position(_timeout: Duration) -> io::Result<Option<Position>> {
        Ok(None)
    }
}

pub use imp::{DEFAULT_TIMEOUT, cursor_position};
