//! Golden gate checks against real measured programs (the 2026-07-06
//! editorial study). These reproduce Finding 11: our published videos fail
//! the picture gates that reference-channel episodes pass.
//!
//! Fixtures are scene-cut boundary times (one f64 per line) produced by
//! ffmpeg scene detection at threshold 0.25.

use montage_eval::{PacingStats, gates, load_cut_times};

fn fixture(name: &str, duration_secs: f64) -> PacingStats {
    let path = format!("{}/fixtures/cuts/{name}", env!("CARGO_MANIFEST_DIR"));
    let cuts = load_cut_times(&path).unwrap_or_else(|e| panic!("fixture {name}: {e}"));
    PacingStats::from_cut_times(&cuts, duration_secs)
        .unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

/// SaaS Doomsday (kaQuWEPIv3I): 30min static composite, zero cuts.
/// The floor rule exists to stop exactly this from shipping.
#[test]
fn golden_saas_composite_fails_floor_and_cold_open() {
    let stats = fixture("own_kaQuWEPIv3I.cuts", 1786.0);
    assert!(!gates::saas_floor(&stats).passed);
    assert!(!gates::cold_open(&stats).passed);
}

/// Founder Journey (4h7bjBsW_Ag): competent 3.2 cuts/min body, but the
/// peak-energy minute is 42 and the opening runs flat — no cold open.
/// It also has a dead 5-min stretch at 35:45.
#[test]
fn golden_founder_journey_fails_cold_open_and_floor() {
    let stats = fixture("own_4h7bjBsW_Ag.cuts", 3274.0);
    let report = gates::cold_open(&stats);
    assert!(!report.passed, "{}", report.detail);
    assert!(!gates::saas_floor(&stats).passed);
}

/// DOAC Anti-Aging (Jk7RAkFN4vk): blitz cold open (46.7 cuts/min first
/// 90s, peak minute 0), informational body at 7.5 — passes everything.
#[test]
fn golden_doac_informational_episode_passes_picture_gates() {
    let stats = fixture("Jk7RAkFN4vk.cuts", 4532.0);
    let cold = gates::cold_open(&stats);
    assert!(cold.passed, "{}", cold.detail);
    let floor = gates::saas_floor(&stats);
    assert!(floor.passed, "{}", floor.detail);
    let body = gates::body_pacing(&stats, 2.0, 9.0);
    assert!(body.passed, "{}", body.detail);
}

/// DOAC Poirier (DM2jRSgjy1o): emotional archetype — body runs 1.98
/// cuts/min once the cold open is excluded (the study's 2.4 figure was
/// whole-program), yet the episode still opens with a blitz (peak minute 1,
/// 26 cuts/min opening). Emotional band floor is therefore 1.5, not 2.0.
/// NOTE: it also has a zero-cut 5-min window, so the saas_floor threshold
/// needs per-archetype calibration before it gates emotional cuts; not
/// asserted here.
#[test]
fn golden_doac_emotional_episode_still_has_a_cold_open() {
    let stats = fixture("DM2jRSgjy1o.cuts", 5602.0);
    let cold = gates::cold_open(&stats);
    assert!(cold.passed, "{}", cold.detail);
    let body = gates::body_pacing(&stats, 1.5, 9.0);
    assert!(body.passed, "{}", body.detail);
}
