//! Professional editing substrate contracts.
//!
//! These types are durable, serializable building blocks for the agent-native
//! editing pipeline: source organization, selects, assembly review, animation,
//! motion graphics, compositing, tracking, finishing, delivery, workflow lenses,
//! and pre-autonomy orchestration.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// Asset catalog independent of timeline clip usage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssetCatalog {
    /// Physical or logical source assets.
    #[serde(default)]
    pub assets: Vec<AssetRecord>,
    /// Human-authored organization.
    #[serde(default)]
    pub bins: Vec<AssetBin>,
    /// Query-backed organization.
    #[serde(default)]
    pub smart_collections: Vec<SmartCollection>,
    /// Agent/user-curated stable asset sets.
    #[serde(default)]
    pub selection_sets: Vec<AssetSelectionSet>,
}

impl AssetCatalog {
    /// Query assets by common professional review fields.
    pub fn query(&self, query: &AssetQuery) -> Vec<&AssetRecord> {
        self.assets
            .iter()
            .filter(|asset| query.matches(asset))
            .collect()
    }

    /// Validate catalog integrity.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut ids = HashSet::new();
        let bin_ids: HashSet<&str> = self.bins.iter().map(|bin| bin.id.as_str()).collect();
        for asset in &self.assets {
            if asset.id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::AssetCatalog,
                    "asset catalog contains an asset with an empty id",
                ));
            }
            if !ids.insert(asset.id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::AssetCatalog,
                    format!("asset catalog contains duplicate asset id {}", asset.id),
                ));
            }
            if asset.path.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::AssetCatalog,
                    format!("asset {} has an empty path", asset.id),
                ));
            }
            if let Some(bin_id) = &asset.bin_id
                && !bin_ids.contains(bin_id.as_str())
            {
                diagnostics.push(ProfessionalDiagnostic::warning(
                    CapabilityArea::AssetCatalog,
                    format!("asset {} references missing bin {}", asset.id, bin_id),
                ));
            }
            if let Some(rating) = asset.rating
                && rating > 5
            {
                diagnostics.push(ProfessionalDiagnostic::warning(
                    CapabilityArea::AssetCatalog,
                    format!("asset {} rating {rating} is outside 0..=5", asset.id),
                ));
            }
        }
        diagnostics
    }
}

/// Asset catalog query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetQuery {
    /// Optional bin id.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bin_id: Option<String>,
    /// Optional role.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub role: Option<AssetRole>,
    /// Required tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Required readiness state for proxy, index, and online.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub readiness: Option<ReadinessState>,
}

impl AssetQuery {
    fn matches(&self, asset: &AssetRecord) -> bool {
        if let Some(bin_id) = &self.bin_id
            && asset.bin_id.as_deref() != Some(bin_id.as_str())
        {
            return false;
        }
        if let Some(role) = self.role
            && asset.role != role
        {
            return false;
        }
        if !self.tags.iter().all(|tag| asset.tags.contains(tag)) {
            return false;
        }
        if let Some(readiness) = self.readiness
            && (asset.readiness.proxy != readiness
                || asset.readiness.index != readiness
                || asset.readiness.online != readiness)
        {
            return false;
        }
        true
    }
}

/// One source asset known to the project.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssetRecord {
    /// Stable asset id.
    pub id: String,
    /// Project-relative path or source URI.
    pub path: String,
    /// Editorial role.
    #[serde(default)]
    pub role: AssetRole,
    /// Optional bin id.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bin_id: Option<String>,
    /// User and agent tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional label color/name.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
    /// Optional 0..=5 rating.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rating: Option<u8>,
    /// Readiness across proxy/index/online state.
    #[serde(default)]
    pub readiness: AssetReadiness,
    /// Source lineage and ingest notes.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub provenance: Option<AssetProvenance>,
    /// Usage status in timelines/stringouts.
    #[serde(default)]
    pub usage: AssetUsage,
}

/// Broad editorial asset role.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetRole {
    /// Video or video+audio source.
    #[default]
    Video,
    /// Dedicated audio source.
    Audio,
    /// Still image.
    Still,
    /// Graphic or design asset.
    Graphic,
    /// Caption/subtitle source.
    Caption,
    /// LUT, preset, or support media.
    Support,
}

/// Asset readiness state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetReadiness {
    /// Proxy availability.
    #[serde(default)]
    pub proxy: ReadinessState,
    /// Evidence/index sidecar availability.
    #[serde(default)]
    pub index: ReadinessState,
    /// Original media online/relink state.
    #[serde(default)]
    pub online: ReadinessState,
}

/// Common readiness state for pipeline stages and assets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessState {
    /// Ready to use.
    Ready,
    /// Work is pending or incomplete.
    #[default]
    Pending,
    /// Blocked by missing media/data/user input.
    Blocked,
    /// Explicitly unavailable or unsupported.
    Unavailable,
}

/// Source lineage and integrity data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssetProvenance {
    /// Import source.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub imported_from: Option<String>,
    /// Optional checksum for relink/integrity checks.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub checksum: Option<String>,
    /// Optional ingest tool or agent id.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub created_by: Option<String>,
}

/// Asset usage summary.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AssetUsage {
    /// Timeline clip ids using this asset.
    #[serde(default)]
    pub clip_ids: Vec<String>,
    /// Select ids using this asset.
    #[serde(default)]
    pub select_ids: Vec<String>,
    /// Whether the asset has been rejected for current editorial purposes.
    #[serde(default)]
    pub rejected: bool,
}

/// Manual bin.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetBin {
    /// Stable bin id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Optional parent bin id.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parent_id: Option<String>,
}

/// Query-backed collection.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SmartCollection {
    /// Stable collection id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Structured filter expression.
    #[serde(default)]
    pub filter: HashMap<String, serde_json::Value>,
}

/// Stable set of assets selected for a task.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetSelectionSet {
    /// Stable set id.
    pub id: String,
    /// Asset ids.
    #[serde(default)]
    pub asset_ids: Vec<String>,
    /// Purpose or task label.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub purpose: Option<String>,
}

/// Time range in source or timeline seconds.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceRange {
    /// Inclusive start in seconds.
    pub start_s: f64,
    /// Exclusive end in seconds.
    pub end_s: f64,
}

impl SourceRange {
    /// True when the range is finite and has positive duration.
    pub fn is_valid(&self) -> bool {
        self.start_s.is_finite() && self.end_s.is_finite() && self.end_s > self.start_s
    }
}

/// Durable source review decision.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceSelect {
    /// Stable select id.
    pub id: String,
    /// Source asset id.
    pub asset_id: String,
    /// Selected source range.
    pub range: SourceRange,
    /// Review decision.
    #[serde(default)]
    pub decision: SelectDecision,
    /// Optional take group id.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub take_group_id: Option<String>,
    /// Optional rank within a take group.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rank: Option<u32>,
    /// Human-readable keep/reject reason.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
    /// Evidence sidecar references.
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    /// Free-form notes.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Source review decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectDecision {
    /// Keep this range.
    #[default]
    Select,
    /// Reject this range.
    Reject,
    /// Maybe keep this range.
    Maybe,
    /// Strong positive signal.
    Favorite,
}

/// Ordered pre-timeline source assembly.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stringout {
    /// Stable stringout id.
    pub id: String,
    /// Display name.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    /// Ordered select ids.
    #[serde(default)]
    pub select_ids: Vec<String>,
}

/// Unified proposal package for any pipeline stage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProposalPackage {
    /// Stable proposal id.
    pub id: String,
    /// Pipeline capability area.
    pub area: CapabilityArea,
    /// Review lifecycle.
    #[serde(default)]
    pub status: ReviewStatus,
    /// User-facing summary.
    pub summary: String,
    /// Operation/evidence trace.
    #[serde(default)]
    pub evidence: Vec<EvidenceTrace>,
    /// Optional rollback context.
    #[serde(default)]
    pub rollback_refs: Vec<String>,
}

/// Evidence attached to a proposal or decision.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EvidenceTrace {
    /// Evidence source, such as `whisper` or `color_analysis`.
    pub source: String,
    /// Referenced asset/select/clip ids.
    #[serde(default)]
    pub refs: Vec<String>,
    /// Reviewable explanation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
    /// Optional confidence in 0..=1.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confidence: Option<f64>,
}

/// Review lifecycle for non-timeline and timeline artifacts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// Proposed and awaiting user decision.
    #[default]
    Proposed,
    /// Accepted.
    Accepted,
    /// Rejected.
    Rejected,
    /// Superseded by a newer proposal.
    Superseded,
}

/// General parameter animation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParameterAnimation {
    /// Stable animation id.
    pub id: String,
    /// Animation target.
    pub target: AnimationTarget,
    /// Ordered keyframes.
    #[serde(default)]
    pub keyframes: Vec<Keyframe>,
    /// Optional summary for review.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rationale: Option<String>,
}

impl ParameterAnimation {
    /// Validate animation target and keyframe ordering.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                "parameter animation has an empty id",
            ));
        }
        if matches!(self.target, AnimationTarget::Unset) {
            diagnostics.push(ProfessionalDiagnostic::warning(
                CapabilityArea::ParameterAnimation,
                format!("parameter animation {} has no target", self.id),
            ));
        }
        let mut previous_time = None;
        for keyframe in &self.keyframes {
            if !keyframe.time_s.is_finite() || !keyframe.value.is_finite() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::ParameterAnimation,
                    format!("parameter animation {} has a non-finite keyframe", self.id),
                ));
            }
            if let Some(previous) = previous_time
                && keyframe.time_s < previous
            {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::ParameterAnimation,
                    format!(
                        "parameter animation {} keyframes must be sorted by time",
                        self.id
                    ),
                ));
            }
            previous_time = Some(keyframe.time_s);
        }
        diagnostics
    }
}

/// Parameter animation target.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnimationTarget {
    /// No target yet.
    #[default]
    Unset,
    /// Clip effect or transform parameter.
    ClipParameter {
        /// Clip id.
        clip_id: String,
        /// Parameter path.
        parameter: String,
    },
    /// Composition node parameter.
    CompositionNodeParameter {
        /// Composition id.
        composition_id: String,
        /// Node id.
        node_id: String,
        /// Parameter path.
        parameter: String,
    },
    /// Track or bus parameter.
    TrackParameter {
        /// Track id/name.
        track: String,
        /// Parameter path.
        parameter: String,
    },
}

/// One animation keyframe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Time in seconds.
    pub time_s: f64,
    /// Numeric value.
    pub value: f64,
    /// Interpolation into the next keyframe.
    #[serde(default)]
    pub interpolation: KeyframeInterpolation,
    /// Easing curve.
    #[serde(default)]
    pub easing: Easing,
}

impl Keyframe {
    /// Convenience constructor for a linear keyframe.
    pub fn linear(time_s: f64, value: f64) -> Self {
        Self {
            time_s,
            value,
            interpolation: KeyframeInterpolation::Linear,
            easing: Easing::Linear,
        }
    }
}

/// Keyframe interpolation mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyframeInterpolation {
    /// Hold previous value.
    Hold,
    /// Linear interpolation.
    #[default]
    Linear,
    /// Bezier curve handles.
    Bezier,
}

/// Easing mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    /// No easing.
    #[default]
    Linear,
    /// Ease in.
    EaseIn,
    /// Ease out.
    EaseOut,
    /// Ease in and out.
    EaseInOut,
}

/// Reusable motion graphics template.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MotionGraphicsTemplate {
    /// Template id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Editable slots.
    #[serde(default)]
    pub slots: Vec<TemplateSlot>,
    /// Safe-area rules.
    #[serde(default)]
    pub safe_areas: Vec<SafeAreaRule>,
    /// Platform-specific variants.
    #[serde(default)]
    pub platform_variants: Vec<String>,
}

impl MotionGraphicsTemplate {
    /// Validate required slots and safe-area constraints.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        for slot in &self.slots {
            if slot.required && slot.value.is_none() {
                diagnostics.push(ProfessionalDiagnostic::warning(
                    CapabilityArea::MotionGraphicsTemplates,
                    format!(
                        "motion template {} required slot {} is not filled",
                        self.id, slot.id
                    ),
                ));
            }
        }
        for rule in &self.safe_areas {
            if !rule.margin_pct.is_finite() || !(0.0..=0.5).contains(&rule.margin_pct) {
                diagnostics.push(ProfessionalDiagnostic::warning(
                    CapabilityArea::MotionGraphicsTemplates,
                    format!(
                        "motion template {} safe area {} margin must be in 0..=0.5",
                        self.id, rule.profile
                    ),
                ));
            }
        }
        diagnostics
    }
}

/// Template slot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TemplateSlot {
    /// Slot id.
    pub id: String,
    /// Slot kind.
    #[serde(default)]
    pub kind: TemplateSlotKind,
    /// Filled value.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub value: Option<serde_json::Value>,
    /// Whether the slot must be filled before render.
    #[serde(default)]
    pub required: bool,
}

/// Template slot kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateSlotKind {
    /// Text content.
    #[default]
    Text,
    /// Image/graphic asset.
    Image,
    /// Color value.
    Color,
    /// Numeric value.
    Number,
}

/// Safe-area rule.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SafeAreaRule {
    /// Profile name.
    pub profile: String,
    /// Margin as fraction of width/height.
    pub margin_pct: f64,
}

/// Serializable composition graph.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompositionGraph {
    /// Composition id.
    pub id: String,
    /// Nodes.
    #[serde(default)]
    pub nodes: Vec<CompositionNode>,
    /// Edges.
    #[serde(default)]
    pub edges: Vec<CompositionEdge>,
    /// Output node id.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_node_id: Option<String>,
}

impl CompositionGraph {
    /// Minimal graph with one output node.
    pub fn single_output(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            id,
            nodes: vec![CompositionNode {
                id: "output".into(),
                node_type: CompositionNodeType::Output,
                ..CompositionNode::default()
            }],
            edges: Vec::new(),
            output_node_id: Some("output".into()),
        }
    }

    /// Validate node references and output reachability contract.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        let node_ids: HashSet<&str> = self.nodes.iter().map(|node| node.id.as_str()).collect();
        if self.nodes.is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::CompositionGraph,
                format!("composition graph {} has no nodes", self.id),
            ));
        }
        match &self.output_node_id {
            Some(output_node_id) if node_ids.contains(output_node_id.as_str()) => {}
            Some(output_node_id) => diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::CompositionGraph,
                format!(
                    "composition graph {} output node {} is missing",
                    self.id, output_node_id
                ),
            )),
            None => diagnostics.push(ProfessionalDiagnostic::warning(
                CapabilityArea::CompositionGraph,
                format!("composition graph {} has no output node", self.id),
            )),
        }
        for edge in &self.edges {
            if !node_ids.contains(edge.from.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::CompositionGraph,
                    format!(
                        "composition graph {} edge references missing source node {}",
                        self.id, edge.from
                    ),
                ));
            }
            if !node_ids.contains(edge.to.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::CompositionGraph,
                    format!(
                        "composition graph {} edge references missing destination node {}",
                        self.id, edge.to
                    ),
                ));
            }
        }
        diagnostics
    }
}

/// Composition node.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompositionNode {
    /// Node id.
    pub id: String,
    /// Node type.
    #[serde(default)]
    pub node_type: CompositionNodeType,
    /// Node parameters.
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

/// Composition node type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositionNodeType {
    /// Media input.
    MediaInput,
    /// Transform.
    Transform,
    /// Merge.
    Merge,
    /// Mask.
    Mask,
    /// Matte.
    Matte,
    /// Text.
    Text,
    /// Blur.
    Blur,
    /// Color.
    Color,
    /// Tracker binding.
    TrackerBind,
    /// Graph output.
    #[default]
    Output,
}

/// Directed graph edge.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositionEdge {
    /// Source node id.
    pub from: String,
    /// Destination node id.
    pub to: String,
    /// Optional input port.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub input: Option<String>,
}

/// Tracking, mask, and matte sidecar contracts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackingPackage {
    /// Point/planar/surface tracks.
    #[serde(default)]
    pub tracks: Vec<TrackSidecar>,
    /// Keyframed masks.
    #[serde(default)]
    pub masks: Vec<MaskSidecar>,
    /// Alpha mattes.
    #[serde(default)]
    pub mattes: Vec<MatteSidecar>,
}

impl TrackingPackage {
    /// Validate tracking, mask, and matte quality data.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        for track in &self.tracks {
            if track.samples.is_empty() {
                diagnostics.push(ProfessionalDiagnostic::warning(
                    CapabilityArea::TrackingMasksMattes,
                    format!("track {} has no samples", track.id),
                ));
            }
            validate_optional_confidence(
                &mut diagnostics,
                CapabilityArea::TrackingMasksMattes,
                track.confidence,
                format!("track {}", track.id),
            );
        }
        for mask in &self.masks {
            for keyframe in &mask.keyframes {
                if !keyframe.time_s.is_finite()
                    || !keyframe.feather.is_finite()
                    || !(0.0..=1.0).contains(&keyframe.opacity)
                {
                    diagnostics.push(ProfessionalDiagnostic::warning(
                        CapabilityArea::TrackingMasksMattes,
                        format!("mask {} has invalid keyframe values", mask.id),
                    ));
                }
            }
        }
        for matte in &self.mattes {
            validate_optional_confidence(
                &mut diagnostics,
                CapabilityArea::TrackingMasksMattes,
                matte.confidence,
                format!("matte {}", matte.id),
            );
        }
        diagnostics
    }
}

/// Track sidecar.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackSidecar {
    /// Track id.
    pub id: String,
    /// Asset id.
    pub asset_id: String,
    /// Track kind.
    #[serde(default)]
    pub kind: TrackKind,
    /// Coordinate space.
    #[serde(default)]
    pub coordinate_space: CoordinateSpace,
    /// Per-frame samples.
    #[serde(default)]
    pub samples: Vec<TrackSample>,
    /// Quality confidence.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confidence: Option<f64>,
}

/// Track type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    /// Point track.
    #[default]
    Point,
    /// Planar track.
    Planar,
    /// Surface/corner-pin track.
    Surface,
}

/// Coordinate space.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinateSpace {
    /// Normalized 0..1 frame coordinates.
    #[default]
    Normalized,
    /// Pixel coordinates.
    Pixels,
}

/// One tracking sample.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackSample {
    /// Frame number.
    pub frame: u64,
    /// Points as `[x, y]`.
    #[serde(default)]
    pub points: Vec<[f64; 2]>,
    /// Confidence for this frame.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confidence: Option<f64>,
}

/// Mask sidecar.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MaskSidecar {
    /// Mask id.
    pub id: String,
    /// Mask operation.
    #[serde(default)]
    pub operation: MaskOperation,
    /// Keyframed paths.
    #[serde(default)]
    pub keyframes: Vec<MaskKeyframe>,
}

/// Mask boolean operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskOperation {
    /// Additive mask.
    #[default]
    Add,
    /// Subtractive mask.
    Subtract,
    /// Intersect mask.
    Intersect,
}

/// One mask keyframe.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MaskKeyframe {
    /// Time in seconds.
    pub time_s: f64,
    /// Closed path points.
    #[serde(default)]
    pub points: Vec<[f64; 2]>,
    /// Feather amount.
    #[serde(default)]
    pub feather: f64,
    /// Opacity 0..=1.
    #[serde(default = "default_one")]
    pub opacity: f64,
}

/// Matte sidecar.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MatteSidecar {
    /// Matte id.
    pub id: String,
    /// Alpha source path or sidecar ref.
    pub alpha_source: String,
    /// Quality confidence.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confidence: Option<f64>,
    /// Review thumbnail paths.
    #[serde(default)]
    pub review_thumbnails: Vec<String>,
}

/// Color finishing workflow state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ColorFinishingState {
    /// Reference stills/gallery.
    #[serde(default)]
    pub reference_stills: Vec<ReferenceStill>,
    /// Shot groups for matching.
    #[serde(default)]
    pub shot_groups: Vec<ShotGroup>,
    /// Grade stacks.
    #[serde(default)]
    pub grade_stacks: Vec<GradeStack>,
    /// Color management target.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color_management: Option<ColorManagement>,
}

/// Reference still.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReferenceStill {
    /// Still id.
    pub id: String,
    /// Asset or image path.
    pub source: String,
}

/// Group of shots to match together.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShotGroup {
    /// Group id.
    pub id: String,
    /// Clip ids.
    #[serde(default)]
    pub clip_ids: Vec<String>,
}

/// Grade stack.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GradeStack {
    /// Stack id.
    pub id: String,
    /// Ordered stages.
    #[serde(default)]
    pub stages: Vec<GradeStage>,
}

/// Grade stage.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GradeStage {
    /// Stage id.
    pub id: String,
    /// Stage kind.
    pub kind: String,
    /// Parameters.
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

/// Color management contract.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorManagement {
    /// Input color space.
    pub input_color_space: String,
    /// Output color space.
    pub output_color_space: String,
}

/// Audio finishing workflow state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioFinishingState {
    /// Mixer buses.
    #[serde(default)]
    pub buses: Vec<AudioBus>,
    /// Automation lanes.
    #[serde(default)]
    pub automation: Vec<AudioAutomationLane>,
    /// Reusable processing chains.
    #[serde(default)]
    pub chains: Vec<AudioChainPreset>,
    /// Review measurements.
    #[serde(default)]
    pub meters: Vec<AudioMeterReading>,
}

/// Audio role.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioRole {
    /// Dialogue.
    #[default]
    Dialogue,
    /// Music.
    Music,
    /// Sound effects.
    Sfx,
    /// Ambience.
    Ambience,
    /// Voiceover.
    Voiceover,
    /// Master bus.
    Master,
}

/// Audio bus.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioBus {
    /// Bus id.
    pub id: String,
    /// Bus role.
    #[serde(default)]
    pub role: AudioRole,
    /// Input track ids/names.
    #[serde(default)]
    pub inputs: Vec<String>,
}

/// Audio automation lane.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioAutomationLane {
    /// Target bus/track id.
    pub target: String,
    /// Parameter name.
    pub parameter: String,
    /// Keyframes.
    #[serde(default)]
    pub keyframes: Vec<Keyframe>,
}

/// Reusable audio processing chain.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioChainPreset {
    /// Chain id.
    pub id: String,
    /// Ordered processors.
    #[serde(default)]
    pub processors: Vec<String>,
}

/// Audio meter reading.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioMeterReading {
    /// Target bus/track.
    pub target: String,
    /// Integrated loudness.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub integrated_lufs: Option<f64>,
    /// True peak.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub true_peak_db: Option<f64>,
    /// Noise floor.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub noise_floor_db: Option<f64>,
    /// Clipping detected.
    #[serde(default)]
    pub clipping: bool,
}

/// Named delivery profile.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeliveryProfile {
    /// Profile id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Platform target.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub platform: Option<String>,
    /// Aspect ratio.
    pub aspect_ratio: String,
    /// Resolution width.
    pub width: u32,
    /// Resolution height.
    pub height: u32,
    /// Video bitrate in kbps.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub video_bitrate_kbps: Option<u32>,
    /// Loudness target.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub loudness_lufs: Option<f64>,
    /// Required checks.
    #[serde(default)]
    pub preflight_checks: Vec<PreflightCheckKind>,
}

impl DeliveryProfile {
    /// Baseline YouTube 1080p profile.
    pub fn youtube_1080p() -> Self {
        Self {
            id: "youtube_1080p".into(),
            name: "YouTube 1080p".into(),
            platform: Some("youtube".into()),
            aspect_ratio: "16:9".into(),
            width: 1920,
            height: 1080,
            video_bitrate_kbps: Some(12_000),
            loudness_lufs: Some(-14.0),
            preflight_checks: vec![
                PreflightCheckKind::AspectRatio,
                PreflightCheckKind::Loudness,
                PreflightCheckKind::Captions,
                PreflightCheckKind::Metadata,
            ],
        }
    }

    /// Run delivery preflight against measured project/output facts.
    pub fn run_preflight(&self, input: DeliveryPreflightInput) -> PreflightReport {
        let mut findings = Vec::new();
        if self
            .preflight_checks
            .contains(&PreflightCheckKind::AspectRatio)
            && input.aspect_ratio != self.aspect_ratio
        {
            findings.push(PreflightFinding::error(
                PreflightCheckKind::AspectRatio,
                format!(
                    "aspect ratio {} does not match delivery profile {}",
                    input.aspect_ratio, self.aspect_ratio
                ),
            ));
        }
        if self.preflight_checks.contains(&PreflightCheckKind::Bitrate)
            && let (Some(actual), Some(target)) =
                (input.video_bitrate_kbps, self.video_bitrate_kbps)
            && actual < target / 2
        {
            findings.push(PreflightFinding::warning(
                PreflightCheckKind::Bitrate,
                format!("bitrate {actual} kbps is far below target {target} kbps"),
            ));
        }
        if self
            .preflight_checks
            .contains(&PreflightCheckKind::Loudness)
            && let (Some(actual), Some(target)) = (input.integrated_lufs, self.loudness_lufs)
            && (actual - target).abs() > 2.0
        {
            findings.push(PreflightFinding::warning(
                PreflightCheckKind::Loudness,
                format!("loudness {actual:.1} LUFS is outside target {target:.1} LUFS"),
            ));
        }
        if self
            .preflight_checks
            .contains(&PreflightCheckKind::Captions)
            && !input.has_captions
        {
            findings.push(PreflightFinding::warning(
                PreflightCheckKind::Captions,
                "captions are missing",
            ));
        }
        if self
            .preflight_checks
            .contains(&PreflightCheckKind::Metadata)
            && !input.has_required_metadata
        {
            findings.push(PreflightFinding::warning(
                PreflightCheckKind::Metadata,
                "required delivery metadata is missing",
            ));
        }
        if self
            .preflight_checks
            .contains(&PreflightCheckKind::Thumbnail)
            && !input.has_thumbnail
        {
            findings.push(PreflightFinding::warning(
                PreflightCheckKind::Thumbnail,
                "thumbnail is missing",
            ));
        }
        if self
            .preflight_checks
            .contains(&PreflightCheckKind::SafeAreas)
            && input.safe_area_violations > 0
        {
            findings.push(PreflightFinding::warning(
                PreflightCheckKind::SafeAreas,
                format!("{} safe-area violations found", input.safe_area_violations),
            ));
        }
        if let Some(duration_s) = input.duration_s
            && self
                .preflight_checks
                .contains(&PreflightCheckKind::Duration)
            && (!duration_s.is_finite() || duration_s <= 0.0)
        {
            findings.push(PreflightFinding::error(
                PreflightCheckKind::Duration,
                "duration must be positive and finite",
            ));
        }
        PreflightReport {
            id: format!("preflight-{}", self.id),
            profile_id: self.id.clone(),
            findings,
        }
    }
}

/// Facts measured before delivery.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeliveryPreflightInput {
    /// Actual aspect ratio.
    pub aspect_ratio: String,
    /// Optional duration.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub duration_s: Option<f64>,
    /// Optional video bitrate.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub video_bitrate_kbps: Option<u32>,
    /// Optional integrated loudness.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub integrated_lufs: Option<f64>,
    /// Captions present.
    #[serde(default)]
    pub has_captions: bool,
    /// Required metadata present.
    #[serde(default)]
    pub has_required_metadata: bool,
    /// Thumbnail present.
    #[serde(default)]
    pub has_thumbnail: bool,
    /// Safe area violation count.
    #[serde(default)]
    pub safe_area_violations: u32,
}

/// Preflight check kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightCheckKind {
    /// Aspect ratio.
    #[default]
    AspectRatio,
    /// Duration.
    Duration,
    /// Bitrate.
    Bitrate,
    /// Captions.
    Captions,
    /// Loudness.
    Loudness,
    /// Safe areas.
    SafeAreas,
    /// Metadata.
    Metadata,
    /// Thumbnail.
    Thumbnail,
}

/// Preflight report.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PreflightReport {
    /// Report id.
    pub id: String,
    /// Delivery profile id.
    pub profile_id: String,
    /// Findings.
    #[serde(default)]
    pub findings: Vec<PreflightFinding>,
}

/// Preflight finding.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PreflightFinding {
    /// Check kind.
    pub check: PreflightCheckKind,
    /// Severity.
    #[serde(default)]
    pub severity: FindingSeverity,
    /// User-facing message.
    pub message: String,
    /// Optional fix proposal id or EDL.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fix_ref: Option<String>,
}

impl PreflightFinding {
    fn warning(check: PreflightCheckKind, message: impl Into<String>) -> Self {
        Self {
            check,
            severity: FindingSeverity::Warning,
            message: message.into(),
            fix_ref: None,
        }
    }

    fn error(check: PreflightCheckKind, message: impl Into<String>) -> Self {
        Self {
            check,
            severity: FindingSeverity::Error,
            message: message.into(),
            fix_ref: None,
        }
    }
}

/// Finding severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    /// Informational.
    Info,
    /// Warning.
    #[default]
    Warning,
    /// Error/blocker.
    Error,
}

/// Deliverable package manifest.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PackageManifest {
    /// Manifest id.
    pub id: String,
    /// Artifact paths.
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// Post-render validation report ids.
    #[serde(default)]
    pub validation_reports: Vec<String>,
}

/// Agent-native workflow lens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowLens {
    /// Media organization.
    Media,
    /// Source review/selects.
    Selects,
    /// Assembly.
    Assembly,
    /// Edit review.
    EditReview,
    /// VFX/compositing.
    Vfx,
    /// Color.
    Color,
    /// Audio.
    Audio,
    /// Delivery.
    Delivery,
    /// Preflight.
    Preflight,
}

/// The 13 core professional capability areas.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityArea {
    /// Asset catalog.
    #[default]
    AssetCatalog,
    /// Source review/selects.
    SourceReviewSelects,
    /// Assembly/timeline operations.
    AssemblyAndTimelineOperations,
    /// Editorial intent/review.
    EditorialIntentAndReview,
    /// Parameter animation.
    ParameterAnimation,
    /// Motion graphics templates.
    MotionGraphicsTemplates,
    /// Composition graph.
    CompositionGraph,
    /// Tracking, masks, mattes.
    TrackingMasksMattes,
    /// Color finishing.
    ColorFinishing,
    /// Audio finishing.
    AudioFinishing,
    /// Delivery profiles/preflight.
    DeliveryProfilesAndPreflight,
    /// Workflow lenses.
    WorkflowLenses,
    /// Pre-autonomy orchestration contracts.
    PreAutonomyOrchestrationContract,
}

/// Capability registry entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityStatus {
    /// Capability area.
    pub area: CapabilityArea,
    /// Operation has a typed contract.
    pub available: bool,
    /// Desktop/agent can preview or summarize it.
    pub previewable: bool,
    /// Renderer can lower it fully.
    pub renderable: bool,
    /// Preflight can validate it.
    pub preflighted: bool,
    /// Safe for autonomous use without extra approval.
    pub safe_for_autopilot: bool,
    /// Blocker when not ready.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub blocker: Option<String>,
}

/// Capability registry for planner/runtime checks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRegistry {
    /// Registered capabilities.
    #[serde(default)]
    pub capabilities: Vec<CapabilityStatus>,
}

impl CapabilityRegistry {
    /// Registry covering the professional substrate v1 contracts.
    pub fn professional_substrate_v1() -> Self {
        Self {
            capabilities: all_capability_areas()
                .into_iter()
                .map(|area| CapabilityStatus {
                    area,
                    available: true,
                    previewable: true,
                    renderable: matches!(
                        area,
                        CapabilityArea::AssemblyAndTimelineOperations
                            | CapabilityArea::ParameterAnimation
                            | CapabilityArea::MotionGraphicsTemplates
                            | CapabilityArea::DeliveryProfilesAndPreflight
                    ),
                    preflighted: matches!(
                        area,
                        CapabilityArea::DeliveryProfilesAndPreflight
                            | CapabilityArea::AudioFinishing
                            | CapabilityArea::ColorFinishing
                    ),
                    safe_for_autopilot: matches!(
                        area,
                        CapabilityArea::AssetCatalog
                            | CapabilityArea::SourceReviewSelects
                            | CapabilityArea::WorkflowLenses
                            | CapabilityArea::PreAutonomyOrchestrationContract
                    ),
                    blocker: None,
                })
                .collect(),
        }
    }
}

/// Pipeline readiness report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineReadinessReport {
    /// Per-capability readiness.
    #[serde(default)]
    pub stages: Vec<PipelineStageReadiness>,
    /// Human-readable blockers.
    #[serde(default)]
    pub blockers: Vec<String>,
}

impl PipelineReadinessReport {
    /// Build readiness from a capability registry.
    pub fn from_registry(registry: CapabilityRegistry) -> Self {
        let mut blockers = Vec::new();
        let stages = registry
            .capabilities
            .into_iter()
            .map(|capability| {
                let state = if capability.available {
                    ReadinessState::Ready
                } else {
                    if let Some(blocker) = &capability.blocker {
                        blockers.push(blocker.clone());
                    }
                    ReadinessState::Blocked
                };
                PipelineStageReadiness {
                    area: capability.area,
                    state,
                    blocker: capability.blocker,
                }
            })
            .collect();
        Self { stages, blockers }
    }

    /// Readiness for one capability area.
    pub fn stage(&self, area: CapabilityArea) -> Option<ReadinessState> {
        self.stages
            .iter()
            .find(|stage| stage.area == area)
            .map(|stage| stage.state)
    }
}

/// One pipeline readiness row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineStageReadiness {
    /// Capability area.
    pub area: CapabilityArea,
    /// Readiness state.
    pub state: ReadinessState,
    /// Optional blocker.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub blocker: Option<String>,
}

/// Structured professional substrate diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfessionalDiagnostic {
    /// Capability area.
    pub area: CapabilityArea,
    /// Severity.
    pub severity: FindingSeverity,
    /// Diagnostic message.
    pub message: String,
}

impl ProfessionalDiagnostic {
    /// Warning diagnostic.
    pub fn warning(area: CapabilityArea, message: impl Into<String>) -> Self {
        Self {
            area,
            severity: FindingSeverity::Warning,
            message: message.into(),
        }
    }

    /// Error diagnostic.
    pub fn error(area: CapabilityArea, message: impl Into<String>) -> Self {
        Self {
            area,
            severity: FindingSeverity::Error,
            message: message.into(),
        }
    }
}

/// Planner input/output contract for one pipeline pass.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlannerPassContract {
    /// Pass id.
    pub id: String,
    /// Target capability area.
    pub area: CapabilityArea,
    /// Input artifact refs.
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Output artifact refs.
    #[serde(default)]
    pub outputs: Vec<String>,
    /// Detected conflicts.
    #[serde(default)]
    pub conflicts: Vec<PipelineConflict>,
}

/// Cross-stage conflict.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineConflict {
    /// Conflict id.
    pub id: String,
    /// Affected capability areas.
    #[serde(default)]
    pub areas: Vec<CapabilityArea>,
    /// Explanation.
    pub message: String,
}

/// Learning signal from user proposal decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearningSignal {
    /// Proposal id.
    pub proposal_id: String,
    /// Capability area.
    pub area: CapabilityArea,
    /// Review status.
    pub status: ReviewStatus,
    /// Optional reason.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
}

fn all_capability_areas() -> Vec<CapabilityArea> {
    vec![
        CapabilityArea::AssetCatalog,
        CapabilityArea::SourceReviewSelects,
        CapabilityArea::AssemblyAndTimelineOperations,
        CapabilityArea::EditorialIntentAndReview,
        CapabilityArea::ParameterAnimation,
        CapabilityArea::MotionGraphicsTemplates,
        CapabilityArea::CompositionGraph,
        CapabilityArea::TrackingMasksMattes,
        CapabilityArea::ColorFinishing,
        CapabilityArea::AudioFinishing,
        CapabilityArea::DeliveryProfilesAndPreflight,
        CapabilityArea::WorkflowLenses,
        CapabilityArea::PreAutonomyOrchestrationContract,
    ]
}

fn validate_optional_confidence(
    diagnostics: &mut Vec<ProfessionalDiagnostic>,
    area: CapabilityArea,
    confidence: Option<f64>,
    label: String,
) {
    if let Some(confidence) = confidence
        && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
    {
        diagnostics.push(ProfessionalDiagnostic::warning(
            area,
            format!("{label} confidence must be in 0..=1"),
        ));
    }
}

fn default_one() -> f64 {
    1.0
}
