use std::collections::HashSet;

#[test]
fn gpt56_codex_matrix_covers_profiles_workflows_and_safety_metrics()
-> Result<(), Box<dyn std::error::Error>> {
    let matrix: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/gpt56-codex/matrix.json"))?;

    assert_eq!(
        matrix["profiles"],
        serde_json::json!(["balanced", "deep_edit"])
    );
    assert_eq!(
        matrix["native_search_default_gate"],
        "organic-edit-discovery"
    );

    let metrics = matrix["required_metrics"]
        .as_array()
        .ok_or_else(|| std::io::Error::other("required_metrics array"))?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<HashSet<_>>();
    for metric in [
        "editorial_outcome",
        "direct_otio_mutations",
        "approval_turns",
        "input_tokens",
        "elapsed_ms",
        "visual_verification",
    ] {
        assert!(metrics.contains(metric), "missing metric {metric}");
    }

    let cases = matrix["cases"]
        .as_array()
        .ok_or_else(|| std::io::Error::other("cases array"))?;
    assert!(cases.len() >= 13);
    let organic = cases
        .iter()
        .find(|case| case["id"] == "organic-edit-discovery")
        .ok_or_else(|| std::io::Error::other("organic discovery gate case"))?;
    let required = organic["required_tools"]
        .as_array()
        .ok_or_else(|| std::io::Error::other("required_tools array"))?;
    for tool in [
        "view_episode",
        "load_skill",
        "apply_edl",
        "view_program_frame",
    ] {
        assert!(required.iter().any(|value| value == tool), "missing {tool}");
    }
    Ok(())
}
