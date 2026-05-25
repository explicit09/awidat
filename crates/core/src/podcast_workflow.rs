//! Podcast production workflow templates.
//!
//! This is the code-level guardrail for broad requests like "edit this
//! podcast": run deterministic intake gates first, then hand the
//! gathered evidence to the model for the creative edit pass.

use crate::structured_plan::StructuredPlan;

/// Returns true when the user's request should begin with the podcast
/// production intake instead of jumping directly into free-form edits.
pub fn should_run_podcast_production_intake(input: &str) -> bool {
    let text = input.to_ascii_lowercase();
    let mentions_podcast = [
        "podcast",
        "interview",
        "episode",
        "conversation",
        "recording",
        "guest",
        "host",
    ]
    .iter()
    .any(|needle| text.contains(needle));
    let broad_edit = [
        "edit",
        "produce",
        "clean up",
        "cleanup",
        "make this",
        "finish",
        "full episode",
        "ready for youtube",
        "publishable",
        "polish",
        "fix this recording",
        "cut this down",
        "tighten",
        "turn this into",
        "make it flow",
    ]
    .iter()
    .any(|needle| text.contains(needle));
    mentions_podcast && broad_edit
}

/// Build the mandatory podcast intake plan.
pub fn podcast_production_intake_plan() -> StructuredPlan {
    let mut plan = StructuredPlan::parse_json(serde_json::json!({
        "summary": "Podcast production intake",
        "steps": [
            {
                "id": "skill-context",
                "description": "Load the podcast production playbook",
                "allowed_tools": ["load_skill"],
                "model_tier": "fast",
                "verification": {
                    "required": true,
                    "tools": ["load_skill"],
                    "requirements": ["Podcast production skill guidance is available before edits"]
                },
                "actions": [
                    {"tool": "load_skill", "args": {"name": "podcast-episode-producer"}}
                ]
            },
            {
                "id": "readiness",
                "description": "Check media readiness and indexed evidence",
                "allowed_tools": ["read_media_readiness"],
                "model_tier": "fast",
                "depends_on": ["skill-context"],
                "verification": {
                    "required": true,
                    "tools": ["read_media_readiness"],
                    "requirements": ["Media/cache/index readiness is available before edits"]
                },
                "actions": [
                    {"tool": "read_media_readiness", "args": {}}
                ]
            },
            {
                "id": "episode-map",
                "description": "Read current episode and timeline map",
                "allowed_tools": ["view_episode"],
                "model_tier": "fast",
                "depends_on": ["readiness"],
                "verification": {
                    "required": true,
                    "tools": ["view_episode"],
                    "requirements": ["Current timeline state is known"]
                },
                "actions": [
                    {"tool": "view_episode", "args": {}}
                ]
            },
            {
                "id": "episode-bounds",
                "description": "Find publishable episode boundary evidence",
                "allowed_tools": ["find_episode_start"],
                "model_tier": "fast",
                "depends_on": ["episode-map"],
                "verification": {
                    "required": true,
                    "tools": ["find_episode_start"],
                    "requirements": ["Publishable start evidence is available before story cleanup"]
                },
                "actions": [
                    {"tool": "find_episode_start", "args": {}}
                ]
            },
            {
                "id": "episode-span",
                "description": "Plan candidate episode spans and multi-episode choices",
                "allowed_tools": ["podcast_episode_spans"],
                "model_tier": "fast",
                "depends_on": ["episode-bounds"],
                "verification": {
                    "required": true,
                    "tools": ["podcast_episode_spans"],
                    "requirements": ["Candidate episode start/end spans are known before cleanup"]
                },
                "actions": [
                    {"tool": "podcast_episode_spans", "args": {}}
                ]
            },
            {
                "id": "sync",
                "description": "Check source sync and drift risk",
                "allowed_tools": ["analyze_sync"],
                "model_tier": "fast",
                "depends_on": ["episode-span"],
                "verification": {
                    "required": true,
                    "tools": ["analyze_sync"],
                    "requirements": ["Audio/video sync risk has been checked"]
                },
                "actions": [
                    {"tool": "analyze_sync", "args": {"bucket_count": 2048}}
                ]
            },
            {
                "id": "story-map",
                "description": "Build first-watch story map and removal candidates",
                "allowed_tools": ["podcast_story_map"],
                "model_tier": {"effort": "medium"},
                "depends_on": ["sync", "episode-span"],
                "verification": {
                    "required": true,
                    "tools": ["podcast_story_map"],
                    "requirements": ["Hooks, story beats, tangents, dead air, and production chatter candidates are summarized"]
                },
                "actions": [
                    {"tool": "podcast_story_map", "args": {}}
                ]
            },
            {
                "id": "editorial-review-pack",
                "description": "Package cleanup and boundary evidence for active AI classification",
                "allowed_tools": ["podcast_editorial_review_pack"],
                "model_tier": "fast",
                "depends_on": ["story-map"],
                "verification": {
                    "required": true,
                    "tools": ["podcast_editorial_review_pack"],
                    "requirements": ["False-start, production-aside, and silence recall signals are packaged with transcript context for cut/keep/review classification"]
                },
                "actions": [
                    {"tool": "podcast_editorial_review_pack", "args": {}}
                ]
            },
            {
                "id": "cleanup-candidates",
                "description": "Group existing audio/transcript cleanup candidates by risk",
                "allowed_tools": ["podcast_cleanup_candidates"],
                "model_tier": "fast",
                "depends_on": ["editorial-review-pack"],
                "verification": {
                    "required": true,
                    "tools": ["podcast_cleanup_candidates"],
                    "requirements": ["Dead air, filler, and false-start candidates are grouped as safe/review/risky"]
                },
                "actions": [
                    {"tool": "podcast_cleanup_candidates", "args": {}}
                ]
            },
            {
                "id": "edit-proposal",
                "description": "Build a reviewable edit proposal with continuity gates",
                "allowed_tools": ["podcast_edit_proposal"],
                "model_tier": "fast",
                "depends_on": ["cleanup-candidates"],
                "verification": {
                    "required": true,
                    "tools": ["podcast_edit_proposal"],
                    "requirements": ["Safe/review/risky edits are proposed with approval and continuity requirements before mutation"]
                },
                "actions": [
                    {"tool": "podcast_edit_proposal", "args": {}}
                ]
            },
            {
                "id": "audio-polish",
                "description": "Check podcast audio polish and mix readiness",
                "allowed_tools": ["podcast_audio_polish"],
                "model_tier": "fast",
                "depends_on": ["edit-proposal"],
                "verification": {
                    "required": true,
                    "tools": ["podcast_audio_polish"],
                    "requirements": ["Loudness, clipping, noise, speaker balance, and audio processing recommendations are known before render"]
                },
                "actions": [
                    {"tool": "podcast_audio_polish", "args": {}}
                ]
            },
            {
                "id": "visual-polish",
                "description": "Check podcast visual polish, multicam, b-roll, chapters, lower thirds, and captions",
                "allowed_tools": ["podcast_visual_polish"],
                "model_tier": "fast",
                "depends_on": ["edit-proposal"],
                "verification": {
                    "required": true,
                    "tools": ["podcast_visual_polish"],
                    "requirements": ["Multicam, reaction-shot, b-roll, chapter/lower-third, and caption readiness is known before render"]
                },
                "actions": [
                    {"tool": "podcast_visual_polish", "args": {}}
                ]
            },
            {
                "id": "timeline-qc",
                "description": "Run pre-render podcast timeline QC",
                "allowed_tools": ["podcast_qc_report"],
                "model_tier": "fast",
                "depends_on": ["audio-polish", "visual-polish"],
                "verification": {
                    "required": true,
                    "tools": ["podcast_qc_report"],
                    "requirements": ["Timeline gaps, missing media, captions, audio metering, and suspicious structure are checked before render"]
                },
                "actions": [
                    {"tool": "podcast_qc_report", "args": {}}
                ]
            },
            {
                "id": "export-preflight",
                "description": "Check export backend and known limitations before promising deliverables",
                "allowed_tools": ["render_preflight"],
                "model_tier": "fast",
                "depends_on": ["timeline-qc"],
                "verification": {
                    "required": true,
                    "tools": ["render_preflight"],
                    "requirements": ["Render backend and output limitations are known"]
                },
                "actions": [
                    {"tool": "render_preflight", "args": {"scope": "timeline", "include_preview_cache": true, "preview_cache_max_tasks": 8}}
                ]
            }
        ]
    }))
    .expect("podcast production intake plan must be valid");
    plan.approve()
        .expect("podcast production intake plan starts approved");
    plan
}

/// Compact the deterministic intake report into context for the
/// follow-up model turn.
pub fn format_intake_report_for_agent(
    report: &crate::structured_plan::PlanExecutionReport,
) -> String {
    const MAX_TOOL_CHARS: usize = 4_000;
    let mut out = String::new();
    out.push_str("Podcast production intake completed. Use this evidence before editing.\n");
    out.push_str(
        "Mandatory follow-up behavior: follow the loaded podcast production skill; use the episode-bounds and episode-span evidence before deciding the publishable start/end; if episode-span reports requires_user_choice, ask which episode to produce before applying extraction or cleanup; use the editorial-review-pack evidence to classify cleanup/boundary packets as cut/keep/review from transcript context before trusting scanner labels; present the edit-proposal safe/review/risky groups before mutation; do not call apply_edl for review/risky proposal items until the user approves them and assess_edit_quality has been run for the proposal item's continuity_gate args; when proposal items are accepted, call podcast_apply_accepted_edits with explicit accepted_ids, then run its returned apply_edl call, then run view_timeline, vedit_diff, podcast_smooth_cut_boundaries, and podcast_post_draft_check; run every assess_edit_quality call returned by podcast_smooth_cut_boundaries before final render, even for safe cuts; if a boundary is dirty, prefer recut, J/L cut, or b-roll before visible transitions; when using b-roll or generated media, verify after apply_edl with view_timeline and podcast_visual_polish that every inserted asset matches its transcript anchor and the overlays are not accidentally clustered or shifted; do not claim b-roll is done until skipped stock/generated candidates are listed explicitly; after applying a draft extraction, cleanup, smoothing, or b-roll EDL, re-run podcast_audio_polish, podcast_visual_polish, and podcast_qc_report before render; do not start final render while podcast_qc_report is blocked; do not say the podcast is done unless the latest post-draft, audio polish, visual polish/B-roll review, and QC reports have been run after the last apply_edl and every non-clean item is explicitly fixed or listed as skipped/pending; preserve source timestamps and media references; ask before final render; after render, ask the user to review sync, captions, pacing, and audio quality.\n\n",
    );
    for step in &report.step_results {
        out.push_str(&format!("## Step: {}\n", step.step_id));
        for result in &step.tool_results {
            out.push_str(&format!("### {}\n", result.tool));
            if result.content.len() > MAX_TOOL_CHARS {
                out.push_str(&result.content[..MAX_TOOL_CHARS]);
                out.push_str("\n[truncated]\n");
            } else {
                out.push_str(&result.content);
                out.push('\n');
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_broad_podcast_edit_requests() {
        assert!(should_run_podcast_production_intake(
            "Hey please edit this podcast"
        ));
        assert!(should_run_podcast_production_intake(
            "produce the full episode"
        ));
        assert!(should_run_podcast_production_intake(
            "make this recording publishable"
        ));
        assert!(!should_run_podcast_production_intake(
            "what is happening at 1:40?"
        ));
    }

    #[test]
    fn intake_plan_contains_required_gates() {
        let plan = podcast_production_intake_plan();
        let ids: Vec<_> = plan.steps.iter().map(|step| step.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "skill-context",
                "readiness",
                "episode-map",
                "episode-bounds",
                "episode-span",
                "sync",
                "story-map",
                "cleanup-candidates",
                "edit-proposal",
                "audio-polish",
                "visual-polish",
                "timeline-qc",
                "export-preflight"
            ]
        );
        assert_eq!(plan.status, crate::structured_plan::PlanStatus::Approved);
        assert!(plan.steps.iter().any(|step| {
            step.allowed_tools
                .iter()
                .any(|tool| tool == "podcast_story_map")
        }));
        assert!(plan.steps.iter().any(|step| {
            step.id == "skill-context"
                && step
                    .actions
                    .iter()
                    .any(|action| action.tool == "load_skill")
        }));
        assert!(plan.steps.iter().any(|step| {
            step.id == "episode-bounds"
                && step
                    .actions
                    .iter()
                    .any(|action| action.tool == "find_episode_start")
        }));
        assert!(plan.steps.iter().any(|step| {
            step.id == "episode-span"
                && step
                    .actions
                    .iter()
                    .any(|action| action.tool == "podcast_episode_spans")
        }));
        assert!(plan.steps.iter().any(|step| {
            step.id == "edit-proposal"
                && step
                    .actions
                    .iter()
                    .any(|action| action.tool == "podcast_edit_proposal")
        }));
        assert!(plan.steps.iter().any(|step| {
            step.id == "audio-polish"
                && step
                    .actions
                    .iter()
                    .any(|action| action.tool == "podcast_audio_polish")
        }));
        assert!(plan.steps.iter().any(|step| {
            step.id == "visual-polish"
                && step
                    .actions
                    .iter()
                    .any(|action| action.tool == "podcast_visual_polish")
        }));
        assert!(plan.steps.iter().any(|step| {
            step.id == "timeline-qc"
                && step
                    .actions
                    .iter()
                    .any(|action| action.tool == "podcast_qc_report")
        }));
        let story_map = plan
            .steps
            .iter()
            .find(|step| step.id == "story-map")
            .expect("story-map step exists");
        assert!(story_map.depends_on.iter().any(|id| id == "episode-span"));
        let export_preflight = plan
            .steps
            .iter()
            .find(|step| step.id == "export-preflight")
            .expect("export-preflight step exists");
        assert!(
            export_preflight
                .depends_on
                .iter()
                .any(|id| id == "timeline-qc")
        );
    }
}
