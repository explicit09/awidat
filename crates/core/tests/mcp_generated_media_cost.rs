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

    let err = match validate_openrouter_cost_confirmation(&args) {
        Ok(()) => panic!("OpenRouter cost confirmation should be required"),
        Err(err) => err,
    };
    assert!(err.contains("OpenRouter cost unknown"));
}
