fn run(args: &[&str]) -> std::process::Output {
    assert_cmd::cargo::cargo_bin_cmd!("montage-eval")
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("montage-eval should run: {error}"))
}

#[test]
fn requested_unimplemented_lane_is_reported_as_skipped() {
    let output = run(&["--product", "--json"]);
    assert!(output.status.success());

    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("stdout should be JSON: {error}"));
    assert_eq!(report["lanes"][0]["lane"], "product");
    assert_eq!(report["lanes"][0]["status"], "skipped");
    assert!(report["lanes"][0]["reason"].is_string());
}

#[test]
fn fail_on_skip_returns_failure_after_writing_the_report() {
    let output = run(&["--live", "--json", "--fail-on-skip"]);
    assert!(!output.status.success());

    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("stdout should be JSON: {error}"));
    assert_eq!(report["lanes"][0]["lane"], "live");
    assert_eq!(report["lanes"][0]["status"], "skipped");
}
