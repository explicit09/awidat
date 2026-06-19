//! Long-form to short-form candidate review planning.
//!
//! This module is intentionally pure: it ranks complete, standalone
//! moments and emits reviewable draft packages. Applying an EDL remains a
//! separate mutating tool call governed by the session approval mode.

#![allow(missing_docs)]

use serde::{Deserialize, Serialize};

use crate::short_form_review_context::{
    TrendAlignment, TrendMoment, VisualDecisionInput, VisualDecisionPlan, trend_alignment,
    trend_alignment_score, visual_decision_plan,
};

const DEFAULT_MAX_CANDIDATES: usize = 15;
const DEFAULT_MAX_DURATION_S: f64 = 300.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortFormReviewInput {
    pub asset_id: String,
    pub source_width: u32,
    pub source_height: u32,
    pub transcript: serde_json::Value,
    pub editorial_moments: serde_json::Value,
    pub audio_energy: serde_json::Value,
    pub topics: serde_json::Value,
    pub scenes: serde_json::Value,
    pub shot: serde_json::Value,
    pub face: serde_json::Value,
    pub gaze: serde_json::Value,
    pub frame_quality: serde_json::Value,
    pub composition: serde_json::Value,
    pub broll_assets: serde_json::Value,
    pub trend_context: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ShortFormReviewOptions {
    pub max_candidates: usize,
    pub max_duration_s: f64,
    pub profile: ShortFormProfile,
    pub discovery_mode: ShortFormDiscoveryMode,
}

impl Default for ShortFormReviewOptions {
    fn default() -> Self {
        Self {
            max_candidates: DEFAULT_MAX_CANDIDATES,
            max_duration_s: DEFAULT_MAX_DURATION_S,
            profile: ShortFormProfile::EditorialReview,
            discovery_mode: ShortFormDiscoveryMode::Review,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortFormProfile {
    EditorialReview,
    ViralSocial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortFormDiscoveryMode {
    Review,
    Harvest,
    Publish,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortFormReview {
    pub asset_id: String,
    pub profile: ShortFormProfile,
    pub discovery_mode: ShortFormDiscoveryMode,
    pub source_format: SourceFormat,
    pub candidates: Vec<ShortFormCandidate>,
    pub ranked_sets: RankedSets,
    pub proposal_policy: ProposalPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFormat {
    pub width: u32,
    pub height: u32,
    pub source_aspect_ratio: String,
    pub target_aspect_ratio: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortFormCandidate {
    pub candidate_id: String,
    pub rank: usize,
    pub asset_id: String,
    pub source_range: SourceRange,
    pub duration_class: DurationClass,
    pub clip_type: String,
    pub hook: String,
    pub story_arc: StoryArc,
    pub short_form_qualification: String,
    pub cluster_id: String,
    pub variant_kind: String,
    pub overlap_ratio: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_candidate_id: Option<String>,
    pub score: ScoreBreakdown,
    pub trend_alignment: TrendAlignment,
    pub why_ai_picked_it: Vec<String>,
    pub evidence: Vec<String>,
    pub visual_decision_plan: VisualDecisionPlan,
    pub profile_plan: ProfilePlan,
    pub broll_plan: BrollPlan,
    pub vertical_layout: VerticalLayoutPlan,
    pub caption_plan: CaptionPlan,
    pub platform_variants: Vec<PlatformVariant>,
    pub suggested_title: String,
    pub suggested_caption: String,
    pub confidence: f64,
    pub review_actions: Vec<String>,
    pub review_action_contracts: Vec<ReviewActionContract>,
    pub workflow: ReviewWorkflow,
    pub draft_edl: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilePlan {
    pub profile: ShortFormProfile,
    pub pace: String,
    pub trim_policy: TrimPolicy,
    pub motion_policy: MotionPolicy,
    pub overlay_recommendations: Vec<OverlayRecommendation>,
    pub sound_cues: Vec<SoundCue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrimPolicy {
    pub pause_policy: String,
    pub filler_policy: String,
    pub repetition_policy: String,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotionPolicy {
    pub cadence_s: (f64, f64),
    pub zoom_style: String,
    pub edl_operations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayRecommendation {
    pub kind: String,
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundCue {
    pub kind: String,
    pub start_s: f64,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRange {
    pub start_s: f64,
    pub end_s: f64,
    pub duration_s: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurationClass {
    Micro,
    Standard,
    Extended,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryArc {
    pub hook: String,
    pub context: String,
    pub main_point: String,
    pub payoff: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub hook_strength: f64,
    pub standalone_clarity: f64,
    pub emotional_intensity: f64,
    pub educational_value: f64,
    pub completeness: f64,
    pub visual_potential: f64,
    pub trend_relevance: f64,
    pub retention_prediction: f64,
    pub context_dependency_penalty: f64,
    pub rambling_penalty: f64,
    pub total: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrollPlan {
    pub needed: bool,
    pub priority: String,
    pub suggestions: Vec<String>,
    pub acquisition_queries: Vec<String>,
    pub acquisition_commands: Vec<ToolCommand>,
    pub attached_assets: Vec<BrollAssetCandidate>,
    pub avoid_during: Vec<String>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrollAssetCandidate {
    pub asset_id: String,
    pub source: String,
    pub start_s: f64,
    pub end_s: f64,
    pub score: f64,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerticalLayoutPlan {
    pub target_aspect_ratio: String,
    pub composition_mode: ShortFormCompositionMode,
    pub strategy: String,
    pub speaker_strategy: String,
    pub segments: Vec<ShortFormLayoutSegment>,
    pub notes: Vec<String>,
    pub edl_operations: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortFormCompositionMode {
    NativeVertical,
    ActiveSpeakerFill,
    SplitStacked,
    DynamicSwitching,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShortFormLayoutSegment {
    pub start_s: f64,
    pub end_s: f64,
    pub layout: ShortFormLayoutMode,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortFormLayoutMode {
    NativeVertical,
    ActiveSpeakerCenter,
    SplitStacked,
    FillSpeaker { speaker_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptionPlan {
    pub group_words_min: usize,
    pub group_words_max: usize,
    pub placement: String,
    pub style: String,
    pub style_options: Vec<String>,
    pub highlight_terms: Vec<String>,
    pub groups: Vec<CaptionGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptionGroup {
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
    pub word_count: usize,
    pub highlight_terms: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformVariant {
    pub platform: String,
    pub fit: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedSets {
    pub safest: Vec<RankedSetEntry>,
    pub viral_risk: Vec<RankedSetEntry>,
    pub educational: Vec<RankedSetEntry>,
    pub funny_personality: Vec<RankedSetEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankedSetEntry {
    pub candidate_id: String,
    pub rank: usize,
    pub score: f64,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalPolicy {
    pub apply_tool: String,
    pub approval_note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewWorkflow {
    pub preview: ToolCommand,
    pub apply: ToolCommand,
    pub render_preview: ToolCommand,
    pub export: ToolCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCommand {
    pub tool: String,
    pub args: serde_json::Value,
    pub approval_required: bool,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewActionContract {
    pub action: String,
    pub kind: String,
    pub description: String,
    pub command: Option<ToolCommand>,
    pub args_schema: serde_json::Value,
}

#[derive(Debug, Clone)]
struct Moment {
    id: Option<String>,
    start_s: f64,
    end_s: f64,
    kind: String,
    text: String,
    reason: String,
    model_score: f64,
    speaker_id: Option<String>,
    cluster_id: Option<String>,
    variant_kind: String,
    primary_moment_id: Option<String>,
    overlap_ratio: f64,
    candidate_id_key: Option<String>,
}

pub fn build_short_form_review(
    input: ShortFormReviewInput,
    options: ShortFormReviewOptions,
) -> ShortFormReview {
    let max_candidates = if options.max_candidates == 0 {
        DEFAULT_MAX_CANDIDATES
    } else {
        options.max_candidates
    };
    let max_duration_s = if options.max_duration_s <= 0.0 {
        DEFAULT_MAX_DURATION_S
    } else {
        options.max_duration_s
    };

    let profile = options.profile;
    let discovery_mode = options.discovery_mode;
    let mut candidates: Vec<ShortFormCandidate> = moments_from_input(&input, discovery_mode)
        .into_iter()
        .filter(|moment| {
            let duration_s = moment.end_s - moment.start_s;
            duration_s > 4.0 && duration_s <= max_duration_s
        })
        .map(|moment| build_candidate(&input, moment, profile))
        .collect();

    candidates.sort_by(|a, b| b.score.total.total_cmp(&a.score.total));
    candidates.truncate(max_candidates);
    for (idx, candidate) in candidates.iter_mut().enumerate() {
        candidate.rank = idx + 1;
    }

    let ranked_sets = build_ranked_sets(&candidates);
    ShortFormReview {
        asset_id: input.asset_id.clone(),
        profile,
        discovery_mode,
        source_format: SourceFormat {
            width: input.source_width,
            height: input.source_height,
            source_aspect_ratio: aspect_ratio(input.source_width, input.source_height),
            target_aspect_ratio: "9:16".to_string(),
        },
        candidates,
        ranked_sets,
        proposal_policy: ProposalPolicy {
            apply_tool: "apply_edl".to_string(),
            approval_note: "Draft EDL packages are read-only proposals. Applying them must go through apply_edl, so the existing autopilot/co-pilot/manual approval behavior is preserved.".to_string(),
        },
    }
}

fn build_candidate(
    input: &ShortFormReviewInput,
    moment: Moment,
    profile: ShortFormProfile,
) -> ShortFormCandidate {
    let range = SourceRange {
        start_s: round2(moment.start_s),
        end_s: round2(moment.end_s),
        duration_s: round2(moment.end_s - moment.start_s),
    };
    let duration_class = duration_class(range.duration_s);
    let hook = hook_from_text(&moment.text);
    let topic = topic_for_range(&input.topics, range.start_s, range.end_s);
    let trend_alignment = trend_alignment(
        &input.trend_context,
        TrendMoment {
            kind: &moment.kind,
            text: &moment.text,
            reason: &moment.reason,
        },
    );
    let score = score_moment(&moment, profile, trend_alignment_score(&trend_alignment));
    let clip_type = clip_type(&moment);
    let broll_plan = plan_broll(input, &moment, topic.as_deref());
    let vertical_layout = plan_vertical_layout(input, &moment, broll_plan.needed, profile);
    let visual_decision_plan = visual_decision_plan(VisualDecisionInput {
        moment_kind: &moment.kind,
        moment_text: &moment.text,
        broll_needed: broll_plan.needed,
        broll_rationale: &broll_plan.rationale,
        speaker_strategy: &vertical_layout.speaker_strategy,
        topic_present: topic.is_some(),
    });
    let highlight_terms = highlight_terms(&moment.text, topic.as_deref());
    let profile_plan = profile_plan(profile, &moment);
    let group_words_max = if matches!(profile, ShortFormProfile::ViralSocial) {
        4
    } else {
        7
    };
    let caption_plan = CaptionPlan {
        group_words_min: 3,
        group_words_max,
        placement: "lower-middle safe area".to_string(),
        style: if matches!(profile, ShortFormProfile::ViralSocial) {
            "viral_kinetic_keyword"
        } else {
            "bold_keyword"
        }
        .to_string(),
        style_options: vec![
            "bold_keyword".to_string(),
            "karaoke".to_string(),
            "clean_lower_middle".to_string(),
        ],
        highlight_terms: highlight_terms.clone(),
        groups: caption_groups(input, &moment, &highlight_terms, group_words_max),
    };
    let suggested_title = suggested_title(&hook, topic.as_deref());
    let suggested_caption = suggested_caption(&moment.text);
    let platform_variants = platform_variants(profile, duration_class, range.duration_s);
    let story_arc = story_arc(&moment.text, &hook);
    let cluster_id = moment
        .cluster_id
        .clone()
        .unwrap_or_else(|| cluster_id_for_moment(input, &moment));
    let candidate_id = candidate_id_for_variant(
        &moment.variant_kind,
        range.start_s,
        range.end_s,
        moment.candidate_id_key.as_deref(),
    );
    let why_ai_picked_it = why_picked(&score, &clip_type, broll_plan.needed, &trend_alignment);
    let evidence = evidence(&moment, topic.as_deref(), input);
    let confidence = round2((score.total / 7.0).clamp(0.2, 0.98));
    let draft_edl = draft_edl(
        input,
        &range,
        &candidate_id,
        &suggested_title,
        &suggested_caption,
        &hook,
        &platform_variants,
        &broll_plan,
        &vertical_layout,
        &caption_plan,
        &profile_plan,
    );
    let workflow = review_workflow(&draft_edl, &input.asset_id, &range);
    let review_action_contracts = review_action_contracts(&workflow);

    ShortFormCandidate {
        candidate_id,
        rank: 0,
        asset_id: input.asset_id.clone(),
        source_range: range,
        duration_class,
        clip_type,
        hook,
        story_arc,
        short_form_qualification:
            "Complete, standalone moment suitable for a reviewable short-form draft.".to_string(),
        cluster_id,
        variant_kind: moment.variant_kind,
        overlap_ratio: moment.overlap_ratio,
        primary_candidate_id: moment.primary_moment_id,
        score,
        trend_alignment,
        why_ai_picked_it,
        evidence,
        visual_decision_plan,
        profile_plan,
        broll_plan,
        vertical_layout,
        caption_plan,
        platform_variants,
        suggested_title,
        suggested_caption,
        confidence,
        review_actions: vec![
            "approve".to_string(),
            "reject".to_string(),
            "extend_start".to_string(),
            "extend_end".to_string(),
            "change_caption_style".to_string(),
            "add_remove_broll".to_string(),
            "export".to_string(),
        ],
        review_action_contracts,
        workflow,
        draft_edl,
    }
}

fn moments_from_input(
    input: &ShortFormReviewInput,
    discovery_mode: ShortFormDiscoveryMode,
) -> Vec<Moment> {
    if matches!(discovery_mode, ShortFormDiscoveryMode::Harvest) {
        return harvest_moments_from_input(input);
    }

    let clip_candidates = array_at(&input.editorial_moments, "/clip_candidates");
    if !clip_candidates.is_empty() {
        return clip_candidates
            .into_iter()
            .filter_map(|value| moment_from_value(value, Some("clip_candidate")))
            .collect();
    }

    let moments = array_at(&input.editorial_moments, "/moments");
    if !moments.is_empty() {
        return moments
            .into_iter()
            .filter_map(|value| moment_from_value(value, None))
            .collect();
    }

    let transcript_moments: Vec<Moment> = array_at(&input.transcript, "/segments")
        .into_iter()
        .filter_map(|value| moment_from_value(value, Some("transcript_segment")))
        .collect();
    let mut candidates = topic_windows_from_transcript(input, &transcript_moments);
    candidates.extend(transcript_moments);
    candidates
}

fn moment_from_value(value: &serde_json::Value, default_kind: Option<&str>) -> Option<Moment> {
    let start_s = number_field(value, &["start_s", "start"])?;
    let end_s = number_field(value, &["end_s", "end"])?;
    let text = string_field(value, &["text", "summary"])?;
    Some(Moment {
        id: string_field(value, &["id", "moment_id", "candidate_id"]),
        start_s,
        end_s,
        kind: string_field(value, &["kind", "type"])
            .or_else(|| default_kind.map(str::to_string))
            .unwrap_or_else(|| "moment".to_string()),
        text,
        reason: string_field(value, &["reason", "rationale"]).unwrap_or_default(),
        model_score: number_field(value, &["score", "confidence"]).unwrap_or(0.5),
        speaker_id: string_field(value, &["speaker_id", "speaker"]),
        cluster_id: string_field(value, &["cluster_id"]),
        variant_kind: string_field(value, &["variant_kind"]).unwrap_or_else(|| "primary".into()),
        primary_moment_id: string_field(value, &["primary_moment_id", "primary_candidate_id"]),
        overlap_ratio: number_field(value, &["overlap_ratio"]).unwrap_or(0.0),
        candidate_id_key: None,
    })
}

fn harvest_moments_from_input(input: &ShortFormReviewInput) -> Vec<Moment> {
    let mut moments = Vec::new();
    moments.extend(
        array_at(&input.editorial_moments, "/clip_candidates")
            .into_iter()
            .filter_map(|value| moment_from_value(value, Some("clip_candidate"))),
    );
    moments.extend(
        array_at(&input.editorial_moments, "/moments")
            .into_iter()
            .filter_map(|value| moment_from_value(value, None)),
    );

    let transcript_moments: Vec<Moment> = array_at(&input.transcript, "/segments")
        .into_iter()
        .filter_map(|value| moment_from_value(value, Some("transcript_segment")))
        .collect();
    moments.extend(topic_windows_from_transcript(input, &transcript_moments));
    moments.extend(transcript_moments);

    let base_moments = dedup_moments(moments);
    let mut harvested = Vec::new();
    for moment in base_moments {
        let cluster_id = cluster_id_for_moment(input, &moment);
        let candidate_id_key = Some(candidate_id_key_for_moment(&moment));
        let primary_id = candidate_id_for_variant(
            "primary",
            moment.start_s,
            moment.end_s,
            candidate_id_key.as_deref(),
        );
        let mut primary = moment.clone();
        primary.cluster_id = Some(cluster_id.clone());
        primary.variant_kind = "primary".to_string();
        primary.overlap_ratio = 0.0;
        primary.candidate_id_key = candidate_id_key.clone();
        harvested.push(primary);
        harvested.extend(harvest_variants(
            input,
            &moment,
            &cluster_id,
            &primary_id,
            candidate_id_key.as_deref(),
        ));
    }
    dedup_moments(harvested)
}

fn dedup_moments(moments: Vec<Moment>) -> Vec<Moment> {
    let mut seen = std::collections::BTreeSet::new();
    let mut deduped = Vec::new();
    for moment in moments {
        let key = format!(
            "{}:{}:{}:{}",
            moment.kind,
            (moment.start_s * 10.0).round() as i64,
            (moment.end_s * 10.0).round() as i64,
            moment.variant_kind
        );
        if seen.insert(key) {
            deduped.push(moment);
        }
    }
    deduped
}

fn harvest_variants(
    input: &ShortFormReviewInput,
    moment: &Moment,
    cluster_id: &str,
    primary_id: &str,
    candidate_id_key: Option<&str>,
) -> Vec<Moment> {
    let duration_s = moment.end_s - moment.start_s;
    if duration_s < 50.0 {
        return Vec::new();
    }

    let mut variants = Vec::new();
    let standard_end = (moment.start_s + 60.0).min(moment.end_s);
    if standard_end - moment.start_s >= 30.0 {
        variants.push(moment_variant(
            input,
            moment,
            cluster_id,
            primary_id,
            "standard",
            moment.start_s,
            standard_end,
            candidate_id_key,
        ));
    }

    let payoff_start = (moment.end_s - 60.0).max(moment.start_s);
    if moment.end_s - payoff_start >= 30.0 && (payoff_start - moment.start_s).abs() > 1.0 {
        variants.push(moment_variant(
            input,
            moment,
            cluster_id,
            primary_id,
            "payoff",
            payoff_start,
            moment.end_s,
            candidate_id_key,
        ));
    }

    variants
}

fn moment_variant(
    input: &ShortFormReviewInput,
    moment: &Moment,
    cluster_id: &str,
    primary_id: &str,
    variant_kind: &str,
    start_s: f64,
    end_s: f64,
    candidate_id_key: Option<&str>,
) -> Moment {
    let mut variant = moment.clone();
    variant.start_s = start_s;
    variant.end_s = end_s;
    variant.text = text_for_range(input, moment, start_s, end_s);
    variant.cluster_id = Some(cluster_id.to_string());
    variant.variant_kind = variant_kind.to_string();
    variant.primary_moment_id = Some(primary_id.to_string());
    variant.overlap_ratio = overlap_ratio(moment.start_s, moment.end_s, start_s, end_s);
    variant.candidate_id_key = candidate_id_key.map(str::to_string);
    variant
}

fn candidate_id_for_variant(
    variant_kind: &str,
    start_s: f64,
    end_s: f64,
    candidate_id_key: Option<&str>,
) -> String {
    let prefix = match (variant_kind, candidate_id_key) {
        ("primary", Some(key)) => format!("short_{}", slug(key)),
        ("primary", None) => "short".to_string(),
        (variant_kind, Some(key)) => format!("short_{}_{}", variant_kind, slug(key)),
        (variant_kind, None) => format!("short_{variant_kind}"),
    };
    format!("{prefix}_{:06}_{:06}", start_s as u32, end_s as u32)
}

fn candidate_id_key_for_moment(moment: &Moment) -> String {
    moment
        .id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or(&moment.kind)
        .to_string()
}

fn text_for_range(
    input: &ShortFormReviewInput,
    fallback: &Moment,
    start_s: f64,
    end_s: f64,
) -> String {
    let text = transcript_segments_for_range(input, start_s, end_s)
        .into_iter()
        .filter_map(|segment| string_field(segment, &["text", "summary"]))
        .collect::<Vec<_>>()
        .join(" ");
    if text.trim().is_empty() {
        fallback.text.clone()
    } else {
        text
    }
}

fn overlap_ratio(parent_start_s: f64, parent_end_s: f64, start_s: f64, end_s: f64) -> f64 {
    let parent_duration = parent_end_s - parent_start_s;
    if parent_duration <= 0.0 {
        return 0.0;
    }
    let overlap_start = parent_start_s.max(start_s);
    let overlap_end = parent_end_s.min(end_s);
    round2(((overlap_end - overlap_start).max(0.0) / parent_duration).clamp(0.0, 1.0))
}

fn cluster_id_for_moment(input: &ShortFormReviewInput, moment: &Moment) -> String {
    if let Some(topic) = topic_for_range(&input.topics, moment.start_s, moment.end_s) {
        return format!("cluster:{}", slug(&topic));
    }
    format!("cluster:{:06}", moment.start_s as u32)
}

fn topic_windows_from_transcript(
    input: &ShortFormReviewInput,
    transcript_moments: &[Moment],
) -> Vec<Moment> {
    array_at(&input.topics, "/topics")
        .into_iter()
        .filter_map(|topic| {
            let topic_start = number_field(topic, &["start_s", "start"])?;
            let topic_end = number_field(topic, &["end_s", "end"])?;
            let label = string_field(topic, &["label", "topic", "title"])
                .unwrap_or_else(|| "transcript topic".to_string());
            let overlapping: Vec<&Moment> = transcript_moments
                .iter()
                .filter(|segment| segment.start_s < topic_end && segment.end_s > topic_start)
                .collect();
            if overlapping.len() < 2 {
                return None;
            }
            let start_s = overlapping
                .first()
                .map(|segment| segment.start_s)
                .unwrap_or(topic_start);
            let end_s = overlapping
                .last()
                .map(|segment| segment.end_s)
                .unwrap_or(topic_end);
            let text = overlapping
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let speaker_id = common_speaker(&overlapping);
            Some(Moment {
                id: None,
                start_s,
                end_s,
                kind: "topic_window".to_string(),
                text,
                reason: format!(
                    "assembled from {} transcript segments inside topic '{label}'",
                    overlapping.len()
                ),
                model_score: 0.62,
                speaker_id,
                cluster_id: None,
                variant_kind: "primary".to_string(),
                primary_moment_id: None,
                overlap_ratio: 0.0,
                candidate_id_key: None,
            })
        })
        .collect()
}

fn common_speaker(segments: &[&Moment]) -> Option<String> {
    let first = segments.first()?.speaker_id.as_ref()?;
    if segments
        .iter()
        .all(|segment| segment.speaker_id.as_ref() == Some(first))
    {
        Some(first.clone())
    } else {
        None
    }
}

fn score_moment(
    moment: &Moment,
    profile: ShortFormProfile,
    trend_relevance: f64,
) -> ScoreBreakdown {
    let text = moment.text.to_lowercase();
    let mut hook_strength =
        bounded(0.35 + moment.model_score * 0.35 + keyword_score(&text, HOOK_KEYWORDS) * 0.3);
    let standalone_clarity = bounded(0.35 + clarity_score(&text) - context_dependency(&text) * 0.4);
    let emotional_intensity = bounded(0.2 + keyword_score(&text, EMOTION_KEYWORDS) * 0.5);
    let educational_value = bounded(0.25 + keyword_score(&text, VALUE_KEYWORDS) * 0.55);
    let completeness = bounded(0.25 + completeness_score(moment));
    let visual_potential = bounded(0.45 + keyword_score(&text, VISUAL_KEYWORDS) * 0.4);
    let mut retention_prediction = bounded(
        hook_strength * 0.35
            + standalone_clarity * 0.2
            + completeness * 0.25
            + educational_value * 0.2,
    );
    if matches!(profile, ShortFormProfile::ViralSocial) {
        hook_strength = bounded(hook_strength + 0.12 + emotional_intensity * 0.08);
        retention_prediction =
            bounded(retention_prediction + 0.15 + hook_strength * 0.08 + visual_potential * 0.05);
    }
    let context_dependency_penalty = bounded(context_dependency(&text));
    let rambling_penalty = bounded(rambling_penalty(&text));
    let total = round2(
        hook_strength
            + standalone_clarity
            + emotional_intensity
            + educational_value
            + completeness
            + visual_potential
            + trend_relevance * 2.0
            + retention_prediction
            - context_dependency_penalty
            - rambling_penalty,
    );

    ScoreBreakdown {
        hook_strength: round2(hook_strength),
        standalone_clarity: round2(standalone_clarity),
        emotional_intensity: round2(emotional_intensity),
        educational_value: round2(educational_value),
        completeness: round2(completeness),
        visual_potential: round2(visual_potential),
        trend_relevance: round2(trend_relevance),
        retention_prediction: round2(retention_prediction),
        context_dependency_penalty: round2(context_dependency_penalty),
        rambling_penalty: round2(rambling_penalty),
        total,
    }
}

fn plan_broll(input: &ShortFormReviewInput, moment: &Moment, topic: Option<&str>) -> BrollPlan {
    let lower_kind = moment.kind.to_lowercase();
    let face_sells_it = lower_kind.contains("funny")
        || lower_kind.contains("joke")
        || lower_kind.contains("reaction")
        || lower_kind.contains("emotional")
        || lower_kind.contains("debate");
    let mut suggestions = broll_suggestions(&moment.text, topic);
    if let Some(recommendation) = broll_recommendation_for_moment(input, moment) {
        let recommendation_suggestions = recommendation_suggestions(recommendation);
        if !recommendation_suggestions.is_empty() {
            suggestions = recommendation_suggestions;
        }
        let acquisition_queries = acquisition_queries(&suggestions, topic);
        let attached_assets = attach_broll_assets(input, &suggestions, topic);
        let acquisition_commands = broll_acquisition_commands(&acquisition_queries);
        return BrollPlan {
            needed: true,
            priority: if number_field(recommendation, &["score"]).unwrap_or(0.0) >= 0.75 {
                "high"
            } else {
                "medium"
            }
            .to_string(),
            suggestions,
            acquisition_queries,
            acquisition_commands,
            attached_assets,
            avoid_during: vec![
                "strong facial reaction".to_string(),
                "emotional personal beat".to_string(),
                "guest credibility moment".to_string(),
            ],
            rationale: string_field(recommendation, &["rationale"]).unwrap_or_else(|| {
                "Fused B-roll recommendation matched this short-form candidate.".to_string()
            }),
        };
    }
    if suggestions.is_empty() && !face_sells_it {
        suggestions.push("supporting visual metaphor for the main claim".to_string());
    }
    let needed = !face_sells_it || !suggestions.is_empty();
    let acquisition_queries = acquisition_queries(&suggestions, topic);
    let attached_assets = attach_broll_assets(input, &suggestions, topic);
    let acquisition_commands = broll_acquisition_commands(&acquisition_queries);

    BrollPlan {
        needed,
        priority: if needed { "high" } else { "low" }.to_string(),
        suggestions,
        acquisition_queries,
        acquisition_commands,
        attached_assets,
        avoid_during: vec![
            "strong facial reaction".to_string(),
            "emotional personal beat".to_string(),
            "guest credibility moment".to_string(),
        ],
        rationale: if needed {
            "Most long-form-derived shorts benefit from support visuals; use B-roll where it clarifies the idea without covering the face-led beat.".to_string()
        } else {
            "Keep the speaker visible because the face/reaction is the main value.".to_string()
        },
    }
}

fn acquisition_queries(suggestions: &[String], topic: Option<&str>) -> Vec<String> {
    let mut queries: Vec<String> = suggestions
        .iter()
        .map(|suggestion| {
            suggestion
                .split([',', ':'])
                .next()
                .unwrap_or(suggestion)
                .trim()
                .to_string()
        })
        .filter(|query| !query.is_empty())
        .collect();
    if let Some(topic) = topic
        && !topic.trim().is_empty()
    {
        queries.push(topic.to_string());
    }
    queries.sort();
    queries.dedup();
    queries.truncate(5);
    queries
}

fn attach_broll_assets(
    input: &ShortFormReviewInput,
    suggestions: &[String],
    topic: Option<&str>,
) -> Vec<BrollAssetCandidate> {
    let terms = broll_match_terms(suggestions, topic);
    let mut assets: Vec<BrollAssetCandidate> = array_at(&input.broll_assets, "/candidates")
        .into_iter()
        .filter_map(|candidate| broll_asset_from_value(candidate, &terms))
        .collect();
    assets.sort_by(|a, b| b.score.total_cmp(&a.score));
    assets.truncate(3);
    assets
}

fn broll_recommendation_for_moment<'a>(
    input: &'a ShortFormReviewInput,
    moment: &Moment,
) -> Option<&'a serde_json::Value> {
    let recommendations = array_at(&input.broll_assets, "/recommendations");
    if recommendations.is_empty() {
        return None;
    }
    if let Some(moment_id) = moment.id.as_deref()
        && let Some(recommendation) = recommendations.iter().copied().find(|recommendation| {
            string_field(recommendation, &["moment_id"])
                .as_deref()
                .is_some_and(|id| id == moment_id)
        })
    {
        return Some(recommendation);
    }
    recommendations
        .into_iter()
        .filter(|recommendation| {
            let start_s = number_field(recommendation, &["start_s", "start"]).unwrap_or(f64::NAN);
            let end_s = number_field(recommendation, &["end_s", "end"]).unwrap_or(f64::NAN);
            start_s.is_finite()
                && end_s.is_finite()
                && start_s < moment.end_s
                && end_s > moment.start_s
        })
        .max_by(|a, b| {
            number_field(a, &["score"])
                .unwrap_or(0.0)
                .total_cmp(&number_field(b, &["score"]).unwrap_or(0.0))
        })
}

fn recommendation_suggestions(recommendation: &serde_json::Value) -> Vec<String> {
    let mut suggestions = Vec::new();
    let category = string_field(recommendation, &["category"]).unwrap_or_default();
    let strategy = string_field(recommendation, &["strategy"]).unwrap_or_default();
    let rationale = string_field(recommendation, &["rationale"]).unwrap_or_default();
    match category.as_str() {
        "statistic" => suggestions.push("animated statistic callout or infographic".to_string()),
        "technical_concept" => {
            suggestions.push("motion graphic explainer for the technical concept".to_string())
        }
        "product_mention" => {
            suggestions.push("product UI, app screen, or workflow capture".to_string())
        }
        "entity_mention" | "historical_reference" => {
            suggestions
                .push("archival/reference visual matched to the mentioned entity".to_string());
        }
        "emotional_moment" | "storytelling" => {
            suggestions
                .push("subtle story-support cutaway that does not cover the reaction".to_string());
        }
        "analogy" => suggestions.push("generated visual metaphor for the analogy".to_string()),
        _ => {}
    }
    if suggestions.is_empty() && !strategy.trim().is_empty() {
        suggestions.push(format!("{strategy} support visual"));
    }
    if suggestions.is_empty() && !rationale.trim().is_empty() {
        suggestions.push(rationale);
    }
    suggestions.sort();
    suggestions.dedup();
    suggestions
}

fn broll_acquisition_commands(queries: &[String]) -> Vec<ToolCommand> {
    let query = queries
        .first()
        .cloned()
        .unwrap_or_else(|| "supporting vertical B-roll".to_string());
    vec![
        ToolCommand {
            tool: "search_broll".to_string(),
            args: serde_json::json!({
                "query": query,
                "per_page": 5
            }),
            approval_required: false,
            description: "Search external stock B-roll candidates before download/use.".to_string(),
        },
        ToolCommand {
            tool: "find_generated_broll_opportunities".to_string(),
            args: serde_json::json!({
                "duration_s": 4.0,
                "max_results": 12
            }),
            approval_required: false,
            description: "Find moments suitable for generated B-roll support.".to_string(),
        },
    ]
}

fn broll_match_terms(suggestions: &[String], topic: Option<&str>) -> Vec<String> {
    let mut terms = Vec::new();
    for text in suggestions {
        for word in text.split(|c: char| !c.is_alphanumeric()) {
            let lower = word.to_lowercase();
            if lower.len() >= 3 {
                terms.push(lower);
            }
        }
    }
    if let Some(topic) = topic {
        for word in topic.split(|c: char| !c.is_alphanumeric()) {
            let lower = word.to_lowercase();
            if lower.len() >= 3 {
                terms.push(lower);
            }
        }
    }
    terms.sort();
    terms.dedup();
    terms
}

fn broll_asset_from_value(
    value: &serde_json::Value,
    terms: &[String],
) -> Option<BrollAssetCandidate> {
    let asset_id = string_field(value, &["asset_id", "asset"])?;
    let query_terms: Vec<String> = value
        .get("query_terms")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_lowercase()))
                .collect()
        })
        .unwrap_or_default();
    if !terms.is_empty()
        && !query_terms.is_empty()
        && !query_terms.iter().any(|term| {
            terms
                .iter()
                .any(|wanted| term.contains(wanted) || wanted.contains(term))
        })
    {
        return None;
    }
    let start_s = number_field(value, &["start_s", "start"]).unwrap_or(0.0);
    let end_s = number_field(value, &["end_s", "end"]).unwrap_or(start_s + 4.0);
    let score = number_field(value, &["score", "confidence"]).unwrap_or(0.5);
    let source =
        string_field(value, &["source", "kind"]).unwrap_or_else(|| "existing_asset".into());
    Some(BrollAssetCandidate {
        asset_id,
        source,
        start_s: round2(start_s),
        end_s: round2(end_s),
        score: round2(score),
        rationale: "matched B-roll candidate terms to the transcript/topic support plan"
            .to_string(),
    })
}

fn plan_vertical_layout(
    input: &ShortFormReviewInput,
    moment: &Moment,
    broll_needed: bool,
    profile: ShortFormProfile,
) -> VerticalLayoutPlan {
    let speakers = input
        .transcript
        .get("speakers")
        .and_then(|v| v.as_array())
        .map(Vec::len)
        .unwrap_or_else(|| if moment.speaker_id.is_some() { 1 } else { 0 });
    let wide_source = input.source_width > input.source_height;
    let face_count = max_face_count(&input.face);
    let scene_count = overlapping_scene_count(&input.scenes, moment.start_s, moment.end_s);
    let shot_hint = shot_hint(&input.shot, moment.start_s, moment.end_s);
    let multi_speaker = speakers > 1 || face_count > 1;
    let composition_mode = composition_mode(input, moment, multi_speaker, wide_source);
    let segments = layout_segments(input, moment, composition_mode);
    let strategy = if multi_speaker {
        "dynamic speaker-aware 9:16 focus with split/stacked fallback"
    } else if broll_needed {
        "speaker crop with B-roll-first support windows"
    } else if wide_source {
        "active speaker center crop"
    } else {
        "native vertical speaker framing"
    };

    VerticalLayoutPlan {
        target_aspect_ratio: "9:16".to_string(),
        composition_mode,
        strategy: strategy.to_string(),
        speaker_strategy: if multi_speaker {
            "Track the active speaker; use stacked/split layout when both speakers matter, then punch in on the current speaker during monologue stretches.".to_string()
        } else {
            "Keep the visible speaker in the safe center crop.".to_string()
        },
        segments,
        notes: layout_notes(scene_count, shot_hint.as_deref()),
        edl_operations: reframe_edl_operations(moment, face_count, profile, composition_mode),
    }
}

fn composition_mode(
    input: &ShortFormReviewInput,
    moment: &Moment,
    multi_speaker: bool,
    wide_source: bool,
) -> ShortFormCompositionMode {
    if !wide_source {
        return ShortFormCompositionMode::NativeVertical;
    }
    if !multi_speaker {
        return ShortFormCompositionMode::ActiveSpeakerFill;
    }
    let active_speakers = active_speakers_for_range(input, moment.start_s, moment.end_s);
    if active_speakers.len() > 1 {
        ShortFormCompositionMode::DynamicSwitching
    } else if active_speakers.is_empty() {
        ShortFormCompositionMode::SplitStacked
    } else {
        ShortFormCompositionMode::ActiveSpeakerFill
    }
}

fn layout_segments(
    input: &ShortFormReviewInput,
    moment: &Moment,
    composition_mode: ShortFormCompositionMode,
) -> Vec<ShortFormLayoutSegment> {
    match composition_mode {
        ShortFormCompositionMode::NativeVertical => vec![ShortFormLayoutSegment {
            start_s: moment.start_s,
            end_s: moment.end_s,
            layout: ShortFormLayoutMode::NativeVertical,
            reason: "source is already vertical; preserve native framing".to_string(),
        }],
        ShortFormCompositionMode::ActiveSpeakerFill => {
            let active_speakers = active_speakers_for_range(input, moment.start_s, moment.end_s);
            let speaker_id = moment.speaker_id.as_deref().or_else(|| {
                if active_speakers.len() == 1 {
                    Some(active_speakers[0].as_str())
                } else {
                    None
                }
            });
            vec![fill_segment(moment.start_s, moment.end_s, speaker_id)]
        }
        ShortFormCompositionMode::SplitStacked => vec![ShortFormLayoutSegment {
            start_s: moment.start_s,
            end_s: moment.end_s,
            layout: ShortFormLayoutMode::SplitStacked,
            reason: "multiple speakers/faces stay valuable throughout the clip".to_string(),
        }],
        ShortFormCompositionMode::DynamicSwitching => dynamic_layout_segments(input, moment),
    }
}

fn dynamic_layout_segments(
    input: &ShortFormReviewInput,
    moment: &Moment,
) -> Vec<ShortFormLayoutSegment> {
    const MIN_FILL_S: f64 = 8.0;
    const MIN_LAYOUT_S: f64 = 3.0;
    const MAX_SAME_SPEAKER_GAP_S: f64 = 2.0;
    const DOMINANT_FILL_RATIO: f64 = 0.8;

    let turns =
        speaker_turns_for_range(input, moment.start_s, moment.end_s, MAX_SAME_SPEAKER_GAP_S);
    if turns.is_empty() {
        return vec![ShortFormLayoutSegment {
            start_s: moment.start_s,
            end_s: moment.end_s,
            layout: ShortFormLayoutMode::SplitStacked,
            reason: "speaker timing unavailable; keep both speakers visible".to_string(),
        }];
    }

    if let Some(dominant) = dominant_speaker(&turns, DOMINANT_FILL_RATIO) {
        return vec![ShortFormLayoutSegment {
            start_s: moment.start_s,
            end_s: moment.end_s,
            layout: ShortFormLayoutMode::FillSpeaker {
                speaker_id: dominant,
            },
            reason: "one speaker owns at least 80 percent of spoken time; fill that speaker"
                .to_string(),
        }];
    }

    let mut segments = Vec::new();
    let mut cursor_s = moment.start_s;
    for turn in turns {
        if turn.start_s > cursor_s {
            append_layout_segment(
                &mut segments,
                split_segment(
                    cursor_s,
                    turn.start_s,
                    "non-speech gap keeps both speakers visible",
                ),
                MIN_LAYOUT_S,
            );
        }
        let duration = turn.end_s - turn.start_s;
        let segment = if duration >= MIN_FILL_S {
            fill_segment(turn.start_s, turn.end_s, Some(&turn.speaker_id))
        } else {
            ShortFormLayoutSegment {
                start_s: round2(turn.start_s),
                end_s: round2(turn.end_s),
                layout: ShortFormLayoutMode::SplitStacked,
                reason: "dialogue or short turn keeps both speakers visible".to_string(),
            }
        };
        append_layout_segment(&mut segments, segment, MIN_LAYOUT_S);
        cursor_s = turn.end_s;
    }
    if cursor_s < moment.end_s {
        append_layout_segment(
            &mut segments,
            split_segment(
                cursor_s,
                moment.end_s,
                "tail gap keeps both speakers visible",
            ),
            MIN_LAYOUT_S,
        );
    }
    segments
}

#[derive(Debug, Clone)]
struct SpeakerTurn {
    start_s: f64,
    end_s: f64,
    speaker_id: String,
}

fn speaker_turns_for_range(
    input: &ShortFormReviewInput,
    start_s: f64,
    end_s: f64,
    max_same_speaker_gap_s: f64,
) -> Vec<SpeakerTurn> {
    let mut turns: Vec<SpeakerTurn> = Vec::new();
    for segment in transcript_segments_for_range(input, start_s, end_s) {
        let Some(speaker_id) = string_field(segment, &["speaker_id", "speaker"]) else {
            continue;
        };
        if speaker_id.trim().is_empty() {
            continue;
        }
        let segment_start = number_field(segment, &["start_s", "start"])
            .unwrap_or(start_s)
            .max(start_s);
        let segment_end = number_field(segment, &["end_s", "end"])
            .unwrap_or(segment_start)
            .min(end_s);
        if segment_end <= segment_start {
            continue;
        }
        if let Some(previous) = turns.last_mut()
            && previous.speaker_id == speaker_id
            && segment_start - previous.end_s <= max_same_speaker_gap_s
        {
            previous.end_s = previous.end_s.max(segment_end);
            continue;
        }
        turns.push(SpeakerTurn {
            start_s: segment_start,
            end_s: segment_end,
            speaker_id,
        });
    }
    turns
}

fn dominant_speaker(turns: &[SpeakerTurn], threshold: f64) -> Option<String> {
    let mut durations = std::collections::BTreeMap::<&str, f64>::new();
    for turn in turns {
        *durations.entry(turn.speaker_id.as_str()).or_default() += turn.end_s - turn.start_s;
    }
    let total: f64 = durations.values().sum();
    if total <= 0.0 {
        return None;
    }
    durations
        .into_iter()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .and_then(|(speaker, duration)| {
            if duration / total >= threshold {
                Some(speaker.to_string())
            } else {
                None
            }
        })
}

fn append_layout_segment(
    segments: &mut Vec<ShortFormLayoutSegment>,
    segment: ShortFormLayoutSegment,
    min_layout_s: f64,
) {
    let Some(previous) = segments.last_mut() else {
        segments.push(segment);
        return;
    };
    if previous.layout == segment.layout {
        previous.end_s = segment.end_s;
        return;
    }
    if segment.end_s - segment.start_s < min_layout_s {
        previous.end_s = segment.end_s;
        previous.reason.push_str("; absorbed short layout switch");
        return;
    }
    segments.push(segment);
}

fn fill_segment(start_s: f64, end_s: f64, speaker_id: Option<&str>) -> ShortFormLayoutSegment {
    ShortFormLayoutSegment {
        start_s: round2(start_s),
        end_s: round2(end_s),
        layout: speaker_id
            .filter(|speaker| !speaker.trim().is_empty())
            .map(|speaker| ShortFormLayoutMode::FillSpeaker {
                speaker_id: speaker.to_string(),
            })
            .unwrap_or(ShortFormLayoutMode::ActiveSpeakerCenter),
        reason: "one speaker carries this beat; fill the vertical frame with that speaker"
            .to_string(),
    }
}

fn split_segment(start_s: f64, end_s: f64, reason: &str) -> ShortFormLayoutSegment {
    ShortFormLayoutSegment {
        start_s: round2(start_s),
        end_s: round2(end_s),
        layout: ShortFormLayoutMode::SplitStacked,
        reason: reason.to_string(),
    }
}

fn active_speakers_for_range(
    input: &ShortFormReviewInput,
    start_s: f64,
    end_s: f64,
) -> Vec<String> {
    let mut speakers: Vec<String> = transcript_segments_for_range(input, start_s, end_s)
        .into_iter()
        .filter_map(|segment| string_field(segment, &["speaker_id", "speaker"]))
        .filter(|speaker| !speaker.trim().is_empty())
        .collect();
    speakers.sort();
    speakers.dedup();
    speakers
}

fn transcript_segments_for_range(
    input: &ShortFormReviewInput,
    start_s: f64,
    end_s: f64,
) -> Vec<&serde_json::Value> {
    array_at(&input.transcript, "/segments")
        .into_iter()
        .filter(|segment| {
            let segment_start = number_field(segment, &["start_s", "start"]).unwrap_or(f64::MAX);
            let segment_end = number_field(segment, &["end_s", "end"]).unwrap_or(f64::MIN);
            segment_start < end_s && segment_end > start_s
        })
        .collect()
}

fn layout_notes(scene_count: usize, shot_hint: Option<&str>) -> Vec<String> {
    let mut notes = vec!["Keep captions lower-middle and clear of faces.".to_string()];
    if scene_count > 0 {
        notes.push(format!(
            "Use {scene_count} source scene/shot boundaries to time reframe and B-roll transitions."
        ));
    } else {
        notes.push(
            "No scene boundaries were available; keep reframe motion conservative.".to_string(),
        );
    }
    if let Some(shot_hint) = shot_hint {
        notes.push(format!("Detected shot context: {shot_hint}."));
    }
    notes
}

fn reframe_edl_operations(
    moment: &Moment,
    face_count: usize,
    profile: ShortFormProfile,
    composition_mode: ShortFormCompositionMode,
) -> Vec<String> {
    let zoom = if face_count > 1 { 1.28 } else { 1.18 };
    let x = if face_count > 1 { 0.0 } else { -0.04 };
    let params = serde_json::json!({
        "aspect_ratio": "9:16",
        "mode": composition_mode,
        "legacy_mode": if face_count > 1 { "dynamic_speaker_focus" } else { "active_speaker_center" },
        "zoom": zoom,
        "x": x,
        "y": 0.0,
        "safe_area": "mobile"
    });
    let mut ops = vec![format!(
        "*** Set Effect\n\
@@ anchor: transcript_snippet=\"{anchor}\"\n\
+ effect: montage.reframe\n\
+ params_json: {params}\n\
+ rationale: speaker-aware 9:16 review reframe\n",
        anchor = edl_string(&hook_from_text(&moment.text)),
    )];
    if matches!(profile, ShortFormProfile::ViralSocial) {
        let viral_params = serde_json::json!({
            "mode": "viral_punch_in",
            "cadence_s": [1.0, 3.0],
            "zoom_sequence": [1.0, 1.18, 1.32],
            "safe_area": "mobile"
        });
        ops.push(format!(
            "*** Set Effect\n\
@@ anchor: transcript_snippet=\"{anchor}\"\n\
+ effect: montage.reframe\n\
+ params_json: {viral_params}\n\
+ rationale: viral_punch_in cadence every 1-3 seconds\n",
            anchor = edl_string(&hook_from_text(&moment.text)),
        ));
    }
    ops
}

fn profile_plan(profile: ShortFormProfile, moment: &Moment) -> ProfilePlan {
    match profile {
        ShortFormProfile::EditorialReview => ProfilePlan {
            profile,
            pace: "natural".to_string(),
            trim_policy: TrimPolicy {
                pause_policy: "balanced".to_string(),
                filler_policy: "light".to_string(),
                repetition_policy: "preserve_meaning".to_string(),
                targets: vec![
                    "trim_dead_air".to_string(),
                    "remove_obvious_restarts".to_string(),
                ],
            },
            motion_policy: MotionPolicy {
                cadence_s: (6.0, 12.0),
                zoom_style: "speaker_safe".to_string(),
                edl_operations: Vec::new(),
            },
            overlay_recommendations: Vec::new(),
            sound_cues: Vec::new(),
        },
        ShortFormProfile::ViralSocial => {
            let hook = hook_from_text(&moment.text);
            ProfilePlan {
                profile,
                pace: "aggressive".to_string(),
                trim_policy: TrimPolicy {
                    pause_policy: "aggressive".to_string(),
                    filler_policy: "remove".to_string(),
                    repetition_policy: "compress".to_string(),
                    targets: vec![
                        "remove_pauses_over_250ms".to_string(),
                        "remove_filler_words".to_string(),
                        "remove_repeated_phrases".to_string(),
                        "tighten_to_hook_payoff".to_string(),
                    ],
                },
                motion_policy: MotionPolicy {
                    cadence_s: (1.0, 3.0),
                    zoom_style: "rapid_punch_in".to_string(),
                    edl_operations: vec![format!(
                        "viral_punch_in: punch or zoom every 1-3s around '{}'",
                        hook
                    )],
                },
                overlay_recommendations: vec![OverlayRecommendation {
                    kind: "meme_overlay".to_string(),
                    start_s: 0.6,
                    end_s: 2.2,
                    text: "quick reaction or screenshot overlay for the hook".to_string(),
                    rationale: "viral_social uses brief overlays to reset attention early."
                        .to_string(),
                }],
                sound_cues: vec![
                    SoundCue {
                        kind: "whoosh".to_string(),
                        start_s: 1.0,
                        rationale: "accent the first punch-in without covering speech.".to_string(),
                    },
                    SoundCue {
                        kind: "subtle_pop".to_string(),
                        start_s: 2.5,
                        rationale: "reinforce highlighted keyword caption beats.".to_string(),
                    },
                ],
            }
        }
    }
}

fn caption_groups(
    input: &ShortFormReviewInput,
    moment: &Moment,
    highlight_terms: &[String],
    group_words_max: usize,
) -> Vec<CaptionGroup> {
    let words = timed_words_for_range(input, moment.start_s, moment.end_s, &moment.text);
    let mut groups = Vec::new();
    let mut idx = 0;
    while idx < words.len() {
        let remaining = words.len() - idx;
        if remaining < 3 {
            break;
        }
        let mut take = remaining.min(group_words_max);
        let tail = remaining - take;
        if (1..3).contains(&tail) && take > 3 {
            take -= 3 - tail;
        }
        let chunk = &words[idx..idx + take];
        let text = chunk
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let group_highlights = highlight_terms
            .iter()
            .filter(|term| text.to_lowercase().contains(&term.to_lowercase()))
            .cloned()
            .collect();
        groups.push(CaptionGroup {
            start_s: round2(chunk.first().map(|word| word.start_s).unwrap_or(0.0) - moment.start_s),
            end_s: round2(
                chunk.last().map(|word| word.end_s).unwrap_or(moment.end_s) - moment.start_s,
            ),
            text,
            word_count: chunk.len(),
            highlight_terms: group_highlights,
        });
        idx += take;
        if groups.len() >= 12 {
            break;
        }
    }
    groups
}

#[derive(Debug, Clone)]
struct TimedWord {
    start_s: f64,
    end_s: f64,
    text: String,
}

fn timed_words_for_range(
    input: &ShortFormReviewInput,
    start_s: f64,
    end_s: f64,
    fallback_text: &str,
) -> Vec<TimedWord> {
    let mut words: Vec<TimedWord> = array_at(&input.transcript, "/segments")
        .into_iter()
        .filter(|segment| {
            let segment_start = number_field(segment, &["start_s", "start"]).unwrap_or(f64::MAX);
            let segment_end = number_field(segment, &["end_s", "end"]).unwrap_or(f64::MIN);
            segment_start < end_s && segment_end > start_s
        })
        .flat_map(|segment| array_at(segment, "/words"))
        .filter_map(|word| {
            let word_start = number_field(word, &["start_s", "start"])?;
            let word_end = number_field(word, &["end_s", "end"])?;
            if word_start < start_s || word_end > end_s {
                return None;
            }
            Some(TimedWord {
                start_s: word_start,
                end_s: word_end,
                text: string_field(word, &["text", "word"]).unwrap_or_default(),
            })
        })
        .filter(|word| !word.text.trim().is_empty())
        .collect();
    if !words.is_empty() {
        words.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
        return words;
    }
    synthetic_words(start_s, fallback_text)
}

fn synthetic_words(start_s: f64, text: &str) -> Vec<TimedWord> {
    text.split_whitespace()
        .enumerate()
        .map(|(idx, raw)| {
            let start = start_s + idx as f64 * 0.32;
            TimedWord {
                start_s: round2(start),
                end_s: round2(start + 0.28),
                text: raw.trim_matches(|c: char| c == '"' || c == ',').to_string(),
            }
        })
        .filter(|word| !word.text.is_empty())
        .collect()
}

fn draft_edl(
    input: &ShortFormReviewInput,
    range: &SourceRange,
    candidate_id: &str,
    title: &str,
    caption: &str,
    hook: &str,
    platforms: &[PlatformVariant],
    broll_plan: &BrollPlan,
    vertical_layout: &VerticalLayoutPlan,
    caption_plan: &CaptionPlan,
    profile_plan: &ProfilePlan,
) -> String {
    let platform = platforms
        .first()
        .map(|variant| variant.platform.as_str())
        .unwrap_or("youtube_shorts");
    let mut edl = format!(
        "*** Begin EDL\n\
*** Set Output Format\n\
+ aspect_ratio: 9:16\n\
+ platform: {platform}\n\
+ safe_area: mobile\n\
*** Insert Clip\n\
+ asset: {asset}\n\
+ track: V1\n\
+ at_s: 0\n\
+ start: {start}\n\
+ end: {end}\n\
+ name: {name}\n\
{reframe_ops}\
{profile_ops}\
{broll_ops}\
{caption_ops}\
*** Set Loudness Target\n\
+ integrated_lufs: -14\n\
+ true_peak_db: -1\n\
*** Set Package Metadata\n\
+ platform: {platform}\n\
+ title: \"{title}\"\n\
+ description: \"{caption}\"\n\
+ tags: short-form,broll,review\n\
",
        asset = input.asset_id,
        start = range.start_s,
        end = range.end_s,
        name = candidate_id,
        reframe_ops = vertical_layout.edl_operations.join(""),
        profile_ops = profile_edl_operations(profile_plan),
        broll_ops = broll_edl_operations(broll_plan, hook),
        caption_ops = caption_edl_operations(caption_plan),
        title = edl_string(title),
        caption = edl_string(caption),
    );
    edl.push_str("*** End EDL\n");
    edl
}

fn review_workflow(draft_edl: &str, asset_id: &str, range: &SourceRange) -> ReviewWorkflow {
    ReviewWorkflow {
        preview: ToolCommand {
            tool: "apply_edl".to_string(),
            args: serde_json::json!({
                "edl": draft_edl,
                "dry_run": true,
                "reasoning": "Preview the short-form review candidate without committing."
            }),
            approval_required: false,
            description: "Validate and preview the EDL as a dry run before committing.".to_string(),
        },
        apply: ToolCommand {
            tool: "apply_edl".to_string(),
            args: serde_json::json!({
                "edl": draft_edl,
                "dry_run": false,
                "reasoning": "Apply the approved short-form review candidate."
            }),
            approval_required: true,
            description: "Apply the approved edit through the normal mutation gate.".to_string(),
        },
        render_preview: ToolCommand {
            tool: "start_render".to_string(),
            args: serde_json::json!({
                "scope": "segment",
                "asset": asset_id,
                "range": {
                    "start_s": range.start_s,
                    "end_s": range.end_s
                }
            }),
            approval_required: true,
            description: "Render a temporary source-range preview before committing the EDL."
                .to_string(),
        },
        export: ToolCommand {
            tool: "export_package".to_string(),
            args: serde_json::json!({"format": "shorts"}),
            approval_required: true,
            description: "Export the approved vertical short through the export gate.".to_string(),
        },
    }
}

fn review_action_contracts(workflow: &ReviewWorkflow) -> Vec<ReviewActionContract> {
    vec![
        ReviewActionContract {
            action: "approve".to_string(),
            kind: "tool_command".to_string(),
            description: "Apply the current draft EDL through the existing approval gate."
                .to_string(),
            command: Some(workflow.apply.clone()),
            args_schema: serde_json::json!({"type": "object", "required": []}),
        },
        ReviewActionContract {
            action: "reject".to_string(),
            kind: "local_review_state".to_string(),
            description: "Mark this candidate rejected without mutating the timeline.".to_string(),
            command: None,
            args_schema: serde_json::json!({"type": "object", "required": []}),
        },
        ReviewActionContract {
            action: "extend_start".to_string(),
            kind: "draft_adjustment".to_string(),
            description: "Adjust the draft source start earlier before preview/apply.".to_string(),
            command: None,
            args_schema: serde_json::json!({
                "type": "object",
                "properties": {"seconds": {"type": "number", "minimum": 0.0}},
                "required": ["seconds"]
            }),
        },
        ReviewActionContract {
            action: "extend_end".to_string(),
            kind: "draft_adjustment".to_string(),
            description: "Adjust the draft source end later before preview/apply.".to_string(),
            command: None,
            args_schema: serde_json::json!({
                "type": "object",
                "properties": {"seconds": {"type": "number", "minimum": 0.0}},
                "required": ["seconds"]
            }),
        },
        ReviewActionContract {
            action: "change_caption_style".to_string(),
            kind: "draft_adjustment".to_string(),
            description: "Switch caption style and regenerate caption EDL groups.".to_string(),
            command: None,
            args_schema: serde_json::json!({
                "type": "object",
                "properties": {"style": {"type": "string"}},
                "required": ["style"]
            }),
        },
        ReviewActionContract {
            action: "add_remove_broll".to_string(),
            kind: "draft_adjustment".to_string(),
            description: "Attach, remove, or replace B-roll before preview/apply.".to_string(),
            command: None,
            args_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "asset_id": {"type": "string"},
                    "operation": {"type": "string", "enum": ["add", "remove", "replace"]}
                },
                "required": ["operation"]
            }),
        },
        ReviewActionContract {
            action: "export".to_string(),
            kind: "tool_command".to_string(),
            description: "Export the approved short through the existing export gate.".to_string(),
            command: Some(workflow.export.clone()),
            args_schema: serde_json::json!({"type": "object", "required": []}),
        },
    ]
}

fn profile_edl_operations(plan: &ProfilePlan) -> String {
    if !matches!(plan.profile, ShortFormProfile::ViralSocial) {
        return String::new();
    }
    plan.overlay_recommendations
        .iter()
        .map(|overlay| {
            format!(
                "*** Insert Title\n\
+ start_s: {start}\n\
+ end_s: {end}\n\
+ text: \"{text}\"\n\
+ position: top\n",
                start = overlay.start_s,
                end = overlay.end_s,
                text = edl_string(&overlay.text),
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn broll_edl_operations(plan: &BrollPlan, hook: &str) -> String {
    plan.attached_assets
        .first()
        .map(|asset| {
            format!(
                "*** Insert BRoll\n\
@@ anchor: transcript_snippet=\"{anchor}\"\n\
+ asset: {asset_id}\n\
+ duration_s: {duration}\n\
+ position: overlay\n",
                anchor = edl_string(hook),
                asset_id = asset.asset_id,
                duration = round2((asset.end_s - asset.start_s).clamp(1.0, 6.0)),
            )
        })
        .unwrap_or_default()
}

fn caption_edl_operations(plan: &CaptionPlan) -> String {
    plan.groups
        .iter()
        .map(|group| {
            format!(
                "*** Insert Caption\n\
+ start_s: {start}\n\
+ end_s: {end}\n\
+ text: \"{text}\"\n\
+ position: center\n\
+ safe_area: mobile\n",
                start = group.start_s,
                end = group.end_s,
                text = edl_string(&group.text),
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn build_ranked_sets(candidates: &[ShortFormCandidate]) -> RankedSets {
    let top = |predicate: fn(&ShortFormCandidate) -> bool, rationale: &str| {
        candidates
            .iter()
            .filter(|candidate| predicate(candidate))
            .take(5)
            .map(|candidate| RankedSetEntry {
                candidate_id: candidate.candidate_id.clone(),
                rank: candidate.rank,
                score: candidate.score.total,
                rationale: rationale.to_string(),
            })
            .collect()
    };

    RankedSets {
        safest: top(
            |candidate| {
                candidate.score.context_dependency_penalty < 0.4
                    && candidate.score.completeness > 0.5
            },
            "safe pick: complete, standalone, and low context dependency",
        ),
        viral_risk: top(
            |candidate| candidate.score.hook_strength > 0.55 || candidate.clip_type == "hot_take",
            "viral-risk pick: strong hook or hot-take framing",
        ),
        educational: top(
            |candidate| candidate.score.educational_value > 0.45,
            "educational pick: explains a lesson, framework, or causal insight",
        ),
        funny_personality: top(
            |candidate| {
                candidate.clip_type == "funny_personality"
                    || candidate.score.emotional_intensity > 0.55
            },
            "funny/personality pick: emotion, humor, or reaction-led value",
        ),
    }
}

fn why_picked(
    score: &ScoreBreakdown,
    clip_type: &str,
    broll_needed: bool,
    trend_alignment: &TrendAlignment,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if score.hook_strength > 0.5 {
        reasons.push("strong hook or claim in the opening language".to_string());
    }
    if score.standalone_clarity > 0.5 {
        reasons.push("standalone enough to understand without the full episode".to_string());
    }
    if score.completeness > 0.5 {
        reasons.push("has a beginning, middle, and payoff shape".to_string());
    }
    if broll_needed {
        reasons.push("clear B-roll or graphic support opportunities".to_string());
    }
    if trend_alignment.matched {
        reasons.push("matches supplied trend context for this episode topic".to_string());
    }
    reasons.push(format!("classified as {clip_type}"));
    reasons
}

fn evidence(moment: &Moment, topic: Option<&str>, input: &ShortFormReviewInput) -> Vec<String> {
    let mut evidence = vec![
        format!("editorial moment kind: {}", moment.kind),
        format!("source range: {:.2}-{:.2}s", moment.start_s, moment.end_s),
    ];
    if let Some(id) = &moment.id {
        evidence.push(format!("stable intelligence id: {id}"));
    }
    if !moment.reason.is_empty() {
        evidence.push(format!("moment reason: {}", moment.reason));
    }
    if let Some(topic) = topic {
        evidence.push(format!("topic: {topic}"));
    }
    if max_face_count(&input.face) > 1 {
        evidence.push("visual evidence includes multiple faces".to_string());
    }
    evidence
}

fn platform_variants(
    profile: ShortFormProfile,
    duration_class: DurationClass,
    duration_s: f64,
) -> Vec<PlatformVariant> {
    if matches!(profile, ShortFormProfile::ViralSocial) {
        return vec![
            variant(
                "tiktok",
                "strong",
                "fast hook, kinetic captions, punch-in cadence",
            ),
            variant(
                "instagram_reels",
                "strong",
                "cleaner viral cut with bold captions",
            ),
            variant(
                "youtube_shorts",
                "medium",
                "tighten for a complete idea under Shorts norms",
            ),
        ];
    }
    match duration_class {
        DurationClass::Micro => vec![
            variant("youtube_shorts", "strong", "fits a compact complete idea"),
            variant("tiktok", "strong", "fast hook and caption-forward"),
            variant("instagram_reels", "strong", "clean polish pass recommended"),
        ],
        DurationClass::Standard => vec![
            variant(
                "youtube_shorts",
                "strong",
                "complete short under the usual cap",
            ),
            variant("instagram_reels", "strong", "works with polished captions"),
            variant("tiktok", "strong", "needs an aggressive opening caption"),
        ],
        DurationClass::Extended => vec![
            variant(
                "youtube",
                "strong",
                "extended short-form idea; publish as short vertical video or clip",
            ),
            variant(
                "tiktok",
                "medium",
                "works if segmented with B-roll and captions",
            ),
            variant(
                "instagram_reels",
                if duration_s <= 180.0 { "medium" } else { "low" },
                "consider a tighter platform cut if needed",
            ),
        ],
    }
}

fn variant(platform: &str, fit: &str, note: &str) -> PlatformVariant {
    PlatformVariant {
        platform: platform.to_string(),
        fit: fit.to_string(),
        note: note.to_string(),
    }
}

fn story_arc(text: &str, hook: &str) -> StoryArc {
    let sentences = sentences(text);
    StoryArc {
        hook: hook.to_string(),
        context: sentences
            .get(1)
            .cloned()
            .unwrap_or_else(|| hook.to_string()),
        main_point: sentences
            .get(2)
            .cloned()
            .unwrap_or_else(|| sentences.first().cloned().unwrap_or_default()),
        payoff: sentences
            .last()
            .cloned()
            .unwrap_or_else(|| hook.to_string()),
    }
}

fn clip_type(moment: &Moment) -> String {
    let lower = format!("{} {}", moment.kind, moment.text).to_lowercase();
    if any_contains(&lower, &["joke", "funny", "laugh"]) {
        "funny_personality".to_string()
    } else if any_contains(&lower, &["disagree", "debate", "controvers"]) {
        "debate_moment".to_string()
    } else if any_contains(&lower, &["why", "how", "because", "framework"]) {
        "explainer".to_string()
    } else if any_contains(&lower, &["story", "journey", "failure", "learned"]) {
        "story".to_string()
    } else if any_contains(&lower, &["hot_take", "people think", "wrong"]) {
        "hot_take".to_string()
    } else {
        "quote_clip".to_string()
    }
}

fn suggested_title(hook: &str, topic: Option<&str>) -> String {
    let base = if hook.len() > 64 {
        format!("{}...", hook.chars().take(61).collect::<String>())
    } else {
        hook.to_string()
    };
    if base.split_whitespace().count() >= 3 {
        title_case(&base)
    } else {
        topic
            .map(title_case)
            .unwrap_or_else(|| "Short-Form Moment".to_string())
    }
}

fn suggested_caption(text: &str) -> String {
    let first = hook_from_text(text);
    format!("{first} Watch the full context before applying the final edit.")
}

fn hook_from_text(text: &str) -> String {
    sentences(text).first().cloned().unwrap_or_else(|| {
        text.split_whitespace()
            .take(12)
            .collect::<Vec<_>>()
            .join(" ")
    })
}

fn duration_class(duration_s: f64) -> DurationClass {
    if duration_s <= 30.0 {
        DurationClass::Micro
    } else if duration_s <= 90.0 {
        DurationClass::Standard
    } else {
        DurationClass::Extended
    }
}

fn topic_for_range(topics: &serde_json::Value, start_s: f64, end_s: f64) -> Option<String> {
    array_at(topics, "/topics").into_iter().find_map(|topic| {
        let topic_start = number_field(topic, &["start_s", "start"])?;
        let topic_end = number_field(topic, &["end_s", "end"])?;
        if topic_start <= end_s && topic_end >= start_s {
            string_field(topic, &["label", "topic", "title"])
        } else {
            None
        }
    })
}

fn broll_suggestions(text: &str, topic: Option<&str>) -> Vec<String> {
    let mut suggestions = Vec::new();
    let lower = text.to_lowercase();
    for (needle, suggestion) in [
        ("ai", "AI coding UI, terminal, or agent review screen"),
        ("coding", "developer workflow screen recording"),
        (
            "data center",
            "servers, racks, grid, and energy meter visuals",
        ),
        ("company", "company logo or product screenshot"),
        ("product", "product UI or contextual screenshot"),
        ("dashboard", "dashboard close-up or screen capture"),
        ("framework", "simple motion graphic of the framework steps"),
        ("percent", "statistic callout graphic"),
    ] {
        if lower.contains(needle) {
            suggestions.push(suggestion.to_string());
        }
    }
    if let Some(topic) = topic
        && !topic.trim().is_empty()
    {
        suggestions.push(format!("topic visual: {topic}"));
    }
    suggestions.sort();
    suggestions.dedup();
    suggestions
}

fn highlight_terms(text: &str, topic: Option<&str>) -> Vec<String> {
    let mut terms = Vec::new();
    for word in text.split_whitespace() {
        let cleaned = word
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_string();
        if cleaned.len() >= 5
            && (HOOK_KEYWORDS.contains(&cleaned.to_lowercase().as_str())
                || VALUE_KEYWORDS.contains(&cleaned.to_lowercase().as_str()))
        {
            terms.push(cleaned);
        }
    }
    if let Some(topic) = topic {
        terms.extend(
            topic
                .split_whitespace()
                .filter(|word| word.len() >= 3)
                .map(str::to_string),
        );
    }
    terms.sort();
    terms.dedup();
    terms.truncate(8);
    terms
}

fn max_face_count(face: &serde_json::Value) -> usize {
    array_at(face, "/per_frame")
        .into_iter()
        .filter_map(|frame| {
            frame
                .get("faces")
                .and_then(|faces| faces.as_array())
                .map(Vec::len)
        })
        .max()
        .unwrap_or(0)
}

fn overlapping_scene_count(scenes: &serde_json::Value, start_s: f64, end_s: f64) -> usize {
    array_at(scenes, "/shots")
        .into_iter()
        .filter(|scene| {
            let scene_start = number_field(scene, &["start_s", "start"]).unwrap_or(f64::MAX);
            let scene_end = number_field(scene, &["end_s", "end"]).unwrap_or(f64::MIN);
            scene_start < end_s && scene_end > start_s
        })
        .count()
}

fn shot_hint(shot: &serde_json::Value, start_s: f64, end_s: f64) -> Option<String> {
    array_at(shot, "/shots").into_iter().find_map(|entry| {
        let shot_start = number_field(entry, &["start_s", "start"]).unwrap_or(f64::MAX);
        let shot_end = number_field(entry, &["end_s", "end"]).unwrap_or(f64::MIN);
        if shot_start < end_s && shot_end > start_s {
            string_field(entry, &["shot_type", "type", "label"])
        } else {
            None
        }
    })
}

fn aspect_ratio(width: u32, height: u32) -> String {
    if width == 0 || height == 0 {
        return "unknown".to_string();
    }
    if width >= height {
        "16:9".to_string()
    } else {
        "9:16".to_string()
    }
}

fn sentences(text: &str) -> Vec<String> {
    text.split(['.', '!', '?'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn clarity_score(text: &str) -> f64 {
    let word_count = text.split_whitespace().count();
    if word_count >= 35 {
        0.45
    } else if word_count >= 12 {
        0.3
    } else {
        0.1
    }
}

fn completeness_score(moment: &Moment) -> f64 {
    let duration_s = moment.end_s - moment.start_s;
    let text = moment.text.to_lowercase();
    let kind = moment.kind.to_lowercase();
    let duration_signal = if duration_s >= 20.0 { 0.25 } else { 0.05 };
    let kind_signal = if any_contains(
        &kind,
        &["explainer", "story", "lesson", "hot_take", "quote"],
    ) {
        0.25
    } else {
        0.1
    };
    let language_signal = if any_contains(
        &text,
        &[
            "because",
            "that changes",
            "here is why",
            "the reason",
            "what happens",
        ],
    ) {
        0.3
    } else {
        0.05
    };
    duration_signal + kind_signal + language_signal
}

fn context_dependency(text: &str) -> f64 {
    let trimmed = text.trim_start();
    let starts_dependent = any_starts(trimmed, &["yeah", "so", "this", "that", "it", "they"]);
    let vague_refs = ["this", "that", "thing", "stuff", "you know"]
        .iter()
        .filter(|needle| trimmed.contains(**needle))
        .count() as f64;
    (if starts_dependent { 0.35 } else { 0.0 }) + (vague_refs * 0.08)
}

fn rambling_penalty(text: &str) -> f64 {
    let fillers = ["um", "uh", "kind of", "sort of", "you know", "like"]
        .iter()
        .filter(|needle| text.contains(**needle))
        .count() as f64;
    (fillers * 0.12).min(0.7)
}

fn keyword_score(text: &str, keywords: &[&str]) -> f64 {
    let hits = keywords
        .iter()
        .filter(|keyword| text.contains(**keyword))
        .count() as f64;
    (hits / 3.0).min(1.0)
}

fn any_contains(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn any_starts(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.starts_with(needle))
}

fn array_at<'a>(value: &'a serde_json::Value, pointer: &str) -> Vec<&'a serde_json::Value> {
    value
        .pointer(pointer)
        .and_then(|v| v.as_array())
        .map(|values| values.iter().collect())
        .unwrap_or_default()
}

fn number_field(value: &serde_json::Value, names: &[&str]) -> Option<f64> {
    names.iter().find_map(|name| value.get(*name)?.as_f64())
}

fn string_field(value: &serde_json::Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value.get(*name)?.as_str().map(str::to_string))
}

fn title_case(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "untitled".to_string()
    } else {
        out
    }
}

fn edl_string(text: &str) -> String {
    text.replace('"', "'")
}

fn bounded(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

const HOOK_KEYWORDS: &[&str] = &[
    "why",
    "how",
    "changed",
    "changing",
    "wrong",
    "faster",
    "surprising",
    "controversial",
    "people think",
    "here is",
];

const VALUE_KEYWORDS: &[&str] = &[
    "lesson",
    "insight",
    "because",
    "framework",
    "reason",
    "explains",
    "review",
    "teams",
    "product",
];

const EMOTION_KEYWORDS: &[&str] = &[
    "laughed", "laugh", "angry", "excited", "mistake", "failure", "disagree", "debate",
];

const VISUAL_KEYWORDS: &[&str] = &[
    "product",
    "place",
    "company",
    "event",
    "technical",
    "statistic",
    "compare",
    "process",
    "coding",
    "dashboard",
    "ai",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn input_with_moments(
        moments: serde_json::Value,
        trend_context: serde_json::Value,
    ) -> ShortFormReviewInput {
        ShortFormReviewInput {
            asset_id: "raw/technologia.mp4".into(),
            source_width: 1920,
            source_height: 1080,
            transcript: serde_json::json!({"segments": []}),
            editorial_moments: serde_json::json!({"clip_candidates": moments}),
            audio_energy: serde_json::Value::Null,
            topics: serde_json::json!({
                "topics": [
                    {"start_s": 0.0, "end_s": 120.0, "label": "AI coding agents"}
                ]
            }),
            scenes: serde_json::Value::Null,
            shot: serde_json::Value::Null,
            face: serde_json::json!({"per_frame": [{"faces": [{}, {}]}]}),
            gaze: serde_json::Value::Null,
            frame_quality: serde_json::Value::Null,
            composition: serde_json::Value::Null,
            broll_assets: serde_json::Value::Null,
            trend_context,
        }
    }

    #[test]
    fn trend_context_boosts_matching_clip_and_records_reason() {
        let input = input_with_moments(
            serde_json::json!([
                {
                    "id": "generic-story",
                    "start_s": 0.0,
                    "end_s": 35.0,
                    "kind": "story",
                    "text": "We learned a lesson about how student founders should talk to customers because it changed the product.",
                    "score": 0.75
                },
                {
                    "id": "trend-match",
                    "start_s": 50.0,
                    "end_s": 85.0,
                    "kind": "hot_take",
                    "text": "AI coding agents are changing how founders ship prototypes, and people are wrong about how fast this gets adopted.",
                    "score": 0.68
                }
            ]),
            serde_json::json!({
                "signals": [
                    {
                        "source": "x",
                        "label": "AI coding agents",
                        "keywords": ["AI coding agents", "prototypes"],
                        "weight": 0.9,
                        "reason": "X discussion is focused on AI coding agents and founder prototypes"
                    }
                ]
            }),
        );

        let review = build_short_form_review(
            input,
            ShortFormReviewOptions {
                profile: ShortFormProfile::ViralSocial,
                ..Default::default()
            },
        );

        let top = review
            .candidates
            .first()
            .expect("expected ranked candidate");
        assert_eq!(top.candidate_id, "short_000050_000085");
        assert!(top.score.trend_relevance > 0.0);
        assert!(top.trend_alignment.matched);
        assert_eq!(top.trend_alignment.signals[0].source, "x");
        assert!(
            top.why_ai_picked_it
                .iter()
                .any(|reason| reason.contains("trend context"))
        );
    }

    #[test]
    fn missing_trend_context_keeps_episode_evidence_ranking_available() {
        let input = input_with_moments(
            serde_json::json!([
                {
                    "id": "strong",
                    "start_s": 10.0,
                    "end_s": 50.0,
                    "kind": "lesson",
                    "text": "Here is why this product lesson matters because it explains how teams make faster decisions.",
                    "score": 0.8
                }
            ]),
            serde_json::Value::Null,
        );

        let review = build_short_form_review(input, ShortFormReviewOptions::default());

        let candidate = review.candidates.first().expect("expected candidate");
        assert!(!candidate.trend_alignment.matched);
        assert_eq!(candidate.score.trend_relevance, 0.0);
        assert!(
            candidate
                .trend_alignment
                .fallback_reason
                .contains("trend context unavailable")
        );
    }

    #[test]
    fn visual_decision_plan_maps_moments_to_existing_tools() {
        let input = input_with_moments(
            serde_json::json!([
                {
                    "id": "analogy",
                    "start_s": 5.0,
                    "end_s": 42.0,
                    "kind": "analogy",
                    "text": "Think about AI agents like junior teammates. The analogy is useful because the product needs review, not blind trust.",
                    "score": 0.72
                }
            ]),
            serde_json::Value::Null,
        );

        let review = build_short_form_review(input, ShortFormReviewOptions::default());

        let candidate = review.candidates.first().expect("expected candidate");
        assert_eq!(candidate.visual_decision_plan.moment_kind, "analogy");
        assert!(
            candidate
                .visual_decision_plan
                .decisions
                .iter()
                .any(|decision| decision.tool == "plan_visual_support_proposals"
                    && decision.action.contains("MotionScene"))
        );
        assert!(
            candidate
                .visual_decision_plan
                .decisions
                .iter()
                .any(|decision| decision.tool == "plan_reframe")
                || candidate
                    .visual_decision_plan
                    .decisions
                    .iter()
                    .any(|decision| decision.tool == "plan_multicam")
        );
    }

    #[test]
    fn multi_speaker_layout_contract_uses_dynamic_split_and_fill_segments() {
        let mut input = input_with_moments(
            serde_json::json!([
                {
                    "id": "founder-debate",
                    "start_s": 12.0,
                    "end_s": 52.0,
                    "kind": "debate",
                    "text": "The first founder disagrees with the product thesis. The second founder reacts and explains why the market signal changes the decision.",
                    "score": 0.82
                }
            ]),
            serde_json::Value::Null,
        );
        input.transcript = serde_json::json!({
            "speakers": ["founder_a", "founder_b"],
            "segments": [
                {"start_s": 12.0, "end_s": 20.0, "speaker_id": "founder_a"},
                {"start_s": 20.0, "end_s": 26.0, "speaker_id": "founder_b"},
                {"start_s": 26.0, "end_s": 36.0, "speaker_id": "founder_a"},
                {"start_s": 36.0, "end_s": 52.0, "speaker_id": "founder_b"}
            ]
        });
        input.face = serde_json::json!({
            "per_frame": [
                {"t_s": 12.0, "faces": [{"speaker_id": "founder_a"}, {"speaker_id": "founder_b"}]},
                {"t_s": 25.0, "faces": [{"speaker_id": "founder_a"}, {"speaker_id": "founder_b"}]}
            ]
        });

        let review = build_short_form_review(
            input,
            ShortFormReviewOptions {
                profile: ShortFormProfile::ViralSocial,
                ..Default::default()
            },
        );

        let candidate = review.candidates.first().expect("expected candidate");
        assert_eq!(
            candidate.vertical_layout.composition_mode,
            ShortFormCompositionMode::DynamicSwitching
        );
        assert!(
            candidate
                .vertical_layout
                .segments
                .iter()
                .any(|segment| segment.layout == ShortFormLayoutMode::SplitStacked)
        );
        assert!(
            candidate
                .vertical_layout
                .segments
                .iter()
                .any(|segment| matches!(segment.layout, ShortFormLayoutMode::FillSpeaker { .. }))
        );
        assert!(
            candidate
                .vertical_layout
                .edl_operations
                .iter()
                .any(|op| op.contains("\"mode\":\"dynamic_switching\""))
        );
    }

    #[test]
    fn single_speaker_layout_contract_avoids_split() {
        let mut input = input_with_moments(
            serde_json::json!([
                {
                    "id": "solo-explainer",
                    "start_s": 8.0,
                    "end_s": 34.0,
                    "kind": "explainer",
                    "text": "Here is why the product decision matters because one founder explains the full market signal.",
                    "score": 0.78
                }
            ]),
            serde_json::Value::Null,
        );
        input.transcript = serde_json::json!({
            "speakers": ["founder_a"],
            "segments": [
                {"start_s": 8.0, "end_s": 34.0, "speaker_id": "founder_a"}
            ]
        });
        input.face = serde_json::json!({
            "per_frame": [
                {"t_s": 8.0, "faces": [{"speaker_id": "founder_a"}]}
            ]
        });

        let review = build_short_form_review(input, ShortFormReviewOptions::default());

        let candidate = review.candidates.first().expect("expected candidate");
        assert_eq!(
            candidate.vertical_layout.composition_mode,
            ShortFormCompositionMode::ActiveSpeakerFill
        );
        assert!(
            candidate
                .vertical_layout
                .segments
                .iter()
                .all(|segment| !matches!(segment.layout, ShortFormLayoutMode::SplitStacked))
        );
    }

    #[test]
    fn dominant_speaker_layout_prefers_fill() {
        let mut input = input_with_moments(
            serde_json::json!([
                {
                    "id": "dominant-founder",
                    "start_s": 0.0,
                    "end_s": 20.0,
                    "kind": "lesson",
                    "text": "One founder explains the lesson while the other only gives a short backchannel.",
                    "score": 0.82
                }
            ]),
            serde_json::Value::Null,
        );
        input.transcript = serde_json::json!({
            "speakers": ["founder_a", "founder_b"],
            "segments": [
                {"start_s": 0.0, "end_s": 17.0, "speaker_id": "founder_a"},
                {"start_s": 17.2, "end_s": 19.0, "speaker_id": "founder_b"}
            ]
        });
        input.face = serde_json::json!({
            "per_frame": [
                {"t_s": 0.0, "faces": [{"speaker_id": "founder_a"}, {"speaker_id": "founder_b"}]}
            ]
        });

        let review = build_short_form_review(input, ShortFormReviewOptions::default());

        let candidate = review.candidates.first().expect("expected candidate");
        assert_eq!(
            candidate.vertical_layout.composition_mode,
            ShortFormCompositionMode::DynamicSwitching
        );
        assert_eq!(candidate.vertical_layout.segments.len(), 1);
        assert!(matches!(
            candidate.vertical_layout.segments[0].layout,
            ShortFormLayoutMode::FillSpeaker { .. }
        ));
    }
}
