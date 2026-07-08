//! Sound-pass gate: loudness against the house profile's delivery spec.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use montage_eval::{HouseProfile, LoudnessStats, gates, parse_ebur128};

fn profile(name: &str) -> HouseProfile {
    let path = format!(
        "{}/fixtures/profiles/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    HouseProfile::from_json_file(&path).unwrap_or_else(|e| panic!("profile {name}: {e}"))
}

/// Trailing summary block of `ffmpeg -af ebur128=peak=true -f null -`.
const EBUR128_SUMMARY: &str = "\
[Parsed_ebur128_0 @ 0x600002f1c000] Summary:

  Integrated loudness:
    I:         -14.2 LUFS
    Threshold: -24.6 LUFS

  Loudness range:
    LRA:         6.4 LU
    Threshold: -34.9 LUFS
    LRA low:   -18.7 LUFS
    LRA high:  -12.3 LUFS

  True peak:
    Peak:       -1.4 dBFS
";

#[test]
fn parse_ebur128_extracts_integrated_and_peak() {
    let stats = parse_ebur128(EBUR128_SUMMARY).expect("summary parses");
    assert!((stats.integrated_lufs - (-14.2)).abs() < 1e-9);
    assert!((stats.true_peak_db - (-1.4)).abs() < 1e-9);
}

#[test]
fn parse_ebur128_rejects_text_without_summary() {
    assert!(parse_ebur128("no loudness here").is_none());
}

#[test]
fn loudness_gate_passes_inside_the_profile_spec() {
    // technologia targets -14 LUFS ±1 LU, true peak ≤ -1 dBTP.
    let house = profile("technologia");
    let stats = LoudnessStats {
        integrated_lufs: -14.2,
        true_peak_db: -1.4,
    };
    let report = gates::loudness(&stats, &house);
    assert!(report.passed, "{}", report.detail);
}

#[test]
fn loudness_gate_fails_quiet_masters_and_hot_peaks() {
    let house = profile("technologia");
    // Way too quiet (un-normalized raw dialogue).
    let quiet = LoudnessStats {
        integrated_lufs: -23.0,
        true_peak_db: -6.0,
    };
    assert!(!gates::loudness(&quiet, &house).passed);
    // Loudness fine but true peak clipping-hot.
    let hot = LoudnessStats {
        integrated_lufs: -14.0,
        true_peak_db: -0.1,
    };
    assert!(!gates::loudness(&hot, &house).passed);
}

#[test]
fn split_edit_coverage_counts_boundaries_with_audio_leads() {
    // 4 speaker-turn boundaries, 2 carry a J/L-cut -> coverage 0.5.
    use montage_eval::SplitEditStats;
    let stats = SplitEditStats::from_boundaries(&[
        (10.0, true),
        (20.0, false),
        (30.0, true),
        (40.0, false),
    ]);
    assert!((stats.coverage() - 0.5).abs() < 1e-9);
    assert_eq!(stats.total(), 4);
}

#[test]
fn split_edit_gate_enforces_profile_minimum() {
    use montage_eval::SplitEditStats;
    let house = profile("technologia");
    // technologia demands >= 0.4 J/L coverage at speaker turns.
    let low = SplitEditStats::from_boundaries(&[(10.0, true), (20.0, false), (30.0, false)]);
    let report = gates::split_edit_coverage(&low, &house);
    assert!(!report.passed, "{}", report.detail);

    let ok = SplitEditStats::from_boundaries(&[(10.0, true), (20.0, true), (30.0, false)]);
    let report = gates::split_edit_coverage(&ok, &house);
    assert!(report.passed, "{}", report.detail);
}

#[test]
fn split_edit_gate_skips_with_no_boundaries_or_no_spec() {
    use montage_eval::SplitEditStats;
    // No speaker turns at all -> nothing to judge, pass as skipped.
    let empty = SplitEditStats::from_boundaries(&[]);
    let report = gates::split_edit_coverage(&empty, &profile("technologia"));
    assert!(report.passed, "{}", report.detail);
    // Profile without a sound spec (tbpn) -> skipped.
    let some = SplitEditStats::from_boundaries(&[(10.0, false)]);
    let report = gates::split_edit_coverage(&some, &profile("tbpn"));
    assert!(report.passed, "{}", report.detail);
}

#[test]
fn loudness_gate_skips_when_profile_has_no_sound_spec() {
    // tbpn profile carries no sound spec yet — gate passes as skipped.
    let house = profile("tbpn");
    let stats = LoudnessStats {
        integrated_lufs: -23.0,
        true_peak_db: -0.1,
    };
    let report = gates::loudness(&stats, &house);
    assert!(report.passed, "{}", report.detail);
}
