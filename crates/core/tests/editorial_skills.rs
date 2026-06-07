//! Regression coverage for the hybrid editorial-skills layer.

#![allow(clippy::expect_used)]

use montage_core::editorial_skills::{EditorialSkillRegistry, MatchEditorialSkillsRequest};

#[test]
fn registry_separates_skill_definitions_from_ranked_instances() {
    let registry = EditorialSkillRegistry::bundled();

    let quote = registry
        .definition("quote-highlight")
        .expect("quote-highlight definition exists");
    assert_eq!(quote.id, "quote-highlight");
    assert_eq!(quote.artifact_type, "quote_highlight");
    assert_eq!(quote.skill_path, "skills/quote-highlight/SKILL.md");
    assert!(quote.inspectable);
    assert!(quote.composable);

    let instances = registry.match_instances(MatchEditorialSkillsRequest {
        selection_text: "This quote changed how people think about AI.".into(),
        request: "make this quote land visually".into(),
        anchor_transcript: Some("quote changed how people think".into()),
    });

    let top = instances.first().expect("at least one instance");
    assert_eq!(top.skill_id, "quote-highlight");
    assert_eq!(top.artifact_type, "quote_highlight");
    assert!(top.confidence >= 0.85);
    assert!(top.reason.contains("quote"));
    assert_eq!(
        top.transcript_anchor.as_deref(),
        Some("quote changed how people think")
    );
}

#[test]
fn matching_ranks_competing_visual_support_skills_deterministically() {
    let registry = EditorialSkillRegistry::bundled();

    let instances = registry.match_instances(MatchEditorialSkillsRequest {
        selection_text: "We will cover hooks, retention, export checks, and source-backed b-roll."
            .into(),
        request: "create visual support for this list and b-roll moment".into(),
        anchor_transcript: None,
    });
    let ids = instances
        .iter()
        .map(|instance| instance.skill_id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids[0], "retention-list-opener");
    assert!(ids.contains(&"source-backed-broll"));
    assert!(
        instances
            .windows(2)
            .all(|pair| pair[0].confidence >= pair[1].confidence),
        "instances must be ranked by confidence: {instances:?}"
    );
}

#[test]
fn story_signals_become_proposal_ready_skill_opportunities() {
    let registry = EditorialSkillRegistry::bundled();

    let opportunities = registry.story_opportunities([
        (
            "hook",
            "The opening quote lands as the strongest hook",
            0.0,
            8.0,
        ),
        (
            "topic_shift",
            "Now we move into the export checklist",
            42.0,
            46.0,
        ),
        (
            "weak_visual",
            "The speaker explains a crowded market with no visual support",
            90.0,
            96.0,
        ),
        (
            "stat",
            "Retention improved by 37 percent after adding chapter intros",
            120.0,
            126.0,
        ),
    ]);

    let ids = opportunities
        .iter()
        .map(|opportunity| opportunity.primary_skill_id.as_str())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"podcast-hook"));
    assert!(ids.contains(&"chapter-intro"));
    assert!(ids.contains(&"source-backed-broll"));
    assert!(ids.contains(&"statistic-counter"));

    for opportunity in opportunities {
        assert_eq!(opportunity.next_tool, "plan_visual_support_proposals");
        assert!(opportunity.timeline_start_s.is_some());
        assert!(!opportunity.selection_text.trim().is_empty());
        assert!(
            opportunity
                .skill_instances
                .iter()
                .any(|instance| instance.skill_id == opportunity.primary_skill_id)
        );
    }
}
