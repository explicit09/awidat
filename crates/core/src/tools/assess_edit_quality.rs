//! `assess_edit_quality` agent tool.
//!
//! This is the editorial-grammar layer above `assess_continuity`.
//! It keeps the existing continuity verdict, then recommends the
//! lowest-attention editorial repair: hard cut, recut, J/L split edit,
//! b-roll, or a motivated transition.

use async_trait::async_trait;
use awidat_index::read_sidecar;
use awidat_proto::index::AssetId;
use awidat_proto::otio::{StackChild, TrackChild};
use awidat_proto::project::Project;
use serde::{Deserialize, Serialize};

use crate::FunctionCallError;
use crate::continuity::{CutKind, RuleVerdict, Verdict, assess_continuity as run_assess};
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

use super::assess_continuity::{build_inputs, resolve_asset_at};

/// Read-only editorial cut-quality assessor.
pub struct AssessEditQualityTool;

#[derive(Debug, Deserialize)]
struct AssessEditQualityArgs {
    /// Timeline-time seconds where the candidate cut would land.
    at_s: f64,
    /// `cut`, `trim_in`, or `trim_out`.
    kind: String,
    /// Optional high-level edit objective.
    #[serde(default)]
    objective: Option<String>,
    /// Optional asset id. If omitted, resolved from timeline.
    #[serde(default)]
    asset_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct EditQualityRecommendation {
    /// Main recommendation category.
    action: &'static str,
    /// Suggested cut grammar.
    cut_type: &'static str,
    /// Machine-readable purpose.
    intent: &'static str,
    /// Optional EDL operation heading that can implement the repair.
    edl_op: Option<&'static str>,
    /// Whether a visible transition should be considered.
    transition: Option<&'static str>,
    /// Whether b-roll/cutaway should be considered.
    broll: bool,
    /// Preferred b-roll mode when `broll` is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    broll_mode: Option<&'static str>,
    /// Human-readable explanation.
    reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct VisualCutContext {
    /// Shot-type bucket from the shot indexer, e.g. `close-up` or `wide`.
    shot_type: Option<String>,
    /// Motion bucket from the shot indexer, e.g. `static`, `slow-pan`, or `handheld`.
    motion: Option<String>,
    /// Average motion magnitude for the containing shot.
    motion_magnitude: Option<f64>,
    /// Dominant screen direction when the shot indexer can infer it.
    dominant_direction: Option<String>,
    /// Normalized subject center `[x, y]` in frame coordinates.
    subject_center: Option<[f64; 2]>,
    /// Normalized face center `[x, y]` in frame coordinates.
    face_center: Option<[f64; 2]>,
    /// Coarse subject placement: `left_third`, `center`, or `right_third`.
    subject_zone: Option<String>,
    /// Coarse face placement such as `upper_left` or `middle_center`.
    face_zone: Option<String>,
    /// Coarse headroom label: `tight`, `balanced`, or `loose`.
    headroom: Option<String>,
    /// Whether gaze has open, closed, direct, or mixed look space.
    look_space: Option<String>,
    /// Source of richer composition labels, e.g. `model:composition-v1`.
    composition_source: Option<String>,
    /// Confidence for model-backed composition labels.
    composition_confidence: Option<f64>,
    /// Model-labeled subject role, e.g. `primary_speaker`.
    subject_role: Option<String>,
    /// Model-labeled depth layer, e.g. `foreground`.
    depth_layer: Option<String>,
    /// Model-labeled framing, e.g. `over_the_shoulder`.
    framing: Option<String>,
    /// Average gaze offset: negative left, positive right, near zero at camera.
    gaze_score: Option<f64>,
    /// Fraction of sampled gaze frames looking at camera.
    at_camera_ratio: Option<f64>,
    /// Shot-level eye-trace bucket, e.g. `left`, `right`, `at_camera`, or `mixed`.
    eye_trace: Option<String>,
    /// Number of sampled gaze frames contributing to the summary.
    gaze_samples: Option<u64>,
    /// Normalized action intensity for cut-on-action decisions.
    action_score: Option<f64>,
    /// Normalized fast-pan confidence for whip-pan transitions.
    whip_pan_score: Option<f64>,
    /// Normalized occlusion/mask confidence for pass-by or invisible cuts.
    occlusion_score: Option<f64>,
    /// Best indexed match-cut candidate for this visual moment.
    match_candidate: Option<VisualMatchCandidate>,
    /// Highest indexed visual match score for match-cut decisions.
    match_cut_score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct VisualMatchCandidate {
    /// Asset id containing the matched visual moment, if it is known.
    asset_id: Option<String>,
    /// Source/media time for the matched visual moment.
    time_s: f64,
    /// Similarity confidence from the visual indexer.
    similarity: f64,
    /// Confidence after match-candidate ranking and margin checks.
    confidence: Option<f64>,
    /// Why the candidate matched, e.g. `shape_and_motion`.
    basis: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct StyleCutContext {
    /// Existing visible transitions in the 30 seconds before this candidate cut.
    transition_density_last_30s: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct LearnedEditorialPreferences {
    accepted_tags: Vec<String>,
    denied_tags: Vec<String>,
}

#[async_trait]
impl ToolHandler for AssessEditQualityTool {
    fn name(&self) -> &'static str {
        "assess_edit_quality"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "assess_edit_quality".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "at_s": {
                        "type": "number",
                        "minimum": 0.0,
                        "description": "Timeline-time seconds where the candidate cut would land."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["cut", "trim_in", "trim_out"],
                        "description": "Candidate edit kind."
                    },
                    "objective": {
                        "type": "string",
                        "description": "Optional high-level goal: tighten, speaker_handoff, hide_jump, beat_hit, scene_transition."
                    },
                    "asset_id": {
                        "type": "string",
                        "description": "Optional asset id. When omitted, the tool resolves it from the timeline."
                    }
                },
                "required": ["at_s", "kind"]
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
        let args: AssessEditQualityArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "assess_edit_quality: invalid args ({e}). Required: at_s, kind."
            ))
        })?;
        let kind = parse_cut_kind(&args.kind)?;

        let project = Project::read(&ctx.project_root).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "assess_edit_quality: failed to read project: {e}"
            ))
        })?;

        let resolved_asset_id = if let Some(asset_id) = args.asset_id.clone() {
            Some((asset_id, args.at_s, args.at_s))
        } else {
            resolve_asset_at(&project.timeline, args.at_s)
        };
        let Some((asset_id, source_at_s, _)) = resolved_asset_id else {
            return Err(FunctionCallError::RespondToModel(format!(
                "assess_edit_quality: no asset on the timeline at {}.",
                args.at_s
            )));
        };

        let inputs = build_inputs(&ctx.project_root, &project.timeline, &asset_id, args.at_s);
        let continuity = run_assess(source_at_s, args.at_s, kind, &inputs);
        let visual_context = load_visual_cut_context(&ctx.project_root, &asset_id, source_at_s);
        let style_context = StyleCutContext {
            transition_density_last_30s: count_recent_transitions(&project.timeline, args.at_s),
        };
        let learned_preferences = load_learned_editorial_preferences();
        let recommendation = recommend_edit(
            &continuity.rules,
            continuity.verdict,
            kind,
            args.objective.as_deref(),
            visual_context.as_ref(),
            &style_context,
            &learned_preferences,
        );
        let missing_signals = missing_signals(&continuity.rules);

        let body = serde_json::json!({
            "at_s": args.at_s,
            "source_at_s": source_at_s,
            "kind": args.kind,
            "objective": args.objective,
            "asset_id": asset_id,
            "continuity": {
                "verdict": continuity.verdict,
                "rules": continuity.rules,
            },
            "visual_context": visual_context,
            "style_context": style_context,
            "recommendation": recommendation,
            "missing_signals": missing_signals,
            "edl_guidance": edl_guidance(&recommendation, &learned_preferences),
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

fn parse_cut_kind(kind: &str) -> Result<CutKind, FunctionCallError> {
    match kind {
        "cut" => Ok(CutKind::Cut),
        "trim_in" => Ok(CutKind::TrimIn),
        "trim_out" => Ok(CutKind::TrimOut),
        other => Err(FunctionCallError::RespondToModel(format!(
            "assess_edit_quality: unknown kind {other:?}. Use cut / trim_in / trim_out."
        ))),
    }
}

fn recommend_edit(
    rules: &[RuleVerdict],
    verdict: Verdict,
    kind: CutKind,
    objective: Option<&str>,
    visual_context: Option<&VisualCutContext>,
    style_context: &StyleCutContext,
    learned_preferences: &LearnedEditorialPreferences,
) -> EditQualityRecommendation {
    let flagged = rules
        .iter()
        .filter(|rule| matches!(rule.verdict, Verdict::Risky | Verdict::Dirty))
        .map(|rule| rule.id.as_str())
        .collect::<Vec<_>>();

    if flagged.is_empty() && matches!(verdict, Verdict::Clean) {
        return EditQualityRecommendation {
            action: "use_hard_cut",
            cut_type: "hard_cut",
            intent: "clean_story_rhythm",
            edl_op: Some("Set Cut Intent"),
            transition: None,
            broll: false,
            broll_mode: None,
            reason: "Continuity checks are clean; a visible transition would call attention to a cut that already works.".into(),
        };
    }

    if flagged.contains(&"mid_sentence") {
        return EditQualityRecommendation {
            action: "recut",
            cut_type: "hard_cut",
            intent: "sentence_boundary",
            edl_op: None,
            transition: None,
            broll: false,
            broll_mode: None,
            reason: "The cut lands inside speech. Move the cut to a word, phrase, or sentence boundary before adding any transition.".into(),
        };
    }

    if flagged.contains(&"speaker_turn")
        || objective.map(|o| o == "speaker_handoff").unwrap_or(false)
    {
        let (action, cut_type, edl_op, reason) = match kind {
            CutKind::TrimIn | CutKind::Cut => (
                "use_j_cut",
                "j_cut",
                "Set Audio Lead",
                "Use a J-cut so incoming dialogue starts before picture; this solves the handoff in audio without a decorative transition.",
            ),
            CutKind::TrimOut => (
                "use_l_cut",
                "l_cut",
                "Set Audio Trail",
                "Use an L-cut so outgoing dialogue carries under the next image; this preserves thought continuity.",
            ),
        };
        return EditQualityRecommendation {
            action,
            cut_type,
            intent: "speaker_handoff",
            edl_op: Some(edl_op),
            transition: None,
            broll: false,
            broll_mode: None,
            reason: reason.into(),
        };
    }

    if flagged.contains(&"mid_motion") {
        let broll_mode = preferred_broll_mode(learned_preferences);
        let broll_suffix = broll_mode_reason_suffix(broll_mode);
        let density_threshold = transition_density_threshold(learned_preferences);
        if style_context.transition_density_last_30s >= density_threshold {
            return EditQualityRecommendation {
                action: "cover_or_cut_on_action",
                cut_type: "cut_on_action",
                intent: "hide_motion_jump_without_transition",
                edl_op: Some("Set Cut Intent"),
                transition: None,
                broll: true,
                broll_mode: Some(broll_mode),
                reason: format!(
                    "Motion continuity is the problem, but there are already {} visible transitions in the last 30s. Prefer moving the cut onto the action or covering it with motivated b-roll instead of adding another transition.{broll_suffix}",
                    style_context.transition_density_last_30s,
                ),
            };
        }
        if let Some(candidate) = strong_match_candidate(visual_context) {
            let basis = candidate
                .basis
                .as_deref()
                .map(|basis| format!(" via {basis}"))
                .unwrap_or_default();
            let location = candidate
                .asset_id
                .as_deref()
                .map(|asset_id| format!("{asset_id} at {:.2}s", candidate.time_s))
                .unwrap_or_else(|| format!("{:.2}s", candidate.time_s));
            return EditQualityRecommendation {
                action: "use_match_cut",
                cut_type: "match_cut",
                intent: "visual_match",
                edl_op: Some("Set Cut Intent"),
                transition: None,
                broll: false,
                broll_mode: None,
                reason: format!(
                    "The visual indexer found a strong match-cut candidate at {location} with {:.0}% similarity{basis}. Prefer a semantic match cut over adding a visible motion-cover transition.",
                    candidate.similarity * 100.0
                ),
            };
        }
        let transition = motion_transition(objective, visual_context);
        if let Some(tag) = learned_denied_transition_tag(transition, learned_preferences) {
            return EditQualityRecommendation {
                action: "cover_or_cut_on_action",
                cut_type: "cut_on_action",
                intent: "hide_motion_jump_without_transition",
                edl_op: Some("Set Cut Intent"),
                transition: None,
                broll: true,
                broll_mode: Some(broll_mode),
                reason: format!(
                    "Motion continuity is the problem, but learned preference `{tag}` is usually rejected. Prefer moving the cut onto the action or covering it with motivated b-roll instead of adding that visible transition.{broll_suffix}"
                ),
            };
        }
        let visual_reason = visual_context
            .and_then(|context| context.motion.as_deref())
            .map(|motion| format!(" The shot indexer labels the shot motion as {motion}."))
            .unwrap_or_default();
        return EditQualityRecommendation {
            action: "cover_or_cut_on_action",
            cut_type: "cut_on_action",
            intent: "hide_motion_jump",
            edl_op: Some("Set Cut Intent"),
            transition: Some(transition),
            broll: true,
            broll_mode: Some(broll_mode),
            reason: format!(
                "Motion continuity is the problem.{visual_reason} Prefer moving the cut onto the action or covering it with motivated b-roll; use {transition} only when it follows the visual motion.{broll_suffix}"
            ),
        };
    }

    if flagged.contains(&"breath_beat") {
        return EditQualityRecommendation {
            action: "preserve_audio_tail",
            cut_type: "l_cut",
            intent: "preserve_breath_beat",
            edl_op: Some("Set Audio Trail"),
            transition: None,
            broll: false,
            broll_mode: None,
            reason: "The edit cuts into a breath beat. Preserve a little outgoing audio instead of hiding the problem with a dissolve.".into(),
        };
    }

    if flagged.contains(&"rhythm") {
        return EditQualityRecommendation {
            action: "reduce_cut_density",
            cut_type: "hard_cut",
            intent: "rhythm_control",
            edl_op: None,
            transition: None,
            broll: false,
            broll_mode: None,
            reason: "Cut density is already high near this point. Avoid adding a visible transition; widen spacing or leave the cut alone.".into(),
        };
    }

    if matches!(verdict, Verdict::Abstain) {
        return EditQualityRecommendation {
            action: "gather_signals",
            cut_type: "hard_cut",
            intent: "needs_indexing",
            edl_op: None,
            transition: None,
            broll: false,
            broll_mode: None,
            reason: "The available sidecars are insufficient to judge the edit. Run indexing or inspect frames/transcript before committing.".into(),
        };
    }

    EditQualityRecommendation {
        action: "use_hard_cut_with_note",
        cut_type: "hard_cut",
        intent: "low_attention_edit",
        edl_op: Some("Set Cut Intent"),
        transition: None,
        broll: false,
        broll_mode: None,
        reason: "No specific repair dominates. Keep the edit low-attention and annotate the intent if you commit it.".into(),
    }
}

fn strong_match_candidate(
    visual_context: Option<&VisualCutContext>,
) -> Option<&VisualMatchCandidate> {
    let context = visual_context?;
    let candidate = context.match_candidate.as_ref()?;
    let score = context.match_cut_score.unwrap_or(candidate.similarity);
    let confidence = candidate.confidence.unwrap_or(candidate.similarity);
    if score >= 0.80 && candidate.similarity >= 0.75 && confidence >= 0.65 {
        Some(candidate)
    } else {
        None
    }
}

fn transition_density_threshold(preferences: &LearnedEditorialPreferences) -> usize {
    if tag_is_preferred(preferences, "transition_density:high") {
        5
    } else if tag_is_denied(preferences, "transition_density:moderate") {
        2
    } else {
        3
    }
}

fn preferred_broll_mode(preferences: &LearnedEditorialPreferences) -> &'static str {
    if tag_is_preferred(preferences, "broll_mode:thematic_montage")
        && tag_is_denied(preferences, "broll_mode:literal_cover")
    {
        "thematic_montage"
    } else {
        "literal_cover"
    }
}

fn broll_mode_reason_suffix(mode: &str) -> &'static str {
    if mode == "thematic_montage" {
        " Learned preferences favor thematic montage over literal cover here; ask before using thematic montage because it changes meaning rather than merely covering continuity."
    } else {
        ""
    }
}

fn tag_is_preferred(preferences: &LearnedEditorialPreferences, tag: &str) -> bool {
    preferences
        .accepted_tags
        .iter()
        .any(|accepted| accepted == tag)
        && !tag_is_denied(preferences, tag)
}

fn tag_is_denied(preferences: &LearnedEditorialPreferences, tag: &str) -> bool {
    preferences.denied_tags.iter().any(|denied| denied == tag)
}

fn load_learned_editorial_preferences() -> LearnedEditorialPreferences {
    crate::lessons::default_output_path()
        .and_then(|path| crate::lessons::read_learned_style(&path))
        .map(|markdown| parse_learned_editorial_preferences(&markdown))
        .unwrap_or_default()
}

fn parse_learned_editorial_preferences(markdown: &str) -> LearnedEditorialPreferences {
    let mut preferences = LearnedEditorialPreferences::default();
    for line in markdown.lines() {
        let Some((_, rest)) = line.split_once("editorial tag `") else {
            continue;
        };
        let Some((tag, after_tag)) = rest.split_once('`') else {
            continue;
        };
        let tag = normalize_editorial_tag(tag);
        if tag.is_empty() {
            continue;
        }
        if after_tag.contains("you accept") {
            preferences.accepted_tags.push(tag);
        } else if after_tag.contains("you deny") {
            preferences.denied_tags.push(tag);
        }
    }
    preferences.accepted_tags.sort();
    preferences.accepted_tags.dedup();
    preferences.denied_tags.sort();
    preferences.denied_tags.dedup();
    preferences
}

fn learned_denied_transition_tag(
    transition: &str,
    preferences: &LearnedEditorialPreferences,
) -> Option<String> {
    let tags = transition_tags(transition);
    if tags.iter().any(|tag| {
        preferences
            .accepted_tags
            .iter()
            .any(|accepted| accepted == tag)
    }) {
        return None;
    }
    tags.into_iter()
        .find(|tag| preferences.denied_tags.iter().any(|denied| denied == tag))
}

fn transition_tags(transition: &str) -> Vec<String> {
    let mut tags = vec![format!(
        "transition_id:{}",
        normalize_editorial_tag(transition)
    )];
    if let Some(definition) = awidat_proto::transitions::lookup_builtin_transition(transition) {
        tags.push(format!(
            "transition_family:{}",
            normalize_editorial_tag(definition.family)
        ));
    }
    tags
}

fn normalize_editorial_tag(tag: &str) -> String {
    tag.trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.'))
        .collect()
}

fn motion_transition(
    objective: Option<&str>,
    visual_context: Option<&VisualCutContext>,
) -> &'static str {
    if let Some(context) = visual_context
        && context.occlusion_score.unwrap_or(0.0) >= 0.75
    {
        return match context
            .dominant_direction
            .as_deref()
            .or_else(|| requested_direction(objective))
        {
            Some("left") => "awidat.pass_by_left",
            Some("right") => "awidat.pass_by_right",
            _ => "awidat.invisible_cut",
        };
    }

    match requested_direction(objective)
        .or_else(|| visual_context.and_then(|context| context.dominant_direction.as_deref()))
    {
        Some("right") => "awidat.whip_pan_right",
        Some("left") => "awidat.whip_pan_left",
        _ => "awidat.motion_blur",
    }
}

fn requested_direction(objective: Option<&str>) -> Option<&'static str> {
    let objective = objective?.to_ascii_lowercase();
    if objective.contains("right") {
        Some("right")
    } else if objective.contains("left") {
        Some("left")
    } else {
        None
    }
}

fn load_visual_cut_context(
    project_root: &std::path::Path,
    asset_id: &str,
    source_at_s: f64,
) -> Option<VisualCutContext> {
    let sidecar = read_sidecar(project_root, "shot", &AssetId::new(asset_id.to_string())).ok()?;
    let shots = sidecar
        .pointer("/data/shots")
        .or_else(|| sidecar.get("shots"))
        .and_then(|value| value.as_array())?;
    let shot = shots.iter().find(|shot| {
        let start = shot
            .get("start_s")
            .or_else(|| shot.get("start"))
            .and_then(|value| value.as_f64())
            .unwrap_or(f64::NEG_INFINITY);
        let end = shot
            .get("end_s")
            .or_else(|| shot.get("end"))
            .and_then(|value| value.as_f64())
            .unwrap_or(f64::INFINITY);
        start <= source_at_s && source_at_s <= end
    })?;
    Some(VisualCutContext {
        shot_type: shot
            .get("type")
            .and_then(|value| value.as_str())
            .map(String::from),
        motion: shot
            .get("motion")
            .and_then(|value| value.as_str())
            .map(String::from),
        motion_magnitude: shot
            .get("motion_magnitude")
            .and_then(|value| value.as_f64()),
        dominant_direction: shot
            .get("dominant_direction")
            .and_then(|value| value.as_str())
            .map(String::from),
        subject_center: point2(shot.get("subject_center")),
        face_center: point2(shot.get("face_center")),
        subject_zone: string_field(shot, "subject_zone"),
        face_zone: string_field(shot, "face_zone"),
        headroom: string_field(shot, "headroom"),
        look_space: string_field(shot, "look_space"),
        composition_source: string_field(shot, "composition_source"),
        composition_confidence: shot
            .get("composition_confidence")
            .and_then(|value| value.as_f64()),
        subject_role: string_field(shot, "subject_role"),
        depth_layer: string_field(shot, "depth_layer"),
        framing: string_field(shot, "framing"),
        gaze_score: shot.get("gaze_score").and_then(|value| value.as_f64()),
        at_camera_ratio: shot.get("at_camera_ratio").and_then(|value| value.as_f64()),
        eye_trace: shot
            .get("eye_trace")
            .and_then(|value| value.as_str())
            .map(String::from),
        gaze_samples: shot.get("gaze_samples").and_then(|value| value.as_u64()),
        action_score: shot.get("action_score").and_then(|value| value.as_f64()),
        whip_pan_score: shot.get("whip_pan_score").and_then(|value| value.as_f64()),
        occlusion_score: shot.get("occlusion_score").and_then(|value| value.as_f64()),
        match_candidate: best_match_candidate(shot),
        match_cut_score: shot
            .get("match_cut_score")
            .and_then(|value| value.as_f64())
            .or_else(|| best_match_candidate(shot).map(|candidate| candidate.similarity)),
    })
}

fn point2(value: Option<&serde_json::Value>) -> Option<[f64; 2]> {
    let values = value?.as_array()?;
    let x = values.first()?.as_f64()?;
    let y = values.get(1)?.as_f64()?;
    if values.len() == 2 {
        Some([x, y])
    } else {
        None
    }
}

fn best_match_candidate(shot: &serde_json::Value) -> Option<VisualMatchCandidate> {
    let candidates = shot.get("match_candidates")?.as_array()?;
    candidates
        .iter()
        .filter_map(|candidate| {
            Some(VisualMatchCandidate {
                asset_id: candidate
                    .get("asset_id")
                    .and_then(|value| value.as_str())
                    .map(String::from),
                time_s: candidate.get("time_s")?.as_f64()?,
                similarity: candidate.get("similarity")?.as_f64()?,
                confidence: candidate.get("confidence").and_then(|value| value.as_f64()),
                basis: candidate
                    .get("basis")
                    .and_then(|value| value.as_str())
                    .map(String::from),
            })
        })
        .max_by(|a, b| {
            a.similarity
                .partial_cmp(&b.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

fn string_field(shot: &serde_json::Value, key: &str) -> Option<String> {
    shot.get(key)
        .and_then(|value| value.as_str())
        .map(String::from)
}

fn count_recent_transitions(timeline: &awidat_proto::otio::Timeline, at_s: f64) -> usize {
    let window_start_s = (at_s - 30.0).max(0.0);
    let mut count = 0_usize;
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        let mut cursor_s = 0.0_f64;
        for child in &track.children {
            match child {
                TrackChild::Clip(clip) => {
                    if let Some(range) = clip.source_range.as_ref() {
                        cursor_s += range.duration.to_seconds();
                    }
                }
                TrackChild::Gap(gap) => {
                    cursor_s += gap.source_range.duration.to_seconds();
                }
                TrackChild::Transition(_) => {
                    if window_start_s <= cursor_s && cursor_s < at_s {
                        count += 1;
                    }
                }
                TrackChild::Stack(_) => {}
            }
        }
    }
    count
}

fn missing_signals(rules: &[RuleVerdict]) -> Vec<&str> {
    rules
        .iter()
        .filter(|rule| rule.verdict == Verdict::Abstain)
        .map(|rule| match rule.id.as_str() {
            "mid_sentence" => "transcript_words",
            "breath_beat" => "silence_ranges",
            "mid_motion" => "motion_or_scene",
            "speaker_turn" => "speaker_segments",
            "rhythm" => "nearby_cuts",
            _ => "unknown",
        })
        .collect()
}

fn edl_guidance(
    recommendation: &EditQualityRecommendation,
    learned_preferences: &LearnedEditorialPreferences,
) -> Option<String> {
    match recommendation.edl_op {
        Some("Set Cut Intent") => Some(format!(
            "*** Set Cut Intent with cut_type={} and intent={}",
            recommendation.cut_type, recommendation.intent
        )),
        Some("Set Audio Lead") => Some(format!(
            "*** Set Audio Lead with lead_s {}, then review by ear",
            preferred_split_range(learned_preferences, "audio_lead_range")
                .unwrap_or("around 0.25-0.60s")
        )),
        Some("Set Audio Trail") => Some(format!(
            "*** Set Audio Trail with trail_s {}, then review by ear",
            preferred_split_range(learned_preferences, "audio_trail_range")
                .unwrap_or("around 0.25-0.80s")
        )),
        _ => None,
    }
}

fn preferred_split_range(
    preferences: &LearnedEditorialPreferences,
    key: &str,
) -> Option<&'static str> {
    let preferred = preferences
        .accepted_tags
        .iter()
        .filter_map(|tag| tag.strip_prefix(key)?.strip_prefix(':'))
        .find(|bucket| {
            let full_tag = format!("{key}:{bucket}");
            !preferences
                .denied_tags
                .iter()
                .any(|denied| denied == &full_tag)
        })?;
    match (key, preferred) {
        ("audio_lead_range", "under_0_25") => Some("under 0.25s"),
        ("audio_lead_range", "0_25_to_0_60") => Some("around 0.25-0.60s"),
        ("audio_lead_range", "over_0_60") => Some("over 0.60s"),
        ("audio_trail_range", "under_0_25") => Some("under 0.25s"),
        ("audio_trail_range", "0_25_to_0_80") => Some("around 0.25-0.80s"),
        ("audio_trail_range", "over_0_80") => Some("over 0.80s"),
        _ => None,
    }
}

const DESCRIPTION: &str = "\
Assess the editorial quality of a candidate cut/trim/split. This wraps \
assess_continuity and returns a recommendation using editing grammar: \
hard cut, recut, cut on action, J-cut, L-cut, b-roll cover, or a \
motivated transition. Read-only; applying the edit is a separate \
apply_edl step. Use this before fixing dirty cuts so cross dissolves are \
not the default repair.";

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, verdict: Verdict) -> RuleVerdict {
        RuleVerdict {
            id: id.into(),
            verdict,
            reason: String::new(),
        }
    }

    #[test]
    fn clean_cut_recommends_hard_cut() {
        let rec = recommend_edit(
            &[rule("mid_sentence", Verdict::Clean)],
            Verdict::Clean,
            CutKind::Cut,
            None,
            None,
            &StyleCutContext {
                transition_density_last_30s: 0,
            },
            &LearnedEditorialPreferences::default(),
        );
        assert_eq!(rec.action, "use_hard_cut");
        assert_eq!(rec.cut_type, "hard_cut");
        assert_eq!(rec.transition, None);
    }

    #[test]
    fn speaker_handoff_recommends_split_edit() {
        let rec = recommend_edit(
            &[rule("speaker_turn", Verdict::Risky)],
            Verdict::Risky,
            CutKind::Cut,
            None,
            None,
            &StyleCutContext {
                transition_density_last_30s: 0,
            },
            &LearnedEditorialPreferences::default(),
        );
        assert_eq!(rec.action, "use_j_cut");
        assert_eq!(rec.edl_op, Some("Set Audio Lead"));
    }

    #[test]
    fn mid_motion_recommends_action_cut_or_broll_before_transition() {
        let rec = recommend_edit(
            &[rule("mid_motion", Verdict::Dirty)],
            Verdict::Dirty,
            CutKind::Cut,
            None,
            None,
            &StyleCutContext {
                transition_density_last_30s: 0,
            },
            &LearnedEditorialPreferences::default(),
        );
        assert_eq!(rec.action, "cover_or_cut_on_action");
        assert!(rec.broll);
        assert_eq!(rec.transition, Some("awidat.motion_blur"));
    }

    #[test]
    fn mid_motion_uses_requested_screen_direction() {
        let rec = recommend_edit(
            &[rule("mid_motion", Verdict::Dirty)],
            Verdict::Dirty,
            CutKind::Cut,
            Some("hide_jump direction:right"),
            Some(&VisualCutContext {
                shot_type: Some("wide".into()),
                motion: Some("handheld".into()),
                motion_magnitude: Some(0.9),
                dominant_direction: Some("right".into()),
                subject_center: None,
                face_center: None,
                subject_zone: None,
                face_zone: None,
                headroom: None,
                look_space: None,
                composition_source: None,
                composition_confidence: None,
                subject_role: None,
                depth_layer: None,
                framing: None,
                gaze_score: None,
                at_camera_ratio: None,
                eye_trace: None,
                gaze_samples: None,
                action_score: Some(0.7),
                whip_pan_score: Some(0.3),
                occlusion_score: Some(0.1),
                match_candidate: None,
                match_cut_score: None,
            }),
            &StyleCutContext {
                transition_density_last_30s: 0,
            },
            &LearnedEditorialPreferences::default(),
        );
        assert_eq!(rec.transition, Some("awidat.whip_pan_right"));
    }

    #[test]
    fn mid_motion_with_occlusion_recommends_pass_by_or_invisible_cut() {
        let pass_by = recommend_edit(
            &[rule("mid_motion", Verdict::Dirty)],
            Verdict::Dirty,
            CutKind::Cut,
            None,
            Some(&VisualCutContext {
                shot_type: Some("wide".into()),
                motion: Some("fast-cut".into()),
                motion_magnitude: Some(1.2),
                dominant_direction: Some("left".into()),
                subject_center: None,
                face_center: None,
                subject_zone: None,
                face_zone: None,
                headroom: None,
                look_space: None,
                composition_source: None,
                composition_confidence: None,
                subject_role: None,
                depth_layer: None,
                framing: None,
                gaze_score: None,
                at_camera_ratio: None,
                eye_trace: None,
                gaze_samples: None,
                action_score: Some(0.8),
                whip_pan_score: Some(0.2),
                occlusion_score: Some(0.82),
                match_candidate: None,
                match_cut_score: None,
            }),
            &StyleCutContext {
                transition_density_last_30s: 0,
            },
            &LearnedEditorialPreferences::default(),
        );
        assert_eq!(pass_by.transition, Some("awidat.pass_by_left"));

        let invisible = recommend_edit(
            &[rule("mid_motion", Verdict::Dirty)],
            Verdict::Dirty,
            CutKind::Cut,
            None,
            Some(&VisualCutContext {
                shot_type: Some("wide".into()),
                motion: Some("fast-cut".into()),
                motion_magnitude: Some(1.2),
                dominant_direction: None,
                subject_center: None,
                face_center: None,
                subject_zone: None,
                face_zone: None,
                headroom: None,
                look_space: None,
                composition_source: None,
                composition_confidence: None,
                subject_role: None,
                depth_layer: None,
                framing: None,
                gaze_score: None,
                at_camera_ratio: None,
                eye_trace: None,
                gaze_samples: None,
                action_score: Some(0.8),
                whip_pan_score: Some(0.2),
                occlusion_score: Some(0.82),
                match_candidate: None,
                match_cut_score: None,
            }),
            &StyleCutContext {
                transition_density_last_30s: 0,
            },
            &LearnedEditorialPreferences::default(),
        );
        assert_eq!(invisible.transition, Some("awidat.invisible_cut"));
    }

    #[test]
    fn mid_motion_with_match_candidate_recommends_match_cut() {
        let rec = recommend_edit(
            &[rule("mid_motion", Verdict::Dirty)],
            Verdict::Dirty,
            CutKind::Cut,
            None,
            Some(&VisualCutContext {
                shot_type: Some("medium".into()),
                motion: Some("handheld".into()),
                motion_magnitude: Some(0.7),
                dominant_direction: Some("right".into()),
                subject_center: None,
                face_center: None,
                subject_zone: None,
                face_zone: None,
                headroom: None,
                look_space: None,
                composition_source: None,
                composition_confidence: None,
                subject_role: None,
                depth_layer: None,
                framing: None,
                gaze_score: None,
                at_camera_ratio: None,
                eye_trace: None,
                gaze_samples: None,
                action_score: Some(0.6),
                whip_pan_score: Some(0.1),
                occlusion_score: Some(0.1),
                match_candidate: Some(VisualMatchCandidate {
                    asset_id: None,
                    time_s: 88.1,
                    similarity: 0.86,
                    confidence: Some(0.88),
                    basis: Some("shape_and_motion".into()),
                }),
                match_cut_score: Some(0.86),
            }),
            &StyleCutContext {
                transition_density_last_30s: 0,
            },
            &LearnedEditorialPreferences::default(),
        );
        assert_eq!(rec.action, "use_match_cut");
        assert_eq!(rec.cut_type, "match_cut");
        assert_eq!(rec.intent, "visual_match");
        assert_eq!(rec.transition, None);
        assert!(rec.reason.contains("88.10s"));
        assert!(rec.reason.contains("shape_and_motion"));
    }

    #[test]
    fn mid_motion_with_low_confidence_match_candidate_does_not_recommend_match_cut() {
        let rec = recommend_edit(
            &[rule("mid_motion", Verdict::Dirty)],
            Verdict::Dirty,
            CutKind::Cut,
            Some("hide_jump direction:right"),
            Some(&VisualCutContext {
                shot_type: Some("medium".into()),
                motion: Some("handheld".into()),
                motion_magnitude: Some(0.7),
                dominant_direction: Some("right".into()),
                subject_center: None,
                face_center: None,
                subject_zone: None,
                face_zone: None,
                headroom: None,
                look_space: None,
                composition_source: None,
                composition_confidence: None,
                subject_role: None,
                depth_layer: None,
                framing: None,
                gaze_score: None,
                at_camera_ratio: None,
                eye_trace: None,
                gaze_samples: None,
                action_score: Some(0.6),
                whip_pan_score: Some(0.1),
                occlusion_score: Some(0.1),
                match_candidate: Some(VisualMatchCandidate {
                    asset_id: Some("raw/broll.mp4".into()),
                    time_s: 88.1,
                    similarity: 0.91,
                    confidence: Some(0.42),
                    basis: Some("clip_embedding".into()),
                }),
                match_cut_score: Some(0.91),
            }),
            &StyleCutContext {
                transition_density_last_30s: 0,
            },
            &LearnedEditorialPreferences::default(),
        );
        assert_ne!(rec.action, "use_match_cut");
        assert_eq!(rec.cut_type, "cut_on_action");
        assert_eq!(rec.transition, Some("awidat.whip_pan_right"));
    }

    #[test]
    fn mid_motion_suppresses_transition_when_density_is_high() {
        let rec = recommend_edit(
            &[rule("mid_motion", Verdict::Dirty)],
            Verdict::Dirty,
            CutKind::Cut,
            Some("hide_jump direction:right"),
            None,
            &StyleCutContext {
                transition_density_last_30s: 3,
            },
            &LearnedEditorialPreferences::default(),
        );
        assert!(rec.broll);
        assert_eq!(rec.transition, None);
        assert_eq!(rec.intent, "hide_motion_jump_without_transition");
    }

    #[test]
    fn learned_high_transition_density_acceptance_raises_suppression_threshold() {
        let learned = parse_learned_editorial_preferences(
            "- When editorial tag `transition_density:high`, you accept 80% of the time (4 of 5) — +34pp vs your overall 46% accept rate for `apply_edl`.",
        );
        let rec = recommend_edit(
            &[rule("mid_motion", Verdict::Dirty)],
            Verdict::Dirty,
            CutKind::Cut,
            Some("hide_jump direction:right"),
            None,
            &StyleCutContext {
                transition_density_last_30s: 3,
            },
            &learned,
        );
        assert_eq!(rec.transition, Some("awidat.whip_pan_right"));
        assert_eq!(rec.intent, "hide_motion_jump");
    }

    #[test]
    fn learned_transition_family_denial_suppresses_motion_transition() {
        let learned = parse_learned_editorial_preferences(
            "- When editorial tag `transition_family:motion_blur`, you deny 83% of the time (5 of 6) — -40pp vs your overall 57% accept rate for `apply_edl`.",
        );
        let rec = recommend_edit(
            &[rule("mid_motion", Verdict::Dirty)],
            Verdict::Dirty,
            CutKind::Cut,
            None,
            None,
            &StyleCutContext {
                transition_density_last_30s: 0,
            },
            &learned,
        );
        assert!(rec.broll);
        assert_eq!(rec.transition, None);
        assert_eq!(rec.intent, "hide_motion_jump_without_transition");
        assert!(rec.reason.contains("learned preference"));
    }

    #[test]
    fn learned_thematic_montage_preference_changes_broll_mode_with_opt_in_guardrail() {
        let learned = parse_learned_editorial_preferences(
            "- When editorial tag `broll_mode:thematic_montage`, you accept 86% of the time (6 of 7) — +38pp vs your overall 48% accept rate for `apply_edl`.\n\
             - When editorial tag `broll_mode:literal_cover`, you deny 78% of the time (7 of 9) — -36pp vs your overall 58% accept rate for `apply_edl`.",
        );
        let rec = recommend_edit(
            &[rule("mid_motion", Verdict::Dirty)],
            Verdict::Dirty,
            CutKind::Cut,
            None,
            None,
            &StyleCutContext {
                transition_density_last_30s: 3,
            },
            &learned,
        );
        assert!(rec.broll);
        assert_eq!(rec.broll_mode, Some("thematic_montage"));
        assert_eq!(rec.transition, None);
        assert!(rec.reason.contains("ask before using thematic montage"));
    }

    #[test]
    fn learned_split_edit_range_changes_audio_guidance() {
        let learned = parse_learned_editorial_preferences(
            "- When editorial tag `audio_lead_range:over_0_60`, you accept 88% of the time (7 of 8) — +36pp vs your overall 52% accept rate for `apply_edl`.\n\
             - When editorial tag `audio_trail_range:under_0_25`, you accept 80% of the time (4 of 5) — +31pp vs your overall 49% accept rate for `apply_edl`.",
        );
        let j_cut = EditQualityRecommendation {
            action: "use_j_cut",
            cut_type: "j_cut",
            intent: "speaker_handoff",
            edl_op: Some("Set Audio Lead"),
            transition: None,
            broll: false,
            broll_mode: None,
            reason: String::new(),
        };
        let l_cut = EditQualityRecommendation {
            action: "preserve_audio_tail",
            cut_type: "l_cut",
            intent: "preserve_breath_beat",
            edl_op: Some("Set Audio Trail"),
            transition: None,
            broll: false,
            broll_mode: None,
            reason: String::new(),
        };

        assert_eq!(
            edl_guidance(&j_cut, &learned).as_deref(),
            Some("*** Set Audio Lead with lead_s over 0.60s, then review by ear")
        );
        assert_eq!(
            edl_guidance(&l_cut, &learned).as_deref(),
            Some("*** Set Audio Trail with trail_s under 0.25s, then review by ear")
        );
    }
}
