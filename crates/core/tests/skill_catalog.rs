//! Regression coverage for bundled workflow skills and their L3 scripts.

#![allow(
    clippy::expect_used,
    clippy::redundant_closure_for_method_calls,
    clippy::unwrap_used
)]

use std::error::Error;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use montage_core::context::ContextualUserFragment;
use montage_core::edl;
use montage_core::skills::SkillRegistry;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[test]
fn bundled_output_workflow_skills_load() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    for name in [
        "audio-separation",
        "auto-cutter",
        "beat-sync-editor",
        "b-roll-suggester",
        "color-corrector",
        "cut-director",
        "generated-explainer",
        "interview-tightener",
        "meeting-highlights",
        "multicam-director",
        "pacing-optimizer",
        "podcast-editor",
        "podcast-episode-producer",
        "rough-cut-assembler",
        "short-form",
        "stock-broll",
        "split-edit-director",
        "thematic-montage-director",
        "talking-head-vertical",
        "tutorial",
        "version-control",
        "viral-clip-extractor",
        "yt-broll",
    ] {
        let Some(skill) = registry.get(name) else {
            panic!("missing bundled skill {name}");
        };
        assert!(
            !skill.meta.description.trim().is_empty(),
            "skill {name} must have a description"
        );
        assert!(
            !skill.body.trim().is_empty(),
            "skill {name} must have a body"
        );
    }
}

#[test]
fn generated_explainer_preserves_scene_sources_and_routes_existing_backends() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    let skill = registry
        .get("generated-explainer")
        .expect("generated-explainer exists");
    for tool in [
        "plan_visual_support",
        "plan_motion_scene",
        "apply_edl",
        "view_timeline",
        "view_frame",
        "start_render",
        "poll_render",
        "bash",
    ] {
        assert!(
            skill
                .meta
                .tools_allowlist
                .iter()
                .any(|allowed| allowed == tool),
            "generated-explainer must allow {tool}"
        );
    }
    for required in [
        "generated/explainers/",
        "explainer_bundle.py",
        "MotionScene",
        "Manim",
        "scene source",
        "regenerate",
        "user confirms the scene plan",
    ] {
        assert!(
            skill.body.contains(required),
            "generated-explainer must mention {required:?}"
        );
    }
}

#[test]
fn audio_separation_skill_is_graph_native() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    let skill = registry
        .get("audio-separation")
        .expect("audio-separation exists");
    for tool in [
        "read_index",
        "view_timeline",
        "inspect_clip",
        "apply_edl",
        "vedit_diff",
        "start_render",
        "poll_render",
    ] {
        assert!(
            skill.meta.tools_allowlist.iter().any(|t| t == tool),
            "audio-separation must allow {tool}"
        );
    }
    // The picture-held audio grammar and its distinction from Set Volume must
    // be spelled out so the agent never deletes the clip or just lowers gain.
    for required in [
        "Mute Clip",
        "Remove Audio",
        "Set Volume",
        "keep its picture",
        "clip-local visible seconds",
        "clear: true",
    ] {
        assert!(
            skill.body.contains(required),
            "audio-separation must mention {required:?}"
        );
    }
}

#[test]
fn visual_support_editorial_skills_are_loadable_and_proposal_driven() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    for name in [
        "retention-list-opener",
        "quote-highlight",
        "search-bar-sequence",
        "source-backed-broll",
        "route-map",
        "statistic-counter",
        "podcast-hook",
        "chapter-intro",
        "short-form-reframing",
    ] {
        let skill = registry
            .get(name)
            .unwrap_or_else(|| panic!("missing bundled skill {name}"));
        for tool in [
            "plan_visual_support_proposals",
            "revise_visual_support_proposal",
            "apply_edl",
            "start_render",
            "verify_render",
        ] {
            assert!(
                skill.meta.tools_allowlist.iter().any(|t| t == tool),
                "{name} must allow {tool}"
            );
        }
        for required in [
            "Proposal-to-Visual-Support",
            "evidence",
            "apply_edl",
            "revise_visual_support_proposal",
            "verify_render",
        ] {
            assert!(
                skill.body.contains(required),
                "{name} must mention {required:?}"
            );
        }
    }
}

#[test]
fn visual_support_editorial_skills_ship_inspectable_examples() {
    let root = workspace_root().join("skills");
    for (name, artifact_type) in [
        ("retention-list-opener", "animated_list"),
        ("quote-highlight", "quote_highlight"),
        ("search-bar-sequence", "search_bar"),
        ("source-backed-broll", "broll_package"),
        ("route-map", "map_visualization"),
        ("statistic-counter", "counter_stat_graphic"),
        ("podcast-hook", "quote_highlight"),
        ("chapter-intro", "title_card"),
        ("short-form-reframing", "title_card"),
    ] {
        let path = root
            .join(name)
            .join("examples")
            .join("visual-support-proposal.json");
        let bytes = std::fs::read(&path).unwrap_or_else(|err| {
            panic!("missing example for {name} at {}: {err}", path.display())
        });
        let example: serde_json::Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|err| panic!("invalid example JSON for {name}: {err}"));
        assert_eq!(
            example["workflow"], "proposal_to_visual_support",
            "{name} example must document the proposal workflow"
        );
        assert_eq!(
            example["artifact_type"], artifact_type,
            "{name} example must use the registered artifact type"
        );
        assert!(
            example["evidence"]
                .as_array()
                .is_some_and(|items| !items.is_empty()),
            "{name} example must include evidence"
        );
        assert!(
            example["verification"]
                .as_array()
                .is_some_and(|items| !items.is_empty()),
            "{name} example must include verification checks"
        );
    }
}

#[test]
fn podcast_episode_producer_routes_visual_polish_through_proposals() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    let skill = registry
        .get("podcast-episode-producer")
        .expect("podcast-episode-producer exists");
    for tool in [
        "plan_visual_support_proposals",
        "revise_visual_support_proposal",
        "apply_edl",
        "start_render",
        "verify_render",
    ] {
        assert!(
            skill.meta.tools_allowlist.iter().any(|t| t == tool),
            "podcast-episode-producer must allow {tool}"
        );
    }
    for required in [
        "Proposal-to-Visual-Support",
        "evidence",
        "revise_visual_support_proposal",
        "apply_edl",
    ] {
        assert!(
            skill.body.contains(required),
            "podcast-episode-producer must mention {required:?}"
        );
    }
}

#[test]
fn cut_and_split_directors_expose_first_class_edit_grammar() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    let cut = registry.get("cut-director").expect("cut-director exists");
    for tool in [
        "view_timeline",
        "inspect_clip",
        "view_frame",
        "assess_edit_quality",
        "apply_edl",
        "vedit_diff",
    ] {
        assert!(
            cut.meta.tools_allowlist.iter().any(|t| t == tool),
            "cut-director must allow {tool}"
        );
    }
    for required in [
        "hard cut is the default",
        "cut_on_action",
        "eyeline_match",
        "match_cut",
        "smash_cut",
        "jump_cut",
        "Set Cut Intent",
        "assess_edit_quality",
        "visible transition",
        "transition-director",
    ] {
        assert!(
            cut.body.contains(required),
            "cut-director must mention {required:?}"
        );
    }

    let split = registry
        .get("split-edit-director")
        .expect("split-edit-director exists");
    for tool in [
        "view_timeline",
        "inspect_clip",
        "assess_edit_quality",
        "find_dead_air",
        "find_false_starts",
        "apply_edl",
        "transition_context",
        "plan_split_edit",
        "vedit_diff",
        "start_render",
        "poll_render",
    ] {
        assert!(
            split.meta.tools_allowlist.iter().any(|t| t == tool),
            "split-edit-director must allow {tool}"
        );
    }
    for required in [
        "J-cut",
        "L-cut",
        "Set Audio Lead",
        "Set Audio Trail",
        "lead_s",
        "trail_s",
        "plan_split_edit",
        "learned",
        "preview-limited",
        "review by ear",
        "visible transition",
    ] {
        assert!(
            split.body.contains(required),
            "split-edit-director must mention {required:?}"
        );
    }
}

#[test]
fn multicam_director_skill_is_graph_native() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    let skill = registry
        .get("multicam-director")
        .expect("multicam-director exists");
    for tool in [
        "read_index",
        "view_timeline",
        "analyze_sync",
        "plan_multicam",
        "view_frame",
        "apply_edl",
        "vedit_diff",
        "start_render",
        "poll_render",
    ] {
        assert!(
            skill.meta.tools_allowlist.iter().any(|t| t == tool),
            "multicam-director must allow {tool}"
        );
    }
    // The two-stage contract (sync then direct) and its graph-native ops must
    // be spelled out so the agent never plans before syncing or bypasses the
    // edit graph.
    for required in [
        "analyze_sync",
        "Set Sync Group",
        "plan_multicam",
        "Apply Multicam Plan",
        "sync_group_id",
        "min_hold_s",
        "manual_offset_required",
        "offset_corrected",
        "Sync first",
    ] {
        assert!(
            skill.body.contains(required),
            "multicam-director must mention {required:?}"
        );
    }
}

#[test]
fn color_corrector_skill_is_graph_native() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");
    let skill = registry
        .get("color-corrector")
        .expect("color-corrector skill exists");

    for tool in [
        "read_index",
        "view_frame",
        "view_timeline",
        "start_look_region_pass",
        "plan_look_regions",
        "apply_edl",
        "vedit_diff",
        "start_render",
        "review_look_regions",
    ] {
        assert!(
            skill.meta.tools_allowlist.iter().any(|t| t == tool),
            "color-corrector must allow {tool}"
        );
    }

    for required in [
        "read_index(channel=\"color\"",
        "Set Color Correction",
        "Apply LUT",
        "summary.policy",
        "recommended_action",
        "auto_correct_safe",
        "color_apply_plan.py",
        "camera_match_plan.py",
        "look_region_plan.py",
        "look_region_review_package.py",
        "color_review_package.py",
        "rendered contact sheet",
        "vedit_diff",
        "edit graph is the source of truth",
    ] {
        assert!(
            skill.body.contains(required),
            "color-corrector must mention {required:?}"
        );
    }

    // Bundled shaper LUTs for camera Log encodings — agents
    // reference these on `montage.color_pipeline.shaper_lut` when
    // the clip's input space is log. If the files vanish, the
    // skill instructions become a lie.
    let shapers_dir = workspace_root().join("skills/color-corrector/shapers");
    for space in [
        "arri_logc3",
        "arri_logc4",
        "slog3_sgamut3",
        "vlog_vgamut",
        "bmd_film_gen5",
    ] {
        let path = shapers_dir.join(format!("{space}_to_rec709_g24.csp"));
        assert!(
            path.is_file(),
            "expected bundled shaper at {}",
            path.display()
        );
        let header = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .take(2)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            header,
            "CSPLUTV100\n1D",
            "shaper {} must start with CineSpace 1D header, got: {header:?}",
            path.display()
        );
    }
}

#[test]
fn auto_cutter_and_podcast_use_semantic_retake_planning() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    for name in ["auto-cutter", "podcast-episode-producer"] {
        let skill = registry.get(name).expect("skill exists");
        assert!(
            skill
                .meta
                .tools_allowlist
                .iter()
                .any(|tool| tool == "assess_continuity"),
            "{name} must allow assess_continuity for risky retake cuts"
        );
        assert!(
            skill
                .meta
                .tools_allowlist
                .iter()
                .any(|tool| tool == "assess_edit_quality"),
            "{name} must allow assess_edit_quality for editorial cut repair"
        );
        for required in [
            "episode_span_plan.py",
            "retake_plan.py",
            "assess_edit_quality",
            "Set Cut Intent",
            "Set Audio Lead",
            "Set Audio Trail",
        ] {
            assert!(
                skill.body.contains(required),
                "{name} must mention {required:?}"
            );
        }
    }
}

#[test]
fn transition_and_broll_skills_use_edit_quality_layer() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    for name in ["transition-director", "stock-broll"] {
        let skill = registry.get(name).expect("skill exists");
        assert!(
            skill
                .meta
                .tools_allowlist
                .iter()
                .any(|tool| tool == "assess_edit_quality"),
            "{name} must allow assess_edit_quality"
        );
    }

    let transition = registry
        .get("transition-director")
        .expect("transition-director skill exists");
    for tool in [
        "transition_context",
        "plan_transition",
        "validate_transition_choice",
    ] {
        assert!(
            transition.meta.tools_allowlist.iter().any(|t| t == tool),
            "transition-director must allow {tool}"
        );
    }
    for required in [
        "assess_edit_quality",
        "transition_context",
        "plan_transition",
        "validate_transition_choice",
        "style_context.transition_density_last_30s",
        "Set Audio Lead",
        "Set Audio Trail",
        "Set Cut Intent",
        "Respect",
    ] {
        assert!(
            transition.body.contains(required),
            "transition-director must mention {required:?}"
        );
    }

    let stock = registry
        .get("stock-broll")
        .expect("stock-broll skill exists");
    for required in [
        "assess_edit_quality",
        "recommendation.broll=true",
        "style_context.transition_density_last_30s",
        "Set Audio Lead",
        "Set Audio Trail",
    ] {
        assert!(
            stock.body.contains(required),
            "stock-broll must mention {required:?}"
        );
    }
}

#[test]
fn thematic_montage_is_separate_from_literal_broll() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    let montage = registry
        .get("thematic-montage-director")
        .expect("thematic-montage-director skill exists");
    for tool in [
        "read_index",
        "find_beat",
        "find_moment",
        "clip_search",
        "view_timeline",
        "apply_edl",
        "vedit_diff",
    ] {
        assert!(
            montage.meta.tools_allowlist.iter().any(|t| t == tool),
            "thematic montage skill must allow {tool}"
        );
    }
    for required in [
        "not a continuity cover",
        "opt-in",
        "associative",
        "literal b-roll",
        "Set Cut Intent",
        "Insert BRoll",
        "view_timeline",
        "vedit_diff",
    ] {
        assert!(
            montage.body.contains(required),
            "thematic montage skill must mention {required:?}"
        );
    }

    let broll = registry
        .get("b-roll-suggester")
        .expect("b-roll-suggester skill exists");
    for required in [
        "literal continuity cover",
        "thematic-montage-director",
        "not a symbolic montage",
    ] {
        assert!(
            broll.body.contains(required),
            "b-roll-suggester must keep literal cover separate via {required:?}"
        );
    }
}

#[test]
fn l1_catalog_exposes_metadata_without_l2_or_l3_content() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    let catalog = registry
        .l1_fragment()
        .expect("bundled skills should produce an L1 fragment")
        .render();

    assert!(catalog.contains("viral-clip-extractor"));
    assert!(catalog.contains("Find and build 30-59 second social clips"));
    assert!(
        !catalog.contains("This is an montage advantage"),
        "L1 catalog must not include SKILL.md body text"
    );
    assert!(
        !catalog.contains("scripts/score_moments.py"),
        "L1 catalog must not include L3 script paths"
    );
    assert!(
        !catalog.contains(root.to_string_lossy().as_ref()),
        "L1 catalog must not include absolute skill roots"
    );
}

#[test]
fn output_workflow_skills_keep_the_edit_graph_in_the_loop() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    for name in [
        "auto-cutter",
        "beat-sync-editor",
        "meeting-highlights",
        "pacing-optimizer",
        "podcast-editor",
        "podcast-episode-producer",
        "rough-cut-assembler",
        "short-form",
        "talking-head-vertical",
        "viral-clip-extractor",
    ] {
        let skill = registry.get(name).expect("workflow skill exists");
        assert!(
            skill
                .meta
                .tools_allowlist
                .iter()
                .any(|tool| tool == "apply_edl"),
            "{name} must expose apply_edl so analysis becomes graph edits"
        );
        assert!(
            skill
                .meta
                .tools_allowlist
                .iter()
                .any(|tool| tool == "view_timeline"),
            "{name} must expose view_timeline so the graph is inspected after edits"
        );
        assert!(
            skill
                .meta
                .tools_allowlist
                .iter()
                .any(|tool| tool == "vedit_diff"),
            "{name} must expose vedit_diff so workflow outputs have an audit checkpoint"
        );
        assert!(
            skill.body.contains("apply_edl"),
            "{name} body must tell the agent to mutate the edit graph"
        );
        assert!(
            skill.body.contains("view_timeline"),
            "{name} body must tell the agent to inspect the graph"
        );
        assert!(
            skill.body.contains("vedit_diff"),
            "{name} body must require an audit diff checkpoint"
        );
    }
}

#[test]
fn podcast_episode_skill_covers_full_editorial_process() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");
    let skill = registry
        .get("podcast-episode-producer")
        .expect("podcast skill exists");

    for tool in [
        "find_episode_start",
        "find_dead_air",
        "find_filler_words",
        "find_false_starts",
        "find_broll_opportunities",
        "apply_edl",
        "vedit_diff",
    ] {
        assert!(
            skill.meta.tools_allowlist.iter().any(|t| t == tool),
            "podcast skill must allow {tool}"
        );
    }

    for required in [
        "real ending",
        "cold open",
        "Radio edit",
        "False starts",
        "J/L-cut",
        "lower thirds",
        "Set Loudness Target",
        "Set Package Metadata",
        "derivatives",
        "Archive",
    ] {
        assert!(
            skill.body.contains(required),
            "podcast skill must cover process anchor {required:?}"
        );
    }
}

#[test]
fn talking_head_vertical_skill_covers_native_pipeline_contract() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    let skill = registry
        .get("talking-head-vertical")
        .expect("talking-head-vertical exists");
    for tool in [
        "read_index",
        "find_beat",
        "find_speaker_oncam",
        "plan_reframe",
        "plan_emphasis",
        "apply_edl",
        "view_timeline",
        "vedit_diff",
        "verify_render",
        "bash",
    ] {
        assert!(
            skill.meta.tools_allowlist.iter().any(|t| t == tool),
            "talking-head-vertical must allow {tool}"
        );
    }
    for required in [
        "talking_head_plan.py",
        "face position",
        "eye line",
        "headroom",
        "negative space",
        "keep native vertical",
        "Reframe horizontal footage to 9:16",
        "hook must appear",
        "0.8s",
        "phrase-level",
        "face-overlap risk",
        "single small punch-in",
        "apply_edl",
        "view_timeline",
        "vedit_diff",
        "verify_render",
    ] {
        assert!(
            skill.body.contains(required),
            "talking-head-vertical must mention {required:?}"
        );
    }
}

#[test]
fn skill_tool_allowlists_match_graph_native_instructions() {
    let root = workspace_root().join("skills");
    let (registry, errors) = SkillRegistry::discover(Some(&root), None);
    assert!(errors.is_empty(), "skill load errors: {errors:?}");

    for skill in registry.all() {
        let body_instructs_apply = skill.body.contains("Use `apply_edl`")
            || skill.body.contains("call `apply_edl`")
            || skill.body.contains("through `apply_edl`")
            || skill.body.contains("via `apply_edl`")
            || skill
                .body
                .contains("Hand the `edl_fragment` to `apply_edl`")
            || skill.body.contains("`apply_edl Insert Clip`");
        if body_instructs_apply {
            assert!(
                skill
                    .meta
                    .tools_allowlist
                    .iter()
                    .any(|tool| tool == "apply_edl"),
                "{} body references apply_edl but frontmatter does not allow it",
                skill.meta.name
            );
        }
        if skill.body.contains("view_timeline") {
            assert!(
                skill
                    .meta
                    .tools_allowlist
                    .iter()
                    .any(|tool| tool == "view_timeline"),
                "{} body references view_timeline but frontmatter does not allow it",
                skill.meta.name
            );
        }
        if skill.body.contains("vedit_diff") {
            assert!(
                skill
                    .meta
                    .tools_allowlist
                    .iter()
                    .any(|tool| tool == "vedit_diff"),
                "{} body references vedit_diff but frontmatter does not allow it",
                skill.meta.name
            );
        }
        if skill.body.contains("assess_edit_quality") {
            assert!(
                skill
                    .meta
                    .tools_allowlist
                    .iter()
                    .any(|tool| tool == "assess_edit_quality"),
                "{} body references assess_edit_quality but frontmatter does not allow it",
                skill.meta.name
            );
        }
    }
}

#[test]
fn workflow_helper_scripts_emit_json() -> Result<(), Box<dyn Error>> {
    let Some(python) = python3()? else {
        return Ok(());
    };
    let root = workspace_root();
    let fixtures = tempfile::tempdir()?;
    let audio = write_json(
        fixtures.path().join("audio.json"),
        serde_json::json!({
            "data": {
                "duration_s": 35.0,
                "loudness_integrated_lufs": -20.0,
                "windows": [
                    {"start_s": 0.0, "rms_db": -22.0},
                    {"start_s": 5.0, "rms_db": -18.0},
                    {"start_s": 12.0, "rms_db": -45.0},
                    {"start_s": 22.0, "rms_db": -20.0}
                ],
                "silences": [{"start_s": 10.0, "end_s": 12.0}]
            }
        }),
    )?;
    let transcript = write_json(
        fixtures.path().join("transcript.json"),
        serde_json::json!({
            "data": {
                "segments": [
                    {"start_s": 0.0, "end_s": 8.0, "text": "Actually this mistake changed everything.", "speaker_id": "A"},
                    {"start_s": 8.0, "end_s": 14.0, "text": "um we decided Sarah will follow up by Friday.", "speaker_id": "A"},
                    {"start_s": 20.0, "end_s": 32.0, "text": "The biggest problem was the launch risk.", "speaker_id": "B"}
                ],
                "words": [
                    {"word": "Actually", "start_s": 0.0, "end_s": 0.4},
                    {"word": "this", "start_s": 0.4, "end_s": 0.7},
                    {"word": "mistake", "start_s": 0.7, "end_s": 1.2},
                    {"word": "changed", "start_s": 1.2, "end_s": 1.7},
                    {"word": "everything", "start_s": 1.7, "end_s": 2.2}
                ]
            }
        }),
    )?;
    let retake_transcript = write_json(
        fixtures.path().join("retake-transcript.json"),
        serde_json::json!({
            "data": {
                "segments": [
                    {"start_s": 0.0, "end_s": 4.0, "text": "The real problem is our launch was wait let me say that again", "speaker_id": "A"},
                    {"start_s": 4.2, "end_s": 10.0, "text": "The real problem is that our launch process had too many manual steps.", "speaker_id": "A"},
                    {"start_s": 12.0, "end_s": 15.0, "text": "Is the camera still recording we can cut that part.", "speaker_id": "B"},
                    {"start_s": 15.5, "end_s": 21.0, "text": "Back to what I was saying, the customer story matters here.", "speaker_id": "A"}
                ]
            }
        }),
    )?;
    let multi_episode_transcript = write_json(
        fixtures.path().join("multi-episode-transcript.json"),
        serde_json::json!({
            "data": {
                "segments": [
                    {"start_s": 0.0, "end_s": 5.0, "text": "Welcome to the show today we have Sarah.", "speaker_id": "A"},
                    {"start_s": 5.0, "end_s": 300.0, "text": "We talk about launch risk and product lessons.", "speaker_id": "A"},
                    {"start_s": 305.0, "end_s": 310.0, "text": "Thanks for listening, see you next time.", "speaker_id": "A"},
                    {"start_s": 380.0, "end_s": 385.0, "text": "Welcome back, this is episode two with Daniel.", "speaker_id": "A"},
                    {"start_s": 385.0, "end_s": 690.0, "text": "Now we're going to discuss fundraising.", "speaker_id": "B"},
                    {"start_s": 695.0, "end_s": 700.0, "text": "Thanks for listening and subscribe.", "speaker_id": "A"}
                ]
            }
        }),
    )?;
    let moments = write_json(
        fixtures.path().join("moments.json"),
        serde_json::json!({
            "data": {
                "moments": [
                    {"moment_id": "m1", "kind": "hook", "start_s": 0.0, "end_s": 30.0, "score": 0.9, "dependencies": [], "note": "Actually this mistake changed everything"}
                ]
            }
        }),
    )?;
    let shot = write_json(
        fixtures.path().join("shot.json"),
        serde_json::json!({
            "data": {
                "shots": [
                    {"start_s": 0.0, "end_s": 30.0, "type": "close-up", "motion": "handheld"}
                ]
            }
        }),
    )?;
    let gaze = write_json(
        fixtures.path().join("gaze.json"),
        serde_json::json!({
            "data": {
                "per_frame": [
                    {"t_s": 1.0, "faces": [{"at_camera": true}]},
                    {"t_s": 2.0, "faces": [{"at_camera": true}]}
                ]
            }
        }),
    )?;
    let face = write_json(
        fixtures.path().join("face.json"),
        serde_json::json!({
            "data": {
                "per_frame": [
                    {"t_s": 1.0, "faces": [{"confidence": 0.95, "bbox": {"x": 0.56, "y": 0.12, "w": 0.22, "h": 0.32}}]},
                    {"t_s": 2.0, "faces": [{"confidence": 0.95, "bbox": {"x": 0.57, "y": 0.12, "w": 0.22, "h": 0.32}}]}
                ]
            }
        }),
    )?;
    let composition = write_json(
        fixtures.path().join("composition.json"),
        serde_json::json!({
            "data": {
                "regions": [{
                    "start_s": 0.0,
                    "end_s": 30.0,
                    "framing": "good",
                    "negative_space": {"side": "left", "score": 0.72}
                }]
            }
        }),
    )?;
    let quality = write_json(
        fixtures.path().join("quality.json"),
        serde_json::json!({
            "data": {
                "per_frame": [
                    {"t_s": 1.0, "is_sharp": true, "blur": 200.0, "brightness": 130.0, "contrast": 60.0},
                    {"t_s": 2.0, "is_sharp": true, "blur": 180.0, "brightness": 120.0, "contrast": 55.0}
                ]
            }
        }),
    )?;
    let topic = write_json(
        fixtures.path().join("topic.json"),
        serde_json::json!({
            "data": {
                "topics": [{"start_s": 0.0, "end_s": 35.0, "label": "Launch Risk"}]
            }
        }),
    )?;
    let multi_topic = write_json(
        fixtures.path().join("multi-topic.json"),
        serde_json::json!({
            "data": {
                "topics": [
                    {"start_s": 0.0, "end_s": 315.0, "label": "Episode One Launch Risk"},
                    {"start_s": 380.0, "end_s": 705.0, "label": "next episode fundraising"}
                ]
            }
        }),
    )?;
    let beats = write_json(
        fixtures.path().join("beats.json"),
        serde_json::json!({"data": {"beats": [0.0, 0.5, 1.0, 1.5, 2.0, 2.5]}}),
    )?;
    let color = write_json(
        fixtures.path().join("color.json"),
        serde_json::json!({
            "data": {
                "frame_count": 2,
                "summary": {
                    "issue_tags": ["underexposed", "cool_cast"],
                    "auto_correct_safe": true,
                    "confidence": 0.86,
                    "policy": {
                        "recommended_action": "auto_correct",
                        "reason": ["underexposed", "cool_cast"],
                        "confidence": 0.86,
                        "edit_types": ["lighting_correction", "white_balance"],
                        "requires_review": false,
                        "requires_contact_sheet": true,
                        "apply_mode": "automatic",
                        "graph_ops": ["Set Color Correction"]
                    },
                    "recommended_correction": {
                        "exposure_ev": 0.4,
                        "contrast": 1.1,
                        "saturation": 1.0,
                        "temperature": 0.2,
                        "tint": 0.0,
                        "shadows": 0.1,
                        "highlights": 0.0
                    }
                },
                "scenes": [{
                    "scene_id": "color_scene_1",
                    "start_s": 0.0,
                    "end_s": 2.0,
                    "issue_tags": ["underexposed"],
                    "auto_correct_safe": true,
                    "confidence": 0.9,
                    "policy": {
                        "recommended_action": "auto_correct",
                        "reason": ["underexposed"],
                        "confidence": 0.9,
                        "edit_types": ["lighting_correction"],
                        "requires_review": false,
                        "requires_contact_sheet": true,
                        "apply_mode": "automatic",
                        "graph_ops": ["Set Color Correction"]
                    },
                    "recommended_correction": {"exposure_ev": 0.4}
                }]
            }
        }),
    )?;
    let color_balanced = write_json(
        fixtures.path().join("color-balanced.json"),
        serde_json::json!({
            "data": {
                "summary": {
                    "brightness_mean": 126.0,
                    "luma_p05_mean": 45.0,
                    "luma_p50_mean": 126.0,
                    "luma_p95_mean": 210.0,
                    "contrast_mean": 60.0,
                    "mean_r": 126.0,
                    "mean_g": 126.0,
                    "mean_b": 126.0,
                    "overexposed_fraction_mean": 0.0,
                    "underexposed_fraction_mean": 0.0,
                    "issue_tags": ["already_balanced"],
                    "auto_correct_safe": false,
                    "confidence": 0.75,
                    "policy": {
                        "recommended_action": "no_op",
                        "reason": ["already_balanced"],
                        "confidence": 0.75,
                        "edit_types": [],
                        "requires_review": false,
                        "requires_contact_sheet": false,
                        "apply_mode": "none",
                        "graph_ops": []
                    },
                    "recommended_correction": {"exposure_ev": 0.0, "contrast": 1.0}
                },
                "scenes": []
            }
        }),
    )?;
    let color_review = write_json(
        fixtures.path().join("color-review-only.json"),
        serde_json::json!({
            "data": {
                "summary": {
                    "issue_tags": ["underexposed", "crushed_shadows", "unsafe_to_auto_correct"],
                    "auto_correct_safe": false,
                    "confidence": 0.35,
                    "policy": {
                        "recommended_action": "review_only",
                        "reason": ["underexposed", "crushed_shadows", "unsafe_to_auto_correct"],
                        "confidence": 0.35,
                        "edit_types": ["lighting_correction"],
                        "requires_review": true,
                        "requires_contact_sheet": true,
                        "apply_mode": "manual_review",
                        "graph_ops": ["Set Color Correction"]
                    },
                    "recommended_correction": {"exposure_ev": 0.8, "shadows": 0.6}
                },
                "scenes": []
            }
        }),
    )?;
    let missing_render = fixtures.path().join("missing.mp4");
    let missing_sheet = fixtures.path().join("sheet.ppm");
    let review_report = fixtures.path().join("color-review.md");
    let review_json = fixtures.path().join("color-review.json");
    let look_plan = write_json(
        fixtures.path().join("look-plan.json"),
        serde_json::json!({
            "version": 1,
            "status": "planned",
            "regions": [{
                "clip_name": "clip-a",
                "clip_anchor": "clip-a",
                "asset_id": "raw/source.mp4",
                "scene_id": "color_scene_1",
                "source_start_s": 0.0,
                "source_end_s": 2.0,
                "timeline_start_s": 10.0,
                "timeline_end_s": 12.0,
                "sample_times_s": [10.1, 11.0, 11.9],
                "source_sample_times_s": [0.1, 1.0, 1.9],
                "issue_tags": ["underexposed"],
                "policy": {"recommended_action": "auto_correct", "confidence": 0.9},
                "correction": {"exposure_ev": 0.4},
                "consistency_group": "raw-source.mp4:shadow_lift:underexposed",
                "look_id": "shadow_lift",
                "lut_path": "luts/generated/cinematic-shadow_lift.cube",
                "score": 0.76,
                "rationale": "fixture"
            }],
            "generated_luts": ["luts/generated/cinematic-shadow_lift.cube"]
        }),
    )?;
    let look_review_sheet = fixtures.path().join("look-review.ppm");
    let look_review_report = fixtures.path().join("look-review.md");
    let look_review_json = fixtures.path().join("look-review.json");

    run_script(
        &python,
        root.join("skills/auto-cutter/scripts/cleanup_plan.py"),
        &[
            "--audio-energy",
            path(&audio),
            "--transcript",
            path(&transcript),
        ],
        "cuts",
    )?;
    let retake_plan = run_script(
        &python,
        root.join("skills/auto-cutter/scripts/retake_plan.py"),
        &[
            "--transcript",
            path(&retake_transcript),
            "--audio-energy",
            path(&audio),
            "--topic",
            path(&topic),
            "--moments",
            path(&moments),
            "--clip-uuid",
            "clip-a",
        ],
        "candidates",
    )?;
    let edl_fragments = retake_plan
        .get("edl_fragments")
        .and_then(|value| value.as_array())
        .expect("retake planner emits edl_fragments array");
    assert!(
        !edl_fragments.is_empty(),
        "obvious leading retake should produce an EDL fragment"
    );
    for fragment in edl_fragments {
        let text = fragment.as_str().expect("EDL fragment is text");
        edl::parse(text).expect("retake planner emitted parseable EDL");
    }
    let retake_candidates = retake_plan
        .get("candidates")
        .and_then(|value| value.as_array())
        .expect("retake planner emits candidates array");
    assert!(
        retake_candidates.iter().any(|candidate| candidate
            .get("requires_review")
            .and_then(|value| value.as_bool())
            == Some(true)),
        "internal or continuity-risk retakes should stay review gated"
    );
    let episode_spans = run_script(
        &python,
        root.join("skills/auto-cutter/scripts/episode_span_plan.py"),
        &[
            "--transcript",
            path(&multi_episode_transcript),
            "--audio-energy",
            path(&audio),
            "--topic",
            path(&multi_topic),
        ],
        "episode_spans",
    )?;
    assert_eq!(
        episode_spans
            .get("requires_user_choice")
            .and_then(|value| value.as_bool()),
        Some(true),
        "multiple publishable episode spans must require user choice"
    );
    run_script(
        &python,
        root.join("skills/podcast-editor/scripts/audio_polish_plan.py"),
        &[
            "--audio-energy",
            path(&audio),
            "--transcript",
            path(&transcript),
        ],
        "speed_changes",
    )?;
    run_script(
        &python,
        root.join("skills/podcast-editor/scripts/audio_mix_plan.py"),
        &[
            "--audio-energy",
            path(&audio),
            "--transcript",
            path(&transcript),
            "--target-lufs",
            "-16",
        ],
        "graph_ops",
    )?;
    run_script(
        &python,
        root.join("skills/viral-clip-extractor/scripts/score_moments.py"),
        &[
            "--moments",
            path(&moments),
            "--audio-energy",
            path(&audio),
            "--transcript",
            path(&transcript),
            "--shot",
            path(&shot),
            "--gaze",
            path(&gaze),
            "--frame-quality",
            path(&quality),
            "--topic",
            path(&topic),
        ],
        "candidates",
    )?;
    run_script(
        &python,
        root.join("skills/pacing-optimizer/scripts/pacing_plan.py"),
        &[
            "--audio-energy",
            path(&audio),
            "--transcript",
            path(&transcript),
            "--topic",
            path(&topic),
            "--shot",
            path(&shot),
        ],
        "cuts",
    )?;
    run_script(
        &python,
        root.join("skills/rough-cut-assembler/scripts/score_takes.py"),
        &[
            "--audio-energy",
            path(&audio),
            "--transcript",
            path(&transcript),
            "--frame-quality",
            path(&quality),
            "--gaze",
            path(&gaze),
        ],
        "takes",
    )?;
    run_script(
        &python,
        root.join("skills/meeting-highlights/scripts/classify_meeting.py"),
        &["--transcript", path(&transcript)],
        "segments",
    )?;
    run_script(
        &python,
        root.join("skills/beat-sync-editor/scripts/beat_cut_plan.py"),
        &["--beats", path(&beats), "--shot", path(&shot)],
        "cuts",
    )?;
    run_script(
        &python,
        root.join("skills/short-form/scripts/caption_plan.py"),
        &["--transcript", path(&transcript)],
        "phrases",
    )?;
    let talking_head_plan = run_script(
        &python,
        root.join("skills/talking-head-vertical/scripts/talking_head_plan.py"),
        &[
            "--asset-id",
            "raw/sample.mov",
            "--clip-id",
            "clip-1",
            "--source-width",
            "1920",
            "--source-height",
            "1080",
            "--transcript",
            path(&transcript),
            "--audio-energy",
            path(&audio),
            "--moments",
            path(&moments),
            "--face",
            path(&face),
            "--shot",
            path(&shot),
            "--gaze",
            path(&gaze),
            "--composition",
            path(&composition),
            "--frame-quality",
            path(&quality),
            "--topic",
            path(&topic),
        ],
        "readiness",
    )?;
    let talking_head_edl = talking_head_plan
        .get("edl")
        .and_then(|value| value.as_str())
        .expect("talking-head planner emits EDL text");
    edl::parse(talking_head_edl).expect("talking-head planner emitted parseable EDL");
    run_script(
        &python,
        root.join("skills/podcast-episode-producer/scripts/metadata_plan.py"),
        &[
            "--transcript",
            path(&transcript),
            "--topic",
            path(&topic),
            "--moments",
            path(&moments),
            "--frame-quality",
            path(&quality),
        ],
        "title",
    )?;
    run_script(
        &python,
        root.join("skills/short-form/scripts/render_verify.py"),
        &["--file", path(&missing_render)],
        "status",
    )?;
    run_script(
        &python,
        root.join("skills/color-corrector/scripts/color_benchmark.py"),
        &[
            "--color-index",
            path(&color),
            "--dataset-dir",
            path(fixtures.path()),
        ],
        "aggregate",
    )?;
    run_script(
        &python,
        root.join("skills/color-corrector/scripts/color_apply_plan.py"),
        &["--color-index", path(&color), "--clip-uuid", "clip-a"],
        "edl_text",
    )?;
    run_script(
        &python,
        root.join("skills/color-corrector/scripts/color_apply_plan.py"),
        &[
            "--color-index",
            path(&color_balanced),
            "--clip-uuid",
            "clip-b",
        ],
        "ops",
    )?;
    run_script(
        &python,
        root.join("skills/color-corrector/scripts/color_apply_plan.py"),
        &[
            "--color-index",
            path(&color_review),
            "--clip-uuid",
            "clip-c",
        ],
        "review",
    )?;
    run_script(
        &python,
        root.join("skills/color-corrector/scripts/camera_match_plan.py"),
        &[
            "--color-index",
            path(&color),
            "--color-index",
            path(&color_balanced),
            "--camera-label",
            "A",
            "--camera-label",
            "B",
        ],
        "matches",
    )?;
    run_script(
        &python,
        root.join("skills/color-corrector/scripts/rendered_contact_sheet.py"),
        &[
            "--before-render",
            path(&missing_render),
            "--after-render",
            path(&missing_render),
            "--output",
            path(&missing_sheet),
        ],
        "status",
    )?;
    run_script(
        &python,
        root.join("skills/color-corrector/scripts/color_review_package.py"),
        &[
            "--color-index",
            path(&color),
            "--before-render",
            path(&missing_render),
            "--after-render",
            path(&missing_render),
            "--contact-sheet",
            path(&missing_sheet),
            "--report-md",
            path(&review_report),
            "--benchmark-json",
            path(&review_json),
        ],
        "status",
    )?;
    run_script(
        &python,
        root.join("skills/color-corrector/scripts/look_region_review_package.py"),
        &[
            "--look-plan",
            path(&look_plan),
            "--after-render",
            path(&missing_render),
            "--contact-sheet",
            path(&look_review_sheet),
            "--report-md",
            path(&look_review_report),
            "--package-json",
            path(&look_review_json),
        ],
        "status",
    )?;

    Ok(())
}

fn write_json(path: PathBuf, value: serde_json::Value) -> Result<PathBuf, Box<dyn Error>> {
    std::fs::write(&path, serde_json::to_vec_pretty(&value)?)?;
    Ok(path)
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap_or("")
}

fn python3() -> Result<Option<PathBuf>, Box<dyn Error>> {
    match Command::new("python3").arg("--version").output() {
        Ok(out) if out.status.success() => Ok(Some(PathBuf::from("python3"))),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Box::new(e)),
    }
}

fn run_script(
    python: &Path,
    script: PathBuf,
    args: &[&str],
    expected_key: &str,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let output = Command::new(python).arg(&script).args(args).output()?;
    assert!(
        output.status.success(),
        "script {} failed: {}",
        script.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(
        json.get(expected_key).is_some(),
        "script {} did not emit key {expected_key}; got {json}",
        script.display()
    );
    Ok(json)
}
