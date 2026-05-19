//! Contract tests for the rendered-output acceptance eval tier.

use std::process::Command;

use awidat_eval::acceptance;

#[test]
fn acceptance_tier_exposes_synthetic_dead_air_scenario() {
    let scenario_ids: Vec<&str> = acceptance::defaults()
        .iter()
        .map(|scenario| scenario.id())
        .collect();

    assert!(
        scenario_ids.contains(&"acceptance::synthetic_dead_air_cleanup_render"),
        "acceptance tier must include the MVP rendered-output scenario; got {scenario_ids:?}",
    );
}

#[test]
fn cli_help_lists_acceptance_tier() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(env!("CARGO_BIN_EXE_awidat-eval"))
        .arg("--help")
        .output()?;

    assert!(
        output.status.success(),
        "awidat-eval --help exited {:?}",
        output.status.code(),
    );

    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.contains("--acceptance"),
        "help output should document --acceptance tier:\n{stdout}",
    );
    assert!(
        stdout.contains("--acceptance-discover"),
        "help output should document real-video discovery:\n{stdout}",
    );
    assert!(
        stdout.contains("--acceptance-discover-write-drafts"),
        "help output should document fixture draft writing:\n{stdout}",
    );
    assert!(
        stdout.contains("--acceptance-fixture-manifest"),
        "help output should document fixture manifest reporting:\n{stdout}",
    );
    assert!(
        stdout.contains("--acceptance-batch-summary"),
        "help output should document batch summary reporting:\n{stdout}",
    );
    assert!(
        stdout.contains("--acceptance-package-failure"),
        "help output should document failure packet packaging:\n{stdout}",
    );
    assert!(
        stdout.contains("--acceptance-promote-fixture"),
        "help output should document fixture promotion:\n{stdout}",
    );
    Ok(())
}
