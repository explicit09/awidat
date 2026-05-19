//! Shared transcript cleanup heuristic tests.

use awidat_core::transcript_cleanup::{
    CleanupConfig, TranscriptSegment, TranscriptWord, default_filler_tokens, false_start_ranges,
    filler_dense_ranges, normalize_transcript_token,
};

fn segment(start_s: f64, end_s: f64, text: &str) -> TranscriptSegment {
    TranscriptSegment {
        start_s,
        end_s,
        text: text.to_string(),
    }
}

#[test]
fn filler_dense_ranges_selects_only_filler_heavy_segments() {
    let ranges = filler_dense_ranges(
        &[
            segment(0.0, 2.0, "keep this opening point"),
            segment(2.0, 4.0, "um uh like um"),
            segment(4.0, 8.0, "keep this final conclusion"),
        ],
        CleanupConfig::default(),
    );

    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].start_s, 2.0);
    assert_eq!(ranges[0].end_s, 4.0);
    assert_eq!(ranges[0].filler_token_count, 4);
    assert_eq!(ranges[0].total_token_count, 4);
}

#[test]
fn filler_dense_ranges_preserves_actionable_advice_segments() {
    let ranges = filler_dense_ranges(
        &[
            segment(
                0.0,
                5.0,
                "biggest thing to all founders is like just make sure papers are signed",
            ),
            segment(5.0, 8.0, "um so like but yeah um"),
        ],
        CleanupConfig::default(),
    );

    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].start_s, 5.0);
    assert_eq!(ranges[0].end_s, 8.0);
}

#[test]
fn default_filler_tokens_keep_basic_and_discourse_modes_distinct() {
    let basic = default_filler_tokens(false);
    assert!(basic.iter().any(|token| token == "um"));
    assert!(basic.iter().any(|token| token == "uh"));
    assert!(!basic.iter().any(|token| token == "like"));

    let discourse = default_filler_tokens(true);
    assert!(discourse.iter().any(|token| token == "um"));
    assert!(discourse.iter().any(|token| token == "like"));
    assert!(discourse.iter().any(|token| token == "basically"));
}

#[test]
fn normalize_transcript_token_is_shared_for_filler_matching() {
    assert_eq!(normalize_transcript_token("Um,"), "um");
    assert_eq!(normalize_transcript_token("Actually!"), "actually");
    assert_eq!(normalize_transcript_token("  "), "");
}

fn word(text: &str, start_s: f64, end_s: f64) -> TranscriptWord {
    TranscriptWord {
        text: text.to_string(),
        start_s,
        end_s,
    }
}

#[test]
fn false_start_ranges_surface_restart_marker_preceding_fragment() {
    let ranges = false_start_ranges(
        &[
            word("So", 0.0, 0.3),
            word("the", 0.3, 0.5),
            word("thing", 0.5, 0.9),
            word("is,", 0.9, 1.2),
            word("wait,", 1.2, 1.5),
            word("actually", 1.5, 2.0),
            word("the", 2.0, 2.2),
            word("real", 2.2, 2.5),
            word("issue", 2.5, 2.9),
        ],
        0.0,
        3.0,
    );

    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].marker, "wait,");
    assert_eq!(ranges[0].start_s, 0.0);
    assert_eq!(ranges[0].end_s, 1.2);
    assert_eq!(ranges[0].snippet, "So the thing is,");
}

#[test]
fn false_start_ranges_detect_let_me_phrase() {
    let ranges = false_start_ranges(
        &[
            word("This", 0.0, 0.3),
            word("is", 0.3, 0.5),
            word("backwards", 0.5, 1.0),
            word("let", 1.0, 1.2),
            word("me", 1.2, 1.4),
            word("restart", 1.4, 1.8),
        ],
        0.0,
        2.0,
    );

    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0].marker, "let me");
    assert_eq!(ranges[0].start_s, 0.0);
    assert_eq!(ranges[0].end_s, 1.0);
}
