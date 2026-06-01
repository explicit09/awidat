//! Deterministic editorial skill registry, matching, and ranking.
//!
//! The bundled `skills/<name>/SKILL.md` files carry inspectable editorial
//! guidance. This module owns the stable registration, triggering, ranking,
//! and proposal-facing contracts so core editorial planning does not depend on
//! prompt interpretation alone.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorialSkillDefinition {
    pub id: &'static str,
    pub title: &'static str,
    pub purpose: &'static str,
    pub pipeline_stage: &'static str,
    pub artifact_type: &'static str,
    pub skill_path: &'static str,
    pub inspectable: bool,
    pub composable: bool,
    pub reusable_across_projects: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorialSkillInstance {
    pub skill_id: String,
    pub artifact_type: String,
    pub confidence: f64,
    pub reason: String,
    pub transcript_anchor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorialSkillOpportunity {
    pub trigger_kind: String,
    pub selection_text: String,
    pub request: String,
    pub timeline_start_s: Option<f64>,
    pub timeline_end_s: Option<f64>,
    pub primary_skill_id: String,
    pub primary_artifact_type: String,
    pub skill_instances: Vec<EditorialSkillInstance>,
    pub next_tool: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchEditorialSkillsRequest {
    pub selection_text: String,
    pub request: String,
    pub anchor_transcript: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EditorialSkillRegistry {
    definitions: Vec<EditorialSkillDefinition>,
}

impl EditorialSkillRegistry {
    pub fn bundled() -> Self {
        Self {
            definitions: BUNDLED_DEFINITIONS.to_vec(),
        }
    }

    pub fn definitions(&self) -> &[EditorialSkillDefinition] {
        &self.definitions
    }

    pub fn definition(&self, id: &str) -> Option<&EditorialSkillDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.id == id)
    }

    pub fn definition_for_artifact(
        &self,
        artifact_type: &str,
    ) -> Option<&EditorialSkillDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.artifact_type == artifact_type)
    }

    pub fn match_instances(
        &self,
        request: MatchEditorialSkillsRequest,
    ) -> Vec<EditorialSkillInstance> {
        let combined = format!("{} {}", request.selection_text, request.request).to_lowercase();
        let anchor = request
            .anchor_transcript
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| request.selection_text.trim())
            .to_string();

        let mut instances: Vec<EditorialSkillInstance> = self
            .definitions
            .iter()
            .filter_map(|definition| {
                let confidence = score_definition(definition, &combined, &request.selection_text);
                (confidence > 0.0).then(|| EditorialSkillInstance {
                    skill_id: definition.id.to_string(),
                    artifact_type: definition.artifact_type.to_string(),
                    confidence,
                    reason: reason_for(definition, &request.selection_text),
                    transcript_anchor: Some(anchor.clone()),
                })
            })
            .collect();

        if instances.is_empty() {
            for fallback in ["quote-highlight", "source-backed-broll"] {
                if let Some(definition) = self.definition(fallback) {
                    instances.push(EditorialSkillInstance {
                        skill_id: definition.id.to_string(),
                        artifact_type: definition.artifact_type.to_string(),
                        confidence: 0.52,
                        reason: reason_for(definition, &request.selection_text),
                        transcript_anchor: Some(anchor.clone()),
                    });
                }
            }
        }

        instances.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.skill_id.cmp(&b.skill_id))
        });
        instances
    }

    pub fn story_opportunities<I, K, T>(&self, signals: I) -> Vec<EditorialSkillOpportunity>
    where
        I: IntoIterator<Item = (K, T, f64, f64)>,
        K: AsRef<str>,
        T: AsRef<str>,
    {
        let mut opportunities = Vec::new();
        for (kind, text, start_s, end_s) in signals {
            let kind = kind.as_ref().trim();
            let text = text.as_ref().trim();
            if kind.is_empty() || text.is_empty() {
                continue;
            }
            let request = request_for_story_signal(kind, text);
            let instances = self.match_instances(MatchEditorialSkillsRequest {
                selection_text: text.to_string(),
                request,
                anchor_transcript: Some(text.to_string()),
            });
            let Some(primary) = instances.first() else {
                continue;
            };
            opportunities.push(EditorialSkillOpportunity {
                trigger_kind: kind.to_string(),
                selection_text: text.to_string(),
                request: request_for_story_signal(kind, text),
                timeline_start_s: Some(start_s),
                timeline_end_s: Some(end_s),
                primary_skill_id: primary.skill_id.clone(),
                primary_artifact_type: primary.artifact_type.clone(),
                skill_instances: instances,
                next_tool: "plan_visual_support_proposals".into(),
            });
        }
        opportunities
    }
}

pub fn definition_json(definition: &EditorialSkillDefinition) -> serde_json::Value {
    serde_json::json!({
        "id": definition.id,
        "title": definition.title,
        "purpose": definition.purpose,
        "pipeline_stage": definition.pipeline_stage,
        "artifact_type": definition.artifact_type,
        "skill_path": definition.skill_path,
        "inspectable": definition.inspectable,
        "composable": definition.composable,
        "reusable_across_projects": definition.reusable_across_projects,
    })
}

pub fn instance_json(instance: &EditorialSkillInstance) -> serde_json::Value {
    serde_json::json!({
        "skill_id": instance.skill_id,
        "artifact_type": instance.artifact_type,
        "confidence": instance.confidence,
        "reason": instance.reason,
        "transcript_anchor": instance.transcript_anchor,
    })
}

pub fn opportunity_json(opportunity: &EditorialSkillOpportunity) -> serde_json::Value {
    serde_json::json!({
        "trigger_kind": opportunity.trigger_kind,
        "selection_text": opportunity.selection_text,
        "request": opportunity.request,
        "timeline_start_s": opportunity.timeline_start_s,
        "timeline_end_s": opportunity.timeline_end_s,
        "primary_skill_id": opportunity.primary_skill_id,
        "primary_artifact_type": opportunity.primary_artifact_type,
        "skill_instances": opportunity.skill_instances.iter().map(instance_json).collect::<Vec<_>>(),
        "next_tool": opportunity.next_tool,
    })
}

fn request_for_story_signal(kind: &str, text: &str) -> String {
    let lower = format!("{kind} {text}").to_lowercase();
    if contains_any(&lower, &["hook", "cold open", "opening"]) {
        "package this podcast hook with visual support".into()
    } else if contains_any(&lower, &["topic_shift", "chapter", "move into", "segment"]) {
        "create a chapter intro title card".into()
    } else if contains_any(
        &lower,
        &["weak_visual", "no visual", "b-roll", "broll", "cutaway"],
    ) {
        "create source-backed B-roll visual support".into()
    } else if contains_any(&lower, &["stat", "percent", "number", "metric", "$"])
        || text.chars().any(|c| c.is_ascii_digit())
    {
        "create a statistic counter graphic".into()
    } else if contains_any(&lower, &["list", "steps", "checklist", "framework"]) {
        "create a retention list opener".into()
    } else if contains_any(&lower, &["map", "route", "city", "country", "location"]) {
        "create a route map visualization".into()
    } else if contains_any(&lower, &["quote", "said", "line"]) {
        "create a quote highlight".into()
    } else {
        "create visual support for this story moment".into()
    }
}

fn score_definition(
    definition: &EditorialSkillDefinition,
    combined: &str,
    selection_text: &str,
) -> f64 {
    match definition.id {
        "quote-highlight" if contains_any(combined, &["quote", "highlight", "pop", "land"]) => 0.90,
        "retention-list-opener"
            if contains_any(
                combined,
                &[
                    "list",
                    "steps",
                    "framework",
                    "cover ",
                    "we will cover",
                    "retention",
                ],
            ) =>
        {
            0.91
        }
        "chapter-intro"
            if contains_any(
                combined,
                &["chapter", "title", "topic shift", "segment intro"],
            ) =>
        {
            0.94
        }
        "search-bar-sequence" if contains_any(combined, &["search", "google", "query"]) => 0.78,
        "statistic-counter"
            if contains_any(
                combined,
                &[
                    "counter",
                    "statistic",
                    "stat card",
                    "stat graphic",
                    "percent",
                    "growth",
                    "cost",
                    "$",
                ],
            ) || selection_text.chars().any(|c| c.is_ascii_digit()) =>
        {
            0.95
        }
        "route-map"
            if contains_any(
                combined,
                &["map", "location", "route", "city", "country", "travel"],
            ) =>
        {
            0.76
        }
        "source-backed-broll"
            if contains_any(
                combined,
                &[
                    "b-roll",
                    "broll",
                    "footage",
                    "cutaway",
                    "show this",
                    "visual support",
                ],
            ) =>
        {
            0.80
        }
        "podcast-hook"
            if contains_any(combined, &["hook", "cold open", "opening"])
                && !contains_any(combined, &["list", "retention", "we will cover"]) =>
        {
            0.93
        }
        "short-form-reframing"
            if contains_any(combined, &["short-form", "shorts", "vertical", "9:16"]) =>
        {
            0.77
        }
        _ => 0.0,
    }
}

fn reason_for(definition: &EditorialSkillDefinition, selection_text: &str) -> String {
    format!(
        "{} matched because the selected context can support {}: {}",
        definition.title,
        definition.artifact_type,
        concise(selection_text)
    )
}

fn concise(value: &str) -> String {
    let words: Vec<&str> = value.split_whitespace().take(12).collect();
    let mut out = words.join(" ");
    if value.split_whitespace().count() > words.len() {
        out.push_str("...");
    }
    out
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

const BUNDLED_DEFINITIONS: &[EditorialSkillDefinition] = &[
    EditorialSkillDefinition {
        id: "quote-highlight",
        title: "Quote Highlight",
        purpose: "Make a selected spoken line land as a visual editorial beat.",
        pipeline_stage: "visual_polish",
        artifact_type: "quote_highlight",
        skill_path: "skills/quote-highlight/SKILL.md",
        inspectable: true,
        composable: true,
        reusable_across_projects: true,
    },
    EditorialSkillDefinition {
        id: "retention-list-opener",
        title: "Retention List Opener",
        purpose: "Preview a sequence of ideas so the viewer understands why to keep watching.",
        pipeline_stage: "story_structure",
        artifact_type: "animated_list",
        skill_path: "skills/retention-list-opener/SKILL.md",
        inspectable: true,
        composable: true,
        reusable_across_projects: true,
    },
    EditorialSkillDefinition {
        id: "search-bar-sequence",
        title: "Search Bar Sequence",
        purpose: "Turn a question or query into a typed visual sequence.",
        pipeline_stage: "visual_polish",
        artifact_type: "search_bar",
        skill_path: "skills/search-bar-sequence/SKILL.md",
        inspectable: true,
        composable: true,
        reusable_across_projects: true,
    },
    EditorialSkillDefinition {
        id: "source-backed-broll",
        title: "Source-backed B-roll",
        purpose: "Use footage-like support with source/provenance requirements.",
        pipeline_stage: "visual_polish",
        artifact_type: "broll_package",
        skill_path: "skills/source-backed-broll/SKILL.md",
        inspectable: true,
        composable: true,
        reusable_across_projects: true,
    },
    EditorialSkillDefinition {
        id: "route-map",
        title: "Route Map",
        purpose: "Show a location, route, or geographic comparison.",
        pipeline_stage: "visual_polish",
        artifact_type: "map_visualization",
        skill_path: "skills/route-map/SKILL.md",
        inspectable: true,
        composable: true,
        reusable_across_projects: true,
    },
    EditorialSkillDefinition {
        id: "statistic-counter",
        title: "Statistic Counter",
        purpose: "Make a number, metric, or scale claim readable and memorable.",
        pipeline_stage: "visual_polish",
        artifact_type: "counter_stat_graphic",
        skill_path: "skills/statistic-counter/SKILL.md",
        inspectable: true,
        composable: true,
        reusable_across_projects: true,
    },
    EditorialSkillDefinition {
        id: "podcast-hook",
        title: "Podcast Hook",
        purpose: "Find and package strong opening moments with visual support.",
        pipeline_stage: "story_structure",
        artifact_type: "quote_highlight",
        skill_path: "skills/podcast-hook/SKILL.md",
        inspectable: true,
        composable: true,
        reusable_across_projects: true,
    },
    EditorialSkillDefinition {
        id: "chapter-intro",
        title: "Chapter Intro",
        purpose: "Introduce a chapter, topic shift, or section boundary.",
        pipeline_stage: "chaptering",
        artifact_type: "title_card",
        skill_path: "skills/chapter-intro/SKILL.md",
        inspectable: true,
        composable: true,
        reusable_across_projects: true,
    },
    EditorialSkillDefinition {
        id: "short-form-reframing",
        title: "Short-form Reframing",
        purpose: "Adapt a selected moment into vertical or short-form framing.",
        pipeline_stage: "short_form",
        artifact_type: "title_card",
        skill_path: "skills/short-form-reframing/SKILL.md",
        inspectable: true,
        composable: true,
        reusable_across_projects: true,
    },
];
