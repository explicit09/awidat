#![allow(missing_docs)]

use awidat_core::edl::parser;
use awidat_core::short_form_review::{
    DurationClass, ShortFormReviewInput, ShortFormReviewOptions, build_short_form_review,
};
use awidat_core::tool::{SandboxMode, ToolContext, ToolHandler, ToolInvocation};
use awidat_core::tools::apply_edl::ApplyEdlTool;
use awidat_core::tools::plan_short_form_review::PlanShortFormReviewTool;
use tokio::sync::broadcast;

fn review_input() -> ShortFormReviewInput {
    ShortFormReviewInput {
        asset_id: "raw/founder-interview.mp4".to_string(),
        source_width: 3840,
        source_height: 2160,
        transcript: serde_json::json!({
            "segments": [
                {
                    "start_s": 12.0,
                    "end_s": 190.0,
                    "speaker_id": "host",
                    "text": "Here is why AI coding is changing faster than people think. The old model was waiting for a tool to finish, but now the important shift is reviewing complete work. That changes how teams ship because the review loop becomes the product."
                },
                {
                    "start_s": 220.0,
                    "end_s": 245.0,
                    "speaker_id": "guest",
                    "text": "Yeah um I think we were just kind of looking at the thing and you know it was interesting."
                }
            ],
            "speakers": [{"id": "host"}, {"id": "guest"}]
        }),
        editorial_moments: serde_json::json!({
            "moments": [
                {
                    "kind": "explainer",
                    "start_s": 12.0,
                    "end_s": 190.0,
                    "score": 0.82,
                    "text": "Here is why AI coding is changing faster than people think. The old model was waiting for a tool to finish, but now the important shift is reviewing complete work. That changes how teams ship because the review loop becomes the product.",
                    "reason": "complete standalone explanation with a strong claim"
                },
                {
                    "kind": "aside",
                    "start_s": 220.0,
                    "end_s": 245.0,
                    "score": 0.42,
                    "text": "Yeah um I think we were just kind of looking at the thing and you know it was interesting.",
                    "reason": "low clarity aside"
                }
            ]
        }),
        audio_energy: serde_json::json!({
            "loudness_integrated_lufs": -18.0,
            "silences": [{"start_s": 82.0, "end_s": 82.8}]
        }),
        topics: serde_json::json!({
            "topics": [{
                "start_s": 0.0,
                "end_s": 210.0,
                "label": "AI coding review loop"
            }]
        }),
        scenes: serde_json::json!({
            "shots": [
                {"start_s": 0.0, "end_s": 60.0},
                {"start_s": 60.0, "end_s": 120.0},
                {"start_s": 120.0, "end_s": 210.0}
            ]
        }),
        shot: serde_json::json!({
            "shots": [{"start_s": 0.0, "end_s": 210.0, "shot_type": "two_shot"}]
        }),
        face: serde_json::json!({
            "per_frame": [{
                "t_s": 20.0,
                "faces": [
                    {"confidence": 0.98, "x": 0.22, "y": 0.42, "w": 0.18, "h": 0.28},
                    {"confidence": 0.97, "x": 0.62, "y": 0.42, "w": 0.18, "h": 0.28}
                ]
            }]
        }),
        gaze: serde_json::json!({"segments": []}),
        frame_quality: serde_json::json!({"regions": []}),
        composition: serde_json::json!({"regions": []}),
    }
}

fn transcript_only_input() -> ShortFormReviewInput {
    ShortFormReviewInput {
        asset_id: "raw/transcript-only.mp4".to_string(),
        source_width: 1920,
        source_height: 1080,
        transcript: serde_json::json!({
            "segments": [
                {
                    "start_s": 10.0,
                    "end_s": 42.0,
                    "speaker_id": "guest",
                    "text": "Here is why data centers are misunderstood."
                },
                {
                    "start_s": 42.0,
                    "end_s": 86.0,
                    "speaker_id": "guest",
                    "text": "People compare the power draw to one building, but the useful comparison is the full infrastructure behind streaming, banking, and AI."
                },
                {
                    "start_s": 86.0,
                    "end_s": 132.0,
                    "speaker_id": "guest",
                    "text": "Because every digital product has hidden physical systems, the better question is whether the system creates enough value for the energy it uses."
                },
                {
                    "start_s": 132.0,
                    "end_s": 156.0,
                    "speaker_id": "guest",
                    "text": "That changes the debate from outrage to measurement."
                }
            ],
            "speakers": [{"id": "guest"}]
        }),
        editorial_moments: serde_json::json!({"moments": []}),
        audio_energy: serde_json::json!({}),
        topics: serde_json::json!({
            "topics": [{
                "start_s": 10.0,
                "end_s": 156.0,
                "label": "data center energy comparison"
            }]
        }),
        scenes: serde_json::json!({"shots": []}),
        shot: serde_json::json!({"shots": []}),
        face: serde_json::json!({"per_frame": []}),
        gaze: serde_json::json!({"segments": []}),
        frame_quality: serde_json::json!({"regions": []}),
        composition: serde_json::json!({"regions": []}),
    }
}

fn ctx_at(root: &std::path::Path) -> ToolContext {
    let (tx, _) = broadcast::channel(8);
    ToolContext {
        project_root: root.to_path_buf(),
        events_tx: tx,
        user_input_tx: None,
        job_manager: awidat_render::JobManager::new(),
        approval_tx: None,
        sandbox_mode: SandboxMode::Default,
        mcp_host: awidat_core::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
            name: "test".into(),
            version: "0.0.0".into(),
        }),
        skills: std::sync::Arc::new(awidat_core::skills::SkillRegistry::default()),
        subagent_return: None,
    }
}

fn write_sidecar(root: &std::path::Path, indexer: &str, asset: &str, data: serde_json::Value) {
    let path = root
        .join("index")
        .join(indexer)
        .join(format!("{asset}.json"));
    let Some(parent) = path.parent() else {
        panic!("sidecar path should have parent");
    };
    std::fs::create_dir_all(parent).unwrap_or_else(|err| {
        panic!("failed to create sidecar dir: {err}");
    });
    let body = serde_json::json!({
        "indexer": indexer,
        "asset_id": asset,
        "data": data,
    });
    let bytes = serde_json::to_vec_pretty(&body).unwrap_or_else(|err| {
        panic!("failed to serialize sidecar: {err}");
    });
    std::fs::write(path, bytes).unwrap_or_else(|err| {
        panic!("failed to write sidecar: {err}");
    });
}

#[test]
fn ranks_complete_extended_candidates_and_prefers_quality_over_duration() {
    let review = build_short_form_review(
        review_input(),
        ShortFormReviewOptions {
            max_candidates: 3,
            max_duration_s: 300.0,
        },
    );

    let Some(top) = review.candidates.first() else {
        panic!("expected ranked candidates");
    };
    assert_eq!(top.duration_class, DurationClass::Extended);
    assert!(top.source_range.duration_s > 90.0);
    assert!(top.score.total > 0.0);
    assert!(
        top.score.completeness > top.score.rambling_penalty,
        "complete ideas should beat rambling penalties"
    );
    assert!(
        top.why_ai_picked_it
            .iter()
            .any(|reason| reason.contains("standalone"))
    );
}

#[test]
fn transcript_only_discovery_builds_complete_topic_windows() {
    let review = build_short_form_review(
        transcript_only_input(),
        ShortFormReviewOptions {
            max_candidates: 3,
            max_duration_s: 300.0,
        },
    );

    let Some(top) = review.candidates.first() else {
        panic!("expected transcript-derived candidate");
    };
    assert_eq!(top.duration_class, DurationClass::Extended);
    assert_eq!(top.source_range.start_s, 10.0);
    assert_eq!(top.source_range.end_s, 156.0);
    assert!(
        top.story_arc.payoff.contains("measurement"),
        "candidate should include the payoff segment"
    );
    assert!(
        top.broll_plan
            .suggestions
            .iter()
            .any(|suggestion| suggestion.contains("servers")),
        "data center topic should create a concrete B-roll suggestion"
    );
}

#[test]
fn candidate_packet_includes_broll_layout_captions_metadata_and_edl() {
    let review = build_short_form_review(
        review_input(),
        ShortFormReviewOptions {
            max_candidates: 1,
            max_duration_s: 300.0,
        },
    );

    let Some(packet) = review.candidates.first() else {
        panic!("expected candidate packet");
    };
    assert!(packet.broll_plan.needed);
    assert!(!packet.broll_plan.suggestions.is_empty());
    assert_eq!(packet.vertical_layout.target_aspect_ratio, "9:16");
    assert!(packet.vertical_layout.strategy.contains("speaker"));
    assert!(
        packet
            .vertical_layout
            .notes
            .iter()
            .any(|note| note.contains("scene/shot boundaries"))
    );
    assert!(
        packet
            .vertical_layout
            .notes
            .iter()
            .any(|note| note.contains("two_shot"))
    );
    assert!(!packet.caption_plan.highlight_terms.is_empty());
    assert!(packet.suggested_title.contains("AI"));
    assert!(packet.review_actions.contains(&"approve".to_string()));
    assert!(
        packet
            .review_actions
            .contains(&"add_remove_broll".to_string())
    );
    assert!(packet.draft_edl.contains("*** Begin EDL"));
    assert!(packet.draft_edl.contains("*** Set Output Format"));
    assert!(packet.draft_edl.contains("*** Insert Clip"));
    assert!(packet.draft_edl.contains("*** Insert Caption"));
    assert!(packet.draft_edl.contains("*** Set Package Metadata"));
    if let Err(err) = parser::parse(&packet.draft_edl) {
        panic!("draft EDL should parse: {err}");
    }
    assert_eq!(review.proposal_policy.apply_tool, "apply_edl");
    assert!(
        review
            .proposal_policy
            .approval_note
            .contains("autopilot/co-pilot/manual")
    );
}

#[test]
fn review_tool_is_read_only_but_apply_edl_proposals_are_permission_gated() {
    let review = build_short_form_review(
        review_input(),
        ShortFormReviewOptions {
            max_candidates: 1,
            max_duration_s: 300.0,
        },
    );
    let Some(packet) = review.candidates.first() else {
        panic!("expected candidate packet");
    };
    let invocation = ToolInvocation {
        call_id: "call-1".to_string(),
        name: "plan_short_form_review".to_string(),
        args: serde_json::json!({"asset_id": "raw/founder-interview.mp4"}),
    };

    assert!(!PlanShortFormReviewTool.is_mutating(&invocation));
    assert!(ApplyEdlTool.is_mutating(&ToolInvocation {
        call_id: "call-2".to_string(),
        name: "apply_edl".to_string(),
        args: serde_json::json!({"edl": packet.draft_edl}),
    }));
    assert!(!ApplyEdlTool.is_mutating(&ToolInvocation {
        call_id: "call-3".to_string(),
        name: "apply_edl".to_string(),
        args: serde_json::json!({"edl": packet.draft_edl, "dry_run": true}),
    }));
}

#[tokio::test]
async fn plan_short_form_review_tool_reads_sidecars_and_returns_review_packets() {
    let dir = tempfile::tempdir().unwrap_or_else(|err| {
        panic!("failed to create temp dir: {err}");
    });
    let asset = "raw/transcript-only.mp4";
    let input = transcript_only_input();
    write_sidecar(dir.path(), "whisper", asset, input.transcript);
    write_sidecar(dir.path(), "topic", asset, input.topics);
    write_sidecar(dir.path(), "face", asset, input.face);

    let output = PlanShortFormReviewTool
        .handle(
            ToolInvocation {
                call_id: "call-4".to_string(),
                name: "plan_short_form_review".to_string(),
                args: serde_json::json!({
                    "asset_id": asset,
                    "source_width": 1920,
                    "source_height": 1080,
                    "max_candidates": 2
                }),
            },
            ctx_at(dir.path()),
        )
        .await
        .unwrap_or_else(|err| {
            panic!("tool should return review packets: {err}");
        });

    let review: serde_json::Value = serde_json::from_str(&output.content).unwrap_or_else(|err| {
        panic!("tool output should be JSON: {err}");
    });
    let candidates = review
        .get("candidates")
        .and_then(|value| value.as_array())
        .unwrap_or_else(|| {
            panic!("tool output should include candidates array");
        });
    assert!(!candidates.is_empty());
    assert_eq!(
        review.pointer("/proposal_policy/apply_tool"),
        Some(&serde_json::json!("apply_edl"))
    );
}
