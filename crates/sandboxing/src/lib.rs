//! Awidat per-OS sandbox.
//!
//! Week 1 stub. Real implementation lands in Week 7 per `PLAN.md` §10 / §15.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Returns the sandbox backend that would be selected on the current platform.
pub fn current_backend() -> &'static str {
    if cfg!(target_os = "macos") {
        "seatbelt"
    } else if cfg!(target_os = "linux") {
        "landlock+seccomp"
    } else {
        "unsupported"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_returns_known_string() {
        let backend = current_backend();
        assert!(matches!(
            backend,
            "seatbelt" | "landlock+seccomp" | "unsupported"
        ));
    }
}
