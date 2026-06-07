//! Professional editing substrate contracts.
//!
//! These types are durable, serializable building blocks for the agent-native
//! editing pipeline: source organization, selects, assembly review, animation,
//! motion graphics, compositing, tracking, finishing, delivery, workflow lenses,
//! and pre-autonomy orchestration.

use std::collections::{BTreeMap, HashMap, HashSet};

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
        validate_asset_bins(&self.bins, &bin_ids, &mut diagnostics);
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

fn validate_asset_bins(
    bins: &[AssetBin],
    bin_ids: &HashSet<&str>,
    diagnostics: &mut Vec<ProfessionalDiagnostic>,
) {
    let mut seen = HashSet::new();
    let parent_by_id: HashMap<&str, Option<&str>> = bins
        .iter()
        .map(|bin| (bin.id.as_str(), bin.parent_id.as_deref()))
        .collect();
    for bin in bins {
        if bin.id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::AssetCatalog,
                "asset catalog contains a bin with an empty id",
            ));
            continue;
        }
        if !seen.insert(bin.id.as_str()) {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::AssetCatalog,
                format!("asset catalog contains duplicate bin id {}", bin.id),
            ));
        }
        if let Some(parent_id) = bin.parent_id.as_deref() {
            if parent_id == bin.id {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::AssetCatalog,
                    format!("bin {} cannot be its own parent", bin.id),
                ));
            } else if !bin_ids.contains(parent_id) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::AssetCatalog,
                    format!("bin {} references missing parent {}", bin.id, parent_id),
                ));
            } else if bin_parent_chain_has_cycle(bin.id.as_str(), &parent_by_id) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::AssetCatalog,
                    format!("bin {} is part of a parent cycle", bin.id),
                ));
            }
        }
    }
}

fn bin_parent_chain_has_cycle(bin_id: &str, parent_by_id: &HashMap<&str, Option<&str>>) -> bool {
    let mut seen = HashSet::new();
    let mut current = Some(bin_id);
    while let Some(id) = current {
        if !seen.insert(id) {
            return true;
        }
        current = parent_by_id.get(id).and_then(|parent| *parent);
    }
    false
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
    /// Optional upload date as reported by the source (e.g. yt-dlp
    /// `%(upload_date)s`), normalized to `YYYY-MM-DD`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub upload_date: Option<String>,
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

/// Awidat-owned transcript alignment package derived from one transcript sidecar.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranscriptAlignmentPackage {
    /// Source asset id, usually a project-relative path under `raw/`.
    pub asset_id: String,
    /// Optional source media checksum used to detect stale alignment packages.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_sha256: Option<String>,
    /// Stable word-level alignment records.
    #[serde(default)]
    pub words: Vec<AlignedTranscriptWord>,
    /// Stable phrase-level groupings over word ids.
    #[serde(default)]
    pub phrases: Vec<AlignedTranscriptPhrase>,
    /// Active correction records, separate from raw sidecar text.
    #[serde(default)]
    pub corrections: Vec<TranscriptCorrection>,
    /// Reversible transcript edit history.
    #[serde(default)]
    pub edit_log: Vec<TranscriptEditRecord>,
}

impl TranscriptAlignmentPackage {
    /// Validate local transcript alignment invariants.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.asset_id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::SourceReviewSelects,
                "transcript alignment package has an empty asset_id",
            ));
        }

        let mut word_ids = HashSet::new();
        for word in &self.words {
            if word.id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::SourceReviewSelects,
                    "transcript alignment contains a word with an empty id",
                ));
            }
            if !word_ids.insert(word.id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::SourceReviewSelects,
                    format!(
                        "transcript alignment contains duplicate word id {}",
                        word.id
                    ),
                ));
            }
            if !word.range.is_valid() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::SourceReviewSelects,
                    format!("word {} has an invalid source range", word.id),
                ));
            }
            if word.original_text.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::warning(
                    CapabilityArea::SourceReviewSelects,
                    format!("word {} has empty original text", word.id),
                ));
            }
        }

        let mut phrase_ids = HashSet::new();
        for phrase in &self.phrases {
            if phrase.id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::SourceReviewSelects,
                    "transcript alignment contains a phrase with an empty id",
                ));
            }
            if !phrase_ids.insert(phrase.id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::SourceReviewSelects,
                    format!(
                        "transcript alignment contains duplicate phrase id {}",
                        phrase.id
                    ),
                ));
            }
            if !phrase.range.is_valid() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::SourceReviewSelects,
                    format!("phrase {} has an invalid source range", phrase.id),
                ));
            }
            if phrase.word_ids.is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::SourceReviewSelects,
                    format!("phrase {} contains no word ids", phrase.id),
                ));
            }
            for word_id in &phrase.word_ids {
                if !word_ids.contains(word_id.as_str()) {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::SourceReviewSelects,
                        format!("phrase {} references missing word {}", phrase.id, word_id),
                    ));
                }
            }
        }

        let phrase_id_set: HashSet<&str> = self.phrases.iter().map(|p| p.id.as_str()).collect();
        for correction in &self.corrections {
            if correction.id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::SourceReviewSelects,
                    "transcript correction has an empty id",
                ));
            }
            if correction.corrected_text.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::SourceReviewSelects,
                    format!(
                        "transcript correction {} has empty corrected text",
                        correction.id
                    ),
                ));
            }
            match &correction.target {
                TranscriptCorrectionTarget::Word { word_id } => {
                    if !word_ids.contains(word_id.as_str()) {
                        diagnostics.push(ProfessionalDiagnostic::error(
                            CapabilityArea::SourceReviewSelects,
                            format!(
                                "transcript correction {} references missing word {}",
                                correction.id, word_id
                            ),
                        ));
                    }
                }
                TranscriptCorrectionTarget::Phrase { phrase_id } => {
                    if !phrase_id_set.contains(phrase_id.as_str()) {
                        diagnostics.push(ProfessionalDiagnostic::error(
                            CapabilityArea::SourceReviewSelects,
                            format!(
                                "transcript correction {} references missing phrase {}",
                                correction.id, phrase_id
                            ),
                        ));
                    }
                }
            }
        }

        let correction_ids: HashSet<&str> = self
            .corrections
            .iter()
            .map(|correction| correction.id.as_str())
            .collect();
        for edit in &self.edit_log {
            if edit.id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::SourceReviewSelects,
                    "transcript edit record has an empty id",
                ));
            }
            if !correction_ids.contains(edit.correction_id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::SourceReviewSelects,
                    format!(
                        "transcript edit {} references missing correction {}",
                        edit.id, edit.correction_id
                    ),
                ));
            }
        }

        diagnostics
    }
}

/// One stable word in a transcript alignment package.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AlignedTranscriptWord {
    /// Stable word id.
    pub id: String,
    /// Original transcript text from the producer sidecar.
    pub original_text: String,
    /// Optional corrected display text.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub corrected_text: Option<String>,
    /// Source-time range in seconds.
    pub range: SourceRange,
    /// Occurrence index among normalized words for the asset.
    pub occurrence: u32,
    /// Diarized speaker id or corrected speaker label.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub speaker_id: Option<String>,
    /// Producer confidence when available.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confidence: Option<f64>,
}

/// One stable phrase in a transcript alignment package.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AlignedTranscriptPhrase {
    /// Stable phrase id.
    pub id: String,
    /// Source-time range spanning the phrase.
    pub range: SourceRange,
    /// Ordered word ids in this phrase.
    #[serde(default)]
    pub word_ids: Vec<String>,
    /// Normalized phrase text at generation time.
    pub text: String,
    /// Speaker id when all phrase words share one speaker.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub speaker_id: Option<String>,
}

/// A transcript correction target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptCorrectionTarget {
    /// Correct a single word.
    Word {
        /// Target word id.
        word_id: String,
    },
    /// Correct a phrase.
    Phrase {
        /// Target phrase id.
        phrase_id: String,
    },
}

impl Default for TranscriptCorrectionTarget {
    fn default() -> Self {
        Self::Word {
            word_id: String::new(),
        }
    }
}

/// One correction applied over the immutable raw transcript alignment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptCorrection {
    /// Stable correction id.
    pub id: String,
    /// Target word or phrase.
    pub target: TranscriptCorrectionTarget,
    /// Replacement text for display/editing.
    pub corrected_text: String,
    /// Original text captured before the correction was applied.
    pub original_text: String,
    /// Source of the correction, e.g. `user`, `agent`, or `import`.
    pub source: String,
    /// RFC3339 timestamp or equivalent producer timestamp.
    pub corrected_at: String,
}

/// Reversible transcript edit lifecycle record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptEditRecord {
    /// Stable edit id.
    pub id: String,
    /// Correction id this edit applies or reverts.
    pub correction_id: String,
    /// Edit action.
    pub action: TranscriptEditAction,
    /// Whether the edit has been reverted.
    #[serde(default)]
    pub reverted: bool,
    /// RFC3339 timestamp or equivalent producer timestamp.
    pub recorded_at: String,
}

/// Transcript edit operation kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptEditAction {
    /// Apply a correction.
    #[default]
    ApplyCorrection,
    /// Revert a correction.
    RevertCorrection,
}

/// Durable per-project media intelligence state derived from current artifacts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaIntelligencePackage {
    /// Per-asset intelligence state.
    #[serde(default)]
    pub assets: Vec<MediaIntelligenceAsset>,
}

impl MediaIntelligencePackage {
    /// Validate media intelligence package invariants.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut asset_ids = HashSet::new();
        for asset in &self.assets {
            if asset.asset_id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    "media intelligence asset has an empty asset_id",
                ));
            }
            if !asset_ids.insert(asset.asset_id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "media intelligence contains duplicate asset {}",
                        asset.asset_id
                    ),
                ));
            }
            diagnostics.extend(asset.validate());
        }
        diagnostics
    }
}

/// Per-asset progressive intelligence state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaIntelligenceAsset {
    /// Source asset id.
    pub asset_id: String,
    /// Independent processing layers for this asset.
    #[serde(default)]
    pub layers: Vec<MediaIntelligenceLayer>,
    /// Aggregate readiness for UI/agent routing.
    #[serde(default)]
    pub aggregate_state: MediaIntelligenceAggregateState,
    /// Narrow next actions an agent can take.
    #[serde(default)]
    pub recommended_actions: Vec<String>,
}

impl MediaIntelligenceAsset {
    fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut kinds = HashSet::new();
        for layer in &self.layers {
            if !kinds.insert(layer.kind) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "media intelligence asset {} has duplicate layer {:?}",
                        self.asset_id, layer.kind
                    ),
                ));
            }
            if layer.status == MediaIntelligenceLayerStatus::Blocked
                && layer
                    .blocking_reason
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
            {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "media intelligence layer {:?} for {} is blocked without a reason",
                        layer.kind, self.asset_id
                    ),
                ));
            }
            if layer.status == MediaIntelligenceLayerStatus::Ready && layer.artifact_refs.is_empty()
            {
                diagnostics.push(ProfessionalDiagnostic::warning(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "media intelligence layer {:?} for {} is ready without artifacts",
                        layer.kind, self.asset_id
                    ),
                ));
            }
        }
        diagnostics
    }
}

/// One independently generated media intelligence layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaIntelligenceLayer {
    /// Layer kind.
    pub kind: MediaIntelligenceLayerKind,
    /// Layer status.
    #[serde(default)]
    pub status: MediaIntelligenceLayerStatus,
    /// Artifact references, usually project-relative paths.
    #[serde(default)]
    pub artifact_refs: Vec<String>,
    /// Producing indexer/tool/cache.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub producer: Option<String>,
    /// Whether the artifact appears stale relative to source.
    #[serde(default)]
    pub stale: bool,
    /// Human-readable blocking reason for blocked layers.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub blocking_reason: Option<String>,
    /// RFC3339 timestamp or filesystem-derived timestamp.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub updated_at: Option<String>,
}

/// Independent intelligence layers generated progressively for one asset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaIntelligenceLayerKind {
    /// Original source media exists and is readable.
    #[default]
    Source,
    /// Editing/playback proxy.
    Proxy,
    /// Waveform or audio overview cache.
    Waveform,
    /// Word/segment transcript.
    Transcript,
    /// Speaker labels/diarization.
    Speakers,
    /// Shot or scene boundaries.
    Scenes,
    /// Semantic topics.
    Topics,
    /// Editorial moments.
    Moments,
    /// Ranked short-form clip candidates.
    ClipCandidates,
    /// B-roll opportunities/candidates.
    Broll,
}

/// Status for one media intelligence layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaIntelligenceLayerStatus {
    /// Artifact is available.
    Ready,
    /// Artifact is not available yet.
    #[default]
    Pending,
    /// Work is currently running.
    Processing,
    /// Work is blocked by missing input or failure.
    Blocked,
    /// Layer is not applicable for this asset.
    Unavailable,
}

/// Aggregate state for an asset across all media intelligence layers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaIntelligenceAggregateState {
    /// All required layers are ready.
    Ready,
    /// Some layers are ready and some are pending.
    #[default]
    Partial,
    /// At least one layer is actively processing.
    Processing,
    /// Source exists but required downstream work is blocked.
    Blocked,
    /// Source media is missing.
    Offline,
}

/// Consolidated understanding sidecar derived from transcript, scene, topic, audio, and moment evidence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UnderstandingPackage {
    /// Per-asset fused understanding records.
    #[serde(default)]
    pub assets: Vec<UnderstandingAsset>,
}

impl UnderstandingPackage {
    /// Validate fused understanding invariants.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut asset_ids = HashSet::new();
        for asset in &self.assets {
            if asset.asset_id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    "understanding asset has an empty asset_id",
                ));
            }
            if !asset_ids.insert(asset.asset_id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!("understanding contains duplicate asset {}", asset.asset_id),
                ));
            }
            diagnostics.extend(asset.validate());
        }
        diagnostics
    }
}

/// Fused understanding for one source asset.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UnderstandingAsset {
    /// Source asset id.
    pub asset_id: String,
    /// Stable fused scene records.
    #[serde(default)]
    pub scenes: Vec<FusedScene>,
    /// Stable fused moment records.
    #[serde(default)]
    pub moments: Vec<FusedMoment>,
    /// Evidence channels that were expected but unavailable.
    #[serde(default)]
    pub missing_evidence: Vec<UnderstandingEvidenceKind>,
}

impl UnderstandingAsset {
    fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut scene_ids = HashSet::new();
        for scene in &self.scenes {
            if scene.id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "understanding asset {} has a scene with an empty id",
                        self.asset_id
                    ),
                ));
            }
            if !scene_ids.insert(scene.id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "understanding asset {} has duplicate scene {}",
                        self.asset_id, scene.id
                    ),
                ));
            }
            if !scene.range.is_valid() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!("understanding scene {} has an invalid range", scene.id),
                ));
            }
        }

        let mut moment_ids = HashSet::new();
        for moment in &self.moments {
            if moment.id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "understanding asset {} has a moment with an empty id",
                        self.asset_id
                    ),
                ));
            }
            if !moment_ids.insert(moment.id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "understanding asset {} has duplicate moment {}",
                        self.asset_id, moment.id
                    ),
                ));
            }
            if !moment.range.is_valid() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!("understanding moment {} has an invalid range", moment.id),
                ));
            }
            if !(0.0..=1.0).contains(&moment.confidence) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "understanding moment {} confidence is outside 0..1",
                        moment.id
                    ),
                ));
            }
            if moment.evidence.is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!("understanding moment {} has no evidence", moment.id),
                ));
            }
            if let Some(scene_id) = moment.scene_id.as_deref()
                && !scene_ids.contains(scene_id)
            {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "understanding moment {} references missing scene {}",
                        moment.id, scene_id
                    ),
                ));
            }
        }
        diagnostics
    }
}

/// Fused scene range from visual scene detection and nearby transcript evidence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FusedScene {
    /// Stable scene id.
    pub id: String,
    /// Source-time range.
    pub range: SourceRange,
    /// Optional coarse visual or semantic label.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub label: Option<String>,
    /// Evidence references used to create this scene.
    #[serde(default)]
    pub evidence: Vec<UnderstandingEvidenceRef>,
}

/// Fused editorial moment with cross-modal evidence.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FusedMoment {
    /// Stable fused moment id.
    pub id: String,
    /// Source asset id.
    pub asset_id: String,
    /// Source-time range.
    pub range: SourceRange,
    /// Moment type, e.g. hook, story, explanation, emotional_peak.
    pub category: String,
    /// Human-readable label.
    pub label: String,
    /// Optional scene this moment falls inside.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub scene_id: Option<String>,
    /// Optional speaker id.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub speaker_id: Option<String>,
    /// Transcript excerpt used for review.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub transcript_excerpt: Option<String>,
    /// Topic labels intersecting this moment.
    #[serde(default)]
    pub topics: Vec<String>,
    /// Energy label or scalar summary from audio evidence.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub energy: Option<String>,
    /// Confidence in the fused understanding record.
    pub confidence: f64,
    /// Evidence references used to create this moment.
    #[serde(default)]
    pub evidence: Vec<UnderstandingEvidenceRef>,
    /// Human-readable review notes.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Evidence channel used by understanding fusion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnderstandingEvidenceKind {
    /// Transcript or word/segment evidence.
    #[default]
    Transcript,
    /// Scene-detection evidence.
    Scene,
    /// Speaker diarization evidence.
    Speaker,
    /// Topic segmentation evidence.
    Topic,
    /// Audio energy / silence evidence.
    Audio,
    /// Editorial moment evidence.
    EditorialMoment,
}

/// Reference to one source evidence record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnderstandingEvidenceRef {
    /// Evidence kind.
    pub kind: UnderstandingEvidenceKind,
    /// Producing indexer/tool name.
    pub producer: String,
    /// Stable sidecar-relative or semantic reference id.
    pub ref_id: String,
}

/// Stored short-form clip candidate package.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipCandidatePackage {
    /// Per-asset clip candidates.
    #[serde(default)]
    pub assets: Vec<ClipCandidateAsset>,
}

impl ClipCandidatePackage {
    /// Validate clip candidate package invariants.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut asset_ids = HashSet::new();
        for asset in &self.assets {
            if asset.asset_id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    "clip candidate asset has an empty asset_id",
                ));
            }
            if !asset_ids.insert(asset.asset_id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!("clip candidates contain duplicate asset {}", asset.asset_id),
                ));
            }
            diagnostics.extend(asset.validate());
        }
        diagnostics
    }
}

/// Clip candidates for one asset.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipCandidateAsset {
    /// Source asset id.
    pub asset_id: String,
    /// Ranked candidates.
    #[serde(default)]
    pub candidates: Vec<ClipCandidate>,
}

impl ClipCandidateAsset {
    fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut ids = HashSet::new();
        for candidate in &self.candidates {
            if candidate.id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "clip candidate asset {} has a candidate with an empty id",
                        self.asset_id
                    ),
                ));
            }
            if !ids.insert(candidate.id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "clip candidate asset {} has duplicate candidate {}",
                        self.asset_id, candidate.id
                    ),
                ));
            }
            if !candidate.range.is_valid() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!("clip candidate {} has an invalid range", candidate.id),
                ));
            }
            if !(0.0..=1.0).contains(&candidate.score) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!("clip candidate {} score is outside 0..1", candidate.id),
                ));
            }
            if candidate.evidence_ids.is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!("clip candidate {} has no evidence ids", candidate.id),
                ));
            }
            for component in &candidate.score_breakdown {
                if component.name.trim().is_empty() {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::PreAutonomyOrchestrationContract,
                        format!(
                            "clip candidate {} has an unnamed score component",
                            candidate.id
                        ),
                    ));
                }
                if !(0.0..=1.0).contains(&component.value) {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::PreAutonomyOrchestrationContract,
                        format!(
                            "clip candidate {} component {} is outside 0..1",
                            candidate.id, component.name
                        ),
                    ));
                }
            }
        }
        diagnostics
    }
}

/// One reviewable short-form candidate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipCandidate {
    /// Stable candidate id.
    pub id: String,
    /// Source asset id.
    pub asset_id: String,
    /// Source-time range.
    pub range: SourceRange,
    /// Candidate rank within asset.
    pub rank: u32,
    /// Overall score from 0..1.
    pub score: f64,
    /// Short explanation for review.
    pub explanation: String,
    /// Score components.
    #[serde(default)]
    pub score_breakdown: Vec<ClipCandidateScoreComponent>,
    /// Evidence ids, usually fused moment ids.
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    /// One-click assembly metadata.
    pub assembly: ClipCandidateAssembly,
}

/// One score component for a clip candidate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipCandidateScoreComponent {
    /// Component name.
    pub name: String,
    /// Component value from 0..1.
    pub value: f64,
    /// Human-readable reason for the value.
    pub reason: String,
}

/// Metadata required to assemble a reviewed short-form candidate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipCandidateAssembly {
    /// Source asset id.
    pub asset_id: String,
    /// Source range to assemble.
    pub source_range: SourceRange,
    /// Suggested aspect ratio, e.g. 9:16 or 1:1.
    pub aspect_ratio: String,
    /// Suggested caption preset/style.
    pub caption_style: String,
    /// Suggested hook text.
    pub hook_text: String,
    /// Required setup fused moment ids.
    #[serde(default)]
    pub required_setup_ids: Vec<String>,
}

/// Stored B-roll recommendations derived from understanding records.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BrollRecommendationPackage {
    /// Per-asset recommendation sets.
    #[serde(default)]
    pub assets: Vec<BrollRecommendationAsset>,
}

impl BrollRecommendationPackage {
    /// Validate B-roll recommendation package invariants.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut asset_ids = HashSet::new();
        for asset in &self.assets {
            if asset.asset_id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    "B-roll recommendation asset has an empty asset_id",
                ));
            }
            if !asset_ids.insert(asset.asset_id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "B-roll recommendations contain duplicate asset {}",
                        asset.asset_id
                    ),
                ));
            }
            diagnostics.extend(asset.validate());
        }
        diagnostics
    }
}

/// B-roll recommendations for one source asset.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BrollRecommendationAsset {
    /// Source asset id.
    pub asset_id: String,
    /// Ranked recommendations.
    #[serde(default)]
    pub recommendations: Vec<BrollRecommendation>,
}

impl BrollRecommendationAsset {
    fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        let mut ids = HashSet::new();
        for recommendation in &self.recommendations {
            if recommendation.id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "B-roll recommendation asset {} has a recommendation with an empty id",
                        self.asset_id
                    ),
                ));
            }
            if !ids.insert(recommendation.id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "B-roll recommendation asset {} has duplicate recommendation {}",
                        self.asset_id, recommendation.id
                    ),
                ));
            }
            if !recommendation.source_range.is_valid() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "B-roll recommendation {} has an invalid source range",
                        recommendation.id
                    ),
                ));
            }
            if !(0.0..=1.0).contains(&recommendation.score) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "B-roll recommendation {} score is outside 0..1",
                        recommendation.id
                    ),
                ));
            }
            if recommendation.evidence_ids.is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "B-roll recommendation {} has no evidence ids",
                        recommendation.id
                    ),
                ));
            }
            if !recommendation.insertion.range.is_valid()
                || recommendation.insertion.rationale.trim().is_empty()
                || recommendation.insertion.placement.trim().is_empty()
            {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::PreAutonomyOrchestrationContract,
                    format!(
                        "B-roll recommendation {} has an invalid insertion plan",
                        recommendation.id
                    ),
                ));
            }
            for component in &recommendation.score_breakdown {
                if component.name.trim().is_empty() {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::PreAutonomyOrchestrationContract,
                        format!(
                            "B-roll recommendation {} has an unnamed score component",
                            recommendation.id
                        ),
                    ));
                }
                if !(0.0..=1.0).contains(&component.value) {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::PreAutonomyOrchestrationContract,
                        format!(
                            "B-roll recommendation {} component {} is outside 0..1",
                            recommendation.id, component.name
                        ),
                    ));
                }
            }
        }
        diagnostics
    }
}

/// One scored B-roll recommendation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BrollRecommendation {
    /// Stable recommendation id.
    pub id: String,
    /// Source asset id.
    pub asset_id: String,
    /// Fused moment id this recommendation supports.
    pub moment_id: String,
    /// Source range being covered or illustrated.
    pub source_range: SourceRange,
    /// Visual opportunity category.
    pub category: BrollRecommendationCategory,
    /// Asset/retrieval/generation strategy.
    pub strategy: BrollAssetStrategy,
    /// Overall score from 0..1.
    pub score: f64,
    /// Human-readable recommendation rationale.
    pub rationale: String,
    /// Score components.
    #[serde(default)]
    pub score_breakdown: Vec<BrollRecommendationScoreComponent>,
    /// Evidence ids, usually fused moment ids and sidecar refs.
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    /// Suggested insertion plan.
    pub insertion: BrollInsertionPlan,
}

/// Visual opportunity category for B-roll.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrollRecommendationCategory {
    /// Explain a concept or process.
    #[default]
    Explanation,
    /// Support a story or anecdote.
    Storytelling,
    /// Visualize a statistic or numeric claim.
    Statistic,
    /// Clarify an analogy.
    Analogy,
    /// Refer to a historical event or archival context.
    HistoricalReference,
    /// Mention a company, person, or brand.
    EntityMention,
    /// Show a product, app, workflow, or UI.
    ProductMention,
    /// Support an emotional story beat.
    EmotionalMoment,
    /// Explain a technical concept.
    TechnicalConcept,
}

/// Strategy for sourcing the B-roll asset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrollAssetStrategy {
    /// Use existing project footage.
    #[default]
    ProjectFootage,
    /// Search stock footage/stills.
    Stock,
    /// Generate a new image/video asset.
    GeneratedVisual,
    /// Use motion graphics/infographic treatment.
    MotionGraphic,
    /// Use archival/reference media.
    ArchivalReference,
}

/// Score component for a B-roll recommendation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BrollRecommendationScoreComponent {
    /// Component name.
    pub name: String,
    /// Component value from 0..1.
    pub value: f64,
    /// Human-readable reason.
    pub reason: String,
}

/// Suggested B-roll insertion plan.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BrollInsertionPlan {
    /// Fused moment or semantic anchor.
    pub anchor_id: String,
    /// Source/timeline-adjacent range to cover.
    pub range: SourceRange,
    /// Suggested duration.
    pub duration_s: f64,
    /// Placement style, e.g. cover_cutaway, pip, lower_third_graphic.
    pub placement: String,
    /// Human-readable insertion rationale.
    pub rationale: String,
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

/// Agent-authored motion package that reviews coherent motion changes before
/// applying them as explicit project records.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MotionPackage {
    /// Stable package id.
    pub id: String,
    /// Human-readable intent.
    pub intent: String,
    /// Affected clip ids.
    #[serde(default)]
    pub affected_clips: Vec<String>,
    /// Affected clip ranges.
    #[serde(default)]
    pub affected_ranges: Vec<MotionPackageRange>,
    /// Template fills used to generate the package.
    #[serde(default)]
    pub template_fills: Vec<MotionTemplateFill>,
    /// Generated explicit parameter animations.
    #[serde(default)]
    pub generated_animations: Vec<ParameterAnimation>,
    /// Rationale for review.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rationale: Option<String>,
    /// Known limitations.
    #[serde(default)]
    pub limitations: Vec<String>,
    /// Review lifecycle.
    #[serde(default)]
    pub status: ReviewStatus,
}

/// One affected clip range in a motion package.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MotionPackageRange {
    /// Clip id.
    pub clip_id: String,
    /// Start time in seconds.
    pub start_s: f64,
    /// End time in seconds.
    pub end_s: f64,
}

/// Serializable template fill inside a motion package.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MotionTemplateFill {
    /// Template id.
    pub template_id: String,
    /// Slot values keyed by slot id.
    #[serde(default)]
    pub slots: BTreeMap<String, serde_json::Value>,
}

/// Agent-authored procedural motion scene.
///
/// A MotionScene is a durable planning object for native animated explainers,
/// callouts, title systems, and other generated motion layers. It records the
/// scene contract without claiming a renderer can lower every layer kind yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MotionScene {
    /// Stable scene id.
    pub id: String,
    /// Absolute timeline second at which the scene begins. Layer `from_s`
    /// values are relative to this offset, so a scene anchored to a moment
    /// later in the program renders over that moment instead of at program
    /// start. Defaults to 0 (program start) for back-compat.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub start_s: f64,
    /// Scene duration in seconds.
    pub duration_s: f64,
    /// Intended scene frame rate.
    pub fps: f64,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// Ordered motion layers.
    #[serde(default)]
    pub layers: Vec<MotionSceneLayer>,
    /// Optional review rationale.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rationale: Option<String>,
}

impl MotionScene {
    /// Validate scene timing and layer bounds.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::MotionGraphicsTemplates,
                "motion scene has an empty id",
            ));
        }
        if !self.start_s.is_finite() || self.start_s < 0.0 {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::MotionGraphicsTemplates,
                format!("motion scene {} start_s must be non-negative", self.id),
            ));
        }
        if !self.duration_s.is_finite() || self.duration_s <= 0.0 {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::MotionGraphicsTemplates,
                format!("motion scene {} duration_s must be positive", self.id),
            ));
        }
        if !self.fps.is_finite() || self.fps <= 0.0 {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::MotionGraphicsTemplates,
                format!("motion scene {} fps must be positive", self.id),
            ));
        }
        if self.width == 0 || self.height == 0 {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::MotionGraphicsTemplates,
                format!(
                    "motion scene {} canvas dimensions must be positive",
                    self.id
                ),
            ));
        }
        for layer in &self.layers {
            if layer.id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::MotionGraphicsTemplates,
                    format!("motion scene {} has a layer with an empty id", self.id),
                ));
            }
            if !layer.from_s.is_finite() || layer.from_s < 0.0 {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::MotionGraphicsTemplates,
                    format!(
                        "motion scene {} layer {} from_s must be non-negative",
                        self.id, layer.id
                    ),
                ));
            }
            if !layer.duration_s.is_finite() || layer.duration_s <= 0.0 {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::MotionGraphicsTemplates,
                    format!(
                        "motion scene {} layer {} duration_s must be positive",
                        self.id, layer.id
                    ),
                ));
            }
            if layer.from_s + layer.duration_s > self.duration_s {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::MotionGraphicsTemplates,
                    format!(
                        "motion scene {} layer {} exceeds scene duration",
                        self.id, layer.id
                    ),
                ));
            }
        }
        diagnostics
    }
}

// serde's `skip_serializing_if` requires `fn(&T) -> bool`, so the reference
// is mandatory here even though clippy would prefer `f64` by value.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}

/// One layer inside a procedural motion scene.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MotionSceneLayer {
    /// Stable layer id.
    pub id: String,
    /// Layer kind.
    #[serde(default)]
    pub kind: MotionSceneLayerKind,
    /// Layer start time in scene seconds.
    pub from_s: f64,
    /// Layer duration in seconds.
    pub duration_s: f64,
    /// Draw order; larger values appear above smaller values.
    #[serde(default)]
    pub z_index: i32,
    /// Layer-specific parameters such as text, color, position, or asset id.
    #[serde(default)]
    pub params: BTreeMap<String, serde_json::Value>,
}

impl MotionSceneLayer {
    /// Parse layer-local MotionScene animations from `params.animations`.
    pub fn motion_animations(&self) -> Vec<MotionSceneLayerAnimation> {
        self.params
            .get("animations")
            .and_then(serde_json::Value::as_array)
            .map(|animations| {
                animations
                    .iter()
                    .filter_map(MotionSceneLayerAnimation::from_value)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Shared normalized MotionScene transform parsed from layer parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotionSceneTransform {
    /// Left offset as a fraction of canvas width.
    pub x: f64,
    /// Top offset as a fraction of canvas height.
    pub y: f64,
    /// Layer width as a fraction of canvas width.
    pub width: f64,
    /// Layer height as a fraction of canvas height.
    pub height: f64,
    /// Layer opacity in 0..=1.
    pub opacity: f64,
    /// Asset fitting behavior inside the transform box.
    pub fit: MotionSceneTransformFit,
    /// Multiplier applied around the layer anchor.
    pub scale: f64,
    /// Anchor x within the layer box, in 0..=1.
    pub anchor_x: f64,
    /// Anchor y within the layer box, in 0..=1.
    pub anchor_y: f64,
    /// Clockwise rotation in degrees.
    pub rotation_deg: f64,
}

impl Default for MotionSceneTransform {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            opacity: 1.0,
            fit: MotionSceneTransformFit::Cover,
            scale: 1.0,
            anchor_x: 0.5,
            anchor_y: 0.5,
            rotation_deg: 0.0,
        }
    }
}

impl MotionSceneTransform {
    /// Parse common transform fields from layer params.
    pub fn from_layer_params(params: &BTreeMap<String, serde_json::Value>) -> Self {
        let mut transform = Self::default();
        transform.x = finite_param(params, "x").unwrap_or(transform.x);
        transform.y = finite_param(params, "y").unwrap_or(transform.y);
        transform.width = positive_param(params, "width").unwrap_or(transform.width);
        transform.height = positive_param(params, "height").unwrap_or(transform.height);
        transform.opacity = finite_param(params, "opacity")
            .unwrap_or(transform.opacity)
            .clamp(0.0, 1.0);
        transform.scale = positive_param(params, "scale").unwrap_or(transform.scale);
        transform.anchor_x = finite_param(params, "anchor_x")
            .unwrap_or(transform.anchor_x)
            .clamp(0.0, 1.0);
        transform.anchor_y = finite_param(params, "anchor_y")
            .unwrap_or(transform.anchor_y)
            .clamp(0.0, 1.0);
        transform.rotation_deg = finite_param(params, "rotation_deg")
            .or_else(|| finite_param(params, "rotation"))
            .unwrap_or(transform.rotation_deg);
        transform.fit = params
            .get("fit")
            .and_then(serde_json::Value::as_str)
            .and_then(MotionSceneTransformFit::parse)
            .unwrap_or(transform.fit);
        transform
    }
}

/// Layer-local MotionScene animation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotionSceneLayerAnimation {
    /// Parameter path, for example `overlay.x`.
    pub parameter: String,
    /// Layer-local scalar keyframes.
    #[serde(default)]
    pub keyframes: Vec<Keyframe>,
    /// Behavior before the first keyframe.
    #[serde(default)]
    pub pre_extrapolation: ExtrapolationMode,
    /// Behavior after the last keyframe.
    #[serde(default)]
    pub post_extrapolation: ExtrapolationMode,
    /// Optional motion path for `overlay.position`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub motion_path: Option<MotionPath>,
}

impl MotionSceneLayerAnimation {
    fn from_value(value: &serde_json::Value) -> Option<Self> {
        let animation: Self = serde_json::from_value(value.clone()).ok()?;
        if animation.parameter.trim().is_empty() || animation.keyframes.is_empty() {
            return None;
        }
        Some(animation)
    }
}

/// Fit mode for image-like MotionScene layers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionSceneTransformFit {
    /// Fill the transform box while preserving aspect ratio.
    #[default]
    Cover,
    /// Fit entirely inside the transform box while preserving aspect ratio.
    Contain,
    /// Stretch to exactly match the transform box.
    Stretch,
}

impl MotionSceneTransformFit {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cover" => Some(Self::Cover),
            "contain" => Some(Self::Contain),
            "stretch" | "fill" => Some(Self::Stretch),
            _ => None,
        }
    }

    /// Stable wire value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Contain => "contain",
            Self::Stretch => "stretch",
        }
    }
}

fn finite_param(params: &BTreeMap<String, serde_json::Value>, key: &str) -> Option<f64> {
    params
        .get(key)
        .and_then(serde_json::Value::as_f64)
        .filter(|value| value.is_finite())
}

fn positive_param(params: &BTreeMap<String, serde_json::Value>, key: &str) -> Option<f64> {
    finite_param(params, key).filter(|value| *value > 0.0)
}

/// Motion scene layer kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionSceneLayerKind {
    /// Text layer.
    #[default]
    Text,
    /// Shape/vector layer.
    Shape,
    /// Still image layer.
    Image,
    /// Video/media layer.
    Video,
    /// Solid color background layer.
    Solid,
    /// Grouping layer.
    Group,
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
    /// Behavior before the first keyframe.
    #[serde(default, skip_serializing_if = "ExtrapolationMode::is_hold")]
    pub pre_extrapolation: ExtrapolationMode,
    /// Behavior after the last keyframe.
    #[serde(default, skip_serializing_if = "ExtrapolationMode::is_hold")]
    pub post_extrapolation: ExtrapolationMode,
    /// Optional 2D motion path for position parameters.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub motion_path: Option<MotionPath>,
    /// Store the animation as reviewable metadata without claiming
    /// preview/render support.
    #[serde(default, skip_serializing_if = "is_false")]
    pub metadata_only: bool,
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
        let parameter = match &self.target {
            AnimationTarget::ClipParameter { parameter, .. } => Some(parameter.as_str()),
            _ => None,
        };
        let mut previous_time = None;
        for keyframe in &self.keyframes {
            if !keyframe.time_s.is_finite() || !keyframe.value.is_finite() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::ParameterAnimation,
                    format!("parameter animation {} has a non-finite keyframe", self.id),
                ));
            }
            if let Some(parameter) = parameter {
                let value_parameter = match canonical_runtime_clip_parameter(parameter) {
                    Some(canonical) => canonical,
                    None => parameter,
                };
                validate_parameter_animation_value(
                    &mut diagnostics,
                    &self.id,
                    value_parameter,
                    keyframe,
                );
            }
            if let Some(handles) = keyframe.bezier {
                validate_bezier_handles(&mut diagnostics, &self.id, handles);
            }
            if let Some(spring) = keyframe.spring {
                validate_spring_parameters(&mut diagnostics, &self.id, spring);
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
        validate_tangent_modes(&mut diagnostics, &self.id, &self.keyframes);
        if let Some(path) = &self.motion_path {
            validate_motion_path(&mut diagnostics, &self.id, path);
        }
        diagnostics
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// Returns true when Awidat currently previews and renders this
/// animation target as an executable runtime animation.
pub fn is_runtime_parameter_animation_target(target: &AnimationTarget) -> bool {
    match target {
        AnimationTarget::ClipParameter { parameter, .. } => is_runtime_clip_parameter(parameter),
        AnimationTarget::TrackParameter { parameter, .. } => is_runtime_track_parameter(parameter),
        AnimationTarget::CompositionNodeParameter { .. } | AnimationTarget::Unset => false,
    }
}

/// Returns true when the named clip parameter is executable by the
/// current preview/render runtime.
pub fn is_runtime_clip_parameter(parameter: &str) -> bool {
    canonical_runtime_clip_parameter(parameter).is_some()
}

/// Returns true when the named track parameter is executable by the
/// current preview/render runtime.
pub fn is_runtime_track_parameter(parameter: &str) -> bool {
    RUNTIME_TRACK_PARAMETERS.contains(&parameter)
}

/// Clip parameter paths executable by the current preview/render runtime.
pub const RUNTIME_CLIP_PARAMETERS: &[&str] = &[
    "title.opacity",
    "title.x",
    "title.y",
    "title.position",
    "title.font_size",
    "overlay.opacity",
    "overlay.x",
    "overlay.y",
    "overlay.position",
    "overlay.scale",
    "overlay.rotation_deg",
    "overlay.blur",
    "awidat.blur.radius_px",
    "awidat.shake.intensity_px",
    "awidat.shake.frequency_hz",
    "awidat.warp.k1",
    "awidat.warp.k2",
    "awidat.warp.center_x",
    "awidat.warp.center_y",
];

/// Runtime effect parameter exposed through the generic
/// `effects.<effect_id>.params.<param>` namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeEffectParameter {
    /// Stable effect id.
    pub effect_id: &'static str,
    /// Parameter name inside the effect.
    pub param: &'static str,
    /// Existing canonical runtime parameter path.
    pub canonical: &'static str,
}

/// Effect parameters executable by the current preview/render runtime.
pub const RUNTIME_EFFECT_PARAMETERS: &[RuntimeEffectParameter] = &[
    RuntimeEffectParameter {
        effect_id: "awidat.blur",
        param: "radius_px",
        canonical: "awidat.blur.radius_px",
    },
    RuntimeEffectParameter {
        effect_id: "awidat.shake",
        param: "intensity_px",
        canonical: "awidat.shake.intensity_px",
    },
    RuntimeEffectParameter {
        effect_id: "awidat.shake",
        param: "frequency_hz",
        canonical: "awidat.shake.frequency_hz",
    },
    RuntimeEffectParameter {
        effect_id: "awidat.warp",
        param: "k1",
        canonical: "awidat.warp.k1",
    },
    RuntimeEffectParameter {
        effect_id: "awidat.warp",
        param: "k2",
        canonical: "awidat.warp.k2",
    },
    RuntimeEffectParameter {
        effect_id: "awidat.warp",
        param: "center_x",
        canonical: "awidat.warp.center_x",
    },
    RuntimeEffectParameter {
        effect_id: "awidat.warp",
        param: "center_y",
        canonical: "awidat.warp.center_y",
    },
];

/// Return the canonical runtime clip parameter for a direct path or
/// generic effect namespace path.
pub fn canonical_runtime_clip_parameter(parameter: &str) -> Option<&'static str> {
    if let Some(runtime) = RUNTIME_CLIP_PARAMETERS
        .iter()
        .copied()
        .find(|runtime| *runtime == parameter)
    {
        return Some(runtime);
    }

    let (effect_id, param) = parse_effect_parameter_alias(parameter)?;
    RUNTIME_EFFECT_PARAMETERS
        .iter()
        .find(|runtime| runtime.effect_id == effect_id && runtime.param == param)
        .map(|runtime| runtime.canonical)
}

fn parse_effect_parameter_alias(parameter: &str) -> Option<(&str, &str)> {
    let rest = parameter.strip_prefix("effects.")?;
    let (effect_id, param) = rest.split_once(".params.")?;
    if effect_id.is_empty() || param.is_empty() {
        return None;
    }
    Some((effect_id, param))
}

/// Track parameter paths executable by the current preview/render runtime.
pub const RUNTIME_TRACK_PARAMETERS: &[&str] = &["volume", "volume_db"];

fn validate_bezier_handles(
    diagnostics: &mut Vec<ProfessionalDiagnostic>,
    animation_id: &str,
    handles: BezierHandles,
) {
    if !handles.out_x.is_finite()
        || !handles.out_y.is_finite()
        || !handles.in_x.is_finite()
        || !handles.in_y.is_finite()
    {
        diagnostics.push(ProfessionalDiagnostic::error(
            CapabilityArea::ParameterAnimation,
            format!("parameter animation {animation_id} has non-finite Bezier handles"),
        ));
    }
    if !(0.0..=1.0).contains(&handles.out_x) || !(0.0..=1.0).contains(&handles.in_x) {
        diagnostics.push(ProfessionalDiagnostic::error(
            CapabilityArea::ParameterAnimation,
            format!("parameter animation {animation_id} Bezier handle x values must be in [0, 1]"),
        ));
    }
}

fn validate_spring_parameters(
    diagnostics: &mut Vec<ProfessionalDiagnostic>,
    animation_id: &str,
    spring: SpringParameters,
) {
    if !spring.mass.is_finite()
        || !spring.stiffness.is_finite()
        || !spring.damping.is_finite()
        || spring.mass <= 0.0
        || spring.stiffness <= 0.0
        || spring.damping < 0.0
    {
        diagnostics.push(ProfessionalDiagnostic::error(
            CapabilityArea::ParameterAnimation,
            format!(
                "parameter animation {animation_id} spring requires positive finite mass/stiffness and non-negative finite damping"
            ),
        ));
    }
}

fn validate_tangent_modes(
    diagnostics: &mut Vec<ProfessionalDiagnostic>,
    animation_id: &str,
    keyframes: &[Keyframe],
) {
    for index in 1..keyframes.len().saturating_sub(1) {
        let keyframe = &keyframes[index];
        if keyframe.tangent_mode != TangentMode::Aligned {
            continue;
        }
        let Some(previous_handles) = keyframes[index - 1].bezier else {
            continue;
        };
        let Some(next_handles) = keyframe.bezier else {
            continue;
        };
        if !aligned_tangents(
            keyframes[index - 1].time_s,
            keyframes[index - 1].value,
            previous_handles,
            keyframe.time_s,
            keyframe.value,
            keyframes[index + 1].time_s,
            keyframes[index + 1].value,
            next_handles,
        ) {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                format!(
                    "parameter animation {animation_id} keyframe at {}s has tangent_mode=aligned but adjacent Bezier handles are not collinear",
                    keyframe.time_s
                ),
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn aligned_tangents(
    previous_time: f64,
    previous_value: f64,
    previous_handles: BezierHandles,
    keyframe_time: f64,
    keyframe_value: f64,
    next_time: f64,
    next_value: f64,
    next_handles: BezierHandles,
) -> bool {
    let incoming_dt = keyframe_time - previous_time;
    let outgoing_dt = next_time - keyframe_time;
    if incoming_dt <= 0.0 || outgoing_dt <= 0.0 {
        return true;
    }
    let incoming_value_delta = keyframe_value - previous_value;
    let outgoing_value_delta = next_value - keyframe_value;
    let incoming_x = previous_time + previous_handles.in_x * incoming_dt - keyframe_time;
    let incoming_y = previous_value + previous_handles.in_y * incoming_value_delta - keyframe_value;
    let outgoing_x = keyframe_time + next_handles.out_x * outgoing_dt - keyframe_time;
    let outgoing_y = keyframe_value + next_handles.out_y * outgoing_value_delta - keyframe_value;
    let incoming_len = incoming_x.hypot(incoming_y);
    let outgoing_len = outgoing_x.hypot(outgoing_y);
    if incoming_len <= f64::EPSILON || outgoing_len <= f64::EPSILON {
        return true;
    }
    let cross = incoming_x * outgoing_y - incoming_y * outgoing_x;
    cross.abs() <= 1e-6 * incoming_len * outgoing_len
}

fn validate_motion_path(
    diagnostics: &mut Vec<ProfessionalDiagnostic>,
    animation_id: &str,
    path: &MotionPath,
) {
    if path.points.len() < 2 {
        diagnostics.push(ProfessionalDiagnostic::error(
            CapabilityArea::ParameterAnimation,
            format!("parameter animation {animation_id} motion path requires at least two points"),
        ));
    }
    for point in &path.points {
        if !point.time_s.is_finite() || !point.x.is_finite() || !point.y.is_finite() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                format!("parameter animation {animation_id} has a non-finite motion path point"),
            ));
        }
        for control in [point.outgoing_control, point.incoming_control]
            .into_iter()
            .flatten()
        {
            if !control.x.is_finite() || !control.y.is_finite() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::ParameterAnimation,
                    format!(
                        "parameter animation {animation_id} has a non-finite motion path control point"
                    ),
                ));
            }
        }
    }
}

fn validate_parameter_animation_value(
    diagnostics: &mut Vec<ProfessionalDiagnostic>,
    animation_id: &str,
    parameter: &str,
    keyframe: &Keyframe,
) {
    if !keyframe.value.is_finite() {
        return;
    }
    match parameter {
        "title.opacity" | "overlay.opacity" if !(0.0..=1.0).contains(&keyframe.value) => {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                format!(
                    "parameter animation {animation_id} target {parameter} value {} must be in [0, 1]",
                    keyframe.value
                ),
            ));
        }
        "title.font_size" | "overlay.scale" if keyframe.value <= 0.0 => {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                format!(
                    "parameter animation {animation_id} target {parameter} value {} must be positive",
                    keyframe.value
                ),
            ));
        }
        "overlay.blur" | "awidat.blur.radius_px" | "awidat.shake.intensity_px"
            if keyframe.value < 0.0 =>
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                format!(
                    "parameter animation {animation_id} target {parameter} value {} must be non-negative",
                    keyframe.value
                ),
            ));
        }
        "awidat.shake.frequency_hz" if keyframe.value <= 0.0 => {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                format!(
                    "parameter animation {animation_id} target {parameter} value {} must be positive",
                    keyframe.value
                ),
            ));
        }
        "awidat.warp.center_x" | "awidat.warp.center_y"
            if !(0.0..=1.0).contains(&keyframe.value) =>
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                format!(
                    "parameter animation {animation_id} target {parameter} value {} must be in [0, 1]",
                    keyframe.value
                ),
            ));
        }
        _ => {}
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
        /// Parameter path. Keyframe times are track-local seconds in the rendered track stream:
        /// `t=0` is the beginning of the track after clips/gaps are concatenated, not the absolute
        /// timeline time of an individual source clip.
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
    /// Optional normalized cubic Bezier handles for interpolation into the next keyframe.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bezier: Option<BezierHandles>,
    /// Constraint mode for Bezier handles at this keyframe.
    #[serde(default, skip_serializing_if = "TangentMode::is_auto")]
    pub tangent_mode: TangentMode,
    /// Optional spring parameters for spring interpolation into the next keyframe.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub spring: Option<SpringParameters>,
}

impl Keyframe {
    /// Convenience constructor for a linear keyframe.
    pub fn linear(time_s: f64, value: f64) -> Self {
        Self {
            time_s,
            value,
            interpolation: KeyframeInterpolation::Linear,
            easing: Easing::Linear,
            bezier: None,
            tangent_mode: TangentMode::Auto,
            spring: None,
        }
    }
}

/// Normalized cubic Bezier handles for a keyframe segment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BezierHandles {
    /// Outgoing control point x, normalized to the segment duration.
    pub out_x: f64,
    /// Outgoing control point y, normalized to the value delta.
    pub out_y: f64,
    /// Incoming control point x, normalized to the segment duration.
    pub in_x: f64,
    /// Incoming control point y, normalized to the value delta.
    pub in_y: f64,
}

/// Constraint mode for Bezier keyframe tangent handles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TangentMode {
    /// Editor/runtime may keep tangents smooth automatically.
    #[default]
    Auto,
    /// Incoming and outgoing handles remain collinear and proportional.
    Aligned,
    /// Handles are independent.
    Broken,
    /// Tangents flatten at the keyframe.
    Flat,
}

impl TangentMode {
    /// True when this mode is the default auto behavior.
    pub fn is_auto(value: &Self) -> bool {
        matches!(value, Self::Auto)
    }
}

/// Physical spring parameters for duration-light motion.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpringParameters {
    /// Moving mass. Must be positive.
    pub mass: f64,
    /// Spring stiffness. Must be positive.
    pub stiffness: f64,
    /// Damping coefficient. Must be non-negative.
    pub damping: f64,
}

impl Default for SpringParameters {
    fn default() -> Self {
        Self {
            mass: 1.0,
            stiffness: 100.0,
            damping: 12.0,
        }
    }
}

/// A 2D motion path attached to a position parameter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MotionPath {
    /// Ordered path samples in clip-local seconds.
    #[serde(default)]
    pub points: Vec<MotionPathPoint>,
}

/// One point on a 2D motion path.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotionPathPoint {
    /// Time in seconds.
    pub time_s: f64,
    /// Horizontal offset in viewport-width units.
    pub x: f64,
    /// Vertical offset in viewport-height units.
    pub y: f64,
    /// Optional outgoing spatial control point for the segment after this point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outgoing_control: Option<MotionPathControlPoint>,
    /// Optional incoming spatial control point for the segment before this point.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incoming_control: Option<MotionPathControlPoint>,
}

/// Absolute 2D control point for a cubic spatial motion-path segment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotionPathControlPoint {
    /// Horizontal offset in viewport-width units.
    pub x: f64,
    /// Vertical offset in viewport-height units.
    pub y: f64,
}

/// Keyframe interpolation mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyframeInterpolation {
    /// Hold previous value.
    Hold,
    /// Discrete step to the next value at the segment midpoint.
    Step,
    /// Linear interpolation.
    #[default]
    Linear,
    /// Bezier curve handles.
    Bezier,
    /// Physically-inspired spring interpolation.
    Spring,
}

/// Behavior outside an animation's explicit keyframe range.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtrapolationMode {
    /// Hold the nearest endpoint value.
    #[default]
    Hold,
    /// Continue the velocity of the first or last keyframe segment.
    Linear,
}

impl ExtrapolationMode {
    /// True when this mode is the default hold behavior.
    pub fn is_hold(value: &Self) -> bool {
        matches!(value, Self::Hold)
    }
}

/// Easing mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    /// No easing.
    #[default]
    Linear,
    /// Sine ease in.
    EaseInSine,
    /// Sine ease out.
    EaseOutSine,
    /// Sine ease in and out.
    EaseInOutSine,
    /// Ease in.
    EaseIn,
    /// Ease out.
    EaseOut,
    /// Ease in and out.
    EaseInOut,
    /// Cubic ease in.
    EaseInCubic,
    /// Cubic ease out.
    EaseOutCubic,
    /// Cubic ease in and out.
    EaseInOutCubic,
    /// Quartic ease in.
    EaseInQuart,
    /// Quartic ease out.
    EaseOutQuart,
    /// Quartic ease in and out.
    EaseInOutQuart,
    /// Quintic ease in.
    EaseInQuint,
    /// Quintic ease out.
    EaseOutQuint,
    /// Quintic ease in and out.
    EaseInOutQuint,
    /// Exponential ease in.
    EaseInExpo,
    /// Exponential ease out.
    EaseOutExpo,
    /// Exponential ease in and out.
    EaseInOutExpo,
    /// Circular ease in.
    EaseInCirc,
    /// Circular ease out.
    EaseOutCirc,
    /// Circular ease in and out.
    EaseInOutCirc,
    /// Back ease in.
    EaseInBack,
    /// Back ease out.
    EaseOutBack,
    /// Back ease in and out.
    EaseInOutBack,
    /// Elastic ease in.
    EaseInElastic,
    /// Elastic ease out.
    EaseOutElastic,
    /// Elastic ease in and out.
    EaseInOutElastic,
    /// Bounce ease in.
    EaseInBounce,
    /// Bounce ease out.
    EaseOutBounce,
    /// Bounce ease in and out.
    EaseInOutBounce,
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
    /// Video/media asset.
    Video,
    /// Color value.
    Color,
    /// Numeric value.
    Number,
    /// Target timeline clip id.
    TargetClip,
    /// Safe-area profile id.
    SafeAreaProfile,
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
        if self.has_cycle(&node_ids) {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::CompositionGraph,
                format!("composition graph {} contains a cycle", self.id),
            ));
        }
        diagnostics
    }

    fn has_cycle(&self, node_ids: &HashSet<&str>) -> bool {
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for edge in &self.edges {
            if node_ids.contains(edge.from.as_str()) && node_ids.contains(edge.to.as_str()) {
                adjacency
                    .entry(edge.from.clone())
                    .or_default()
                    .push(edge.to.clone());
            }
        }

        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        for node in &self.nodes {
            if composition_visit_has_cycle(
                node.id.as_str(),
                &adjacency,
                &mut visiting,
                &mut visited,
            ) {
                return true;
            }
        }
        false
    }
}

fn composition_visit_has_cycle(
    node_id: &str,
    adjacency: &HashMap<String, Vec<String>>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
) -> bool {
    if visited.contains(node_id) {
        return false;
    }
    if !visiting.insert(node_id.to_string()) {
        return true;
    }
    if let Some(children) = adjacency.get(node_id) {
        for child in children {
            if composition_visit_has_cycle(child, adjacency, visiting, visited) {
                return true;
            }
        }
    }
    visiting.remove(node_id);
    visited.insert(node_id.to_string());
    false
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
    /// Persisted but unsupported/custom node.
    Unsupported,
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
    /// Four-corner perspective/corner-pin distortion driven by a surface track.
    CornerPin,
    /// 3D scene container.
    Scene3d,
    /// Particle emitter.
    ParticleEmitter,
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
    /// Reviewable segmentation prompt/correction packages.
    #[serde(default)]
    pub prompt_packages: Vec<SegmentationPromptPackage>,
    /// Runtime/session operations used to produce or update segmentation outputs.
    #[serde(default)]
    pub session_operations: Vec<SegmentationSessionOperation>,
    /// Interchange profiles for generated mask artifacts.
    #[serde(default)]
    pub mask_artifacts: Vec<MaskArtifactProfile>,
    /// Point/planar/surface tracks.
    #[serde(default)]
    pub tracks: Vec<TrackSidecar>,
    /// Subject-aware crop/reframe paths for vertical, square, or custom output.
    #[serde(default)]
    pub reframe_paths: Vec<ReframePath>,
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
        for prompt_package in &self.prompt_packages {
            diagnostics.extend(prompt_package.validate());
        }
        for operation in &self.session_operations {
            diagnostics.extend(operation.validate());
        }
        for artifact in &self.mask_artifacts {
            diagnostics.extend(artifact.validate());
        }
        for track in &self.tracks {
            if track.samples.is_empty() {
                diagnostics.push(ProfessionalDiagnostic::warning(
                    CapabilityArea::TrackingMasksMattes,
                    format!("track {} has no samples", track.id),
                ));
            }
            let mut previous_frame = None;
            for sample in &track.samples {
                if let Some(previous) = previous_frame
                    && sample.frame < previous
                {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::TrackingMasksMattes,
                        format!("track {} sample frames must be sorted", track.id),
                    ));
                    break;
                }
                previous_frame = Some(sample.frame);
            }
            validate_optional_confidence(
                &mut diagnostics,
                CapabilityArea::TrackingMasksMattes,
                track.confidence,
                format!("track {}", track.id),
            );
        }
        for path in &self.reframe_paths {
            diagnostics.extend(path.validate());
        }
        for mask in &self.masks {
            if let Some(quality) = &mask.quality {
                diagnostics.extend(quality.validate(format!("mask {}", mask.id)));
            }
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
            if matte.alpha_source.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::TrackingMasksMattes,
                    format!("matte {} has an empty alpha source", matte.id),
                ));
            }
            if let Some(generation) = &matte.generation {
                diagnostics.extend(generation.validate(format!("matte {}", matte.id)));
            }
            validate_optional_confidence(
                &mut diagnostics,
                CapabilityArea::TrackingMasksMattes,
                matte.confidence,
                format!("matte {}", matte.id),
            );
            if let Some(quality) = &matte.quality {
                diagnostics.extend(quality.validate(format!("matte {}", matte.id)));
            }
        }
        diagnostics
    }
}

/// One operation in an optional segmentation runtime session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SegmentationSessionOperation {
    /// Stable operation id.
    pub id: String,
    /// Stable session id.
    pub session_id: String,
    /// Operation kind.
    #[serde(default)]
    pub kind: SegmentationSessionOperationKind,
    /// Optional frame associated with the operation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub frame: Option<u64>,
    /// Optional object id affected by the operation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target_object_id: Option<String>,
    /// Optional prompt package consumed by the operation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub prompt_package_id: Option<String>,
    /// Optional output mask id produced or persisted by the operation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_mask_id: Option<String>,
    /// Runtime status.
    #[serde(default)]
    pub status: SegmentationRuntimeStatus,
    /// Human-readable diagnostic or status detail.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub message: Option<String>,
    /// Artifact paths or ids produced/read by this operation.
    #[serde(default)]
    pub artifact_refs: Vec<String>,
}

impl SegmentationSessionOperation {
    fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                "segmentation session operation has an empty id",
            ));
        }
        if self.session_id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation session operation {} has an empty session id",
                    self.id
                ),
            ));
        }
        if matches!(
            self.kind,
            SegmentationSessionOperationKind::AddPrompt
                | SegmentationSessionOperationKind::CorrectPrompt
        ) && self
            .prompt_package_id
            .as_ref()
            .map(|value| value.trim().is_empty())
            .unwrap_or(true)
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation session operation {} requires a prompt package id",
                    self.id
                ),
            ));
        }
        if matches!(self.kind, SegmentationSessionOperationKind::RemoveObject)
            && self
                .target_object_id
                .as_ref()
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation session operation {} requires a target object id",
                    self.id
                ),
            ));
        }
        if matches!(self.kind, SegmentationSessionOperationKind::PersistOutput) {
            if self
                .output_mask_id
                .as_ref()
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
            {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::TrackingMasksMattes,
                    format!(
                        "segmentation session operation {} requires an output mask id",
                        self.id
                    ),
                ));
            }
            if self
                .artifact_refs
                .iter()
                .all(|value| value.trim().is_empty())
            {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::TrackingMasksMattes,
                    format!(
                        "segmentation session operation {} requires an artifact ref",
                        self.id
                    ),
                ));
            }
        }
        if self.status.requires_message()
            && self
                .message
                .as_ref()
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
        {
            diagnostics.push(ProfessionalDiagnostic::warning(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation session operation {} status requires a diagnostic message",
                    self.id
                ),
            ));
        }
        diagnostics
    }
}

/// Segmentation runtime operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentationSessionOperationKind {
    /// Start or initialize a segmentation session.
    #[default]
    Start,
    /// Add a prompt to the session.
    AddPrompt,
    /// Correct an existing object or frame prompt.
    CorrectPrompt,
    /// Clear prompts or outputs for a frame.
    ClearFrame,
    /// Clear all prompts or outputs for a video/session.
    ClearVideo,
    /// Remove an object id from the session.
    RemoveObject,
    /// Propagate masks through time.
    Propagate,
    /// Cancel active propagation or processing.
    Cancel,
    /// Close the runtime session.
    Close,
    /// Persist runtime output into durable artifacts.
    PersistOutput,
}

/// Segmentation runtime status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentationRuntimeStatus {
    /// Operation is planned but not started.
    #[default]
    Planned,
    /// Runtime is ready.
    Ready,
    /// Operation is running.
    Running,
    /// Operation produced partial output.
    Partial,
    /// Operation completed.
    Complete,
    /// Operation was canceled.
    Canceled,
    /// Runtime needs prompts before it can proceed.
    NoPrompts,
    /// Runtime expected masks that are missing.
    MissingMasks,
    /// Runtime expected object ids that are missing.
    NoObjectIds,
    /// Selected device or backend is unsupported.
    UnsupportedDevice,
    /// Output confidence was too low for automatic use.
    LowConfidence,
    /// Operation failed.
    Failed,
}

impl SegmentationRuntimeStatus {
    fn requires_message(self) -> bool {
        matches!(
            self,
            Self::Partial
                | Self::Canceled
                | Self::NoPrompts
                | Self::MissingMasks
                | Self::NoObjectIds
                | Self::UnsupportedDevice
                | Self::LowConfidence
                | Self::Failed
        )
    }
}

/// Durable description of a generated mask artifact.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MaskArtifactProfile {
    /// Stable artifact id.
    pub id: String,
    /// Artifact encoding kind.
    #[serde(default)]
    pub kind: MaskArtifactKind,
    /// Project-relative artifact path or URI.
    pub path: String,
    /// Object ids represented by this artifact.
    #[serde(default)]
    pub object_ids: Vec<String>,
    /// Optional source/timeline range covered by the artifact.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub frame_range: Option<SourceRange>,
    /// Optional number of frames represented by this artifact.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub frame_count: Option<u64>,
    /// Optional width in pixels.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub width: Option<u32>,
    /// Optional height in pixels.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub height: Option<u32>,
    /// Optional checksum for artifact validation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub checksum: Option<String>,
}

impl MaskArtifactProfile {
    fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                "mask artifact has an empty id",
            ));
        }
        if self.path.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("mask artifact {} has an empty path", self.id),
            ));
        }
        if self.object_ids.iter().any(|id| id.trim().is_empty()) {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("mask artifact {} has an empty object id", self.id),
            ));
        }
        if matches!(
            self.kind,
            MaskArtifactKind::PerObjectPng | MaskArtifactKind::BinaryMask
        ) && self.object_ids.is_empty()
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("mask artifact {} requires at least one object id", self.id),
            ));
        }
        if matches!(self.frame_count, Some(0)) {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "mask artifact {} frame count must be greater than zero",
                    self.id
                ),
            ));
        }
        if matches!(self.width, Some(0)) {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("mask artifact {} width must be greater than zero", self.id),
            ));
        }
        if matches!(self.height, Some(0)) {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("mask artifact {} height must be greater than zero", self.id),
            ));
        }
        if let Some(range) = &self.frame_range
            && !range.is_valid()
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("mask artifact {} has an invalid range", self.id),
            ));
        }
        if self
            .checksum
            .as_ref()
            .map(|value| value.trim().is_empty())
            .unwrap_or(false)
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("mask artifact {} has an empty checksum", self.id),
            ));
        }
        diagnostics
    }
}

/// Mask artifact interchange encoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskArtifactKind {
    /// Single image packing object/frame mask data.
    #[default]
    PackedPng,
    /// One PNG sequence per object.
    PerObjectPng,
    /// Raw or sidecar binary mask.
    BinaryMask,
    /// Run-length encoded mask.
    Rle,
    /// COCO-compatible run-length encoded mask.
    CocoRle,
    /// Video container with alpha channel.
    VideoAlpha,
}

/// Reviewable prompt/correction package for subject or object segmentation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SegmentationPromptPackage {
    /// Stable package id.
    pub id: String,
    /// Timeline clip id or source clip id receiving segmentation.
    pub clip_id: String,
    /// Optional source/timeline range covered by the prompt package.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub range: Option<SourceRange>,
    /// Stable object id inside the prompt/tracking session.
    pub target_object_id: String,
    /// Human-readable subject/object label.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub subject_label: Option<String>,
    /// Intended downstream use for generated masks/mattes.
    #[serde(default)]
    pub intent: SegmentationIntent,
    /// Ordered prompts and corrections.
    #[serde(default)]
    pub prompts: Vec<SegmentationPrompt>,
    /// Optional text-grounding evidence that produced this prompt package.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub grounding: Option<GroundingEvidence>,
    /// Optional mask id produced from this package.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_mask_id: Option<String>,
    /// Optional matte id produced from this package.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output_matte_id: Option<String>,
    /// Review lifecycle.
    #[serde(default)]
    pub status: ReviewStatus,
}

impl SegmentationPromptPackage {
    fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                "segmentation prompt package has an empty id",
            ));
        }
        if self.clip_id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation prompt package {} has an empty clip id",
                    self.id
                ),
            ));
        }
        if self.target_object_id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation prompt package {} has an empty target object id",
                    self.id
                ),
            ));
        }
        if let Some(range) = &self.range
            && (!range.start_s.is_finite()
                || !range.end_s.is_finite()
                || range.end_s <= range.start_s)
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation prompt package {} has an invalid range",
                    self.id
                ),
            ));
        }
        if self.prompts.is_empty() {
            diagnostics.push(ProfessionalDiagnostic::warning(
                CapabilityArea::TrackingMasksMattes,
                format!("segmentation prompt package {} has no prompts", self.id),
            ));
        }
        let mut previous_frame = None;
        let mut reported_unsorted_prompts = false;
        for prompt in &self.prompts {
            if let Some(previous) = previous_frame
                && prompt.frame < previous
                && !reported_unsorted_prompts
            {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::TrackingMasksMattes,
                    format!(
                        "segmentation prompt package {} prompts must be sorted by frame",
                        self.id
                    ),
                ));
                reported_unsorted_prompts = true;
            }
            previous_frame = Some(prompt.frame);
            diagnostics.extend(prompt.validate(&self.id));
        }
        if let Some(grounding) = &self.grounding {
            diagnostics.extend(grounding.validate(&self.id));
        }
        diagnostics
    }
}

/// Intended use for a segmentation prompt package.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentationIntent {
    /// General segmentation mask.
    #[default]
    Mask,
    /// Subject alpha matte or cutout.
    SubjectMatte,
    /// Text/overlay should appear behind this subject.
    TextBehindSubject,
    /// Background blur/removal around this subject.
    BackgroundTreatment,
    /// Subject-aware reframe evidence.
    Reframe,
}

/// One segmentation prompt or correction.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SegmentationPrompt {
    /// Frame number for this prompt.
    pub frame: u64,
    /// Prompt type.
    #[serde(default)]
    pub kind: SegmentationPromptKind,
    /// Positive/negative label.
    #[serde(default)]
    pub label: SegmentationPromptLabel,
    /// Normalized prompt points as `[x, y]`.
    #[serde(default)]
    pub points: Vec<[f64; 2]>,
    /// Optional normalized `[x0, y0, x1, y1]` box.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub box_xyxy: Option<[f64; 4]>,
    /// Optional existing mask/matte sidecar id or path.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mask_ref: Option<String>,
    /// Optional note explaining the prompt/correction.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,
}

impl SegmentationPrompt {
    fn validate(&self, package_id: &str) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        match self.kind {
            SegmentationPromptKind::Point => {
                if self.points.is_empty() {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::TrackingMasksMattes,
                        format!(
                            "segmentation prompt package {package_id} point prompt has no points"
                        ),
                    ));
                }
            }
            SegmentationPromptKind::Box => {
                if self.box_xyxy.is_none() {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::TrackingMasksMattes,
                        format!("segmentation prompt package {package_id} box prompt has no box"),
                    ));
                }
            }
            SegmentationPromptKind::Mask => {
                if self
                    .mask_ref
                    .as_ref()
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true)
                {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::TrackingMasksMattes,
                        format!(
                            "segmentation prompt package {package_id} mask prompt has no mask ref"
                        ),
                    ));
                }
            }
        }
        for point in &self.points {
            if !normalized_pair_is_valid(*point) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::TrackingMasksMattes,
                    format!(
                        "segmentation prompt package {package_id} point coordinates must be in 0..=1"
                    ),
                ));
            }
        }
        if let Some(bbox) = self.box_xyxy
            && !normalized_box_is_valid(bbox)
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation prompt package {package_id} box coordinates must be ordered and in 0..=1"
                ),
            ));
        }
        diagnostics
    }
}

/// Segmentation prompt shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentationPromptKind {
    /// One or more positive/negative point prompts.
    #[default]
    Point,
    /// Bounding box prompt.
    Box,
    /// Existing mask/matte prompt or correction.
    Mask,
}

/// Segmentation prompt polarity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentationPromptLabel {
    /// Foreground/positive prompt.
    #[default]
    Positive,
    /// Background/negative prompt.
    Negative,
}

/// Text-grounding evidence used to create a segmentation prompt package.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GroundingEvidence {
    /// Text query sent to a detector or vision-language model.
    pub text_query: String,
    /// Model, adapter, or tool source that produced the detections.
    pub source: String,
    /// Coordinate system used by detection boxes.
    #[serde(default)]
    pub box_format: GroundingBoxFormat,
    /// Source image width in pixels when known.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image_width: Option<u32>,
    /// Source image height in pixels when known.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image_height: Option<u32>,
    /// Detector box threshold in 0..=1 when applicable.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub box_threshold: Option<f64>,
    /// Detector text threshold in 0..=1 when applicable.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text_threshold: Option<f64>,
    /// Evidence outcome for agent/UI decision-making.
    #[serde(default)]
    pub status: GroundingEvidenceStatus,
    /// Required detail for blocked or degraded grounding outcomes.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub status_reason: Option<String>,
    /// Detections returned by the grounding step.
    #[serde(default)]
    pub detections: Vec<GroundingDetection>,
}

impl GroundingEvidence {
    fn validate(&self, package_id: &str) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        let query = self.text_query.trim();
        if query.is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("segmentation prompt package {package_id} has an empty grounding query"),
            ));
        } else if query != query.to_lowercase() || !query.ends_with('.') {
            diagnostics.push(ProfessionalDiagnostic::warning(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation prompt package {package_id} grounding query should be lowercase and end with a period"
                ),
            ));
        }
        if self.source.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("segmentation prompt package {package_id} has an empty grounding source"),
            ));
        }
        if let Some(width) = self.image_width
            && width == 0
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("segmentation prompt package {package_id} has invalid image width"),
            ));
        }
        if let Some(height) = self.image_height
            && height == 0
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("segmentation prompt package {package_id} has invalid image height"),
            ));
        }
        validate_optional_unit_interval(
            &mut diagnostics,
            self.box_threshold,
            format!("segmentation prompt package {package_id} box threshold"),
        );
        validate_optional_unit_interval(
            &mut diagnostics,
            self.text_threshold,
            format!("segmentation prompt package {package_id} text threshold"),
        );
        let status_reason = self
            .status_reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty());
        match self.status {
            GroundingEvidenceStatus::NotEvaluated => {}
            GroundingEvidenceStatus::AcceptedDetections => {
                if self.detections.is_empty() {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::TrackingMasksMattes,
                        format!(
                            "segmentation prompt package {package_id} grounding accepted detections status requires at least one detection"
                        ),
                    ));
                }
            }
            GroundingEvidenceStatus::NoDetections => {
                if !self.detections.is_empty() {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::TrackingMasksMattes,
                        format!(
                            "segmentation prompt package {package_id} grounding no detections status cannot include detections"
                        ),
                    ));
                }
            }
            GroundingEvidenceStatus::LowConfidence => {
                if status_reason.is_none() {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::TrackingMasksMattes,
                        format!(
                            "segmentation prompt package {package_id} grounding low confidence status requires a reason"
                        ),
                    ));
                }
            }
            GroundingEvidenceStatus::MissingRuntimeEvidence => {
                if status_reason.is_none() {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::TrackingMasksMattes,
                        format!(
                            "segmentation prompt package {package_id} grounding missing runtime evidence status requires a reason"
                        ),
                    ));
                }
                if !self.detections.is_empty() {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::TrackingMasksMattes,
                        format!(
                            "segmentation prompt package {package_id} grounding missing runtime evidence status cannot include detections"
                        ),
                    ));
                }
            }
        }
        if self
            .status_reason
            .as_ref()
            .map(|value| value.trim().is_empty())
            .unwrap_or(false)
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation prompt package {package_id} grounding status reason is empty"
                ),
            ));
        }
        for detection in &self.detections {
            diagnostics.extend(detection.validate(
                package_id,
                self.box_format,
                self.image_width,
                self.image_height,
            ));
        }
        diagnostics
    }
}

/// Text-grounding evidence outcome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundingEvidenceStatus {
    /// Grounding was recorded before an outcome was evaluated.
    #[default]
    NotEvaluated,
    /// Runtime produced detections accepted for downstream prompting.
    AcceptedDetections,
    /// Runtime completed but produced no detections above threshold.
    NoDetections,
    /// Runtime produced detections that require review due low confidence.
    LowConfidence,
    /// Grounding could not run or evidence was not captured.
    MissingRuntimeEvidence,
}

/// Grounding detection box coordinate format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundingBoxFormat {
    /// Normalized `[x0, y0, x1, y1]` coordinates in 0..=1.
    #[default]
    XyxyNormalized,
    /// Pixel `[x0, y0, x1, y1]` coordinates.
    XyxyPixels,
}

/// One object detection returned by a grounding step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundingDetection {
    /// Class/object label returned by the grounding step.
    pub class_name: String,
    /// Bounding box in the parent evidence `box_format`.
    pub bbox_xyxy: [f64; 4],
    /// Confidence score in 0..=1 when available.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub score: Option<f64>,
    /// Frame number for video detections when known.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub frame: Option<u64>,
    /// Optional mask sidecar id or artifact path produced by the grounding step.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mask_ref: Option<String>,
}

impl GroundingDetection {
    fn validate(
        &self,
        package_id: &str,
        box_format: GroundingBoxFormat,
        image_width: Option<u32>,
        image_height: Option<u32>,
    ) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.class_name.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation prompt package {package_id} grounding detection has an empty class name"
                ),
            ));
        }
        let bbox_valid = match box_format {
            GroundingBoxFormat::XyxyNormalized => normalized_box_is_valid(self.bbox_xyxy),
            GroundingBoxFormat::XyxyPixels => {
                pixel_box_is_valid(self.bbox_xyxy, image_width, image_height)
            }
        };
        if !bbox_valid {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation prompt package {package_id} grounding detection bbox is invalid"
                ),
            ));
        }
        validate_optional_unit_interval(
            &mut diagnostics,
            self.score,
            format!("segmentation prompt package {package_id} grounding detection score"),
        );
        if self
            .mask_ref
            .as_ref()
            .map(|value| value.trim().is_empty())
            .unwrap_or(false)
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "segmentation prompt package {package_id} grounding detection mask ref is empty"
                ),
            ));
        }
        diagnostics
    }
}

impl Default for GroundingDetection {
    fn default() -> Self {
        Self {
            class_name: String::new(),
            bbox_xyxy: [0.0, 0.0, 0.0, 0.0],
            score: None,
            frame: None,
            mask_ref: None,
        }
    }
}

fn normalized_pair_is_valid(point: [f64; 2]) -> bool {
    point
        .iter()
        .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
}

fn normalized_box_is_valid(bbox: [f64; 4]) -> bool {
    bbox.iter()
        .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        && bbox[0] < bbox[2]
        && bbox[1] < bbox[3]
}

fn pixel_box_is_valid(bbox: [f64; 4], image_width: Option<u32>, image_height: Option<u32>) -> bool {
    if !bbox.iter().all(|value| value.is_finite() && *value >= 0.0)
        || bbox[0] >= bbox[2]
        || bbox[1] >= bbox[3]
    {
        return false;
    }
    if let Some(width) = image_width
        && width > 0
        && bbox[2] > f64::from(width)
    {
        return false;
    }
    if let Some(height) = image_height
        && height > 0
        && bbox[3] > f64::from(height)
    {
        return false;
    }
    true
}

/// Subject-aware crop/reframe path for one timeline clip.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReframePath {
    /// Stable reframe id.
    pub id: String,
    /// Timeline clip id that receives the crop path.
    pub clip_id: String,
    /// Delivery aspect ratio such as `9:16`, `1:1`, or `16:9`.
    pub aspect_ratio: String,
    /// Source media width in pixels.
    #[serde(default)]
    pub source_width: u32,
    /// Source media height in pixels.
    #[serde(default)]
    pub source_height: u32,
    /// Target canvas width in pixels.
    #[serde(default)]
    pub target_width: u32,
    /// Target canvas height in pixels.
    #[serde(default)]
    pub target_height: u32,
    /// Ordered crop center/scale samples in output time.
    #[serde(default)]
    pub keyframes: Vec<ReframeKeyframe>,
    /// Smoothing policy for interpolation between keyframes.
    #[serde(default)]
    pub smoothing: ReframeSmoothing,
    /// Optional evidence track that drove the path.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub evidence_track_id: Option<String>,
    /// Optional safe-area policy label, e.g. `mobile`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub safe_area: Option<String>,
}

impl ReframePath {
    fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                "reframe path has an empty id",
            ));
        }
        if self.clip_id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("reframe path {} has an empty clip id", self.id),
            ));
        }
        if self.aspect_ratio.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::warning(
                CapabilityArea::TrackingMasksMattes,
                format!("reframe path {} has an empty aspect ratio", self.id),
            ));
        }
        if self.source_width == 0
            || self.source_height == 0
            || self.target_width == 0
            || self.target_height == 0
        {
            diagnostics.push(ProfessionalDiagnostic::warning(
                CapabilityArea::TrackingMasksMattes,
                format!(
                    "reframe path {} has incomplete source/target dimensions",
                    self.id
                ),
            ));
        }
        if self.keyframes.is_empty() {
            diagnostics.push(ProfessionalDiagnostic::warning(
                CapabilityArea::TrackingMasksMattes,
                format!("reframe path {} has no keyframes", self.id),
            ));
        }
        let mut previous_time = None;
        for keyframe in &self.keyframes {
            if let Some(previous) = previous_time
                && keyframe.time_s < previous
            {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::TrackingMasksMattes,
                    format!("reframe path {} keyframes must be sorted", self.id),
                ));
            }
            previous_time = Some(keyframe.time_s);
            if !keyframe.time_s.is_finite() {
                diagnostics.push(ProfessionalDiagnostic::warning(
                    CapabilityArea::TrackingMasksMattes,
                    format!("reframe path {} has a non-finite keyframe time", self.id),
                ));
            }
            if !(0.0..=1.0).contains(&keyframe.center[0])
                || !(0.0..=1.0).contains(&keyframe.center[1])
            {
                diagnostics.push(ProfessionalDiagnostic::warning(
                    CapabilityArea::TrackingMasksMattes,
                    format!(
                        "reframe path {} center coordinates must be in 0..=1",
                        self.id
                    ),
                ));
            }
            if !keyframe.scale.is_finite() || keyframe.scale < 1.0 {
                diagnostics.push(ProfessionalDiagnostic::warning(
                    CapabilityArea::TrackingMasksMattes,
                    format!("reframe path {} scale must be finite and >= 1", self.id),
                ));
            }
            validate_optional_confidence(
                &mut diagnostics,
                CapabilityArea::TrackingMasksMattes,
                keyframe.confidence,
                format!("reframe path {}", self.id),
            );
        }
        diagnostics
    }
}

/// One crop center/scale sample in a [`ReframePath`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReframeKeyframe {
    /// Time in clip/output seconds.
    pub time_s: f64,
    /// Normalized crop center `[x, y]`, each 0..=1.
    pub center: [f64; 2],
    /// Crop zoom where `1.0` means no extra zoom.
    pub scale: f64,
    /// Confidence of the subject/framing evidence.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confidence: Option<f64>,
}

impl Default for ReframeKeyframe {
    fn default() -> Self {
        Self {
            time_s: 0.0,
            center: [0.5, 0.5],
            scale: 1.0,
            confidence: None,
        }
    }
}

/// Reframe path smoothing policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReframeSmoothing {
    /// No smoothing; use authored keyframes directly.
    None,
    /// Mild interpolation for manual or high-confidence paths.
    Gentle,
    /// Default smoothing for automatic subject-follow paths.
    #[default]
    Moderate,
    /// Heavy smoothing for noisy detections.
    Strong,
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
    /// Optional track binding.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub track_id: Option<String>,
    /// Optional clip/range attachment.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub attached_clip_id: Option<String>,
    /// Mask operation.
    #[serde(default)]
    pub operation: MaskOperation,
    /// Keyframed paths.
    #[serde(default)]
    pub keyframes: Vec<MaskKeyframe>,
    /// Quality and review metadata for this mask.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quality: Option<MaskQualityScorecard>,
}

/// Quality metadata used to decide whether a mask/matte is usable in a finished edit.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MaskQualityScorecard {
    /// Normalized `[x0, y0, x1, y1]` bounds for the mask/matte.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bbox_xyxy: Option<[f64; 4]>,
    /// Approximate foreground area in pixels.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub area_px: Option<u64>,
    /// Foreground coverage as a normalized fraction of the frame.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub coverage: Option<f64>,
    /// Stability score in 0..=1.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub stability: Option<f64>,
    /// Confidence score in 0..=1.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confidence: Option<f64>,
    /// Whether this mask overlaps another subject/object in a conflicting way.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub overlap_conflict: Option<bool>,
    /// Fraction of expected frames with usable mask/matte evidence.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub frame_coverage: Option<f64>,
    /// Human/agent review decision.
    #[serde(default)]
    pub decision: MaskReviewDecision,
    /// Review notes.
    #[serde(default)]
    pub notes: Vec<String>,
}

impl MaskQualityScorecard {
    fn validate(&self, label: String) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if let Some(bbox) = self.bbox_xyxy
            && !normalized_box_is_valid(bbox)
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("{label} quality bbox must be ordered and in 0..=1"),
            ));
        }
        if matches!(self.area_px, Some(0)) {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("{label} quality area must be greater than zero"),
            ));
        }
        validate_optional_unit_interval(
            &mut diagnostics,
            self.coverage,
            format!("{label} quality coverage"),
        );
        validate_optional_unit_interval(
            &mut diagnostics,
            self.stability,
            format!("{label} quality stability"),
        );
        validate_optional_unit_interval(
            &mut diagnostics,
            self.confidence,
            format!("{label} quality confidence"),
        );
        validate_optional_unit_interval(
            &mut diagnostics,
            self.frame_coverage,
            format!("{label} quality frame coverage"),
        );
        if self.overlap_conflict == Some(true) {
            diagnostics.push(ProfessionalDiagnostic::warning(
                CapabilityArea::TrackingMasksMattes,
                format!("{label} quality reports an overlap conflict"),
            ));
        }
        diagnostics
    }
}

/// Review decision for mask/matte quality.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskReviewDecision {
    /// Proposed and not reviewed yet.
    #[default]
    Proposed,
    /// Accepted for downstream compositing/reframing.
    Accepted,
    /// Needs review before use.
    NeedsReview,
    /// Rejected for quality or fit.
    Rejected,
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
    /// Reproducible generation recipe for this matte.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub generation: Option<MatteGenerationRecipe>,
    /// Quality and review metadata for this matte.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub quality: Option<MaskQualityScorecard>,
}

/// Model/tool settings that produced a matte or cutout artifact.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MatteGenerationRecipe {
    /// Tool, adapter, or model provider label.
    pub source: String,
    /// Optional model/checkpoint label.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    /// Output artifact kind.
    #[serde(default)]
    pub output: MatteGenerationOutput,
    /// Matte processing settings.
    #[serde(default)]
    pub settings: MatteGenerationSettings,
    /// Adapter-specific options that should remain opaque to the core protocol.
    #[serde(default)]
    pub options: BTreeMap<String, serde_json::Value>,
}

impl MatteGenerationRecipe {
    fn validate(&self, label: String) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.source.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("{label} generation source is empty"),
            ));
        }
        if self
            .model
            .as_ref()
            .map(|value| value.trim().is_empty())
            .unwrap_or(false)
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("{label} generation model is empty"),
            ));
        }
        diagnostics.extend(self.settings.validate(label.clone(), self.output));
        for key in self.options.keys() {
            if key.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::TrackingMasksMattes,
                    format!("{label} generation option key is empty"),
                ));
            }
        }
        diagnostics
    }
}

/// Output kind produced by a matte generation recipe.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatteGenerationOutput {
    /// Binary or grayscale mask only.
    MaskOnly,
    /// Alpha matte/cutout with transparency.
    #[default]
    AlphaMatte,
    /// Foreground cutout image.
    Cutout,
    /// Foreground composited over a replacement background.
    BackgroundReplaced,
}

/// Fallback used when the preferred matte generation mode fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatteGenerationFallback {
    /// Use a simple binary cutout.
    SimpleCutout,
    /// Apply the mask directly as alpha.
    PutAlpha,
    /// Keep the original frame without a usable matte.
    OriginalFrame,
}

/// Processing settings for a generated matte.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatteGenerationSettings {
    /// Whether alpha matting was requested.
    #[serde(default)]
    pub alpha_matting: bool,
    /// Foreground threshold in 0..=255.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub foreground_threshold: Option<u16>,
    /// Background threshold in 0..=255.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub background_threshold: Option<u16>,
    /// Erosion structure size in pixels.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub erode_size: Option<u32>,
    /// Whether mask post-processing was requested.
    #[serde(default)]
    pub post_process_mask: bool,
    /// Optional replacement background color as RGBA values in 0..=255.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub background_color_rgba: Option<[u16; 4]>,
    /// Fallback behavior when the preferred mode fails.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fallback: Option<MatteGenerationFallback>,
}

impl MatteGenerationSettings {
    fn validate(
        &self,
        label: String,
        output: MatteGenerationOutput,
    ) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        validate_optional_byte_value(
            &mut diagnostics,
            self.foreground_threshold,
            format!("{label} generation foreground threshold"),
        );
        validate_optional_byte_value(
            &mut diagnostics,
            self.background_threshold,
            format!("{label} generation background threshold"),
        );
        if let Some(erode_size) = self.erode_size
            && erode_size == 0
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("{label} generation erode size must be greater than zero"),
            ));
        }
        if let Some(color) = self.background_color_rgba
            && color.iter().any(|value| *value > 255)
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::TrackingMasksMattes,
                format!("{label} generation background color must be in 0..=255"),
            ));
        }
        if self.fallback == Some(MatteGenerationFallback::OriginalFrame)
            && matches!(
                output,
                MatteGenerationOutput::MaskOnly | MatteGenerationOutput::AlphaMatte
            )
        {
            diagnostics.push(ProfessionalDiagnostic::warning(
                CapabilityArea::TrackingMasksMattes,
                format!("{label} generation fallback cannot preserve a usable matte"),
            ));
        }
        diagnostics
    }
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

/// Master-bus two-pass loudnorm settings.
///
/// When present on the timeline-level render spec, the orchestrator runs
/// the EBU-compliant FFmpeg two-pass workflow: measure first with
/// `print_format=json`, parse the measured values off stderr, then apply
/// with those values plugged into the master audio filter chain.
///
/// Single-pass `loudnorm` (via `LoudnessTargetPlan` / `AudioFxPlan`)
/// remains the default; this struct is purely additive.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MasterLoudnorm {
    /// Integrated loudness target in LUFS (negative).
    pub target_lufs: f32,
    /// Loudness range target in LU.
    pub target_lra: f32,
    /// True peak ceiling in dBTP (negative or zero).
    pub target_tp: f32,
}

impl MasterLoudnorm {
    /// EBU R128 broadcast preset: -23 LUFS, 7 LU range, -1 dBTP ceiling.
    pub const EBU_R128: Self = Self {
        target_lufs: -23.0,
        target_lra: 7.0,
        target_tp: -1.0,
    };

    /// Streaming preset: -16 LUFS, 11 LU range, -1.5 dBTP ceiling.
    pub const STREAMING: Self = Self {
        target_lufs: -16.0,
        target_lra: 11.0,
        target_tp: -1.5,
    };
}

/// Explicit procedural link from a source parameter/signal to a target parameter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ExpressionLink {
    /// Stable link id.
    pub id: String,
    /// Target clip id.
    pub target_clip_id: String,
    /// Target parameter path.
    pub target_parameter: String,
    /// Source parameter or analysis signal.
    #[serde(default)]
    pub source: ExpressionSource,
    /// Deterministic expression subset.
    pub expression: String,
    /// Optional output clamp.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub clamp: Option<ExpressionClamp>,
    /// Enabled flag.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl ExpressionLink {
    /// Validate expression link shape and supported signal references.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                "expression link has an empty id",
            ));
        }
        if self.target_clip_id.trim().is_empty() || self.target_parameter.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                format!("expression link {} has an incomplete target", self.id),
            ));
        }
        if self.expression.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                format!("expression link {} has an empty expression", self.id),
            ));
        }
        match &self.source {
            ExpressionSource::Unset => diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                format!("expression link {} has no source", self.id),
            )),
            ExpressionSource::Parameter { clip_id, parameter } => {
                if clip_id.trim().is_empty() || parameter.trim().is_empty() {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::ParameterAnimation,
                        format!(
                            "expression link {} has an incomplete parameter source",
                            self.id
                        ),
                    ));
                }
            }
            ExpressionSource::Signal { signal } => {
                if !SUPPORTED_EXPRESSION_SIGNALS.contains(&signal.as_str()) {
                    diagnostics.push(ProfessionalDiagnostic::warning(
                        CapabilityArea::ParameterAnimation,
                        format!(
                            "expression link {} references unsupported signal {}",
                            self.id, signal
                        ),
                    ));
                }
            }
        }
        if let Some(clamp) = self.clamp
            && (!clamp.min.is_finite() || !clamp.max.is_finite() || clamp.min > clamp.max)
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::ParameterAnimation,
                format!("expression link {} has an invalid clamp", self.id),
            ));
        }
        diagnostics
    }
}

const SUPPORTED_EXPRESSION_SIGNALS: &[&str] = &[
    "audio_energy",
    "beat_markers",
    "motion_magnitude",
    "speaker_emphasis",
    "cut_proximity",
];

/// Expression source.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExpressionSource {
    /// No source.
    #[default]
    Unset,
    /// Another clip parameter.
    Parameter {
        /// Clip id.
        clip_id: String,
        /// Parameter path.
        parameter: String,
    },
    /// An analyzed signal such as audio energy or beat markers.
    Signal {
        /// Signal id.
        signal: String,
    },
}

/// Optional expression output clamp.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ExpressionClamp {
    /// Minimum value.
    pub min: f64,
    /// Maximum value.
    pub max: f64,
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
        if self.preflight_checks.contains(&PreflightCheckKind::Bitrate) {
            match (input.video_bitrate_kbps, self.video_bitrate_kbps) {
                (Some(actual), Some(target)) if actual < target / 2 => {
                    findings.push(PreflightFinding::warning(
                        PreflightCheckKind::Bitrate,
                        format!("bitrate {actual} kbps is far below target {target} kbps"),
                    ));
                }
                (Some(actual), Some(target)) if actual > target => {
                    findings.push(PreflightFinding::warning(
                        PreflightCheckKind::Bitrate,
                        format!("bitrate {actual} kbps is above target {target} kbps"),
                    ));
                }
                (None, Some(_)) => findings.push(PreflightFinding::error(
                    PreflightCheckKind::Bitrate,
                    "bitrate measurement is missing",
                )),
                _ => {}
            }
        }
        if self
            .preflight_checks
            .contains(&PreflightCheckKind::Loudness)
        {
            match (input.integrated_lufs, self.loudness_lufs) {
                (Some(actual), Some(target)) if (actual - target).abs() > 2.0 => {
                    findings.push(PreflightFinding::warning(
                        PreflightCheckKind::Loudness,
                        format!("loudness {actual:.1} LUFS is outside target {target:.1} LUFS"),
                    ));
                }
                (None, Some(_)) => findings.push(PreflightFinding::error(
                    PreflightCheckKind::Loudness,
                    "loudness measurement is missing",
                )),
                _ => {}
            }
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
        if self
            .preflight_checks
            .contains(&PreflightCheckKind::Duration)
        {
            match input.duration_s {
                Some(duration_s) if !duration_s.is_finite() || duration_s <= 0.0 => {
                    findings.push(PreflightFinding::error(
                        PreflightCheckKind::Duration,
                        "duration must be positive and finite",
                    ));
                }
                None => findings.push(PreflightFinding::error(
                    PreflightCheckKind::Duration,
                    "duration measurement is missing",
                )),
                _ => {}
            }
        }
        PreflightReport {
            id: format!("preflight-{}", self.id),
            profile_id: self.id.clone(),
            findings,
        }
    }
}

/// Reusable export preset for rendering and delivery planning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    /// Preset id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Export media mode.
    #[serde(default)]
    pub mode: ExportMode,
    /// Delivery profile that describes dimensions, aspect ratio, and checks.
    pub profile: DeliveryProfile,
    /// Container/output settings.
    #[serde(default)]
    pub output: ExportOutputSettings,
    /// Optional video settings. Required for video-carrying modes.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub video: Option<VideoExportSettings>,
    /// Optional audio settings. Required for audio-carrying modes.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub audio: Option<AudioExportSettings>,
    /// Optional export range.
    #[serde(default)]
    pub range: ExportRange,
}

impl ExportPreset {
    /// Vertical short-form social video preset.
    pub fn vertical_short_form() -> Self {
        Self {
            id: "vertical_short_form".into(),
            name: "Vertical Short Form".into(),
            mode: ExportMode::AudioVideo,
            profile: DeliveryProfile {
                id: "vertical_short_form".into(),
                name: "Vertical Short Form".into(),
                platform: Some("social".into()),
                aspect_ratio: "9:16".into(),
                width: 1080,
                height: 1920,
                video_bitrate_kbps: Some(12_000),
                loudness_lufs: Some(-14.0),
                preflight_checks: vec![
                    PreflightCheckKind::AspectRatio,
                    PreflightCheckKind::Bitrate,
                    PreflightCheckKind::Loudness,
                    PreflightCheckKind::SafeAreas,
                    PreflightCheckKind::Captions,
                    PreflightCheckKind::Metadata,
                ],
            },
            output: ExportOutputSettings::mp4(),
            video: Some(VideoExportSettings::h264(12_000)),
            audio: Some(AudioExportSettings::aac(192)),
            range: ExportRange::default(),
        }
    }

    /// Audio-only podcast preset.
    pub fn podcast_audio() -> Self {
        Self {
            id: "podcast_audio".into(),
            name: "Podcast Audio".into(),
            mode: ExportMode::AudioOnly,
            profile: DeliveryProfile {
                id: "podcast_audio".into(),
                name: "Podcast Audio".into(),
                platform: Some("podcast".into()),
                aspect_ratio: "audio".into(),
                width: 0,
                height: 0,
                video_bitrate_kbps: None,
                loudness_lufs: Some(-16.0),
                preflight_checks: vec![PreflightCheckKind::Loudness, PreflightCheckKind::Metadata],
            },
            output: ExportOutputSettings {
                extension: "m4a".into(),
                container: "ipod".into(),
                hardware_acceleration: HardwareAccelerationPolicy::Off,
            },
            video: None,
            audio: Some(AudioExportSettings::aac(128)),
            range: ExportRange::default(),
        }
    }

    /// Broadcast / archive HEVC (H.265) 1080p preset. CRF 23 / preset
    /// medium are baked into the FFmpeg argv lowering — the preset itself
    /// only declares the codec, container, and bitrate envelope.
    pub fn archival_hevc() -> Self {
        Self {
            id: "archival_hevc_1080p".into(),
            name: "Archival HEVC 1080p".into(),
            mode: ExportMode::AudioVideo,
            profile: DeliveryProfile {
                id: "archival_hevc_1080p".into(),
                name: "Archival HEVC 1080p".into(),
                platform: Some("archive".into()),
                aspect_ratio: "16:9".into(),
                width: 1920,
                height: 1080,
                video_bitrate_kbps: Some(10_000),
                loudness_lufs: Some(-23.0),
                preflight_checks: vec![
                    PreflightCheckKind::AspectRatio,
                    PreflightCheckKind::Bitrate,
                    PreflightCheckKind::Loudness,
                    PreflightCheckKind::Metadata,
                ],
            },
            output: ExportOutputSettings::mp4(),
            video: Some(VideoExportSettings::libx265(10_000)),
            audio: Some(AudioExportSettings::aac(192)),
            range: ExportRange::default(),
        }
    }

    /// Apple ProRes 422 HQ 1080p mastering preset. The MOV container and
    /// `yuv422p10le` 10-bit 4:2:2 pixel format are required by ProRes HQ
    /// (`-profile:v 3`); the FFmpeg argv lowering supplies those flags.
    pub fn archival_prores() -> Self {
        Self {
            id: "archival_prores_hq_1080p".into(),
            name: "Apple ProRes 422 HQ 1080p".into(),
            mode: ExportMode::AudioVideo,
            profile: DeliveryProfile {
                id: "archival_prores_hq_1080p".into(),
                name: "Apple ProRes 422 HQ 1080p".into(),
                platform: Some("mastering".into()),
                aspect_ratio: "16:9".into(),
                width: 1920,
                height: 1080,
                video_bitrate_kbps: None,
                loudness_lufs: Some(-23.0),
                preflight_checks: vec![
                    PreflightCheckKind::AspectRatio,
                    PreflightCheckKind::Loudness,
                    PreflightCheckKind::Metadata,
                ],
            },
            output: ExportOutputSettings {
                extension: "mov".into(),
                container: "mov".into(),
                hardware_acceleration: HardwareAccelerationPolicy::Off,
            },
            video: Some(VideoExportSettings::prores_ks()),
            audio: Some(AudioExportSettings {
                codec: "pcm_s24le".into(),
                bitrate_kbps: None,
                sample_rate_hz: 48_000,
                channels: 2,
            }),
            range: ExportRange::default(),
        }
    }

    /// Image sequence preset for frame-accurate review or external finishing.
    pub fn image_sequence() -> Self {
        Self {
            id: "image_sequence".into(),
            name: "Image Sequence".into(),
            mode: ExportMode::ImageSequence,
            profile: DeliveryProfile {
                id: "image_sequence".into(),
                name: "Image Sequence".into(),
                platform: Some("finishing".into()),
                aspect_ratio: "16:9".into(),
                width: 1920,
                height: 1080,
                video_bitrate_kbps: None,
                loudness_lufs: None,
                preflight_checks: vec![PreflightCheckKind::AspectRatio],
            },
            output: ExportOutputSettings {
                extension: "png".into(),
                container: "image2".into(),
                hardware_acceleration: HardwareAccelerationPolicy::Off,
            },
            video: Some(VideoExportSettings {
                codec: "png".into(),
                bitrate_kbps: None,
                frame_rate: None,
            }),
            audio: None,
            range: ExportRange::default(),
        }
    }

    /// Validate preset shape and mode-specific settings.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::DeliveryProfilesAndPreflight,
                "export preset has an empty id",
            ));
        }
        if self.name.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::DeliveryProfilesAndPreflight,
                format!("export preset {} has an empty name", self.id),
            ));
        }
        if self.output.container.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::DeliveryProfilesAndPreflight,
                format!("export preset {} has an empty container", self.id),
            ));
        }
        if self.output.extension.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::DeliveryProfilesAndPreflight,
                format!("export preset {} has an empty extension", self.id),
            ));
        }
        if self.mode.carries_video() && self.video.is_none() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::DeliveryProfilesAndPreflight,
                format!("export preset {} is missing video settings", self.id),
            ));
        }
        if self.mode.carries_audio() && self.audio.is_none() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::DeliveryProfilesAndPreflight,
                format!("export preset {} is missing audio settings", self.id),
            ));
        }
        if !self.mode.carries_video() && self.video.is_some() {
            diagnostics.push(ProfessionalDiagnostic::warning(
                CapabilityArea::DeliveryProfilesAndPreflight,
                format!("export preset {} ignores video settings", self.id),
            ));
        }
        if !self.mode.carries_audio() && self.audio.is_some() {
            diagnostics.push(ProfessionalDiagnostic::warning(
                CapabilityArea::DeliveryProfilesAndPreflight,
                format!("export preset {} ignores audio settings", self.id),
            ));
        }
        if let Some(video) = &self.video {
            if video.codec.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::DeliveryProfilesAndPreflight,
                    format!("export preset {} has an empty video codec", self.id),
                ));
            }
            if let Some(frame_rate) = video.frame_rate
                && (!frame_rate.is_finite() || frame_rate <= 0.0)
            {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::DeliveryProfilesAndPreflight,
                    format!("export preset {} frame rate must be positive", self.id),
                ));
            }
        }
        if let Some(audio) = &self.audio {
            if audio.codec.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::DeliveryProfilesAndPreflight,
                    format!("export preset {} has an empty audio codec", self.id),
                ));
            }
            if audio.sample_rate_hz == 0 {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::DeliveryProfilesAndPreflight,
                    format!(
                        "export preset {} audio sample rate must be positive",
                        self.id
                    ),
                ));
            }
            if audio.channels == 0 {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::DeliveryProfilesAndPreflight,
                    format!(
                        "export preset {} audio channel count must be positive",
                        self.id
                    ),
                ));
            }
        }
        if let (Some(start_s), Some(end_s)) = (self.range.start_s, self.range.end_s)
            && (!start_s.is_finite() || !end_s.is_finite() || end_s <= start_s)
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::DeliveryProfilesAndPreflight,
                format!(
                    "export preset {} end_s must be greater than start_s",
                    self.id
                ),
            ));
        }
        if self.mode.carries_video()
            && !profile_dimensions_match_aspect_ratio(
                self.profile.width,
                self.profile.height,
                &self.profile.aspect_ratio,
            )
        {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::DeliveryProfilesAndPreflight,
                format!(
                    "export preset {} dimensions do not match aspect ratio {}",
                    self.id, self.profile.aspect_ratio
                ),
            ));
        }
        diagnostics
    }
}

/// Export media mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportMode {
    /// Export video and audio.
    #[default]
    AudioVideo,
    /// Export video without audio.
    VideoOnly,
    /// Export audio without video.
    AudioOnly,
    /// Export one still image per frame.
    ImageSequence,
}

impl ExportMode {
    fn carries_video(self) -> bool {
        matches!(
            self,
            Self::AudioVideo | Self::VideoOnly | Self::ImageSequence
        )
    }

    fn carries_audio(self) -> bool {
        matches!(self, Self::AudioVideo | Self::AudioOnly)
    }
}

/// Source stream kind in a stream-level export contract.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// Video stream.
    #[default]
    Video,
    /// Audio stream.
    Audio,
    /// Subtitle stream.
    Subtitle,
    /// Data or attachment stream.
    Data,
}

/// Handling mode for one output stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamExportMode {
    /// Preserve source packets when the container supports the stream.
    #[default]
    Copy,
    /// Re-encode the stream with the requested codec.
    Transcode,
}

/// One mapped stream in a stream-level export contract.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamExportSpec {
    /// Stable stream id for diagnostics and planner output.
    pub id: String,
    /// Media kind of the source stream.
    #[serde(default)]
    pub kind: StreamKind,
    /// Zero-based source stream index as reported by the source container.
    pub source_index: u32,
    /// Copy or transcode handling.
    #[serde(default)]
    pub mode: StreamExportMode,
    /// Required when transcoding.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub codec: Option<String>,
    /// Optional BCP-47/ISO language tag for the output stream.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub language: Option<String>,
    /// FFmpeg-compatible disposition flags.
    #[serde(default)]
    pub disposition: Vec<String>,
    /// Additional stream metadata.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

/// Durable stream-level export intent for stream copy/transcode planning.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamExportContract {
    /// Stable contract id.
    pub id: String,
    /// Output container or muxer name.
    pub container: String,
    /// Ordered output stream mappings.
    #[serde(default)]
    pub streams: Vec<StreamExportSpec>,
    /// Global output metadata.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl StreamExportContract {
    /// Validate stream mapping, codec, disposition, and metadata shape.
    pub fn validate(&self) -> Vec<ProfessionalDiagnostic> {
        let mut diagnostics = Vec::new();
        if self.id.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::DeliveryProfilesAndPreflight,
                "stream export contract has an empty id",
            ));
        }
        if self.container.trim().is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::DeliveryProfilesAndPreflight,
                format!("stream export contract {} has an empty container", self.id),
            ));
        }
        if self.streams.is_empty() {
            diagnostics.push(ProfessionalDiagnostic::error(
                CapabilityArea::DeliveryProfilesAndPreflight,
                format!("stream export contract {} has no streams", self.id),
            ));
        }

        let mut stream_ids = HashSet::new();
        for stream in &self.streams {
            let stream_label = stream_label(stream);
            if stream.id.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::DeliveryProfilesAndPreflight,
                    "stream export contract has a stream with an empty id",
                ));
            } else if !stream_ids.insert(stream.id.as_str()) {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::DeliveryProfilesAndPreflight,
                    format!(
                        "stream export contract {} duplicates stream id {}",
                        self.id, stream.id
                    ),
                ));
            }
            if matches!(stream.mode, StreamExportMode::Transcode)
                && stream
                    .codec
                    .as_ref()
                    .is_none_or(|codec| codec.trim().is_empty())
            {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::DeliveryProfilesAndPreflight,
                    format!("transcode stream {stream_label} needs codec"),
                ));
            }
            if let Some(language) = &stream.language
                && language.trim().is_empty()
            {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::DeliveryProfilesAndPreflight,
                    format!("stream {stream_label} has an empty language tag"),
                ));
            }
            for disposition in &stream.disposition {
                if !is_supported_stream_disposition(disposition) {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::DeliveryProfilesAndPreflight,
                        format!("stream {stream_label} has unsupported disposition {disposition}"),
                    ));
                }
            }
            for key in stream.metadata.keys() {
                if key.trim().is_empty() {
                    diagnostics.push(ProfessionalDiagnostic::error(
                        CapabilityArea::DeliveryProfilesAndPreflight,
                        format!("stream {stream_label} has an empty metadata key"),
                    ));
                }
            }
        }
        for key in self.metadata.keys() {
            if key.trim().is_empty() {
                diagnostics.push(ProfessionalDiagnostic::error(
                    CapabilityArea::DeliveryProfilesAndPreflight,
                    format!(
                        "stream export contract {} has an empty metadata key",
                        self.id
                    ),
                ));
            }
        }
        diagnostics
    }
}

fn stream_label(stream: &StreamExportSpec) -> &str {
    if stream.id.trim().is_empty() {
        "<unnamed>"
    } else {
        stream.id.as_str()
    }
}

fn is_supported_stream_disposition(disposition: &str) -> bool {
    matches!(
        disposition,
        "default"
            | "dub"
            | "original"
            | "comment"
            | "lyrics"
            | "karaoke"
            | "forced"
            | "hearing_impaired"
            | "visual_impaired"
    )
}

/// Container and file-output settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportOutputSettings {
    /// File extension without dot.
    pub extension: String,
    /// FFmpeg container/muxer name.
    pub container: String,
    /// Hardware acceleration preference.
    #[serde(default)]
    pub hardware_acceleration: HardwareAccelerationPolicy,
}

impl Default for ExportOutputSettings {
    fn default() -> Self {
        Self::mp4()
    }
}

impl ExportOutputSettings {
    /// H.264/AAC MP4 container settings.
    pub fn mp4() -> Self {
        Self {
            extension: "mp4".into(),
            container: "mp4".into(),
            hardware_acceleration: HardwareAccelerationPolicy::Off,
        }
    }
}

/// Hardware acceleration preference for export.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareAccelerationPolicy {
    /// Do not request hardware acceleration.
    #[default]
    Off,
    /// Use hardware acceleration if available and fall back to software.
    Auto,
    /// Require hardware acceleration; fail if unavailable.
    Require,
}

/// Video codec settings for an export preset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoExportSettings {
    /// FFmpeg video codec name.
    pub codec: String,
    /// Target video bitrate in kbps.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bitrate_kbps: Option<u32>,
    /// Optional frame rate override.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub frame_rate: Option<f64>,
}

impl VideoExportSettings {
    /// H.264 video settings.
    pub fn h264(bitrate_kbps: u32) -> Self {
        Self {
            codec: "libx264".into(),
            bitrate_kbps: Some(bitrate_kbps),
            frame_rate: None,
        }
    }

    /// HEVC (H.265) video settings. The export preset wiring contributes
    /// the CRF / preset flags; this constructor only declares the codec
    /// identity and target bitrate.
    pub fn libx265(bitrate_kbps: u32) -> Self {
        Self {
            codec: "libx265".into(),
            bitrate_kbps: Some(bitrate_kbps),
            frame_rate: None,
        }
    }

    /// Apple ProRes (via FFmpeg's `prores_ks` encoder). ProRes is a
    /// constant-quality intra codec — bitrate is implied by the profile,
    /// so no bitrate is recorded here.
    pub fn prores_ks() -> Self {
        Self {
            codec: "prores_ks".into(),
            bitrate_kbps: None,
            frame_rate: None,
        }
    }

    // TODO(overnight-wave1-export-codecs): add DNxHR (`dnxhd`) and AV1
    // (`libsvtav1` / `libaom-av1`) constructors once the broader preset
    // catalog needs them — current MVP slice intentionally covers only
    // HEVC + ProRes.
}

/// Audio codec settings for an export preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioExportSettings {
    /// FFmpeg audio codec name.
    pub codec: String,
    /// Audio bitrate in kbps.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub bitrate_kbps: Option<u32>,
    /// Audio sample rate in Hz.
    pub sample_rate_hz: u32,
    /// Channel count.
    pub channels: u8,
}

impl AudioExportSettings {
    /// AAC audio settings at 48 kHz stereo.
    pub fn aac(bitrate_kbps: u32) -> Self {
        Self {
            codec: "aac".into(),
            bitrate_kbps: Some(bitrate_kbps),
            sample_rate_hz: 48_000,
            channels: 2,
        }
    }
}

/// Export range and timeline trimming controls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ExportRange {
    /// Optional range start in timeline seconds.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub start_s: Option<f64>,
    /// Optional range end in timeline seconds.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub end_s: Option<f64>,
    /// Let planner choose the first frame containing media.
    #[serde(default)]
    pub start_at_first_clip: bool,
    /// Let planner choose the last frame containing media.
    #[serde(default)]
    pub end_at_last_clip: bool,
}

fn profile_dimensions_match_aspect_ratio(width: u32, height: u32, aspect_ratio: &str) -> bool {
    if width == 0 && height == 0 && aspect_ratio == "audio" {
        return true;
    }
    let Some((ratio_width, ratio_height)) = parse_ratio(aspect_ratio) else {
        return false;
    };
    width > 0 && height > 0 && u64::from(width) * ratio_height == u64::from(height) * ratio_width
}

fn parse_ratio(value: &str) -> Option<(u64, u64)> {
    let (left, right) = value.split_once(':')?;
    let width = left.parse::<u64>().ok()?;
    let height = right.parse::<u64>().ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
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
    } else if let Some(confidence) = confidence
        && confidence < 0.5
    {
        diagnostics.push(ProfessionalDiagnostic::warning(
            area,
            format!("{label} low confidence may drift"),
        ));
    }
}

fn validate_optional_unit_interval(
    diagnostics: &mut Vec<ProfessionalDiagnostic>,
    value: Option<f64>,
    label: String,
) {
    if let Some(value) = value
        && (!value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        diagnostics.push(ProfessionalDiagnostic::error(
            CapabilityArea::TrackingMasksMattes,
            format!("{label} must be in 0..=1"),
        ));
    }
}

fn validate_optional_byte_value(
    diagnostics: &mut Vec<ProfessionalDiagnostic>,
    value: Option<u16>,
    label: String,
) {
    if let Some(value) = value
        && value > 255
    {
        diagnostics.push(ProfessionalDiagnostic::error(
            CapabilityArea::TrackingMasksMattes,
            format!("{label} must be in 0..=255"),
        ));
    }
}

fn default_one() -> f64 {
    1.0
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic_messages(catalog: AssetCatalog) -> Vec<String> {
        catalog
            .validate()
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    #[test]
    fn runtime_effect_parameter_aliases_are_executable() {
        assert!(is_runtime_clip_parameter(
            "effects.awidat.blur.params.radius_px"
        ));
        assert!(is_runtime_clip_parameter(
            "effects.awidat.shake.params.intensity_px"
        ));
        assert!(is_runtime_clip_parameter(
            "effects.awidat.shake.params.frequency_hz"
        ));
        assert!(is_runtime_clip_parameter("effects.awidat.warp.params.k1"));
        assert!(is_runtime_clip_parameter(
            "effects.awidat.warp.params.center_x"
        ));

        assert_eq!(
            canonical_runtime_clip_parameter("effects.awidat.blur.params.radius_px"),
            Some("awidat.blur.radius_px")
        );
        assert_eq!(
            canonical_runtime_clip_parameter("effects.awidat.warp.params.center_y"),
            Some("awidat.warp.center_y")
        );
        assert_eq!(
            canonical_runtime_clip_parameter("effects.unknown.params.amount"),
            None
        );
    }

    #[test]
    fn asset_catalog_rejects_duplicate_bin_ids() {
        let messages = diagnostic_messages(AssetCatalog {
            bins: vec![
                AssetBin {
                    id: "scene-1".into(),
                    name: "Scene 1".into(),
                    parent_id: None,
                },
                AssetBin {
                    id: "scene-1".into(),
                    name: "Duplicate".into(),
                    parent_id: None,
                },
            ],
            ..AssetCatalog::default()
        });

        assert!(
            messages
                .iter()
                .any(|message| { message == "asset catalog contains duplicate bin id scene-1" })
        );
    }

    #[test]
    fn runtime_effect_parameter_aliases_use_existing_value_validation() {
        let animation = ParameterAnimation {
            id: "bad-blur".to_string(),
            target: AnimationTarget::ClipParameter {
                clip_id: "clip-a".to_string(),
                parameter: "effects.awidat.blur.params.radius_px".to_string(),
            },
            keyframes: vec![Keyframe::linear(0.0, -1.0)],
            pre_extrapolation: ExtrapolationMode::Hold,
            post_extrapolation: ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: None,
        };

        let diagnostics = animation.validate();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("must be non-negative")),
            "expected blur alias to share canonical blur validation, got {diagnostics:?}"
        );
    }

    #[test]
    fn asset_catalog_rejects_missing_bin_parent() {
        let messages = diagnostic_messages(AssetCatalog {
            bins: vec![AssetBin {
                id: "takes".into(),
                name: "Takes".into(),
                parent_id: Some("missing".into()),
            }],
            ..AssetCatalog::default()
        });

        assert!(
            messages
                .iter()
                .any(|message| { message == "bin takes references missing parent missing" })
        );
    }

    #[test]
    fn asset_catalog_rejects_self_parent_bin() {
        let messages = diagnostic_messages(AssetCatalog {
            bins: vec![AssetBin {
                id: "takes".into(),
                name: "Takes".into(),
                parent_id: Some("takes".into()),
            }],
            ..AssetCatalog::default()
        });

        assert!(
            messages
                .iter()
                .any(|message| message == "bin takes cannot be its own parent")
        );
    }

    #[test]
    fn asset_catalog_rejects_bin_parent_cycles() {
        let messages = diagnostic_messages(AssetCatalog {
            bins: vec![
                AssetBin {
                    id: "a".into(),
                    name: "A".into(),
                    parent_id: Some("b".into()),
                },
                AssetBin {
                    id: "b".into(),
                    name: "B".into(),
                    parent_id: Some("a".into()),
                },
            ],
            ..AssetCatalog::default()
        });

        assert!(messages.iter().any(|message| {
            message == "bin a is part of a parent cycle"
                || message == "bin b is part of a parent cycle"
        }));
    }
}
