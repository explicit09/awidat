//! ffmpeg progress parsing.
//!
//! ffmpeg writes lines like
//! `frame=  234 fps= 29.5 q=28.0 size=    256kB time=00:00:09.83 bitrate= 213.4kbits/s speed=0.99x`
//! to stderr. We tail it, parse `frame=`, `time=`, `speed=` from the most
//! recent line, and surface a [`ProgressSnapshot`].
//!
//! There's also `-progress <url>` which writes structured `key=value`
//! lines to a file. Cleaner but adds disk I/O — we prefer the stderr
//! path because ffmpeg writes it unconditionally.

use std::time::Duration;

/// Parsed snapshot from ffmpeg's most recent progress line.
#[derive(Debug, Default, Clone)]
pub struct ProgressSnapshot {
    /// Frames processed so far. None if ffmpeg hasn't emitted yet.
    pub frames_done: Option<u64>,
    /// Output time position (`time=HH:MM:SS.mmm`). None until first line.
    pub time_done_s: Option<f64>,
    /// Speed multiplier (`speed=0.99x`). None until first line.
    pub speed: Option<f64>,
    /// Output size in bytes (parsed from `size= 256kB`). None until first.
    pub size_bytes: Option<u64>,
    /// Last raw line we saw (for diagnostics).
    pub last_line: Option<String>,
}

impl ProgressSnapshot {
    /// Estimated time remaining, given the total source duration. Returns
    /// `None` if any of `time_done_s`, `speed`, or `total` is unknown.
    pub fn eta(&self, total_duration_s: Option<f64>) -> Option<Duration> {
        let time_done = self.time_done_s?;
        let speed = self.speed?;
        let total = total_duration_s?;
        if speed <= 0.0 || total <= time_done {
            return Some(Duration::ZERO);
        }
        let remaining_real = (total - time_done) / speed;
        Some(Duration::from_secs_f64(remaining_real.max(0.0)))
    }

    /// Percentage complete in `0..=100` given the total source duration.
    /// Returns `None` if either piece is missing.
    pub fn percent(&self, total_duration_s: Option<f64>) -> Option<f64> {
        let time_done = self.time_done_s?;
        let total = total_duration_s?;
        if total <= 0.0 {
            return Some(0.0);
        }
        Some(((time_done / total) * 100.0).clamp(0.0, 100.0))
    }
}

/// Parse one ffmpeg progress line into a snapshot. Caller should call
/// repeatedly with the latest line; later calls supersede earlier.
pub fn parse_progress_line(line: &str, prev: &mut ProgressSnapshot) {
    prev.last_line = Some(line.to_string());
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        let (key, value) = if let Some((k, v)) = tok.split_once('=') {
            if v.is_empty() && i + 1 < tokens.len() {
                // ffmpeg right-pads numeric values: `frame=  234` arrives
                // as the tokens `frame=` and `234`. Reach forward.
                let v = tokens[i + 1];
                i += 2;
                (k, v)
            } else {
                i += 1;
                (k, v)
            }
        } else {
            i += 1;
            continue;
        };
        match key {
            "frame" => {
                if let Ok(n) = value.parse::<u64>() {
                    prev.frames_done = Some(n);
                }
            }
            "time" => {
                if let Some(secs) = parse_time(value) {
                    prev.time_done_s = Some(secs);
                }
            }
            "speed" => {
                if let Some(stripped) = value.strip_suffix('x')
                    && let Ok(f) = stripped.parse::<f64>()
                {
                    prev.speed = Some(f);
                }
            }
            "size" => {
                if let Some(bytes) = parse_size(value) {
                    prev.size_bytes = Some(bytes);
                }
            }
            _ => {}
        }
    }
}

fn parse_time(s: &str) -> Option<f64> {
    // ffmpeg: `HH:MM:SS.mmm` (always 3 components).
    let mut parts = s.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + s)
}

fn parse_size(s: &str) -> Option<u64> {
    // ffmpeg writes `1024kB` or `2MB`. Lowercase, then strip suffix.
    let lower = s.to_ascii_lowercase();
    let (num, mult): (&str, u64) = if let Some(rest) = lower.strip_suffix("kb") {
        (rest, 1024)
    } else if let Some(rest) = lower.strip_suffix("mb") {
        (rest, 1024 * 1024)
    } else if let Some(rest) = lower.strip_suffix("gb") {
        (rest, 1024 * 1024 * 1024)
    } else if let Some(rest) = lower.strip_suffix('b') {
        (rest, 1)
    } else {
        return None;
    };
    let n: f64 = num.trim().parse().ok()?;
    Some((n * mult as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_ffmpeg_progress_line() {
        let line = "frame=  234 fps= 29.5 q=28.0 size=    256kB time=00:00:09.83 bitrate= 213.4kbits/s speed=0.99x";
        let mut snap = ProgressSnapshot::default();
        parse_progress_line(line, &mut snap);
        assert_eq!(snap.frames_done, Some(234));
        assert!((snap.time_done_s.unwrap() - 9.83).abs() < 1e-6);
        assert!((snap.speed.unwrap() - 0.99).abs() < 1e-9);
        assert_eq!(snap.size_bytes, Some(256 * 1024));
    }

    #[test]
    fn supersedes_with_each_call() {
        let mut snap = ProgressSnapshot::default();
        parse_progress_line("frame=10 time=00:00:01.00 speed=1.0x", &mut snap);
        assert_eq!(snap.frames_done, Some(10));
        parse_progress_line("frame=20 time=00:00:02.00 speed=2.0x", &mut snap);
        assert_eq!(snap.frames_done, Some(20));
        assert!((snap.time_done_s.unwrap() - 2.0).abs() < 1e-9);
        assert!((snap.speed.unwrap() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn percent_clamps_to_unit_range() {
        let mut snap = ProgressSnapshot::default();
        snap.time_done_s = Some(5.0);
        assert_eq!(snap.percent(Some(10.0)), Some(50.0));
        snap.time_done_s = Some(20.0);
        assert_eq!(snap.percent(Some(10.0)), Some(100.0));
        assert_eq!(snap.percent(None), None);
    }

    #[test]
    fn eta_uses_speed_and_remaining() {
        let mut snap = ProgressSnapshot::default();
        snap.time_done_s = Some(10.0);
        snap.speed = Some(2.0); // running at 2× realtime
        // 60s total, 10s done at 2× → (60-10)/2 = 25 real seconds left.
        let eta = snap.eta(Some(60.0)).unwrap();
        assert_eq!(eta.as_secs(), 25);
    }

    #[test]
    fn eta_zero_when_complete() {
        let mut snap = ProgressSnapshot::default();
        snap.time_done_s = Some(60.0);
        snap.speed = Some(1.0);
        assert_eq!(snap.eta(Some(60.0)), Some(Duration::ZERO));
    }

    #[test]
    fn eta_none_when_missing_pieces() {
        let snap = ProgressSnapshot::default();
        assert!(snap.eta(Some(60.0)).is_none());
    }

    #[test]
    fn parse_time_handles_fractional_seconds() {
        assert_eq!(parse_time("00:01:23.456"), Some(83.456));
        assert_eq!(parse_time("01:00:00.000"), Some(3600.0));
    }

    #[test]
    fn parse_size_handles_units() {
        assert_eq!(parse_size("256kB"), Some(256 * 1024));
        assert_eq!(parse_size("2MB"), Some(2 * 1024 * 1024));
        assert_eq!(parse_size("100B"), Some(100));
        assert_eq!(parse_size("garbage"), None);
    }
}
