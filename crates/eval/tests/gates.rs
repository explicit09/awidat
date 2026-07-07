use montage_eval::{ArchetypeTargets, ColdOpenSpec, FloorSpec, HouseProfile, PacingStats, gates};

fn stats(cuts: &[f64], dur: f64) -> PacingStats {
    PacingStats::from_cut_times(cuts, dur).unwrap_or_else(|e| panic!("valid input rejected: {e}"))
}

/// A DOAC-shaped test profile: blitz cold open, floored informational
/// archetype, floorless emotional archetype.
fn test_profile() -> HouseProfile {
    let mut archetypes = std::collections::BTreeMap::new();
    archetypes.insert(
        "informational".to_string(),
        ArchetypeTargets {
            body_band: (2.0, 9.0),
            floor: Some(FloorSpec {
                window_secs: 300.0,
                min_rate: 0.5,
            }),
        },
    );
    archetypes.insert(
        "emotional".to_string(),
        ArchetypeTargets {
            body_band: (1.5, 3.5),
            floor: None,
        },
    );
    HouseProfile {
        name: "test".to_string(),
        version: 1,
        cold_open: Some(ColdOpenSpec {
            window_secs: 90.0,
            min_rate: 20.0,
            last_peak_minute: 1,
        }),
        archetypes,
        sound: None,
    }
}

#[test]
fn pacing_stats_computes_cuts_per_min() {
    // 3 cuts over a 60s program = 3 cuts/min.
    let stats = PacingStats::from_cut_times(&[10.0, 20.0, 30.0], 60.0)
        .unwrap_or_else(|e| panic!("valid input rejected: {e}"));
    assert!((stats.cuts_per_min() - 3.0).abs() < 1e-9);
}

#[test]
fn pacing_stats_rejects_non_positive_duration() {
    assert!(PacingStats::from_cut_times(&[1.0], 0.0).is_err());
}

#[test]
fn rate_in_window_counts_only_cuts_inside() {
    // Cuts at 5s, 50s, 100s; window [0, 90) holds two of them -> 2 cuts
    // over 90s = 1.333 cuts/min.
    let stats = PacingStats::from_cut_times(&[5.0, 50.0, 100.0], 120.0)
        .unwrap_or_else(|e| panic!("valid input rejected: {e}"));
    assert!((stats.rate_in_window(0.0, 90.0) - 2.0 / 1.5).abs() < 1e-9);
}

#[test]
fn sparsest_window_rate_finds_the_dead_stretch() {
    // 600s program: blitz in the first 100s, then nothing. The sparsest
    // 300s window contains zero cuts.
    let cuts: Vec<f64> = (0..20).map(|i| f64::from(i) * 5.0).collect();
    let stats = PacingStats::from_cut_times(&cuts, 600.0)
        .unwrap_or_else(|e| panic!("valid input rejected: {e}"));
    assert!((stats.sparsest_window_rate(300.0) - 0.0).abs() < 1e-9);
}

#[test]
fn peak_minute_is_the_densest_minute_index() {
    // Minute 2 (120..180s) has three cuts; every other minute has at most one.
    let stats = PacingStats::from_cut_times(&[10.0, 125.0, 130.0, 150.0, 300.0], 360.0)
        .unwrap_or_else(|e| panic!("valid input rejected: {e}"));
    assert_eq!(stats.peak_minute(), Some(2));
}

#[test]
fn floor_fails_a_zero_cut_program_for_floored_archetypes() {
    // The shipped-raw-composite failure mode: 30 minutes, no cuts at all.
    let report = gates::floor(&stats(&[], 1800.0), &test_profile(), "informational");
    assert!(!report.passed);
}

#[test]
fn floor_passes_a_steadily_cut_program() {
    // One cut every 30s comfortably clears 0.5 cuts/min in every window.
    let cuts: Vec<f64> = (1..60).map(|i| f64::from(i) * 30.0).collect();
    let report = gates::floor(&stats(&cuts, 1800.0), &test_profile(), "informational");
    assert!(report.passed, "{}", report.detail);
}

#[test]
fn floor_is_skipped_for_floorless_archetypes() {
    // Emotional cuts legitimately hold shots for minutes (Poirier has a
    // zero-cut 5-min window): no floor means the gate passes as skipped.
    let report = gates::floor(&stats(&[], 1800.0), &test_profile(), "emotional");
    assert!(report.passed, "{}", report.detail);
}

#[test]
fn floor_fails_closed_on_unknown_archetype() {
    let report = gates::floor(&stats(&[], 1800.0), &test_profile(), "nope");
    assert!(!report.passed);
}

#[test]
fn cold_open_passes_a_front_loaded_blitz() {
    // DOAC shape: ~40 cuts in the first 90s, sparse after.
    let mut cuts: Vec<f64> = (0..40).map(|i| f64::from(i) * 2.2).collect();
    cuts.extend((2..30).map(|i| f64::from(i) * 60.0));
    let report = gates::cold_open(&stats(&cuts, 1800.0), &test_profile());
    assert!(report.passed, "{}", report.detail);
}

#[test]
fn cold_open_fails_when_the_peak_is_buried_mid_show() {
    // Finding 11 shape: flat opening, densest minute deep in the body.
    let mut cuts: Vec<f64> = (0..10).map(|i| f64::from(i) * 9.0).collect(); // 6.7/min opening
    cuts.extend((0..20).map(|i| 2520.0 + f64::from(i) * 3.0)); // blitz at minute 42
    let report = gates::cold_open(&stats(&cuts, 3274.0), &test_profile());
    assert!(!report.passed);
}

#[test]
fn cold_open_is_skipped_when_the_profile_has_none() {
    // TBPN live-chrome format: no cold-open requirement, flat opening is fine.
    let mut profile = test_profile();
    profile.cold_open = None;
    let cuts: Vec<f64> = (2..30).map(|i| f64::from(i) * 60.0).collect();
    let report = gates::cold_open(&stats(&cuts, 1800.0), &profile);
    assert!(report.passed, "{}", report.detail);
}

#[test]
fn body_pacing_flags_rates_outside_the_archetype_band() {
    // 1 cut/min body is under the informational band floor of 2.0.
    let cuts: Vec<f64> = (2..30).map(|i| f64::from(i) * 60.0).collect();
    let report = gates::body_pacing(&stats(&cuts, 1800.0), &test_profile(), "informational");
    assert!(!report.passed);

    // 4 cuts/min body sits inside the band.
    let cuts: Vec<f64> = (6..120).map(|i| f64::from(i) * 15.0).collect();
    let report = gates::body_pacing(&stats(&cuts, 1800.0), &test_profile(), "informational");
    assert!(report.passed, "{}", report.detail);
}
