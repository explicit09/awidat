//! Typed `EdlOp` shape — what the parser emits and what
//! `apply_to_timeline` consumes.
//!
//! Each variant identifies its target by an [`Anchor`] (transcript
//! snippet, clip uuid, or scene-change index — content, not absolute
//! timestamps; per `PLAN.md` §6.2 property 1).

use std::collections::BTreeMap;
use std::fmt;

use awidat_proto::awidat_meta::{BroadcastOverlayConfig, SemanticCutSpec, SplitEditSpec};
use awidat_proto::professional::{
    AssetCatalog, AudioFinishingState, ColorFinishingState, CompositionGraph, DeliveryProfile,
    MotionGraphicsTemplate, ParameterAnimation, PipelineReadinessReport, PlannerPassContract,
    PreflightReport, ProposalPackage, ReframeSmoothing, SourceRange, SourceSelect, Stringout,
    TrackingPackage, WorkflowLens,
};
use awidat_proto::transitions::SemanticTransitionSpec;
use serde::{Deserialize, Serialize};

/// One change in an EDL envelope.
///
/// Uses `op` as the discriminator tag (not `kind`) because
/// `InsertTransition` has a `kind: String` field for the SMPTE name
/// which would collide with serde's internal-tag injection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EdlOp {
    /// Trim a clip's source range. `start` / `end` are absolute seconds
    /// into the clip's media.
    TrimClip {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// New start (seconds into the source media), if changing.
        start: Option<f64>,
        /// New end (seconds into the source media), if changing.
        end: Option<f64>,
    },
    /// Remove a clip outright. Subsequent clips don't shift — a Gap of
    /// the deleted clip's duration takes its place. (Use `Move Clip` to
    /// re-order; the model shouldn't rely on Delete-then-collapse.)
    DeleteClip {
        /// Anchor identifying the clip.
        anchor: Anchor,
    },
    /// Split one clip into two at a timestamp. The original clip's
    /// `source_range` partitions: `[start..at_s)` becomes the left
    /// piece (keeps the original name + metadata), `[at_s..end)`
    /// becomes a new right piece (new name `<original>-b`). Both
    /// pieces share the same media reference.
    ///
    /// `at_s` is in seconds **into the source media**, not absolute
    /// timeline seconds. The agent's typical flow is: find_moment to
    /// get a transcript timestamp → that timestamp is already into
    /// the source → pass it as `at_s`.
    SplitClip {
        /// Anchor identifying the clip to split.
        anchor: Anchor,
        /// Cut point, in seconds into the source media. Must lie
        /// strictly inside the clip's source_range or the op fails.
        at_s: f64,
    },
    /// Reset / extend a previously-trimmed clip's source range. This
    /// is the inverse-direction op of `Trim Clip` — Trim can only
    /// narrow; Untrim can widen back out (toward the original media
    /// bounds). Required because the agent's most common failure mode
    /// is overshoot-then-recover: trim too aggressively, realize the
    /// kept content extends further, need to widen the source range
    /// before splitting.
    ///
    /// Both fields are optional; omitted fields keep the current
    /// source-range value. If the media reference declares an
    /// `available_range`, the new range is capped to it; otherwise
    /// the agent is trusted (OTIO round-trip validation catches the
    /// pathological case).
    UntrimClip {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// New source-range start in seconds, if widening backward.
        /// `None` keeps the current start.
        start: Option<f64>,
        /// New source-range end in seconds, if widening forward.
        /// `None` keeps the current end.
        end: Option<f64>,
    },
    /// Insert a fresh clip on a track from an asset on disk. The
    /// load-bearing op for *building* a timeline from raw assets —
    /// previous ops only mutate existing clips. Without this the
    /// agent has to bash-edit the OTIO file to start a project.
    ///
    /// Track is identified by name; if the named track doesn't exist
    /// the op creates it. New-track kind defaults to video for
    /// compatibility, can be forced via `track_kind`, or inferred
    /// from A*/Audio* names when `track_kind = Auto`. The new clip's media_reference is an
    /// `ExternalReference` to the asset; the source_range defaults
    /// to `[0, available_range.duration)` if the asset declares an
    /// available range, otherwise the agent must specify `start`/`end`.
    ///
    /// `at_position` controls insertion order in the track's children
    /// vec. Default = append (end of track).
    ///
    /// New clip's `name` defaults to `clip-N` where N is the new
    /// child's index. The model can override via the optional `name`
    /// field.
    InsertClip {
        /// Project-relative asset path, e.g. `"raw/clip-1.MOV"`.
        asset: String,
        /// Track name. Existing track kind is preserved.
        track: String,
        /// Optional new-track kind hint. Ignored for existing tracks.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        track_kind: Option<InsertTrackKind>,
        /// Where in the track to insert. `None` = append.
        at_position: Option<usize>,
        /// Source-media start in seconds. Defaults to 0.
        start: Option<f64>,
        /// Source-media end in seconds. Defaults to the asset's
        /// available_range end (if known) or to start+1 (the agent
        /// is expected to know the duration via inspect_clip /
        /// list_assets first).
        end: Option<f64>,
        /// Optional clip name override.
        name: Option<String>,
        /// Optional linkage id shared by related video/audio clips
        /// imported from the same source asset.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        link_group_id: Option<String>,
    },
    /// Insert b-roll over an anchor moment. Currently F2; carried in the
    /// type so the parser/handler signatures stay stable.
    InsertBRoll {
        /// Where to insert.
        anchor: Anchor,
        /// Asset path (project-relative).
        asset: String,
        /// Duration in seconds.
        duration_s: f64,
        /// Where it sits relative to the existing clips.
        position: BRollPosition,
    },
    /// Insert a picture-in-picture clip over an anchor moment.
    InsertPiP {
        /// Where to insert.
        anchor: Anchor,
        /// Asset path (project-relative).
        asset: String,
        /// Duration in seconds.
        duration_s: f64,
        /// Source-media start in seconds.
        source_start_s: f64,
        /// Output corner.
        corner: PiPCorner,
        /// Fraction of output width.
        scale: f64,
        /// Fraction of output width/height used as the margin.
        margin_pct: f64,
    },
    /// Move a clip to a new track position. F2.
    MoveClip {
        /// Source anchor.
        anchor: Anchor,
        /// 0-based index in the destination track's children.
        to_position: usize,
        /// Optional absolute timeline start time on the same track.
        /// When set, gaps are split/created so the clip starts here.
        at_s: Option<f64>,
    },
    /// Insert a transition between two adjacent clips. F2.
    InsertTransition {
        /// Anchor pair: the transition lands between these two anchors.
        between: TransitionBetween,
        /// Transition kind, e.g. `"SMPTE_Dissolve"`.
        kind: String,
        /// Duration in seconds.
        duration_s: f64,
        /// Transition alignment when explicit offsets are not supplied.
        /// `None` means centered on the cut.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        alignment: Option<TransitionAlignment>,
        /// Amount of the incoming clip used before the cut, in seconds.
        /// When set, this overrides `alignment`.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        in_offset_s: Option<f64>,
        /// Amount of the outgoing clip used after the cut, in seconds.
        /// When set, this overrides `alignment`.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        out_offset_s: Option<f64>,
        /// Optional Awidat semantic transition metadata. When present,
        /// this is persisted on the OTIO transition's metadata so the
        /// agent's editorial intent survives render/export round trips.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        spec: Option<SemanticTransitionSpec>,
    },
    /// Remove an existing transition between two adjacent clips.
    DeleteTransition {
        /// Anchor pair identifying the two clips around the transition.
        between: TransitionBetween,
    },
    /// Store semantic editorial intent for a hard-cut boundary without
    /// inserting a visible transition.
    SetCutIntent {
        /// Anchor pair identifying the outgoing and incoming clips.
        between: TransitionBetween,
        /// Cut grammar, purpose, audio relationship, and explanation.
        spec: SemanticCutSpec,
    },
    /// Set per-clip audio volume. `value` is a linear gain multiplier
    /// where `0.0` mutes the clip, `1.0` is unity (no change), and
    /// values above 1.0 amplify (clipping risk). The apply layer
    /// stamps an `awidat.volume` Effect on the clip; render emits
    /// `volume=<value>` on that segment's audio stream before concat.
    /// Re-applying replaces the existing effect rather than stacking.
    SetVolume {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// Linear gain multiplier. Must be finite and `>= 0.0`.
        value: f64,
    },
    /// Set per-clip audio fades in seconds. Replaces the existing
    /// `awidat.audio_fade` effect on the clip.
    SetAudioFade {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// Fade-in duration from the clip start, seconds.
        fade_in_s: Option<f64>,
        /// Fade-out duration into the clip end, seconds.
        fade_out_s: Option<f64>,
    },
    /// Mark an incoming clip as a J-cut: its audio should lead picture.
    SetAudioLead {
        /// Anchor identifying the incoming clip.
        anchor: Anchor,
        /// Lead duration in seconds.
        lead_s: f64,
        /// Optional reason/confidence for the split edit.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        split_edit: Option<SplitEditSpec>,
    },
    /// Mark an outgoing clip as an L-cut: its audio should trail picture.
    SetAudioTrail {
        /// Anchor identifying the outgoing clip.
        anchor: Anchor,
        /// Trail duration in seconds.
        trail_s: f64,
        /// Optional reason/confidence for the split edit.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        split_edit: Option<SplitEditSpec>,
    },
    /// Set audio controls on a track by name.
    SetTrackAudio {
        /// Track name to update.
        track: String,
        /// Optional role: dialogue, music, or sfx.
        role: Option<String>,
        /// Optional linear track gain.
        volume: Option<f64>,
        /// Optional mute flag.
        muted: Option<bool>,
        /// Optional solo flag.
        solo: Option<bool>,
    },
    /// Configure ducking on an audio track by name. Ducking is applied
    /// against dialogue tracks during render.
    SetDucking {
        /// Track name to update.
        track: String,
        /// Enable/disable ducking. Defaults to true when omitted.
        enabled: Option<bool>,
        /// Gain reduction in dB. Defaults to -12.
        amount_db: Option<f64>,
        /// Attack in milliseconds. Defaults to 80.
        attack_ms: Option<f64>,
        /// Release in milliseconds. Defaults to 300.
        release_ms: Option<f64>,
    },
    /// Store sync metadata on a clip and optionally align it on its
    /// current track. Positive offsets place the clip later on the
    /// timeline; negative offsets trim the source range earlier clips
    /// cannot reach and place the clip at the track start. Small drift
    /// corrections are carried as a speed factor.
    SetSyncGroup {
        /// Anchor identifying the clip/source to align.
        anchor: Anchor,
        /// Stable group id shared by synced camera/audio assets.
        sync_group_id: String,
        /// Timeline offset in seconds relative to the reference asset.
        offset_s: f64,
        /// Optional stable drift correction. `1.0` means no drift.
        speed_factor: Option<f64>,
        /// Tool confidence in [0, 1].
        confidence: Option<f64>,
    },
    /// Set FFmpeg-native audio repair/effect parameters on a clip.
    SetClipAudioFx {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// Effect configuration.
        fx: AudioFxConfig,
    },
    /// Set FFmpeg-native audio repair/effect parameters on a track.
    SetTrackAudioFx {
        /// Track name to update.
        track: String,
        /// Effect configuration.
        fx: AudioFxConfig,
    },
    /// Set a generic graph-native Awidat effect on a clip. The `effect`
    /// id must be registered in `awidat-effects`; `params` are
    /// validated against that registry before the OTIO `Effect.1` is
    /// stamped. Re-applying a non-stackable effect replaces the prior
    /// same-id effect.
    SetEffect {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// Stable effect id, e.g. `awidat.color_correction`.
        effect: String,
        /// Raw effect parameters from `params_json`.
        #[serde(default)]
        params: serde_json::Map<String, serde_json::Value>,
        /// Optional editorial rationale persisted in metadata.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        rationale: Option<String>,
    },
    /// Set per-clip playback speed. `factor` rescales both video and
    /// audio: `2.0` plays at double speed (half the timeline length),
    /// `0.5` plays at half speed (double the timeline length). `1.0`
    /// is unity. Render emits `setpts=<1/factor>*PTS` on video and
    /// chains `atempo=` filters on audio (atempo's per-instance
    /// range is `[0.5, 2.0]`; factors outside that chain). The
    /// segment's contribution to the master timeline duration is
    /// `source_duration / factor`.
    SetSpeed {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// Speed multiplier. Must be finite and `> 0.0`.
        factor: f64,
    },
    /// Set a clip-level time remap curve. The apply layer stamps an
    /// `awidat.time_remap` Effect on the clip; render can then lower the
    /// keyed curve into retiming filters. Re-applying replaces the prior
    /// time remap.
    SetTimeRemap {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// Ordered curve points from `curve_json`.
        curve: Vec<serde_json::Value>,
    },
    /// Insert a held frame at a clip-local source time. The apply layer
    /// stamps an `awidat.freeze` Effect on the clip and render splices
    /// the hold into the segment.
    SetFreeze {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// Source time, in seconds, where the frame is held.
        freeze_at_source_s: f64,
        /// Hold duration inserted into the timeline.
        duration_s: f64,
        /// Placement mode. Currently `at`.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        freeze_position: Option<String>,
        /// Audio fill behavior for the inserted hold. Currently `silence`.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        audio_behavior: Option<String>,
    },
    /// Set clip-level color correction controls. The apply layer stamps
    /// an `awidat.color_correction` Effect on the clip; render converts
    /// those normalized controls into FFmpeg video filters before speed,
    /// titles, and concat. Re-applying replaces the prior correction.
    SetColorCorrection {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// Exposure offset in stops, typically `[-4.0, 4.0]`.
        exposure_ev: Option<f64>,
        /// Contrast multiplier, where `1.0` is unchanged.
        contrast: Option<f64>,
        /// Saturation multiplier, where `1.0` is unchanged.
        saturation: Option<f64>,
        /// Normalized warm/cool control, where positive warms.
        temperature: Option<f64>,
        /// Normalized green/magenta control, where positive magentas.
        tint: Option<f64>,
        /// Normalized shadow lift/drop control.
        shadows: Option<f64>,
        /// Normalized highlight recovery/boost control.
        highlights: Option<f64>,
    },
    /// Apply a 3D LUT to a clip. The LUT path is project-relative and
    /// stored as an `awidat.lut` Effect so render can emit `lut3d`.
    /// Re-applying replaces the prior LUT, making looks mutable.
    ApplyLut {
        /// Anchor identifying the clip.
        anchor: Anchor,
        /// Project-relative LUT path. Must not be absolute or contain `..`.
        lut_path: String,
        /// Optional FFmpeg `lut3d` interpolation mode.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        interpolation: Option<String>,
        /// Optional blend strength in `0.0..=1.0`. `None` or `1.0`
        /// applies the LUT at full intensity (single `lut3d` filter).
        /// Anything below 1.0 lowers the render to a
        /// `split → lut3d → blend=all_opacity` pattern so the LUT
        /// mixes with the un-graded source.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        strength: Option<f64>,
    },
    /// Remove the clip-level LUT effect, leaving other effects intact.
    RemoveLut {
        /// Anchor identifying the clip.
        anchor: Anchor,
    },
    /// Insert a title overlay on the project's "Titles" track. The
    /// titles track auto-creates on first insert (Video kind, flagged
    /// via metadata.awidat.extra["track_role"] = "titles") so the
    /// render pipeline can route these clips into `drawtext` filters
    /// instead of trying to decode them as media.
    ///
    /// `start_s` / `end_s` are absolute timeline-time seconds — when
    /// the overlay should appear and disappear over the underlying
    /// composition. The synthesized title clip carries an
    /// `awidat.title` Effect whose metadata holds all the styling
    /// fields below.
    InsertTitle {
        /// Title appears at this timeline-time, in seconds.
        start_s: f64,
        /// Title disappears at this timeline-time, in seconds.
        /// Must be `> start_s`.
        end_s: f64,
        /// Text to display.
        text: String,
        /// Vertical band on the frame.
        position: TitlePosition,
        /// Font size in pixels (rendered against a 1080p reference
        /// frame; ffmpeg scales proportionally).
        font_size: u32,
        /// Hex colour string like `"#FFFFFF"`. Validation deferred
        /// to ffmpeg's `drawtext` filter at render time.
        color: String,
        /// Bold vs normal weight.
        font_weight: TitleWeight,
        /// Entry / exit animation.
        animation: TitleAnimation,
    },
    /// Insert a title with multiple inline style runs. This keeps the
    /// common single-string title path simple while giving agents a
    /// compact operation for emphasis words and phrase color changes.
    InsertRichTitle {
        /// Title appears at this timeline-time, in seconds.
        start_s: f64,
        /// Title disappears at this timeline-time, in seconds.
        end_s: f64,
        /// Inline styled text runs.
        segments: Vec<RichTextSegment>,
        /// Vertical band on the frame.
        position: TitlePosition,
        /// Font size in pixels.
        font_size: u32,
        /// Entry / exit animation.
        animation: TitleAnimation,
    },
    /// Instantiate a reusable motion graphics template into concrete title
    /// clips and runtime parameter animations. This is the agent-facing
    /// lowering path for first-class graphics templates such as lower thirds:
    /// the agent fills named slots instead of hand-authoring text boxes,
    /// rectangles, keyframes, and timing.
    InstantiateMotionTemplate {
        /// Template id. Resolved against stored timeline templates first,
        /// then the built-in render template catalog.
        template_id: String,
        /// Template starts at this timeline-time, in seconds.
        start_s: f64,
        /// Template ends at this timeline-time, in seconds. Must be `> start_s`.
        end_s: f64,
        /// Runtime animation style used when lowering the template.
        animation: MotionTemplateAnimation,
        /// Slot values keyed by the template's slot ids.
        slot_values: BTreeMap<String, serde_json::Value>,
    },
    /// Update an existing title overlay's styling. Anchored by the
    /// title clip's uuid (every InsertTitle stamps one). All fields
    /// are optional — only non-None fields update.
    SetTitle {
        /// Anchor identifying the title clip.
        anchor: Anchor,
        /// New start timestamp, if changing.
        start_s: Option<f64>,
        /// New end timestamp, if changing.
        end_s: Option<f64>,
        /// New text, if changing.
        text: Option<String>,
        /// New position, if changing.
        position: Option<TitlePosition>,
        /// New font size, if changing.
        font_size: Option<u32>,
        /// New colour, if changing.
        color: Option<String>,
        /// New weight, if changing.
        font_weight: Option<TitleWeight>,
        /// New animation, if changing.
        animation: Option<TitleAnimation>,
    },
    /// Insert a burned-in caption overlay on the project's Titles
    /// track. Captions are represented as graph nodes, not a render
    /// sidecar: the apply layer creates a clip with an `awidat.title`
    /// effect whose metadata carries `role = "caption"` plus caption
    /// styling. Render currently maps this through the same drawtext
    /// path as titles.
    InsertCaption {
        /// Caption appears at this master-timeline time, in seconds.
        start_s: f64,
        /// Caption disappears at this master-timeline time, in seconds.
        /// Must be `> start_s`.
        end_s: f64,
        /// Caption phrase to display.
        text: String,
        /// Vertical band on the frame. Defaults to bottom in the parser.
        position: TitlePosition,
        /// Font size in pixels.
        font_size: u32,
        /// Hex colour string like `"#FFFFFF"`.
        color: String,
        /// Safe-area profile such as `"mobile"` or `"standard"`.
        safe_area: String,
    },
    /// Insert a first-class annotation overlay such as a rectangle,
    /// circle, arrow, bracket, or blur region. Coordinates are
    /// normalized frame fractions in `[0.0, 1.0]` so annotations survive
    /// output resolution changes.
    InsertAnnotation {
        /// Annotation appears at this timeline-time, in seconds.
        start_s: f64,
        /// Annotation disappears at this timeline-time, in seconds.
        end_s: f64,
        /// Primitive to draw or apply.
        kind: AnnotationKind,
        /// Normalized left/start x coordinate.
        x: f64,
        /// Normalized top/start y coordinate.
        y: f64,
        /// Normalized width or arrow delta x.
        width: f64,
        /// Normalized height or arrow delta y.
        height: f64,
        /// Stroke/accent color, e.g. `"#FFCC00"`.
        color: String,
        /// Stroke width in pixels for shape primitives.
        stroke_width: u32,
        /// Optional human label retained in metadata for review surfaces.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        label: Option<String>,
    },
    /// Store the intended output format on the timeline graph. This is
    /// where vertical/short-form intent lives before rendering.
    SetOutputFormat {
        /// Aspect ratio string, e.g. `"16:9"`, `"9:16"`, `"1:1"`, or
        /// `"4:5"`.
        aspect_ratio: String,
        /// Optional platform package target, e.g. `"youtube_shorts"`.
        platform: Option<String>,
        /// Optional safe-area profile, e.g. `"mobile"`.
        safe_area: Option<String>,
    },
    /// Store project-level loudness intent on the timeline graph.
    SetLoudnessTarget {
        /// Integrated loudness target in LUFS, e.g. `-16.0`.
        integrated_lufs: f64,
        /// Optional true-peak ceiling in dBTP, e.g. `-1.0`.
        true_peak_db: Option<f64>,
    },
    /// Store delivery/package metadata on the timeline graph.
    SetPackageMetadata {
        /// Optional target platform.
        platform: Option<String>,
        /// Optional package title.
        title: Option<String>,
        /// Optional package description.
        description: Option<String>,
        /// Optional comma-separated tags.
        tags: Option<String>,
    },
    /// Store or replace a reusable timeline-level broadcast overlay.
    SetBroadcastOverlay {
        /// Complete overlay config.
        config: BroadcastOverlayConfig,
    },
    /// Store or replace the source asset catalog.
    SetAssetCatalog {
        /// Complete asset catalog.
        catalog: AssetCatalog,
    },
    /// Store source review selects and stringouts.
    SetSourceReview {
        /// Durable source range decisions.
        selects: Vec<SourceSelect>,
        /// Ordered select assemblies.
        stringouts: Vec<Stringout>,
    },
    /// Record a professional timeline operation contract for review/planning.
    ///
    /// This substrate op is metadata-only today. Concrete timeline mutations
    /// should use the existing edit ops until these contracts grow lowering
    /// semantics.
    ProfessionalTimelineEdit {
        /// Typed edit operation contract.
        edit: ProfessionalTimelineEdit,
    },
    /// Store a cross-stage proposal package with evidence.
    AddProposalPackage {
        /// Proposal package.
        package: ProposalPackage,
    },
    /// Store or replace a general parameter animation.
    SetParameterAnimation {
        /// Animation document.
        animation: ParameterAnimation,
    },
    /// Store or replace a reusable motion graphics template.
    SetMotionTemplate {
        /// Template document.
        template: MotionGraphicsTemplate,
    },
    /// Attach a composition graph to a timeline range or keep it reusable.
    AttachComposition {
        /// Composition graph.
        graph: CompositionGraph,
        /// Optional timeline attachment range.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        attach_to: Option<SourceRange>,
    },
    /// Store tracking, mask, and matte contracts.
    SetTrackingPackage {
        /// Tracking package.
        package: TrackingPackage,
    },
    /// Author a subject-aware reframe path from an existing tracker.
    AuthorSubjectReframeFromTrack {
        /// Existing tracker id in the stored tracking package.
        track_id: String,
        /// Timeline clip id receiving the reframe path.
        clip_id: String,
        /// Delivery aspect ratio label, e.g. `9:16`.
        aspect_ratio: String,
        /// Source media width in pixels.
        source_width: u32,
        /// Source media height in pixels.
        source_height: u32,
        /// Target canvas width in pixels.
        target_width: u32,
        /// Target canvas height in pixels.
        target_height: u32,
        /// Tracker sample frame rate.
        frame_rate: f64,
        /// Smoothing policy for generated crop centers.
        smoothing: ReframeSmoothing,
        /// Optional safe-area policy label.
        safe_area: Option<String>,
    },
    /// Store color finishing workflow state.
    SetColorFinishing {
        /// Color finishing state.
        state: ColorFinishingState,
    },
    /// Store audio finishing workflow state.
    SetAudioFinishing {
        /// Audio finishing state.
        state: AudioFinishingState,
    },
    /// Select and persist a named delivery profile.
    SelectDeliveryProfile {
        /// Delivery profile.
        profile: DeliveryProfile,
    },
    /// Store a delivery preflight report.
    AddPreflightReport {
        /// Preflight report.
        report: PreflightReport,
    },
    /// Set the active workflow lens.
    SetWorkflowLens {
        /// Workflow lens.
        lens: WorkflowLens,
    },
    /// Store pre-autonomy readiness and planner contracts.
    SetPipelineReadiness {
        /// Pipeline readiness.
        readiness: PipelineReadinessReport,
        /// Planner pass contracts.
        #[serde(default)]
        planner_passes: Vec<PlannerPassContract>,
    },
}

/// Return true when a graphic color can be passed to FFmpeg filters without
/// escaping or option injection risk.
pub fn valid_graphic_color(raw: &str) -> bool {
    let color = raw.trim();
    if color.is_empty() || color != raw {
        return false;
    }
    if let Some(hex) = color.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit());
    }
    color.chars().all(|ch| ch.is_ascii_alphanumeric())
}

/// Professional timeline operation contracts recorded for review/planning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "edit", rename_all = "snake_case")]
pub enum ProfessionalTimelineEdit {
    /// Ripple trim a clip and shift downstream material.
    RippleTrim {
        /// Target clip.
        anchor: Anchor,
        /// New source/timeline end in seconds.
        new_end_s: f64,
        /// Tracks affected by the ripple.
        #[serde(default)]
        ripple_tracks: Vec<String>,
    },
    /// Roll an edit point between adjacent clips.
    RollEdit {
        /// Adjacent clips.
        between: TransitionBetween,
        /// Delta seconds.
        delta_s: f64,
    },
    /// Slip a clip's source range without moving timeline placement.
    SlipClip {
        /// Target clip.
        anchor: Anchor,
        /// Source offset delta in seconds.
        delta_s: f64,
    },
    /// Slide a clip's timeline placement while preserving duration.
    SlideClip {
        /// Target clip.
        anchor: Anchor,
        /// Timeline offset delta in seconds.
        delta_s: f64,
    },
    /// Lift a timeline range, leaving gap.
    LiftRange {
        /// Timeline range.
        range: SourceRange,
        /// Track names.
        #[serde(default)]
        tracks: Vec<String>,
    },
    /// Extract a timeline range and close the gap.
    ExtractRange {
        /// Timeline range.
        range: SourceRange,
        /// Track names.
        #[serde(default)]
        tracks: Vec<String>,
    },
    /// Replace an anchored clip with a source asset/range.
    ReplaceClip {
        /// Target clip.
        anchor: Anchor,
        /// Replacement asset id/path.
        asset: String,
        /// Replacement source range.
        range: SourceRange,
    },
    /// Overwrite material at a timeline range.
    Overwrite {
        /// Destination track.
        track: String,
        /// Destination range.
        range: SourceRange,
        /// Source asset id/path.
        asset: String,
    },
    /// Append material to a track.
    Append {
        /// Destination track.
        track: String,
        /// Source asset id/path.
        asset: String,
        /// Source range.
        range: SourceRange,
    },
    /// Add a timeline marker.
    AddMarker {
        /// Marker time in seconds.
        at_s: f64,
        /// Marker label.
        label: String,
        /// Optional category.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        category: Option<String>,
    },
}

/// Hint for the kind of track created by `Insert Clip` when the named
/// track does not already exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertTrackKind {
    /// Force a video track.
    Video,
    /// Force an audio track.
    Audio,
    /// Infer from track naming conventions (`A*` / `Audio*` -> audio).
    Auto,
}

/// FFmpeg-native audio effect plan stored on clips/tracks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioFxConfig {
    /// High-pass cutoff in Hz.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub high_pass_hz: Option<f64>,
    /// Low-pass cutoff in Hz.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub low_pass_hz: Option<f64>,
    /// Parametric EQ bands.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub eq_bands: Vec<EqBand>,
    /// Compressor threshold in dB.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub compressor_threshold_db: Option<f64>,
    /// Compressor ratio.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub compressor_ratio: Option<f64>,
    /// Limiter ceiling in dBFS.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub limiter_limit_db: Option<f64>,
    /// Noise gate threshold in dBFS.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub noise_gate_threshold_db: Option<f64>,
    /// Hum notch frequency in Hz, usually 50 or 60.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hum_notch_hz: Option<f64>,
    /// Center frequency for the de-ess approximation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub de_ess_hz: Option<f64>,
    /// Gain reduction for the de-ess approximation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub de_ess_reduction_db: Option<f64>,
    /// Loudnorm integrated loudness target.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub loudnorm_i: Option<f64>,
    /// Loudnorm true-peak target.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub loudnorm_tp: Option<f64>,
}

impl AudioFxConfig {
    /// True when no effect parameters were supplied.
    pub fn is_empty(&self) -> bool {
        self.high_pass_hz.is_none()
            && self.low_pass_hz.is_none()
            && self.eq_bands.is_empty()
            && self.compressor_threshold_db.is_none()
            && self.compressor_ratio.is_none()
            && self.limiter_limit_db.is_none()
            && self.noise_gate_threshold_db.is_none()
            && self.hum_notch_hz.is_none()
            && self.de_ess_hz.is_none()
            && self.de_ess_reduction_db.is_none()
            && self.loudnorm_i.is_none()
            && self.loudnorm_tp.is_none()
    }
}

/// One parametric EQ band.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqBand {
    /// Center frequency in Hz.
    pub freq_hz: f64,
    /// Gain in dB.
    pub gain_db: f64,
    /// Band width in Hz.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub width_hz: Option<f64>,
}

/// Where on the frame a title sits. Render maps these to proportional
/// `y=` expressions (`h*0.05` for top, etc) so they survive resolution
/// changes without hard-coded pixel positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlePosition {
    /// Near the top edge.
    Top,
    /// Vertically centered.
    Center,
    /// Near the bottom edge.
    Bottom,
}

/// Font weight for title rendering. v1 ships only normal + bold;
/// custom weights would need a richer font bundling story.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleWeight {
    /// Regular weight.
    Normal,
    /// Bold weight.
    Bold,
}

/// Entry / exit animation for a title. v1 lands fade variants;
/// slide variants are wired in 16.4 if time permits and gated behind
/// careful x/y expression construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleAnimation {
    /// No animation — title pops on at start_s and off at end_s.
    None,
    /// Fade in over the leading 500ms; full alpha thereafter.
    FadeIn,
    /// Full alpha until the trailing 500ms; fade to zero.
    FadeOut,
    /// Fade in at start_s, fade out at end_s.
    FadeInOut,
    /// Slide in from off-screen on the side matching the position.
    SlideIn,
    /// Slide out off-screen on the side matching the position.
    SlideOut,
}

/// One styled run inside a rich title.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RichTextSegment {
    /// Text for this run.
    pub text: String,
    /// Optional run color. Falls back to the title color.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub color: Option<String>,
    /// Optional run weight. Falls back to the title weight.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub font_weight: Option<TitleWeight>,
}

/// Runtime animation style for motion template instantiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionTemplateAnimation {
    /// No generated animation.
    None,
    /// Fade generated template title opacity.
    Opacity,
    /// Slide/transform generated template title elements.
    Transform,
    /// Reveal text progressively.
    TextReveal,
    /// Write-on approximation using progressive text.
    WriteOn,
}

/// First-class visual annotation primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationKind {
    /// Rectangle outline around a region.
    Rectangle,
    /// Circle/ellipse approximation around a region.
    Circle,
    /// Arrow pointing across a region.
    Arrow,
    /// Bracket/corner callout around a region.
    Bracket,
    /// Blur a rectangular region, typically a face or private detail.
    Blur,
}

/// Where to anchor a `Trim`/`Delete`/`Move`/etc.
///
/// Uses `anchor_kind` as the discriminator tag instead of `kind` to avoid
/// collision with [`EdlOp`]'s own `kind` tag when an op contains an
/// anchor as a field (`#[serde(tag = "kind")]` on the outer enum injects
/// `kind` into the JSON object, conflicting with the inner enum's tag).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "anchor_kind", rename_all = "snake_case")]
pub enum Anchor {
    /// Match a clip whose `awidat.anchor.transcript_snippet` (or marker
    /// metadata) contains this substring.
    TranscriptSnippet {
        /// The text to look for.
        text: String,
    },
    /// Match a clip by its UUID. Most robust; survives any text edit.
    ClipUuid {
        /// UUID string.
        uuid: String,
    },
    /// Match by scene-change index (zero-based, into the scenedetect
    /// sidecar's `shots` array).
    SceneChangeIndex {
        /// Asset id whose scenes we index into.
        asset_id: String,
        /// 0-based index.
        index: u32,
    },
}

impl fmt::Display for Anchor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Anchor::TranscriptSnippet { text } => write!(f, "transcript_snippet={text:?}"),
            Anchor::ClipUuid { uuid } => write!(f, "clip_uuid={uuid}"),
            Anchor::SceneChangeIndex { asset_id, index } => {
                write!(f, "scene_change_index={asset_id}:{index}")
            }
        }
    }
}

/// Layering for b-roll inserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BRollPosition {
    /// New clip overlays the existing video on a higher video track.
    Overlay,
    /// New clip replaces the existing media (split + insert).
    Replace,
}

/// Output corner for picture-in-picture clips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiPCorner {
    /// Upper-left corner.
    TopLeft,
    /// Upper-right corner.
    TopRight,
    /// Lower-left corner.
    BottomLeft,
    /// Lower-right corner.
    BottomRight,
}

/// Anchor pair for `InsertTransition`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionBetween {
    /// Outgoing clip anchor.
    pub from: Anchor,
    /// Incoming clip anchor.
    pub to: Anchor,
}

/// Placement of a transition relative to the edit point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionAlignment {
    /// Transition straddles the cut evenly.
    Center,
    /// Transition begins at the cut and consumes outgoing post-roll.
    StartAtCut,
    /// Transition ends at the cut and consumes incoming pre-roll.
    EndAtCut,
}

/// One parsed EDL envelope (Begin EDL ... End EDL block).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EdlEnvelope {
    /// Operations in source order. `apply` walks them left-to-right.
    pub ops: Vec<EdlOp>,
}

impl EdlEnvelope {
    /// Empty envelope.
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    /// Number of ops.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// True iff no ops.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_display_is_informative() {
        let a = Anchor::TranscriptSnippet {
            text: "hello world".into(),
        };
        assert_eq!(a.to_string(), "transcript_snippet=\"hello world\"");
        let a = Anchor::ClipUuid {
            uuid: "c-001".into(),
        };
        assert_eq!(a.to_string(), "clip_uuid=c-001");
    }

    #[test]
    fn envelope_default_is_empty() {
        let e = EdlEnvelope::default();
        assert!(e.is_empty());
        assert_eq!(e.len(), 0);
    }

    #[test]
    fn op_serde_roundtrip() {
        let op = EdlOp::TrimClip {
            anchor: Anchor::TranscriptSnippet {
                text: "foo bar".into(),
            },
            start: Some(1.5),
            end: Some(3.0),
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: EdlOp = serde_json::from_str(&json).unwrap();
        assert_eq!(op, back);
    }
}
