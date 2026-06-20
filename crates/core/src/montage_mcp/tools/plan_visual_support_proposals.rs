//! `plan_visual_support_proposals` — first-class proposal planner for
//! transcript/timeline visual support.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::editorial_skills::{
    EditorialSkillInstance, EditorialSkillRegistry, MatchEditorialSkillsRequest, definition_json,
    instance_json,
};
use crate::edl::{EdlOp, parse};
use crate::montage_mcp::context::McpToolCtx;
use montage_proto::professional::{CapabilityArea, EvidenceTrace, ProposalPackage, ReviewStatus};

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanVisualSupportProposalArgs {
    /// Selected transcript text, topic label, quote, claim, chapter, list item,
    /// or timeline-region description.
    pub selection_text: String,
    /// What the editor wants Montage to add or improve.
    pub request: String,
    /// Transcript phrase used by apply_edl to anchor generated timeline objects.
    #[serde(default)]
    pub anchor_transcript: Option<String>,
    /// Absolute timeline second where the anchored moment begins. When set, a
    /// generated MotionScene records it as its `start_s` so the graphic
    /// renders over the selected words instead of at program start.
    #[serde(default)]
    pub anchor_timeline_start_s: Option<f64>,
    /// Optional project-relative B-roll asset. When omitted, the B-roll package
    /// is returned as a generation package with missing information instead of
    /// an apply-ready Insert BRoll envelope.
    #[serde(default)]
    pub broll_asset: Option<String>,
    /// Optional artifact duration. Defaults are type-specific.
    #[serde(default)]
    pub duration_s: Option<f64>,
    /// Optional project-relative or external visual references that should
    /// inform style, layout, or source selection.
    #[serde(default)]
    pub reference_assets: Vec<String>,
    /// Optional story-map, beat, shot, topic, or transcript-window context that
    /// can trigger editorial skills even when the direct editor request is
    /// generic.
    #[serde(default)]
    pub story_context: Vec<StoryContextSignalArgs>,
    /// Intended output aspect ratio, such as `16:9`, `9:16`, or `1:1`.
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    /// Intended publishing surface or platform.
    #[serde(default)]
    pub platform: Option<String>,
    /// Whether the generated visual should preserve alpha/transparent
    /// background intent when supported.
    #[serde(default)]
    pub alpha: Option<bool>,
    /// Preferred typography guidance reused from clarification answers.
    #[serde(default)]
    pub typography: Option<String>,
    /// Preferred color palette guidance reused from clarification answers.
    #[serde(default)]
    pub color_palette: Option<String>,
    /// Preferred motion intensity, such as `low`, `standard`, or `high`.
    #[serde(default)]
    pub motion_intensity: Option<String>,
    /// Preferred safe-area policy for platform-specific overlays.
    #[serde(default)]
    pub safe_area_policy: Option<String>,
    /// Optional show/project brand package name.
    #[serde(default)]
    pub brand_package: Option<String>,
    /// Natural-language revision instruction for a pending visual-support
    /// proposal.
    #[serde(default)]
    pub revision_instruction: Option<String>,
    /// Optional lane constraint: `"motion_scene"` returns only native
    /// MotionScene proposals and `"broll"` only the B-roll package.
    /// The planner honors the constraint instead of switching lanes;
    /// omit it to let editorial skills choose.
    #[serde(default)]
    pub lane: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct StoryContextSignalArgs {
    /// Kind of story signal, such as `topic_shift`, `hook`, `beat`, `shot`, or
    /// `transcript_window`.
    #[serde(default)]
    pub kind: String,
    /// Human-readable story-map/topic/beat/shot label.
    #[serde(default)]
    pub label: String,
    /// Optional transcript text associated with this story signal.
    #[serde(default)]
    pub transcript: Option<String>,
    /// Optional timeline start, in seconds.
    #[serde(default)]
    pub start_s: Option<f64>,
    /// Optional timeline end, in seconds.
    #[serde(default)]
    pub end_s: Option<f64>,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ReviseVisualSupportProposalArgs {
    /// A single proposal returned by `plan_visual_support_proposals`.
    pub proposal: serde_json::Value,
    /// Natural-language change request, such as "make it 3x faster" or
    /// "use transparent background".
    pub instruction: String,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct SaveVisualSupportDefaultsArgs {
    /// Preferred output aspect ratio, such as `16:9`, `9:16`, or `1:1`.
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    /// Preferred publishing surface or platform.
    #[serde(default)]
    pub platform: Option<String>,
    /// Whether generated visual support should preserve alpha intent by default.
    #[serde(default)]
    pub alpha: Option<bool>,
    /// Project-relative reference assets reused as visual style/source defaults.
    #[serde(default)]
    pub reference_assets: Vec<String>,
    /// Preferred typography guidance reused from clarification answers.
    #[serde(default)]
    pub typography: Option<String>,
    /// Preferred color palette guidance reused from clarification answers.
    #[serde(default)]
    pub color_palette: Option<String>,
    /// Preferred motion intensity, such as `low`, `standard`, or `high`.
    #[serde(default)]
    pub motion_intensity: Option<String>,
    /// Preferred safe-area policy for platform-specific overlays.
    #[serde(default)]
    pub safe_area_policy: Option<String>,
    /// Optional show/project brand package name.
    #[serde(default)]
    pub brand_package: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct VerifyVisualSupportArtifactArgs {
    /// A single proposal returned by `plan_visual_support_proposals`.
    pub proposal: serde_json::Value,
    /// When true, B-roll proposals fail if their project-relative asset is not
    /// present under the current project root.
    #[serde(default)]
    pub require_project_asset_exists: bool,
    /// Optional rendered-frame verification report produced after render.
    #[serde(default)]
    pub render_frame_report: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct VisualSupportDefaults {
    #[serde(default)]
    aspect_ratio: Option<String>,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    alpha: Option<bool>,
    #[serde(default)]
    reference_assets: Vec<String>,
    #[serde(default)]
    typography: Option<String>,
    #[serde(default)]
    color_palette: Option<String>,
    #[serde(default)]
    motion_intensity: Option<String>,
    #[serde(default)]
    safe_area_policy: Option<String>,
    #[serde(default)]
    brand_package: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtifactType {
    QuoteHighlight,
    AnimatedList,
    TitleCard,
    SearchBar,
    CounterStatGraphic,
    MapVisualization,
    BrollPackage,
}

impl ArtifactType {
    fn as_str(self) -> &'static str {
        match self {
            ArtifactType::QuoteHighlight => "quote_highlight",
            ArtifactType::AnimatedList => "animated_list",
            ArtifactType::TitleCard => "title_card",
            ArtifactType::SearchBar => "search_bar",
            ArtifactType::CounterStatGraphic => "counter_stat_graphic",
            ArtifactType::MapVisualization => "map_visualization",
            ArtifactType::BrollPackage => "broll_package",
        }
    }

    fn default_duration_s(self) -> f64 {
        match self {
            ArtifactType::QuoteHighlight => 4.0,
            ArtifactType::AnimatedList => 6.0,
            ArtifactType::TitleCard => 3.5,
            ArtifactType::SearchBar => 5.0,
            ArtifactType::CounterStatGraphic => 4.0,
            ArtifactType::MapVisualization => 6.0,
            ArtifactType::BrollPackage => 3.0,
        }
    }
}

pub fn run(args: PlanVisualSupportProposalArgs, ctx: McpToolCtx) -> Result<String, String> {
    let args = merge_visual_support_defaults(args, &ctx.project_root);
    let body = plan_visual_support_proposals(args)?;
    serde_json::to_string_pretty(&body)
        .map_err(|e| format!("plan_visual_support_proposals: serialize failed: {e}"))
}

pub fn save_visual_support_defaults(
    args: SaveVisualSupportDefaultsArgs,
    ctx: McpToolCtx,
) -> Result<String, String> {
    let defaults = VisualSupportDefaults {
        aspect_ratio: clean_option(args.aspect_ratio),
        platform: clean_option(args.platform),
        alpha: args.alpha,
        reference_assets: args
            .reference_assets
            .into_iter()
            .map(|asset| asset.trim().to_string())
            .filter(|asset| !asset.is_empty())
            .collect(),
        typography: clean_option(args.typography),
        color_palette: clean_option(args.color_palette),
        motion_intensity: clean_option(args.motion_intensity),
        safe_area_policy: clean_option(args.safe_area_policy),
        brand_package: clean_option(args.brand_package),
    };
    let path = visual_support_defaults_path(&ctx.project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("save_visual_support_defaults: create .montage failed: {e}"))?;
    }
    let body = serde_json::to_string_pretty(&defaults)
        .map_err(|e| format!("save_visual_support_defaults: serialize failed: {e}"))?;
    std::fs::write(&path, body)
        .map_err(|e| format!("save_visual_support_defaults: write failed: {e}"))?;
    serde_json::to_string_pretty(&serde_json::json!({
        "status": "saved",
        "path": ".montage/visual_support_defaults.json",
        "defaults": defaults,
    }))
    .map_err(|e| format!("save_visual_support_defaults: response serialize failed: {e}"))
}

pub fn run_revision(
    args: ReviseVisualSupportProposalArgs,
    _ctx: McpToolCtx,
) -> Result<String, String> {
    let body = revise_visual_support_proposal(args)?;
    serde_json::to_string_pretty(&body)
        .map_err(|e| format!("revise_visual_support_proposal: serialize failed: {e}"))
}

pub fn run_artifact_verification(
    args: VerifyVisualSupportArtifactArgs,
    ctx: McpToolCtx,
) -> Result<String, String> {
    // The project root is always available so the verifier can
    // recognize MotionScene proposals that are already stored in
    // timeline metadata; B-roll asset existence stays gated by
    // `require_project_asset_exists` inside the checks.
    let body = verify_visual_support_artifact(args, Some(ctx.project_root.as_path()))?;
    serde_json::to_string_pretty(&body)
        .map_err(|e| format!("verify_visual_support_artifact: serialize failed: {e}"))
}

pub fn plan_visual_support_proposals(
    args: PlanVisualSupportProposalArgs,
) -> Result<serde_json::Value, String> {
    let selection = args.selection_text.trim();
    let request = args.request.trim();
    if selection.is_empty() {
        return Err("plan_visual_support_proposals: selection_text must be non-empty.".into());
    }
    if request.is_empty() {
        return Err("plan_visual_support_proposals: request must be non-empty.".into());
    }

    let anchor = args
        .anchor_transcript
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(selection);
    let story_context_text = story_context_match_text(&args.story_context);
    let match_selection = if story_context_text.is_empty() {
        selection.to_string()
    } else {
        format!("{selection} {story_context_text}")
    };
    let lane = parse_lane_constraint(args.lane.as_deref())?;
    let registry = EditorialSkillRegistry::bundled();
    let skill_instances = registry.match_instances(MatchEditorialSkillsRequest {
        selection_text: match_selection,
        request: request.to_string(),
        anchor_transcript: Some(anchor.to_string()),
    });
    let artifacts = artifact_types_for_instances(&skill_instances);
    let (artifacts, lane_constraint) = apply_lane_constraint(artifacts, lane);
    let proposals: Vec<serde_json::Value> = artifacts
        .into_iter()
        .map(|artifact| {
            let instance = skill_instances
                .iter()
                .find(|instance| instance.artifact_type == artifact.as_str());
            proposal_for_artifact(artifact, instance, selection, request, anchor, &args)
        })
        .collect();

    Ok(serde_json::json!({
        "workflow": "proposal_to_visual_support",
        "selection": {
            "text": selection,
            "anchor_transcript": anchor,
        },
        "lane_constraint": lane_constraint,
        "story_context": story_context_contract(&args.story_context),
        "project_defaults": {
            "path": ".montage/visual_support_defaults.json",
            "applied": args.project_defaults_applied(),
        },
        "editorial_skill_registry": editorial_skill_registry_summary(&registry),
        "skill_candidates": skill_instances.iter().map(instance_json).collect::<Vec<_>>(),
        "proposal_count": proposals.len(),
        "proposals": proposals,
        "review_surface": "Reuse ProposedEdit/ProposalInspector by passing a proposal's apply_edl payload through apply_edl; the returned EDL is the timeline proposal.",
        "iteration": {
            "natural_language_supported": true,
            "examples": [
                "make it shorter",
                "make the list faster",
                "use a title card instead",
                "replace this with B-roll",
                "make the quote highlight less intense"
            ]
        },
        "verification_contract": [
            "apply proposal through apply_edl",
            "inspect with view_timeline",
            "preview the affected timeline region",
            "render with start_render",
            "verify with verify_render",
            "verify artifact-specific proposal contract with verify_visual_support_artifact",
            "for generated B-roll, confirm provenance and transcript anchor still match"
        ]
    }))
}

impl PlanVisualSupportProposalArgs {
    fn project_defaults_applied(&self) -> bool {
        self.aspect_ratio.is_some()
            || self.platform.is_some()
            || self.alpha.is_some()
            || !self.reference_assets.is_empty()
            || self.typography.is_some()
            || self.color_palette.is_some()
            || self.motion_intensity.is_some()
            || self.safe_area_policy.is_some()
            || self.brand_package.is_some()
    }
}

/// Merge the project's saved `.montage/visual_support_defaults.json` into
/// `args`, filling only fields the caller left unset. Both the MCP `run`
/// wrapper and the desktop transcript-selection path call this so
/// agent-planned and user-initiated proposals share the same brand / aspect /
/// safe-area / reference contract.
pub fn merge_visual_support_defaults(
    mut args: PlanVisualSupportProposalArgs,
    project_root: &Path,
) -> PlanVisualSupportProposalArgs {
    let Some(defaults) = load_visual_support_defaults(project_root) else {
        return args;
    };
    if args.aspect_ratio.is_none() {
        args.aspect_ratio = defaults.aspect_ratio;
    }
    if args.platform.is_none() {
        args.platform = defaults.platform;
    }
    if args.alpha.is_none() {
        args.alpha = defaults.alpha;
    }
    if args.reference_assets.is_empty() {
        args.reference_assets = defaults.reference_assets;
    }
    if args.typography.is_none() {
        args.typography = defaults.typography;
    }
    if args.color_palette.is_none() {
        args.color_palette = defaults.color_palette;
    }
    if args.motion_intensity.is_none() {
        args.motion_intensity = defaults.motion_intensity;
    }
    if args.safe_area_policy.is_none() {
        args.safe_area_policy = defaults.safe_area_policy;
    }
    if args.brand_package.is_none() {
        args.brand_package = defaults.brand_package;
    }
    args
}

fn load_visual_support_defaults(project_root: &Path) -> Option<VisualSupportDefaults> {
    let path = visual_support_defaults_path(project_root);
    let body = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&body).ok()
}

fn visual_support_defaults_path(project_root: &Path) -> std::path::PathBuf {
    project_root
        .join(".montage")
        .join("visual_support_defaults.json")
}

fn clean_option(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn story_context_match_text(story_context: &[StoryContextSignalArgs]) -> String {
    story_context
        .iter()
        .filter_map(|signal| {
            let mut parts = Vec::new();
            if !signal.kind.trim().is_empty() {
                parts.push(signal.kind.trim().to_string());
            }
            if !signal.label.trim().is_empty() {
                parts.push(signal.label.trim().to_string());
            }
            if let Some(transcript) = signal
                .transcript
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                parts.push(transcript.to_string());
            }
            (!parts.is_empty()).then(|| parts.join(" "))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn story_context_contract(story_context: &[StoryContextSignalArgs]) -> Vec<serde_json::Value> {
    story_context
        .iter()
        .filter_map(|signal| {
            let kind = signal.kind.trim();
            let label = signal.label.trim();
            if kind.is_empty() && label.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "kind": kind,
                "label": label,
                "transcript": signal.transcript,
                "timeline_start_s": signal.start_s,
                "timeline_end_s": signal.end_s,
            }))
        })
        .collect()
}

pub fn revise_visual_support_proposal(
    args: ReviseVisualSupportProposalArgs,
) -> Result<serde_json::Value, String> {
    let instruction = args.instruction.trim();
    if instruction.is_empty() {
        return Err("revise_visual_support_proposal: instruction must be non-empty.".into());
    }
    let mut proposal = args.proposal;
    if !proposal.is_object() {
        return Err("revise_visual_support_proposal: proposal must be a JSON object.".into());
    }

    let original_proposal = proposal.clone();
    let mut diff = Vec::new();
    let lower = instruction.to_ascii_lowercase();
    let before_visual = visual_diff_snapshot(&proposal);
    if let Some(target_artifact) = revision_target_artifact(&lower) {
        let from_artifact = proposal["artifact_type"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        if from_artifact != target_artifact.as_str() {
            proposal = convert_proposal_to_artifact(&proposal, instruction, target_artifact);
            diff.push(serde_json::json!({
                "field": "artifact_type",
                "from": from_artifact,
                "to": target_artifact.as_str(),
                "reason": instruction,
            }));
        }
    }
    if let Some(multiplier) = speed_multiplier(&lower) {
        if let Some(old_duration) = proposal["export_intent"]["duration_s"].as_f64() {
            let new_duration = round3((old_duration / multiplier).max(0.25));
            set_json_path(
                &mut proposal,
                &["export_intent", "duration_s"],
                new_duration.into(),
            );
            revise_apply_edl_duration(&mut proposal, new_duration);
            diff.push(serde_json::json!({
                "field": "export_intent.duration_s",
                "from": old_duration,
                "to": new_duration,
                "reason": instruction,
            }));
        }
    } else if lower.contains("shorter") {
        if let Some(old_duration) = proposal["export_intent"]["duration_s"].as_f64() {
            let new_duration = round3((old_duration * 0.5).max(0.25));
            set_json_path(
                &mut proposal,
                &["export_intent", "duration_s"],
                new_duration.into(),
            );
            revise_apply_edl_duration(&mut proposal, new_duration);
            diff.push(serde_json::json!({
                "field": "export_intent.duration_s",
                "from": old_duration,
                "to": new_duration,
                "reason": instruction,
            }));
        }
    }

    if lower.contains("transparent") || lower.contains("alpha") {
        let old_alpha = proposal["export_intent"]["alpha"]
            .as_bool()
            .unwrap_or(false);
        if !old_alpha {
            set_json_path(&mut proposal, &["export_intent", "alpha"], true.into());
            diff.push(serde_json::json!({
                "field": "export_intent.alpha",
                "from": old_alpha,
                "to": true,
                "reason": instruction,
            }));
        }
    }

    if wants_lower_intensity(&lower) {
        let old_intensity = proposal["export_intent"]["visual_intensity"]
            .as_str()
            .unwrap_or("standard")
            .to_string();
        if old_intensity != "low" {
            set_json_path(
                &mut proposal,
                &["export_intent", "visual_intensity"],
                "low".into(),
            );
            diff.push(serde_json::json!({
                "field": "export_intent.visual_intensity",
                "from": old_intensity,
                "to": "low",
                "reason": instruction,
            }));
        }
    }

    if let Some(revision) = proposal.get_mut("revision") {
        revision["instruction"] = serde_json::json!(instruction);
        revision["last_diff"] = serde_json::json!(diff);
    } else {
        proposal["revision"] = serde_json::json!({
            "supported": true,
            "instruction": instruction,
            "last_diff": diff,
        });
    }

    let after_visual = visual_diff_snapshot(&proposal);
    let preview_comparison =
        revision_preview_comparison(&original_proposal, &proposal, &before_visual, &after_visual);
    Ok(serde_json::json!({
        "workflow": "proposal_to_visual_support_revision",
        "proposal": proposal,
        "diff": diff,
        "visual_diff": {
            "before": before_visual,
            "after": after_visual,
            "policy": "Review changed artifact type, timeline object, duration, alpha, and missing information before apply_edl."
        },
        "preview_comparison": preview_comparison,
        "next_step": "review the diff before apply_edl; accepted revised proposals still render and verify through the existing pipeline"
    }))
}

fn revision_target_artifact(instruction: &str) -> Option<ArtifactType> {
    if instruction.contains("b-roll")
        || instruction.contains("broll")
        || instruction.contains("footage")
        || instruction.contains("cutaway")
    {
        Some(ArtifactType::BrollPackage)
    } else if instruction.contains("title card") || instruction.contains("chapter card") {
        Some(ArtifactType::TitleCard)
    } else if instruction.contains("quote") {
        Some(ArtifactType::QuoteHighlight)
    } else if instruction.contains("list") || instruction.contains("retention opener") {
        Some(ArtifactType::AnimatedList)
    } else if instruction.contains("search bar") || instruction.contains("search sequence") {
        Some(ArtifactType::SearchBar)
    } else if instruction.contains("counter") || instruction.contains("stat") {
        Some(ArtifactType::CounterStatGraphic)
    } else if instruction.contains("map") || instruction.contains("route") {
        Some(ArtifactType::MapVisualization)
    } else {
        None
    }
}

fn wants_lower_intensity(instruction: &str) -> bool {
    instruction.contains("less intense")
        || instruction.contains("less intensity")
        || instruction.contains("subtle")
        || instruction.contains("tone down")
        || instruction.contains("quieter")
}

fn convert_proposal_to_artifact(
    proposal: &serde_json::Value,
    instruction: &str,
    artifact: ArtifactType,
) -> serde_json::Value {
    let anchor = transcript_evidence_anchor(proposal)
        .or_else(|| proposal["review"]["rationale"].as_str().map(concise))
        .unwrap_or_else(|| "selected transcript moment".to_string());
    // Preserve the existing scene's timeline anchor across a revision so a
    // "make it faster" / artifact-swap revise doesn't snap the graphic back
    // to program start.
    let existing_start_s = motion_scene_from_proposal(proposal)
        .map(|scene| scene.start_s)
        .filter(|value| *value > 0.0);
    let args = PlanVisualSupportProposalArgs {
        selection_text: anchor.clone(),
        request: instruction.to_string(),
        anchor_transcript: Some(anchor.clone()),
        anchor_timeline_start_s: existing_start_s,
        broll_asset: None,
        duration_s: proposal["export_intent"]["duration_s"].as_f64(),
        reference_assets: proposal_reference_assets(proposal),
        story_context: Vec::new(),
        aspect_ratio: proposal["export_intent"]["aspect_ratio"]
            .as_str()
            .map(str::to_string)
            .filter(|value| value != "project"),
        platform: proposal["export_intent"]["platform"]
            .as_str()
            .map(str::to_string)
            .filter(|value| value != "project"),
        alpha: proposal["export_intent"]["alpha"].as_bool(),
        typography: proposal_style_default(proposal, "typography"),
        color_palette: proposal_style_default(proposal, "color_palette"),
        motion_intensity: proposal["export_intent"]["visual_intensity"]
            .as_str()
            .map(str::to_string)
            .filter(|value| value != "standard"),
        safe_area_policy: proposal_style_default(proposal, "safe_area_policy"),
        brand_package: proposal_style_default(proposal, "brand_package"),
        revision_instruction: Some(instruction.to_string()),
        lane: None,
    };
    proposal_for_artifact(artifact, None, &anchor, instruction, &anchor, &args)
}

fn proposal_style_default(proposal: &serde_json::Value, field: &str) -> Option<String> {
    proposal["style_defaults"][field]
        .as_str()
        .map(str::to_string)
        .filter(|value| value != "project")
}

fn proposal_reference_assets(proposal: &serde_json::Value) -> Vec<String> {
    proposal["references"]["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|asset| asset.as_str().map(str::to_string))
        .collect()
}

fn visual_diff_snapshot(proposal: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "artifact_type": proposal["artifact_type"].as_str().unwrap_or("unknown"),
        "title": proposal["title"].as_str().unwrap_or(""),
        "timeline_object": proposal["preview"]["timeline_object"].as_str().unwrap_or(""),
        "duration_s": proposal["export_intent"]["duration_s"].as_f64(),
        "alpha": proposal["export_intent"]["alpha"].as_bool(),
        "visual_intensity": proposal["export_intent"]["visual_intensity"]
            .as_str()
            .unwrap_or("standard"),
        "missing_information_count": proposal["missing_information"]
            .as_array()
            .map_or(0, Vec::len),
    })
}

fn revision_preview_comparison(
    before_proposal: &serde_json::Value,
    after_proposal: &serde_json::Value,
    before_visual: &serde_json::Value,
    after_visual: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "layout": "side_by_side",
        "before": revision_preview_panel("before", before_proposal, before_visual),
        "after": revision_preview_panel("after", after_proposal, after_visual),
        "render_contract": {
            "mode": "preview_cache_pair",
            "panels": ["before", "after"],
            "timeline_region": {
                "start_s": 0.0,
                "duration_s": after_proposal["export_intent"]["duration_s"].as_f64()
                    .or_else(|| before_proposal["export_intent"]["duration_s"].as_f64()),
            },
            "hide_backend_details": true,
            "outputs": {
                "before_thumbnail": "preview_comparison/before.png",
                "after_thumbnail": "preview_comparison/after.png",
                "diff_manifest": "preview_comparison/diff.json",
            }
        },
        "verification_contract": {
            "checks": [
                "before_after_frame_count_available",
                "changed_fields_visible_or_declared_metadata_only",
                "preview_pair_matches_revision_artifact_types",
            ],
            "required_before_acceptance": false,
            "required_before_final_delivery": true,
        }
    })
}

fn revision_preview_panel(
    label: &str,
    proposal: &serde_json::Value,
    visual: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "label": label,
        "artifact_type": visual["artifact_type"].as_str().unwrap_or("unknown"),
        "title": visual["title"].as_str().unwrap_or(""),
        "timeline_object": visual["timeline_object"].as_str().unwrap_or(""),
        "duration_s": visual["duration_s"].as_f64(),
        "alpha": visual["alpha"].as_bool(),
        "visual_intensity": visual["visual_intensity"].as_str().unwrap_or("standard"),
        "supported_preview": proposal["preview"]["supported_preview"].as_bool().unwrap_or(false),
    })
}

pub fn verify_visual_support_artifact(
    args: VerifyVisualSupportArtifactArgs,
    project_root: Option<&Path>,
) -> Result<serde_json::Value, String> {
    if !args.proposal.is_object() {
        return Err("verify_visual_support_artifact: proposal must be a JSON object.".into());
    }
    let artifact_type = args.proposal["artifact_type"]
        .as_str()
        .ok_or_else(|| "verify_visual_support_artifact: missing artifact_type".to_string())?
        .to_string();
    // A proposal whose MotionScene is already stored in timeline
    // metadata is applied, not "not apply-ready". Recognize it, and
    // audit the stored scene when the proposal carries no apply_edl.
    let stored_scene = project_root
        .zip(proposal_motion_scene_id(&args.proposal))
        .and_then(|(root, scene_id)| stored_motion_scene(root, &scene_id));
    let mut proposal = args.proposal;
    if let Some(scene) = &stored_scene {
        let has_edl = proposal["apply_edl"]["edl"]
            .as_str()
            .is_some_and(|edl| !edl.trim().is_empty());
        if !has_edl && let Ok(scene_json) = serde_json::to_string(scene) {
            proposal["apply_edl"] = serde_json::json!({
                "tool": "apply_edl",
                "edl": format!(
                    "*** Begin EDL\n*** Set Motion Scene\n+ scene_json: {scene_json}\n*** End EDL\n"
                ),
                "reasoning": "synthesized from the MotionScene already stored in timeline metadata",
            });
        }
    }
    let mut checks = common_artifact_checks(
        &proposal,
        stored_scene.as_ref().map(|scene| scene.id.as_str()),
    );
    if let Some(scene) = &stored_scene {
        checks.push(check(
            "motion_scene_stored_in_timeline",
            "pass",
            &format!(
                "MotionScene {} is stored in timeline metadata at start_s {} with {} layer(s) — the proposal is already applied",
                scene.id,
                scene.start_s,
                scene.layers.len()
            ),
            "high",
        ));
    }
    match artifact_type.as_str() {
        "quote_highlight" => checks.extend(quote_highlight_checks(&proposal)),
        "animated_list" => checks.extend(animated_list_checks(&proposal)),
        "broll_package" => checks.extend(broll_package_checks(
            &proposal,
            project_root,
            args.require_project_asset_exists,
        )),
        "map_visualization" => checks.extend(map_visualization_checks(&proposal)),
        "counter_stat_graphic" => checks.extend(counter_stat_graphic_checks(&proposal)),
        "search_bar" => checks.extend(search_bar_checks(&proposal)),
        "title_card" => checks.extend(title_card_checks(&proposal)),
        "motion_scene" => checks.extend(motion_scene_artifact_checks(&proposal)),
        _ => checks.push(check(
            "artifact_contract_known",
            "needs_review",
            "artifact-specific verifier is not implemented for this artifact type",
            "medium",
        )),
    }
    let (frame_verification, frame_checks) =
        render_frame_verification(&proposal, args.render_frame_report.as_ref());
    checks.extend(frame_checks);
    let status = aggregate_status(&checks);
    Ok(serde_json::json!({
        "workflow": "visual_support_artifact_verification",
        "artifact_type": artifact_type,
        "status": status,
        "checks": checks,
        "frame_verification_contract": frame_verification_contract(&proposal),
        "frame_verification": frame_verification,
        "render_verification_required": true,
        "next_step": if status == "pass" {
            "artifact-specific checks pass; apply or render the accepted proposal, then run verify_render"
        } else {
            "review failed or incomplete artifact checks before apply_edl/render"
        }
    }))
}

/// Generic checks for a `motion_scene` artifact: the proposal (or the
/// stored scene synthesized into it) must contain a readable
/// MotionScene with at least one renderable layer.
fn motion_scene_artifact_checks(proposal: &serde_json::Value) -> Vec<serde_json::Value> {
    let Some(scene) = motion_scene_from_proposal(proposal) else {
        return vec![check(
            "motion_scene_readable",
            "fail",
            "proposal does not contain a readable MotionScene EDL and no stored scene with its id exists in timeline metadata",
            "high",
        )];
    };
    let text_layer_count = scene
        .layers
        .iter()
        .filter(|layer| {
            layer.kind == montage_proto::professional::MotionSceneLayerKind::Text
                && layer
                    .params
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
        })
        .count();
    vec![
        check(
            "motion_scene_readable",
            "pass",
            &format!(
                "MotionScene {} has {} layer(s) over {}s",
                scene.id,
                scene.layers.len(),
                scene.duration_s
            ),
            "high",
        ),
        check_bool(
            "motion_scene_has_renderable_layers",
            !scene.layers.is_empty(),
            "MotionScene contains renderable layers",
            "MotionScene has no layers",
            "high",
        ),
        check_bool(
            "motion_scene_text_layers_have_content",
            text_layer_count > 0,
            &format!("MotionScene contains {text_layer_count} non-empty text layer(s)"),
            "MotionScene has no non-empty text layers",
            "medium",
        ),
    ]
}

/// Scene id a proposal refers to: parsed from its `apply_edl`
/// Set Motion Scene payload, or named directly via `scene_id`,
/// `scene.id`, `motion_scene.id`, or `motion_scene_id`.
fn proposal_motion_scene_id(proposal: &serde_json::Value) -> Option<String> {
    if let Some(scene) = motion_scene_from_proposal(proposal) {
        return Some(scene.id);
    }
    [
        &proposal["scene_id"],
        &proposal["scene"]["id"],
        &proposal["motion_scene"]["id"],
        &proposal["motion_scene_id"],
    ]
    .into_iter()
    .find_map(|value| value.as_str())
    .map(str::trim)
    .filter(|id| !id.is_empty())
    .map(str::to_string)
}

/// Load the stored MotionScene with `scene_id` from the project's
/// timeline metadata, if present.
fn stored_motion_scene(
    project_root: &Path,
    scene_id: &str,
) -> Option<montage_proto::professional::MotionScene> {
    let project = montage_proto::project::Project::read(project_root).ok()?;
    project
        .timeline
        .metadata
        .montage
        .as_ref()?
        .motion_scenes
        .iter()
        .find(|scene| scene.id == scene_id)
        .cloned()
}

fn render_frame_verification(
    proposal: &serde_json::Value,
    report: Option<&serde_json::Value>,
) -> (serde_json::Value, Vec<serde_json::Value>) {
    let Some(report) = report else {
        return (
            serde_json::json!({
                "status": "not_run",
                "evidence": "no rendered-frame report was provided",
            }),
            Vec::new(),
        );
    };
    let artifact_type = proposal["artifact_type"].as_str().unwrap_or("unknown");
    let reported_artifact = report["artifact_type"].as_str().unwrap_or("unknown");
    let sample_count = report["sample_count"].as_u64().unwrap_or(0);
    let mut checks = vec![
        check_bool(
            "render_frame_artifact_type_matches_proposal",
            reported_artifact == artifact_type,
            "rendered-frame report artifact type matches proposal",
            "rendered-frame report artifact type does not match proposal",
            "high",
        ),
        check_bool(
            "render_frame_samples_present",
            sample_count > 0,
            "rendered-frame report includes sampled frames",
            "rendered-frame report does not include sampled frames",
            "high",
        ),
    ];
    if let Some(report_checks) = report["checks"].as_array() {
        for report_check in report_checks {
            let id = report_check["id"].as_str().unwrap_or("unknown");
            let status = report_check["status"].as_str().unwrap_or("needs_review");
            let evidence = report_check["evidence"]
                .as_str()
                .unwrap_or("rendered-frame report check");
            let severity = report_check["severity"].as_str().unwrap_or("high");
            checks.push(check(
                &format!("render_frame_{id}"),
                status,
                evidence,
                severity,
            ));
        }
    }
    let status = aggregate_status(&checks);
    (
        serde_json::json!({
            "status": status,
            "sample_count": sample_count,
            "artifact_type": reported_artifact,
        }),
        checks,
    )
}

fn frame_verification_contract(proposal: &serde_json::Value) -> serde_json::Value {
    let duration_s = proposal["export_intent"]["duration_s"]
        .as_f64()
        .unwrap_or(3.0);
    let alpha_requested = proposal["export_intent"]["alpha"]
        .as_bool()
        .unwrap_or(false);
    let mut checks = vec![
        serde_json::json!({
            "id": "overlay_visible_in_expected_range",
            "description": "sampled rendered frames show the artifact in the expected timeline range",
            "severity": "high",
        }),
        serde_json::json!({
            "id": "text_readable_in_safe_area",
            "description": "sampled rendered frames keep text readable and inside the declared safe area",
            "severity": "high",
        }),
        serde_json::json!({
            "id": "no_black_or_empty_artifact_frames",
            "description": "sampled rendered frames are not blank or fully black for the artifact window",
            "severity": "high",
        }),
    ];
    if alpha_requested {
        checks.push(serde_json::json!({
            "id": "alpha_channel_present_when_requested",
            "description": "rendered artifact preserves alpha/transparent background intent",
            "severity": "high",
        }));
    }
    serde_json::json!({
        "scope": "rendered_artifact",
        "timeline_object": proposal["preview"]["timeline_object"].as_str().unwrap_or("unknown"),
        "artifact_type": proposal["artifact_type"].as_str().unwrap_or("unknown"),
        "sample_points_s": frame_sample_points(duration_s),
        "checks": checks,
        "evidence_outputs": [
            "visual_support/frame-samples.json",
            "visual_support/frame-contact-sheet.png",
            "visual_support/frame-verification.json",
        ],
        "hide_backend_details": true,
    })
}

fn frame_sample_points(duration_s: f64) -> Vec<f64> {
    let duration_s = duration_s.max(0.5);
    let mut points = vec![0.5_f64.min(duration_s)];
    let midpoint = round3(duration_s / 2.0);
    if !points.iter().any(|point| (*point - midpoint).abs() < 0.001) {
        points.push(midpoint);
    }
    let tail = round3((duration_s - 0.5).max(0.0));
    if !points.iter().any(|point| (*point - tail).abs() < 0.001) {
        points.push(tail);
    }
    points
}

fn common_artifact_checks(
    proposal: &serde_json::Value,
    applied_scene_id: Option<&str>,
) -> Vec<serde_json::Value> {
    let mut checks = Vec::new();
    let has_evidence = proposal["review"]["evidence"]
        .as_array()
        .is_some_and(|rows| !rows.is_empty());
    if has_evidence {
        checks.push(check(
            "proposal_has_evidence",
            "pass",
            "proposal includes review evidence",
            "high",
        ));
    } else if let Some(scene_id) = applied_scene_id {
        // The scene is already stored in the timeline; missing review
        // evidence is worth flagging but must not read as "the scene
        // failed to apply".
        checks.push(check(
            "proposal_has_evidence",
            "needs_review",
            &format!(
                "proposal carries no review evidence, but MotionScene {scene_id} is already stored in timeline metadata; attach transcript evidence for the editorial record"
            ),
            "medium",
        ));
    } else {
        checks.push(check(
            "proposal_has_evidence",
            "fail",
            "proposal is missing review evidence",
            "high",
        ));
    }
    let has_edl = proposal["apply_edl"]["edl"]
        .as_str()
        .is_some_and(|edl| !edl.trim().is_empty());
    if has_edl {
        checks.push(check(
            "proposal_has_apply_edl",
            "pass",
            "proposal includes an apply_edl payload",
            "high",
        ));
    } else if let Some(scene_id) = applied_scene_id {
        checks.push(check(
            "proposal_has_apply_edl",
            "pass",
            &format!(
                "proposal has no apply_edl payload, but MotionScene {scene_id} already exists in timeline metadata — it is applied, not apply-pending"
            ),
            "high",
        ));
    } else {
        checks.push(check(
            "proposal_has_apply_edl",
            "fail",
            "proposal is not apply-ready",
            "high",
        ));
    }
    checks
}

fn quote_highlight_checks(proposal: &serde_json::Value) -> Vec<serde_json::Value> {
    let Some(anchor) = transcript_evidence_anchor(proposal) else {
        return vec![check(
            "quote_matches_transcript_anchor",
            "fail",
            "missing transcript evidence anchor",
            "high",
        )];
    };
    let Some(scene_text) = motion_scene_edl_json(proposal) else {
        return vec![check(
            "motion_scene_contains_quote_text",
            "fail",
            "proposal does not contain a readable MotionScene EDL",
            "high",
        )];
    };
    let normalized_scene = normalize_for_match(&scene_text);
    // The MotionScene renders `concise()`-truncated display text, so compare
    // against that same truncated form rather than the full anchor — a long
    // selection would otherwise never appear verbatim in the scene.
    let normalized_anchor = normalize_for_match(&concise(&anchor));
    vec![
        check_bool(
            "quote_matches_transcript_anchor",
            !normalized_anchor.is_empty() && normalized_scene.contains(&normalized_anchor),
            "quote text is present in the MotionScene text layer",
            "quote text does not match the transcript evidence anchor",
            "high",
        ),
        check_bool(
            "motion_scene_contains_quote_text",
            scene_text.contains('"') && !normalized_anchor.is_empty(),
            "MotionScene preserves quote styling around the selected text",
            "MotionScene does not preserve quote-highlight text styling",
            "medium",
        ),
    ]
}

fn animated_list_checks(proposal: &serde_json::Value) -> Vec<serde_json::Value> {
    let missing_information_empty = proposal["missing_information"]
        .as_array()
        .is_some_and(Vec::is_empty);
    let Some(scene_text) = motion_scene_edl_json(proposal) else {
        return vec![check(
            "animated_list_has_steps",
            "fail",
            "proposal does not contain a readable MotionScene EDL",
            "high",
        )];
    };
    let step_count = scene_text.matches("\"id\":\"step-").count();
    let mut checks = vec![
        check_bool(
            "missing_information_resolved",
            missing_information_empty,
            "animated-list proposal has the list structure required for audit",
            "animated-list proposal still needs list-item clarification",
            "high",
        ),
        check_bool(
            "animated_list_has_steps",
            step_count > 0,
            &format!("MotionScene contains {step_count} animated list step layer(s)"),
            "MotionScene does not contain animated list step layers",
            "high",
        ),
    ];
    let item_check = transcript_evidence_anchor(proposal).map_or_else(
        || {
            check(
                "animated_list_items_match_transcript_anchor",
                "fail",
                "missing transcript evidence anchor",
                "high",
            )
        },
        |anchor| {
            let normalized_scene = normalize_for_match(&scene_text);
            let items = normalized_list_items(&anchor);
            check_bool(
                "animated_list_items_match_transcript_anchor",
                missing_information_empty
                    && !items.is_empty()
                    && contains_in_order(&normalized_scene, &items),
                "animated-list step text preserves transcript evidence items in order",
                "animated-list step text does not preserve transcript evidence items in order",
                "high",
            )
        },
    );
    checks.push(item_check);
    checks
}

fn normalized_list_items(selection: &str) -> Vec<String> {
    list_items(selection)
        .into_iter()
        .map(|item| {
            let normalized = normalize_for_match(&item);
            normalized
                .strip_prefix("and ")
                .unwrap_or(normalized.as_str())
                .to_string()
        })
        .filter(|item| !item.is_empty())
        .collect()
}

fn contains_in_order(text: &str, items: &[String]) -> bool {
    let mut offset = 0;
    for item in items {
        let Some(position) = text[offset..].find(item) else {
            return false;
        };
        offset += position + item.len();
    }
    true
}

fn map_visualization_checks(proposal: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut checks = Vec::new();
    let missing_information_empty = proposal["missing_information"]
        .as_array()
        .is_some_and(Vec::is_empty);
    checks.push(check_bool(
        "missing_information_resolved",
        missing_information_empty,
        "map proposal has the route/location information required for audit",
        "map proposal still needs route/location clarification",
        "high",
    ));
    let Some(anchor) = transcript_evidence_anchor(proposal) else {
        checks.push(check(
            "map_labels_match_transcript_anchor",
            "fail",
            "missing transcript evidence anchor",
            "high",
        ));
        return checks;
    };
    let Some(scene_text) = motion_scene_edl_json(proposal) else {
        checks.push(check(
            "map_labels_match_transcript_anchor",
            "fail",
            "proposal does not contain a readable MotionScene EDL",
            "high",
        ));
        return checks;
    };
    let normalized_scene = normalize_for_match(&scene_text);
    let route_tokens = route_label_tokens(&anchor);
    let labels_match = !route_tokens.is_empty()
        && route_tokens
            .iter()
            .all(|token| normalized_scene.contains(token));
    checks.push(check_bool(
        "map_labels_match_transcript_anchor",
        labels_match,
        "map labels appear in the MotionScene text from transcript evidence",
        "map labels do not match the transcript evidence anchor",
        "high",
    ));
    if let Some(scene) = motion_scene_from_proposal(proposal) {
        checks.push(check_bool(
            "map_has_route_line_slot",
            scene.layers.iter().any(|layer| layer.id == "route-line")
                && scene.layers.iter().any(|layer| layer.id == "map-origin")
                && scene
                    .layers
                    .iter()
                    .any(|layer| layer.id == "map-destination"),
            "map MotionScene includes origin, destination, and route-line slots",
            "map MotionScene is missing origin, destination, or route-line slots",
            "medium",
        ));
    }
    checks
}

fn route_label_tokens(anchor: &str) -> Vec<String> {
    normalize_for_match(anchor)
        .split_whitespace()
        .filter(|token| !matches!(*token, "from" | "to" | "via" | "route" | "map"))
        .map(str::to_string)
        .collect()
}

fn counter_stat_graphic_checks(proposal: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut checks = Vec::new();
    let missing_information_empty = proposal["missing_information"]
        .as_array()
        .is_some_and(Vec::is_empty);
    checks.push(check_bool(
        "missing_information_resolved",
        missing_information_empty,
        "counter/stat proposal has the numeric value required for audit",
        "counter/stat proposal still needs numeric value clarification",
        "high",
    ));
    let Some(anchor) = transcript_evidence_anchor(proposal) else {
        checks.push(check(
            "counter_value_matches_transcript_anchor",
            "fail",
            "missing transcript evidence anchor",
            "high",
        ));
        return checks;
    };
    let Some(scene_text) = motion_scene_edl_json(proposal) else {
        checks.push(check(
            "counter_value_matches_transcript_anchor",
            "fail",
            "proposal does not contain a readable MotionScene EDL",
            "high",
        ));
        return checks;
    };
    let expected_value = number_or_concise(&anchor);
    let normalized_scene = normalize_for_match(&scene_text);
    let normalized_value = normalize_for_match(&expected_value);
    checks.push(check_bool(
        "counter_value_matches_transcript_anchor",
        missing_information_empty
            && !normalized_value.is_empty()
            && normalized_scene.contains(&normalized_value),
        "counter/stat value appears in the MotionScene text from transcript evidence",
        "counter/stat value does not match the transcript evidence anchor",
        "high",
    ));
    if let Some(scene) = motion_scene_from_proposal(proposal) {
        checks.push(check_bool(
            "counter_has_count_up_slot",
            layer_param_bool(&scene, "stat-value", "count_up"),
            "counter/stat MotionScene includes a count-up value slot",
            "counter/stat MotionScene is missing a count-up value slot",
            "medium",
        ));
    }
    checks
}

fn search_bar_checks(proposal: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut checks = Vec::new();
    let missing_information_empty = proposal["missing_information"]
        .as_array()
        .is_some_and(Vec::is_empty);
    checks.push(check_bool(
        "missing_information_resolved",
        missing_information_empty,
        "search-bar proposal has the query required for audit",
        "search-bar proposal still needs query clarification",
        "high",
    ));
    let Some(anchor) = transcript_evidence_anchor(proposal) else {
        checks.push(check(
            "search_query_matches_transcript_anchor",
            "fail",
            "missing transcript evidence anchor",
            "high",
        ));
        return checks;
    };
    let Some(scene_text) = motion_scene_edl_json(proposal) else {
        checks.push(check(
            "search_query_matches_transcript_anchor",
            "fail",
            "proposal does not contain a readable MotionScene EDL",
            "high",
        ));
        return checks;
    };
    let normalized_scene = normalize_for_match(&scene_text);
    let normalized_anchor = normalize_for_match(&anchor);
    checks.push(check_bool(
        "search_query_matches_transcript_anchor",
        missing_information_empty
            && !normalized_anchor.is_empty()
            && normalized_scene.contains(&normalized_anchor),
        "search-bar query text appears in the MotionScene text from transcript evidence",
        "search-bar query text does not match the transcript evidence anchor",
        "high",
    ));
    if let Some(scene) = motion_scene_from_proposal(proposal) {
        checks.push(check_bool(
            "search_has_typed_query_slot",
            layer_param_bool(&scene, "search-query", "typing_reveal"),
            "search-bar MotionScene includes a typed query slot",
            "search-bar MotionScene is missing a typed query slot",
            "medium",
        ));
    }
    checks
}

fn title_card_checks(proposal: &serde_json::Value) -> Vec<serde_json::Value> {
    let Some(anchor) = transcript_evidence_anchor(proposal) else {
        return vec![check(
            "title_text_matches_transcript_anchor",
            "fail",
            "missing transcript evidence anchor",
            "high",
        )];
    };
    let Some(scene_text) = motion_scene_edl_json(proposal) else {
        return vec![check(
            "title_text_matches_transcript_anchor",
            "fail",
            "proposal does not contain a readable MotionScene EDL",
            "high",
        )];
    };
    let normalized_scene = normalize_for_match(&scene_text);
    // The title card renders `concise()`-truncated text, so compare against
    // the same truncated form rather than the full anchor.
    let normalized_anchor = normalize_for_match(&concise(&anchor));
    vec![check_bool(
        "title_text_matches_transcript_anchor",
        !normalized_anchor.is_empty() && normalized_scene.contains(&normalized_anchor),
        "title-card text appears in the MotionScene text from transcript evidence",
        "title-card text does not match the transcript evidence anchor",
        "high",
    )]
}

fn broll_package_checks(
    proposal: &serde_json::Value,
    project_root: Option<&Path>,
    require_project_asset_exists: bool,
) -> Vec<serde_json::Value> {
    let Some(edl) = proposal["apply_edl"]["edl"].as_str() else {
        return vec![check(
            "broll_apply_edl_present",
            "fail",
            "B-roll package is missing apply_edl; answer missing information first",
            "high",
        )];
    };
    let Ok(envelope) = parse(edl) else {
        return vec![check(
            "broll_apply_edl_present",
            "fail",
            "B-roll apply_edl could not be parsed",
            "high",
        )];
    };
    let Some((anchor, asset)) = envelope.ops.iter().find_map(|op| {
        if let EdlOp::InsertBRoll { anchor, asset, .. } = op {
            Some((anchor, asset))
        } else {
            None
        }
    }) else {
        return vec![check(
            "broll_apply_edl_present",
            "fail",
            "B-roll proposal does not apply as InsertBRoll",
            "high",
        )];
    };
    let anchor_text = match anchor {
        crate::edl::Anchor::TranscriptSnippet { text } => text.as_str(),
        _ => "",
    };
    let evidence_anchor = transcript_evidence_anchor(proposal).unwrap_or_default();
    let anchor_matches = !anchor_text.is_empty()
        && normalize_for_match(&evidence_anchor).contains(&normalize_for_match(anchor_text));
    let mut checks = vec![
        check_bool(
            "broll_asset_path_present",
            !asset.trim().is_empty(),
            "B-roll asset path is present",
            "B-roll asset path is missing",
            "high",
        ),
        check_bool(
            "broll_anchor_matches_evidence",
            anchor_matches,
            "B-roll EDL anchor matches transcript evidence",
            "B-roll EDL anchor does not match transcript evidence",
            "high",
        ),
        check_bool(
            "broll_has_source_provenance",
            broll_has_source_provenance(proposal, asset, &evidence_anchor),
            "B-roll proposal includes source provenance tied to the asset and transcript anchor",
            "B-roll proposal is missing source provenance for the asset or transcript anchor",
            "high",
        ),
        check_bool(
            "broll_has_disclosure_policy",
            proposal["source_provenance"]["disclosure_policy"]
                .as_str()
                .is_some_and(|policy| !policy.trim().is_empty()),
            "B-roll proposal includes an editor-facing disclosure policy",
            "B-roll proposal is missing an editor-facing disclosure policy",
            "medium",
        ),
    ];
    if require_project_asset_exists {
        let exists = project_root
            .map(|root| asset_is_file_under_root(root, asset))
            .unwrap_or(false);
        checks.push(check_bool(
            "broll_asset_exists",
            exists,
            "B-roll asset exists under the project root",
            "B-roll asset does not exist under the project root",
            "high",
        ));
    }
    checks
}

/// True only when `asset` resolves to an existing file that stays inside
/// `root`. An absolute path or a `../` traversal can make `root.join(asset)`
/// land on a file outside the project, which would otherwise pass the
/// "exists under the project root" check; canonicalizing both sides and
/// requiring the asset to remain below the root closes that gap.
fn asset_is_file_under_root(root: &Path, asset: &str) -> bool {
    let candidate = root.join(asset);
    let (Ok(canonical_root), Ok(canonical_asset)) = (root.canonicalize(), candidate.canonicalize())
    else {
        return false;
    };
    canonical_asset.starts_with(&canonical_root) && canonical_asset.is_file()
}

fn broll_has_source_provenance(
    proposal: &serde_json::Value,
    asset: &str,
    evidence_anchor: &str,
) -> bool {
    let provenance = &proposal["source_provenance"];
    let asset_matches = provenance["asset"].as_str() == Some(asset);
    let anchor_matches = provenance["transcript_anchor"]
        .as_str()
        .is_some_and(|anchor| {
            normalize_for_match(evidence_anchor).contains(&normalize_for_match(anchor))
        });
    let origin_present = provenance["origin"]
        .as_str()
        .is_some_and(|origin| !origin.trim().is_empty());
    asset_matches && anchor_matches && origin_present
}

fn motion_scene_edl_json(proposal: &serde_json::Value) -> Option<String> {
    serde_json::to_string(&motion_scene_from_proposal(proposal)?).ok()
}

fn motion_scene_from_proposal(
    proposal: &serde_json::Value,
) -> Option<montage_proto::professional::MotionScene> {
    let edl = proposal["apply_edl"]["edl"].as_str()?;
    let envelope = parse(edl).ok()?;
    envelope.ops.into_iter().find_map(|op| {
        if let EdlOp::SetMotionScene { scene } = op {
            Some(scene)
        } else {
            None
        }
    })
}

fn layer_param_bool(
    scene: &montage_proto::professional::MotionScene,
    layer_id: &str,
    param: &str,
) -> bool {
    scene
        .layers
        .iter()
        .find(|layer| layer.id == layer_id)
        .and_then(|layer| layer.params.get(param))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn transcript_evidence_anchor(proposal: &serde_json::Value) -> Option<String> {
    let evidence = proposal["review"]["evidence"].as_array()?;
    for row in evidence {
        if row["kind"] == "transcript" {
            if let Some(anchor) = row["anchor_transcript"].as_str() {
                return Some(anchor.to_string());
            }
            let label = row["label"].as_str()?;
            return label
                .strip_prefix("Selected transcript anchor: ")
                .map(str::to_string);
        }
    }
    None
}

fn check_bool(
    id: &str,
    passed: bool,
    pass_evidence: &str,
    fail_evidence: &str,
    severity: &str,
) -> serde_json::Value {
    if passed {
        check(id, "pass", pass_evidence, severity)
    } else {
        check(id, "fail", fail_evidence, severity)
    }
}

fn check(id: &str, status: &str, evidence: &str, severity: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "status": status,
        "evidence": evidence,
        "severity": severity,
    })
}

fn aggregate_status(checks: &[serde_json::Value]) -> &'static str {
    if checks.iter().any(|check| check["status"] == "fail") {
        "fail"
    } else if checks.iter().any(|check| check["status"] == "needs_review") {
        "needs_review"
    } else {
        "pass"
    }
}

fn normalize_for_match(value: &str) -> String {
    value
        .chars()
        .filter_map(|character| {
            if character.is_ascii_alphanumeric() || character.is_ascii_whitespace() {
                Some(character.to_ascii_lowercase())
            } else {
                None
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn speed_multiplier(instruction: &str) -> Option<f64> {
    if instruction.contains("3x") || instruction.contains("three times") {
        Some(3.0)
    } else if instruction.contains("2x") || instruction.contains("twice") {
        Some(2.0)
    } else if instruction.contains("faster") {
        Some(1.5)
    } else {
        None
    }
}

fn set_json_path(value: &mut serde_json::Value, path: &[&str], replacement: serde_json::Value) {
    let Some((last, parents)) = path.split_last() else {
        *value = replacement;
        return;
    };
    let mut cursor = value;
    for key in parents {
        cursor = &mut cursor[*key];
    }
    cursor[*last] = replacement;
}

fn revise_apply_edl_duration(proposal: &mut serde_json::Value, duration_s: f64) {
    let Some(edl) = proposal["apply_edl"]["edl"].as_str() else {
        return;
    };
    let revised = revise_motion_scene_edl_duration(edl, duration_s)
        .or_else(|| revise_broll_edl_duration(edl, duration_s));
    if let Some(revised) = revised {
        proposal["apply_edl"]["edl"] = serde_json::json!(revised);
    }
}

fn revise_motion_scene_edl_duration(edl: &str, duration_s: f64) -> Option<String> {
    let envelope = parse(edl).ok()?;
    let scene = envelope.ops.iter().find_map(|op| {
        if let EdlOp::SetMotionScene { scene } = op {
            Some(scene)
        } else {
            None
        }
    })?;
    let mut scene = scene.clone();
    scene.duration_s = duration_s;
    for layer in &mut scene.layers {
        if layer.from_s >= duration_s {
            layer.from_s = 0.0;
            layer.duration_s = duration_s;
        } else {
            layer.duration_s = layer.duration_s.min((duration_s - layer.from_s).max(0.1));
        }
    }
    let scene_json = serde_json::to_string(&scene).ok()?;
    let package_edl = envelope
        .ops
        .iter()
        .find_map(|op| {
            if let EdlOp::AddProposalPackage { package } = op {
                Some(proposal_package_edl(package))
            } else {
                None
            }
        })
        .unwrap_or_default();
    Some(format!(
        "*** Begin EDL\n{package_edl}*** Set Motion Scene\n+ scene_json: {scene_json}\n*** End EDL\n"
    ))
}

fn revise_broll_edl_duration(edl: &str, duration_s: f64) -> Option<String> {
    let envelope = parse(edl).ok()?;
    if !envelope
        .ops
        .iter()
        .any(|op| matches!(op, EdlOp::InsertBRoll { .. }))
    {
        return None;
    }
    Some(
        edl.lines()
            .map(|line| {
                if line.trim_start().starts_with("+ duration_s:") {
                    format!("+ duration_s: {duration_s}")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Lane constraint a caller can put on the proposal planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaneConstraint {
    /// Only native MotionScene proposals.
    MotionScene,
    /// Only the B-roll package proposal.
    Broll,
}

fn parse_lane_constraint(lane: Option<&str>) -> Result<Option<LaneConstraint>, String> {
    let Some(lane) = lane.map(str::trim).filter(|lane| !lane.is_empty()) else {
        return Ok(None);
    };
    match lane.to_ascii_lowercase().replace('-', "_").as_str() {
        "motion_scene" | "motionscene" => Ok(Some(LaneConstraint::MotionScene)),
        "broll" | "b_roll" | "broll_package" | "source_backed_broll" => {
            Ok(Some(LaneConstraint::Broll))
        }
        other => Err(format!(
            "plan_visual_support_proposals: unknown lane {other:?}. Supported lanes: \
\"motion_scene\" (native MotionScene proposals only) and \"broll\" (B-roll package only). \
Omit lane to let editorial skills choose."
        )),
    }
}

/// Honor a caller's lane constraint instead of switching lanes.
///
/// Returns the constrained artifact list plus a `lane_constraint` JSON
/// block explaining what the constraint did — including when the
/// skill-matched artifacts had to be replaced to stay in-lane.
fn apply_lane_constraint(
    artifacts: Vec<ArtifactType>,
    lane: Option<LaneConstraint>,
) -> (Vec<ArtifactType>, serde_json::Value) {
    let Some(lane) = lane else {
        return (artifacts, serde_json::Value::Null);
    };
    let matched: Vec<ArtifactType> = artifacts
        .iter()
        .copied()
        .filter(|artifact| match lane {
            LaneConstraint::MotionScene => *artifact != ArtifactType::BrollPackage,
            LaneConstraint::Broll => *artifact == ArtifactType::BrollPackage,
        })
        .collect();
    let (constrained, note) = if matched.is_empty() {
        match lane {
            // A MotionScene can always be planned procedurally; fall
            // back to a quote-highlight scene rather than leaving the
            // lane or returning nothing.
            LaneConstraint::MotionScene => (
                vec![ArtifactType::QuoteHighlight],
                "no MotionScene artifact matched the editorial skills for this selection, so a \
quote-highlight MotionScene was planned to honor the lane constraint"
                    .to_string(),
            ),
            LaneConstraint::Broll => (
                vec![ArtifactType::BrollPackage],
                "no B-roll artifact matched the editorial skills for this selection, so a B-roll \
package was planned to honor the lane constraint (pass broll_asset to make it apply-ready)"
                    .to_string(),
            ),
        }
    } else {
        let dropped = artifacts.len() - matched.len();
        (
            matched,
            if dropped > 0 {
                format!(
                    "{dropped} skill-matched proposal(s) outside the requested lane were dropped"
                )
            } else {
                "all skill-matched proposals already satisfy the lane constraint".to_string()
            },
        )
    };
    let lane_name = match lane {
        LaneConstraint::MotionScene => "motion_scene",
        LaneConstraint::Broll => "broll",
    };
    (
        constrained,
        serde_json::json!({
            "lane": lane_name,
            "honored": true,
            "note": note,
        }),
    )
}

fn artifact_types_for_instances(instances: &[EditorialSkillInstance]) -> Vec<ArtifactType> {
    let mut artifacts = Vec::new();
    for instance in instances {
        if let Some(artifact) = artifact_type_from_str(&instance.artifact_type) {
            if !artifacts.contains(&artifact) {
                artifacts.push(artifact);
            }
        }
    }
    if artifacts.is_empty() {
        artifacts.push(ArtifactType::QuoteHighlight);
        artifacts.push(ArtifactType::BrollPackage);
    }
    artifacts
}

fn artifact_type_from_str(value: &str) -> Option<ArtifactType> {
    match value {
        "quote_highlight" => Some(ArtifactType::QuoteHighlight),
        "animated_list" => Some(ArtifactType::AnimatedList),
        "title_card" => Some(ArtifactType::TitleCard),
        "search_bar" => Some(ArtifactType::SearchBar),
        "counter_stat_graphic" => Some(ArtifactType::CounterStatGraphic),
        "map_visualization" => Some(ArtifactType::MapVisualization),
        "broll_package" => Some(ArtifactType::BrollPackage),
        _ => None,
    }
}

fn proposal_for_artifact(
    artifact: ArtifactType,
    instance: Option<&EditorialSkillInstance>,
    selection: &str,
    request: &str,
    anchor: &str,
    args: &PlanVisualSupportProposalArgs,
) -> serde_json::Value {
    match artifact {
        ArtifactType::BrollPackage => {
            broll_package_proposal(instance, selection, request, anchor, args)
        }
        _ => motion_scene_proposal(artifact, instance, selection, request, anchor, args),
    }
}

fn motion_scene_proposal(
    artifact: ArtifactType,
    instance: Option<&EditorialSkillInstance>,
    selection: &str,
    request: &str,
    anchor: &str,
    args: &PlanVisualSupportProposalArgs,
) -> serde_json::Value {
    let duration_s = args
        .duration_s
        .unwrap_or_else(|| artifact.default_duration_s());
    let scene_id = motion_scene_id(artifact, anchor, selection);
    let scene_start_s = args
        .anchor_timeline_start_s
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(0.0);
    let scene_json = motion_scene_json(
        artifact,
        selection,
        anchor,
        scene_start_s,
        duration_s,
        &scene_id,
    );
    let skill = editorial_skill_json_for_artifact(artifact, instance);
    let missing_information = motion_scene_missing_information(artifact, selection, anchor);
    let supported_preview = missing_information.is_empty();
    let package = proposal_package_for_artifact(artifact, selection, request, anchor, args);
    let package_edl = proposal_package_edl(&package);
    let edl = format!(
        "*** Begin EDL\n{}*** Set Motion Scene\n+ scene_json: {}\n*** End EDL\n",
        package_edl, scene_json
    );
    serde_json::json!({
        "artifact_type": artifact.as_str(),
        "title": title_for_artifact(artifact),
        "editorial_skill": skill,
        "references": reference_contract(args),
        "review": {
            "intent": intent_for_artifact(artifact),
            "rationale": rationale_for_artifact(artifact, selection),
            "confidence": confidence_for_artifact(artifact),
            "risk": "low",
            "evidence": evidence_rows(artifact, selection, request, anchor, args)
        },
        "style_defaults": style_defaults_contract(args),
        "missing_information": missing_information,
        "apply_edl": {
            "tool": "apply_edl",
            "edl": edl,
            "reasoning": format!("Add {} visual support for the selected transcript moment.", title_for_artifact(artifact))
        },
        "preview": {
            "timeline_object": "MotionScene",
            "expected_duration_s": duration_s,
            "supported_preview": supported_preview
        },
        "export_intent": export_intent_contract(args, duration_s),
        "revision": revision_contract(args),
        "verification": verification_steps(artifact, false)
    })
}

fn motion_scene_missing_information(
    artifact: ArtifactType,
    selection: &str,
    anchor: &str,
) -> Vec<serde_json::Value> {
    match artifact {
        ArtifactType::MapVisualization if !contains_place_or_route(selection, anchor) => {
            vec![serde_json::json!({
                "kind": "route_or_location",
                "question": "Which location or route should this map show?",
                "reason": "A map visualization needs an explicit place, pair of places, or route before it can be audited against transcript evidence.",
                "fallback": "Use the transcript phrase, chapter notes, or a supplied reference asset as the location source."
            })]
        }
        ArtifactType::CounterStatGraphic if !contains_counter_value(selection, anchor) => {
            vec![serde_json::json!({
                "kind": "stat_value_or_source",
                "question": "Which number or statistic should this counter show?",
                "reason": "A counter/stat graphic needs an explicit numeric value before it can be audited against transcript evidence.",
                "fallback": "Use the transcript phrase, source notes, or a supplied reference asset as the statistic source."
            })]
        }
        ArtifactType::SearchBar if !contains_search_query(selection, anchor) => {
            vec![serde_json::json!({
                "kind": "search_query",
                "question": "What exact search query should this search bar type?",
                "reason": "A search-bar sequence needs an explicit query before it can be audited against transcript evidence.",
                "fallback": "Use the transcript question, claim, source notes, or a supplied reference asset as the query source."
            })]
        }
        ArtifactType::AnimatedList if !contains_list_structure(selection, anchor) => {
            vec![serde_json::json!({
                "kind": "list_items_or_structure",
                "question": "Which list items should this animated list show?",
                "reason": "An animated list needs at least two explicit list items or a clear sequence before it can be audited against transcript evidence.",
                "fallback": "Use the selected transcript, chapter notes, or provide 2-5 list items."
            })]
        }
        _ => Vec::new(),
    }
}

fn broll_package_proposal(
    instance: Option<&EditorialSkillInstance>,
    selection: &str,
    request: &str,
    anchor: &str,
    args: &PlanVisualSupportProposalArgs,
) -> serde_json::Value {
    let duration_s = args
        .duration_s
        .unwrap_or_else(|| ArtifactType::BrollPackage.default_duration_s());
    let missing_information = if args.broll_asset.is_some() {
        Vec::new()
    } else {
        vec![serde_json::json!({
            "kind": "broll_asset_or_generation_approval",
            "question": "Which source or generated B-roll asset should support this transcript moment?",
            "reason": "An accepted B-roll package needs a project-relative video asset before it can become a timeline object.",
            "fallback": "Choose the moment from transcript flow first. Use find_generated_broll_opportunities only as optional scouting or coverage review, then start_generated_media_job, poll_generated_media_job, and use_generated_media for an accepted anchor."
        })]
    };
    let edl = args.broll_asset.as_ref().map(|asset| {
        let package = proposal_package_for_artifact(
            ArtifactType::BrollPackage,
            selection,
            request,
            anchor,
            args,
        );
        let package_edl = proposal_package_edl(&package);
        format!(
            "*** Begin EDL\n{}*** Insert BRoll\n@@ anchor: transcript_snippet=\"{}\"\n+ asset: {}\n+ duration_s: {}\n+ position: overlay\n*** End EDL\n",
            package_edl,
            snippet_for_edl(anchor),
            asset,
            duration_s
        )
    });
    serde_json::json!({
        "artifact_type": ArtifactType::BrollPackage.as_str(),
        "title": "B-roll package",
        "editorial_skill": editorial_skill_json_for_artifact(ArtifactType::BrollPackage, instance),
        "references": reference_contract(args),
        "review": {
            "intent": "Show concrete visual evidence or cover the spoken moment with footage-like support.",
            "rationale": format!("The selected moment benefits from footage-like support: {}", concise(selection)),
            "confidence": 0.78,
            "risk": if edl.is_some() { "medium" } else { "high" },
            "evidence": evidence_rows(ArtifactType::BrollPackage, selection, request, anchor, args)
        },
        "style_defaults": style_defaults_contract(args),
        "missing_information": missing_information,
        "generation_plan": {
            "tool": "llm_editorial_transcript_pass",
            "prompt_seed": selection,
            "coverage_tool": "find_generated_broll_opportunities",
            "next_tools": ["start_generated_media_job", "poll_generated_media_job", "use_generated_media"]
        },
        "source_provenance": broll_source_provenance(args, anchor),
        "apply_edl": edl.map(|edl| serde_json::json!({
            "tool": "apply_edl",
            "edl": edl,
            "reasoning": "Add B-roll visual support at the selected transcript anchor."
        })),
        "preview": {
            "timeline_object": "InsertBRoll",
            "expected_duration_s": duration_s,
            "supported_preview": args.broll_asset.is_some()
        },
        "export_intent": export_intent_contract(args, duration_s),
        "revision": revision_contract(args),
        "verification": verification_steps(ArtifactType::BrollPackage, true)
    })
}

fn broll_source_provenance(
    args: &PlanVisualSupportProposalArgs,
    anchor: &str,
) -> serde_json::Value {
    let Some(asset) = args.broll_asset.as_deref() else {
        return serde_json::Value::Null;
    };
    let lower = asset.to_ascii_lowercase();
    let origin = if lower.contains("/generated/") || lower.contains("generated") {
        "generated_media_asset"
    } else {
        "project_source_asset"
    };
    serde_json::json!({
        "asset": asset,
        "origin": origin,
        "transcript_anchor": anchor,
        "reference_role": "source_asset",
        "disclosure_policy": "Keep generated/source provenance attached to the accepted B-roll package and disclose generated footage when required by the publishing surface.",
        "audit_policy": "Verify the B-roll asset, transcript anchor, and disclosure policy again after render."
    })
}

fn proposal_package_for_artifact(
    artifact: ArtifactType,
    selection: &str,
    request: &str,
    anchor: &str,
    args: &PlanVisualSupportProposalArgs,
) -> ProposalPackage {
    ProposalPackage {
        id: format!(
            "visual-support-{}-{}",
            artifact.as_str().replace('_', "-"),
            proposal_slug(anchor)
        ),
        area: CapabilityArea::EditorialIntentAndReview,
        status: ReviewStatus::Proposed,
        summary: title_for_artifact(artifact).to_string(),
        evidence: evidence_rows(artifact, selection, request, anchor, args)
            .into_iter()
            .map(evidence_trace_from_row)
            .collect(),
        rollback_refs: vec![format!("transcript:{}", concise(anchor))],
    }
}

fn proposal_package_edl(package: &ProposalPackage) -> String {
    let package_json = serde_json::to_string(package).unwrap_or_else(|_| "{}".to_string());
    format!("*** Add Proposal Package\n+ package_json: {package_json}\n")
}

fn evidence_trace_from_row(row: serde_json::Value) -> EvidenceTrace {
    let source = row["kind"].as_str().unwrap_or("visual_support").to_string();
    let mut refs = Vec::new();
    if let Some(anchor) = row["anchor_transcript"].as_str()
        && !anchor.trim().is_empty()
    {
        refs.push(format!("transcript:{anchor}"));
    }
    if let Some(trigger_kind) = row["trigger_kind"].as_str()
        && !trigger_kind.trim().is_empty()
    {
        refs.push(format!("story_context:{trigger_kind}"));
    }
    EvidenceTrace {
        source,
        refs,
        reason: row["label"].as_str().map(str::to_string),
        confidence: row["confidence"].as_f64(),
    }
}

/// Scene id for a MotionScene proposal.
///
/// Applying `Set Motion Scene` stores scenes with `replace_by_id`, so two
/// proposals of the same artifact type need distinct ids — otherwise a
/// second quote highlight / list / title overwrites the first accepted one.
/// Bind the id to the transcript moment (anchor, falling back to selection)
/// so distinct moments yield distinct scenes while the same moment stays
/// stable across re-plans.
fn motion_scene_id(artifact: ArtifactType, anchor: &str, selection: &str) -> String {
    let artifact_slug = artifact.as_str().replace('_', "-");
    let moment = if anchor.trim().is_empty() {
        selection
    } else {
        anchor
    };
    let fingerprint = stable_short_hash(moment);
    format!("visual-support-{artifact_slug}-{fingerprint}")
}

/// Deterministic short hex fingerprint of `value`, stable across runs (unlike
/// `DefaultHasher`, whose seed is randomized) so re-planning the same moment
/// reproduces the same scene id.
fn stable_short_hash(value: &str) -> String {
    // FNV-1a 64-bit over the normalized moment text.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in normalize_for_match(value).bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn proposal_slug(anchor: &str) -> String {
    let slug = anchor
        .chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() {
                Some(c.to_ascii_lowercase())
            } else if c.is_whitespace() || matches!(c, '-' | '_' | ':') {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>();
    let compact = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    concise(&compact).replace(' ', "-")
}

/// Placement for a delayed layer inside a scene of `scene_duration_s`.
///
/// Returns `(from_s, duration_s)` guaranteeing `from_s > 0`, `duration_s > 0`,
/// and `from_s + duration_s <= scene_duration_s`, which is what
/// `MotionScene::validate` requires. Short scenes (or "make it faster"
/// revisions) get a proportionally compressed entrance instead of the fixed
/// 1s minimum that used to overflow the scene.
fn delayed_layer(desired_from_s: f64, scene_duration_s: f64) -> (f64, f64) {
    let scene = scene_duration_s.max(f64::EPSILON);
    // Never start a layer at/after the scene end; leave a sliver to play.
    let from_s = desired_from_s.clamp(0.0, scene * 0.5);
    let duration_s = (scene - from_s).max(f64::EPSILON);
    (from_s, duration_s)
}

fn motion_scene_json(
    artifact: ArtifactType,
    selection: &str,
    anchor: &str,
    start_s: f64,
    duration_s: f64,
    scene_id: &str,
) -> String {
    let headline = match artifact {
        ArtifactType::QuoteHighlight => quote_text(selection),
        ArtifactType::AnimatedList => "Key points".to_string(),
        ArtifactType::TitleCard => concise(selection),
        ArtifactType::SearchBar => "Search".to_string(),
        ArtifactType::CounterStatGraphic => "By the numbers".to_string(),
        ArtifactType::MapVisualization => "Route map".to_string(),
        ArtifactType::BrollPackage => concise(selection),
    };
    let mut layers = vec![
        serde_json::json!({
            "id": "background-panel",
            "kind": "solid",
            "from_s": 0.0,
            "duration_s": duration_s,
            "z_index": 0,
            "params": {
                "x": 0.08,
                "y": 0.18,
                "width": 0.84,
                "height": 0.64,
                "color": "#101820",
                "opacity": 0.72,
                "animations": [{"parameter": "overlay.opacity", "keyframes": [{"time_s": 0.0, "value": 0.0}, {"time_s": 0.35, "value": 0.72}]}]
            }
        }),
        serde_json::json!({
            "id": "headline",
            "kind": "text",
            "from_s": 0.0,
            "duration_s": duration_s,
            "z_index": 10,
            "params": {
                "text": headline,
                "position": "center",
                "font_size": 64,
                "font_weight": "bold",
                "color": "#f8fafc",
                "animation": "fade_slide_in"
            }
        }),
    ];
    if artifact == ArtifactType::AnimatedList {
        for (index, item) in list_items(selection).into_iter().enumerate() {
            let (from_s, layer_duration) = delayed_layer(0.35 + index as f64 * 0.28, duration_s);
            layers.push(serde_json::json!({
                "id": format!("step-{}", index + 1),
                "kind": "text",
                "from_s": from_s,
                "duration_s": layer_duration,
                "z_index": 20 + index,
                "params": {
                    "text": item,
                    "position": "bottom",
                    "font_size": 42,
                    "font_weight": "normal",
                    "color": "#d7ff6f",
                    "animation": "fade_slide_in"
                }
            }));
        }
    } else if artifact == ArtifactType::SearchBar {
        layers.extend(search_bar_layers(anchor, duration_s));
    } else if artifact == ArtifactType::CounterStatGraphic {
        layers.extend(counter_stat_layers(anchor, duration_s));
    } else if artifact == ArtifactType::MapVisualization {
        layers.extend(map_visualization_layers(anchor, duration_s));
    }
    serde_json::json!({
        "id": scene_id,
        "start_s": start_s,
        "duration_s": duration_s,
        "fps": 30.0,
        "width": 1920,
        "height": 1080,
        "layers": layers,
        "rationale": format!("planned as {}", artifact.as_str())
    })
    .to_string()
}

fn search_bar_layers(anchor: &str, duration_s: f64) -> Vec<serde_json::Value> {
    let (shell_from_s, shell_duration) = delayed_layer(0.25, duration_s);
    let (query_from_s, query_duration) = delayed_layer(0.55, duration_s);
    vec![
        serde_json::json!({
            "id": "search-shell",
            "kind": "shape",
            "from_s": shell_from_s,
            "duration_s": shell_duration,
            "z_index": 18,
            "params": {
                "shape": "rounded_rect",
                "x": 0.18,
                "y": 0.46,
                "width": 0.64,
                "height": 0.12,
                "radius": 0.035,
                "color": "#f8fafc",
                "opacity": 0.96,
                "animation": "scale_fade_in"
            }
        }),
        serde_json::json!({
            "id": "search-query",
            "kind": "text",
            "from_s": query_from_s,
            "duration_s": query_duration,
            "z_index": 22,
            "params": {
                "text": concise(anchor),
                "position": "center",
                "font_size": 46,
                "font_weight": "medium",
                "color": "#101820",
                "typing_reveal": true,
                "animation": "type_on"
            }
        }),
    ]
}

fn counter_stat_layers(anchor: &str, duration_s: f64) -> Vec<serde_json::Value> {
    let value = number_or_concise(anchor);
    let label = counter_label(anchor, &value);
    let (value_from_s, value_duration) = delayed_layer(0.2, duration_s);
    let (label_from_s, label_duration) = delayed_layer(0.55, duration_s);
    vec![
        serde_json::json!({
            "id": "stat-value",
            "kind": "text",
            "from_s": value_from_s,
            "duration_s": value_duration,
            "z_index": 24,
            "params": {
                "text": value,
                "position": "center",
                "font_size": 120,
                "font_weight": "bold",
                "color": "#d7ff6f",
                "count_up": true,
                "animation": "scale_fade_in"
            }
        }),
        serde_json::json!({
            "id": "stat-label",
            "kind": "text",
            "from_s": label_from_s,
            "duration_s": label_duration,
            "z_index": 25,
            "params": {
                "text": label,
                "position": "bottom",
                "font_size": 38,
                "font_weight": "medium",
                "color": "#f8fafc",
                "animation": "fade_slide_in"
            }
        }),
    ]
}

fn map_visualization_layers(anchor: &str, duration_s: f64) -> Vec<serde_json::Value> {
    let (origin, destination) = route_endpoints(anchor);
    let (line_from_s, line_duration) = delayed_layer(0.35, duration_s);
    let (origin_from_s, origin_duration) = delayed_layer(0.25, duration_s);
    let (dest_from_s, dest_duration) = delayed_layer(0.55, duration_s);
    vec![
        serde_json::json!({
            "id": "route-line",
            "kind": "shape",
            "from_s": line_from_s,
            "duration_s": line_duration,
            "z_index": 18,
            "params": {
                "shape": "line",
                "x1": 0.28,
                "y1": 0.58,
                "x2": 0.72,
                "y2": 0.42,
                "stroke": "#d7ff6f",
                "stroke_width": 8,
                "animation": "draw_on"
            }
        }),
        serde_json::json!({
            "id": "map-origin",
            "kind": "text",
            "from_s": origin_from_s,
            "duration_s": origin_duration,
            "z_index": 22,
            "params": {
                "text": origin,
                "position": "left",
                "font_size": 44,
                "font_weight": "bold",
                "color": "#f8fafc",
                "animation": "fade_slide_in"
            }
        }),
        serde_json::json!({
            "id": "map-destination",
            "kind": "text",
            "from_s": dest_from_s,
            "duration_s": dest_duration,
            "z_index": 22,
            "params": {
                "text": destination,
                "position": "right",
                "font_size": 44,
                "font_weight": "bold",
                "color": "#f8fafc",
                "animation": "fade_slide_in"
            }
        }),
    ]
}

fn counter_label(anchor: &str, value: &str) -> String {
    let label = anchor.replace(value, "");
    let label = label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .trim_matches(|character: char| character == '-' || character == ':' || character == ',')
        .trim()
        .to_string();
    if label.is_empty() {
        "Key statistic".into()
    } else {
        concise(&label)
    }
}

fn route_endpoints(anchor: &str) -> (String, String) {
    let normalized = anchor.trim().trim_end_matches('.');
    let lower = normalized.to_ascii_lowercase();
    if let Some(index) = lower.find(" to ") {
        let origin = normalized[..index]
            .trim()
            .trim_start_matches("from ")
            .trim_start_matches("From ")
            .trim();
        let destination = normalized[index + 4..].trim();
        return (concise(origin), concise(destination));
    }
    let tokens = route_label_tokens(normalized);
    if tokens.len() >= 2 {
        return (tokens[0].clone(), tokens[1..].join(" "));
    }
    (concise(normalized), "Destination".into())
}

fn evidence_rows(
    artifact: ArtifactType,
    selection: &str,
    request: &str,
    anchor: &str,
    args: &PlanVisualSupportProposalArgs,
) -> Vec<serde_json::Value> {
    let mut rows = vec![
        serde_json::json!({
            "kind": "transcript",
            "label": format!("Selected transcript anchor: {}", concise(anchor)),
            "anchor_transcript": anchor,
            "confidence": 0.92,
            "confidence_level": "high"
        }),
        serde_json::json!({
            "kind": "visual",
            "label": rationale_for_artifact(artifact, selection),
            "confidence": confidence_for_artifact(artifact),
            "confidence_level": "high"
        }),
        serde_json::json!({
            "kind": "pacing",
            "label": format!("Editor request: {}", concise(request)),
            "confidence": 0.74,
            "confidence_level": "medium"
        }),
    ];
    rows.extend(story_context_evidence_rows(args));
    rows.extend(reference_evidence_rows(args));
    rows
}

fn story_context_evidence_rows(args: &PlanVisualSupportProposalArgs) -> Vec<serde_json::Value> {
    args.story_context
        .iter()
        .filter_map(|signal| {
            let kind = signal.kind.trim();
            let label = signal.label.trim();
            if kind.is_empty() && label.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "kind": "story_context",
                "trigger_kind": kind,
                "label": if label.is_empty() { kind } else { label },
                "anchor_transcript": signal.transcript,
                "timeline_start_s": signal.start_s,
                "timeline_end_s": signal.end_s,
                "confidence": 0.76,
                "confidence_level": "medium"
            }))
        })
        .collect()
}

fn editorial_skill_json_for_artifact(
    artifact: ArtifactType,
    instance: Option<&EditorialSkillInstance>,
) -> serde_json::Value {
    let registry = EditorialSkillRegistry::bundled();
    let definition = instance
        .and_then(|instance| registry.definition(&instance.skill_id))
        .or_else(|| registry.definition_for_artifact(artifact.as_str()));
    let mut value = definition
        .map(definition_json)
        .unwrap_or_else(|| serde_json::json!({ "artifact_type": artifact.as_str() }));
    if let Some(instance) = instance {
        value["instance"] = instance_json(instance);
    }
    value
}

fn editorial_skill_registry_summary(registry: &EditorialSkillRegistry) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "architecture": "hybrid",
        "definitions": registry.definitions().iter().map(definition_json).collect::<Vec<_>>(),
        "definition_instance_split": true,
        "rust_owns": [
            "registration",
            "triggering",
            "ranking",
            "composition",
            "verification contracts",
            "proposal generation interfaces"
        ],
        "skill_md_owns": [
            "editorial reasoning",
            "examples",
            "best practices",
            "style guidance",
            "reusable prompts"
        ]
    })
}

fn reference_contract(args: &PlanVisualSupportProposalArgs) -> serde_json::Value {
    serde_json::json!({
        "assets": args.reference_assets.clone(),
        "items": reference_items(args),
        "accepted_kinds": ["image", "screenshot", "clip", "style_example", "source_asset"],
        "policy": if args.reference_assets.is_empty() {
            "No reference supplied; use transcript and project style defaults."
        } else {
            "Use references as style/source evidence without detaching the artifact from the timeline anchor."
        }
    })
}

fn reference_items(args: &PlanVisualSupportProposalArgs) -> Vec<serde_json::Value> {
    args.reference_assets
        .iter()
        .map(|asset| {
            let kind = reference_kind(asset);
            let role = reference_role(asset, kind);
            serde_json::json!({
                "path": asset,
                "kind": kind,
                "role": role,
                "style_tokens": reference_style_tokens(asset),
                "inspection": {
                    "status": "inferred_from_reference_path",
                    "limitations": "Style tokens come from the asset filename/path; pixel-level reference inspection is not performed here."
                },
                "influence": reference_influence(role),
                "audit_policy": "Reference informs style/source choice but transcript anchor remains the primary editorial evidence."
            })
        })
        .collect()
}

fn reference_style_tokens(asset: &str) -> Vec<String> {
    let Some(name) = std::path::Path::new(asset)
        .file_stem()
        .and_then(|stem| stem.to_str())
    else {
        return Vec::new();
    };
    let mut tokens = Vec::new();
    for token in name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() > 2)
    {
        if !matches!(token.as_str(), "ref" | "reference" | "style" | "asset")
            && !tokens.contains(&token)
        {
            tokens.push(token);
        }
    }
    tokens
}

fn reference_evidence_rows(args: &PlanVisualSupportProposalArgs) -> Vec<serde_json::Value> {
    reference_items(args)
        .into_iter()
        .map(|item| {
            let asset = item["path"].as_str().unwrap_or_default();
            let role = item["role"].as_str().unwrap_or_default();
            let style_tokens = item["style_tokens"].clone();
            serde_json::json!({
                "kind": "reference",
                "asset": asset,
                "usage": role,
                "label": format!("Reference asset ({role}): {asset}"),
                "style_tokens": style_tokens,
                "confidence": 0.68,
                "confidence_level": "medium"
            })
        })
        .collect()
}

fn reference_kind(asset: &str) -> &'static str {
    let lower = asset.to_ascii_lowercase();
    if lower.ends_with(".mp4")
        || lower.ends_with(".mov")
        || lower.ends_with(".m4v")
        || lower.ends_with(".webm")
    {
        "clip"
    } else if lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".jpeg")
        || lower.ends_with(".webp")
        || lower.ends_with(".gif")
    {
        "image"
    } else {
        "reference"
    }
}

fn reference_role(asset: &str, kind: &str) -> &'static str {
    let lower = asset.to_ascii_lowercase();
    if kind == "clip" || lower.contains("source") || lower.contains("evidence") {
        "source_asset"
    } else {
        "style_example"
    }
}

fn reference_influence(role: &str) -> &'static str {
    match role {
        "source_asset" => {
            "Use as source/provenance evidence when the artifact needs factual or footage-like support."
        }
        _ => "Use for visual style, layout, color, typography, or motion pacing guidance.",
    }
}

fn export_intent_contract(
    args: &PlanVisualSupportProposalArgs,
    duration_s: f64,
) -> serde_json::Value {
    serde_json::json!({
        "aspect_ratio": args.aspect_ratio.as_deref().unwrap_or("project"),
        "platform": args.platform.as_deref().unwrap_or("project"),
        "alpha": args.alpha.unwrap_or(false),
        "duration_s": duration_s,
        "visual_intensity": args.motion_intensity.as_deref().unwrap_or("standard"),
        "preview_required": true,
        "render_verification_required": true,
        "limitations": [
            "backend details stay hidden from the editor",
            "artifact-specific verification is best-effort until deeper frame checks land"
        ]
    })
}

fn style_defaults_contract(args: &PlanVisualSupportProposalArgs) -> serde_json::Value {
    serde_json::json!({
        "typography": args.typography.as_deref().unwrap_or("project"),
        "color_palette": args.color_palette.as_deref().unwrap_or("project"),
        "safe_area_policy": args.safe_area_policy.as_deref().unwrap_or("project"),
        "brand_package": args.brand_package.as_deref().unwrap_or("project")
    })
}

fn revision_contract(args: &PlanVisualSupportProposalArgs) -> serde_json::Value {
    serde_json::json!({
        "supported": true,
        "instruction": args.revision_instruction.as_deref().unwrap_or(""),
        "examples": [
            "make it shorter",
            "make it 3x faster",
            "make the glow less intense",
            "use transparent background",
            "switch this to B-roll"
        ],
        "diff_policy": "Return a revised proposal and describe timeline/export-intent changes before apply_edl."
    })
}

fn verification_steps(artifact: ArtifactType, generated_media: bool) -> serde_json::Value {
    let mut after_acceptance = vec![
        "view_timeline confirms the accepted visual support object is present at the transcript anchor",
        "preview the affected timeline region in desktop or preview cache",
        "start_render renders the timeline or guide section",
        "verify_render checks the exported artifact and render manifest",
    ];
    let artifact_specific = artifact_specific_verification(artifact);
    if generated_media {
        after_acceptance.push(
            "confirm generated-media provenance, disclosure fields, and transcript anchor still match",
        );
    }
    serde_json::json!({
        "after_acceptance": after_acceptance,
        "artifact_specific": artifact_specific,
        "done_definition": "artifact is present on the timeline, previewable, rendered, and verified against the render manifest"
    })
}

fn artifact_specific_verification(artifact: ArtifactType) -> Vec<&'static str> {
    match artifact {
        ArtifactType::QuoteHighlight => vec![
            "quote text matches the selected transcript anchor",
            "overlay remains readable in the expected timeline range",
        ],
        ArtifactType::AnimatedList => vec![
            "list item order matches the selected transcript structure",
            "animation duration matches export intent",
        ],
        ArtifactType::TitleCard => vec!["chapter title matches the selected topic boundary"],
        ArtifactType::SearchBar => vec!["typed query text matches the selected question or claim"],
        ArtifactType::CounterStatGraphic => {
            vec!["counter/stat text matches the selected number or claim"]
        }
        ArtifactType::MapVisualization => {
            vec!["route/map labels match the selected place or route"]
        }
        ArtifactType::BrollPackage => vec![
            "B-roll asset path/provenance is present",
            "B-roll anchor still matches the transcript moment",
        ],
    }
}

fn title_for_artifact(artifact: ArtifactType) -> &'static str {
    match artifact {
        ArtifactType::QuoteHighlight => "Quote highlight",
        ArtifactType::AnimatedList => "Animated list",
        ArtifactType::TitleCard => "Title card",
        ArtifactType::SearchBar => "Search bar",
        ArtifactType::CounterStatGraphic => "Counter/stat graphic",
        ArtifactType::MapVisualization => "Map visualization",
        ArtifactType::BrollPackage => "B-roll package",
    }
}

fn intent_for_artifact(artifact: ArtifactType) -> &'static str {
    match artifact {
        ArtifactType::QuoteHighlight => {
            "Make the selected quote land without changing the edit meaning."
        }
        ArtifactType::AnimatedList => {
            "Preview or summarize a sequence so the viewer can track the structure."
        }
        ArtifactType::TitleCard => "Introduce a chapter or topic boundary.",
        ArtifactType::SearchBar => "Turn a question or query into a readable motion prompt.",
        ArtifactType::CounterStatGraphic => "Make a number or scale claim visually legible.",
        ArtifactType::MapVisualization => {
            "Show a location or route without exposing the rendering backend."
        }
        ArtifactType::BrollPackage => "Add footage-like support for the spoken moment.",
    }
}

fn rationale_for_artifact(artifact: ArtifactType, selection: &str) -> String {
    match artifact {
        ArtifactType::QuoteHighlight => {
            format!(
                "The selected quote is short enough to highlight directly: {}",
                concise(selection)
            )
        }
        ArtifactType::AnimatedList => {
            format!(
                "The selected text reads like a sequence or list: {}",
                concise(selection)
            )
        }
        ArtifactType::TitleCard => {
            format!(
                "The selected text can introduce a topic boundary: {}",
                concise(selection)
            )
        }
        ArtifactType::SearchBar => {
            format!(
                "The selected text can be framed as a typed search/query: {}",
                concise(selection)
            )
        }
        ArtifactType::CounterStatGraphic => {
            format!(
                "The selected text contains a number, scale, or claim: {}",
                concise(selection)
            )
        }
        ArtifactType::MapVisualization => {
            format!(
                "The selected text references a place or route: {}",
                concise(selection)
            )
        }
        ArtifactType::BrollPackage => {
            format!(
                "The selected moment can be supported with footage-like evidence: {}",
                concise(selection)
            )
        }
    }
}

fn confidence_for_artifact(artifact: ArtifactType) -> f64 {
    match artifact {
        ArtifactType::QuoteHighlight => 0.86,
        ArtifactType::AnimatedList => 0.84,
        ArtifactType::TitleCard => 0.80,
        ArtifactType::SearchBar => 0.76,
        ArtifactType::CounterStatGraphic => 0.82,
        ArtifactType::MapVisualization => 0.74,
        ArtifactType::BrollPackage => 0.78,
    }
}

fn list_items(selection: &str) -> Vec<String> {
    let mut items: Vec<String> = selection
        .split([',', ';'])
        .map(|item| item.trim().trim_matches('.').to_string())
        .filter(|item| !item.is_empty())
        .collect();
    if items.len() < 2 {
        items = selection
            .split(" and ")
            .map(|item| item.trim().trim_matches('.').to_string())
            .filter(|item| !item.is_empty())
            .collect();
    }
    if items.len() < 2 {
        items = vec![concise(selection)];
    }
    items.truncate(5);
    items
}

fn quote_text(selection: &str) -> String {
    format!("\"{}\"", concise(selection))
}

fn number_or_concise(selection: &str) -> String {
    let number: String = selection
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '%' || *c == '$' || *c == '.')
        .collect();
    if number.trim().is_empty() {
        concise(selection)
    } else {
        number
    }
}

fn contains_counter_value(selection: &str, anchor: &str) -> bool {
    format!("{selection} {anchor}")
        .chars()
        .any(|c| c.is_ascii_digit())
}

fn contains_search_query(selection: &str, anchor: &str) -> bool {
    let combined = format!("{selection} {anchor}");
    let lower = combined.to_ascii_lowercase();
    combined.contains('?')
        || lower.starts_with("how ")
        || lower.starts_with("what ")
        || lower.starts_with("why ")
        || lower.starts_with("who ")
        || lower.starts_with("where ")
        || lower.starts_with("when ")
        || lower.contains(" vs ")
        || lower.contains(" versus ")
}

fn contains_list_structure(selection: &str, anchor: &str) -> bool {
    let combined = format!("{selection} {anchor}");
    let lower = combined.to_ascii_lowercase();
    combined.contains(',')
        || combined.contains(';')
        || lower.contains(" and ")
        || lower.contains(" first ")
        || lower.contains(" second ")
        || lower.contains(" third ")
        || lower.contains(" steps")
        || lower.contains("list")
        || lower.contains("framework")
        || lower.contains("cover ")
}

fn contains_place_or_route(selection: &str, anchor: &str) -> bool {
    let combined = format!("{selection} {anchor}");
    let lower = combined.to_ascii_lowercase();
    if lower.contains(" to ") || lower.contains(" from ") || lower.contains(" via ") {
        return true;
    }
    combined.split_whitespace().any(|word| {
        let candidate = word.trim_matches(|c: char| !c.is_alphabetic());
        candidate.len() > 2
            && !matches!(
                candidate,
                "Show" | "Make" | "Route" | "Map" | "The" | "This" | "That"
            )
            && candidate.chars().next().is_some_and(char::is_uppercase)
    })
}

fn concise(value: &str) -> String {
    let words: Vec<&str> = value.split_whitespace().take(12).collect();
    let mut out = words.join(" ");
    if value.split_whitespace().count() > words.len() {
        out.push_str("...");
    }
    out
}

/// Serialize a transcript anchor for a quoted `transcript_snippet="..."`
/// EDL field.
///
/// The EDL parser uses the surrounding ASCII double quotes as the field
/// delimiter and strips only that outer pair without unescaping inner
/// sequences, so an embedded ASCII `"` cannot round-trip — escaping it as
/// `\"` leaves a literal backslash in the parsed snippet that then fails to
/// resolve against the transcript. Map embedded ASCII double quotes to
/// typographic quotes instead: those carry through the parser untouched, and
/// anchor resolution's `UnicodeNorm` tier folds `\u{201c}`/`\u{201d}` back to
/// `"`, so the snippet still matches transcripts written with either quote
/// style.
fn snippet_for_edl(value: &str) -> String {
    value.replace('"', "\u{201c}")
}

pub const DESCRIPTION: &str = "\
Read-only Proposal-to-Visual-Support planner. Given selected transcript/topic \
text and an editor request, returns reviewable visual artifact proposals with \
evidence, missing-information prompts, apply_edl payloads, preview expectations, \
and render-verification steps. Use before apply_edl for quote highlights, \
animated lists, title cards, search bars, counters, maps, and B-roll packages.";

#[cfg(test)]
mod tests {
    use std::process::Command;

    use montage_proto::montage_meta::{Anchor, MontageClipMetadata};
    use montage_proto::otio::{
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
        Track, TrackChild, TrackKind,
    };
    use montage_proto::project::Project;

    use super::{
        PlanVisualSupportProposalArgs, ReviseVisualSupportProposalArgs,
        SaveVisualSupportDefaultsArgs, StoryContextSignalArgs, VerifyVisualSupportArtifactArgs,
        plan_visual_support_proposals, revise_visual_support_proposal, run,
        save_visual_support_defaults, verify_visual_support_artifact,
    };
    use crate::edl::{EdlOp, parse};
    use crate::montage_mcp::context::McpToolCtx;
    use crate::montage_mcp::tools::apply_edl::{self, ApplyEdlArgs};
    use crate::montage_mcp::tools::start_render::{self, StartRenderArgs};
    use crate::montage_mcp::tools::verify_render::{self, VerifyRenderArgs};

    fn proposal_titles(value: &serde_json::Value) -> Vec<String> {
        value["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|proposal| proposal["artifact_type"].as_str().unwrap().to_string())
            .collect()
    }

    fn proposal_edl(proposal: &serde_json::Value) -> &str {
        proposal["apply_edl"]["edl"].as_str().unwrap()
    }

    fn proposal_by_type<'a>(
        value: &'a serde_json::Value,
        artifact_type: &str,
    ) -> &'a serde_json::Value {
        value["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|proposal| proposal["artifact_type"] == artifact_type)
            .unwrap()
    }

    fn motion_scene_for(proposal: &serde_json::Value) -> montage_proto::professional::MotionScene {
        let envelope = parse(proposal_edl(proposal)).unwrap();
        envelope
            .ops
            .into_iter()
            .find_map(|op| {
                if let EdlOp::SetMotionScene { scene } = op {
                    Some(scene)
                } else {
                    None
                }
            })
            .unwrap()
    }

    fn ctx_at(path: &std::path::Path) -> McpToolCtx {
        McpToolCtx {
            project_root: path.to_path_buf(),
        }
    }

    fn clip(id: &str, asset: &str, transcript_snippet: &str, duration_s: f64) -> Clip {
        let mut clip = Clip::empty(id.to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new(asset.to_string()));
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(duration_s * 24.0, 24.0),
        ));
        clip.metadata = ClipMetadata {
            montage: Some(MontageClipMetadata {
                anchor: Some(Anchor {
                    transcript_snippet: Some(transcript_snippet.to_string()),
                    ..Anchor::default()
                }),
                ..MontageClipMetadata::default()
            }),
            ..ClipMetadata::default()
        };
        clip
    }

    fn project_with_media() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();

        let raw_dir = dir.path().join("raw");
        let generated_dir = raw_dir.join("generated");
        std::fs::create_dir_all(&generated_dir).unwrap();
        synthesize_fixture(&raw_dir.join("source.mp4"), 2.0).unwrap();
        synthesize_fixture(&generated_dir.join("crowded-market.mp4"), 1.0).unwrap();

        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip(
            "clip-0",
            "raw/source.mp4",
            "quote that should land harder hooks retention export checks crowded market",
            2.0,
        )));
        project
            .timeline
            .tracks
            .children
            .push(StackChild::Track(track));
        project.write(dir.path()).unwrap();
        dir
    }

    fn synthesize_fixture(path: &std::path::Path, duration_s: f64) -> std::io::Result<()> {
        let status = Command::new(montage_render::ffmpeg_path().unwrap())
            .arg("-loglevel")
            .arg("error")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg(format!("testsrc=size=160x90:rate=24:duration={duration_s}"))
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg(format!(
                "sine=frequency=440:sample_rate=44100:duration={duration_s}"
            ))
            .arg("-shortest")
            .arg("-c:v")
            .arg("libx264")
            .arg("-preset")
            .arg("ultrafast")
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-c:a")
            .arg("aac")
            .arg(path)
            .status()?;
        assert!(status.success());
        Ok(())
    }

    #[test]
    fn quote_selection_returns_reviewable_quote_highlight_proposal() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the quote that should land harder.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("quote that should land harder".into()),
            broll_asset: None,
            duration_s: Some(4.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        assert!(proposal_titles(&value).contains(&"quote_highlight".to_string()));
        let proposal = &value["proposals"][0];
        assert_eq!(proposal["review"]["risk"], "low");
        assert!(proposal["review"]["evidence"].as_array().unwrap().len() >= 2);
        let envelope = parse(proposal_edl(proposal)).unwrap();
        assert!(matches!(
            envelope.ops.as_slice(),
            [
                EdlOp::AddProposalPackage { .. },
                EdlOp::SetMotionScene { .. }
            ]
        ));
        assert!(
            proposal["verification"]["after_acceptance"]
                .as_array()
                .unwrap()
                .iter()
                .any(|step| { step.as_str().unwrap().contains("view_timeline") })
        );
    }

    #[test]
    fn visual_support_apply_edl_carries_proposal_inspector_package() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the quote that should land harder.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("quote that should land harder".into()),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposal = proposal_by_type(&value, "quote_highlight");
        let envelope = parse(proposal_edl(proposal)).unwrap();
        let [
            EdlOp::AddProposalPackage { package },
            EdlOp::SetMotionScene { .. },
        ] = envelope.ops.as_slice()
        else {
            panic!("expected inspector package followed by timeline artifact")
        };

        assert_eq!(package.summary, "Quote highlight");
        assert!(!package.evidence.is_empty());
        assert!(package.evidence.iter().any(|evidence| {
            evidence.source == "transcript"
                && evidence
                    .refs
                    .iter()
                    .any(|reference| reference.contains("quote that should land harder"))
        }));
    }

    #[test]
    fn list_selection_returns_animated_list_motion_scene() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "We will cover hooks, retention, and export checks.".into(),
            request: "turn this into an animated list".into(),
            anchor_transcript: Some("hooks, retention, and export checks".into()),
            broll_asset: None,
            duration_s: None,
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        assert!(proposal_titles(&value).contains(&"animated_list".to_string()));
        let proposal = value["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|proposal| proposal["artifact_type"] == "animated_list")
            .unwrap();
        let envelope = parse(proposal_edl(proposal)).unwrap();
        let [
            EdlOp::AddProposalPackage { .. },
            EdlOp::SetMotionScene { scene },
        ] = envelope.ops.as_slice()
        else {
            panic!("expected SetMotionScene proposal")
        };
        assert!(scene.layers.iter().any(|layer| layer.id == "step-1"));
        assert!(
            proposal["review"]["rationale"]
                .as_str()
                .unwrap()
                .contains("sequence")
        );
    }

    #[test]
    fn broll_request_with_asset_returns_apply_ready_package() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "The founder describes a crowded market.".into(),
            request: "add a b-roll package here".into(),
            anchor_transcript: Some("crowded market".into()),
            broll_asset: Some("raw/generated/crowded-market.mp4".into()),
            duration_s: Some(3.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        assert!(proposal_titles(&value).contains(&"broll_package".to_string()));
        let proposal = value["proposals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|proposal| proposal["artifact_type"] == "broll_package")
            .unwrap();
        let envelope = parse(proposal_edl(proposal)).unwrap();
        let [
            EdlOp::AddProposalPackage { .. },
            EdlOp::InsertBRoll {
                asset, duration_s, ..
            },
        ] = envelope.ops.as_slice()
        else {
            panic!("expected InsertBRoll proposal")
        };
        assert_eq!(asset, "raw/generated/crowded-market.mp4");
        assert_eq!(*duration_s, 3.0);
        assert!(
            proposal["missing_information"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn broll_quoted_anchor_round_trips_through_edl_parser() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "She said \"stripe is great\" on stage.".into(),
            request: "add a b-roll package here".into(),
            anchor_transcript: Some("she said \"stripe is great\"".into()),
            broll_asset: Some("raw/generated/crowded-market.mp4".into()),
            duration_s: Some(3.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposal = proposal_by_type(&value, "broll_package");
        let envelope = parse(proposal_edl(proposal)).unwrap();
        let anchor = envelope
            .ops
            .iter()
            .find_map(|op| match op {
                EdlOp::InsertBRoll { anchor, .. } => Some(anchor),
                _ => None,
            })
            .expect("InsertBRoll op");
        let crate::edl::Anchor::TranscriptSnippet { text } = anchor else {
            panic!("expected transcript snippet anchor, got {anchor:?}");
        };
        assert!(
            !text.contains('\\'),
            "parsed B-roll anchor still carries an escape backslash: {text:?}"
        );
        assert_eq!(text, "she said \u{201c}stripe is great\u{201c}");

        // The serialized snippet still resolves against a transcript written
        // with ASCII quotes, because the UnicodeNorm tier folds typographic
        // quotes back to ASCII before matching.
        use crate::edl::anchor::{AnchorContext, resolve};
        use montage_proto::otio::{StackChild, Timeline, Track, TrackChild, TrackKind};
        let mut track = Track::empty("V1", TrackKind::Video);
        track.children.push(TrackChild::Clip(clip(
            "clip-q",
            "raw/source.mp4",
            "before she said \"stripe is great\" on stage after",
            4.0,
        )));
        let mut timeline = Timeline::empty("timeline");
        timeline.tracks.children.push(StackChild::Track(track));
        resolve(&timeline, anchor, &AnchorContext::empty())
            .expect("quote-free anchor should resolve against the quoted transcript");
    }

    #[test]
    fn map_request_without_place_asks_tight_clarification() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Show the route we discussed.".into(),
            request: "make this a route map".into(),
            anchor_transcript: Some("route we discussed".into()),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposal = proposal_by_type(&value, "map_visualization");
        assert!(
            proposal["missing_information"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["kind"] == "route_or_location"
                    && item["question"]
                        .as_str()
                        .unwrap()
                        .contains("location or route"))
        );
        assert_eq!(proposal["preview"]["supported_preview"], false);
    }

    #[test]
    fn map_request_with_place_does_not_ask_clarification() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Show the route from Lagos to Nairobi.".into(),
            request: "make this a route map".into(),
            anchor_transcript: Some("Lagos to Nairobi".into()),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposal = proposal_by_type(&value, "map_visualization");
        assert!(
            proposal["missing_information"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(proposal["preview"]["supported_preview"], true);
    }

    #[test]
    fn counter_request_without_number_asks_tight_clarification() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "The market changed dramatically this quarter.".into(),
            request: "turn this into a statistic counter".into(),
            anchor_transcript: Some("market changed dramatically".into()),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposal = proposal_by_type(&value, "counter_stat_graphic");
        assert!(
            proposal["missing_information"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["kind"] == "stat_value_or_source"
                    && item["question"]
                        .as_str()
                        .unwrap()
                        .contains("number or statistic"))
        );
        assert_eq!(proposal["preview"]["supported_preview"], false);
    }

    #[test]
    fn search_bar_without_query_asks_tight_clarification() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the thing we should look up.".into(),
            request: "turn this into a search bar sequence".into(),
            anchor_transcript: Some("thing we should look up".into()),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposal = proposal_by_type(&value, "search_bar");
        assert!(
            proposal["missing_information"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["kind"] == "search_query"
                    && item["question"].as_str().unwrap().contains("search query"))
        );
        assert_eq!(proposal["preview"]["supported_preview"], false);
    }

    #[test]
    fn animated_list_without_list_items_asks_tight_clarification() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This moment needs more structure.".into(),
            request: "turn this into an animated list".into(),
            anchor_transcript: Some("moment needs more structure".into()),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposal = proposal_by_type(&value, "animated_list");
        assert!(
            proposal["missing_information"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["kind"] == "list_items_or_structure"
                    && item["question"].as_str().unwrap().contains("list items"))
        );
        assert_eq!(proposal["preview"]["supported_preview"], false);
    }

    #[test]
    fn story_context_can_trigger_title_card_and_review_evidence() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Now the economics of inference change.".into(),
            request: "request visual support".into(),
            anchor_transcript: Some("economics of inference change".into()),
            story_context: vec![StoryContextSignalArgs {
                kind: "topic_shift".into(),
                label: "Chapter: AI inference costs".into(),
                transcript: Some("Now the economics of inference change.".into()),
                start_s: Some(42.0),
                end_s: Some(49.5),
            }],
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        assert!(
            value["skill_candidates"]
                .as_array()
                .unwrap()
                .iter()
                .any(|candidate| candidate["skill_id"] == "chapter-intro")
        );
        assert_eq!(
            proposal_by_type(&value, "title_card")["artifact_type"],
            "title_card"
        );
        assert_eq!(value["story_context"][0]["kind"], "topic_shift");
        assert!(
            proposal_by_type(&value, "title_card")["review"]["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["kind"] == "story_context"
                    && row["trigger_kind"] == "topic_shift"
                    && row["timeline_start_s"] == 42.0)
        );
    }

    #[test]
    fn proposal_includes_editorial_skill_reference_export_and_revision_contracts() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "We will cover hooks, retention, and export checks.".into(),
            request: "make this a faster retention opener with transparent background".into(),
            anchor_transcript: Some("hooks retention export checks".into()),
            anchor_timeline_start_s: None,
            broll_asset: None,
            duration_s: Some(4.0),
            reference_assets: vec!["references/pill-buttons.png".into()],
            story_context: Vec::new(),
            aspect_ratio: Some("16:9".into()),
            platform: Some("youtube".into()),
            alpha: Some(true),
            typography: None,
            color_palette: None,
            motion_intensity: None,
            safe_area_policy: None,
            brand_package: None,
            revision_instruction: Some("make it 3x faster and less intense".into()),
            lane: None,
        })
        .unwrap();

        let proposal = proposal_by_type(&value, "animated_list");
        assert_eq!(value["editorial_skill_registry"]["architecture"], "hybrid");
        assert_eq!(
            value["editorial_skill_registry"]["definition_instance_split"],
            true
        );
        assert!(
            value["skill_candidates"]
                .as_array()
                .unwrap()
                .iter()
                .any(|candidate| candidate["skill_id"] == "retention-list-opener")
        );
        assert_eq!(proposal["editorial_skill"]["id"], "retention-list-opener");
        assert_eq!(
            proposal["editorial_skill"]["skill_path"],
            "skills/retention-list-opener/SKILL.md"
        );
        assert_eq!(
            proposal["editorial_skill"]["instance"]["skill_id"],
            "retention-list-opener"
        );
        assert_eq!(proposal["editorial_skill"]["composable"], true);
        assert_eq!(
            proposal["references"]["assets"][0],
            "references/pill-buttons.png"
        );
        assert_eq!(
            proposal["references"]["items"][0]["path"],
            "references/pill-buttons.png"
        );
        assert_eq!(proposal["references"]["items"][0]["kind"], "image");
        assert_eq!(proposal["references"]["items"][0]["role"], "style_example");
        assert!(
            proposal["references"]["items"][0]["style_tokens"]
                .as_array()
                .unwrap()
                .iter()
                .any(|token| token == "pill")
        );
        assert!(
            proposal["references"]["items"][0]["style_tokens"]
                .as_array()
                .unwrap()
                .iter()
                .any(|token| token == "buttons")
        );
        assert!(
            proposal["review"]["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["kind"] == "reference"
                    && row["asset"] == "references/pill-buttons.png"
                    && row["usage"] == "style_example")
        );
        assert_eq!(proposal["export_intent"]["aspect_ratio"], "16:9");
        assert_eq!(proposal["export_intent"]["alpha"], true);
        assert_eq!(proposal["export_intent"]["platform"], "youtube");
        assert_eq!(
            proposal["revision"]["instruction"],
            "make it 3x faster and less intense"
        );
        assert!(
            proposal["verification"]["artifact_specific"]
                .as_array()
                .unwrap()
                .iter()
                .any(|step| step.as_str().unwrap().contains("list item order"))
        );
    }

    #[test]
    fn saved_visual_support_defaults_are_reused_by_planner_run() {
        let dir = tempfile::tempdir().unwrap();
        save_visual_support_defaults(
            SaveVisualSupportDefaultsArgs {
                aspect_ratio: Some("9:16".into()),
                platform: Some("shorts".into()),
                alpha: Some(true),
                reference_assets: vec!["references/brand-style.png".into()],
                typography: Some("Inter Tight bold captions".into()),
                color_palette: Some("technolgia lime on charcoal".into()),
                motion_intensity: Some("low".into()),
                safe_area_policy: Some("shorts captions below face".into()),
                brand_package: Some("TechNolgia Talks".into()),
            },
            ctx_at(dir.path()),
        )
        .unwrap();

        let body = run(
            PlanVisualSupportProposalArgs {
                selection_text: "We will cover hooks, retention, and export checks.".into(),
                request: "make this an animated list".into(),
                anchor_transcript: Some("hooks retention export checks".into()),
                ..PlanVisualSupportProposalArgs::default()
            },
            ctx_at(dir.path()),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        let proposal = proposal_by_type(&value, "animated_list");

        assert_eq!(proposal["export_intent"]["aspect_ratio"], "9:16");
        assert_eq!(proposal["export_intent"]["platform"], "shorts");
        assert_eq!(proposal["export_intent"]["alpha"], true);
        assert_eq!(proposal["export_intent"]["visual_intensity"], "low");
        assert_eq!(
            proposal["style_defaults"]["typography"],
            "Inter Tight bold captions"
        );
        assert_eq!(
            proposal["style_defaults"]["color_palette"],
            "technolgia lime on charcoal"
        );
        assert_eq!(
            proposal["style_defaults"]["safe_area_policy"],
            "shorts captions below face"
        );
        assert_eq!(
            proposal["style_defaults"]["brand_package"],
            "TechNolgia Talks"
        );
        assert_eq!(
            proposal["references"]["assets"][0],
            "references/brand-style.png"
        );
        assert_eq!(
            value["project_defaults"]["path"],
            ".montage/visual_support_defaults.json"
        );
    }

    #[test]
    fn revise_visual_support_proposal_returns_revised_proposal_and_diff() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "We will cover hooks, retention, and export checks.".into(),
            request: "turn this into an animated list".into(),
            anchor_transcript: Some("hooks retention export checks".into()),
            duration_s: Some(6.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "animated_list").clone();

        let revised = revise_visual_support_proposal(ReviseVisualSupportProposalArgs {
            proposal,
            instruction: "make it 3x faster and use transparent background".into(),
        })
        .unwrap();

        assert_eq!(revised["proposal"]["export_intent"]["duration_s"], 2.0);
        assert_eq!(revised["proposal"]["export_intent"]["alpha"], true);
        assert_eq!(
            revised["proposal"]["revision"]["instruction"],
            "make it 3x faster and use transparent background"
        );
        assert!(
            revised["diff"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| change["field"] == "export_intent.duration_s")
        );
        assert!(
            revised["diff"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| change["field"] == "export_intent.alpha")
        );
        assert!(
            revised["next_step"]
                .as_str()
                .unwrap()
                .contains("review the diff before apply_edl")
        );
    }

    #[test]
    fn revise_visual_support_proposal_includes_side_by_side_preview_contract() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the quote that should land harder.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("quote that should land harder".into()),
            duration_s: Some(4.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "quote_highlight").clone();

        let revised = revise_visual_support_proposal(ReviseVisualSupportProposalArgs {
            proposal,
            instruction: "make it shorter and use transparent background".into(),
        })
        .unwrap();

        assert_eq!(revised["preview_comparison"]["layout"], "side_by_side");
        assert_eq!(
            revised["preview_comparison"]["before"]["artifact_type"],
            "quote_highlight"
        );
        assert_eq!(
            revised["preview_comparison"]["after"]["duration_s"],
            revised["proposal"]["export_intent"]["duration_s"]
        );
        assert_eq!(
            revised["preview_comparison"]["render_contract"]["mode"],
            "preview_cache_pair"
        );
        assert_eq!(
            revised["preview_comparison"]["render_contract"]["panels"][0],
            "before"
        );
        assert_eq!(
            revised["preview_comparison"]["render_contract"]["panels"][1],
            "after"
        );
        assert!(
            revised["preview_comparison"]["verification_contract"]["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check == "before_after_frame_count_available")
        );
        assert!(
            revised["preview_comparison"]["verification_contract"]["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check == "changed_fields_visible_or_declared_metadata_only")
        );
    }

    #[test]
    fn revise_visual_support_proposal_can_reduce_visual_intensity() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the quote that should land harder.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("quote that should land harder".into()),
            duration_s: Some(4.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "quote_highlight").clone();

        let revised = revise_visual_support_proposal(ReviseVisualSupportProposalArgs {
            proposal,
            instruction: "make the glow less intense".into(),
        })
        .unwrap();

        assert_eq!(
            revised["proposal"]["export_intent"]["visual_intensity"],
            "low"
        );
        assert_eq!(
            revised["proposal"]["revision"]["instruction"],
            "make the glow less intense"
        );
        assert!(
            revised["diff"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| change["field"] == "export_intent.visual_intensity"
                    && change["from"] == "standard"
                    && change["to"] == "low")
        );
        assert_eq!(
            revised["visual_diff"]["before"]["visual_intensity"],
            "standard"
        );
        assert_eq!(revised["visual_diff"]["after"]["visual_intensity"], "low");
    }

    #[test]
    fn revise_visual_support_proposal_can_switch_artifact_type_with_visual_diff() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "We will cover hooks, retention, and export checks.".into(),
            request: "turn this into an animated list".into(),
            anchor_transcript: Some("hooks retention export checks".into()),
            duration_s: Some(6.0),
            reference_assets: vec!["references/style-frame.png".into()],
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "animated_list").clone();

        let revised = revise_visual_support_proposal(ReviseVisualSupportProposalArgs {
            proposal,
            instruction: "switch this to source-backed B-roll instead".into(),
        })
        .unwrap();

        assert_eq!(revised["proposal"]["artifact_type"], "broll_package");
        assert_eq!(
            revised["proposal"]["references"]["assets"][0],
            "references/style-frame.png"
        );
        assert!(
            revised["proposal"]["missing_information"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["kind"] == "broll_asset_or_generation_approval")
        );
        assert!(
            revised["diff"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| change["field"] == "artifact_type"
                    && change["from"] == "animated_list"
                    && change["to"] == "broll_package")
        );
        assert_eq!(
            revised["visual_diff"]["before"]["artifact_type"],
            "animated_list"
        );
        assert_eq!(
            revised["visual_diff"]["after"]["artifact_type"],
            "broll_package"
        );
    }

    #[test]
    fn revise_visual_support_proposal_can_switch_to_title_card() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Chapter two: the retention problem.".into(),
            request: "turn this into an animated list".into(),
            anchor_transcript: Some("Chapter two: the retention problem".into()),
            duration_s: Some(5.0),
            reference_assets: vec!["references/title-style.png".into()],
            aspect_ratio: Some("9:16".into()),
            motion_intensity: Some("low".into()),
            typography: Some("Inter Tight bold captions".into()),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "animated_list").clone();

        let revised = revise_visual_support_proposal(ReviseVisualSupportProposalArgs {
            proposal,
            instruction: "switch this to a title card instead".into(),
        })
        .unwrap();

        assert_eq!(revised["proposal"]["artifact_type"], "title_card");
        assert_eq!(
            revised["proposal"]["preview"]["timeline_object"],
            "MotionScene"
        );
        assert_eq!(revised["proposal"]["export_intent"]["aspect_ratio"], "9:16");
        assert_eq!(
            revised["proposal"]["export_intent"]["visual_intensity"],
            "low"
        );
        assert_eq!(
            revised["proposal"]["style_defaults"]["typography"],
            "Inter Tight bold captions"
        );
        assert_eq!(
            revised["proposal"]["references"]["assets"][0],
            "references/title-style.png"
        );
        assert!(
            revised["diff"]
                .as_array()
                .unwrap()
                .iter()
                .any(|change| change["field"] == "artifact_type"
                    && change["from"] == "animated_list"
                    && change["to"] == "title_card")
        );
        assert_eq!(
            revised["visual_diff"]["after"]["artifact_type"],
            "title_card"
        );
    }

    #[test]
    fn verify_quote_highlight_checks_anchor_and_motion_scene_text() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the quote that should land harder.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("quote that should land harder".into()),
            duration_s: Some(4.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "quote_highlight").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();

        assert_eq!(report["status"], "pass");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "quote_matches_transcript_anchor"
                    && check["status"] == "pass")
        );
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "motion_scene_contains_quote_text"
                    && check["status"] == "pass")
        );
    }

    #[test]
    fn verify_quote_and_title_pass_for_long_truncated_anchors() {
        // A selection longer than `concise()`'s 12-word window: the planner
        // renders the truncated display text, so verification must compare
        // against what is actually drawn, not the full anchor.
        let long = "the first second third fourth fifth sixth seventh eighth ninth \
                    tenth eleventh twelfth thirteenth fourteenth point";

        for artifact in ["quote_highlight", "title_card"] {
            let request = if artifact == "quote_highlight" {
                "make this quote pop visually"
            } else {
                "turn this into a title card"
            };
            let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
                selection_text: long.into(),
                request: request.into(),
                anchor_transcript: Some(long.into()),
                duration_s: Some(4.0),
                ..PlanVisualSupportProposalArgs::default()
            })
            .unwrap();
            let proposal = proposal_by_type(&value, artifact).clone();

            let report = verify_visual_support_artifact(
                VerifyVisualSupportArtifactArgs {
                    proposal,
                    require_project_asset_exists: false,
                    render_frame_report: None,
                },
                None,
            )
            .unwrap();

            let check_id = if artifact == "quote_highlight" {
                "quote_matches_transcript_anchor"
            } else {
                "title_text_matches_transcript_anchor"
            };
            assert!(
                report["checks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|check| check["id"] == check_id && check["status"] == "pass"),
                "{artifact} long-anchor verification should pass: {report:#}"
            );
        }
    }

    #[test]
    fn verify_visual_support_artifact_includes_frame_level_contract() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the quote that should land harder.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("quote that should land harder".into()),
            duration_s: Some(4.0),
            alpha: Some(true),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "quote_highlight").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();

        assert_eq!(
            report["frame_verification_contract"]["scope"],
            "rendered_artifact"
        );
        assert_eq!(
            report["frame_verification_contract"]["timeline_object"],
            "MotionScene"
        );
        assert_eq!(
            report["frame_verification_contract"]["sample_points_s"][0],
            0.5
        );
        assert!(
            report["frame_verification_contract"]["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "overlay_visible_in_expected_range")
        );
        assert!(
            report["frame_verification_contract"]["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "alpha_channel_present_when_requested")
        );
        assert_eq!(
            report["frame_verification_contract"]["evidence_outputs"][0],
            "visual_support/frame-samples.json"
        );
    }

    #[test]
    fn verify_visual_support_artifact_fails_when_render_frame_report_fails() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the quote that should land harder.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("quote that should land harder".into()),
            duration_s: Some(4.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "quote_highlight").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: false,
                render_frame_report: Some(serde_json::json!({
                    "artifact_type": "quote_highlight",
                    "sample_count": 3,
                    "checks": [
                        {"id": "overlay_visible_in_expected_range", "status": "fail"}
                    ]
                })),
            },
            None,
        )
        .unwrap();

        assert_eq!(report["status"], "fail");
        assert_eq!(report["frame_verification"]["status"], "fail");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |check| check["id"] == "render_frame_overlay_visible_in_expected_range"
                        && check["status"] == "fail"
                )
        );
    }

    #[test]
    fn verify_broll_package_requires_existing_asset_when_requested() {
        let dir = project_with_media();
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "The founder describes a crowded market.".into(),
            request: "add a b-roll package here".into(),
            anchor_transcript: Some("crowded market".into()),
            broll_asset: Some("raw/generated/crowded-market.mp4".into()),
            duration_s: Some(3.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "broll_package").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: true,
                render_frame_report: None,
            },
            Some(dir.path()),
        )
        .unwrap();

        assert_eq!(report["status"], "pass");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "broll_asset_exists" && check["status"] == "pass")
        );
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "broll_anchor_matches_evidence"
                    && check["status"] == "pass")
        );
        assert!(report["checks"].as_array().unwrap().iter().any(|check| {
            check["id"] == "broll_has_source_provenance" && check["status"] == "pass"
        }));
        assert!(report["checks"].as_array().unwrap().iter().any(|check| {
            check["id"] == "broll_has_disclosure_policy" && check["status"] == "pass"
        }));
    }

    #[test]
    fn verify_broll_asset_exists_rejects_paths_outside_project_root() {
        let dir = project_with_media();

        // A real file that lives OUTSIDE the project root.
        let outside = tempfile::tempdir().unwrap();
        let escaping_asset = outside.path().join("crowded-market.mp4");
        std::fs::write(&escaping_asset, b"not a project asset").unwrap();

        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "The founder describes a crowded market.".into(),
            request: "add a b-roll package here".into(),
            anchor_transcript: Some("crowded market".into()),
            broll_asset: Some(escaping_asset.to_string_lossy().into_owned()),
            duration_s: Some(3.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "broll_package").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: true,
                render_frame_report: None,
            },
            Some(dir.path()),
        )
        .unwrap();

        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "broll_asset_exists" && check["status"] == "fail"),
            "absolute path outside the project must not pass broll_asset_exists: {report:#}"
        );
    }

    #[test]
    fn verify_animated_list_checks_motion_scene_step_layers() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "We will cover hooks, retention, and export checks.".into(),
            request: "turn this into an animated list".into(),
            anchor_transcript: Some("hooks, retention, and export checks".into()),
            duration_s: Some(6.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "animated_list").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();

        assert_eq!(report["status"], "pass");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "animated_list_has_steps" && check["status"] == "pass")
        );
        assert!(report["checks"].as_array().unwrap().iter().any(|check| {
            check["id"] == "animated_list_items_match_transcript_anchor"
                && check["status"] == "pass"
        }));
    }

    #[test]
    fn verify_animated_list_fails_when_list_items_clarification_is_missing() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This moment needs more structure.".into(),
            request: "turn this into an animated list".into(),
            anchor_transcript: Some("moment needs more structure".into()),
            duration_s: Some(5.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "animated_list").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();

        assert_eq!(report["status"], "fail");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "missing_information_resolved"
                    && check["status"] == "fail")
        );
    }

    #[test]
    fn verify_map_visualization_checks_route_label_contract() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Show the route from Lagos to Nairobi.".into(),
            request: "make this a route map".into(),
            anchor_transcript: Some("Lagos to Nairobi".into()),
            duration_s: Some(6.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "map_visualization").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();

        assert_eq!(report["status"], "pass");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "map_labels_match_transcript_anchor"
                    && check["status"] == "pass")
        );
    }

    #[test]
    fn verify_map_visualization_fails_when_location_clarification_is_missing() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Show the route we discussed.".into(),
            request: "make this a route map".into(),
            anchor_transcript: Some("route we discussed".into()),
            duration_s: Some(6.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "map_visualization").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();

        assert_eq!(report["status"], "fail");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "missing_information_resolved"
                    && check["status"] == "fail")
        );
    }

    #[test]
    fn verify_counter_stat_graphic_checks_selected_number() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Revenue grew by 42% this quarter.".into(),
            request: "turn this into a statistic counter".into(),
            anchor_transcript: Some("Revenue grew by 42%".into()),
            duration_s: Some(4.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "counter_stat_graphic").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();

        assert_eq!(report["status"], "pass");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |check| check["id"] == "counter_value_matches_transcript_anchor"
                        && check["status"] == "pass"
                )
        );
    }

    #[test]
    fn verify_counter_stat_graphic_fails_when_stat_clarification_is_missing() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "The market changed dramatically this quarter.".into(),
            request: "turn this into a statistic counter".into(),
            anchor_transcript: Some("market changed dramatically".into()),
            duration_s: Some(4.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "counter_stat_graphic").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();

        assert_eq!(report["status"], "fail");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "missing_information_resolved"
                    && check["status"] == "fail")
        );
    }

    #[test]
    fn verify_search_bar_checks_query_text_contract() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "How does AI search change podcast discovery?".into(),
            request: "turn this into a search bar sequence".into(),
            anchor_transcript: Some("AI search change podcast discovery".into()),
            duration_s: Some(5.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "search_bar").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();

        assert_eq!(report["status"], "pass");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |check| check["id"] == "search_query_matches_transcript_anchor"
                        && check["status"] == "pass"
                )
        );
    }

    #[test]
    fn verify_search_bar_fails_when_query_clarification_is_missing() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the thing we should look up.".into(),
            request: "turn this into a search bar sequence".into(),
            anchor_transcript: Some("thing we should look up".into()),
            duration_s: Some(5.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "search_bar").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();

        assert_eq!(report["status"], "fail");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "missing_information_resolved"
                    && check["status"] == "fail")
        );
    }

    #[test]
    fn search_counter_and_map_have_auditable_motion_scene_slots() {
        let search_value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "How does AI search change podcast discovery?".into(),
            request: "turn this into a search bar sequence".into(),
            anchor_transcript: Some("AI search change podcast discovery".into()),
            duration_s: Some(5.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let search_scene = motion_scene_for(proposal_by_type(&search_value, "search_bar"));
        let query = search_scene
            .layers
            .iter()
            .find(|layer| layer.id == "search-query")
            .unwrap();
        assert_eq!(query.params["text"], "AI search change podcast discovery");
        assert_eq!(query.params["typing_reveal"], true);

        let counter_value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Revenue grew by 42% this quarter.".into(),
            request: "turn this into a statistic counter".into(),
            anchor_transcript: Some("Revenue grew by 42%".into()),
            duration_s: Some(4.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let counter_scene =
            motion_scene_for(proposal_by_type(&counter_value, "counter_stat_graphic"));
        let value = counter_scene
            .layers
            .iter()
            .find(|layer| layer.id == "stat-value")
            .unwrap();
        assert_eq!(value.params["text"], "42%");
        assert_eq!(value.params["count_up"], true);
        let counter_report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal: proposal_by_type(&counter_value, "counter_stat_graphic").clone(),
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();
        assert!(
            counter_report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |check| check["id"] == "counter_has_count_up_slot" && check["status"] == "pass"
                )
        );

        let map_value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Show the route from Lagos to Nairobi.".into(),
            request: "make this a route map".into(),
            anchor_transcript: Some("Lagos to Nairobi".into()),
            duration_s: Some(6.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let map_scene = motion_scene_for(proposal_by_type(&map_value, "map_visualization"));
        assert!(
            map_scene
                .layers
                .iter()
                .any(|layer| { layer.id == "map-origin" && layer.params["text"] == "Lagos" })
        );
        assert!(
            map_scene.layers.iter().any(|layer| {
                layer.id == "map-destination" && layer.params["text"] == "Nairobi"
            })
        );
        assert!(
            map_scene
                .layers
                .iter()
                .any(|layer| layer.id == "route-line")
        );
        let map_report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal: proposal_by_type(&map_value, "map_visualization").clone(),
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();
        assert!(
            map_report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "map_has_route_line_slot" && check["status"] == "pass")
        );

        let search_report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal: proposal_by_type(&search_value, "search_bar").clone(),
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();
        assert!(
            search_report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "search_has_typed_query_slot"
                    && check["status"] == "pass")
        );
    }

    #[test]
    fn verify_title_card_checks_topic_text_contract() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Chapter: AI agents reshape podcast production.".into(),
            request: "make this a chapter intro title card".into(),
            anchor_transcript: Some("AI agents reshape podcast production".into()),
            duration_s: Some(3.5),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let proposal = proposal_by_type(&value, "title_card").clone();

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal,
                require_project_asset_exists: false,
                render_frame_report: None,
            },
            None,
        )
        .unwrap();

        assert_eq!(report["status"], "pass");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(
                    |check| check["id"] == "title_text_matches_transcript_anchor"
                        && check["status"] == "pass"
                )
        );
    }

    #[test]
    fn motion_scene_lane_constraint_never_returns_broll() {
        // A footage-flavored request used to come back as a
        // source-backed B-roll proposal with apply_edl null even when
        // the caller constrained the lane to motion_scene.
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "The founder describes a crowded market.".into(),
            request: "add footage-like b-roll support here".into(),
            lane: Some("motion_scene".into()),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposals = value["proposals"].as_array().unwrap();
        assert!(!proposals.is_empty());
        for proposal in proposals {
            assert_ne!(proposal["artifact_type"], "broll_package");
            assert!(
                proposal["apply_edl"]["edl"]
                    .as_str()
                    .is_some_and(|edl| edl.contains("Set Motion Scene")),
                "motion_scene lane proposals must carry a Set Motion Scene apply_edl: {proposal}"
            );
        }
        assert_eq!(value["lane_constraint"]["lane"], "motion_scene");
        assert_eq!(value["lane_constraint"]["honored"], true);
        assert!(
            value["lane_constraint"]["note"]
                .as_str()
                .is_some_and(|note| !note.is_empty())
        );
    }

    #[test]
    fn broll_lane_constraint_returns_only_broll() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the quote that should land harder.".into(),
            request: "make this quote pop visually".into(),
            lane: Some("broll".into()),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposals = value["proposals"].as_array().unwrap();
        assert!(!proposals.is_empty());
        for proposal in proposals {
            assert_eq!(proposal["artifact_type"], "broll_package");
        }
        assert_eq!(value["lane_constraint"]["lane"], "broll");
    }

    #[test]
    fn unknown_lane_constraint_is_rejected_with_supported_lanes() {
        let error = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Some selection.".into(),
            request: "support this".into(),
            lane: Some("timeline_edit".into()),
            ..PlanVisualSupportProposalArgs::default()
        })
        .expect_err("unknown lane must error");

        assert!(error.contains("unknown lane"), "error: {error}");
        assert!(error.contains("motion_scene"), "error: {error}");
        assert!(error.contains("broll"), "error: {error}");
    }

    fn project_with_stored_scene(scene_id: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::init(dir.path()).unwrap();
        project
            .timeline
            .metadata
            .montage
            .get_or_insert_with(Default::default)
            .motion_scenes
            .push(montage_proto::professional::MotionScene {
                id: scene_id.to_string(),
                start_s: 531.14,
                duration_s: 6.0,
                fps: 30.0,
                width: 1920,
                height: 1080,
                layers: vec![montage_proto::professional::MotionSceneLayer {
                    id: "heading".into(),
                    kind: montage_proto::professional::MotionSceneLayerKind::Text,
                    from_s: 0.0,
                    duration_s: 6.0,
                    z_index: 10,
                    params: [(
                        "text".to_string(),
                        serde_json::json!("AI MISTAKE VS DRONE MISTAKE"),
                    )]
                    .into_iter()
                    .collect(),
                }],
                rationale: None,
            });
        project.write(dir.path()).unwrap();
        dir
    }

    #[test]
    fn verify_recognizes_stored_motion_scene_as_applied() {
        // An agent that hand-applied a Set Motion Scene EDL then asked
        // for verification used to get "proposal is not apply-ready" —
        // the scene was stored, not pending.
        let scene_id = "ai-vs-drone-mistake-comparison-manual";
        let dir = project_with_stored_scene(scene_id);

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal: serde_json::json!({
                    "artifact_type": "motion_scene",
                    "scene_id": scene_id,
                }),
                ..VerifyVisualSupportArtifactArgs::default()
            },
            Some(dir.path()),
        )
        .unwrap();

        assert_ne!(report["status"], "fail", "report: {report}");
        let checks = report["checks"].as_array().unwrap();
        let check_status = |id: &str| {
            checks
                .iter()
                .find(|check| check["id"] == id)
                .map(|check| check["status"].as_str().unwrap_or_default().to_string())
                .unwrap_or_default()
        };
        assert_eq!(check_status("motion_scene_stored_in_timeline"), "pass");
        assert_eq!(check_status("proposal_has_apply_edl"), "pass");
        assert_eq!(check_status("motion_scene_readable"), "pass");
        assert_eq!(
            check_status("motion_scene_text_layers_have_content"),
            "pass"
        );
    }

    #[test]
    fn verify_still_fails_unknown_scene_without_apply_edl() {
        let dir = project_with_stored_scene("some-other-scene");

        let report = verify_visual_support_artifact(
            VerifyVisualSupportArtifactArgs {
                proposal: serde_json::json!({
                    "artifact_type": "motion_scene",
                    "scene_id": "never-applied-scene",
                }),
                ..VerifyVisualSupportArtifactArgs::default()
            },
            Some(dir.path()),
        )
        .unwrap();

        assert_eq!(report["status"], "fail");
        assert!(
            report["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|check| check["id"] == "proposal_has_apply_edl" && check["status"] == "fail")
        );
    }

    #[test]
    fn broll_generation_plan_requires_editorial_transcript_pass_first() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "The founder describes a crowded market.".into(),
            request: "add a b-roll package here".into(),
            anchor_transcript: Some("crowded market".into()),
            broll_asset: None,
            duration_s: Some(6.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposal = proposal_by_type(&value, "broll_package");
        assert_eq!(
            proposal["generation_plan"]["tool"],
            "llm_editorial_transcript_pass"
        );
        assert_eq!(
            proposal["generation_plan"]["coverage_tool"],
            "find_generated_broll_opportunities"
        );
        assert!(
            proposal["missing_information"][0]["fallback"]
                .as_str()
                .unwrap()
                .contains("Choose the moment from transcript flow first")
        );
    }

    #[tokio::test]
    async fn quote_list_and_broll_workflow_applies_renders_and_verifies() {
        if montage_render::ffmpeg_path().is_err() || montage_render::ffprobe_path().is_err() {
            return;
        }

        let dir = project_with_media();
        let ctx = ctx_at(dir.path());
        let quote = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the quote that should land harder.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("quote that should land harder".into()),
            broll_asset: None,
            duration_s: Some(1.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let list = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "We will cover hooks, retention, and export checks.".into(),
            request: "turn this into an animated list".into(),
            anchor_transcript: Some("hooks retention export checks".into()),
            broll_asset: None,
            duration_s: Some(1.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let broll = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "The founder describes a crowded market.".into(),
            request: "add a b-roll package here".into(),
            anchor_transcript: Some("crowded market".into()),
            broll_asset: Some("raw/generated/crowded-market.mp4".into()),
            duration_s: Some(1.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        for proposal in [
            proposal_by_type(&quote, "quote_highlight"),
            proposal_by_type(&list, "animated_list"),
            proposal_by_type(&broll, "broll_package"),
        ] {
            let applied = apply_edl::run(
                ApplyEdlArgs {
                    edl: proposal_edl(proposal).to_string(),
                    dry_run: false,
                    reasoning: Some("proposal-to-visual-support workflow test".into()),
                },
                ctx.clone(),
            )
            .unwrap();
            assert!(applied.contains("committed 2 op"), "{applied}");
        }

        let project = Project::read(dir.path()).unwrap();
        let metadata = project.timeline.metadata.montage.as_ref().unwrap();
        assert_eq!(metadata.proposal_packages.len(), 3);
        assert_eq!(metadata.motion_scenes.len(), 2);
        assert!(
            serde_json::to_value(&project.timeline)
                .unwrap()
                .to_string()
                .contains("raw/generated/crowded-market.mp4")
        );

        let render = start_render::run(
            StartRenderArgs {
                scope: "timeline".into(),
                asset: None,
                range: None,
                guide: None,
                preset: None,
            },
            ctx.clone(),
        )
        .await
        .unwrap();
        let render: serde_json::Value = serde_json::from_str(&render).unwrap();
        if render["state"] == "failed"
            && render["stderr_tail"]
                .as_str()
                .is_some_and(|stderr| stderr.contains("No such filter: 'drawtext'"))
        {
            assert_eq!(render["render_kind"], "final_timeline_export");
            assert!(render["manifest_path"].as_str().is_some());
            assert_eq!(
                render["next_step"],
                "Render did not succeed (state=failed); inspect stderr_tail and re-run after fixing inputs."
            );
            return;
        }
        assert_eq!(render["state"], "done", "{render:#}");

        let report = verify_render::run(
            VerifyRenderArgs {
                output_path: render["output_path"].as_str().map(str::to_string),
                expected_duration_s: Some(2.0),
                duration_tolerance_s: Some(0.75),
                mode: Some("quick".into()),
                source_range_manifest_path: None,
                silence_threshold_db: None,
                max_unexpected_silence_s: None,
                black_min_duration_s: None,
                max_black_segment_s: None,
            },
            ctx,
        )
        .await
        .unwrap();
        let report: serde_json::Value = serde_json::from_str(&report).unwrap();
        assert_eq!(report["passed"], true, "{report:#}");
    }

    #[test]
    fn generic_visual_support_request_yields_an_apply_ready_proposal() {
        // The desktop "Visual support" button submits a generic request with
        // no B-roll asset. It must not collapse to a B-roll-only plan (which
        // is missing its asset and gets filtered out), leaving the user with
        // no openable proposal.
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "The team rebuilt the onboarding flow from scratch.".into(),
            request: "request visual support".into(),
            anchor_transcript: Some("rebuilt the onboarding flow from scratch".into()),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposals = value["proposals"].as_array().unwrap();
        let apply_ready: Vec<&serde_json::Value> = proposals
            .iter()
            .filter(|proposal| {
                proposal["missing_information"]
                    .as_array()
                    .is_some_and(Vec::is_empty)
                    && proposal["apply_edl"]["edl"].as_str().is_some()
            })
            .collect();
        assert!(
            !apply_ready.is_empty(),
            "generic visual-support request must produce at least one apply-ready proposal: {:?}",
            proposal_titles(&value)
        );
        // And it must not be only a B-roll package.
        assert!(
            apply_ready
                .iter()
                .any(|proposal| proposal["artifact_type"] != "broll_package"),
            "generic request must offer a non-B-roll apply-ready proposal: {:?}",
            proposal_titles(&value)
        );
    }

    #[test]
    fn anchor_timeline_start_is_recorded_on_the_motion_scene() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the quote that should land harder.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("quote that should land harder".into()),
            anchor_timeline_start_s: Some(42.5),
            duration_s: Some(4.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let scene = motion_scene_for(proposal_by_type(&value, "quote_highlight"));
        assert_eq!(
            scene.start_s, 42.5,
            "scene must record the anchor's timeline start so it renders over the selected words"
        );
        assert!(
            scene.validate().is_empty(),
            "anchored scene must still validate: {:?}",
            scene.validate()
        );
    }

    #[test]
    fn motion_scene_ids_are_unique_per_anchor() {
        let first = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "This is the first quote that should land harder.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("first quote that should land harder".into()),
            duration_s: Some(4.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();
        let second = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Here is a completely different second highlight moment.".into(),
            request: "make this quote pop visually".into(),
            anchor_transcript: Some("completely different second highlight moment".into()),
            duration_s: Some(4.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let first_scene = motion_scene_for(proposal_by_type(&first, "quote_highlight"));
        let second_scene = motion_scene_for(proposal_by_type(&second, "quote_highlight"));
        assert_ne!(
            first_scene.id, second_scene.id,
            "two distinct anchors must produce distinct scene ids so accepting \
             both does not overwrite the first via replace_by_id"
        );
    }

    #[test]
    fn short_duration_list_scene_stays_within_scene_bounds() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "We will cover hooks, retention, export checks, and reviews.".into(),
            request: "turn this into an animated list".into(),
            anchor_transcript: Some("hooks, retention, export checks, and reviews".into()),
            duration_s: Some(1.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposal = proposal_by_type(&value, "animated_list");
        let scene = motion_scene_for(proposal);
        assert!(
            scene.validate().is_empty(),
            "short-duration animated list produced an invalid scene: {:?}",
            scene.validate()
        );
    }

    #[test]
    fn short_duration_search_scene_stays_within_scene_bounds() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "How does the funding round actually work?".into(),
            request: "show this as a search bar".into(),
            anchor_transcript: Some("how does the funding round actually work".into()),
            duration_s: Some(1.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposal = proposal_by_type(&value, "search_bar");
        let scene = motion_scene_for(proposal);
        assert!(
            scene.validate().is_empty(),
            "short-duration search bar produced an invalid scene: {:?}",
            scene.validate()
        );
    }

    #[test]
    fn short_duration_counter_scene_stays_within_scene_bounds() {
        let value = plan_visual_support_proposals(PlanVisualSupportProposalArgs {
            selection_text: "Revenue grew 240% last year.".into(),
            request: "make a counter stat graphic".into(),
            anchor_transcript: Some("revenue grew 240% last year".into()),
            duration_s: Some(1.0),
            ..PlanVisualSupportProposalArgs::default()
        })
        .unwrap();

        let proposal = proposal_by_type(&value, "counter_stat_graphic");
        let scene = motion_scene_for(proposal);
        assert!(
            scene.validate().is_empty(),
            "short-duration counter produced an invalid scene: {:?}",
            scene.validate()
        );
    }
}
