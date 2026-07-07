//! Golden gate checks against real measured programs (the 2026-07-06
//! editorial study). These reproduce Finding 11: our published videos fail
//! the picture gates that reference-channel episodes pass — each program
//! scored against its house profile.
//!
//! Fixtures are scene-cut boundary times (one f64 per line) produced by
//! ffmpeg scene detection at threshold 0.25.

use montage_eval::{HouseProfile, PacingStats, gates, load_cut_times};

fn fixture(name: &str, duration_secs: f64) -> PacingStats {
    let path = format!("{}/fixtures/cuts/{name}", env!("CARGO_MANIFEST_DIR"));
    let cuts = load_cut_times(&path).unwrap_or_else(|e| panic!("fixture {name}: {e}"));
    PacingStats::from_cut_times(&cuts, duration_secs)
        .unwrap_or_else(|e| panic!("fixture {name}: {e}"))
}

fn profile(name: &str) -> HouseProfile {
    let path = format!(
        "{}/fixtures/profiles/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    HouseProfile::from_json_file(&path).unwrap_or_else(|e| panic!("profile {name}: {e}"))
}

/// SaaS Doomsday (kaQuWEPIv3I): 30min static composite, zero cuts. The
/// floor rule exists to stop exactly this from shipping.
#[test]
fn golden_saas_composite_fails_floor_and_cold_open() {
    let stats = fixture("own_kaQuWEPIv3I.cuts", 1786.0);
    let house = profile("technologia");
    assert!(!gates::floor(&stats, &house, "informational").passed);
    assert!(!gates::cold_open(&stats, &house).passed);
}

/// Founder Journey (4h7bjBsW_Ag): competent 3.2 cuts/min body, but the
/// peak-energy minute is deep in the show and the opening runs flat — no
/// cold open. It also has a dead 5-min stretch at 35:45.
#[test]
fn golden_founder_journey_fails_cold_open_and_floor() {
    let stats = fixture("own_4h7bjBsW_Ag.cuts", 3274.0);
    let house = profile("technologia");
    let report = gates::cold_open(&stats, &house);
    assert!(!report.passed, "{}", report.detail);
    assert!(!gates::floor(&stats, &house, "informational").passed);
}

/// DOAC Anti-Aging (Jk7RAkFN4vk): blitz cold open (46.7 cuts/min first
/// 90s, peak minute 0), informational body at 7.5 — passes everything.
#[test]
fn golden_doac_informational_episode_passes_picture_gates() {
    let stats = fixture("Jk7RAkFN4vk.cuts", 4532.0);
    let house = profile("doac");
    let cold = gates::cold_open(&stats, &house);
    assert!(cold.passed, "{}", cold.detail);
    let floor = gates::floor(&stats, &house, "informational");
    assert!(floor.passed, "{}", floor.detail);
    let body = gates::body_pacing(&stats, &house, "informational");
    assert!(body.passed, "{}", body.detail);
}

/// DOAC Poirier (DM2jRSgjy1o): emotional archetype — body runs 1.98
/// cuts/min once the cold open is excluded, holds shots for minutes (it
/// has a zero-cut 5-min window), yet still opens with a blitz. The
/// emotional archetype has no floor, so the floor gate passes as skipped.
#[test]
fn golden_doac_emotional_episode_passes_with_floorless_archetype() {
    let stats = fixture("DM2jRSgjy1o.cuts", 5602.0);
    let house = profile("doac");
    let cold = gates::cold_open(&stats, &house);
    assert!(cold.passed, "{}", cold.detail);
    let body = gates::body_pacing(&stats, &house, "emotional");
    assert!(body.passed, "{}", body.detail);
    let floor = gates::floor(&stats, &house, "emotional");
    assert!(floor.passed, "{}", floor.detail);
}
