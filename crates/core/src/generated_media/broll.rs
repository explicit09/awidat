//! Shared agent-facing contract text for generated B-roll.
//!
//! The `find_generated_broll_opportunities` finder (both the legacy
//! `crates/core/src/tools/` copy and the MCP `montage_mcp/tools/` copy) and
//! the `plan_visual_support_proposals` proposal builder all emit the same
//! generation contract: the duration formula, the OpenRouter cost gate, and
//! the "scout first, then generate" next-step. Keeping the text in one place
//! stops the copies from drifting.
//!
//! Note: `#[tool(description = ...)]` in `montage_mcp::mod` only accepts a
//! string literal, so tool *descriptions* cannot reference these constants and
//! must mirror them by hand. These constants cover the emitted tool *output*
//! (JSON bodies and `next_step` fields), which is built at runtime and can
//! reference them directly.

pub use crate::montage_mcp::tools::start_generated_media_job::OPENROUTER_COST_CONFIRMATION;

/// The duration a generated B-roll clip must be requested and inserted at.
///
/// Mirrors [`crate::tools::find_generated_broll_opportunities`]'s
/// `job_duration_s` helper: `max(4, ceil(duration_s))` capped at 15.
pub const DURATION_CONTRACT: &str = "Use max(4, ceil(duration_s)) capped at 15 for start_generated_media_job and use_generated_media.";

/// Next-step guidance emitted alongside finder results: findings are scouting
/// only; the agent selects from transcript flow before generating.
pub const NEXT_STEP: &str = "Treat these findings as scouting only. First choose the B-roll moment from transcript flow and confirm the exact anchor, rationale, prompt, duration_s, and overlap/cancellation safety. If a finding still makes editorial sense, call start_generated_media_job with provider=openrouter, artifact_kind=video, workflow_purpose=broll, prompt, model, duration set to max(4, ceil(duration_s)) capped at 15, and cost_confirmation=\"OpenRouter cost unknown; explicit confirmation required\". After the job succeeds, call use_generated_media with the same clamped duration.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_step_uses_generated_media_duration_contract() {
        assert!(NEXT_STEP.contains("max(4, ceil(duration_s))"));
        assert!(NEXT_STEP.contains("capped at 15"));
        assert!(NEXT_STEP.contains("same clamped duration"));
        assert!(NEXT_STEP.contains(&format!(
            "cost_confirmation=\"{OPENROUTER_COST_CONFIRMATION}\""
        )));
        assert!(!NEXT_STEP.contains("round(duration_s)"));
    }

    #[test]
    fn duration_contract_matches_next_step_formula() {
        assert!(DURATION_CONTRACT.contains("max(4, ceil(duration_s)) capped at 15"));
    }
}
