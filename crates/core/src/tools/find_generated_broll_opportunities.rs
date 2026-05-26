//! `find_generated_broll_opportunities` tool — surface podcast moments
//! where AI-generated B-roll would help explain, pace, or reinforce the
//! transcript.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use awidat_proto::otio::{MediaReference, StackChild, Timeline, TrackChild};
use awidat_proto::project::Project;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};

const DEFAULT_MAX_RESULTS: usize = 12;
const HARD_MAX_RESULTS: usize = 40;
const DEFAULT_BROLL_DURATION_S: f64 = 4.0;
const SCORE_THRESHOLD: f64 = 0.45;

const VISUAL_TERMS: &[&str] = &[
    "GPU data centers",
    "data centers",
    "cooling systems",
    "server farms",
    "GPU racks",
    "servers",
    "Mars rover",
    "rover",
    "Mars",
    "dust storm",
    "robot",
    "robots",
    "office",
    "founders",
    "pitch",
    "stock charts",
    "charts",
    "graph",
    "graphs",
    "map",
    "city",
    "fintech app",
    "solar farms",
    "ocean",
    "carbon capture",
    "power grid",
    "factory",
    "warehouse",
    "keyboard",
    "code",
    "laptop",
    "screen",
    "acquisition",
    "CFO",
    "investor",
    "investors",
    "boardroom",
    "IPO",
    "product",
    "customers",
    "ads",
    "ad variants",
    "valuation",
    "secondary market",
    "funding round",
    "ARR",
    "revenue",
    "tokens",
    "worktree",
    "branch",
    "Nvidia",
    "benchmark",
    "model",
    "vulnerability",
    "security",
    "open source",
    "rate limit",
    "social media",
    "attention",
    "summarize",
    "developer",
    "solo founder",
    "deploy",
    "marketing",
    "operations",
    "agents",
    "dog",
    "cancer",
    "university",
    "protein",
    "lab",
    "regulation",
    "timeline",
];

const ABSTRACT_TERMS: &[&str] = &[
    "AI",
    "compute",
    "economy",
    "funding",
    "market",
    "automation",
    "infrastructure",
    "startup",
    "startups",
    "growth",
    "jobs",
    "climate",
    "energy",
];

const ABSTRACT_INTENSIFIERS: &[&str] = &[
    "exploding",
    "accelerating",
    "replacing",
    "disrupting",
    "transforming",
    "scaling",
    "growing",
];

const EXPLANATION_PHRASES: &[&str] = &[
    "the way",
    "how",
    "works",
    "basically",
    "means",
    "because",
    "so ",
    "the reason",
];

const EMOTIONAL_PHRASES: &[&str] = &[
    "insane",
    "crazy",
    "failed",
    "almost failed",
    "impossible",
    "scary",
    "exciting",
    "nobody saw",
    "huge",
    "wild",
];

const STORY_PHRASES: &[&str] = &[
    "we were",
    "i was",
    "then",
    "suddenly",
    "in the meeting",
    "at the time",
    "we had to",
];

const STATISTIC_PHRASES: &[&str] = &[
    "percent",
    "%",
    "million",
    "millions",
    "billion",
    "billions",
    "hundreds",
    "thousands",
    "cost",
    "revenue",
    "dollars",
];

const PRODUCT_DEMO_TERMS: &[&str] = &[
    "product",
    "demo",
    "app",
    "ui",
    "dashboard",
    "website",
    "marketplace",
    "menu",
    "screen",
    "tool",
    "feature",
    "workflow",
];

const BRAND_CONTEXT_TERMS: &[&str] = &[
    "company",
    "startup",
    "acquisition",
    "valuation",
    "funding",
    "ipo",
    "arr",
    "revenue",
    "brand",
    "logo",
];

const PERSON_CONTEXT_TERMS: &[&str] =
    &["founder", "ceo", "guest", "interview", "creator", "speaker"];

/// Read-only tool that proposes Seedance/OpenRouter B-roll moments from transcripts.
pub struct FindGeneratedBrollOpportunitiesTool;

#[derive(Debug, Deserialize)]
struct Args {
    #[serde(default)]
    asset_id: Option<String>,
    #[serde(default)]
    duration_s: Option<f64>,
    #[serde(default)]
    max_results: Option<usize>,
}

#[async_trait]
impl ToolHandler for FindGeneratedBrollOpportunitiesTool {
    fn name(&self) -> &'static str {
        "find_generated_broll_opportunities"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "find_generated_broll_opportunities".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {
                        "type": "string",
                        "description": "Restrict to one asset's whisper sidecar."
                    },
                    "duration_s": {
                        "type": "number",
                        "minimum": 1.0,
                        "maximum": 8.0,
                        "description": "Default generated B-roll length in seconds. Default 4.0."
                    },
                    "max_results": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 40,
                        "description": "Default 12, hard cap 40."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_generated_broll_opportunities: invalid args ({e}). All fields optional."
            ))
        })?;
        let max_results = args
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .min(HARD_MAX_RESULTS);
        let duration_s = args
            .duration_s
            .unwrap_or(DEFAULT_BROLL_DURATION_S)
            .clamp(1.0, 8.0);
        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "find_generated_broll_opportunities: failed to read project: {e}"
            ))
        })?;

        let findings = scan_generated_broll_opportunities(
            &ctx.project_root,
            &project.timeline,
            args.asset_id.as_deref(),
            duration_s,
            max_results,
        );
        let body = serde_json::json!({
            "findings": findings,
            "more_available": findings.len() == max_results,
            "next_step": "Review a finding, then call start_generated_media_job with provider=openrouter, artifact_kind=video, workflow_purpose=broll, prompt, and duration."
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

/// Scan the timeline's transcript sidecars for generated B-roll opportunities.
pub fn scan_generated_broll_opportunities(
    project_root: &Path,
    timeline: &Timeline,
    only_asset: Option<&str>,
    duration_s: f64,
    max_results: usize,
) -> Vec<GeneratedBrollFinding> {
    let mut out = Vec::new();
    let timeline_cursor_s = 0.0_f64;

    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let mut track_cursor_s = 0.0_f64;
        let mut last_emit_timeline_end: Option<f64> = None;
        for tc in &track.children {
            match tc {
                TrackChild::Clip(clip) => {
                    let MediaReference::External(ext) = &clip.media_reference else {
                        if let Some(range) = clip.source_range.as_ref() {
                            track_cursor_s += range.duration.to_seconds();
                        }
                        continue;
                    };
                    let Some(range) = clip.source_range.as_ref() else {
                        continue;
                    };
                    let asset_id = ext.target_url.clone();
                    if let Some(filter) = only_asset
                        && asset_id != filter
                    {
                        track_cursor_s += range.duration.to_seconds();
                        continue;
                    }

                    let clip_source_start = range.start_time.to_seconds();
                    let clip_source_end = clip_source_start + range.duration.to_seconds();
                    let clip_track_start = track_cursor_s;
                    if let Some(items) = load_transcript_items(project_root, &asset_id) {
                        for item in items {
                            if item.start_s < clip_source_start || item.start_s >= clip_source_end {
                                continue;
                            }
                            let timeline_start = timeline_cursor_s
                                + clip_track_start
                                + (item.start_s - clip_source_start);
                            if let Some(prev_end) = last_emit_timeline_end
                                && timeline_start < prev_end + duration_s
                            {
                                continue;
                            }
                            let Some(signal) = score_segment(&item.text) else {
                                continue;
                            };
                            let timeline_end = (timeline_start + duration_s)
                                .min(clip_track_start + range.duration.to_seconds());
                            let output_duration_s = timeline_end - timeline_start;
                            let asset_requests =
                                asset_requests_for_moment(&item.text, &signal.subject);
                            out.push(GeneratedBrollFinding {
                                asset_id: asset_id.clone(),
                                source_start_s: item.start_s,
                                source_end_s: (item.start_s + output_duration_s)
                                    .min(item.end_s)
                                    .min(clip_source_end),
                                timeline_start_s: timeline_start,
                                timeline_end_s: timeline_end,
                                category: signal.category,
                                score: signal.score,
                                reason: signal.reason,
                                transcript_excerpt: item.text,
                                prompt: build_prompt(&signal.subject, output_duration_s),
                                asset_requests,
                                fallback_prompt: build_fallback_prompt(
                                    &signal.subject,
                                    output_duration_s,
                                ),
                                provider: "openrouter".into(),
                                model: crate::generated_media::openrouter::DEFAULT_VIDEO_MODEL
                                    .into(),
                            });
                            last_emit_timeline_end = Some(timeline_end);
                            if out.len() >= max_results {
                                return out;
                            }
                        }
                    }
                    track_cursor_s += range.duration.to_seconds();
                }
                TrackChild::Gap(gap) => {
                    track_cursor_s += gap.source_range.duration.to_seconds();
                }
                TrackChild::Transition(_) | TrackChild::Stack(_) => {}
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
/// High-level reason a transcript moment is suitable for generated B-roll.
pub enum GeneratedBrollCategory {
    /// A concrete visual noun or scene appears in the transcript.
    VisualConcept,
    /// The speaker is teaching or explaining something visualizable.
    Explanatory,
    /// An abstract topic needs a concrete visual anchor.
    AbstractToConcrete,
    /// Emotion or intensity spikes and can be reinforced visually.
    Emotional,
    /// The speaker is narrating an event or scenario.
    StoryReconstruction,
    /// Numbers, scale, cost, or growth can be made visible.
    Statistics,
}

/// One generated B-roll candidate ready for review and provider submission.
#[derive(Debug, Clone, Serialize)]
pub struct GeneratedBrollFinding {
    /// Source asset where the trigger fired.
    pub asset_id: String,
    /// Source-media seconds where the generated B-roll should anchor.
    pub source_start_s: f64,
    /// Source-media seconds where the candidate window ends.
    pub source_end_s: f64,
    /// Timeline-time start for the generated B-roll.
    pub timeline_start_s: f64,
    /// Timeline-time end for the generated B-roll.
    pub timeline_end_s: f64,
    /// Heuristic class that caused this finding.
    pub category: GeneratedBrollCategory,
    /// Normalized heuristic confidence, from 0.0 to 1.0.
    pub score: f64,
    /// Short explanation for the reviewer or agent.
    pub reason: String,
    /// Transcript text that triggered the suggestion.
    pub transcript_excerpt: String,
    /// Provider-ready text-to-video prompt.
    pub prompt: String,
    /// Reference assets or extra context that would materially improve this plan.
    pub asset_requests: Vec<GeneratedBrollAssetRequest>,
    /// Provider-ready fallback prompt if requested assets are unavailable.
    pub fallback_prompt: String,
    /// Suggested generation provider.
    pub provider: String,
    /// Suggested provider model.
    pub model: String,
}

/// One optional or recommended user-provided asset/context request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GeneratedBrollAssetRequest {
    /// Stable request kind, such as `logo`, `screenshot`, or `screen_recording`.
    pub kind: String,
    /// `optional`, `recommended`, or `strongly_recommended`.
    pub priority: String,
    /// Question the agent can ask the user.
    pub question: String,
    /// Why this asset/context matters for the planned B-roll.
    pub reason: String,
    /// What the agent should do if the user does not have this asset/context.
    pub fallback: String,
}

#[derive(Debug)]
struct Signal {
    category: GeneratedBrollCategory,
    score: f64,
    subject: String,
    reason: String,
}

#[derive(Debug)]
struct TranscriptItem {
    text: String,
    start_s: f64,
    end_s: f64,
}

fn score_segment(text: &str) -> Option<Signal> {
    let lower = text.to_lowercase();
    let visual = first_case_preserving_match(text, VISUAL_TERMS);
    let abstract_hit = contains_any(&lower, ABSTRACT_TERMS);
    let abstract_intensity = contains_any(&lower, ABSTRACT_INTENSIFIERS);
    let explanation = contains_any(&lower, EXPLANATION_PHRASES);
    let emotion = contains_any(&lower, EMOTIONAL_PHRASES);
    let story = contains_any(&lower, STORY_PHRASES);
    let statistics =
        contains_any(&lower, STATISTIC_PHRASES) || lower.chars().any(|c| c.is_ascii_digit());

    let mut score = 0.0_f64;
    if visual.is_some() {
        score += 0.45;
    }
    if explanation {
        score += 0.25;
    }
    if abstract_hit {
        score += 0.20;
    }
    if abstract_hit && abstract_intensity {
        score += 0.25;
    }
    if statistics {
        score += 0.35;
    }
    if story {
        score += 0.30;
    }
    if emotion {
        score += 0.25;
    }
    if score < SCORE_THRESHOLD {
        return None;
    }

    let category = if statistics {
        GeneratedBrollCategory::Statistics
    } else if explanation && (visual.is_some() || abstract_hit) {
        GeneratedBrollCategory::Explanatory
    } else if visual.is_some() {
        GeneratedBrollCategory::VisualConcept
    } else if abstract_hit {
        GeneratedBrollCategory::AbstractToConcrete
    } else if story {
        GeneratedBrollCategory::StoryReconstruction
    } else {
        GeneratedBrollCategory::Emotional
    };
    let subject = match category {
        GeneratedBrollCategory::Statistics | GeneratedBrollCategory::AbstractToConcrete => {
            concise_subject(text)
        }
        _ => visual.unwrap_or_else(|| concise_subject(text)),
    };
    Some(Signal {
        category,
        score: score.min(1.0),
        reason: reason_for(category, &subject),
        subject,
    })
}

fn reason_for(category: GeneratedBrollCategory, subject: &str) -> String {
    match category {
        GeneratedBrollCategory::VisualConcept => {
            format!("visual concept reference: {subject}")
        }
        GeneratedBrollCategory::Explanatory => {
            format!("explanation becomes easier to follow with visuals: {subject}")
        }
        GeneratedBrollCategory::AbstractToConcrete => {
            format!("abstract idea can be anchored with concrete imagery: {subject}")
        }
        GeneratedBrollCategory::Emotional => {
            format!("emotional spike can be reinforced visually: {subject}")
        }
        GeneratedBrollCategory::StoryReconstruction => {
            format!("story moment can be reconstructed visually: {subject}")
        }
        GeneratedBrollCategory::Statistics => {
            format!("number or scale claim benefits from a visual anchor: {subject}")
        }
    }
}

fn build_prompt(subject: &str, duration_s: f64) -> String {
    format!(
        "Realistic documentary B-roll of {subject}; unidentifiable people if present, no famous likeness, no logos, no readable text overlays, natural light, smooth camera movement, modern tech podcast style, 16:9, {:.0} seconds.",
        duration_s.clamp(1.0, 8.0).round()
    )
}

fn build_fallback_prompt(subject: &str, duration_s: f64) -> String {
    format!(
        "Generic unbranded documentary B-roll of {subject}; avoid real logos, avoid readable UI text, use fictional interface details if needed, unidentifiable people if present, 16:9, {:.0} seconds.",
        duration_s.clamp(1.0, 8.0).round()
    )
}

/// Return reference-asset/context requests implied by a planned generated B-roll moment.
pub fn asset_requests_for_moment(text: &str, subject: &str) -> Vec<GeneratedBrollAssetRequest> {
    let lower = text.to_lowercase();
    let mut requests = Vec::new();
    if contains_any(&lower, PRODUCT_DEMO_TERMS) {
        requests.push(asset_request(
            "screenshot",
            "strongly_recommended",
            "Do you have product screenshots or app UI images for this demo-style B-roll?",
            "A product-demo visual is much stronger when the generated shot can match the actual interface.",
            "Use a generic fictional product interface with no readable text or real branding.",
        ));
        requests.push(asset_request(
            "logo",
            "recommended",
            "Do you have the product or company logo for this B-roll?",
            "A logo helps make the product reference clear when the plan is brand or demo oriented.",
            "Use an unbranded product-demo visual and avoid logos.",
        ));
        requests.push(asset_request(
            "screen_recording",
            "optional",
            "Do you have a short screen recording of the product flow?",
            "A screen recording can guide the generated camera move and UI sequence.",
            "Use static generic UI B-roll instead of recreating the exact flow.",
        ));
    } else if contains_any(&lower, BRAND_CONTEXT_TERMS) {
        requests.push(asset_request(
            "logo",
            "recommended",
            "Do you have a logo or approved brand asset for this company/product reference?",
            "Brand assets can make company, acquisition, funding, and valuation B-roll feel specific.",
            "Use an unbranded chart, office, or abstract business visual.",
        ));
    }

    if contains_any(&lower, PERSON_CONTEXT_TERMS) {
        requests.push(asset_request(
            "person_reference",
            "optional",
            "Do you have an approved reference image for the guest/founder, or should this stay unidentifiable?",
            "A person reference can support likeness-aware visuals when the user has rights and wants that specificity.",
            "Use unidentifiable people or hands-at-work visuals.",
        ));
    }

    if !requests.is_empty() {
        requests.push(asset_request(
            "context_note",
            "optional",
            &format!("Any extra context for how `{subject}` should look?"),
            "Short context can prevent the generated B-roll from inventing the wrong product, setting, or tone.",
            "Use the fallback prompt exactly as written.",
        ));
    }
    requests
}

fn asset_request(
    kind: &str,
    priority: &str,
    question: &str,
    reason: &str,
    fallback: &str,
) -> GeneratedBrollAssetRequest {
    GeneratedBrollAssetRequest {
        kind: kind.into(),
        priority: priority.into(),
        question: question.into(),
        reason: reason.into(),
        fallback: fallback.into(),
    }
}

fn concise_subject(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().take(10).collect();
    words.join(" ")
}

fn contains_any(lower_text: &str, needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| lower_text.contains(&needle.to_lowercase()))
}

fn first_case_preserving_match(text: &str, needles: &[&str]) -> Option<String> {
    let lower_text = text.to_lowercase();
    needles.iter().find_map(|needle| {
        let lower_needle = needle.to_lowercase();
        lower_text.find(&lower_needle).map(|start| {
            let end = start + needle.len();
            text.get(start..end).unwrap_or(needle).to_string()
        })
    })
}

fn load_transcript_items(project_root: &Path, asset_id: &str) -> Option<Vec<TranscriptItem>> {
    let path = whisper_sidecar_path(project_root, asset_id);
    let bytes = std::fs::read(&path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let segments = value
        .pointer("/data/segments")
        .and_then(|v| v.as_array())
        .map(|items| transcript_items_from_json(items.as_slice()))
        .filter(|items| !items.is_empty());
    if segments.is_some() {
        return segments;
    }
    value
        .pointer("/data/words")
        .and_then(|v| v.as_array())
        .map(|items| transcript_items_from_json(items.as_slice()))
        .filter(|items| !items.is_empty())
}

fn transcript_items_from_json(items: &[serde_json::Value]) -> Vec<TranscriptItem> {
    let mut out = Vec::new();
    for item in items {
        let text = item
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            continue;
        }
        let start_s = item
            .get("start_s")
            .or_else(|| item.get("start"))
            .and_then(|v| v.as_f64());
        let end_s = item
            .get("end_s")
            .or_else(|| item.get("end"))
            .and_then(|v| v.as_f64());
        if let (Some(start_s), Some(end_s)) = (start_s, end_s)
            && end_s > start_s
        {
            out.push(TranscriptItem {
                text: text.to_string(),
                start_s,
                end_s,
            });
        }
    }
    out
}

fn whisper_sidecar_path(project_root: &Path, asset_id: &str) -> PathBuf {
    project_root
        .join("index")
        .join("whisper")
        .join(format!("{asset_id}.json"))
}

const DESCRIPTION: &str = "\
Find transcript moments where AI-generated podcast B-roll would help: \
visual concepts, explanations, abstract-to-concrete moments, emotional \
spikes, story reconstruction, and statistics. This is read-only and \
does not call a generation provider. It returns scored candidates with \
OpenRouter/Seedance-ready prompts so the agent can review the moment \
before calling `start_generated_media_job`.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use awidat_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, Stack, StackChild, TimeRange, Track,
        TrackChild, TrackKind,
    };

    fn timeline_with_clip(asset_id: &str, source_start_s: f64, duration_s: f64) -> Timeline {
        let mut clip = Clip::empty("c1");
        clip.media_reference = MediaReference::External(ExternalReference::new(asset_id));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(source_start_s * 24.0, 24.0),
            RationalTime::new(duration_s * 24.0, 24.0),
        ));
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip));
        let mut tl = Timeline::empty("p");
        let mut stack = Stack::empty("root");
        stack.children.push(StackChild::Track(track));
        tl.tracks = stack;
        tl
    }

    fn write_segments(project_root: &Path, asset_id: &str, segments: Vec<(&str, f64, f64)>) {
        let path = whisper_sidecar_path(project_root, asset_id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let payload = serde_json::json!({
            "indexer": "whisper",
            "asset_id": asset_id,
            "data": {
                "segments": segments
                    .into_iter()
                    .map(|(text, start_s, end_s)| {
                        serde_json::json!({"text": text, "start_s": start_s, "end_s": end_s})
                    })
                    .collect::<Vec<_>>()
            }
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&payload).unwrap()).unwrap();
    }

    #[test]
    fn finds_generated_tech_explanation_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let asset_id = "raw/episode.mp4";
        write_segments(
            dir.path(),
            asset_id,
            vec![(
                "So the way AI compute is exploding is these GPU data centers need massive cooling systems",
                10.0,
                15.0,
            )],
        );

        let findings = scan_generated_broll_opportunities(
            dir.path(),
            &timeline_with_clip(asset_id, 0.0, 60.0),
            None,
            4.0,
            12,
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].provider, "openrouter");
        assert_eq!(
            findings[0].model,
            crate::generated_media::openrouter::DEFAULT_VIDEO_MODEL
        );
        assert!(matches!(
            findings[0].category,
            GeneratedBrollCategory::Explanatory
                | GeneratedBrollCategory::AbstractToConcrete
                | GeneratedBrollCategory::VisualConcept
        ));
        assert!(findings[0].prompt.contains("GPU data centers"));
    }

    #[test]
    fn skips_generic_podcast_filler() {
        let dir = tempfile::tempdir().unwrap();
        let asset_id = "raw/episode.mp4";
        write_segments(
            dir.path(),
            asset_id,
            vec![("yeah I mean you know that was really interesting", 5.0, 8.0)],
        );

        let findings = scan_generated_broll_opportunities(
            dir.path(),
            &timeline_with_clip(asset_id, 0.0, 20.0),
            None,
            4.0,
            12,
        );

        assert!(findings.is_empty());
    }

    #[test]
    fn classifies_statistics_as_visualizable() {
        let dir = tempfile::tempdir().unwrap();
        let asset_id = "raw/episode.mp4";
        write_segments(
            dir.path(),
            asset_id,
            vec![(
                "AI models now cost hundreds of millions to train",
                20.0,
                24.0,
            )],
        );

        let findings = scan_generated_broll_opportunities(
            dir.path(),
            &timeline_with_clip(asset_id, 0.0, 60.0),
            None,
            4.0,
            12,
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, GeneratedBrollCategory::Statistics);
        assert!(findings[0].prompt.contains("hundreds of millions"));
    }

    #[test]
    fn emits_abstract_to_concrete_when_scale_changes() {
        let dir = tempfile::tempdir().unwrap();
        let asset_id = "raw/episode.mp4";
        write_segments(
            dir.path(),
            asset_id,
            vec![(
                "AI compute is exploding across the whole economy",
                30.0,
                33.0,
            )],
        );

        let findings = scan_generated_broll_opportunities(
            dir.path(),
            &timeline_with_clip(asset_id, 0.0, 60.0),
            None,
            4.0,
            12,
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].category,
            GeneratedBrollCategory::AbstractToConcrete
        );
        assert!(findings[0].prompt.contains("AI compute is exploding"));
    }

    #[test]
    fn product_demo_requests_reference_assets_with_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let asset_id = "raw/episode.mp4";
        write_segments(
            dir.path(),
            asset_id,
            vec![(
                "Then we open the product demo and show the app dashboard workflow",
                40.0,
                44.0,
            )],
        );

        let findings = scan_generated_broll_opportunities(
            dir.path(),
            &timeline_with_clip(asset_id, 0.0, 60.0),
            None,
            4.0,
            12,
        );

        assert_eq!(findings.len(), 1);
        let kinds = findings[0]
            .asset_requests
            .iter()
            .map(|request| request.kind.as_str())
            .collect::<Vec<_>>();
        assert!(kinds.contains(&"screenshot"));
        assert!(kinds.contains(&"logo"));
        assert!(kinds.contains(&"screen_recording"));
        assert!(findings[0].fallback_prompt.contains("Generic unbranded"));
    }

    #[test]
    fn plain_statistics_do_not_request_brand_assets() {
        let dir = tempfile::tempdir().unwrap();
        let asset_id = "raw/episode.mp4";
        write_segments(
            dir.path(),
            asset_id,
            vec![("The market grew by 40 percent last year", 50.0, 54.0)],
        );

        let findings = scan_generated_broll_opportunities(
            dir.path(),
            &timeline_with_clip(asset_id, 0.0, 60.0),
            None,
            4.0,
            12,
        );

        assert_eq!(findings.len(), 1);
        assert!(findings[0].asset_requests.is_empty());
    }
}
