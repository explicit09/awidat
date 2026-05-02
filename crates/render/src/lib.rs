//! Awidat ffmpeg wrapper, proxy renders, preview job manager.
//!
//! Week 1 stub. Real implementation lands in Week 4 per `PLAN.md` §15.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Returns true if ffmpeg is required. Always true in v1.
pub fn requires_ffmpeg() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_ffmpeg_true() {
        assert!(requires_ffmpeg());
    }
}
