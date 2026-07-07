use montage_eval::HouseProfile;

fn load(name: &str) -> HouseProfile {
    let path = format!(
        "{}/fixtures/profiles/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    HouseProfile::from_json_file(&path).unwrap_or_else(|e| panic!("profile {name}: {e}"))
}

#[test]
fn doac_profile_carries_measured_targets() {
    let p = load("doac");
    assert_eq!(p.name, "doac");
    let cold = p
        .cold_open
        .as_ref()
        .unwrap_or_else(|| panic!("doac has a cold open"));
    assert!((cold.min_rate - 20.0).abs() < 1e-9);
    assert_eq!(cold.last_peak_minute, 1);

    let info = p
        .archetype("informational")
        .unwrap_or_else(|| panic!("doac defines informational"));
    assert!(info.body_band.0 > 5.0 && info.body_band.1 <= 10.0);
    assert!(info.floor.is_some());

    // Emotional cuts legitimately hold shots for minutes (Poirier has a
    // zero-cut 5-min window) — the floor gate is disabled for them.
    let emo = p
        .archetype("emotional")
        .unwrap_or_else(|| panic!("doac defines emotional"));
    assert!(emo.floor.is_none());
}

#[test]
fn tbpn_profile_has_no_cold_open() {
    // Live-chrome format: the show opens cold into the desk, no blitz.
    let p = load("tbpn");
    assert!(p.cold_open.is_none());
}

#[test]
fn unknown_archetype_is_none() {
    let p = load("doac");
    assert!(p.archetype("does-not-exist").is_none());
}
