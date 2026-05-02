//! Awidat Ratatui terminal UI.
//!
//! Week 1 stub. Real TUI lands in Week 5 per `PLAN.md` §12.1 / §15.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Returns true if the TUI is implemented. Stubbed false until Week 5.
pub fn is_implemented() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_not_implemented() {
        assert!(!is_implemented());
    }
}
