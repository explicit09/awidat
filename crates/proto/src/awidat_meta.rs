//! Strongly-typed `metadata.awidat` namespace.
//!
//! Per `PLAN.md` §3, the only schema extension Awidat makes to OTIO is the
//! `metadata.awidat` block. We model it strongly (NOT as `serde_json::Value`)
//! so unknown fields surface as parse errors against the schema we own.
//!
//! Three locations of awidat metadata exist:
//!
//! - On a [`crate::otio::Timeline`]'s `metadata`: see
//!   [`AwidatTimelineMetadata`].
//! - On a [`crate::otio::Clip`]'s `metadata`: see [`AwidatClipMetadata`].
//! - On a [`crate::otio::Marker`]'s `metadata`: see
//!   [`AwidatMarkerMetadata`].

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::professional::{
    AnimationTarget, AssetCatalog, AudioFinishingState, CapabilityRegistry, ColorFinishingState,
    CompositionGraph, DeliveryProfile, ExpressionLink, ExpressionSource, FindingSeverity,
    LearningSignal, MotionGraphicsTemplate, MotionPackage, PackageManifest, ParameterAnimation,
    PipelineReadinessReport, PipelineStageReadiness, PlannerPassContract, PreflightReport,
    ProfessionalDiagnostic, ProposalPackage, ReadinessState, SourceSelect, Stringout,
    TrackingPackage, WorkflowLens,
};

/// Top-level awidat metadata on a `Timeline.metadata.awidat`.
///
/// Forward-compat: `version` is the only required field. All others have
/// `#[serde(default)]`, and unknown keys land in [`Self::extra`] (round-trip
/// preserved). Adding a new field in v1.5 is a non-breaking change as long as
/// the new field is optional or has a serde default — old engines reading
/// new files keep the value in `extra`, new engines reading old files use
/// the default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AwidatTimelineMetadata {
    /// Awidat project format version, e.g. `"0.1"`.
    #[serde(default)]
    pub version: String,
    /// Source asset paths relative to the project root.
    #[serde(default)]
    pub source_assets: Vec<String>,
    /// Per-clip content anchors, keyed by clip UUID. The anchor lets
    /// `apply_edl` find a clip after upstream edits shift its timeline
    /// position. See `PLAN.md` §3 / §6.2.
    #[serde(default)]
    pub anchors: HashMap<String, Anchor>,
    /// Editorial intent for hard-cut boundaries, keyed by
    /// `from_clip_id::to_clip_id`. Transitions already carry their own
    /// semantic metadata on OTIO `Transition.1` nodes; this map gives hard
    /// cuts the same durable editorial memory without inventing fake
    /// transition items.
    #[serde(default)]
    pub cut_boundaries: HashMap<String, SemanticCutSpec>,
    /// Most recent `edit-plan.json` item id this timeline applied.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub edit_plan_id: Option<String>,
    /// Timeline-level broadcast overlay package. This is graph-native
    /// presentation state: render and desktop preview derive visible
    /// title cards, lower thirds, ticker, chapter cards, and host intro
    /// strips from this config instead of treating them as detached
    /// media files.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub broadcast_overlay: Option<BroadcastOverlayConfig>,
    /// First-class asset catalog independent of timeline clips.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub asset_catalog: Option<AssetCatalog>,
    /// Durable source review decisions.
    #[serde(default)]
    pub selects: Vec<SourceSelect>,
    /// Pre-timeline ordered selects.
    #[serde(default)]
    pub stringouts: Vec<Stringout>,
    /// Unified reviewable proposal packages across pipeline stages.
    #[serde(default)]
    pub proposal_packages: Vec<ProposalPackage>,
    /// General keyframe/parameter animation documents.
    #[serde(default)]
    pub parameter_animations: Vec<ParameterAnimation>,
    /// Reusable motion graphics templates.
    #[serde(default)]
    pub motion_templates: Vec<MotionGraphicsTemplate>,
    /// Reviewable motion packages proposed by agents.
    #[serde(default)]
    pub motion_packages: Vec<MotionPackage>,
    /// Serializable composition graphs.
    #[serde(default)]
    pub composition_graphs: Vec<CompositionGraph>,
    /// Explicit procedural expression links.
    #[serde(default)]
    pub expression_links: Vec<ExpressionLink>,
    /// Tracking, mask, and matte sidecar references/contracts.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tracking_package: Option<TrackingPackage>,
    /// Color finishing workflow state.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color_finishing: Option<ColorFinishingState>,
    /// Audio finishing workflow state.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub audio_finishing: Option<AudioFinishingState>,
    /// Named delivery profiles.
    #[serde(default)]
    pub delivery_profiles: Vec<DeliveryProfile>,
    /// Preflight reports.
    #[serde(default)]
    pub preflight_reports: Vec<PreflightReport>,
    /// Package manifests for render/delivery artifacts.
    #[serde(default)]
    pub package_manifests: Vec<PackageManifest>,
    /// Active or available workflow lenses.
    #[serde(default)]
    pub workflow_lenses: Vec<WorkflowLens>,
    /// Capability registry for pre-autonomy orchestration.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub capability_registry: Option<CapabilityRegistry>,
    /// Pipeline readiness report.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pipeline_readiness: Option<PipelineReadinessReport>,
    /// Typed planner pass contracts.
    #[serde(default)]
    pub planner_passes: Vec<PlannerPassContract>,
    /// User acceptance/rejection signals.
    #[serde(default)]
    pub learning_signals: Vec<LearningSignal>,
    /// Forward-compat passthrough. Future versions of this metadata can add
    /// fields here without breaking older readers. Engines that don't know
    /// about a field still round-trip it intact.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl AwidatTimelineMetadata {
    /// Validate professional pipeline substrate references and local invariants.
    pub fn validate_professional_substrate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        let asset_ids = self.asset_ids();
        let select_ids: HashSet<&str> = self
            .selects
            .iter()
            .map(|select| select.id.as_str())
            .collect();

        if let Some(catalog) = &self.asset_catalog {
            diagnostics.extend(catalog.validate());
        }

        for select in &self.selects {
            if !asset_ids.contains(select.asset_id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    crate::professional::CapabilityArea::SourceReviewSelects,
                    format!(
                        "select {} references missing asset {}",
                        select.id, select.asset_id
                    ),
                ));
            }
            if !select.range.is_valid() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    crate::professional::CapabilityArea::SourceReviewSelects,
                    format!(
                        "select {} range start_s {} must be before end_s {}",
                        select.id, select.range.start_s, select.range.end_s
                    ),
                ));
            }
        }

        for stringout in &self.stringouts {
            for select_id in &stringout.select_ids {
                if !select_ids.contains(select_id.as_str()) {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        crate::professional::CapabilityArea::SourceReviewSelects,
                        format!(
                            "stringout {} references missing select {}",
                            stringout.id, select_id
                        ),
                    ));
                }
            }
        }

        for animation in &self.parameter_animations {
            diagnostics.extend(animation.validate());
        }
        diagnostics.extend(detect_parameter_animation_conflicts(
            &self.parameter_animations,
        ));
        for template in &self.motion_templates {
            diagnostics.extend(template.validate());
        }
        for graph in &self.composition_graphs {
            diagnostics.extend(graph.validate());
        }
        for link in &self.expression_links {
            diagnostics.extend(link.validate());
        }
        diagnostics.extend(detect_expression_link_cycles(&self.expression_links));
        if let Some(package) = &self.tracking_package {
            diagnostics.extend(package.validate());
        }

        diagnostics
    }

    /// Build a pipeline readiness report covering all 13 professional areas.
    pub fn build_professional_readiness_report(&self) -> PipelineReadinessReport {
        use crate::professional::CapabilityArea;

        let diagnostics = self.validate_professional_substrate();
        let mut stages = vec![
            stage(
                CapabilityArea::AssetCatalog,
                self.asset_catalog
                    .as_ref()
                    .is_some_and(|catalog| !catalog.assets.is_empty()),
                "asset catalog has no assets",
            ),
            stage(
                CapabilityArea::SourceReviewSelects,
                !self.selects.is_empty() && !self.stringouts.is_empty(),
                "selects and stringouts are incomplete",
            ),
            stage(
                CapabilityArea::AssemblyAndTimelineOperations,
                self.extra.contains_key("last_professional_timeline_edit")
                    || !self.anchors.is_empty()
                    || !self.source_assets.is_empty(),
                "assembly state has no professional timeline operation or clip anchors",
            ),
            stage(
                CapabilityArea::EditorialIntentAndReview,
                !self.cut_boundaries.is_empty() || !self.proposal_packages.is_empty(),
                "editorial intent and proposal evidence are missing",
            ),
            stage(
                CapabilityArea::ParameterAnimation,
                !self.parameter_animations.is_empty() || !self.expression_links.is_empty(),
                "parameter animations and expression links are missing",
            ),
            stage(
                CapabilityArea::MotionGraphicsTemplates,
                !self.motion_templates.is_empty(),
                "motion graphics templates are missing",
            ),
            stage(
                CapabilityArea::CompositionGraph,
                !self.composition_graphs.is_empty(),
                "composition graphs are missing",
            ),
            stage(
                CapabilityArea::TrackingMasksMattes,
                self.tracking_package.is_some(),
                "tracking, mask, and matte package is missing",
            ),
            stage(
                CapabilityArea::ColorFinishing,
                self.color_finishing.is_some(),
                "color finishing state is missing",
            ),
            stage(
                CapabilityArea::AudioFinishing,
                self.audio_finishing.is_some(),
                "audio finishing state is missing",
            ),
            stage(
                CapabilityArea::DeliveryProfilesAndPreflight,
                !self.delivery_profiles.is_empty() || !self.preflight_reports.is_empty(),
                "delivery profiles and preflight reports are missing",
            ),
            stage(
                CapabilityArea::WorkflowLenses,
                !self.workflow_lenses.is_empty(),
                "workflow lenses are missing",
            ),
            stage(
                CapabilityArea::PreAutonomyOrchestrationContract,
                self.capability_registry.is_some()
                    || self.pipeline_readiness.is_some()
                    || !self.planner_passes.is_empty(),
                "pre-autonomy registry/readiness/planner contracts are missing",
            ),
        ];
        for diagnostic in diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == FindingSeverity::Error)
        {
            if let Some(stage) = stages
                .iter_mut()
                .find(|stage| stage.area == diagnostic.area)
            {
                stage.state = ReadinessState::Blocked;
                if stage.blocker.is_none() {
                    stage.blocker = Some(diagnostic.message.clone());
                }
            }
        }

        let mut blockers: Vec<String> = stages
            .iter()
            .filter_map(|stage| stage.blocker.clone())
            .collect();
        for diagnostic in diagnostics
            .into_iter()
            .filter(|diagnostic| diagnostic.severity == FindingSeverity::Error)
        {
            if !blockers.contains(&diagnostic.message) {
                blockers.push(diagnostic.message);
            }
        }
        PipelineReadinessReport { stages, blockers }
    }

    fn asset_ids(&self) -> HashSet<&str> {
        self.asset_catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .assets
                    .iter()
                    .map(|asset| asset.id.as_str())
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn detect_parameter_animation_conflicts(
    animations: &[ParameterAnimation],
) -> Vec<ProfessionalDiagnostic> {
    type AnimationTimeRange = (f64, f64);
    type AnimationConflictEntry<'a> = (&'a str, AnimationTimeRange);

    let mut by_target: BTreeMap<String, Vec<AnimationConflictEntry<'_>>> = BTreeMap::new();
    for animation in animations {
        let Some((target_key, range)) = animation_clip_parameter_range(animation) else {
            continue;
        };
        by_target
            .entry(target_key)
            .or_default()
            .push((animation.id.as_str(), range));
    }

    let mut diagnostics = Vec::new();
    for (target_key, entries) in by_target {
        let Some((first_id, second_id)) = first_overlapping_animation_pair(&entries) else {
            continue;
        };
        diagnostics.push(ProfessionalDiagnostic::error(
            crate::professional::CapabilityArea::ParameterAnimation,
            format!(
                "animation conflict on {target_key}: {first_id} overlaps {second_id}; only one animation may target a clip parameter over the same time range"
            ),
        ));
    }
    diagnostics
}

fn animation_clip_parameter_range(animation: &ParameterAnimation) -> Option<(String, (f64, f64))> {
    let AnimationTarget::ClipParameter { clip_id, parameter } = &animation.target else {
        return None;
    };
    let mut start_s = f64::INFINITY;
    let mut end_s = f64::NEG_INFINITY;
    for keyframe in &animation.keyframes {
        if !keyframe.time_s.is_finite() {
            return None;
        }
        start_s = start_s.min(keyframe.time_s);
        end_s = end_s.max(keyframe.time_s);
    }
    if start_s == f64::INFINITY || end_s == f64::NEG_INFINITY {
        return None;
    }
    Some((format!("{clip_id}/{parameter}"), (start_s, end_s)))
}

fn detect_expression_link_cycles(links: &[ExpressionLink]) -> Vec<ProfessionalDiagnostic> {
    let targets = links
        .iter()
        .map(|link| (expression_target_key(link), link))
        .collect::<BTreeMap<_, _>>();

    for link in links {
        let mut seen = HashSet::new();
        let mut current = link;
        while let ExpressionSource::Parameter { clip_id, parameter } = &current.source {
            let key = format!("{clip_id}/{parameter}");
            if !seen.insert(key.clone()) {
                return vec![ProfessionalDiagnostic::error(
                    crate::professional::CapabilityArea::ParameterAnimation,
                    format!("expression link cycle detected at {key}"),
                )];
            }
            let Some(next) = targets.get(&key).copied() else {
                break;
            };
            current = next;
        }
    }

    Vec::new()
}

fn expression_target_key(link: &ExpressionLink) -> String {
    format!("{}/{}", link.target_clip_id, link.target_parameter)
}

fn first_overlapping_animation_pair<'a>(
    entries: &'a [(&'a str, (f64, f64))],
) -> Option<(&'a str, &'a str)> {
    for (index, (first_id, first_range)) in entries.iter().enumerate() {
        for (second_id, second_range) in entries.iter().skip(index + 1) {
            if ranges_overlap(*first_range, *second_range) {
                return Some((*first_id, *second_id));
            }
        }
    }
    None
}

fn ranges_overlap(first: (f64, f64), second: (f64, f64)) -> bool {
    first.0 <= second.1 && second.0 <= first.1
}

fn stage(
    area: crate::professional::CapabilityArea,
    ready: bool,
    blocker: &str,
) -> PipelineStageReadiness {
    PipelineStageReadiness {
        area,
        state: if ready {
            ReadinessState::Ready
        } else {
            ReadinessState::Blocked
        },
        blocker: if ready { None } else { Some(blocker.into()) },
    }
}

/// Reusable timeline-level broadcast overlay configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BroadcastOverlayConfig {
    /// Enable or disable rendering without deleting the saved preset.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Optional source template identifier for audit/debug UI.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub template_name: Option<String>,
    /// Episode title shown by the title card.
    #[serde(default)]
    pub episode_title: String,
    /// Optional subtitle shown under the title.
    #[serde(default)]
    pub episode_subtitle: String,
    /// Persistent show/brand name used by the ticker label.
    #[serde(default)]
    pub show_name: String,
    /// Left/primary host.
    #[serde(default)]
    pub host_a: BroadcastHost,
    /// Right/secondary host.
    #[serde(default)]
    pub host_b: BroadcastHost,
    /// Sponsor/brand names that scroll in the ticker.
    #[serde(default)]
    pub sponsors: Vec<String>,
    /// Timed topic labels used by the smart ticker.
    #[serde(default)]
    pub topics: Vec<BroadcastTimedEntry>,
    /// Timed chapter cards.
    #[serde(default)]
    pub chapters: Vec<BroadcastTimedEntry>,
    /// Optional project-relative logo path.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub brand_logo_path: Option<String>,
    /// Render only the minimal short-form brand bar.
    #[serde(default)]
    pub short_form_mode: bool,
    /// Timing, colour, and layout style.
    #[serde(default)]
    pub style: BroadcastOverlayStyle,
}

impl Default for BroadcastOverlayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            template_name: None,
            episode_title: String::new(),
            episode_subtitle: String::new(),
            show_name: String::new(),
            host_a: BroadcastHost::default(),
            host_b: BroadcastHost::default(),
            sponsors: Vec::new(),
            topics: Vec::new(),
            chapters: Vec::new(),
            brand_logo_path: None,
            short_form_mode: false,
            style: BroadcastOverlayStyle::default(),
        }
    }
}

/// Host data used by name bars and host intro strips.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BroadcastHost {
    /// Display name rendered in the lower-third.
    #[serde(default)]
    pub name: String,
    /// Subtitle / role rendered beside or below the name.
    #[serde(default)]
    pub title: String,
    /// Optional project-relative portrait path.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub photo_path: Option<String>,
}

/// Timestamped chapter/topic label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BroadcastTimedEntry {
    /// Timeline time, in seconds.
    pub time_seconds: f64,
    /// Text displayed at that time.
    pub text: String,
}

/// Visual/timing defaults for the broadcast overlay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BroadcastOverlayStyle {
    /// Primary brand gold colour.
    #[serde(default = "default_gold_hex")]
    pub gold_hex: String,
    /// Lighter gold used by split host-intro bars.
    #[serde(default = "default_gold_light_hex")]
    pub gold_light_hex: String,
    /// Accent cyan used by topic badges.
    #[serde(default = "default_cyan_hex")]
    pub cyan_hex: String,
    /// Dark lower-third/ticker background colour.
    #[serde(default = "default_dark_navy_hex")]
    pub dark_navy_hex: String,
    /// End of title-card fade-in, in seconds.
    #[serde(default = "default_title_fade_in_end")]
    pub title_fade_in_end: f64,
    /// Start of title-card fade-out, in seconds.
    #[serde(default = "default_title_fade_out_start")]
    pub title_fade_out_start: f64,
    /// End of title-card visibility, in seconds.
    #[serde(default = "default_title_visible_end")]
    pub title_visible_end: f64,
    /// Start of host-intro lower-third, in seconds.
    #[serde(default = "default_host_intro_start")]
    pub host_intro_start: f64,
    /// End of host-intro lower-third, in seconds.
    #[serde(default = "default_host_intro_end")]
    pub host_intro_end: f64,
    /// Sponsor ticker display cadence, in seconds.
    #[serde(default = "default_ticker_sponsor_duration")]
    pub ticker_sponsor_duration: f64,
    /// Ticker fade duration, in seconds.
    #[serde(default = "default_ticker_fade_duration")]
    pub ticker_fade_duration: f64,
    /// Topic badge display duration, in seconds.
    #[serde(default = "default_ticker_topic_duration")]
    pub ticker_topic_duration: f64,
    /// Chapter-card display duration, in seconds.
    #[serde(default = "default_chapter_display_duration")]
    pub chapter_display_duration: f64,
    /// Persistent host-name bar height, in pixels at 3840x2160 reference.
    #[serde(default = "default_name_bar_height")]
    pub name_bar_height: f64,
    /// Bottom ticker height, in pixels at 3840x2160 reference.
    #[serde(default = "default_ticker_height")]
    pub ticker_height: f64,
    /// Host-intro strip height, in pixels at 3840x2160 reference.
    #[serde(default = "default_host_strip_height")]
    pub host_strip_height: f64,
}

impl Default for BroadcastOverlayStyle {
    fn default() -> Self {
        Self {
            gold_hex: default_gold_hex(),
            gold_light_hex: default_gold_light_hex(),
            cyan_hex: default_cyan_hex(),
            dark_navy_hex: default_dark_navy_hex(),
            title_fade_in_end: default_title_fade_in_end(),
            title_fade_out_start: default_title_fade_out_start(),
            title_visible_end: default_title_visible_end(),
            host_intro_start: default_host_intro_start(),
            host_intro_end: default_host_intro_end(),
            ticker_sponsor_duration: default_ticker_sponsor_duration(),
            ticker_fade_duration: default_ticker_fade_duration(),
            ticker_topic_duration: default_ticker_topic_duration(),
            chapter_display_duration: default_chapter_display_duration(),
            name_bar_height: default_name_bar_height(),
            ticker_height: default_ticker_height(),
            host_strip_height: default_host_strip_height(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_gold_hex() -> String {
    "#C9A028".into()
}
fn default_gold_light_hex() -> String {
    "#E8C040".into()
}
fn default_cyan_hex() -> String {
    "#22D3EE".into()
}
fn default_dark_navy_hex() -> String {
    "#070D17".into()
}
fn default_title_fade_in_end() -> f64 {
    1.5
}
fn default_title_fade_out_start() -> f64 {
    29.0
}
fn default_title_visible_end() -> f64 {
    30.0
}
fn default_host_intro_start() -> f64 {
    38.0
}
fn default_host_intro_end() -> f64 {
    92.0
}
fn default_ticker_sponsor_duration() -> f64 {
    25.0
}
fn default_ticker_fade_duration() -> f64 {
    1.0
}
fn default_ticker_topic_duration() -> f64 {
    14.0
}
fn default_chapter_display_duration() -> f64 {
    6.0
}
fn default_name_bar_height() -> f64 {
    150.0
}
fn default_ticker_height() -> f64 {
    200.0
}
fn default_host_strip_height() -> f64 {
    320.0
}

/// Build the canonical key for timeline-level cut boundary metadata.
pub fn cut_boundary_key(from_clip_id: &str, to_clip_id: &str) -> String {
    format!("{from_clip_id}::{to_clip_id}")
}

/// Semantic editorial metadata for a hard-cut boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticCutSpec {
    /// Editorial grammar for the boundary.
    pub cut_type: CutType,
    /// Short machine-readable purpose, for example
    /// `"hide_action_continuity"` or `"speaker_handoff"`.
    pub intent: String,
    /// Optional energy/intensity in `[0, 1]`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub energy: Option<f64>,
    /// How audio relates to picture at this boundary.
    #[serde(default)]
    pub audio_relation: AudioRelation,
    /// Optional confidence in `[0, 1]` from the planner/tool.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confidence: Option<f64>,
    /// Human-readable reason suitable for proposal review or UI display.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
    /// Forward-compat passthrough for future visual anchors, match
    /// candidates, or tool traces.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Editorial grammar classes for hard cuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutType {
    /// Plain hard cut; usually the best choice when story/rhythm are clean.
    HardCut,
    /// Cut while motion/action carries through the boundary.
    CutOnAction,
    /// Cut to related coverage that hides or clarifies the main action.
    Cutaway,
    /// Detail shot inserted into an otherwise continuous scene.
    Insert,
    /// Cut following a subject's look or implied sightline.
    EyelineMatch,
    /// Dialogue grammar alternating complementary angles.
    ShotReverseShot,
    /// Graphic/action/composition match across two shots.
    MatchCut,
    /// Abrupt high-contrast cut for impact.
    SmashCut,
    /// Deliberate discontinuity or time compression.
    JumpCut,
    /// Alternating simultaneous or related action lines.
    CrossCut,
    /// Incoming audio leads picture.
    JCut,
    /// Outgoing audio trails picture.
    LCut,
}

/// Audio-picture relationship at a cut boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AudioRelation {
    /// Audio and picture cut together.
    #[default]
    Sync,
    /// Incoming audio starts before incoming picture.
    AudioLeads,
    /// Outgoing audio continues after outgoing picture.
    AudioTrails,
    /// Both sides overlap intentionally.
    Overlap,
    /// Audio cuts sharply as a deliberate effect.
    AudioCut,
}

/// Content anchor for a clip. Used by `apply_edl` to relocate a clip after
/// upstream edits drift its absolute timeline position.
///
/// New anchor channels (e.g. v1.5's `face_embedding_sha`) slot in via
/// [`Self::extra`] without breaking older engines.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Anchor {
    /// A snippet of transcript that uniquely identifies the clip.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub transcript_snippet: Option<String>,
    /// Index into the source asset's shot-boundary list.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub scene_change_index: Option<u32>,
    /// SHA of an audio fingerprint window — most robust anchor.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub audio_fingerprint_sha: Option<String>,
    /// Hash of the energy curve over the clip range — useful when the
    /// transcript is sparse.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub energy_curve_hash: Option<String>,
    /// Forward-compat passthrough.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Per-clip awidat metadata on a `Clip.metadata.awidat`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AwidatClipMetadata {
    /// The agent's reasoning for keeping / inserting this clip. Free text.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning: Option<String>,
    /// Cross-reference to an `edit-plan.json` item id.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub edit_plan_ref: Option<String>,
    /// Per-clip anchor — same shape as in [`AwidatTimelineMetadata::anchors`]
    /// but inlined for clips that prefer it on the clip object. Either
    /// location is allowed; tools should check both.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub anchor: Option<Anchor>,
    /// Intentional audio-picture offset for J-cuts and L-cuts.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub split_edit: Option<SplitEditSpec>,
    /// Forward-compat passthrough.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Per-clip split-edit metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SplitEditSpec {
    /// Incoming audio starts this many seconds before picture. This is
    /// normally stamped on the incoming clip for a J-cut.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub audio_lead_s: Option<f64>,
    /// Clip id whose outgoing picture boundary this incoming lead was
    /// authored against. Used to drop stale J-cut offsets after deletes
    /// or moves change the neighboring boundary.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub audio_lead_from_clip_id: Option<String>,
    /// Outgoing audio continues this many seconds after picture. This is
    /// normally stamped on the outgoing clip for an L-cut.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub audio_trail_s: Option<f64>,
    /// Clip id whose incoming picture boundary this outgoing trail was
    /// authored against. Used to drop stale L-cut offsets after deletes
    /// or moves change the neighboring boundary.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub audio_trail_to_clip_id: Option<String>,
    /// Human-readable reason suitable for proposal review or UI display.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
    /// Optional planner confidence in `[0, 1]`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confidence: Option<f64>,
}

/// Per-marker awidat metadata on a `Marker.metadata.awidat`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AwidatMarkerMetadata {
    /// Marker category, e.g. `"laugh"`, `"key-quote"`, `"b-roll-cue"`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub category: Option<String>,
    /// Free-form note attached to the marker.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
    /// Forward-compat passthrough.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_partial_roundtrip() {
        let a = Anchor {
            transcript_snippet: Some("hello".into()),
            ..Anchor::default()
        };
        let s = serde_json::to_string(&a).unwrap();
        // Optional `None` fields should be omitted.
        assert!(!s.contains("scene_change_index"));
        let back: Anchor = serde_json::from_str(&s).unwrap();
        assert_eq!(back.transcript_snippet.as_deref(), Some("hello"));
    }

    #[test]
    fn timeline_metadata_roundtrip() {
        let mut m = AwidatTimelineMetadata {
            version: "0.1".into(),
            source_assets: vec!["raw/foo.mp4".into()],
            ..AwidatTimelineMetadata::default()
        };
        m.cut_boundaries.insert(
            cut_boundary_key("clip-a", "clip-b"),
            SemanticCutSpec {
                cut_type: CutType::Cutaway,
                intent: "hide_visual_jump".into(),
                energy: Some(0.4),
                audio_relation: AudioRelation::Sync,
                confidence: Some(0.8),
                reason: Some("b-roll clarifies the sentence".into()),
                extra: HashMap::new(),
            },
        );
        let s = serde_json::to_string(&m).unwrap();
        let back: AwidatTimelineMetadata = serde_json::from_str(&s).unwrap();
        assert_eq!(back.version, "0.1");
        assert_eq!(back.source_assets.len(), 1);
        let spec = back
            .cut_boundaries
            .get("clip-a::clip-b")
            .expect("cut boundary metadata should round-trip");
        assert_eq!(spec.cut_type, CutType::Cutaway);
        assert_eq!(spec.intent, "hide_visual_jump");
    }

    #[test]
    fn unknown_fields_round_trip_through_extra() {
        // A future engine adds a `taste_profile_id` field to the awidat
        // namespace. An older reader (this code) deserializes it into
        // `extra`, then serializes it back out unchanged.
        let json = serde_json::json!({
            "version": "0.1",
            "source_assets": [],
            "taste_profile_id": "tp-001",
            "fancy_new_field": { "nested": true }
        });
        let m: AwidatTimelineMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(
            m.extra.get("taste_profile_id").and_then(|v| v.as_str()),
            Some("tp-001")
        );
        // Round-trips intact.
        let out = serde_json::to_value(&m).unwrap();
        assert_eq!(out["taste_profile_id"], "tp-001");
        assert_eq!(out["fancy_new_field"]["nested"], true);
    }

    #[test]
    fn clip_split_edit_roundtrip() {
        let m = AwidatClipMetadata {
            split_edit: Some(SplitEditSpec {
                audio_lead_s: Some(0.35),
                audio_lead_from_clip_id: Some("clip-a".into()),
                audio_trail_s: Some(0.5),
                audio_trail_to_clip_id: Some("clip-b".into()),
                reason: Some("smooth speaker handoff".into()),
                confidence: Some(0.9),
            }),
            ..AwidatClipMetadata::default()
        };
        let s = serde_json::to_string(&m).unwrap();
        let back: AwidatClipMetadata = serde_json::from_str(&s).unwrap();
        let split = back.split_edit.expect("split edit");
        assert_eq!(split.audio_lead_s, Some(0.35));
        assert_eq!(split.audio_lead_from_clip_id.as_deref(), Some("clip-a"));
        assert_eq!(split.audio_trail_s, Some(0.5));
        assert_eq!(split.audio_trail_to_clip_id.as_deref(), Some("clip-b"));
        assert_eq!(split.reason.as_deref(), Some("smooth speaker handoff"));
    }
}
