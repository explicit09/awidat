use montage_core::montage_mcp::tools::start_generated_media_job::{
    StartGeneratedMediaJobArgs, validate_openrouter_cost_confirmation,
};

#[test]
fn openrouter_requires_visible_cost_confirmation_argument() {
    let args = StartGeneratedMediaJobArgs {
        provider: "openrouter".into(),
        artifact_kind: "video".into(),
        workflow_purpose: "broll".into(),
        prompt: "office b-roll".into(),
        ..Default::default()
    };

    let err = validate_openrouter_cost_confirmation(&args).unwrap_err();
    assert!(err.contains("OpenRouter cost unknown"));
}
