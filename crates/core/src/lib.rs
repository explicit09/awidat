//! Awidat agent loop and session state.
//!
//! Week 1 stub. Implementations land in Week 3 per `PLAN.md` §15.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Returns the version of the agent core. Placeholder for Week 1.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_returns_pkg_version() {
        assert!(!version().is_empty());
    }
}
