use montage_eval::suite;

#[test]
fn golden_suite_passes_on_committed_fixtures() {
    let results = suite::run_golden(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures"))
        .unwrap_or_else(|e| panic!("suite failed to run: {e}"));
    assert!(!results.is_empty());
    for r in &results {
        assert!(r.ok, "golden case {} regressed: {}", r.case, r.detail);
    }
}
