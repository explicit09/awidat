//! Awidat tool registry and tool implementations.
//!
//! Week 1 stub. Real tools (`apply_edl`, `view_timeline`, ...) land in Week 4
//! per `PLAN.md` §6 / §15.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Number of tools planned for v1 per `PLAN.md` §6.1.
pub const PLANNED_V1_TOOL_COUNT: usize = 12;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_count_matches_plan() {
        assert_eq!(PLANNED_V1_TOOL_COUNT, 12);
    }
}
