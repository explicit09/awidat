#![allow(clippy::expect_used, clippy::unwrap_used)]
#![allow(missing_docs)]

use montage_core::edl::parser;
use montage_core::montage_mcp::context::McpToolCtx;
use montage_core::montage_mcp::tools::plan_short_form_review::{
    self, PlanShortFormReviewArgs, ShortFormDiscoveryModeArg, ShortFormProfileArg,
};
use montage_core::montage_mcp::tools::podcast_flow_shape::{PodcastFlowShapeArgs, run};
use montage_core::short_form_review::{
    DurationClass, ShortFormCompositionMode, ShortFormDiscoveryMode, ShortFormLayoutMode,
    ShortFormProfile, ShortFormReviewInput, ShortFormReviewOptions, build_short_form_review,
};

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
                    "text": "Here is why AI coding is changing faster than people think. The old model was waiting for a tool to finish, but now the important shift is reviewing complete work. That changes how teams ship because the review loop becomes the product.",
                    "words": [
                        {"start_s": 12.4, "end_s": 12.7, "text": "Here"},
                        {"start_s": 12.7, "end_s": 12.9, "text": "is"},
                        {"start_s": 12.9, "end_s": 13.1, "text": "why"},
                        {"start_s": 13.1, "end_s": 13.4, "text": "AI"},
                        {"start_s": 13.4, "end_s": 13.8, "text": "coding"},
                        {"start_s": 13.8, "end_s": 14.0, "text": "is"},
                        {"start_s": 14.0, "end_s": 14.4, "text": "changing"},
                        {"start_s": 14.4, "end_s": 14.8, "text": "faster"},
                        {"start_s": 14.8, "end_s": 15.0, "text": "than"},
                        {"start_s": 15.0, "end_s": 15.4, "text": "people"}
                    ]
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
        broll_assets: serde_json::json!({
            "candidates": [{
                "asset_id": "raw/broll/ai-coding-ui.mp4",
                "query_terms": ["AI", "coding", "review"],
                "start_s": 0.0,
                "end_s": 6.0,
                "source": "existing_asset",
                "score": 0.91
            }]
        }),
        trend_context: serde_json::json!({}),
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
        broll_assets: serde_json::json!({
            "candidates": [{
                "asset_id": "raw/broll/server-racks.mp4",
                "query_terms": ["data", "center", "energy", "servers"],
                "start_s": 2.0,
                "end_s": 8.0,
                "source": "existing_asset",
                "score": 0.88
            }]
        }),
        trend_context: serde_json::json!({}),
    }
}

fn ctx_at(root: &std::path::Path) -> McpToolCtx {
    McpToolCtx {
        project_root: root.to_path_buf(),
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
            profile: ShortFormProfile::EditorialReview,
            discovery_mode: ShortFormDiscoveryMode::Review,
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
            profile: ShortFormProfile::EditorialReview,
            discovery_mode: ShortFormDiscoveryMode::Review,
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
fn fused_clip_and_broll_recommendations_drive_review_packets() {
    let mut input = transcript_only_input();
    input.transcript = serde_json::json!({"segments": []});
    input.editorial_moments = serde_json::json!({
        "source": "fused_understanding",
        "moments": [],
        "clip_candidates": [{
            "id": "clip-candidate:asset:moment:5000",
            "kind": "clip_candidate",
            "start_s": 5.0,
            "end_s": 18.0,
            "score": 0.92,
            "text": "Retention improved by 42 percent once the transcript pipeline became stable.",
            "reason": "Strong statistic candidate from fused understanding.",
            "evidence_ids": ["understanding:asset:stat:5000"]
        }]
    });
    input.broll_assets = serde_json::json!({
        "source": "fused_understanding",
        "recommendations": [{
            "id": "broll-rec:asset:stat:5000",
            "moment_id": "clip-candidate:asset:moment:5000",
            "start_s": 5.0,
            "end_s": 18.0,
            "score": 0.94,
            "category": "statistic",
            "strategy": "motion_graphic",
            "rationale": "Turn the numeric claim into an animated stat card.",
            "placement": "motion_graphic_overlay"
        }],
        "candidates": []
    });

    let review = build_short_form_review(
        input,
        ShortFormReviewOptions {
            max_candidates: 1,
            max_duration_s: 60.0,
            profile: ShortFormProfile::EditorialReview,
            discovery_mode: ShortFormDiscoveryMode::Review,
        },
    );

    let candidate = review
        .candidates
        .first()
        .expect("fused clip candidate should produce review packet");
    assert!(
        candidate
            .evidence
            .iter()
            .any(|line| line.contains("clip-candidate:asset:moment:5000"))
    );
    assert_eq!(
        candidate.broll_plan.rationale,
        "Turn the numeric claim into an animated stat card."
    );
    assert!(
        candidate
            .broll_plan
            .suggestions
            .iter()
            .any(|suggestion| suggestion.contains("statistic"))
    );
}

#[test]
fn harvest_mode_merges_clip_candidates_moments_and_transcript_windows() {
    let mut input = transcript_only_input();
    input.editorial_moments = serde_json::json!({
        "source": "fused_understanding",
        "moments": [{
            "id": "moment:space-data-centers",
            "kind": "explanation",
            "start_s": 10.0,
            "end_s": 156.0,
            "score": 0.82,
            "text": "Here is why data centers are misunderstood. People compare the power draw to one building, but the useful comparison is the full infrastructure behind streaming, banking, and AI. That changes the debate from outrage to measurement.",
            "reason": "complete educational topic window"
        }],
        "clip_candidates": [{
            "id": "clip-candidate:space-data-centers:short",
            "kind": "clip_candidate",
            "start_s": 42.0,
            "end_s": 86.0,
            "score": 0.91,
            "text": "People compare the power draw to one building, but the useful comparison is the full infrastructure behind streaming, banking, and AI.",
            "reason": "shorter high-retention excerpt"
        }]
    });

    let review = build_short_form_review(
        input,
        ShortFormReviewOptions {
            max_candidates: 10,
            max_duration_s: 300.0,
            profile: ShortFormProfile::ViralSocial,
            discovery_mode: ShortFormDiscoveryMode::Harvest,
        },
    );

    assert_eq!(review.discovery_mode, ShortFormDiscoveryMode::Harvest);
    assert!(
        review.candidates.iter().any(|candidate| candidate
            .evidence
            .iter()
            .any(|line| { line.contains("clip-candidate:space-data-centers:short") })),
        "harvest should retain fused clip candidates"
    );
    assert!(
        review.candidates.iter().any(|candidate| candidate
            .evidence
            .iter()
            .any(|line| { line.contains("moment:space-data-centers") })),
        "harvest should also include broader editorial moments"
    );
    assert!(
        review
            .candidates
            .iter()
            .any(|candidate| candidate.variant_kind != "primary"),
        "harvest should expose alternate cuts instead of only final picks"
    );
}

#[test]
fn harvest_variants_keep_primary_candidate_links_overlap_and_range_text() {
    let mut input = transcript_only_input();
    input.transcript = serde_json::json!({
        "segments": [
            {
                "start_s": 10.0,
                "end_s": 40.0,
                "speaker_id": "guest",
                "text": "Opening setup explains why the old comparison is misleading."
            },
            {
                "start_s": 40.0,
                "end_s": 70.0,
                "speaker_id": "guest",
                "text": "Middle detail explains the infrastructure behind every digital product."
            },
            {
                "start_s": 70.0,
                "end_s": 100.0,
                "speaker_id": "guest",
                "text": "Payoff reframes the argument around value and measurement."
            }
        ],
        "speakers": [{"id": "guest"}]
    });
    input.editorial_moments = serde_json::json!({
        "moments": [{
            "id": "moment:comparison",
            "kind": "explanation",
            "start_s": 10.0,
            "end_s": 100.0,
            "score": 0.92,
            "text": "Opening setup explains why the old comparison is misleading. Middle detail explains the infrastructure behind every digital product. Payoff reframes the argument around value and measurement.",
            "reason": "complete educational topic window"
        }]
    });

    let review = build_short_form_review(
        input,
        ShortFormReviewOptions {
            max_candidates: 10,
            max_duration_s: 300.0,
            profile: ShortFormProfile::ViralSocial,
            discovery_mode: ShortFormDiscoveryMode::Harvest,
        },
    );

    let standard = review
        .candidates
        .iter()
        .find(|candidate| candidate.variant_kind == "standard")
        .expect("standard harvested variant");
    let primary = review
        .candidates
        .iter()
        .find(|candidate| {
            candidate.variant_kind == "primary" && candidate.cluster_id == standard.cluster_id
        })
        .expect("primary candidate for harvested cluster");

    assert_eq!(
        standard.primary_candidate_id.as_deref(),
        Some(primary.candidate_id.as_str())
    );
    assert!(standard.overlap_ratio > 0.0);
    assert!(standard.hook.contains("Opening setup"));
    assert!(
        !standard.hook.contains("Payoff reframes"),
        "standard variant text should be limited to the selected range"
    );
}

#[test]
fn harvest_candidate_ids_stay_unique_for_same_range_sources() {
    let mut input = transcript_only_input();
    input.transcript = serde_json::json!({
        "segments": [
            {
                "start_s": 10.0,
                "end_s": 100.0,
                "speaker_id": "guest",
                "text": "A complete explanation spans the exact same range as an editorial moment."
            }
        ],
        "speakers": [{"id": "guest"}]
    });
    input.editorial_moments = serde_json::json!({
        "moments": [{
            "id": "moment:same-range",
            "kind": "explanation",
            "start_s": 10.0,
            "end_s": 100.0,
            "score": 0.92,
            "text": "A complete explanation spans the exact same range as an editorial moment.",
            "reason": "same range as transcript segment"
        }]
    });

    let review = build_short_form_review(
        input,
        ShortFormReviewOptions {
            max_candidates: 10,
            max_duration_s: 300.0,
            profile: ShortFormProfile::ViralSocial,
            discovery_mode: ShortFormDiscoveryMode::Harvest,
        },
    );

    let mut ids = std::collections::BTreeSet::new();
    for candidate in &review.candidates {
        assert!(
            ids.insert(candidate.candidate_id.clone()),
            "duplicate candidate id: {}",
            candidate.candidate_id
        );
    }
}

#[test]
fn dynamic_layout_keeps_split_fallback_and_covers_gaps() {
    let mut input = transcript_only_input();
    input.transcript = serde_json::json!({
        "segments": [
            {"start_s": 15.0, "end_s": 25.0, "speaker_id": "host", "text": "Host explains the setup."},
            {"start_s": 35.0, "end_s": 45.0, "speaker_id": "guest", "text": "Guest lands the reply."}
        ],
        "speakers": [{"id": "host"}, {"id": "guest"}]
    });
    input.editorial_moments = serde_json::json!({
        "moments": [{
            "kind": "dialogue",
            "start_s": 10.0,
            "end_s": 50.0,
            "score": 0.85,
            "text": "Host explains the setup. Guest lands the reply.",
            "reason": "two-speaker exchange"
        }]
    });
    input.face = serde_json::json!({
        "per_frame": [{
            "t_s": 12.0,
            "faces": [
                {"confidence": 0.98, "x": 0.22, "y": 0.42, "w": 0.18, "h": 0.28},
                {"confidence": 0.97, "x": 0.62, "y": 0.42, "w": 0.18, "h": 0.28}
            ]
        }]
    });

    let review = build_short_form_review(
        input.clone(),
        ShortFormReviewOptions {
            max_candidates: 1,
            max_duration_s: 90.0,
            profile: ShortFormProfile::ViralSocial,
            discovery_mode: ShortFormDiscoveryMode::Review,
        },
    );
    let packet = review.candidates.first().expect("dialogue candidate");
    assert_eq!(
        packet.vertical_layout.composition_mode,
        ShortFormCompositionMode::DynamicSwitching
    );
    assert_eq!(
        packet.vertical_layout.segments.first().unwrap().start_s,
        10.0
    );
    assert_eq!(packet.vertical_layout.segments.last().unwrap().end_s, 50.0);
    for pair in packet.vertical_layout.segments.windows(2) {
        assert_eq!(pair[0].end_s, pair[1].start_s);
    }

    input.transcript = serde_json::json!({
        "segments": [
            {"start_s": 10.0, "end_s": 50.0, "text": "Two people are visible, but diarized speaker timing is unavailable."}
        ],
        "speakers": [{"id": "host"}, {"id": "guest"}]
    });
    let review = build_short_form_review(
        input,
        ShortFormReviewOptions {
            max_candidates: 1,
            max_duration_s: 90.0,
            profile: ShortFormProfile::ViralSocial,
            discovery_mode: ShortFormDiscoveryMode::Review,
        },
    );
    let packet = review.candidates.first().expect("fallback candidate");
    assert_eq!(
        packet.vertical_layout.composition_mode,
        ShortFormCompositionMode::SplitStacked
    );
    assert_eq!(
        packet.vertical_layout.segments[0].layout,
        ShortFormLayoutMode::SplitStacked
    );
}

#[test]
fn active_speaker_fill_targets_single_speaker_from_transcript() {
    let mut input = transcript_only_input();
    input.transcript = serde_json::json!({
        "segments": [
            {"start_s": 10.0, "end_s": 50.0, "speaker_id": "guest", "text": "The guest carries this full selected moment."}
        ],
        "speakers": [{"id": "host"}, {"id": "guest"}]
    });
    input.editorial_moments = serde_json::json!({
        "moments": [{
            "kind": "explanation",
            "start_s": 10.0,
            "end_s": 50.0,
            "score": 0.85,
            "text": "The guest carries this full selected moment.",
            "reason": "single diarized speaker in a two-person source"
        }]
    });
    input.face = serde_json::json!({
        "per_frame": [{
            "t_s": 12.0,
            "faces": [
                {"confidence": 0.98, "x": 0.22, "y": 0.42, "w": 0.18, "h": 0.28},
                {"confidence": 0.97, "x": 0.62, "y": 0.42, "w": 0.18, "h": 0.28}
            ]
        }]
    });

    let review = build_short_form_review(
        input,
        ShortFormReviewOptions {
            max_candidates: 1,
            max_duration_s: 90.0,
            profile: ShortFormProfile::ViralSocial,
            discovery_mode: ShortFormDiscoveryMode::Review,
        },
    );
    let packet = review.candidates.first().expect("single speaker candidate");

    assert_eq!(
        packet.vertical_layout.composition_mode,
        ShortFormCompositionMode::ActiveSpeakerFill
    );
    assert_eq!(
        packet.vertical_layout.segments[0].layout,
        ShortFormLayoutMode::FillSpeaker {
            speaker_id: "guest".to_string()
        }
    );
}

#[test]
fn podcast_flow_shape_preserves_missing_transcript_status() {
    let temp = tempfile::tempdir().expect("temp project");
    write_sidecar(
        temp.path(),
        "whisper",
        "raw/empty.mp4",
        serde_json::json!({"segments": []}),
    );

    let output = run(
        PodcastFlowShapeArgs {
            asset_id: Some("raw/empty.mp4".to_string()),
        },
        McpToolCtx {
            project_root: temp.path().to_path_buf(),
        },
    )
    .expect("podcast flow shape output");
    let report: serde_json::Value = serde_json::from_str(&output).expect("json report");

    assert_eq!(report["status"], "missing_transcript");
    assert!(
        report["missing_evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "whisper transcript segments")
    );
}

#[test]
fn candidate_packet_includes_broll_layout_captions_metadata_and_edl() {
    let review = build_short_form_review(
        review_input(),
        ShortFormReviewOptions {
            max_candidates: 1,
            max_duration_s: 300.0,
            profile: ShortFormProfile::EditorialReview,
            discovery_mode: ShortFormDiscoveryMode::Review,
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
    for action in [
        "approve",
        "reject",
        "extend_start",
        "extend_end",
        "change_caption_style",
        "add_remove_broll",
        "export",
    ] {
        assert!(
            packet
                .review_action_contracts
                .iter()
                .any(|contract| contract.action == action),
            "missing action contract for {action}"
        );
    }
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
fn v2_packet_attaches_broll_generates_reframe_caption_groups_and_workflow_commands() {
    let review = build_short_form_review(
        review_input(),
        ShortFormReviewOptions {
            max_candidates: 1,
            max_duration_s: 300.0,
            profile: ShortFormProfile::EditorialReview,
            discovery_mode: ShortFormDiscoveryMode::Review,
        },
    );

    let Some(packet) = review.candidates.first() else {
        panic!("expected candidate packet");
    };
    assert!(
        packet
            .broll_plan
            .attached_assets
            .iter()
            .any(|asset| asset.asset_id == "raw/broll/ai-coding-ui.mp4")
    );
    assert!(
        packet
            .broll_plan
            .acquisition_queries
            .iter()
            .any(|query| query.contains("AI coding"))
    );
    assert!(
        packet
            .broll_plan
            .acquisition_commands
            .iter()
            .any(|command| command.tool == "search_broll")
    );
    assert!(
        packet
            .broll_plan
            .acquisition_commands
            .iter()
            .any(|command| command.tool == "find_generated_broll_opportunities")
    );
    assert_eq!(packet.caption_plan.style, "bold_keyword");
    assert!(
        packet
            .caption_plan
            .style_options
            .contains(&"karaoke".to_string())
    );
    assert!(!packet.caption_plan.groups.is_empty());
    assert_eq!(packet.caption_plan.groups[0].start_s, 0.4);
    assert!(
        packet
            .caption_plan
            .groups
            .iter()
            .all(|group| (3..=7).contains(&group.word_count))
    );
    assert!(
        packet
            .vertical_layout
            .edl_operations
            .iter()
            .any(|op| op.contains("*** Set Effect") && op.contains("montage.reframe"))
    );
    assert!(packet.draft_edl.contains("*** Insert BRoll"));
    assert!(packet.draft_edl.contains("raw/broll/ai-coding-ui.mp4"));
    assert!(packet.draft_edl.contains("*** Set Effect"));
    if let Err(err) = parser::parse(&packet.draft_edl) {
        panic!("V2 draft EDL should parse: {err}");
    }
    assert_eq!(packet.workflow.preview.tool, "apply_edl");
    assert_eq!(
        packet.workflow.preview.args["dry_run"],
        serde_json::json!(true)
    );
    assert_eq!(packet.workflow.apply.tool, "apply_edl");
    assert_eq!(
        packet.workflow.apply.args["dry_run"],
        serde_json::json!(false)
    );
    assert_eq!(packet.workflow.export.tool, "export_package");
    assert_eq!(
        packet.workflow.export.args["format"],
        serde_json::json!("shorts")
    );
    assert_eq!(
        packet.workflow.render_preview.args["scope"],
        serde_json::json!("segment")
    );
    assert_eq!(
        packet.workflow.render_preview.args["asset"],
        serde_json::json!("raw/founder-interview.mp4")
    );
    assert!(!packet.workflow.preview.approval_required);
    assert!(packet.workflow.apply.approval_required);
    assert!(packet.workflow.render_preview.approval_required);
    assert!(packet.workflow.export.approval_required);
}

#[test]
fn viral_social_profile_coexists_with_editorial_review_and_changes_packet_generation() {
    let editorial = build_short_form_review(
        review_input(),
        ShortFormReviewOptions {
            max_candidates: 1,
            max_duration_s: 300.0,
            profile: ShortFormProfile::EditorialReview,
            discovery_mode: ShortFormDiscoveryMode::Review,
        },
    );
    let viral = build_short_form_review(
        review_input(),
        ShortFormReviewOptions {
            max_candidates: 1,
            max_duration_s: 300.0,
            profile: ShortFormProfile::ViralSocial,
            discovery_mode: ShortFormDiscoveryMode::Review,
        },
    );

    let Some(editorial_packet) = editorial.candidates.first() else {
        panic!("expected editorial packet");
    };
    let Some(viral_packet) = viral.candidates.first() else {
        panic!("expected viral packet");
    };

    assert_eq!(editorial.profile, ShortFormProfile::EditorialReview);
    assert_eq!(viral.profile, ShortFormProfile::ViralSocial);
    assert_eq!(
        editorial_packet.profile_plan.profile,
        ShortFormProfile::EditorialReview
    );
    assert_eq!(
        viral_packet.profile_plan.profile,
        ShortFormProfile::ViralSocial
    );
    assert!(viral_packet.score.hook_strength >= editorial_packet.score.hook_strength);
    assert!(viral_packet.score.retention_prediction >= editorial_packet.score.retention_prediction);
    assert_ne!(
        viral_packet.caption_plan.style,
        editorial_packet.caption_plan.style
    );
    assert_ne!(
        viral_packet.platform_variants[0].platform,
        editorial_packet.platform_variants[0].platform
    );
}

#[test]
fn viral_social_packet_contains_aggressive_pacing_overlay_sound_and_parseable_edl() {
    let review = build_short_form_review(
        review_input(),
        ShortFormReviewOptions {
            max_candidates: 1,
            max_duration_s: 300.0,
            profile: ShortFormProfile::ViralSocial,
            discovery_mode: ShortFormDiscoveryMode::Review,
        },
    );

    let Some(packet) = review.candidates.first() else {
        panic!("expected viral packet");
    };
    assert_eq!(packet.profile_plan.pace, "aggressive");
    assert_eq!(packet.profile_plan.trim_policy.pause_policy, "aggressive");
    assert!(
        packet
            .profile_plan
            .trim_policy
            .targets
            .contains(&"remove_filler_words".to_string())
    );
    assert_eq!(packet.profile_plan.motion_policy.cadence_s, (1.0, 3.0));
    assert!(
        packet
            .profile_plan
            .motion_policy
            .edl_operations
            .iter()
            .any(|op| op.contains("viral_punch_in"))
    );
    assert!(
        packet
            .profile_plan
            .overlay_recommendations
            .iter()
            .any(|overlay| overlay.kind == "meme_overlay")
    );
    assert!(
        packet
            .profile_plan
            .sound_cues
            .iter()
            .any(|cue| cue.kind == "whoosh")
    );
    assert_eq!(packet.caption_plan.group_words_max, 4);
    assert_eq!(packet.caption_plan.style, "viral_kinetic_keyword");
    assert_eq!(packet.platform_variants[0].platform, "tiktok");
    assert!(packet.draft_edl.contains("*** Insert Title"));
    assert!(packet.draft_edl.contains("viral_punch_in"));
    if let Err(err) = parser::parse(&packet.draft_edl) {
        panic!("viral draft EDL should parse: {err}");
    }
    assert!(!packet.workflow.preview.approval_required);
    assert!(packet.workflow.apply.approval_required);
}

#[test]
fn ranked_sets_expose_top_five_candidate_packets_per_bucket() {
    let review = build_short_form_review(
        review_input(),
        ShortFormReviewOptions {
            max_candidates: 10,
            max_duration_s: 300.0,
            profile: ShortFormProfile::EditorialReview,
            discovery_mode: ShortFormDiscoveryMode::Review,
        },
    );

    assert!(review.ranked_sets.safest.iter().all(|entry| entry.rank > 0));
    assert!(
        review
            .ranked_sets
            .educational
            .iter()
            .any(|entry| entry.rationale.contains("educational"))
    );
    assert!(
        review
            .ranked_sets
            .viral_risk
            .iter()
            .any(|entry| entry.candidate_id.starts_with("short_"))
    );
    assert!(review.ranked_sets.safest.len() <= 5);
    assert!(review.ranked_sets.viral_risk.len() <= 5);
    assert!(review.ranked_sets.educational.len() <= 5);
    assert!(review.ranked_sets.funny_personality.len() <= 5);
}

#[test]
fn plan_short_form_review_tool_reads_sidecars_and_returns_review_packets() {
    let dir = tempfile::tempdir().unwrap_or_else(|err| {
        panic!("failed to create temp dir: {err}");
    });
    let asset = "raw/transcript-only.mp4";
    let input = transcript_only_input();
    write_sidecar(dir.path(), "whisper", asset, input.transcript);
    write_sidecar(dir.path(), "topic", asset, input.topics);
    write_sidecar(dir.path(), "face", asset, input.face);
    write_sidecar(dir.path(), "broll-candidates", asset, input.broll_assets);

    let output = plan_short_form_review::run(
        PlanShortFormReviewArgs {
            asset_id: asset.to_string(),
            source_width: Some(1920),
            source_height: Some(1080),
            max_candidates: Some(2),
            max_duration_s: None,
            profile: None,
            discovery_mode: None,
            trend_context: serde_json::json!({}),
        },
        ctx_at(dir.path()),
    )
    .unwrap_or_else(|err| {
        panic!("tool should return review packets: {err}");
    });

    let review: serde_json::Value = serde_json::from_str(&output).unwrap_or_else(|err| {
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
        review.pointer("/candidates/0/broll_plan/attached_assets/0/asset_id"),
        Some(&serde_json::json!("raw/broll/server-racks.mp4"))
    );
    assert_eq!(
        review.pointer("/proposal_policy/apply_tool"),
        Some(&serde_json::json!("apply_edl"))
    );
}

#[test]
fn plan_short_form_review_tool_accepts_harvest_discovery_mode() {
    let dir = tempfile::tempdir().unwrap_or_else(|err| {
        panic!("failed to create temp dir: {err}");
    });
    let asset = "raw/transcript-only.mp4";
    let input = transcript_only_input();
    write_sidecar(dir.path(), "whisper", asset, input.transcript);
    write_sidecar(dir.path(), "topic", asset, input.topics);
    write_sidecar(dir.path(), "face", asset, input.face);
    write_sidecar(dir.path(), "broll-candidates", asset, input.broll_assets);

    let output = plan_short_form_review::run(
        PlanShortFormReviewArgs {
            asset_id: asset.to_string(),
            source_width: None,
            source_height: None,
            max_candidates: Some(50),
            max_duration_s: None,
            profile: Some(ShortFormProfileArg::ViralSocial),
            discovery_mode: Some(ShortFormDiscoveryModeArg::Harvest),
            trend_context: serde_json::json!({}),
        },
        ctx_at(dir.path()),
    )
    .unwrap_or_else(|err| {
        panic!("tool should return harvest review packets: {err}");
    });

    let review: serde_json::Value = serde_json::from_str(&output).unwrap_or_else(|err| {
        panic!("tool output should be JSON: {err}");
    });
    assert_eq!(
        review.pointer("/discovery_mode"),
        Some(&serde_json::json!("harvest"))
    );
    assert!(
        review
            .get("candidates")
            .and_then(|value| value.as_array())
            .is_some_and(|candidates| !candidates.is_empty())
    );
}
