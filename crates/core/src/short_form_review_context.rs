//! Trend and visual-decision context for short-form review candidates.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendAlignment {
    pub matched: bool,
    pub signals: Vec<MatchedTrendSignal>,
    pub fallback_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedTrendSignal {
    pub source: String,
    pub label: String,
    pub score: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualDecisionPlan {
    pub moment_kind: String,
    pub decisions: Vec<VisualDecision>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisualDecision {
    pub tool: String,
    pub action: String,
    pub reason: String,
}

pub(crate) struct TrendMoment<'a> {
    pub kind: &'a str,
    pub text: &'a str,
    pub reason: &'a str,
}

pub(crate) struct VisualDecisionInput<'a> {
    pub moment_kind: &'a str,
    pub moment_text: &'a str,
    pub broll_needed: bool,
    pub broll_rationale: &'a str,
    pub speaker_strategy: &'a str,
    pub topic_present: bool,
}

#[derive(Debug)]
struct TrendSignal {
    source: String,
    label: String,
    keywords: Vec<String>,
    weight: f64,
    reason: String,
}

pub(crate) fn trend_alignment(
    context: &serde_json::Value,
    moment: TrendMoment<'_>,
) -> TrendAlignment {
    let signals = trend_signals(context);
    if signals.is_empty() {
        return TrendAlignment {
            matched: false,
            signals: Vec::new(),
            fallback_reason: "trend context unavailable; ranked from episode evidence only"
                .to_string(),
        };
    }

    let haystack = normalized_terms(&format!(
        "{} {} {}",
        moment.kind, moment.text, moment.reason
    ));
    let mut matches: Vec<MatchedTrendSignal> = signals
        .into_iter()
        .filter_map(|signal| {
            let score = trend_signal_score(&haystack, &signal);
            (score > 0.0).then_some(MatchedTrendSignal {
                source: signal.source,
                label: signal.label,
                score,
                reason: signal.reason,
            })
        })
        .collect();
    matches.sort_by(|a, b| b.score.total_cmp(&a.score));
    matches.truncate(3);

    if matches.is_empty() {
        TrendAlignment {
            matched: false,
            signals: Vec::new(),
            fallback_reason:
                "provided trend context did not match this candidate; ranked from episode evidence"
                    .to_string(),
        }
    } else {
        TrendAlignment {
            matched: true,
            signals: matches,
            fallback_reason: String::new(),
        }
    }
}

pub(crate) fn trend_alignment_score(alignment: &TrendAlignment) -> f64 {
    alignment
        .signals
        .iter()
        .map(|signal| signal.score)
        .fold(0.0, f64::max)
}

pub(crate) fn visual_decision_plan(input: VisualDecisionInput<'_>) -> VisualDecisionPlan {
    let moment_kind = visual_moment_kind(input.moment_kind, input.moment_text);
    let mut decisions = Vec::new();
    if input.speaker_strategy.contains("split") || input.speaker_strategy.contains("both speakers")
    {
        decisions.push(VisualDecision {
            tool: "plan_multicam".to_string(),
            action: "preserve split/stacked speaker view, then punch in only when one speaker carries the beat".to_string(),
            reason: "multiple faces or speakers are part of the value".to_string(),
        });
    } else {
        decisions.push(VisualDecision {
            tool: "plan_reframe".to_string(),
            action: "speaker-safe punch-in for the strongest line".to_string(),
            reason: "short-form framing needs active visual focus".to_string(),
        });
    }

    match moment_kind.as_str() {
        "analogy" | "process" | "technical_concept" => decisions.push(VisualDecision {
            tool: "plan_visual_support_proposals".to_string(),
            action: "request MotionScene explainer or simple diagram for the abstract idea"
                .to_string(),
            reason: "the transcript asks the viewer to understand a concept, not just watch a face"
                .to_string(),
        }),
        "thesis" | "counter_thesis" | "founder_lesson" => decisions.push(VisualDecision {
            tool: "plan_visual_support_proposals".to_string(),
            action: "request quote highlight, title card, or founder-craft callout".to_string(),
            reason: "the line should land as an editorial takeaway".to_string(),
        }),
        "stat_or_claim" | "product_reference" => decisions.push(VisualDecision {
            tool: "plan_visual_support_proposals".to_string(),
            action: "request source card, product screenshot, or evidence-backed visual"
                .to_string(),
            reason: "factual and product references should be shown or sourced when possible"
                .to_string(),
        }),
        "joke_or_reaction" => decisions.push(VisualDecision {
            tool: "plan_emphasis".to_string(),
            action: "emphasize the face/reaction; avoid covering the punchline with B-roll"
                .to_string(),
            reason: "the viewer value is personality and timing".to_string(),
        }),
        _ => {}
    }

    if input.broll_needed {
        decisions.push(VisualDecision {
            tool: "find_generated_broll_opportunities".to_string(),
            action: "use B-roll when it clarifies the point or resets short-form attention"
                .to_string(),
            reason: input.broll_rationale.to_string(),
        });
    }

    VisualDecisionPlan {
        moment_kind,
        decisions,
        rationale: format!(
            "visual plan is based on transcript moment, {}topic, B-roll need, and speaker layout",
            if input.topic_present {
                "matched "
            } else {
                "no matched "
            }
        ),
    }
}

fn trend_signals(context: &serde_json::Value) -> Vec<TrendSignal> {
    let values = if let Some(signals) = context.pointer("/signals").and_then(|v| v.as_array()) {
        signals.iter().collect()
    } else if let Some(signals) = context.as_array() {
        signals.iter().collect()
    } else {
        Vec::new()
    };

    values
        .into_iter()
        .filter_map(|value| {
            let label = string_field(value, &["label", "topic", "query"])?;
            let mut keywords = string_array_field(value, "keywords");
            if keywords.is_empty() {
                keywords.push(label.clone());
            }
            Some(TrendSignal {
                source: string_field(value, &["source", "provider"])
                    .unwrap_or_else(|| "trend".into()),
                label,
                keywords,
                weight: number_field(value, &["weight", "score", "confidence"])
                    .unwrap_or(0.5)
                    .clamp(0.0, 1.0),
                reason: string_field(value, &["reason", "rationale"])
                    .unwrap_or_else(|| "matched supplied trend context".into()),
            })
        })
        .collect()
}

fn trend_signal_score(haystack: &[String], signal: &TrendSignal) -> f64 {
    let mut hits = 0.0;
    for keyword in &signal.keywords {
        let keyword_terms = normalized_terms(keyword);
        if keyword_terms.is_empty() {
            continue;
        }
        if terms_contain_phrase(haystack, &keyword_terms) {
            hits += if keyword_terms.len() > 1 { 0.65 } else { 0.35 };
        }
    }
    round2((hits * signal.weight).clamp(0.0, 1.0))
}

fn normalized_terms(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_alphanumeric())
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn terms_contain_phrase(haystack: &[String], phrase: &[String]) -> bool {
    !phrase.is_empty()
        && phrase.len() <= haystack.len()
        && haystack
            .windows(phrase.len())
            .any(|window| window.iter().zip(phrase).all(|(left, right)| left == right))
}

fn visual_moment_kind(kind: &str, text: &str) -> String {
    let lower = format!("{kind} {text}").to_lowercase();
    if contains_any(&lower, &["analogy", "like ", "metaphor"]) {
        "analogy".to_string()
    } else if contains_any(&lower, &["disagree", "debate", "counter", "wrong"]) {
        "counter_thesis".to_string()
    } else if contains_any(&lower, &["lesson", "learned", "founder", "craft"]) {
        "founder_lesson".to_string()
    } else if contains_any(&lower, &["percent", "stat", "number", "data", "claim"]) {
        "stat_or_claim".to_string()
    } else if contains_any(&lower, &["product", "dashboard", "app", "company"]) {
        "product_reference".to_string()
    } else if contains_any(&lower, &["joke", "funny", "laugh", "reaction"]) {
        "joke_or_reaction".to_string()
    } else if contains_any(&lower, &["process", "framework", "system", "technical"]) {
        "process".to_string()
    } else if contains_any(&lower, &["why", "because", "thesis", "take"]) {
        "thesis".to_string()
    } else {
        "general_clip".to_string()
    }
}

fn number_field(value: &serde_json::Value, names: &[&str]) -> Option<f64> {
    names.iter().find_map(|name| value.get(*name)?.as_f64())
}

fn string_field(value: &serde_json::Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value.get(*name)?.as_str().map(str::to_string))
}

fn string_array_field(value: &serde_json::Value, name: &str) -> Vec<String> {
    value
        .get(name)
        .and_then(|items| items.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trend_alignment_does_not_match_keyword_inside_unrelated_word() {
        let context = serde_json::json!({
            "signals": [{
                "source": "x",
                "label": "app store policy",
                "keywords": ["app"],
                "weight": 0.9,
                "reason": "recent X search"
            }]
        });

        let alignment = trend_alignment(
            &context,
            TrendMoment {
                kind: "thesis",
                text: "That happened because review work moved upstream.",
                reason: "standalone explanation",
            },
        );

        assert!(!alignment.matched);
    }
}
