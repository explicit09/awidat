//! Multi-line / structural assertion helpers for test output.

use serde_json::Value;

/// Assert two `serde_json::Value`s are equal, panicking with a
/// `pretty_assertions`-style colored diff on mismatch.
///
/// Use for OTIO round-trip / sidecar tests where the failure diff
/// is otherwise a wall of escaped JSON.
#[track_caller]
pub fn assert_json_eq(left: &Value, right: &Value) {
    if left != right {
        // Pretty-print both sides; print the whole structural diff.
        let l = serde_json::to_string_pretty(left).unwrap_or_else(|_| left.to_string());
        let r = serde_json::to_string_pretty(right).unwrap_or_else(|_| right.to_string());
        // Use pretty_assertions::assert_eq! so the runner produces a diff
        // colored by the assertion macro itself.
        pretty_assertions::assert_eq!(l, r);
    }
}
