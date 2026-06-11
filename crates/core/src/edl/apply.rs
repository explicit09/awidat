//! Apply a parsed [`EdlEnvelope`] to an OTIO [`Timeline`] (clone).
//!
//! Per `PLAN.md` §6.2:
//! 1. Lark parse → already done by `parser.rs`.
//! 2. Anchor resolution → `anchor::resolve`.
//! 3. Schema validation → frame ranges in bounds, asset paths, etc.
//! 4. **OTIO round-trip** — apply to a clone, validate against the OTIO
//!    schema. Reject if invalid.
//! 5. Hooks — deferred (skills phase).
//! 6. Commit to disk + emit `TimelineDiff` event — done at the tool
//!    handler level, not here.
//!
//! Per-track cursor: the survey (and codex's `seek_sequence.rs:12,16`)
//! flags this as load-bearing — when a single envelope makes multiple
//! changes to the same track, later anchors must be resolved against the
//! *post-prior-op* timeline state, not the original. We re-resolve every
//! op against the working clone, which has the same effect as the
//! cursor pattern but reuses one resolver path.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};

use montage_effects::StackPolicy;
use montage_proto::montage_meta::{
    AudioRange, AudioRelation, BrandKit, BroadcastOverlayConfig, BroadcastOverlayStyle,
    BroadcastTimedEntry, ClipAudioOverride, CutType, MontageClipMetadata, MontageTimelineMetadata,
    SemanticCutSpec, SplitEditSpec, cut_boundary_key,
};
use montage_proto::otio::{Clip, StackChild, Timeline, TrackChild, TrackKind};
use montage_render::professional::{SubjectReframeRequest, author_subject_reframe_path_from_track};
use thiserror::Error;

use super::anchor::{AnchorContext, ClipLocator, resolve};
use super::op::{
    Anchor, AnnotationKind, AudioFxConfig, EdlEnvelope, EdlOp, GapSide, InsertTrackKind,
    MarkerSelector, MotionTemplateAnimation, MulticamApplyPlan, MulticamDecision,
    ProfessionalTimelineEdit, RippleTrimEdge, SnapOptions, SnapTargetKind, TransitionAlignment,
    valid_graphic_color,
};

/// One record of what was applied. Surfaced back to the model + the TUI.
#[derive(Debug, Clone)]
pub struct AppliedOp {
    /// Op index in the envelope.
    pub index: usize,
    /// One-line human description of what landed.
    pub description: String,
    /// The clip locator that resolved (when applicable).
    pub locator: Option<ClipLocator>,
    /// Command-history-style structured metadata for audit/history surfaces.
    pub metadata: AppliedOpMetadata,
}

/// Compact command-history metadata for one applied operation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppliedOpMetadata {
    /// Stable EDL operation kind, e.g. `trim_clip`.
    pub kind: String,
    /// Stable clip IDs/names touched by this operation.
    #[serde(default)]
    pub affected_clip_ids: Vec<String>,
    /// Stable track IDs/names touched by this operation.
    #[serde(default)]
    pub affected_track_ids: Vec<String>,
    /// Source actor when known (`agent`, `user`, etc.).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
    /// Compact structured operation parameters.
    #[serde(default)]
    pub parameters: BTreeMap<String, serde_json::Value>,
}

/// Outcome of applying one envelope.
#[derive(Debug, Clone)]
pub struct ApplyOutcome {
    /// What landed, in source order.
    pub applied: Vec<AppliedOp>,
}

/// Apply errors. All are `RespondToModel`-shaped at the handler layer.
#[derive(Debug, Error)]
pub enum ApplyError {
    /// Anchor resolution failed for op at `index`.
    #[error("op #{index}: {miss}")]
    AnchorMiss {
        /// Op index in the envelope.
        index: usize,
        /// Underlying miss with candidates.
        miss: super::anchor::AnchorMiss,
    },
    /// Op asked for a field outside the legal range.
    #[error("op #{index}: invalid field: {message}")]
    Invalid {
        /// Op index.
        index: usize,
        /// Diagnostic.
        message: String,
    },
    /// OTIO round-trip validation failed after applying. The model gets
    /// the OTIO error verbatim — usually a duration/range issue.
    #[error("op #{index}: OTIO validation failed after apply: {message}")]
    OtioInvalid {
        /// Op index that triggered the rejection.
        index: usize,
        /// Diagnostic from `Project::read_otio_timeline` / validate.
        message: String,
    },
    /// An op type isn't yet implemented at the apply layer (parser
    /// accepted it; apply hasn't caught up). Surfaced as a clear
    /// "deferred" error rather than silent skip.
    #[error("op #{index}: '{op}' not yet implemented (deferred to a later batch)")]
    NotImplemented {
        /// Op index.
        index: usize,
        /// Human-readable op kind.
        op: String,
    },
}

/// Apply `envelope` to a clone of `original`. Returns `(new_timeline,
/// outcome)` on success; `(original_unchanged, error)` is implicit — the
/// original timeline is never mutated by this function.
///
/// `ctx` carries side data the resolver needs (project root for
/// loading whisper sidecars). Pass [`AnchorContext::empty()`] when
/// you don't have a project root — the resolver falls back to clip
/// metadata only.
pub fn apply(
    original: &Timeline,
    envelope: &EdlEnvelope,
    ctx: &AnchorContext,
) -> Result<(Timeline, ApplyOutcome), ApplyError> {
    let mut working = original.clone();
    let mut applied = Vec::with_capacity(envelope.ops.len());
    // Envelope-scoped split aliases: `Split Clip` keeps the original
    // uuid on the left half and stamps a FRESH uuid on the right, but
    // batch compilers (and agents) deterministically reference the
    // right half as `{left_uuid}-b` in the SAME envelope. Map that
    // convention to the real uuid so multi-op split chains resolve.
    let mut split_aliases: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for (index, op) in envelope.ops.iter().enumerate() {
        let locator = resolve_locator_for_op(&working, index, op, ctx, &split_aliases)?;
        let mut metadata = metadata_before_apply(&working, op, locator.as_ref());
        let description = apply_one(&mut working, index, op, ctx, locator)?;
        metadata_after_apply(&working, op, &mut metadata);
        if matches!(op, EdlOp::SplitClip { .. })
            && let [left_id, .., right_id] = metadata.affected_clip_ids.as_slice()
        {
            split_aliases.insert(format!("{left_id}-b"), right_id.clone());
        }
        prune_stale_cut_boundaries(&mut working);
        prune_stale_split_edits(&mut working);
        applied.push(AppliedOp {
            index,
            description,
            locator,
            metadata,
        });
    }

    // OTIO round-trip: validate the working timeline's invariants. Per
    // PLAN §6.2 step 4, this is the "linter on edit" — we reject
    // structurally-bad edits before committing.
    if let Err(e) = working.validate_for_test() {
        return Err(ApplyError::OtioInvalid {
            index: applied.last().map(|a| a.index).unwrap_or(0),
            message: e,
        });
    }

    Ok((working, ApplyOutcome { applied }))
}

fn metadata_before_apply(
    timeline: &Timeline,
    op: &EdlOp,
    locator: Option<&ClipLocator>,
) -> AppliedOpMetadata {
    let mut metadata = AppliedOpMetadata {
        kind: op_kind(op),
        affected_clip_ids: Vec::new(),
        affected_track_ids: Vec::new(),
        source: Some("agent".to_string()),
        parameters: op_parameters(op),
    };

    if let Some(locator) = locator
        && let Some((track_name, clip_id)) = ids_at_locator(timeline, locator)
    {
        push_unique(&mut metadata.affected_track_ids, track_name);
        push_unique(&mut metadata.affected_clip_ids, clip_id);
    }

    match op {
        EdlOp::InsertClip { track, asset, .. } => {
            push_unique(&mut metadata.affected_track_ids, track.clone());
            metadata
                .parameters
                .entry("asset".to_string())
                .or_insert_with(|| serde_json::Value::String(asset.clone()));
        }
        EdlOp::SetTrackAudio { track, .. } | EdlOp::SetTrackAudioFx { track, .. } => {
            push_unique(&mut metadata.affected_track_ids, track.clone());
        }
        _ => {}
    }

    metadata
}

fn metadata_after_apply(timeline: &Timeline, op: &EdlOp, metadata: &mut AppliedOpMetadata) {
    match op {
        EdlOp::InsertClip {
            asset, track, name, ..
        } => {
            if let Some(clip_id) = find_inserted_clip_id(timeline, track, asset, name.as_deref()) {
                push_unique(&mut metadata.affected_clip_ids, clip_id);
            }
        }
        EdlOp::SplitClip { .. } => {
            if let Some(left_id) = metadata.affected_clip_ids.first().cloned()
                && let Some(right_id) = find_split_right_clip_id(timeline, &left_id)
            {
                push_unique(&mut metadata.affected_clip_ids, right_id);
            }
        }
        _ => {}
    }
}

fn op_kind(op: &EdlOp) -> String {
    serde_json::to_value(op)
        .ok()
        .and_then(|value| {
            value
                .get("op")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn op_parameters(op: &EdlOp) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    let Ok(serde_json::Value::Object(mut object)) = serde_json::to_value(op) else {
        return out;
    };
    for skipped in [
        "op",
        "anchor",
        "between",
        "plan",
        "edit",
        "spec",
        "split_edit",
    ] {
        object.remove(skipped);
    }
    for (key, value) in object {
        if is_compact_parameter_value(&value) {
            out.insert(key, value);
        }
    }
    out
}

fn is_compact_parameter_value(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => true,
        serde_json::Value::Array(values) => values.len() <= 8 && values.iter().all(is_scalar_json),
        serde_json::Value::Object(values) => {
            values.len() <= 8 && values.values().all(is_scalar_json)
        }
    }
}

fn is_scalar_json(value: &serde_json::Value) -> bool {
    matches!(
        value,
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) | serde_json::Value::String(_)
    )
}

fn ids_at_locator(timeline: &Timeline, locator: &ClipLocator) -> Option<(String, String)> {
    let StackChild::Track(track) = timeline.tracks.children.get(locator.track_index)? else {
        return None;
    };
    let track_id = track.name.clone();
    let TrackChild::Clip(clip) = track.children.get(locator.child_index)? else {
        return None;
    };
    Some((track_id, clip_id(clip)))
}

fn find_inserted_clip_id(
    timeline: &Timeline,
    track_name: &str,
    asset: &str,
    name: Option<&str>,
) -> Option<String> {
    let track = timeline
        .tracks
        .children
        .iter()
        .find_map(|child| match child {
            StackChild::Track(track) if track.name == track_name => Some(track),
            _ => None,
        })?;
    track.children.iter().rev().find_map(|child| match child {
        TrackChild::Clip(clip)
            if name.is_none_or(|name| clip.name == name)
                && clip_media_target(clip).as_deref() == Some(asset) =>
        {
            Some(clip_id(clip))
        }
        _ => None,
    })
}

fn find_split_right_clip_id(timeline: &Timeline, left_id: &str) -> Option<String> {
    for child in &timeline.tracks.children {
        let StackChild::Track(track) = child else {
            continue;
        };
        for pair in track.children.windows(2) {
            let TrackChild::Clip(left) = &pair[0] else {
                continue;
            };
            let TrackChild::Clip(right) = &pair[1] else {
                continue;
            };
            if clip_id(left) == left_id {
                return Some(clip_id(right));
            }
        }
    }
    None
}

fn clip_id(clip: &Clip) -> String {
    clip_uuid(clip).unwrap_or(clip.name.as_str()).to_string()
}

fn clip_media_target(clip: &Clip) -> Option<String> {
    match &clip.media_reference {
        montage_proto::otio::MediaReference::External(reference) => {
            Some(reference.target_url.clone())
        }
        montage_proto::otio::MediaReference::Missing(_) => None,
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !value.is_empty() && !values.contains(&value) {
        values.push(value);
    }
}

/// Apply one op in place. Returns a one-line description of what landed.
fn apply_one(
    working: &mut Timeline,
    index: usize,
    op: &EdlOp,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    match op {
        EdlOp::TrimClip { anchor, start, end } => {
            apply_trim(working, index, anchor, *start, *end, ctx, locator)
        }
        EdlOp::DeleteClip { anchor } => apply_delete(working, index, anchor, ctx, locator),
        EdlOp::SplitClip { anchor, at_s, snap } => {
            apply_split(working, index, anchor, *at_s, snap.as_ref(), ctx, locator)
        }
        EdlOp::UntrimClip { anchor, start, end } => {
            apply_untrim(working, index, anchor, *start, *end, ctx, locator)
        }
        EdlOp::InsertClip {
            asset,
            track,
            track_kind,
            at_position,
            at_s,
            start,
            end,
            name,
            link_group_id,
            snap,
        } => apply_insert_clip(
            working,
            index,
            asset,
            track,
            *track_kind,
            *at_position,
            *at_s,
            *start,
            *end,
            name.as_deref(),
            link_group_id.as_deref(),
            snap.as_ref(),
        ),
        EdlOp::InsertBRoll {
            anchor,
            asset,
            duration_s,
            position,
        } => apply_insert_broll(
            working,
            index,
            anchor,
            asset,
            *duration_s,
            *position,
            ctx,
            locator,
        ),
        EdlOp::InsertPiP {
            anchor,
            asset,
            duration_s,
            source_start_s,
            corner,
            scale,
            margin_pct,
        } => apply_insert_pip(
            working,
            index,
            anchor,
            asset,
            *duration_s,
            *source_start_s,
            *corner,
            *scale,
            *margin_pct,
            ctx,
            locator,
        ),
        EdlOp::MoveClip {
            anchor,
            to_position,
            at_s,
            snap,
        } => apply_move_clip(
            working,
            index,
            anchor,
            *to_position,
            *at_s,
            snap.as_ref(),
            ctx,
            locator,
        ),
        EdlOp::RippleMove {
            anchor,
            delta_s,
            snap,
        } => apply_ripple_move(
            working,
            index,
            anchor,
            *delta_s,
            snap.as_ref(),
            ctx,
            locator,
        ),
        EdlOp::RippleDelete { anchor } => apply_ripple_delete(working, index, anchor, ctx, locator),
        EdlOp::RippleTrim {
            anchor,
            edge,
            value_s,
        } => apply_ripple_trim(working, index, anchor, *edge, *value_s, ctx, locator),
        EdlOp::DeleteGap { anchor, side } => {
            apply_delete_gap(working, index, anchor, *side, ctx, locator)
        }
        EdlOp::TrimTrackTail { track } => apply_trim_track_tail(working, index, track),
        EdlOp::InsertTrack {
            name,
            kind,
            at_position,
        } => apply_insert_track(working, index, name, *kind, *at_position),
        EdlOp::DeleteTrack { name, force } => apply_delete_track(working, index, name, *force),
        EdlOp::ApplyMulticamPlan { plan } => apply_multicam_plan(working, index, plan),
        EdlOp::InsertTransition {
            between,
            kind,
            duration_s,
            alignment,
            in_offset_s,
            out_offset_s,
            spec,
        } => apply_insert_transition(
            working,
            index,
            between,
            kind,
            *duration_s,
            *alignment,
            *in_offset_s,
            *out_offset_s,
            spec.as_ref(),
            ctx,
        ),
        EdlOp::DeleteTransition { between } => {
            apply_delete_transition(working, index, between, ctx)
        }
        EdlOp::SetCutIntent { between, spec } => {
            apply_set_cut_intent(working, index, between, spec, ctx)
        }
        EdlOp::SetVolume { anchor, value } => {
            apply_set_volume(working, index, anchor, *value, ctx, locator)
        }
        EdlOp::MuteClip { anchor, muted } => {
            apply_mute_clip(working, index, anchor, *muted, ctx, locator)
        }
        EdlOp::RemoveAudio {
            anchor,
            start_s,
            end_s,
            clear,
        } => apply_remove_audio(
            working, index, anchor, *start_s, *end_s, *clear, ctx, locator,
        ),
        EdlOp::SetAudioFade {
            anchor,
            fade_in_s,
            fade_out_s,
        } => apply_set_audio_fade(
            working,
            index,
            anchor,
            *fade_in_s,
            *fade_out_s,
            ctx,
            locator,
        ),
        EdlOp::SetAudioLead {
            anchor,
            lead_s,
            split_edit,
        } => apply_set_audio_lead(
            working,
            index,
            anchor,
            *lead_s,
            split_edit.as_ref(),
            ctx,
            locator,
        ),
        EdlOp::SetAudioTrail {
            anchor,
            trail_s,
            split_edit,
        } => apply_set_audio_trail(
            working,
            index,
            anchor,
            *trail_s,
            split_edit.as_ref(),
            ctx,
            locator,
        ),
        EdlOp::SetTrackAudio {
            track,
            role,
            volume,
            muted,
            solo,
        } => apply_set_track_audio(
            working,
            index,
            track,
            role.as_deref(),
            *volume,
            *muted,
            *solo,
        ),
        EdlOp::SetDucking {
            track,
            enabled,
            amount_db,
            attack_ms,
            release_ms,
        } => apply_set_ducking(
            working,
            index,
            track,
            *enabled,
            *amount_db,
            *attack_ms,
            *release_ms,
        ),
        EdlOp::SetSyncGroup {
            anchor,
            sync_group_id,
            offset_s,
            speed_factor,
            confidence,
        } => apply_set_sync_group(
            working,
            index,
            anchor,
            sync_group_id,
            *offset_s,
            *speed_factor,
            *confidence,
            ctx,
            locator,
        ),
        EdlOp::SetClipAudioFx { anchor, fx } => {
            apply_set_clip_audio_fx(working, index, anchor, fx, ctx, locator)
        }
        EdlOp::SetTrackAudioFx { track, fx } => apply_set_track_audio_fx(working, index, track, fx),
        EdlOp::SetEffect {
            anchor,
            effect,
            params,
            rationale,
        } => apply_set_effect(
            working,
            index,
            anchor,
            effect,
            params,
            rationale.as_deref(),
            ctx,
            locator,
        ),
        EdlOp::SetSpeed { anchor, factor } => {
            apply_set_speed(working, index, anchor, *factor, ctx, locator)
        }
        EdlOp::SetTimeRemap { anchor, curve } => {
            apply_set_time_remap(working, index, anchor, curve, ctx, locator)
        }
        EdlOp::SetFreeze {
            anchor,
            freeze_at_source_s,
            duration_s,
            freeze_position,
            audio_behavior,
        } => apply_set_freeze(
            working,
            index,
            anchor,
            *freeze_at_source_s,
            *duration_s,
            freeze_position.as_deref(),
            audio_behavior.as_deref(),
            ctx,
            locator,
        ),
        EdlOp::SetColorCorrection {
            anchor,
            exposure_ev,
            contrast,
            saturation,
            temperature,
            tint,
            shadows,
            highlights,
        } => apply_set_color_correction(
            working,
            index,
            anchor,
            *exposure_ev,
            *contrast,
            *saturation,
            *temperature,
            *tint,
            *shadows,
            *highlights,
            ctx,
            locator,
        ),
        EdlOp::ApplyLut {
            anchor,
            lut_path,
            interpolation,
            strength,
        } => apply_lut(
            working,
            index,
            anchor,
            lut_path,
            interpolation.as_deref(),
            *strength,
            ctx,
            locator,
        ),
        EdlOp::RemoveLut { anchor } => apply_remove_lut(working, index, anchor, ctx, locator),
        EdlOp::InsertTitle {
            start_s,
            end_s,
            text,
            position,
            font_size,
            color,
            font_weight,
            animation,
            phases,
            font_family,
            font_path,
        } => apply_insert_title(
            working,
            index,
            *start_s,
            *end_s,
            text,
            *position,
            *font_size,
            color,
            *font_weight,
            *animation,
            *phases,
            font_family.as_deref(),
            font_path.as_deref(),
        ),
        EdlOp::InsertRichTitle {
            start_s,
            end_s,
            segments,
            position,
            font_size,
            animation,
            phases,
            font_family,
            font_path,
        } => apply_insert_rich_title(
            working,
            index,
            *start_s,
            *end_s,
            segments,
            *position,
            *font_size,
            *animation,
            *phases,
            font_family.as_deref(),
            font_path.as_deref(),
        ),
        EdlOp::InstantiateMotionTemplate {
            template_id,
            start_s,
            end_s,
            animation,
            slot_values,
        } => apply_instantiate_motion_template(
            working,
            index,
            template_id,
            *start_s,
            *end_s,
            *animation,
            slot_values,
        ),
        EdlOp::SetTitle {
            anchor,
            start_s,
            end_s,
            text,
            position,
            font_size,
            color,
            font_weight,
            animation,
            phases,
            font_family,
            font_path,
        } => apply_set_title(
            working,
            index,
            anchor,
            *start_s,
            *end_s,
            text.as_deref(),
            *position,
            *font_size,
            color.as_deref(),
            *font_weight,
            *animation,
            *phases,
            font_family.as_deref(),
            font_path.as_deref(),
            ctx,
            locator,
        ),
        EdlOp::InsertCaption {
            start_s,
            end_s,
            text,
            position,
            font_size,
            color,
            safe_area,
            word_timings,
            style_json,
        } => apply_insert_caption(
            working,
            index,
            *start_s,
            *end_s,
            text,
            *position,
            *font_size,
            color,
            safe_area,
            word_timings,
            style_json.as_ref(),
        ),
        EdlOp::InsertAnnotation {
            start_s,
            end_s,
            kind,
            x,
            y,
            width,
            height,
            color,
            stroke_width,
            label,
        } => apply_insert_annotation(
            working,
            index,
            *start_s,
            *end_s,
            *kind,
            *x,
            *y,
            *width,
            *height,
            color,
            *stroke_width,
            label.as_deref(),
        ),
        EdlOp::SetOutputFormat {
            aspect_ratio,
            platform,
            safe_area,
        } => apply_set_output_format(
            working,
            index,
            aspect_ratio,
            platform.as_deref(),
            safe_area.as_deref(),
        ),
        EdlOp::SetLoudnessTarget {
            integrated_lufs,
            true_peak_db,
        } => apply_set_loudness_target(working, index, *integrated_lufs, *true_peak_db),
        EdlOp::SetPackageMetadata {
            platform,
            title,
            description,
            tags,
        } => apply_set_package_metadata(
            working,
            index,
            platform.as_deref(),
            title.as_deref(),
            description.as_deref(),
            tags.as_deref(),
        ),
        EdlOp::SetBroadcastOverlay { config } => {
            apply_set_broadcast_overlay(working, index, config)
        }
        EdlOp::SetAssetCatalog { catalog } => {
            let meta = timeline_montage_metadata(working);
            meta.asset_catalog = Some(catalog.clone());
            Ok(format!(
                "stored asset catalog with {} assets",
                catalog.assets.len()
            ))
        }
        EdlOp::SetSourceReview {
            selects,
            stringouts,
        } => {
            let meta = timeline_montage_metadata(working);
            meta.selects = selects.clone();
            meta.stringouts = stringouts.clone();
            Ok(format!(
                "stored source review with {} selects and {} stringouts",
                selects.len(),
                stringouts.len()
            ))
        }
        EdlOp::ProfessionalTimelineEdit { edit } => {
            apply_professional_timeline_edit(working, index, edit, ctx, locator)
        }
        EdlOp::AddProposalPackage { package } => {
            let meta = timeline_montage_metadata(working);
            replace_by_id(&mut meta.proposal_packages, package.clone(), |item| {
                &item.id
            });
            Ok(format!("stored proposal package {}", package.id))
        }
        EdlOp::SetParameterAnimation { animation } => {
            apply_set_parameter_animation(working, index, animation)
        }
        EdlOp::SetMotionTemplate { template } => {
            let meta = timeline_montage_metadata(working);
            replace_by_id(&mut meta.motion_templates, template.clone(), |item| {
                &item.id
            });
            Ok(format!("stored motion template {}", template.id))
        }
        EdlOp::SetMotionScene { scene } => {
            let meta = timeline_montage_metadata(working);
            replace_by_id(&mut meta.motion_scenes, scene.clone(), |item| &item.id);
            Ok(format!("stored motion scene {}", scene.id))
        }
        EdlOp::AttachComposition { graph, attach_to } => {
            let meta = timeline_montage_metadata(working);
            replace_by_id(&mut meta.composition_graphs, graph.clone(), |item| &item.id);
            if let Some(range) = attach_to {
                meta.extra.insert(
                    format!("composition_attach_{}", graph.id),
                    serde_json::to_value(range).map_err(|e| ApplyError::Invalid {
                        index,
                        message: format!("composition attachment could not serialize: {e}"),
                    })?,
                );
            }
            Ok(format!("stored composition graph {}", graph.id))
        }
        EdlOp::SetTrackingPackage { package } => {
            let meta = timeline_montage_metadata(working);
            meta.tracking_package = Some(package.clone());
            Ok(format!(
                "stored tracking package with {} tracks, {} masks, {} mattes",
                package.tracks.len(),
                package.masks.len(),
                package.mattes.len()
            ))
        }
        EdlOp::AuthorSubjectReframeFromTrack {
            track_id,
            clip_id,
            aspect_ratio,
            source_width,
            source_height,
            target_width,
            target_height,
            frame_rate,
            smoothing,
            safe_area,
        } => apply_author_subject_reframe_from_track(
            working,
            index,
            track_id,
            SubjectReframeRequest {
                clip_id: clip_id.clone(),
                aspect_ratio: aspect_ratio.clone(),
                source_width: *source_width,
                source_height: *source_height,
                target_width: *target_width,
                target_height: *target_height,
                frame_rate: *frame_rate,
                smoothing: *smoothing,
                safe_area: safe_area.clone(),
            },
        ),
        EdlOp::SetColorFinishing { state } => {
            let meta = timeline_montage_metadata(working);
            meta.color_finishing = Some(state.clone());
            Ok(format!(
                "stored color finishing state with {} shot groups",
                state.shot_groups.len()
            ))
        }
        EdlOp::SetAudioFinishing { state } => {
            let meta = timeline_montage_metadata(working);
            meta.audio_finishing = Some(state.clone());
            Ok(format!(
                "stored audio finishing state with {} buses",
                state.buses.len()
            ))
        }
        EdlOp::SelectDeliveryProfile { profile } => {
            let meta = timeline_montage_metadata(working);
            replace_by_id(&mut meta.delivery_profiles, profile.clone(), |item| {
                &item.id
            });
            Ok(format!("selected delivery profile {}", profile.id))
        }
        EdlOp::AddPreflightReport { report } => {
            let meta = timeline_montage_metadata(working);
            replace_by_id(&mut meta.preflight_reports, report.clone(), |item| &item.id);
            Ok(format!("stored preflight report {}", report.id))
        }
        EdlOp::SetWorkflowLens { lens } => {
            let meta = timeline_montage_metadata(working);
            if !meta.workflow_lenses.contains(lens) {
                meta.workflow_lenses.push(*lens);
            }
            meta.extra
                .insert("active_workflow_lens".into(), serde_json::json!(lens));
            Ok(format!("set workflow lens to {lens:?}"))
        }
        EdlOp::SetPipelineReadiness {
            readiness,
            planner_passes,
        } => {
            let meta = timeline_montage_metadata(working);
            meta.pipeline_readiness = Some(readiness.clone());
            meta.planner_passes = planner_passes.clone();
            Ok(format!(
                "stored pipeline readiness with {} stages",
                readiness.stages.len()
            ))
        }
        EdlOp::SetBrandKit { kit } => apply_set_brand_kit(working, index, kit),
    }
}

fn resolve_locator_for_op(
    working: &Timeline,
    index: usize,
    op: &EdlOp,
    ctx: &AnchorContext,
    split_aliases: &std::collections::HashMap<String, String>,
) -> Result<Option<ClipLocator>, ApplyError> {
    let anchor = match op {
        EdlOp::TrimClip { anchor, .. }
        | EdlOp::DeleteClip { anchor }
        | EdlOp::SplitClip { anchor, .. }
        | EdlOp::UntrimClip { anchor, .. }
        | EdlOp::MoveClip { anchor, .. }
        | EdlOp::RippleMove { anchor, .. }
        | EdlOp::RippleDelete { anchor }
        | EdlOp::RippleTrim { anchor, .. }
        | EdlOp::DeleteGap { anchor, .. }
        | EdlOp::InsertBRoll { anchor, .. }
        | EdlOp::InsertPiP { anchor, .. }
        | EdlOp::SetVolume { anchor, .. }
        | EdlOp::MuteClip { anchor, .. }
        | EdlOp::RemoveAudio { anchor, .. }
        | EdlOp::SetAudioFade { anchor, .. }
        | EdlOp::SetAudioLead { anchor, .. }
        | EdlOp::SetAudioTrail { anchor, .. }
        | EdlOp::SetSyncGroup { anchor, .. }
        | EdlOp::SetClipAudioFx { anchor, .. }
        | EdlOp::SetEffect { anchor, .. }
        | EdlOp::SetSpeed { anchor, .. }
        | EdlOp::SetTimeRemap { anchor, .. }
        | EdlOp::SetFreeze { anchor, .. }
        | EdlOp::SetColorCorrection { anchor, .. }
        | EdlOp::ApplyLut { anchor, .. }
        | EdlOp::RemoveLut { anchor }
        | EdlOp::SetTitle { anchor, .. }
        | EdlOp::ProfessionalTimelineEdit {
            edit: ProfessionalTimelineEdit::RippleTrim { anchor, .. },
        }
        | EdlOp::ProfessionalTimelineEdit {
            edit: ProfessionalTimelineEdit::SlipClip { anchor, .. },
        }
        | EdlOp::ProfessionalTimelineEdit {
            edit: ProfessionalTimelineEdit::SlideClip { anchor, .. },
        }
        | EdlOp::ProfessionalTimelineEdit {
            edit: ProfessionalTimelineEdit::ReplaceClip { anchor, .. },
        } => anchor,
        EdlOp::InsertClip { .. }
        | EdlOp::ApplyMulticamPlan { .. }
        | EdlOp::InsertTransition { .. }
        | EdlOp::DeleteTransition { .. }
        | EdlOp::SetCutIntent { .. }
        | EdlOp::SetTrackAudio { .. }
        | EdlOp::SetDucking { .. }
        | EdlOp::SetTrackAudioFx { .. }
        | EdlOp::InsertTitle { .. }
        | EdlOp::InsertRichTitle { .. }
        | EdlOp::InstantiateMotionTemplate { .. }
        | EdlOp::InsertCaption { .. }
        | EdlOp::InsertAnnotation { .. }
        | EdlOp::SetOutputFormat { .. }
        | EdlOp::SetLoudnessTarget { .. }
        | EdlOp::SetPackageMetadata { .. }
        | EdlOp::SetBroadcastOverlay { .. }
        | EdlOp::SetAssetCatalog { .. }
        | EdlOp::SetSourceReview { .. }
        | EdlOp::ProfessionalTimelineEdit { .. }
        | EdlOp::AddProposalPackage { .. }
        | EdlOp::SetParameterAnimation { .. }
        | EdlOp::SetMotionTemplate { .. }
        | EdlOp::SetMotionScene { .. }
        | EdlOp::AttachComposition { .. }
        | EdlOp::SetTrackingPackage { .. }
        | EdlOp::AuthorSubjectReframeFromTrack { .. }
        | EdlOp::SetColorFinishing { .. }
        | EdlOp::SetAudioFinishing { .. }
        | EdlOp::SelectDeliveryProfile { .. }
        | EdlOp::AddPreflightReport { .. }
        | EdlOp::SetWorkflowLens { .. }
        | EdlOp::SetPipelineReadiness { .. }
        | EdlOp::SetBrandKit { .. }
        | EdlOp::TrimTrackTail { .. }
        | EdlOp::InsertTrack { .. }
        | EdlOp::DeleteTrack { .. } => return Ok(None),
    };
    // Rewrite `{left_uuid}-b` references to the fresh uuid the engine
    // actually stamped on the split's right half (see `apply`).
    let aliased = match anchor {
        Anchor::ClipUuid { uuid } => split_aliases
            .get(uuid)
            .map(|real| Anchor::ClipUuid { uuid: real.clone() }),
        _ => None,
    };
    resolve(working, aliased.as_ref().unwrap_or(anchor), ctx)
        .map(Some)
        .map_err(|miss| ApplyError::AnchorMiss { index, miss })
}

fn required_locator(index: usize, locator: Option<ClipLocator>) -> Result<ClipLocator, ApplyError> {
    locator.ok_or_else(|| ApplyError::Invalid {
        index,
        message: "internal error: anchored op applied without a resolved locator".into(),
    })
}

fn apply_author_subject_reframe_from_track(
    working: &mut Timeline,
    index: usize,
    track_id: &str,
    request: SubjectReframeRequest,
) -> Result<String, ApplyError> {
    let clip_id = request.clip_id.clone();
    let meta = timeline_montage_metadata(working);
    let package = meta
        .tracking_package
        .as_mut()
        .ok_or_else(|| ApplyError::Invalid {
            index,
            message: "author subject reframe requires an existing tracking package".into(),
        })?;
    let plan =
        author_subject_reframe_path_from_track(package, track_id, request).map_err(|err| {
            ApplyError::Invalid {
                index,
                message: format!("author subject reframe failed: {err}"),
            }
        })?;

    Ok(format!(
        "authored subject reframe path {} for clip {} from tracker {}",
        plan.reframe_id, clip_id, track_id
    ))
}

fn apply_set_parameter_animation(
    working: &mut Timeline,
    index: usize,
    animation: &montage_proto::professional::ParameterAnimation,
) -> Result<String, ApplyError> {
    use montage_proto::professional::{
        FindingSeverity, RUNTIME_CLIP_PARAMETERS, RUNTIME_TRACK_PARAMETERS,
        is_runtime_parameter_animation_target,
    };

    if let Some(diagnostic) = animation
        .validate()
        .into_iter()
        .find(|diagnostic| diagnostic.severity == FindingSeverity::Error)
    {
        return Err(ApplyError::Invalid {
            index,
            message: diagnostic.message,
        });
    }

    if !animation.metadata_only && !is_runtime_parameter_animation_target(&animation.target) {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "set_parameter_animation: unsupported parameter animation target for {}; supported clip parameters: {}; supported track parameters: {}; set metadata_only=true to store this as non-runtime metadata",
                animation.id,
                RUNTIME_CLIP_PARAMETERS.join(", "),
                RUNTIME_TRACK_PARAMETERS.join(", ")
            ),
        });
    }

    let meta = timeline_montage_metadata(working);
    replace_by_id(&mut meta.parameter_animations, animation.clone(), |item| {
        &item.id
    });
    Ok(format!("stored parameter animation {}", animation.id))
}

fn apply_instantiate_motion_template(
    working: &mut Timeline,
    index: usize,
    template_id: &str,
    start_s: f64,
    end_s: f64,
    animation: MotionTemplateAnimation,
    slot_values: &BTreeMap<String, serde_json::Value>,
) -> Result<String, ApplyError> {
    if template_id.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "instantiate_motion_template: template_id must be non-empty".into(),
        });
    }
    if !start_s.is_finite() || !end_s.is_finite() || end_s <= start_s {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "instantiate_motion_template: invalid window [{start_s}..{end_s}]; end_s must be finite and > start_s"
            ),
        });
    }

    let template =
        resolve_motion_template(working, template_id).ok_or_else(|| ApplyError::Invalid {
            index,
            message: format!("instantiate_motion_template: unknown template {template_id:?}"),
        })?;
    let mut values = slot_values.clone();
    let generated_target_clip = if template.slots.iter().any(|slot| slot.id == "target_clip")
        && !values.contains_key("target_clip")
    {
        let clip_id = motion_template_clip_id(template_id, index, 0);
        values.insert(
            "target_clip".into(),
            serde_json::Value::String(clip_id.clone()),
        );
        Some(clip_id)
    } else {
        None
    };

    let filled =
        montage_render::professional::fill_motion_template(&template, values).map_err(|error| {
            ApplyError::Invalid {
                index,
                message: format!("instantiate_motion_template: {error}"),
            }
        })?;
    let render = montage_render::professional::lower_motion_template(
        &filled,
        montage_render::professional::MotionTemplateTiming {
            start_s,
            end_s,
            animation: render_template_animation(animation),
        },
    );

    for (title_index, title) in render.titles.iter().enumerate() {
        let clip_uuid = generated_target_clip
            .as_deref()
            .filter(|_| title_index == 0);
        apply_insert_text_overlay(
            working,
            index,
            &title.role,
            title.safe_area.as_deref(),
            clip_uuid,
            None,
            title.start_s,
            title.end_s,
            &title.text,
            core_title_position(title.position),
            title.font_size,
            &title.color,
            core_title_weight(title.font_weight),
            core_title_animation(title.animation),
            None,
        )?;
    }

    for parameter_animation in &render.parameter_animations {
        apply_set_parameter_animation(working, index, parameter_animation)?;
    }

    Ok(format!(
        "instantiated motion template {template_id} into {} title clip(s) and {} parameter animation(s)",
        render.titles.len(),
        render.parameter_animations.len()
    ))
}

fn resolve_motion_template(
    working: &Timeline,
    template_id: &str,
) -> Option<montage_proto::professional::MotionGraphicsTemplate> {
    working
        .metadata
        .montage
        .as_ref()
        .and_then(|metadata| {
            metadata
                .motion_templates
                .iter()
                .find(|template| template.id == template_id)
                .cloned()
        })
        .or_else(|| {
            montage_render::professional::built_in_motion_templates()
                .into_iter()
                .find(|template| template.id == template_id)
        })
}

fn motion_template_clip_id(template_id: &str, op_index: usize, title_index: usize) -> String {
    let sanitized = template_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("motion-template-{sanitized}-{op_index}-title-{title_index}")
}

fn render_template_animation(
    animation: MotionTemplateAnimation,
) -> montage_render::professional::TemplateAnimation {
    match animation {
        MotionTemplateAnimation::None => montage_render::professional::TemplateAnimation::None,
        MotionTemplateAnimation::Opacity => {
            montage_render::professional::TemplateAnimation::Opacity
        }
        MotionTemplateAnimation::Transform => {
            montage_render::professional::TemplateAnimation::Transform
        }
        MotionTemplateAnimation::TextReveal => {
            montage_render::professional::TemplateAnimation::TextReveal
        }
        MotionTemplateAnimation::WriteOn => {
            montage_render::professional::TemplateAnimation::WriteOn
        }
    }
}

fn core_title_position(position: montage_render::TitlePosition) -> super::op::TitlePosition {
    match position {
        montage_render::TitlePosition::Top => super::op::TitlePosition::Top,
        montage_render::TitlePosition::Center => super::op::TitlePosition::Center,
        montage_render::TitlePosition::Bottom => super::op::TitlePosition::Bottom,
    }
}

fn core_title_weight(weight: montage_render::TitleWeight) -> super::op::TitleWeight {
    match weight {
        montage_render::TitleWeight::Normal => super::op::TitleWeight::Normal,
        montage_render::TitleWeight::Bold => super::op::TitleWeight::Bold,
    }
}

fn core_title_animation(animation: montage_render::TitleAnimation) -> super::op::TitleAnimation {
    match animation {
        montage_render::TitleAnimation::None => super::op::TitleAnimation::None,
        montage_render::TitleAnimation::FadeIn => super::op::TitleAnimation::FadeIn,
        montage_render::TitleAnimation::FadeOut => super::op::TitleAnimation::FadeOut,
        montage_render::TitleAnimation::FadeInOut => super::op::TitleAnimation::FadeInOut,
        montage_render::TitleAnimation::SlideIn => super::op::TitleAnimation::SlideIn,
        montage_render::TitleAnimation::SlideOut => super::op::TitleAnimation::SlideOut,
    }
}

fn apply_professional_timeline_edit(
    working: &mut Timeline,
    index: usize,
    edit: &ProfessionalTimelineEdit,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    match edit {
        ProfessionalTimelineEdit::RippleTrim {
            anchor,
            new_end_s,
            ripple_tracks,
        } => {
            let locator = required_locator(index, locator)?;
            let StackChild::Track(track) = &working.tracks.children[locator.track_index] else {
                return Err(ApplyError::Invalid {
                    index,
                    message: "ripple_trim: anchor resolved to a non-track stack child".into(),
                });
            };
            if !ripple_tracks.is_empty() && !ripple_tracks.iter().any(|name| name == &track.name) {
                return Err(ApplyError::Invalid {
                    index,
                    message: format!(
                        "ripple_trim: resolved track {:?} is not included in ripple_tracks {:?}",
                        track.name, ripple_tracks
                    ),
                });
            }
            let description = apply_trim(
                working,
                index,
                anchor,
                None,
                Some(*new_end_s),
                ctx,
                Some(locator),
            )?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "ripple trimmed via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::SlipClip { anchor, delta_s } => {
            let description = apply_slip_clip(working, index, anchor, *delta_s, locator)?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "slipped via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::AddMarker {
            at_s,
            label,
            category,
        } => {
            let description = apply_professional_marker(working, index, *at_s, label, category)?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "added marker via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::UpdateMarker {
            selector,
            label,
            category,
            note,
            at_s,
            duration_s,
        } => {
            let description = apply_update_marker(
                working,
                index,
                selector,
                label,
                category,
                note,
                *at_s,
                *duration_s,
                ctx,
            )?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "updated marker via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::DeleteMarker { selector } => {
            let description = apply_delete_marker(working, index, selector, ctx)?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "deleted marker via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::RollEdit { between, delta_s } => {
            let description = apply_roll_edit(working, index, between, *delta_s, ctx)?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "rolled edit via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::SlideClip { anchor, delta_s } => {
            let description = apply_slide_clip(working, index, anchor, *delta_s, locator)?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "slid clip via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::LiftRange { range, tracks } => {
            let description = apply_lift_range(working, index, range, tracks)?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "lifted range via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::ExtractRange { range, tracks } => {
            let description = apply_extract_range(working, index, range, tracks)?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "extracted range via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::ReplaceClip {
            anchor,
            asset,
            range,
        } => {
            let description = apply_replace_clip(working, index, anchor, asset, range, locator)?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "replaced clip via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::Append {
            track,
            asset,
            range,
        } => {
            let description = apply_append_clip(working, index, track, asset, range)?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "appended clip via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::Overwrite {
            track,
            range,
            asset,
        } => {
            let description = apply_overwrite_clip(working, index, track, range, asset)?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "overwrote range via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::WrapAsNested {
            track,
            start_index,
            end_index,
            name,
        } => {
            let description =
                apply_wrap_as_nested(working, index, track, *start_index, *end_index, name)?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "wrapped nested stack via professional timeline edit: {description}"
            ))
        }
        ProfessionalTimelineEdit::FlattenNested { track, child_index } => {
            let description = apply_flatten_nested(working, index, track, *child_index)?;
            record_professional_timeline_edit(working, index, edit)?;
            Ok(format!(
                "flattened nested stack via professional timeline edit: {description}"
            ))
        }
    }
}

fn apply_roll_edit(
    working: &mut Timeline,
    index: usize,
    between: &super::op::TransitionBetween,
    delta_s: f64,
    ctx: &AnchorContext,
) -> Result<String, ApplyError> {
    if !delta_s.is_finite() {
        return Err(ApplyError::Invalid {
            index,
            message: format!("roll_edit: delta_s must be finite, got {delta_s}"),
        });
    }
    if delta_s.abs() < 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: "roll_edit: delta_s is a no-op".into(),
        });
    }
    let from_loc = resolve(working, &between.from, ctx)
        .map_err(|miss| ApplyError::AnchorMiss { index, miss })?;
    let to_loc = resolve(working, &between.to, ctx)
        .map_err(|miss| ApplyError::AnchorMiss { index, miss })?;
    if from_loc.track_index != to_loc.track_index {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "roll_edit: anchors resolve to different tracks ({} vs {})",
                from_loc.track_index, to_loc.track_index
            ),
        });
    }
    if to_loc.child_index != from_loc.child_index + 1 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "roll_edit: anchors are not adjacent clips (from at index {}, to at index {})",
                from_loc.child_index, to_loc.child_index
            ),
        });
    }

    let StackChild::Track(track) = &mut working.tracks.children[from_loc.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "roll_edit: anchor resolved to a non-track stack child".into(),
        });
    };
    let (left, right) = track.children.split_at_mut(to_loc.child_index);
    let TrackChild::Clip(from_clip) = &mut left[from_loc.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "roll_edit: from anchor resolved to a non-clip child".into(),
        });
    };
    let TrackChild::Clip(to_clip) = &mut right[0] else {
        return Err(ApplyError::Invalid {
            index,
            message: "roll_edit: to anchor resolved to a non-clip child".into(),
        });
    };

    let Some(from_range) = from_clip.source_range.as_ref() else {
        return Err(ApplyError::Invalid {
            index,
            message: "roll_edit: from clip has no source_range".into(),
        });
    };
    let Some(to_range) = to_clip.source_range.as_ref() else {
        return Err(ApplyError::Invalid {
            index,
            message: "roll_edit: to clip has no source_range".into(),
        });
    };
    let from_rate = from_range.start_time.rate;
    let to_rate = to_range.start_time.rate;
    let from_start_s = from_range.start_time.to_seconds();
    let from_end_s = from_start_s + from_range.duration.to_seconds();
    let to_start_s = to_range.start_time.to_seconds();
    let to_end_s = to_start_s + to_range.duration.to_seconds();
    let target_from_end_s = from_end_s + delta_s;
    let target_to_start_s = to_start_s + delta_s;
    if target_from_end_s <= from_start_s + 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: "roll_edit: delta would collapse or invert the outgoing clip".into(),
        });
    }
    if target_to_start_s >= to_end_s - 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: "roll_edit: delta would collapse or invert the incoming clip".into(),
        });
    }
    validate_available_source_range(
        index,
        "roll_edit outgoing",
        from_clip,
        from_start_s,
        target_from_end_s,
    )?;
    validate_available_source_range(
        index,
        "roll_edit incoming",
        to_clip,
        target_to_start_s,
        to_end_s,
    )?;

    from_clip.source_range = Some(montage_proto::otio::TimeRange::new(
        montage_proto::otio::RationalTime::new(from_start_s * from_rate, from_rate),
        montage_proto::otio::RationalTime::new(
            (target_from_end_s - from_start_s) * from_rate,
            from_rate,
        ),
    ));
    to_clip.source_range = Some(montage_proto::otio::TimeRange::new(
        montage_proto::otio::RationalTime::new(target_to_start_s * to_rate, to_rate),
        montage_proto::otio::RationalTime::new((to_end_s - target_to_start_s) * to_rate, to_rate),
    ));
    Ok(format!(
        "rolled edit between {:?} and {:?} by {delta_s:.3}s",
        from_clip.name, to_clip.name
    ))
}

fn apply_slide_clip(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    delta_s: f64,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = anchor;
    if !delta_s.is_finite() {
        return Err(ApplyError::Invalid {
            index,
            message: format!("slide_clip: delta_s must be finite, got {delta_s}"),
        });
    }
    if delta_s.abs() < 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: "slide_clip: delta_s is a no-op".into(),
        });
    }
    let locator = required_locator(index, locator)?;
    if locator.child_index == 0 {
        return Err(ApplyError::Invalid {
            index,
            message: "slide_clip: target clip has no previous neighbor to absorb the slide".into(),
        });
    }
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "slide_clip: anchor resolved to a non-track stack child".into(),
        });
    };
    if locator.child_index + 1 >= track.children.len() {
        return Err(ApplyError::Invalid {
            index,
            message: "slide_clip: target clip has no next neighbor to absorb the slide".into(),
        });
    }

    let (before_target, target_and_after) = track.children.split_at_mut(locator.child_index);
    let previous_child = before_target
        .last_mut()
        .ok_or_else(|| ApplyError::Invalid {
            index,
            message: "slide_clip: target clip has no previous neighbor to absorb the slide".into(),
        })?;
    let (target_slice, after_target) = target_and_after.split_at_mut(1);
    let target_child = &mut target_slice[0];
    let next_child = after_target
        .first_mut()
        .ok_or_else(|| ApplyError::Invalid {
            index,
            message: "slide_clip: target clip has no next neighbor to absorb the slide".into(),
        })?;
    let TrackChild::Clip(previous_clip) = previous_child else {
        return Err(ApplyError::Invalid {
            index,
            message: "slide_clip: previous neighbor is not a clip".into(),
        });
    };
    let TrackChild::Clip(target_clip) = target_child else {
        return Err(ApplyError::Invalid {
            index,
            message: "slide_clip: anchor resolved to a non-clip track child".into(),
        });
    };
    let TrackChild::Clip(next_clip) = next_child else {
        return Err(ApplyError::Invalid {
            index,
            message: "slide_clip: next neighbor is not a clip".into(),
        });
    };

    let Some(previous_range) = previous_clip.source_range.as_ref() else {
        return Err(ApplyError::Invalid {
            index,
            message: "slide_clip: previous clip has no source_range".into(),
        });
    };
    let Some(target_range) = target_clip.source_range.as_ref() else {
        return Err(ApplyError::Invalid {
            index,
            message: "slide_clip: target clip has no source_range".into(),
        });
    };
    let Some(next_range) = next_clip.source_range.as_ref() else {
        return Err(ApplyError::Invalid {
            index,
            message: "slide_clip: next clip has no source_range".into(),
        });
    };

    let previous_rate = previous_range.start_time.rate;
    let next_rate = next_range.start_time.rate;
    let previous_start_s = previous_range.start_time.to_seconds();
    let previous_end_s = previous_start_s + previous_range.duration.to_seconds();
    let next_start_s = next_range.start_time.to_seconds();
    let next_end_s = next_start_s + next_range.duration.to_seconds();
    let target_previous_end_s = previous_end_s + delta_s;
    let target_next_start_s = next_start_s + delta_s;

    if target_previous_end_s <= previous_start_s + 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: "slide_clip: delta would collapse or invert the previous clip".into(),
        });
    }
    if target_next_start_s >= next_end_s - 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: "slide_clip: delta would collapse or invert the next clip".into(),
        });
    }
    validate_available_source_range(
        index,
        "slide_clip previous",
        previous_clip,
        previous_start_s,
        target_previous_end_s,
    )?;
    validate_available_source_range(
        index,
        "slide_clip next",
        next_clip,
        target_next_start_s,
        next_end_s,
    )?;

    previous_clip.source_range = Some(montage_proto::otio::TimeRange::new(
        montage_proto::otio::RationalTime::new(previous_start_s * previous_rate, previous_rate),
        montage_proto::otio::RationalTime::new(
            (target_previous_end_s - previous_start_s) * previous_rate,
            previous_rate,
        ),
    ));
    next_clip.source_range = Some(montage_proto::otio::TimeRange::new(
        montage_proto::otio::RationalTime::new(target_next_start_s * next_rate, next_rate),
        montage_proto::otio::RationalTime::new(
            (next_end_s - target_next_start_s) * next_rate,
            next_rate,
        ),
    ));
    Ok(format!(
        "slid clip {:?} by {delta_s:.3}s while preserving its {:.3}s duration",
        target_clip.name,
        target_range.duration.to_seconds()
    ))
}

fn apply_lift_range(
    working: &mut Timeline,
    index: usize,
    range: &montage_proto::professional::SourceRange,
    tracks: &[String],
) -> Result<String, ApplyError> {
    validate_professional_timeline_range(index, "lift_range", range)?;
    let mut matched_tracks = 0usize;
    let mut touched_tracks = 0usize;
    let mut lifted_s = 0.0_f64;
    for child in &mut working.tracks.children {
        let StackChild::Track(track) = child else {
            continue;
        };
        if !tracks.is_empty() && !tracks.iter().any(|name| name == &track.name) {
            continue;
        }
        matched_tracks += 1;
        let rate = track
            .children
            .iter()
            .find_map(TrackChildClipRate::as_clip_rate)
            .unwrap_or(24.0);
        let lifted_on_track = lift_range_on_track(index, track, range.start_s, range.end_s, rate)?;
        if lifted_on_track > 1e-6 {
            touched_tracks += 1;
            lifted_s += lifted_on_track;
        }
    }
    if matched_tracks == 0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("lift_range: no tracks matched {:?}", tracks),
        });
    }
    if touched_tracks == 0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "lift_range: range [{:.3}s..{:.3}s] did not overlap selected tracks",
                range.start_s, range.end_s
            ),
        });
    }
    Ok(format!(
        "lifted range [{:.3}s..{:.3}s] on {touched_tracks} track(s), leaving gaps for {lifted_s:.3}s of selected material",
        range.start_s, range.end_s
    ))
}

fn apply_extract_range(
    working: &mut Timeline,
    index: usize,
    range: &montage_proto::professional::SourceRange,
    tracks: &[String],
) -> Result<String, ApplyError> {
    validate_professional_timeline_range(index, "extract_range", range)?;
    let mut matched_tracks = 0usize;
    let mut touched_tracks = 0usize;
    let mut extracted_s = 0.0_f64;
    for child in &mut working.tracks.children {
        let StackChild::Track(track) = child else {
            continue;
        };
        if !tracks.is_empty() && !tracks.iter().any(|name| name == &track.name) {
            continue;
        }
        matched_tracks += 1;
        let extracted_on_track = extract_range_on_track(index, track, range.start_s, range.end_s)?;
        if extracted_on_track > 1e-6 {
            touched_tracks += 1;
            extracted_s += extracted_on_track;
        }
    }
    if matched_tracks == 0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("extract_range: no tracks matched {:?}", tracks),
        });
    }
    if touched_tracks == 0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "extract_range: range [{:.3}s..{:.3}s] did not overlap selected tracks",
                range.start_s, range.end_s
            ),
        });
    }
    Ok(format!(
        "extracted range [{:.3}s..{:.3}s] on {touched_tracks} track(s), closing {extracted_s:.3}s of selected material",
        range.start_s, range.end_s
    ))
}

fn apply_replace_clip(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    asset: &str,
    range: &montage_proto::professional::SourceRange,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = anchor;
    validate_professional_timeline_range(index, "replace_clip", range)?;
    if asset.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "replace_clip: asset must not be empty".into(),
        });
    }
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "replace_clip: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "replace_clip: anchor resolved to a non-clip track child".into(),
        });
    };
    let rate = clip
        .source_range
        .as_ref()
        .map(|range| range.start_time.rate)
        .filter(|rate| *rate > 0.0)
        .unwrap_or(24.0);
    let original_name = clip.name.clone();
    clip.media_reference = montage_proto::otio::MediaReference::External(
        montage_proto::otio::ExternalReference::new(asset.to_string()),
    );
    clip.source_range = Some(montage_proto::otio::TimeRange::new(
        montage_proto::otio::RationalTime::new(range.start_s * rate, rate),
        montage_proto::otio::RationalTime::new((range.end_s - range.start_s) * rate, rate),
    ));
    Ok(format!(
        "replaced clip {original_name:?} with asset {asset:?} source=[{:.3}s..{:.3}s]",
        range.start_s, range.end_s
    ))
}

fn apply_append_clip(
    working: &mut Timeline,
    index: usize,
    track: &str,
    asset: &str,
    range: &montage_proto::professional::SourceRange,
) -> Result<String, ApplyError> {
    validate_professional_timeline_range(index, "append", range)?;
    if track.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "append: track must not be empty".into(),
        });
    }
    if asset.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "append: asset must not be empty".into(),
        });
    }
    apply_insert_clip(
        working,
        index,
        asset,
        track,
        Some(InsertTrackKind::Auto),
        None,
        None,
        Some(range.start_s),
        Some(range.end_s),
        None,
        None,
        None,
    )
}

fn apply_overwrite_clip(
    working: &mut Timeline,
    index: usize,
    track_name: &str,
    range: &montage_proto::professional::SourceRange,
    asset: &str,
) -> Result<String, ApplyError> {
    validate_professional_timeline_range(index, "overwrite", range)?;
    if track_name.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "overwrite: track must not be empty".into(),
        });
    }
    if asset.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "overwrite: asset must not be empty".into(),
        });
    }
    let track_idx = working
        .tracks
        .children
        .iter()
        .enumerate()
        .find_map(|(i, sc)| match sc {
            StackChild::Track(track) if track.name == track_name => Some(i),
            _ => None,
        });
    let track_idx = match track_idx {
        Some(i) => i,
        None => {
            let kind = infer_insert_track_kind(track_name, Some(InsertTrackKind::Auto));
            working
                .tracks
                .children
                .push(StackChild::Track(montage_proto::otio::Track::empty(
                    track_name.to_string(),
                    kind,
                )));
            working.tracks.children.len() - 1
        }
    };
    let StackChild::Track(track) = &mut working.tracks.children[track_idx] else {
        return Err(ApplyError::Invalid {
            index,
            message: "overwrite: resolved destination is not a track".into(),
        });
    };
    let rate = track
        .children
        .iter()
        .find_map(TrackChildClipRate::as_clip_rate)
        .unwrap_or(24.0);
    let source_duration_s = range.end_s - range.start_s;
    let overwrite = build_professional_source_clip(
        track,
        asset,
        0.0,
        source_duration_s,
        rate,
        Some("overwrite"),
    );
    overwrite_range_on_track(index, track, range.start_s, range.end_s, overwrite, rate)?;
    Ok(format!(
        "overwrote range [{:.3}s..{:.3}s] on track {track_name:?} with asset {asset:?}",
        range.start_s, range.end_s
    ))
}

fn build_professional_source_clip(
    track: &montage_proto::otio::Track,
    asset: &str,
    source_start_s: f64,
    source_end_s: f64,
    rate: f64,
    name_prefix: Option<&str>,
) -> Clip {
    let name = name_prefix
        .map(|prefix| format!("{prefix}-{}", track.children.len()))
        .unwrap_or_else(|| default_clip_name_for_asset(track, asset));
    let mut clip = Clip::empty(name);
    clip.media_reference = montage_proto::otio::MediaReference::External(
        montage_proto::otio::ExternalReference::new(asset.to_string()),
    );
    clip.source_range = Some(montage_proto::otio::TimeRange::new(
        montage_proto::otio::RationalTime::new(source_start_s * rate, rate),
        montage_proto::otio::RationalTime::new((source_end_s - source_start_s) * rate, rate),
    ));
    stamp_fresh_clip_uuid(&mut clip);
    clip
}

fn validate_professional_timeline_range(
    index: usize,
    label: &str,
    range: &montage_proto::professional::SourceRange,
) -> Result<(), ApplyError> {
    if !range.start_s.is_finite() || !range.end_s.is_finite() {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "{label}: range bounds must be finite, got [{:?}..{:?}]",
                range.start_s, range.end_s
            ),
        });
    }
    if range.start_s < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "{label}: range start must be >= 0, got {:.3}s",
                range.start_s
            ),
        });
    }
    if range.end_s <= range.start_s {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "{label}: range end {:.3}s must be after start {:.3}s",
                range.end_s, range.start_s
            ),
        });
    }
    Ok(())
}

fn lift_range_on_track(
    index: usize,
    track: &mut montage_proto::otio::Track,
    range_start_s: f64,
    range_end_s: f64,
    rate: f64,
) -> Result<f64, ApplyError> {
    let mut cursor_s = 0.0_f64;
    let mut lifted_s = 0.0_f64;
    let mut replacement = Vec::with_capacity(track.children.len() + 2);
    for child in std::mem::take(&mut track.children) {
        let duration_s = child_duration(&child);
        if duration_s <= 1e-6 {
            if cursor_s < range_start_s || cursor_s >= range_end_s {
                replacement.push(child);
            }
            continue;
        }
        let child_start_s = cursor_s;
        let child_end_s = child_start_s + duration_s;
        let overlap_start_s = child_start_s.max(range_start_s);
        let overlap_end_s = child_end_s.min(range_end_s);
        if overlap_end_s <= overlap_start_s + 1e-6 {
            replacement.push(child);
            cursor_s = child_end_s;
            continue;
        }

        let mut emitted_clip_piece = false;
        if child_start_s < range_start_s {
            let leading_end_s = range_start_s.min(child_end_s);
            if let Some(piece) = timeline_child_segment(
                index,
                &child,
                child_start_s,
                child_start_s,
                leading_end_s,
                false,
            )? {
                emitted_clip_piece |= matches!(piece, TrackChild::Clip(_));
                replacement.push(piece);
            }
        }

        let overlap_duration_s = overlap_end_s - overlap_start_s;
        lifted_s += overlap_duration_s;
        replacement.push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
            overlap_duration_s,
            rate,
        )));

        if range_end_s < child_end_s {
            let trailing_start_s = range_end_s.max(child_start_s);
            if let Some(piece) = timeline_child_segment(
                index,
                &child,
                child_start_s,
                trailing_start_s,
                child_end_s,
                emitted_clip_piece,
            )? {
                replacement.push(piece);
            }
        }
        cursor_s = child_end_s;
    }
    track.children = replacement;
    merge_adjacent_gaps(&mut track.children, rate);
    Ok(lifted_s)
}

fn extract_range_on_track(
    index: usize,
    track: &mut montage_proto::otio::Track,
    range_start_s: f64,
    range_end_s: f64,
) -> Result<f64, ApplyError> {
    let mut cursor_s = 0.0_f64;
    let mut extracted_s = 0.0_f64;
    let mut replacement = Vec::with_capacity(track.children.len());
    for child in std::mem::take(&mut track.children) {
        let duration_s = child_duration(&child);
        if duration_s <= 1e-6 {
            if cursor_s < range_start_s || cursor_s >= range_end_s {
                replacement.push(child);
            }
            continue;
        }
        let child_start_s = cursor_s;
        let child_end_s = child_start_s + duration_s;
        let overlap_start_s = child_start_s.max(range_start_s);
        let overlap_end_s = child_end_s.min(range_end_s);
        if overlap_end_s <= overlap_start_s + 1e-6 {
            replacement.push(child);
            cursor_s = child_end_s;
            continue;
        }

        let mut emitted_clip_piece = false;
        if child_start_s < range_start_s {
            let leading_end_s = range_start_s.min(child_end_s);
            if let Some(piece) = timeline_child_segment(
                index,
                &child,
                child_start_s,
                child_start_s,
                leading_end_s,
                false,
            )? {
                emitted_clip_piece |= matches!(piece, TrackChild::Clip(_));
                replacement.push(piece);
            }
        }

        extracted_s += overlap_end_s - overlap_start_s;

        if range_end_s < child_end_s {
            let trailing_start_s = range_end_s.max(child_start_s);
            if let Some(piece) = timeline_child_segment(
                index,
                &child,
                child_start_s,
                trailing_start_s,
                child_end_s,
                emitted_clip_piece,
            )? {
                replacement.push(piece);
            }
        }
        cursor_s = child_end_s;
    }
    track.children = replacement;
    let rate = track
        .children
        .iter()
        .find_map(TrackChildClipRate::as_clip_rate)
        .unwrap_or(24.0);
    merge_adjacent_gaps(&mut track.children, rate);
    Ok(extracted_s)
}

fn overwrite_range_on_track(
    index: usize,
    track: &mut montage_proto::otio::Track,
    range_start_s: f64,
    range_end_s: f64,
    overwrite: Clip,
    rate: f64,
) -> Result<(), ApplyError> {
    let mut cursor_s = 0.0_f64;
    let mut pending_overwrite = Some(overwrite);
    let mut replacement = Vec::with_capacity(track.children.len() + 3);
    for child in std::mem::take(&mut track.children) {
        let duration_s = child_duration(&child);
        if duration_s <= 1e-6 {
            if cursor_s < range_start_s || cursor_s >= range_end_s {
                replacement.push(child);
            }
            continue;
        }
        let child_start_s = cursor_s;
        let child_end_s = child_start_s + duration_s;
        if child_end_s <= range_start_s + 1e-6 {
            replacement.push(child);
            cursor_s = child_end_s;
            continue;
        }
        if pending_overwrite.is_some() && child_start_s >= range_end_s - 1e-6 {
            let overwrite = pending_overwrite
                .take()
                .ok_or_else(|| ApplyError::Invalid {
                    index,
                    message: "overwrite: replacement clip was already inserted".into(),
                })?;
            replacement.push(TrackChild::Clip(overwrite));
            replacement.push(child);
            cursor_s = child_end_s;
            continue;
        }

        let overlap_start_s = child_start_s.max(range_start_s);
        let overlap_end_s = child_end_s.min(range_end_s);
        if overlap_end_s <= overlap_start_s + 1e-6 {
            replacement.push(child);
            cursor_s = child_end_s;
            continue;
        }

        let mut emitted_clip_piece = false;
        if child_start_s < range_start_s {
            let leading_end_s = range_start_s.min(child_end_s);
            if let Some(piece) = timeline_child_segment(
                index,
                &child,
                child_start_s,
                child_start_s,
                leading_end_s,
                false,
            )? {
                emitted_clip_piece |= matches!(piece, TrackChild::Clip(_));
                replacement.push(piece);
            }
        }

        if pending_overwrite.is_some() {
            let overwrite = pending_overwrite
                .take()
                .ok_or_else(|| ApplyError::Invalid {
                    index,
                    message: "overwrite: replacement clip was already inserted".into(),
                })?;
            replacement.push(TrackChild::Clip(overwrite));
        }

        if range_end_s < child_end_s {
            let trailing_start_s = range_end_s.max(child_start_s);
            if let Some(piece) = timeline_child_segment(
                index,
                &child,
                child_start_s,
                trailing_start_s,
                child_end_s,
                emitted_clip_piece,
            )? {
                replacement.push(piece);
            }
        }
        cursor_s = child_end_s;
    }
    if let Some(overwrite) = pending_overwrite {
        if cursor_s < range_start_s - 1e-6 {
            replacement.push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                range_start_s - cursor_s,
                rate,
            )));
        }
        replacement.push(TrackChild::Clip(overwrite));
    }
    track.children = replacement;
    merge_adjacent_gaps(&mut track.children, rate);
    Ok(())
}

fn timeline_child_segment(
    index: usize,
    child: &TrackChild,
    child_start_s: f64,
    segment_start_s: f64,
    segment_end_s: f64,
    fresh_clip_uuid: bool,
) -> Result<Option<TrackChild>, ApplyError> {
    let duration_s = segment_end_s - segment_start_s;
    if duration_s <= 1e-6 {
        return Ok(None);
    }
    match child {
        TrackChild::Clip(clip) => {
            let Some(range) = clip.source_range.as_ref() else {
                return Err(ApplyError::Invalid {
                    index,
                    message: format!(
                        "lift_range: clip {:?} has no source_range and cannot be segmented",
                        clip.name
                    ),
                });
            };
            let mut segment = clip.clone();
            let rate = range.start_time.rate;
            let source_start_s = range.start_time.to_seconds() + (segment_start_s - child_start_s);
            segment.source_range = Some(montage_proto::otio::TimeRange::new(
                montage_proto::otio::RationalTime::new(source_start_s * rate, rate),
                montage_proto::otio::RationalTime::new(duration_s * rate, rate),
            ));
            if fresh_clip_uuid {
                segment.name = format!("{}-lift-tail", segment.name);
                stamp_fresh_clip_uuid(&mut segment);
            }
            Ok(Some(TrackChild::Clip(segment)))
        }
        TrackChild::Gap(gap) => Ok(Some(TrackChild::Gap(
            montage_proto::otio::Gap::of_duration(duration_s, gap.source_range.duration.rate),
        ))),
        TrackChild::Transition(_) => Ok(None),
        TrackChild::Stack(_) => Err(ApplyError::Invalid {
            index,
            message: "lift_range: nested stack segmentation is not supported".into(),
        }),
    }
}

fn validate_available_source_range(
    index: usize,
    label: &str,
    clip: &Clip,
    start_s: f64,
    end_s: f64,
) -> Result<(), ApplyError> {
    if start_s < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("{label}: target source start {start_s:.3}s is before 0"),
        });
    }
    if let montage_proto::otio::MediaReference::External(reference) = &clip.media_reference
        && let Some(available_range) = &reference.available_range
    {
        let available_start_s = available_range.start_time.to_seconds();
        let available_end_s = available_start_s + available_range.duration.to_seconds();
        if start_s < available_start_s - 1e-6 || end_s > available_end_s + 1e-6 {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "{label}: target source range [{start_s:.3}s..{end_s:.3}s] exceeds \
                     available media range [{available_start_s:.3}s..{available_end_s:.3}s]"
                ),
            });
        }
    }
    Ok(())
}

fn record_professional_timeline_edit(
    working: &mut Timeline,
    index: usize,
    edit: &ProfessionalTimelineEdit,
) -> Result<(), ApplyError> {
    let meta = timeline_montage_metadata(working);
    meta.extra.insert(
        "last_professional_timeline_edit".into(),
        serde_json::to_value(edit).map_err(|e| ApplyError::Invalid {
            index,
            message: format!("professional timeline edit could not serialize: {e}"),
        })?,
    );
    Ok(())
}

fn apply_slip_clip(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    delta_s: f64,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = anchor;
    if !delta_s.is_finite() {
        return Err(ApplyError::Invalid {
            index,
            message: format!("slip_clip: delta_s must be finite, got {delta_s}"),
        });
    }
    if delta_s.abs() < 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: "slip_clip: delta_s is a no-op".into(),
        });
    }
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "slip_clip: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "slip_clip: anchor resolved to a non-clip track child".into(),
        });
    };
    let Some(range) = clip.source_range.as_ref() else {
        return Err(ApplyError::Invalid {
            index,
            message: "slip_clip: clip has no source_range".into(),
        });
    };
    let rate = range.start_time.rate;
    let duration_s = range.duration.to_seconds();
    let target_start_s = range.start_time.to_seconds() + delta_s;
    let target_end_s = target_start_s + duration_s;
    if target_start_s < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("slip_clip: target source start {target_start_s:.3}s is before 0"),
        });
    }
    if let montage_proto::otio::MediaReference::External(reference) = &clip.media_reference
        && let Some(available_range) = &reference.available_range
    {
        let available_start_s = available_range.start_time.to_seconds();
        let available_end_s = available_start_s + available_range.duration.to_seconds();
        if target_start_s < available_start_s - 1e-6 || target_end_s > available_end_s + 1e-6 {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "slip_clip: target source range [{target_start_s:.3}s..{target_end_s:.3}s] \
                     exceeds available media range [{available_start_s:.3}s..{available_end_s:.3}s]"
                ),
            });
        }
    }
    clip.source_range = Some(montage_proto::otio::TimeRange::new(
        montage_proto::otio::RationalTime::new(target_start_s * rate, rate),
        montage_proto::otio::RationalTime::new(duration_s * rate, rate),
    ));
    let name = clip.name.clone();
    Ok(format!(
        "slipped clip {name:?} source range by {delta_s:.3}s to \
         [{target_start_s:.3}s..{target_end_s:.3}s]"
    ))
}

fn apply_professional_marker(
    working: &mut Timeline,
    index: usize,
    at_s: f64,
    label: &str,
    category: &Option<String>,
) -> Result<String, ApplyError> {
    if !at_s.is_finite() || at_s < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("add_marker: at_s must be finite and >= 0, got {at_s}"),
        });
    }
    let label = label.trim();
    if label.is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "add_marker: label must not be empty".into(),
        });
    }
    for (track_index, child) in working.tracks.children.iter_mut().enumerate() {
        let StackChild::Track(track) = child else {
            continue;
        };
        let mut cursor_s = 0.0;
        for (child_index, child) in track.children.iter_mut().enumerate() {
            let duration_s = child_duration(child);
            let start_s = cursor_s;
            let end_s = start_s + duration_s;
            cursor_s = end_s;
            if at_s < start_s || at_s >= end_s {
                continue;
            }
            let TrackChild::Clip(clip) = child else {
                return Err(ApplyError::Invalid {
                    index,
                    message: format!(
                        "add_marker: timeline time {at_s:.3}s resolves to non-clip item on track {:?}",
                        track.name
                    ),
                });
            };
            let rate = clip
                .source_range
                .as_ref()
                .map(|range| range.start_time.rate)
                .unwrap_or(24.0);
            let relative_s = at_s - start_s;
            let marker_id = format!("marker-{track_index}-{child_index}-{}", clip.markers.len());
            let mut marker = montage_proto::otio::Marker::new(
                label.to_string(),
                montage_proto::otio::TimeRange::new(
                    montage_proto::otio::RationalTime::new(relative_s * rate, rate),
                    montage_proto::otio::RationalTime::new(0.0, rate),
                ),
            );
            let mut metadata = montage_proto::montage_meta::MontageMarkerMetadata {
                category: category.clone(),
                note: Some(label.to_string()),
                ..montage_proto::montage_meta::MontageMarkerMetadata::default()
            };
            metadata
                .extra
                .insert("id".into(), serde_json::Value::String(marker_id));
            marker.metadata.montage = Some(metadata);
            clip.markers.push(marker);
            return Ok(format!(
                "added marker {label:?} at {at_s:.3}s on clip {:?}",
                clip.name
            ));
        }
    }
    Err(ApplyError::Invalid {
        index,
        message: format!("add_marker: no clip covers timeline time {at_s:.3}s"),
    })
}

#[derive(Debug, Clone)]
struct MarkerCandidate {
    track_index: usize,
    child_index: usize,
    marker_index: usize,
    clip_start_s: f64,
    clip_end_s: f64,
    timeline_s: f64,
}

#[allow(clippy::too_many_arguments)]
fn apply_update_marker(
    working: &mut Timeline,
    index: usize,
    selector: &MarkerSelector,
    label: &Option<String>,
    category: &Option<String>,
    note: &Option<String>,
    at_s: Option<f64>,
    duration_s: Option<f64>,
    ctx: &AnchorContext,
) -> Result<String, ApplyError> {
    let candidate = resolve_marker_candidate(working, index, selector, ctx)?;
    if let Some(new_at_s) = at_s
        && (!new_at_s.is_finite() || new_at_s < 0.0)
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("update_marker: at_s must be finite and >= 0, got {new_at_s}"),
        });
    }
    if let Some(new_duration_s) = duration_s
        && (!new_duration_s.is_finite() || new_duration_s < 0.0)
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "update_marker: duration_s must be finite and >= 0, got {new_duration_s}"
            ),
        });
    }
    if let Some(new_at_s) = at_s
        && (new_at_s < candidate.clip_start_s || new_at_s >= candidate.clip_end_s)
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "update_marker: moving marker to {new_at_s:.3}s would leave its current clip range [{:.3}s..{:.3}s]",
                candidate.clip_start_s, candidate.clip_end_s
            ),
        });
    }
    let StackChild::Track(track) = &mut working.tracks.children[candidate.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "update_marker: resolved marker track is not a track".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[candidate.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "update_marker: resolved marker child is not a clip".into(),
        });
    };
    let marker = clip
        .markers
        .get_mut(candidate.marker_index)
        .ok_or_else(|| ApplyError::Invalid {
            index,
            message: "update_marker: resolved marker index disappeared".into(),
        })?;
    if let Some(label) = label {
        let label = label.trim();
        if label.is_empty() {
            return Err(ApplyError::Invalid {
                index,
                message: "update_marker: label must not be empty".into(),
            });
        }
        marker.name = label.to_string();
    }
    let marker_metadata = marker
        .metadata
        .montage
        .get_or_insert_with(montage_proto::montage_meta::MontageMarkerMetadata::default);
    if let Some(category) = category {
        marker_metadata.category = Some(category.trim().to_string());
    }
    if let Some(note) = note {
        let note = note.trim().to_string();
        marker.comment = Some(note.clone());
        marker_metadata.note = Some(note);
    }
    if let Some(new_at_s) = at_s {
        let rate = marker.marked_range.start_time.rate;
        let relative_s = new_at_s - candidate.clip_start_s;
        marker.marked_range.start_time =
            montage_proto::otio::RationalTime::new(relative_s * rate, rate);
    }
    if let Some(new_duration_s) = duration_s {
        let rate = marker.marked_range.duration.rate;
        marker.marked_range.duration =
            montage_proto::otio::RationalTime::new(new_duration_s * rate, rate);
    }
    Ok(format!(
        "updated marker {:?} at {:.3}s on clip {:?}",
        marker.name, candidate.timeline_s, clip.name
    ))
}

fn apply_delete_marker(
    working: &mut Timeline,
    index: usize,
    selector: &MarkerSelector,
    ctx: &AnchorContext,
) -> Result<String, ApplyError> {
    let candidate = resolve_marker_candidate(working, index, selector, ctx)?;
    let StackChild::Track(track) = &mut working.tracks.children[candidate.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "delete_marker: resolved marker track is not a track".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[candidate.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "delete_marker: resolved marker child is not a clip".into(),
        });
    };
    if candidate.marker_index >= clip.markers.len() {
        return Err(ApplyError::Invalid {
            index,
            message: "delete_marker: resolved marker index disappeared".into(),
        });
    }
    let marker = clip.markers.remove(candidate.marker_index);
    Ok(format!(
        "deleted marker {:?} at {:.3}s from clip {:?}",
        marker.name, candidate.timeline_s, clip.name
    ))
}

fn resolve_marker_candidate(
    working: &Timeline,
    index: usize,
    selector: &MarkerSelector,
    ctx: &AnchorContext,
) -> Result<MarkerCandidate, ApplyError> {
    validate_marker_selector(index, selector)?;
    let locator = selector
        .anchor
        .as_ref()
        .map(|anchor| {
            resolve(working, anchor, ctx).map_err(|miss| ApplyError::AnchorMiss { index, miss })
        })
        .transpose()?;
    let matches = collect_marker_candidates(working, selector, locator);
    match matches.as_slice() {
        [candidate] => Ok(candidate.clone()),
        [] => Err(ApplyError::Invalid {
            index,
            message: format!("marker selector {selector:?} matched no markers"),
        }),
        many => Err(ApplyError::Invalid {
            index,
            message: format!(
                "marker selector {selector:?} matches {} markers; add marker_id, anchor, or at_s to disambiguate",
                many.len()
            ),
        }),
    }
}

fn validate_marker_selector(index: usize, selector: &MarkerSelector) -> Result<(), ApplyError> {
    if selector.marker_id.is_none()
        && selector.anchor.is_none()
        && selector.label.is_none()
        && selector.at_s.is_none()
    {
        return Err(ApplyError::Invalid {
            index,
            message: "marker selector must include marker_id, anchor, label, or at_s".into(),
        });
    }
    if let Some(marker_id) = selector.marker_id.as_deref()
        && marker_id.trim().is_empty()
    {
        return Err(ApplyError::Invalid {
            index,
            message: "marker selector marker_id must not be empty".into(),
        });
    }
    if let Some(label) = selector.label.as_deref()
        && label.trim().is_empty()
    {
        return Err(ApplyError::Invalid {
            index,
            message: "marker selector label must not be empty".into(),
        });
    }
    if let Some(at_s) = selector.at_s
        && (!at_s.is_finite() || at_s < 0.0)
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("marker selector at_s must be finite and >= 0, got {at_s}"),
        });
    }
    Ok(())
}

fn collect_marker_candidates(
    timeline: &Timeline,
    selector: &MarkerSelector,
    locator: Option<ClipLocator>,
) -> Vec<MarkerCandidate> {
    let mut out = Vec::new();
    for (track_index, child) in timeline.tracks.children.iter().enumerate() {
        let StackChild::Track(track) = child else {
            continue;
        };
        let mut cursor_s = 0.0;
        for (child_index, child) in track.children.iter().enumerate() {
            let duration_s = child_duration(child);
            let clip_start_s = cursor_s;
            let clip_end_s = cursor_s + duration_s;
            cursor_s = clip_end_s;
            if let Some(locator) = locator
                && (locator.track_index != track_index || locator.child_index != child_index)
            {
                continue;
            }
            let TrackChild::Clip(clip) = child else {
                continue;
            };
            for (marker_index, marker) in clip.markers.iter().enumerate() {
                let timeline_s = clip_start_s + marker.marked_range.start_time.to_seconds();
                if marker_matches_selector(marker, timeline_s, selector) {
                    out.push(MarkerCandidate {
                        track_index,
                        child_index,
                        marker_index,
                        clip_start_s,
                        clip_end_s,
                        timeline_s,
                    });
                }
            }
        }
    }
    out
}

fn marker_matches_selector(
    marker: &montage_proto::otio::Marker,
    timeline_s: f64,
    selector: &MarkerSelector,
) -> bool {
    if let Some(expected_id) = selector.marker_id.as_deref()
        && marker_metadata_id(marker) != Some(expected_id.trim())
    {
        return false;
    }
    if let Some(label) = selector.label.as_deref()
        && marker.name != label.trim()
    {
        return false;
    }
    if let Some(at_s) = selector.at_s
        && (timeline_s - at_s).abs() > 1e-6
    {
        return false;
    }
    true
}

fn marker_metadata_id(marker: &montage_proto::otio::Marker) -> Option<&str> {
    marker
        .metadata
        .montage
        .as_ref()
        .and_then(|metadata| metadata.extra.get("id"))
        .and_then(serde_json::Value::as_str)
}

fn apply_trim(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    new_start: Option<f64>,
    new_end: Option<f64>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-clip track child".into(),
        });
    };
    let Some(range) = clip.source_range.as_ref() else {
        return Err(ApplyError::Invalid {
            index,
            message: "clip has no source_range; cannot trim a clip with implicit range".into(),
        });
    };
    let link_group_id = clip_link_group_id(clip);
    let rate = range.start_time.rate;
    let original_start_s = range.start_time.to_seconds();
    let original_end_s = range.start_time.to_seconds() + range.duration.to_seconds();
    let target_start = new_start.unwrap_or(original_start_s);
    let target_end = new_end.unwrap_or(original_end_s);
    if target_end < target_start {
        return Err(ApplyError::Invalid {
            index,
            message: format!("trim: end {target_end} must be >= start {target_start}"),
        });
    }
    // If the agent asks for a trim that *expands* past the current
    // source range, reject with a clear hint. This happened in real
    // video runs: agent passed timeline-position seconds that
    // exceeded the post-trim source range, expecting an extension
    // of the clip; got a silently-different OTIO that round-tripped
    // wrong. Better to fail loud and tell them how to recover.
    if target_end > original_end_s + 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "trim: end {target_end:.3}s is past the current source range \
                 end {original_end_s:.3}s. Trim can only narrow a clip's source \
                 range, not extend it. If you want a longer clip, you may need \
                 to inspect_clip first to see the current source bounds, then \
                 trim within them."
            ),
        });
    }
    if target_start < original_start_s - 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "trim: start {target_start:.3}s is before the current source \
                 range start {original_start_s:.3}s. Trim can only narrow a \
                 clip's source range, not extend it backward. inspect_clip \
                 to see the current bounds before re-trimming."
            ),
        });
    }
    if (target_start - original_start_s).abs() < 1e-6 && (target_end - original_end_s).abs() < 1e-6
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "trim: requested range [{target_start:.3}s..{target_end:.3}s] is a no-op; \
                 the clip is already at that source range. view_timeline shows each clip's \
                 current source=[start..end]. To trim the first N seconds from the current \
                 visible clip, set start to current source start + N. To trim the last N \
                 seconds, set end to current source end - N."
            ),
        });
    }
    let new_dur = target_end - target_start;
    if new_dur < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("trim: computed negative duration {new_dur}"),
        });
    }
    clip.source_range = Some(montage_proto::otio::TimeRange::new(
        montage_proto::otio::RationalTime::new(target_start * rate, rate),
        montage_proto::otio::RationalTime::new(new_dur * rate, rate),
    ));
    let name = clip.name.clone();
    let mut linked_trim_count = 0usize;
    if let Some(group_id) = link_group_id.as_deref() {
        linked_trim_count = trim_linked_siblings(
            working,
            locator,
            group_id,
            target_start,
            target_end,
            original_start_s,
            original_end_s,
            index,
        )?;
    }
    let linked_note = if linked_trim_count == 0 {
        String::new()
    } else {
        format!(
            " ({} linked sibling{} also trimmed)",
            linked_trim_count,
            if linked_trim_count == 1 { "" } else { "s" }
        )
    };
    Ok(format!(
        "trimmed clip {name:?} to [{target_start:.3}s..{target_end:.3}s] ({new_dur:.3}s){linked_note}"
    ))
}

fn trim_linked_siblings(
    working: &mut Timeline,
    primary: ClipLocator,
    group_id: &str,
    target_start: f64,
    target_end: f64,
    original_start: f64,
    original_end: f64,
    index: usize,
) -> Result<usize, ApplyError> {
    let sibling_locators: Vec<_> = working
        .tracks
        .children
        .iter()
        .enumerate()
        .filter_map(|(track_index, stack_child)| {
            if track_index == primary.track_index {
                return None;
            }
            clip_with_link_group_covering_range(stack_child, group_id, original_start, original_end)
                .map(|child_index| ClipLocator {
                    track_index,
                    child_index,
                })
        })
        .collect();
    let mut count = 0usize;
    for locator in sibling_locators {
        let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
            continue;
        };
        let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
            continue;
        };
        let Some(range) = clip.source_range.as_ref() else {
            continue;
        };
        let rate = range.start_time.rate;
        if target_end < target_start {
            return Err(ApplyError::Invalid {
                index,
                message: format!("linked trim: end {target_end} must be >= start {target_start}"),
            });
        }
        let new_dur = target_end - target_start;
        clip.source_range = Some(montage_proto::otio::TimeRange::new(
            montage_proto::otio::RationalTime::new(target_start * rate, rate),
            montage_proto::otio::RationalTime::new(new_dur * rate, rate),
        ));
        count += 1;
    }
    Ok(count)
}

/// Reset / extend a previously-trimmed clip's source range outward.
/// Inverse-direction of `apply_trim`: Trim narrows; Untrim widens.
///
/// `new_start` and `new_end` default to the clip's *original* media
/// bounds when the media reference declares an `available_range`.
/// Without that bound we trust the agent's values (OTIO round-trip
/// validation catches structurally-bad widening) — defaulting to
/// `0.0` / `current_end` is the conservative choice in that case.
///
/// Errors when:
/// - The clip has no `source_range` (no current state to widen from).
/// - The new range would be narrower than the current one (use
///   `Trim Clip` for that — keeps the two ops semantically distinct
///   so the model can't accidentally narrow when it meant to widen).
/// - The new range is invalid (`end < start`).
fn apply_untrim(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    new_start: Option<f64>,
    new_end: Option<f64>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    use montage_proto::otio::{ExternalReference, MediaReference};
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-clip track child".into(),
        });
    };
    let link_group_id = clip_link_group_id(clip);
    let Some(range) = clip.source_range.as_ref() else {
        return Err(ApplyError::Invalid {
            index,
            message: "clip has no source_range; nothing to untrim. Set source_range via a \
                      previous Trim Clip first."
                .into(),
        });
    };
    let rate = range.start_time.rate;
    let cur_start_s = range.start_time.to_seconds();
    let cur_end_s = cur_start_s + range.duration.to_seconds();

    // Discover the media's outer bounds from `available_range` if
    // declared on the external reference. Falls back to trusting the
    // agent — the OTIO validator will reject anything truly broken.
    let available = match &clip.media_reference {
        MediaReference::External(ExternalReference {
            available_range, ..
        }) => available_range.as_ref(),
        _ => None,
    };
    let (avail_start_s, avail_end_s) = match available {
        Some(r) => (
            r.start_time.to_seconds(),
            r.start_time.to_seconds() + r.duration.to_seconds(),
        ),
        None => (0.0, f64::INFINITY),
    };

    // Defaults: omitted fields keep the *current* source_range value,
    // matching apply_trim's behavior. A previous version of this op
    // defaulted to the available_range bounds — which meant an Untrim
    // that only widens `end` would also reset `start` to 0, surprising
    // the agent (real-video run trace: "the untrim wiped the start
    // trim too"). Preserve-on-omit is the right contract.
    let target_start = new_start.unwrap_or(cur_start_s);
    let target_end = new_end.unwrap_or(cur_end_s);

    if target_end < target_start {
        return Err(ApplyError::Invalid {
            index,
            message: format!("untrim: end {target_end} must be >= start {target_start}"),
        });
    }

    // Refuse to NARROW via Untrim — if the agent wants to narrow,
    // they should use Trim Clip. Keeps the two ops distinct.
    if target_start > cur_start_s + 1e-6 || target_end < cur_end_s - 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "untrim: requested range [{target_start:.3}s..{target_end:.3}s] is \
                 narrower than current [{cur_start_s:.3}s..{cur_end_s:.3}s]. Untrim only \
                 widens; use Trim Clip to narrow."
            ),
        });
    }

    // Cap to available range when we know it.
    let final_start = target_start.max(avail_start_s);
    let final_end = if avail_end_s.is_finite() {
        target_end.min(avail_end_s)
    } else {
        target_end
    };

    let new_dur = final_end - final_start;
    if new_dur <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "untrim: computed non-positive duration {new_dur} after capping to \
                 available range"
            ),
        });
    }

    clip.source_range = Some(montage_proto::otio::TimeRange::new(
        montage_proto::otio::RationalTime::new(final_start * rate, rate),
        montage_proto::otio::RationalTime::new(new_dur * rate, rate),
    ));
    let name = clip.name.clone();

    // Widen link-group siblings (the audio half of an A/V pair) to the
    // same range, mirroring Trim Clip. Without this, restoring a cut
    // via Untrim leaves the linked audio still trimmed and the pair
    // out of sync (observed in the podcast-producer session trace).
    let mut linked_untrim_count = 0usize;
    if let Some(group_id) = link_group_id.as_deref() {
        linked_untrim_count = trim_linked_siblings(
            working,
            locator,
            group_id,
            final_start,
            final_end,
            cur_start_s,
            cur_end_s,
            index,
        )?;
    }
    let linked_note = if linked_untrim_count == 0 {
        String::new()
    } else {
        format!(
            " ({} linked sibling{} also untrimmed)",
            linked_untrim_count,
            if linked_untrim_count == 1 { "" } else { "s" }
        )
    };
    Ok(format!(
        "untrimmed clip {name:?} to [{final_start:.3}s..{final_end:.3}s] \
         ({new_dur:.3}s) — was [{cur_start_s:.3}s..{cur_end_s:.3}s]{linked_note}"
    ))
}

/// Split one clip into two at `at_s` seconds into the source media.
///
/// Insert a new clip on a (possibly fresh) track from an asset on
/// disk. This is the load-bearing op for *building* a timeline —
/// every other op mutates an existing one.
///
/// Behavior:
/// - If `track` doesn't exist, create it (Video kind by default) and
///   append it to the timeline's tracks.
/// - Auto-name the clip `clip-N` where N is the new child's index in
///   the track, unless `name` is supplied.
/// - Build an ExternalReference to `asset`.
/// - Default `start = 0.0`, `end = 1.0` if neither supplied (the
///   agent is expected to pass at least one — the OTIO validator
///   will reject zero-duration clips, so the model gets a clear
///   error if it forgot).
/// - Insert at `at_position` (clamped to `[0, len]`); default
///   `at_position = len` (append).
#[allow(clippy::too_many_arguments)]
fn apply_insert_clip(
    working: &mut Timeline,
    index: usize,
    asset: &str,
    track_name: &str,
    track_kind_hint: Option<InsertTrackKind>,
    at_position: Option<usize>,
    at_s: Option<f64>,
    start_s: Option<f64>,
    end_s: Option<f64>,
    name_override: Option<&str>,
    link_group_id: Option<&str>,
    snap: Option<&SnapOptions>,
) -> Result<String, ApplyError> {
    use montage_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, TimeRange, Track,
    };

    // Snap resolution runs against the un-mutated timeline so the
    // current track edges/markers are visible to the proposer.
    let snapped_at_s = match at_s {
        Some(requested) => Some(resolve_snap_time(working, index, requested, snap)?),
        None => None,
    };

    // Find or create the named track.
    let track_idx = working
        .tracks
        .children
        .iter()
        .enumerate()
        .find_map(|(i, sc)| match sc {
            StackChild::Track(t) if t.name == track_name => Some(i),
            _ => None,
        });
    let track_idx = match track_idx {
        Some(i) => i,
        None => {
            let kind = infer_insert_track_kind(track_name, track_kind_hint);
            let track = Track::empty(track_name.to_string(), kind);
            working.tracks.children.push(StackChild::Track(track));
            working.tracks.children.len() - 1
        }
    };

    // Determine the source range.
    let start = start_s.unwrap_or(0.0);
    let end = end_s.unwrap_or(start + 1.0);
    if end <= start {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "insert: end {end:.3}s must be > start {start:.3}s. \
                 Pass `+ start: <s>` and `+ end: <s>` (in source-media \
                 seconds). Use inspect_clip on the asset first to see \
                 its duration."
            ),
        });
    }
    if let Some(at_s) = snapped_at_s {
        if !at_s.is_finite() || at_s < 0.0 {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "insert: at_s must be a finite non-negative timeline time, got {at_s}"
                ),
            });
        }
    }
    let rate = 24.0_f64;
    let source_range = TimeRange::new(
        RationalTime::new(start * rate, rate),
        RationalTime::new((end - start) * rate, rate),
    );

    let linked_video_start_s = link_group_id.and_then(|id| {
        let StackChild::Track(track) = &working.tracks.children[track_idx] else {
            return None;
        };
        if at_position.is_none() && snapped_at_s.is_none() && matches!(track.kind, TrackKind::Audio)
        {
            linked_clip_track_time(working, id, TrackKind::Video)
        } else {
            None
        }
    });

    let StackChild::Track(track) = &mut working.tracks.children[track_idx] else {
        return Err(ApplyError::Invalid {
            index,
            message: "track index resolved to a non-track stack child".into(),
        });
    };

    // Timeline-time placement takes precedence over `at_position`.
    // Walk existing children to find the child index whose start matches
    // the requested cursor; pad with a gap when the cursor extends
    // past the track's end. Anchors that land mid-clip fall back to
    // the nearest boundary at or before the cursor (the agent is
    // expected to snap to a true edge — mid-clip inserts would
    // otherwise require a split first, which this op does not do).
    let timeline_pos = if let Some(target_time_s) = snapped_at_s {
        let mut cursor_s = 0.0_f64;
        let mut position = track.children.len();
        for (idx, child) in track.children.iter().enumerate() {
            if (cursor_s - target_time_s).abs() <= 0.001 {
                position = idx;
                break;
            }
            cursor_s += child_duration(child);
        }
        // Past the end → pad with a gap so the clip lands exactly at
        // `target_time_s`.
        if position == track.children.len() && cursor_s + 0.001 < target_time_s {
            track
                .children
                .push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                    target_time_s - cursor_s,
                    rate,
                )));
            position = track.children.len();
        }
        Some(position)
    } else {
        None
    };

    if timeline_pos.is_none()
        && let Some(target_time_s) = linked_video_start_s
    {
        let cursor_s = track_cursor(track);
        if cursor_s + 0.001 < target_time_s {
            track
                .children
                .push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                    target_time_s - cursor_s,
                    rate,
                )));
        }
    }

    // Build the clip.
    let position = timeline_pos
        .or(at_position)
        .unwrap_or(track.children.len())
        .min(track.children.len());
    let chosen_name = name_override
        .map(str::to_string)
        .unwrap_or_else(|| default_clip_name_for_asset(track, asset));
    let mut clip = Clip::empty(chosen_name.clone());
    clip.media_reference = MediaReference::External(ExternalReference::new(asset.to_string()));
    clip.source_range = Some(source_range);
    // Stamp a fresh anchor uuid so Step 8's drag-to-trim can build
    // `Anchor::ClipUuid { uuid }` against this clip without falling
    // back to name-based matching (which works but is brittle when
    // two inserts pick the same default name).
    stamp_fresh_clip_uuid(&mut clip);
    if let Some(link_group_id) = link_group_id {
        stamp_link_group_id(&mut clip, link_group_id);
    }

    track.children.insert(position, TrackChild::Clip(clip));

    Ok(format!(
        "inserted clip {chosen_name:?} on track {track_name:?} at \
         position {position}: asset={asset:?} source=[{start:.3}s..{end:.3}s] \
         ({:.3}s)",
        end - start
    ))
}

fn infer_insert_track_kind(track_name: &str, hint: Option<InsertTrackKind>) -> TrackKind {
    match hint {
        Some(InsertTrackKind::Video) => TrackKind::Video,
        Some(InsertTrackKind::Audio) => TrackKind::Audio,
        Some(InsertTrackKind::Auto) | None => {
            let name = track_name.trim().to_ascii_lowercase();
            if name == "a1"
                || name.starts_with("a ")
                || name.starts_with("a-")
                || name.starts_with("audio")
                || name.starts_with("music")
            {
                TrackKind::Audio
            } else {
                TrackKind::Video
            }
        }
    }
}

/// Stamp a fresh, process-unique clip uuid into `clip.metadata.montage
/// .extra["clip_uuid"]`. Used by InsertClip (new clip created) and
/// Split (right piece needs its own uuid — clip.clone() inherited
/// the parent's, which breaks Anchor::ClipUuid resolution because
/// two clips would share the same uuid).
///
/// Uniqueness is "monotonic counter + nanosecond timestamp formatted
/// as a 16-char hex string" — short, stable across cargo build IDs,
/// and uniqueness-by-construction across rapid back-to-back calls.
/// Not cryptographically random; we don't need that here.
fn stamp_fresh_clip_uuid(clip: &mut Clip) {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64;
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let uuid = format!("c-{:013x}{:03x}", nanos & 0xFFFF_FFFF_FFFF_F, seq & 0xFFF);
    stamp_clip_uuid(clip, &uuid);
}

fn stamp_clip_uuid(clip: &mut Clip, uuid: &str) {
    let montage = clip
        .metadata
        .montage
        .get_or_insert_with(MontageClipMetadata::default);
    montage
        .extra
        .insert("clip_uuid".into(), serde_json::Value::String(uuid.into()));
}

fn stamp_link_group_id(clip: &mut Clip, link_group_id: &str) {
    let montage = clip
        .metadata
        .montage
        .get_or_insert_with(MontageClipMetadata::default);
    montage.extra.insert(
        "link_group_id".into(),
        serde_json::Value::String(link_group_id.to_string()),
    );
}

fn keep_only_split_edit_lead(clip: &mut Clip) {
    let Some(montage) = clip.metadata.montage.as_mut() else {
        return;
    };
    let Some(split_edit) = montage.split_edit.as_mut() else {
        return;
    };
    split_edit.audio_trail_s = None;
    split_edit.audio_trail_to_clip_id = None;
    if split_edit.audio_lead_s.is_none() {
        montage.split_edit = None;
    }
}

fn keep_only_split_edit_trail(clip: &mut Clip) {
    let Some(montage) = clip.metadata.montage.as_mut() else {
        return;
    };
    let Some(split_edit) = montage.split_edit.as_mut() else {
        return;
    };
    split_edit.audio_lead_s = None;
    split_edit.audio_lead_from_clip_id = None;
    if split_edit.audio_trail_s.is_none() {
        montage.split_edit = None;
    }
}

fn linked_clip_track_time(
    timeline: &Timeline,
    link_group_id: &str,
    kind: TrackKind,
) -> Option<f64> {
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        if track.kind != kind {
            continue;
        }
        let mut cursor_s = 0.0;
        for child in &track.children {
            if let TrackChild::Clip(clip) = child
                && clip_link_group_id(clip).as_deref() == Some(link_group_id)
            {
                return Some(cursor_s);
            }
            cursor_s += child_duration(child);
        }
    }
    None
}

fn clip_link_group_id(clip: &Clip) -> Option<String> {
    clip.metadata
        .montage
        .as_ref()
        .and_then(|m| m.extra.get("link_group_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn clip_at_locator(timeline: &Timeline, locator: ClipLocator) -> Option<&Clip> {
    let StackChild::Track(track) = timeline.tracks.children.get(locator.track_index)? else {
        return None;
    };
    let TrackChild::Clip(clip) = track.children.get(locator.child_index)? else {
        return None;
    };
    Some(clip)
}

fn clip_source_range_s(clip: &Clip) -> Option<(f64, f64)> {
    let range = clip.source_range.as_ref()?;
    let start_s = range.start_time.to_seconds();
    Some((start_s, start_s + range.duration.to_seconds()))
}

fn clip_contains_source_time(clip: &Clip, at_s: f64) -> bool {
    clip_source_range_s(clip).is_some_and(|(start_s, end_s)| at_s > start_s && at_s < end_s)
}

fn clip_covers_source_range(clip: &Clip, start_s: f64, end_s: f64) -> bool {
    clip_source_range_s(clip).is_some_and(|(clip_start_s, clip_end_s)| {
        start_s >= clip_start_s - 1e-6 && end_s <= clip_end_s + 1e-6
    })
}

fn clip_with_link_group_containing(
    stack_child: &StackChild,
    group_id: &str,
    at_s: f64,
) -> Option<usize> {
    let StackChild::Track(track) = stack_child else {
        return None;
    };
    track.children.iter().position(|child| {
        let TrackChild::Clip(clip) = child else {
            return false;
        };
        clip_link_group_id(clip).as_deref() == Some(group_id)
            && clip_contains_source_time(clip, at_s)
    })
}

fn clip_with_link_group_covering_range(
    stack_child: &StackChild,
    group_id: &str,
    start_s: f64,
    end_s: f64,
) -> Option<usize> {
    let StackChild::Track(track) = stack_child else {
        return None;
    };
    track.children.iter().position(|child| {
        let TrackChild::Clip(clip) = child else {
            return false;
        };
        clip_link_group_id(clip).as_deref() == Some(group_id)
            && clip_covers_source_range(clip, start_s, end_s)
    })
}

fn clip_uuid(clip: &Clip) -> Option<&str> {
    clip.metadata
        .montage
        .as_ref()
        .and_then(|m| m.extra.get("clip_uuid"))
        .and_then(|v| v.as_str())
}

fn next_clip_name_in_track(track: &montage_proto::otio::Track) -> String {
    let used = track
        .children
        .iter()
        .filter_map(|tc| match tc {
            TrackChild::Clip(clip) => Some(clip.name.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    for i in 0usize.. {
        let candidate = format!("clip-{i}");
        if !used.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!("usize iterator is unbounded")
}

/// Default name for a newly-inserted clip. Prefer the source asset's
/// file stem (e.g. `raw/copy_F65206FA-....MOV` → `copy_F65206FA-...`),
/// disambiguating with a `-2`, `-3`, ... suffix when the track already
/// has clips referencing the same asset. Falls back to the legacy
/// `clip-{i}` naming when no usable stem can be derived (e.g. a URL
/// scheme with no path, or an empty asset string).
fn default_clip_name_for_asset(track: &montage_proto::otio::Track, asset: &str) -> String {
    let stem = std::path::Path::new(asset)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let Some(stem) = stem else {
        return next_clip_name_in_track(track);
    };
    let used = track
        .children
        .iter()
        .filter_map(|tc| match tc {
            TrackChild::Clip(clip) => Some(clip.name.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    if !used.contains(stem) {
        return stem.to_string();
    }
    for i in 2usize.. {
        let candidate = format!("{stem}-{i}");
        if !used.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!("usize iterator is unbounded")
}

/// Both pieces share the original media reference; the left piece
/// keeps the original name and metadata, the right piece gets a
/// `<name>-b` suffix so the agent can anchor each independently
/// afterward (e.g. `Delete Clip` on the new b-piece).
///
/// Errors when:
/// - The clip has no `source_range` (we can't compute a partition).
/// - `at_s` is outside `[start, end)` of the source range.
fn apply_split(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    at_s: f64,
    snap: Option<&SnapOptions>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    // Compute the timeline-time projection of `at_s` (source-time)
    // before borrowing the track mutably so we can run the snap
    // proposer against the whole timeline.
    let projected_timeline_s = {
        let StackChild::Track(track) = &working.tracks.children[locator.track_index] else {
            return Err(ApplyError::Invalid {
                index,
                message: "anchor resolved to a non-track stack child".into(),
            });
        };
        let TrackChild::Clip(clip) = &track.children[locator.child_index] else {
            return Err(ApplyError::Invalid {
                index,
                message: "anchor resolved to a non-clip track child".into(),
            });
        };
        let Some(range) = clip.source_range.as_ref() else {
            return Err(ApplyError::Invalid {
                index,
                message: "clip has no source_range; cannot split a clip with implicit range".into(),
            });
        };
        let clip_timeline_start_s: f64 = track.children[..locator.child_index]
            .iter()
            .map(child_duration)
            .sum();
        let source_start_s = range.start_time.to_seconds();
        clip_timeline_start_s + (at_s - source_start_s)
    };
    // Run snap on the timeline-time projection, then translate the
    // snapped timeline-time back to a source-time so the rest of the
    // split logic can stay in source coordinates.
    let snapped_at_s = if snap.is_some() {
        let snapped_timeline_s = resolve_snap_time(working, index, projected_timeline_s, snap)?;
        at_s + (snapped_timeline_s - projected_timeline_s)
    } else {
        at_s
    };
    let at_s = snapped_at_s;
    let link_group_id = clip_at_locator(working, locator).and_then(clip_link_group_id);
    let split = split_clip_at_locator(working, index, locator, at_s)?;
    let mut linked_split_count = 0usize;
    if let Some(group_id) = link_group_id.as_deref() {
        linked_split_count = split_linked_siblings(working, locator, group_id, at_s, index)?;
    }
    let linked_note = if linked_split_count == 0 {
        String::new()
    } else {
        format!(
            " ({} linked sibling{} also split)",
            linked_split_count,
            if linked_split_count == 1 { "" } else { "s" }
        )
    };
    Ok(format!(
        "split clip {original_name:?} at {at_s:.3}s → {original_name:?} \
         [{start_s:.3}s..{at_s:.3}s] + {right_name:?}{right_anchor} \
         [{at_s:.3}s..{end_s:.3}s]{linked_note}",
        original_name = split.original_name,
        right_name = split.right_name,
        right_anchor = split.right_anchor,
        start_s = split.start_s,
        end_s = split.end_s,
    ))
}

struct SplitResult {
    original_name: String,
    right_name: String,
    right_anchor: String,
    start_s: f64,
    end_s: f64,
}

fn split_clip_at_locator(
    working: &mut Timeline,
    index: usize,
    locator: ClipLocator,
    at_s: f64,
) -> Result<SplitResult, ApplyError> {
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-clip track child".into(),
        });
    };
    let Some(range) = clip.source_range.clone() else {
        return Err(ApplyError::Invalid {
            index,
            message: "clip has no source_range; cannot split a clip with implicit range".into(),
        });
    };
    let rate = range.start_time.rate;
    let start_s = range.start_time.to_seconds();
    let end_s = start_s + range.duration.to_seconds();
    if at_s <= start_s || at_s >= end_s {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "split: at_s {at_s} must lie strictly inside source range [{start_s}, {end_s})"
            ),
        });
    }

    // Build the right piece (clone before we mutate the left in
    // place, so we still have access to the original media ref +
    // metadata).
    let original_name = clip.name.clone();
    let original_clip_id = clip_uuid(clip).unwrap_or(clip.name.as_str()).to_string();
    let mut right = clip.clone();
    right.name = format!("{original_name}-b");
    right.source_range = Some(montage_proto::otio::TimeRange::new(
        montage_proto::otio::RationalTime::new(at_s * rate, rate),
        montage_proto::otio::RationalTime::new((end_s - at_s) * rate, rate),
    ));
    keep_only_split_edit_trail(&mut right);
    // The clip.clone() above also cloned the parent's clip_uuid —
    // which would mean two clips share the same anchor uuid after
    // this op. Stamp a fresh one so Anchor::ClipUuid resolves
    // unambiguously to whichever piece the agent (or user) names.
    stamp_fresh_clip_uuid(&mut right);
    let right_uuid = clip_uuid(&right).map(str::to_string);

    // Trim the left piece in place.
    let TrackChild::Clip(left) = &mut track.children[locator.child_index] else {
        // Already type-checked above; this branch can't actually fire.
        return Err(ApplyError::Invalid {
            index,
            message: "split: left piece type-check vanished mid-op".into(),
        });
    };
    left.source_range = Some(montage_proto::otio::TimeRange::new(
        montage_proto::otio::RationalTime::new(start_s * rate, rate),
        montage_proto::otio::RationalTime::new((at_s - start_s) * rate, rate),
    ));
    keep_only_split_edit_lead(left);

    // Insert the right piece directly after the left.
    track
        .children
        .insert(locator.child_index + 1, TrackChild::Clip(right));
    if let Some(right_uuid) = right_uuid.as_deref() {
        transfer_outgoing_cut_boundary_to_split_right(working, &original_clip_id, right_uuid);
    }

    let right_name = format!("{original_name}-b");
    let right_anchor = right_uuid
        .as_deref()
        .map(|uuid| format!(" anchor=clip_uuid={uuid}"))
        .unwrap_or_default();
    Ok(SplitResult {
        original_name,
        right_name,
        right_anchor,
        start_s,
        end_s,
    })
}

fn split_linked_siblings(
    working: &mut Timeline,
    primary: ClipLocator,
    group_id: &str,
    at_s: f64,
    index: usize,
) -> Result<usize, ApplyError> {
    let sibling_locators: Vec<_> = working
        .tracks
        .children
        .iter()
        .enumerate()
        .filter_map(|(track_index, stack_child)| {
            if track_index == primary.track_index {
                return None;
            }
            clip_with_link_group_containing(stack_child, group_id, at_s).map(|child_index| {
                ClipLocator {
                    track_index,
                    child_index,
                }
            })
        })
        .collect();
    let mut count = 0usize;
    for locator in sibling_locators {
        split_clip_at_locator(working, index, locator, at_s)?;
        count += 1;
    }
    Ok(count)
}

fn apply_delete(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "anchor resolved to a non-track stack child".into(),
        });
    };
    let removed_duration_s = child_duration(&track.children[locator.child_index]);
    let gap_rate = track.children[locator.child_index]
        .as_clip_rate()
        .unwrap_or(24.0);
    let removed = std::mem::replace(
        &mut track.children[locator.child_index],
        TrackChild::Gap(montage_proto::otio::Gap::of_duration(
            removed_duration_s,
            gap_rate,
        )),
    );
    let (name, removed_clip_id) = match &removed {
        TrackChild::Clip(c) => (
            c.name.clone(),
            Some(clip_uuid(c).unwrap_or(c.name.as_str()).to_string()),
        ),
        _ => ("<non-clip>".to_string(), None),
    };
    let removed_transitions = remove_transitions_around_child(track, locator.child_index);
    merge_adjacent_gaps(&mut track.children, gap_rate);
    if let Some(removed_clip_id) = removed_clip_id {
        remove_cut_boundaries_for_clip(working, &removed_clip_id);
    }
    if removed_transitions == 0 {
        Ok(format!("deleted clip {name:?}"))
    } else {
        Ok(format!(
            "deleted clip {name:?} and removed {removed_transitions} adjacent transition(s)"
        ))
    }
}

fn remove_transitions_around_child(track: &mut montage_proto::otio::Track, index: usize) -> usize {
    let mut removed = 0;
    if matches!(
        track.children.get(index + 1),
        Some(TrackChild::Transition(_))
    ) {
        track.children.remove(index + 1);
        removed += 1;
    }
    if index > 0
        && matches!(
            track.children.get(index - 1),
            Some(TrackChild::Transition(_))
        )
    {
        track.children.remove(index - 1);
        removed += 1;
    }
    removed
}

fn remove_cut_boundaries_for_clip(timeline: &mut Timeline, clip_id: &str) {
    let Some(meta) = timeline.metadata.montage.as_mut() else {
        return;
    };
    meta.cut_boundaries
        .retain(|key, _| !cut_boundary_key_references_clip(key, clip_id));
}

fn cut_boundary_key_references_clip(key: &str, clip_id: &str) -> bool {
    let Some((from, to)) = key.split_once("::") else {
        return false;
    };
    from == clip_id || to == clip_id
}

fn transfer_outgoing_cut_boundary_to_split_right(
    timeline: &mut Timeline,
    original_clip_id: &str,
    right_clip_id: &str,
) {
    let Some(meta) = timeline.metadata.montage.as_mut() else {
        return;
    };
    let transfers = meta
        .cut_boundaries
        .iter()
        .filter_map(|(key, spec)| {
            let (from_id, to_id) = key.split_once("::")?;
            (from_id == original_clip_id).then(|| {
                (
                    key.clone(),
                    cut_boundary_key(right_clip_id, to_id),
                    spec.clone(),
                )
            })
        })
        .collect::<Vec<_>>();
    for (old_key, new_key, spec) in transfers {
        meta.cut_boundaries.remove(&old_key);
        meta.cut_boundaries.insert(new_key, spec);
    }
}

fn prune_stale_cut_boundaries(timeline: &mut Timeline) {
    let valid_boundaries = current_cut_boundary_keys(timeline);
    let Some(meta) = timeline.metadata.montage.as_mut() else {
        return;
    };
    meta.cut_boundaries
        .retain(|key, _| valid_boundaries.contains(key));
}

fn current_cut_boundary_keys(timeline: &Timeline) -> HashSet<String> {
    let mut keys = HashSet::new();
    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        for (index, child) in track.children.iter().enumerate() {
            let Some(from_id) = track_child_clip_id(child) else {
                continue;
            };
            match track.children.get(index + 1) {
                Some(next @ TrackChild::Clip(_)) => {
                    if let Some(to_id) = track_child_clip_id(next) {
                        keys.insert(cut_boundary_key(&from_id, &to_id));
                    }
                }
                Some(TrackChild::Transition(_)) => {
                    if let Some(to_id) = track.children.get(index + 2).and_then(track_child_clip_id)
                    {
                        keys.insert(cut_boundary_key(&from_id, &to_id));
                    }
                }
                Some(TrackChild::Gap(_)) => {
                    if let Some(to_id) = next_clip_id_after_gaps(track, index) {
                        keys.insert(cut_boundary_key(&from_id, &to_id));
                    }
                }
                _ => {}
            }
        }
    }
    keys
}

fn prune_stale_split_edits(timeline: &mut Timeline) {
    for stack_child in &mut timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        for index in 0..track.children.len() {
            let previous_id = previous_clip_id(track, index);
            let next_id = next_clip_id(track, index);
            let Some(TrackChild::Clip(clip)) = track.children.get_mut(index) else {
                continue;
            };
            prune_split_edit_for_clip(clip, previous_id.as_deref(), next_id.as_deref());
        }
    }
}

fn prune_split_edit_for_clip(clip: &mut Clip, previous_id: Option<&str>, next_id: Option<&str>) {
    let Some(montage) = clip.metadata.montage.as_mut() else {
        return;
    };
    let Some(split_edit) = montage.split_edit.as_mut() else {
        return;
    };

    if split_edit.audio_lead_s.is_some() {
        let lead_still_matches = split_edit
            .audio_lead_from_clip_id
            .as_deref()
            .map_or(previous_id.is_some(), |expected| {
                previous_id == Some(expected)
            });
        if !lead_still_matches {
            split_edit.audio_lead_s = None;
            split_edit.audio_lead_from_clip_id = None;
        }
    }

    if split_edit.audio_trail_s.is_some() {
        let trail_still_matches = split_edit
            .audio_trail_to_clip_id
            .as_deref()
            .map_or(next_id.is_some(), |expected| next_id == Some(expected));
        if !trail_still_matches {
            split_edit.audio_trail_s = None;
            split_edit.audio_trail_to_clip_id = None;
        }
    }

    if split_edit.audio_lead_s.is_none() && split_edit.audio_trail_s.is_none() {
        montage.split_edit = None;
    }
}

/// Insert a `Transition` node between two adjacent clips on the same
/// track. The transition straddles the cut at `from`'s end /
/// `to`'s start; the render pipeline interprets this as an xfade-style
/// overlap (Step 14.5).
///
/// Validation:
/// - both anchors must resolve
/// - both must be on the *same* track
/// - they must be adjacent clips, or separated only by an existing
///   transition node that this operation replaces
/// - `duration_s > 0`
fn apply_insert_transition(
    working: &mut Timeline,
    index: usize,
    between: &super::op::TransitionBetween,
    kind: &str,
    duration_s: f64,
    alignment: Option<TransitionAlignment>,
    in_offset_s: Option<f64>,
    out_offset_s: Option<f64>,
    spec: Option<&montage_proto::transitions::SemanticTransitionSpec>,
    ctx: &AnchorContext,
) -> Result<String, ApplyError> {
    use montage_proto::otio::{RationalTime, Transition};

    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("transition duration {duration_s} must be > 0"),
        });
    }
    montage_proto::transitions::validate_transition_duration(kind, duration_s).map_err(|e| {
        ApplyError::Invalid {
            index,
            message: e.to_string(),
        }
    })?;
    let xfade = montage_proto::transitions::resolve_ffmpeg_xfade(kind)
        .map_err(|e| ApplyError::Invalid {
            index,
            message: format!("transition {kind:?} is not supported by the phase-one renderer: {e}"),
        })?
        .ok_or_else(|| ApplyError::Invalid {
            index,
            message: format!("transition {kind:?} is semantic-only and cannot be inserted"),
        })?;
    if !is_authorable_transition_kind(kind) {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "transition {kind:?} is a raw FFmpeg transition name. New EDLs must use a registered montage.* transition id or SMPTE_Dissolve."
            ),
        });
    }
    if kind == "montage.composite" && spec.is_none() {
        return Err(ApplyError::Invalid {
            index,
            message:
                "transition \"montage.composite\" requires semantic metadata with composition_json"
                    .into(),
        });
    }
    if let Some(spec) = spec {
        if spec.id != kind {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "transition spec id {:?} must match kind {:?}; use one stable transition id",
                    spec.id, kind
                ),
            });
        }
        let spec_xfade =
            montage_proto::transitions::resolve_ffmpeg_xfade(&spec.id).map_err(|e| {
                ApplyError::Invalid {
                    index,
                    message: format!("transition spec for {:?} is invalid: {e}", spec.id),
                }
            })?;
        if spec_xfade != Some(xfade) {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "transition spec id {:?} resolves to {:?}, but kind {:?} resolves to {:?}",
                    spec.id, spec_xfade, kind, xfade
                ),
            });
        }
        montage_proto::transitions::validate_semantic_transition_spec(spec).map_err(|e| {
            ApplyError::Invalid {
                index,
                message: format!("transition spec for {:?} is invalid: {e}", spec.id),
            }
        })?;
    }
    let persisted_spec = spec.cloned().map(|mut spec| {
        if spec.composition.is_none() {
            spec.composition = montage_proto::transitions::builtin_transition_composition(&spec.id);
        }
        spec
    });
    let (in_offset_s, out_offset_s) =
        resolve_transition_offsets(index, duration_s, alignment, in_offset_s, out_offset_s)?;

    let from_loc = resolve(working, &between.from, ctx)
        .map_err(|miss| ApplyError::AnchorMiss { index, miss })?;
    let to_loc = resolve(working, &between.to, ctx)
        .map_err(|miss| ApplyError::AnchorMiss { index, miss })?;

    if from_loc.track_index != to_loc.track_index {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "transition: anchors resolve to different tracks \
                 ({} vs {}). Both clips must live on the same track.",
                from_loc.track_index, to_loc.track_index,
            ),
        });
    }
    let replaces_existing_transition = to_loc.child_index == from_loc.child_index + 2
        && matches!(
            working.tracks.children.get(from_loc.track_index),
            Some(StackChild::Track(track))
                if matches!(
                    track.children.get(from_loc.child_index + 1),
                    Some(TrackChild::Transition(_))
                )
        );
    if to_loc.child_index != from_loc.child_index + 1 && !replaces_existing_transition {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "transition: anchors are not adjacent (from at \
                 index {}, to at index {}). Transitions insert between \
                 consecutive clips, or replace an existing transition \
                 between the same clips.",
                from_loc.child_index, to_loc.child_index,
            ),
        });
    }

    let StackChild::Track(track) = &working.tracks.children[from_loc.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "transition: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(from_clip) = &track.children[from_loc.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "transition: from anchor resolved to a non-clip child".into(),
        });
    };
    let TrackChild::Clip(to_clip) = &track.children[to_loc.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "transition: to anchor resolved to a non-clip child".into(),
        });
    };
    validate_transition_handle(index, kind, from_clip, "outgoing", out_offset_s)?;
    validate_transition_handle(index, kind, to_clip, "incoming", in_offset_s)?;

    // Pull the rate off the from-clip's source_range so the transition's
    // RationalTime offsets share a denominator with the surrounding
    // clips (avoids round-trip drift on save).
    let rate = from_clip
        .source_range
        .as_ref()
        .map(|r| r.start_time.rate)
        .unwrap_or(24.0);

    let mut transition = Transition {
        name: String::new(),
        transition_type: kind.to_string(),
        in_offset: RationalTime::new(in_offset_s * rate, rate),
        out_offset: RationalTime::new(out_offset_s * rate, rate),
        metadata: std::collections::HashMap::new(),
    };
    if let Some(spec) = persisted_spec.as_ref() {
        transition.metadata.insert(
            "montage_transition".into(),
            serde_json::to_value(spec).map_err(|e| ApplyError::Invalid {
                index,
                message: format!("transition spec could not be serialized: {e}"),
            })?,
        );
    }

    let StackChild::Track(track) = &mut working.tracks.children[from_loc.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "transition: anchor resolved to a non-track stack child".into(),
        });
    };
    if replaces_existing_transition {
        track.children.remove(from_loc.child_index + 1);
    }
    track
        .children
        .insert(from_loc.child_index + 1, TrackChild::Transition(transition));

    let action = if replaces_existing_transition {
        "replaced"
    } else {
        "inserted"
    };
    Ok(format!(
        "{action} transition {kind:?} ({duration_s:.3}s, in={in_offset_s:.3}s, out={out_offset_s:.3}s) between \
         clips at indices {} and {} on track {}",
        from_loc.child_index,
        from_loc.child_index + 2, // shifted by the insert
        from_loc.track_index,
    ))
}

fn is_authorable_transition_kind(kind: &str) -> bool {
    kind == "SMPTE_Dissolve" || kind.starts_with("montage.")
}

fn apply_delete_transition(
    working: &mut Timeline,
    index: usize,
    between: &super::op::TransitionBetween,
    ctx: &AnchorContext,
) -> Result<String, ApplyError> {
    let from_loc = resolve(working, &between.from, ctx)
        .map_err(|miss| ApplyError::AnchorMiss { index, miss })?;
    let to_loc = resolve(working, &between.to, ctx)
        .map_err(|miss| ApplyError::AnchorMiss { index, miss })?;
    if from_loc.track_index != to_loc.track_index {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "delete_transition: anchors resolve to different tracks ({} vs {})",
                from_loc.track_index, to_loc.track_index
            ),
        });
    }
    if to_loc.child_index != from_loc.child_index + 2 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "delete_transition: expected clips separated by one transition, got indices {} and {}",
                from_loc.child_index, to_loc.child_index
            ),
        });
    }
    let StackChild::Track(track) = &mut working.tracks.children[from_loc.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "delete_transition: anchor resolved to a non-track stack child".into(),
        });
    };
    if !matches!(
        track.children.get(from_loc.child_index + 1),
        Some(TrackChild::Transition(_))
    ) {
        return Err(ApplyError::Invalid {
            index,
            message: "delete_transition: no transition exists between those clips".into(),
        });
    }
    track.children.remove(from_loc.child_index + 1);
    Ok(format!(
        "deleted transition between clips at indices {} and {} on track {}",
        from_loc.child_index,
        from_loc.child_index + 2,
        from_loc.track_index
    ))
}

fn apply_set_cut_intent(
    working: &mut Timeline,
    index: usize,
    between: &super::op::TransitionBetween,
    spec: &SemanticCutSpec,
    ctx: &AnchorContext,
) -> Result<String, ApplyError> {
    validate_semantic_cut_spec(index, spec)?;
    let from_loc = resolve(working, &between.from, ctx)
        .map_err(|miss| ApplyError::AnchorMiss { index, miss })?;
    let to_loc = resolve(working, &between.to, ctx)
        .map_err(|miss| ApplyError::AnchorMiss { index, miss })?;
    if from_loc.track_index != to_loc.track_index {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "set_cut_intent: anchors resolve to different tracks ({} vs {})",
                from_loc.track_index, to_loc.track_index
            ),
        });
    }
    let separated_by_transition = to_loc.child_index == from_loc.child_index + 2
        && matches!(
            working.tracks.children.get(from_loc.track_index),
            Some(StackChild::Track(track))
                if matches!(
                    track.children.get(from_loc.child_index + 1),
                    Some(TrackChild::Transition(_))
                )
        );
    let separated_by_lifted_gap = matches!(
        working.tracks.children.get(from_loc.track_index),
        Some(StackChild::Track(track))
            if clips_are_separated_only_by_gaps(track, from_loc.child_index, to_loc.child_index)
    );
    if to_loc.child_index != from_loc.child_index + 1
        && !separated_by_transition
        && !separated_by_lifted_gap
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "set_cut_intent: anchors are not adjacent boundary clips (from at index {}, to at index {})",
                from_loc.child_index, to_loc.child_index
            ),
        });
    }

    let StackChild::Track(track) = &working.tracks.children[from_loc.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_cut_intent: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(from_clip) = &track.children[from_loc.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_cut_intent: from anchor resolved to a non-clip child".into(),
        });
    };
    let TrackChild::Clip(to_clip) = &track.children[to_loc.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_cut_intent: to anchor resolved to a non-clip child".into(),
        });
    };
    let from_id = clip_uuid(from_clip)
        .unwrap_or(from_clip.name.as_str())
        .to_string();
    let to_id = clip_uuid(to_clip)
        .unwrap_or(to_clip.name.as_str())
        .to_string();
    let key = cut_boundary_key(&from_id, &to_id);
    timeline_montage_metadata(working)
        .cut_boundaries
        .insert(key.clone(), spec.clone());
    Ok(format!(
        "set cut intent {:?} ({:?}) for boundary {key}",
        spec.cut_type, spec.audio_relation
    ))
}

fn clips_are_separated_only_by_gaps(
    track: &montage_proto::otio::Track,
    from_index: usize,
    to_index: usize,
) -> bool {
    to_index > from_index + 1
        && track.children[from_index + 1..to_index]
            .iter()
            .all(|child| matches!(child, TrackChild::Gap(_)))
}

fn validate_semantic_cut_spec(index: usize, spec: &SemanticCutSpec) -> Result<(), ApplyError> {
    if spec.intent.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "set_cut_intent: intent must be non-empty".into(),
        });
    }
    if let Some(energy) = spec.energy
        && (!energy.is_finite() || !(0.0..=1.0).contains(&energy))
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_cut_intent: energy {energy} must be in [0, 1]"),
        });
    }
    if let Some(confidence) = spec.confidence
        && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_cut_intent: confidence {confidence} must be in [0, 1]"),
        });
    }
    Ok(())
}

fn resolve_transition_offsets(
    index: usize,
    duration_s: f64,
    alignment: Option<TransitionAlignment>,
    in_offset_s: Option<f64>,
    out_offset_s: Option<f64>,
) -> Result<(f64, f64), ApplyError> {
    let validate = |name: &str, value: f64| {
        if !value.is_finite() || value < 0.0 {
            Err(ApplyError::Invalid {
                index,
                message: format!("transition {name} {value} must be finite and >= 0"),
            })
        } else {
            Ok(())
        }
    };
    if let Some(v) = in_offset_s {
        validate("in_offset_s", v)?;
    }
    if let Some(v) = out_offset_s {
        validate("out_offset_s", v)?;
    }
    let (in_s, out_s) = match (in_offset_s, out_offset_s) {
        (Some(in_s), Some(out_s)) => (in_s, out_s),
        (Some(in_s), None) => (in_s, duration_s - in_s),
        (None, Some(out_s)) => (duration_s - out_s, out_s),
        (None, None) => match alignment.unwrap_or(TransitionAlignment::Center) {
            TransitionAlignment::Center => (duration_s / 2.0, duration_s / 2.0),
            TransitionAlignment::StartAtCut => (0.0, duration_s),
            TransitionAlignment::EndAtCut => (duration_s, 0.0),
        },
    };
    validate("in_offset_s", in_s)?;
    validate("out_offset_s", out_s)?;
    if (in_s + out_s - duration_s).abs() > 1e-6 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "transition offsets must sum to duration_s ({duration_s:.3}); got in_offset_s={in_s:.3}, out_offset_s={out_s:.3}"
            ),
        });
    }
    Ok((in_s, out_s))
}

fn validate_transition_handle(
    index: usize,
    kind: &str,
    clip: &Clip,
    side: &'static str,
    needed_s: f64,
) -> Result<(), ApplyError> {
    use montage_proto::otio::{ExternalReference, MediaReference};
    if needed_s <= 0.0 {
        return Ok(());
    }
    let Some(range) = clip.source_range.as_ref() else {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "transition {kind:?} around clip {:?} needs {needed_s:.3}s {side} handle, but the clip has no source_range",
                clip.name
            ),
        });
    };
    let start_s = range.start_time.to_seconds();
    let end_s = start_s + range.duration.to_seconds();
    let available = match &clip.media_reference {
        MediaReference::External(ExternalReference {
            available_range, ..
        }) => available_range.as_ref(),
        _ => None,
    };
    let available_s = match (side, available) {
        ("incoming", Some(r)) => (start_s - r.start_time.to_seconds()).max(0.0),
        ("incoming", None) => start_s.max(0.0),
        ("outgoing", Some(r)) => {
            let avail_end_s = r.start_time.to_seconds() + r.duration.to_seconds();
            (avail_end_s - end_s).max(0.0)
        }
        ("outgoing", None) => f64::INFINITY,
        _ => 0.0,
    };
    if available_s + 1e-6 < needed_s {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "transition {kind:?} around clip {:?} needs {needed_s:.3}s {side} handle, but only {available_s:.3}s is available. Repair by shortening the transition, choosing a different alignment, or applying Untrim Clip to widen the source range before inserting it.",
                clip.name
            ),
        });
    }
    Ok(())
}

/// Move an anchored clip to a different position within its track.
/// Cross-track moves aren't supported in v1 — `to_position` indexes
/// into the *anchor's* track, clamped to the track's child count.
///
/// The vec-extract / vec-insert pattern means the move is conceptually
/// a swap when `to_position == child_index + 1` (inserting just past
/// where we extracted lands the clip back where it started); the
/// helper below normalizes that case to a no-op rather than confusing
/// the user with an op that "succeeded but did nothing."
#[derive(Debug, Clone, Copy)]
struct SnapTarget {
    kind: SnapTargetKind,
    time_s: f64,
}

fn resolve_snap_time(
    timeline: &Timeline,
    index: usize,
    requested_s: f64,
    snap: Option<&SnapOptions>,
) -> Result<f64, ApplyError> {
    let Some(snap) = snap else {
        return Ok(requested_s);
    };
    if !snap.enabled {
        return Ok(requested_s);
    }
    if !snap.tolerance_s.is_finite() || snap.tolerance_s < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "move: snap_tolerance_s must be finite and >= 0, got {}",
                snap.tolerance_s
            ),
        });
    }
    let targets = collect_snap_targets(timeline, snap);
    if let Some(target) = targets
        .into_iter()
        .filter(|target| (target.time_s - requested_s).abs() <= snap.tolerance_s)
        .min_by(|a, b| {
            (a.time_s - requested_s)
                .abs()
                .total_cmp(&(b.time_s - requested_s).abs())
                .then_with(|| snap_target_priority(a.kind).cmp(&snap_target_priority(b.kind)))
                .then_with(|| a.time_s.total_cmp(&b.time_s))
        })
    {
        Ok(target.time_s)
    } else {
        Ok(requested_s)
    }
}

fn collect_snap_targets(timeline: &Timeline, snap: &SnapOptions) -> Vec<SnapTarget> {
    let mut targets = Vec::new();
    let include_clip_edges = snap_includes(snap, SnapTargetKind::ClipEdge);
    let include_markers = snap_includes(snap, SnapTargetKind::Marker);
    if !include_clip_edges && !include_markers {
        return targets;
    }
    for child in &timeline.tracks.children {
        let StackChild::Track(track) = child else {
            continue;
        };
        let mut cursor_s = 0.0;
        for child in &track.children {
            let duration_s = child_duration(child);
            if include_clip_edges {
                targets.push(SnapTarget {
                    kind: SnapTargetKind::ClipEdge,
                    time_s: cursor_s,
                });
                targets.push(SnapTarget {
                    kind: SnapTargetKind::ClipEdge,
                    time_s: cursor_s + duration_s,
                });
            }
            if include_markers && let TrackChild::Clip(clip) = child {
                for marker in &clip.markers {
                    targets.push(SnapTarget {
                        kind: SnapTargetKind::Marker,
                        time_s: cursor_s + marker.marked_range.start_time.to_seconds(),
                    });
                }
            }
            cursor_s += duration_s;
        }
    }
    targets
}

fn snap_includes(snap: &SnapOptions, kind: SnapTargetKind) -> bool {
    snap.targets.is_empty() || snap.targets.contains(&kind)
}

fn snap_target_priority(kind: SnapTargetKind) -> u8 {
    match kind {
        SnapTargetKind::Marker => 0,
        SnapTargetKind::ClipEdge => 1,
        SnapTargetKind::Playhead => 2,
    }
}

fn apply_multicam_plan(
    working: &mut Timeline,
    index: usize,
    plan: &MulticamApplyPlan,
) -> Result<String, ApplyError> {
    let program_track = plan.program_track.trim();
    if program_track.is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "apply_multicam_plan: program_track must not be empty".into(),
        });
    }
    validate_multicam_decisions(index, &plan.decisions)?;

    let replacement = build_multicam_program_track(program_track, &plan.decisions);
    if let Some(existing_index) =
        working.tracks.children.iter().position(
            |child| matches!(child, StackChild::Track(track) if track.name == program_track),
        )
    {
        working.tracks.children[existing_index] = StackChild::Track(replacement);
    } else {
        working.tracks.children.push(StackChild::Track(replacement));
    }
    Ok(format!(
        "applied multicam plan to track {program_track:?} with {} decisions",
        plan.decisions.len()
    ))
}

fn validate_multicam_decisions(
    index: usize,
    decisions: &[MulticamDecision],
) -> Result<(), ApplyError> {
    if decisions.is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "apply_multicam_plan: decisions must not be empty".into(),
        });
    }
    let mut cursor_s = 0.0;
    for (decision_index, decision) in decisions.iter().enumerate() {
        let source_asset = decision.source_asset.trim();
        if source_asset.is_empty() {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "apply_multicam_plan: decision {decision_index} source_asset must not be empty"
                ),
            });
        }
        if !decision.start_s.is_finite() || decision.start_s < 0.0 {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "apply_multicam_plan: decision {decision_index} start_s must be finite and >= 0"
                ),
            });
        }
        if !decision.end_s.is_finite() || decision.end_s <= decision.start_s {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "apply_multicam_plan: decision {decision_index} end_s must be finite and > start_s"
                ),
            });
        }
        if decision.start_s + 1e-6 < cursor_s {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "apply_multicam_plan: decision {decision_index} starts at {:.3}s before previous decision ends at {cursor_s:.3}s",
                    decision.start_s
                ),
            });
        }
        cursor_s = decision.end_s;
    }
    Ok(())
}

fn build_multicam_program_track(
    program_track: &str,
    decisions: &[MulticamDecision],
) -> montage_proto::otio::Track {
    use montage_proto::otio::{ExternalReference, MediaReference, RationalTime, TimeRange, Track};

    let rate = 24.0;
    let mut track = Track::empty(program_track.to_string(), TrackKind::Video);
    let mut cursor_s = 0.0;
    for (decision_index, decision) in decisions.iter().enumerate() {
        if decision.start_s > cursor_s + 0.001 {
            track
                .children
                .push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                    decision.start_s - cursor_s,
                    rate,
                )));
        }
        let mut clip = Clip::empty(format!("multicam-{decision_index}"));
        clip.media_reference =
            MediaReference::External(ExternalReference::new(decision.source_asset.clone()));
        // Timeline duration is always the program span; the source IN point
        // is the offset-corrected source time when the planner supplied one
        // (separate-device cameras), else the program start (shared timebase).
        let timeline_duration_s = decision.end_s - decision.start_s;
        let source_in_s = decision.source_start_s.unwrap_or(decision.start_s);
        clip.source_range = Some(TimeRange::new(
            RationalTime::new(source_in_s * rate, rate),
            RationalTime::new(timeline_duration_s * rate, rate),
        ));
        {
            let montage = clip
                .metadata
                .montage
                .get_or_insert_with(MontageClipMetadata::default);
            montage.reasoning = decision.reason.clone();
            montage.extra.insert(
                "multicam_source_asset".into(),
                serde_json::json!(&decision.source_asset),
            );
            montage.extra.insert(
                "multicam_decision_index".into(),
                serde_json::json!(decision_index),
            );
            if let Some(speaker) = decision.speaker.as_deref() {
                montage
                    .extra
                    .insert("speaker".into(), serde_json::json!(speaker));
            }
            if let Some(sync_group_id) = decision.sync_group_id.as_deref() {
                montage
                    .extra
                    .insert("sync_group_id".into(), serde_json::json!(sync_group_id));
            }
        }
        stamp_fresh_clip_uuid(&mut clip);
        track.children.push(TrackChild::Clip(clip));
        cursor_s = decision.end_s;
    }
    track
}

fn apply_wrap_as_nested(
    working: &mut Timeline,
    index: usize,
    track_name: &str,
    start_index: usize,
    end_index: usize,
    name: &Option<String>,
) -> Result<String, ApplyError> {
    let track_name = track_name.trim();
    if track_name.is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "wrap_as_nested: track must not be empty".into(),
        });
    }
    if start_index >= end_index {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "wrap_as_nested: start_index {start_index} must be < end_index {end_index}"
            ),
        });
    }
    let track = find_track_mut(working, track_name).ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!("wrap_as_nested: track {track_name:?} not found"),
    })?;
    if end_index > track.children.len() {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "wrap_as_nested: range [{start_index}..{end_index}) exceeds track length {}",
                track.children.len()
            ),
        });
    }
    let nested_name = name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Nested Stack");
    let nested_children: Vec<TrackChild> = track.children.drain(start_index..end_index).collect();
    let mut nested_track =
        montage_proto::otio::Track::empty(format!("{nested_name} Track"), track.kind);
    nested_track.children = nested_children;
    let mut stack = montage_proto::otio::Stack::empty(nested_name.to_string());
    stack.children.push(StackChild::Track(nested_track));
    track.children.insert(start_index, TrackChild::Stack(stack));
    Ok(format!(
        "wrapped children [{start_index}..{end_index}) on track {track_name:?} into nested stack {nested_name:?}"
    ))
}

fn apply_flatten_nested(
    working: &mut Timeline,
    index: usize,
    track_name: &str,
    child_index: usize,
) -> Result<String, ApplyError> {
    let track_name = track_name.trim();
    if track_name.is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "flatten_nested: track must not be empty".into(),
        });
    }
    let track = find_track_mut(working, track_name).ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!("flatten_nested: track {track_name:?} not found"),
    })?;
    if child_index >= track.children.len() {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "flatten_nested: child_index {child_index} exceeds track length {}",
                track.children.len()
            ),
        });
    }
    let TrackChild::Stack(stack) = track.children.remove(child_index) else {
        return Err(ApplyError::Invalid {
            index,
            message: format!("flatten_nested: child {child_index} is not a nested stack"),
        });
    };
    let mut flattened = Vec::new();
    for child in stack.children {
        match child {
            StackChild::Track(nested_track) => {
                if nested_track.kind != track.kind {
                    return Err(ApplyError::Invalid {
                        index,
                        message: format!(
                            "flatten_nested: nested track {:?} kind {:?} does not match parent kind {:?}",
                            nested_track.name, nested_track.kind, track.kind
                        ),
                    });
                }
                flattened.extend(nested_track.children);
            }
            StackChild::Clip(clip) => flattened.push(TrackChild::Clip(clip)),
            StackChild::Gap(gap) => flattened.push(TrackChild::Gap(gap)),
            StackChild::Stack(stack) => flattened.push(TrackChild::Stack(stack)),
        }
    }
    let flattened_len = flattened.len();
    track.children.splice(child_index..child_index, flattened);
    Ok(format!(
        "flattened nested stack at child {child_index} on track {track_name:?} into {flattened_len} children"
    ))
}

fn apply_move_clip(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    to_position: usize,
    at_s: Option<f64>,
    snap: Option<&SnapOptions>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let target_start_s = at_s
        .map(|target_start_s| resolve_snap_time(working, index, target_start_s, snap))
        .transpose()?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "move: anchor resolved to a non-track stack child".into(),
        });
    };

    if locator.child_index >= track.children.len() {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "move: source index {} out of bounds for track of length {}",
                locator.child_index,
                track.children.len()
            ),
        });
    }

    if let Some(target_start_s) = target_start_s {
        return apply_move_clip_to_time(index, track, locator.child_index, target_start_s);
    }

    // The user-facing target is "the clip's index in the post-move
    // track", clamped to a legal slot (0..=len). After extraction the
    // track is one shorter, so when the user-target is past the
    // extraction point we shift the insert index down by 1 to
    // compensate for the gap that just collapsed.
    let len_before = track.children.len();
    let target_user = to_position.min(len_before.saturating_sub(1));
    if target_user == locator.child_index {
        // Same-position move — describe as a no-op rather than
        // mutating + producing an effective-no-change OTIO.
        let TrackChild::Clip(c) = &track.children[locator.child_index] else {
            return Ok("move: source is not a clip; left timeline unchanged".into());
        };
        return Ok(format!(
            "move: clip {:?} already at position {}; left timeline unchanged",
            c.name, target_user,
        ));
    }

    let removed = track.children.remove(locator.child_index);
    // Translate user-facing target into a post-extract insertion
    // index. When moving forward (target_user > source), the
    // extraction shifted everyone after the source left by 1, so the
    // user's target index now means "insert just after element
    // target_user - 1" → insert at target_user.
    let insert_at = if target_user >= locator.child_index {
        target_user.min(track.children.len())
    } else {
        target_user
    };
    track.children.insert(insert_at, removed);

    let TrackChild::Clip(c) = &track.children[insert_at] else {
        return Ok(format!(
            "move: moved a non-clip child from {} to {} on track {}",
            locator.child_index, insert_at, locator.track_index,
        ));
    };
    Ok(format!(
        "moved clip {:?} from position {} to position {} on track {}",
        c.name, locator.child_index, target_user, locator.track_index,
    ))
}

/// Ripple-move: shift the moved clip by `delta_s` AND every clip after
/// it on the same track by the same amount, plus every clip in the
/// same `link_group_id` on other tracks. Implemented as a gap-resize
/// in front of each target clip (positive delta = grow preceding gap;
/// negative delta = shrink it, then merge gaps).
///
/// "Per-track ripple plus link group" matches Resolve/Premiere's
/// default body-drag behavior.
fn apply_ripple_move(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    delta_s: f64,
    snap: Option<&SnapOptions>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    if !delta_s.is_finite() {
        return Err(ApplyError::Invalid {
            index,
            message: format!("ripple_move: delta_s must be finite, got {delta_s}"),
        });
    }
    let StackChild::Track(source_track) = &working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "ripple_move: anchor resolved to a non-track stack child".into(),
        });
    };
    if locator.child_index >= source_track.children.len() {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "ripple_move: source index {} out of bounds for track of length {}",
                locator.child_index,
                source_track.children.len()
            ),
        });
    }

    // Identify the moved clip's link_group_id (if any) so we can ripple
    // its synced peers on other tracks together.
    let (link_group_id, original_name, original_start_s) =
        match &source_track.children[locator.child_index] {
            TrackChild::Clip(c) => (
                c.metadata
                    .montage
                    .as_ref()
                    .and_then(|m| m.extra.get("link_group_id"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                c.name.clone(),
                source_track.children[..locator.child_index]
                    .iter()
                    .map(child_duration)
                    .sum::<f64>(),
            ),
            _ => {
                return Err(ApplyError::Invalid {
                    index,
                    message: "ripple_move: source is not a clip".into(),
                });
            }
        };

    // Snap the *resulting* start time so the moved clip lands on a
    // tidy boundary. Clamp the delta so the moved clip's new start
    // stays non-negative.
    let raw_target = original_start_s + delta_s;
    let snapped_target = resolve_snap_time(working, index, raw_target.max(0.0), snap)?;
    let effective_delta = snapped_target - original_start_s;
    if effective_delta.abs() < 0.001 {
        return Ok(format!(
            "ripple_move: clip {original_name:?} already at {snapped_target:.3}s; left timeline unchanged"
        ));
    }

    // Apply the shift on the source track first. Then walk every other
    // track and shift each link-group sibling by the same amount.
    let source_track_index = locator.track_index;
    shift_clip_at_index(
        working,
        source_track_index,
        locator.child_index,
        effective_delta,
    )?;
    let mut shifted_siblings = 0usize;
    if let Some(group_id) = link_group_id.as_deref() {
        for track_index in 0..working.tracks.children.len() {
            if track_index == source_track_index {
                continue;
            }
            // Resolve the sibling's child index inside its own track.
            let Some(sibling_child_index) =
                first_clip_with_link_group(&working.tracks.children[track_index], group_id)
            else {
                continue;
            };
            shift_clip_at_index(working, track_index, sibling_child_index, effective_delta)?;
            shifted_siblings += 1;
        }
    }

    Ok(format!(
        "ripple-moved clip {original_name:?} by {effective_delta:+.3}s on track {} ({} linked sibling{} also shifted)",
        source_track_index,
        shifted_siblings,
        if shifted_siblings == 1 { "" } else { "s" },
    ))
}

/// Find the first clip on `stack_child` (must be a Track) whose
/// `link_group_id` matches `group_id`. Returns the child index in the
/// track's children vec, or None.
fn first_clip_with_link_group(stack_child: &StackChild, group_id: &str) -> Option<usize> {
    let StackChild::Track(track) = stack_child else {
        return None;
    };
    track.children.iter().position(|child| match child {
        TrackChild::Clip(c) => c
            .metadata
            .montage
            .as_ref()
            .and_then(|m| m.extra.get("link_group_id"))
            .and_then(|v| v.as_str())
            .map(|s| s == group_id)
            .unwrap_or(false),
        _ => false,
    })
}

/// Shift the clip at `(track_index, child_index)` by `delta_s` by
/// resizing the gap immediately before it. Positive delta grows the
/// preceding gap (creating one if absent). Negative delta shrinks /
/// removes the preceding gap, then trims earlier gaps if needed,
/// stopping at the track start (the clip can't go below 0).
fn shift_clip_at_index(
    working: &mut Timeline,
    track_index: usize,
    child_index: usize,
    delta_s: f64,
) -> Result<(), ApplyError> {
    if delta_s.abs() < 0.001 {
        return Ok(());
    }
    let StackChild::Track(track) = &mut working.tracks.children[track_index] else {
        return Err(ApplyError::Invalid {
            index: 0,
            message: "ripple_move: target slot is not a track".into(),
        });
    };
    if child_index >= track.children.len() {
        return Err(ApplyError::Invalid {
            index: 0,
            message: "ripple_move: child index out of bounds during shift".into(),
        });
    }
    let rate = track.children[child_index]
        .as_clip_rate()
        .unwrap_or_else(|| {
            // Fall back to scanning neighbors for a rate; default to 24.
            track
                .children
                .iter()
                .find_map(|c| c.as_clip_rate())
                .unwrap_or(24.0)
        });

    if delta_s > 0.0 {
        // Grow preceding gap. If the previous sibling is already a
        // gap, extend it; otherwise insert a new gap.
        let insert_pos = child_index;
        if child_index > 0 {
            let prev_dur = child_duration(&track.children[child_index - 1]);
            if let TrackChild::Gap(gap) = &mut track.children[child_index - 1] {
                let new_duration = prev_dur + delta_s;
                *gap = montage_proto::otio::Gap::of_duration(new_duration, rate);
                return Ok(());
            }
        }
        track.children.insert(
            insert_pos,
            TrackChild::Gap(montage_proto::otio::Gap::of_duration(delta_s, rate)),
        );
        return Ok(());
    }

    // delta_s < 0 — shrink preceding gaps. Walk backwards from
    // child_index - 1, eating gap duration off each one. Clamp at 0.
    let mut remaining = -delta_s;
    let mut cursor = child_index;
    while remaining > 0.001 && cursor > 0 {
        cursor -= 1;
        let dur = child_duration(&track.children[cursor]);
        match &mut track.children[cursor] {
            TrackChild::Gap(gap) => {
                if dur <= remaining + 0.001 {
                    // Consume the whole gap.
                    remaining -= dur;
                    track.children.remove(cursor);
                } else {
                    // Shrink in place.
                    let new_duration = dur - remaining;
                    *gap = montage_proto::otio::Gap::of_duration(new_duration, rate);
                    remaining = 0.0;
                }
            }
            _ => {
                // Hit a clip — can't shift past it. Stop here.
                break;
            }
        }
    }
    // `remaining > 0` here just means the move bumped into the start
    // of the track (or another clip); the shift was clamped, which
    // is the expected ripple behavior.
    Ok(())
}

/// Ripple-delete: remove the clip and shift every downstream clip on
/// the same track left by its duration, AND shift each link-group
/// sibling on every other track by the same delta. Matches Premiere
/// / Resolve's Shift+Delete behavior.
fn apply_ripple_delete(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "ripple_delete: anchor resolved to a non-track stack child".into(),
        });
    };
    if locator.child_index >= track.children.len() {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "ripple_delete: source index {} out of bounds for track of length {}",
                locator.child_index,
                track.children.len()
            ),
        });
    }
    let (link_group_id, removed_name, removed_duration, removed_source_range) =
        match &track.children[locator.child_index] {
            TrackChild::Clip(c) => (
                c.metadata
                    .montage
                    .as_ref()
                    .and_then(|m| m.extra.get("link_group_id"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                c.name.clone(),
                child_duration(&track.children[locator.child_index]),
                clip_source_range_s(c),
            ),
            _ => {
                return Err(ApplyError::Invalid {
                    index,
                    message: "ripple_delete: source is not a clip".into(),
                });
            }
        };
    if removed_duration <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: "ripple_delete: source clip has zero duration".into(),
        });
    }
    // Remove the source clip; do not leave a gap.
    track.children.remove(locator.child_index);
    // Link-group siblings on other tracks: locate, remove, and ripple-
    // shrink the leading gap of their track-time position too.
    let mut shifted_siblings = 0usize;
    if let Some(group_id) = link_group_id.as_deref() {
        let source_track_index = locator.track_index;
        for track_index in 0..working.tracks.children.len() {
            if track_index == source_track_index {
                continue;
            }
            let sibling_child_index = if let Some((start_s, end_s)) = removed_source_range {
                clip_with_link_group_covering_range(
                    &working.tracks.children[track_index],
                    group_id,
                    start_s,
                    end_s,
                )
            } else {
                first_clip_with_link_group(&working.tracks.children[track_index], group_id)
            };
            let Some(sibling_child_index) = sibling_child_index else {
                continue;
            };
            let StackChild::Track(sibling_track) = &mut working.tracks.children[track_index] else {
                continue;
            };
            sibling_track.children.remove(sibling_child_index);
            shifted_siblings += 1;
        }
    }
    Ok(format!(
        "ripple-deleted clip {removed_name:?} ({removed_duration:.3}s) on track {} ({} linked sibling{} also deleted)",
        locator.track_index,
        shifted_siblings,
        if shifted_siblings == 1 { "" } else { "s" },
    ))
}

/// Ripple-trim: change a clip's source-time edge AND shift every
/// downstream clip on the same track by the resulting duration delta.
/// Link-group siblings get the same shift on their own tracks.
fn apply_ripple_trim(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    edge: RippleTrimEdge,
    value_s: f64,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    if !value_s.is_finite() || value_s < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "ripple_trim: value_s must be a finite non-negative source-time, got {value_s}"
            ),
        });
    }
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "ripple_trim: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "ripple_trim: source is not a clip".into(),
        });
    };
    let Some(range) = clip.source_range.as_ref() else {
        return Err(ApplyError::Invalid {
            index,
            message: "ripple_trim: source clip has no source_range".into(),
        });
    };
    let rate = range.start_time.rate;
    let old_start = range.start_time.to_seconds();
    let old_end = old_start + range.duration.to_seconds();
    let link_group_id = clip
        .metadata
        .montage
        .as_ref()
        .and_then(|m| m.extra.get("link_group_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let clip_name = clip.name.clone();

    let (new_start, new_end) = match edge {
        RippleTrimEdge::Start => {
            let ns = value_s.min(old_end - 0.001).max(0.0);
            (ns, old_end)
        }
        RippleTrimEdge::End => {
            let ne = value_s.max(old_start + 0.001);
            (old_start, ne)
        }
    };
    let new_duration = new_end - new_start;
    if new_duration <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "ripple_trim: resulting duration {new_duration:.3}s would be non-positive"
            ),
        });
    }
    clip.source_range = Some(montage_proto::otio::TimeRange::new(
        montage_proto::otio::RationalTime::new(new_start * rate, rate),
        montage_proto::otio::RationalTime::new(new_duration * rate, rate),
    ));

    // Track-time effect of the trim:
    //   - Start edge: pushing source_start later (value_s > old_start)
    //     shortens the clip from its left → downstream clips shift
    //     LEFT by (new_start - old_start). The moved clip's own
    //     track_start_s stays put (it's anchored by leading gaps).
    //   - End edge: pushing source_end later (value_s > old_end)
    //     lengthens the clip from its right → downstream clips shift
    //     RIGHT by (new_end - old_end).
    let downstream_delta = match edge {
        RippleTrimEdge::Start => -(new_start - old_start),
        RippleTrimEdge::End => new_end - old_end,
    };
    if downstream_delta.abs() < 0.001 {
        return Ok(format!(
            "ripple_trim: clip {clip_name:?} edge {edge:?} unchanged"
        ));
    }

    // Shift downstream clips on the same track.
    ripple_shift_after(
        working,
        locator.track_index,
        locator.child_index,
        downstream_delta,
        rate,
    )?;
    // Shift link-group siblings on other tracks. For each sibling,
    // we trim its own source_range by the same delta (so the
    // synced audio shrinks/extends with the video) and ripple-shift
    // its own downstream clips.
    let mut shifted_siblings = 0usize;
    if let Some(group_id) = link_group_id.as_deref() {
        let source_track_index = locator.track_index;
        for track_index in 0..working.tracks.children.len() {
            if track_index == source_track_index {
                continue;
            }
            let Some(sibling_child_index) =
                first_clip_with_link_group(&working.tracks.children[track_index], group_id)
            else {
                continue;
            };
            // Apply the same edge trim to the sibling.
            {
                let StackChild::Track(sibling_track) = &mut working.tracks.children[track_index]
                else {
                    continue;
                };
                let TrackChild::Clip(sibling_clip) =
                    &mut sibling_track.children[sibling_child_index]
                else {
                    continue;
                };
                let Some(sib_range) = sibling_clip.source_range.as_ref() else {
                    continue;
                };
                let sib_rate = sib_range.start_time.rate;
                let sib_old_start = sib_range.start_time.to_seconds();
                let sib_old_end = sib_old_start + sib_range.duration.to_seconds();
                let (sib_new_start, sib_new_end) = match edge {
                    RippleTrimEdge::Start => (
                        (sib_old_start + (new_start - old_start)).max(0.0),
                        sib_old_end,
                    ),
                    RippleTrimEdge::End => (
                        sib_old_start,
                        (sib_old_end + (new_end - old_end)).max(sib_old_start + 0.001),
                    ),
                };
                let sib_new_dur = sib_new_end - sib_new_start;
                if sib_new_dur > 0.0 {
                    sibling_clip.source_range = Some(montage_proto::otio::TimeRange::new(
                        montage_proto::otio::RationalTime::new(sib_new_start * sib_rate, sib_rate),
                        montage_proto::otio::RationalTime::new(sib_new_dur * sib_rate, sib_rate),
                    ));
                }
            }
            ripple_shift_after(
                working,
                track_index,
                sibling_child_index,
                downstream_delta,
                rate,
            )?;
            shifted_siblings += 1;
        }
    }

    Ok(format!(
        "ripple-trimmed clip {clip_name:?} edge {edge:?} → new range [{new_start:.3}..{new_end:.3}]; downstream shifted {downstream_delta:+.3}s ({} linked sibling{} also trimmed)",
        shifted_siblings,
        if shifted_siblings == 1 { "" } else { "s" },
    ))
}

/// Shift every child strictly after `child_index` on `track_index` by
/// `delta_s` by resizing/inserting/removing gaps. Positive delta inserts
/// or grows a gap immediately after the anchor child; negative delta
/// shrinks gaps starting after the anchor.
fn ripple_shift_after(
    working: &mut Timeline,
    track_index: usize,
    child_index: usize,
    delta_s: f64,
    rate: f64,
) -> Result<(), ApplyError> {
    if delta_s.abs() < 0.001 {
        return Ok(());
    }
    let StackChild::Track(track) = &mut working.tracks.children[track_index] else {
        return Ok(());
    };
    let after_index = child_index + 1;
    if after_index > track.children.len() {
        return Ok(());
    }
    if delta_s > 0.0 {
        // Grow / insert gap right after the anchor child.
        if after_index < track.children.len() {
            if let TrackChild::Gap(gap) = &track.children[after_index] {
                let new_dur = child_duration(&track.children[after_index]) + delta_s;
                let _ = gap;
                track.children[after_index] =
                    TrackChild::Gap(montage_proto::otio::Gap::of_duration(new_dur, rate));
                return Ok(());
            }
        }
        track.children.insert(
            after_index,
            TrackChild::Gap(montage_proto::otio::Gap::of_duration(delta_s, rate)),
        );
        return Ok(());
    }
    // delta_s < 0 — shrink/remove gaps starting just after the anchor.
    let mut remaining = -delta_s;
    let cursor = after_index;
    while remaining > 0.001 && cursor < track.children.len() {
        let dur = child_duration(&track.children[cursor]);
        match &mut track.children[cursor] {
            TrackChild::Gap(gap) => {
                if dur <= remaining + 0.001 {
                    remaining -= dur;
                    track.children.remove(cursor);
                    // do not advance cursor — we just removed the
                    // element at `cursor`.
                } else {
                    let new_dur = dur - remaining;
                    *gap = montage_proto::otio::Gap::of_duration(new_dur, rate);
                    remaining = 0.0;
                }
            }
            _ => break,
        }
    }
    Ok(())
}

/// Delete the gap immediately before/after the anchor clip on its
/// track. Returns a descriptive no-op when the indicated side isn't
/// a gap (e.g. the clip is already at the track start, or the
/// trailing child is another clip).
/// Snapshot the display names of every `Track` in the timeline's
/// top-level stack. Used to attach a "did you mean..." hint when a
/// track-anchored op (delete_gap, trim_track_tail) names a track that
/// isn't there — the agent otherwise has to call view_timeline first
/// just to learn the names.
fn list_track_names(timeline: &Timeline) -> Vec<String> {
    timeline
        .tracks
        .children
        .iter()
        .filter_map(|child| match child {
            StackChild::Track(t) => Some(t.name.clone()),
            _ => None,
        })
        .collect()
}

fn apply_delete_gap(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    side: GapSide,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    // Snapshot the anchor clip's `link_group_id` before we mutate the
    // track. Cascading the same delete to linked siblings on other
    // tracks is the default — keeps paired V+A in sync after gap
    // edits. The agent can deliberately break the pair by anchoring
    // to a clip that has no link_group_id.
    let link_group_id = match working.tracks.children.get(locator.track_index) {
        Some(StackChild::Track(t)) => match t.children.get(locator.child_index) {
            Some(TrackChild::Clip(c)) => c
                .metadata
                .montage
                .as_ref()
                .and_then(|m| m.extra.get("link_group_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            _ => None,
        },
        _ => None,
    };

    let (primary_removed_dur, primary_track_index) = match remove_one_gap(
        working,
        index,
        locator.track_index,
        locator.child_index,
        side,
    ) {
        Ok(v) => v,
        Err(ApplyError::Invalid { message, .. }) => {
            // No gap to remove on the primary track is a soft
            // no-op, not an error — matches the pre-cascade
            // behavior and lets the agent re-issue freely without
            // crashing the envelope.
            return Ok(format!("delete_gap: {message}; left timeline unchanged",));
        }
        Err(other) => return Err(other),
    };

    let mut cascaded = Vec::<usize>::new();
    if let Some(group_id) = link_group_id.as_deref() {
        for ti in 0..working.tracks.children.len() {
            if ti == primary_track_index {
                continue;
            }
            let Some(child_index) =
                first_clip_with_link_group(&working.tracks.children[ti], group_id)
            else {
                continue;
            };
            // Cascading miss = no-op for that track. We don't want a
            // missing sibling gap to fail the whole op, since the
            // user's intent was "remove this gap"; siblings without
            // a matching gap simply weren't paired in the first
            // place (e.g. audio track is shorter).
            if let Ok((_, _)) = remove_one_gap(working, index, ti, child_index, side) {
                cascaded.push(ti);
            }
        }
    }

    if cascaded.is_empty() {
        Ok(format!(
            "deleted {side:?} gap ({primary_removed_dur:.3}s) adjacent to anchor on track {primary_track_index}",
        ))
    } else {
        Ok(format!(
            "deleted {side:?} gap ({primary_removed_dur:.3}s) on track {primary_track_index} (cascaded to {} linked track{})",
            cascaded.len(),
            if cascaded.len() == 1 { "" } else { "s" },
        ))
    }
}

/// Inner helper for `apply_delete_gap`: remove the gap on one specific
/// `(track_index, child_index)` pair, returning (removed_duration_s,
/// track_index) on success. Returns the same no-op-style descriptive
/// error when the anchor side has no gap, but as an `Err` so the
/// cascade loop knows to skip silently.
fn remove_one_gap(
    working: &mut Timeline,
    index: usize,
    track_index: usize,
    child_index: usize,
    side: GapSide,
) -> Result<(f64, usize), ApplyError> {
    let StackChild::Track(track) = &mut working.tracks.children[track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "delete_gap: target slot is not a track".into(),
        });
    };
    if child_index >= track.children.len() {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "delete_gap: source index {child_index} out of bounds for track of length {}",
                track.children.len(),
            ),
        });
    }
    let gap_index = match side {
        GapSide::Before => {
            if child_index == 0 {
                return Err(ApplyError::Invalid {
                    index,
                    message: "delete_gap: anchor at track start; no preceding gap".into(),
                });
            }
            child_index - 1
        }
        GapSide::After => {
            if child_index + 1 >= track.children.len() {
                return Err(ApplyError::Invalid {
                    index,
                    message: "delete_gap: anchor is last child; no trailing gap".into(),
                });
            }
            child_index + 1
        }
    };
    if !matches!(track.children[gap_index], TrackChild::Gap(_)) {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "delete_gap: child at {gap_index} on track {track_index} is not a gap",
            ),
        });
    }
    let removed_dur = child_duration(&track.children[gap_index]);
    track.children.remove(gap_index);
    Ok((removed_dur, track_index))
}

/// Strip every trailing `Gap` from a track so its last child is a
/// real clip (or transition). Returns a no-op description when the
/// track's tail is already non-gap. Looks up the track by display
/// name and errors if no track matches.
fn apply_trim_track_tail(
    working: &mut Timeline,
    index: usize,
    track_name: &str,
) -> Result<String, ApplyError> {
    if track_name.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "trim_track_tail: track must not be empty".into(),
        });
    }

    let available_names = list_track_names(working);
    let primary_idx = working
        .tracks
        .children
        .iter()
        .position(|c| matches!(c, StackChild::Track(t) if t.name == track_name))
        .ok_or_else(|| {
            let hint = if available_names.is_empty() {
                "; no tracks on the timeline".to_string()
            } else {
                format!("; available tracks: {}", available_names.join(", "))
            };
            ApplyError::Invalid {
                index,
                message: format!("trim_track_tail: no track named {track_name:?}{hint}"),
            }
        })?;

    // Snapshot the named track's last-clip link_group_id BEFORE
    // mutation so the cascade can decide which sibling tracks share
    // a paired endpoint. Once we pop trailing gaps the "last clip"
    // doesn't change (gaps aren't clips), so reading from the
    // pre-mutation state is fine.
    let primary_last_link_group_id = last_clip_link_group_id(&working.tracks.children[primary_idx]);

    let (primary_removed_count, primary_removed_total_s) =
        pop_trailing_gaps(&mut working.tracks.children[primary_idx]);

    // Cascade: for every OTHER track, if its last clip shares the
    // primary's last-clip link_group_id, strip its trailing gaps too.
    // Common case = paired V+A linked at ingest time; the audio
    // track's tail comes off in step with the video track.
    let mut cascaded: Vec<(String, usize, f64)> = Vec::new();
    if let Some(group_id) = primary_last_link_group_id.as_deref() {
        for ti in 0..working.tracks.children.len() {
            if ti == primary_idx {
                continue;
            }
            let last_group = last_clip_link_group_id(&working.tracks.children[ti]);
            if last_group.as_deref() != Some(group_id) {
                continue;
            }
            let name = match &working.tracks.children[ti] {
                StackChild::Track(t) => t.name.clone(),
                _ => continue,
            };
            let (count, total_s) = pop_trailing_gaps(&mut working.tracks.children[ti]);
            if count > 0 {
                cascaded.push((name, count, total_s));
            }
        }
    }

    if primary_removed_count == 0 && cascaded.is_empty() {
        return Ok(format!(
            "trim_track_tail: track {track_name:?} already ends in a clip; left timeline unchanged",
        ));
    }
    let mut summary = format!(
        "trim_track_tail: removed {primary_removed_count} trailing gap{} ({primary_removed_total_s:.3}s) from track {track_name:?}",
        if primary_removed_count == 1 { "" } else { "s" },
    );
    if !cascaded.is_empty() {
        let parts: Vec<String> = cascaded
            .iter()
            .map(|(name, count, total_s)| {
                format!(
                    "{name:?} ({count} gap{}, {total_s:.3}s)",
                    if *count == 1 { "" } else { "s" }
                )
            })
            .collect();
        summary.push_str(&format!(
            "; cascaded to linked tracks: {}",
            parts.join(", ")
        ));
    }
    Ok(summary)
}

/// Add an empty track to the timeline. Idempotent on `(name, kind)`:
/// a same-name, same-kind track returns success without mutating.
/// Same name, different kind errors so the V/A naming invariant the
/// renderer assumes (track named `"V*"` is a video track, `"A*"` /
/// `"Audio*"` is audio) is preserved.
///
/// `at_position` is an index among same-kind tracks. The new track is
/// inserted right after the Nth same-kind track in the stack's child
/// order. `None` (or an index past the end) appends after the last
/// same-kind track; if no tracks of that kind exist, the new track
/// goes to the end of the stack.
fn apply_insert_track(
    working: &mut Timeline,
    index: usize,
    name: &str,
    kind: InsertTrackKind,
    at_position: Option<usize>,
) -> Result<String, ApplyError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "insert_track: name must not be empty".into(),
        });
    }
    let resolved_kind = match kind {
        InsertTrackKind::Video => TrackKind::Video,
        InsertTrackKind::Audio => TrackKind::Audio,
        InsertTrackKind::Auto => {
            // Same convention apply_insert_clip uses: A* / Audio* → audio,
            // everything else → video.
            let lower = trimmed.to_ascii_lowercase();
            if lower.starts_with('a') || lower.starts_with("audio") {
                TrackKind::Audio
            } else {
                TrackKind::Video
            }
        }
    };

    // Idempotency + kind-conflict check on existing tracks.
    for child in working.tracks.children.iter() {
        if let StackChild::Track(t) = child {
            if t.name == trimmed {
                if t.kind == resolved_kind {
                    return Ok(format!(
                        "insert_track: track {trimmed:?} already exists; left timeline unchanged",
                    ));
                }
                return Err(ApplyError::Invalid {
                    index,
                    message: format!(
                        "insert_track: track {trimmed:?} exists as {:?}, cannot recreate as {:?}",
                        t.kind, resolved_kind
                    ),
                });
            }
        }
    }

    let new_track = StackChild::Track(montage_proto::otio::Track::empty(
        trimmed.to_string(),
        resolved_kind,
    ));

    // Resolve insertion index. Walk same-kind tracks in stack order and
    // insert after the Nth one (0-based). Out-of-range and None both
    // append-after-last-same-kind.
    let insert_at: usize = match at_position {
        Some(target) => {
            let mut last_match: Option<usize> = None;
            let mut seen = 0usize;
            for (i, c) in working.tracks.children.iter().enumerate() {
                if matches!(c, StackChild::Track(t) if t.kind == resolved_kind) {
                    if seen == target {
                        last_match = Some(i);
                        break;
                    }
                    last_match = Some(i);
                    seen += 1;
                }
            }
            match last_match {
                Some(i) => i + 1,
                None => working.tracks.children.len(),
            }
        }
        None => {
            let mut last_match: Option<usize> = None;
            for (i, c) in working.tracks.children.iter().enumerate() {
                if matches!(c, StackChild::Track(t) if t.kind == resolved_kind) {
                    last_match = Some(i);
                }
            }
            match last_match {
                Some(i) => i + 1,
                None => working.tracks.children.len(),
            }
        }
    };
    working.tracks.children.insert(insert_at, new_track);

    Ok(format!(
        "insert_track: added {trimmed:?} ({:?}) at stack index {insert_at}",
        resolved_kind
    ))
}

/// Remove a track from the timeline by name. Refuses if the track
/// has any non-gap children unless `force = true` — the default
/// guards against typos that would silently drop content. Listing
/// the available track names in the error means the agent (or user)
/// can self-correct without a separate view_timeline call.
///
/// Counterpart to `apply_insert_track`. Gives the agent a safe verb
/// for "delete empty track" instead of reaching for the shell to
/// mutate `project.otio.json` directly.
fn apply_delete_track(
    working: &mut Timeline,
    index: usize,
    track_name: &str,
    force: bool,
) -> Result<String, ApplyError> {
    let trimmed = track_name.trim();
    if trimmed.is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "delete_track: name must not be empty".into(),
        });
    }
    let available_names = list_track_names(working);
    let position = working
        .tracks
        .children
        .iter()
        .position(|c| matches!(c, StackChild::Track(t) if t.name == trimmed))
        .ok_or_else(|| {
            let hint = if available_names.is_empty() {
                "; no tracks on the timeline".to_string()
            } else {
                format!("; available tracks: {}", available_names.join(", "))
            };
            ApplyError::Invalid {
                index,
                message: format!("delete_track: no track named {trimmed:?}{hint}"),
            }
        })?;

    // Count non-gap children. A track populated only with gaps is
    // safe to drop without `force` — gaps are placeholders, not
    // editorial content.
    let clip_count = match &working.tracks.children[position] {
        StackChild::Track(t) => t
            .children
            .iter()
            .filter(|c| !matches!(c, TrackChild::Gap(_)))
            .count(),
        _ => 0,
    };
    if clip_count > 0 && !force {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "delete_track: track {trimmed:?} has {clip_count} clip(s); refusing to delete without `force: true`. \
                 Move clips off the track first or pass `+ force: true` if intentional.",
            ),
        });
    }

    let kind_label = match &working.tracks.children[position] {
        StackChild::Track(t) => match t.kind {
            TrackKind::Video => "video",
            TrackKind::Audio => "audio",
        },
        _ => "track",
    };
    working.tracks.children.remove(position);
    Ok(format!(
        "delete_track: removed {kind_label} track {trimmed:?} (had {clip_count} clip{})",
        if clip_count == 1 { "" } else { "s" },
    ))
}

/// Pop every trailing `Gap` from the given stack child (if it is a
/// Track). Returns `(count_popped, total_seconds_removed)`. Non-track
/// children return `(0, 0.0)`.
fn pop_trailing_gaps(stack_child: &mut StackChild) -> (usize, f64) {
    let StackChild::Track(track) = stack_child else {
        return (0, 0.0);
    };
    let mut count = 0usize;
    let mut total_s = 0.0;
    while let Some(last) = track.children.last()
        && matches!(last, TrackChild::Gap(_))
    {
        total_s += child_duration(last);
        track.children.pop();
        count += 1;
    }
    (count, total_s)
}

/// Return the `link_group_id` of the last `Clip` child on the track,
/// skipping past trailing gaps and transitions. None when the track
/// is empty, when there are no clips at all, or when the last clip
/// has no `link_group_id`.
fn last_clip_link_group_id(stack_child: &StackChild) -> Option<String> {
    let StackChild::Track(track) = stack_child else {
        return None;
    };
    for child in track.children.iter().rev() {
        if let TrackChild::Clip(clip) = child {
            return clip
                .metadata
                .montage
                .as_ref()
                .and_then(|m| m.extra.get("link_group_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }
    None
}

fn apply_move_clip_to_time(
    index: usize,
    track: &mut montage_proto::otio::Track,
    child_index: usize,
    target_start_s: f64,
) -> Result<String, ApplyError> {
    if !target_start_s.is_finite() || target_start_s < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("move: at_s must be a finite non-negative time, got {target_start_s}"),
        });
    }

    let original_start_s: f64 = track.children[..child_index]
        .iter()
        .map(child_duration)
        .sum();
    let duration_s = child_duration(&track.children[child_index]);
    if duration_s <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: "move: source clip has zero duration".into(),
        });
    }
    if (target_start_s - original_start_s).abs() < 0.01 {
        let TrackChild::Clip(c) = &track.children[child_index] else {
            return Ok("move: source is not a clip; left timeline unchanged".into());
        };
        return Ok(format!(
            "move: clip {:?} already starts at {target_start_s:.3}s; left timeline unchanged",
            c.name
        ));
    }

    let rate = track.children[child_index].as_clip_rate().unwrap_or(24.0);
    let removed = std::mem::replace(
        &mut track.children[child_index],
        TrackChild::Gap(montage_proto::otio::Gap::of_duration(duration_s, rate)),
    );
    let name = match &removed {
        TrackChild::Clip(c) => c.name.clone(),
        _ => "<non-clip>".to_string(),
    };
    insert_child_at_time(track, removed, target_start_s, duration_s, rate);
    merge_adjacent_gaps(&mut track.children, rate);

    Ok(format!(
        "moved clip {name:?} from {original_start_s:.3}s to {target_start_s:.3}s on track {:?}",
        track.name
    ))
}

trait TrackChildClipRate {
    fn as_clip_rate(&self) -> Option<f64>;
}

impl TrackChildClipRate for TrackChild {
    fn as_clip_rate(&self) -> Option<f64> {
        match self {
            TrackChild::Clip(c) => c
                .source_range
                .as_ref()
                .map(|r| r.start_time.rate)
                .filter(|r| *r > 0.0),
            _ => None,
        }
    }
}

fn insert_child_at_time(
    track: &mut montage_proto::otio::Track,
    child: TrackChild,
    target_start_s: f64,
    child_duration_s: f64,
    rate: f64,
) {
    const EPS: f64 = 0.001;
    let mut cursor_s = 0.0;
    for i in 0..track.children.len() {
        let dur_s = child_duration(&track.children[i]);
        let end_s = cursor_s + dur_s;
        if target_start_s <= cursor_s + EPS {
            track.children.insert(i, child);
            return;
        }
        if target_start_s < end_s - EPS {
            if matches!(track.children[i], TrackChild::Gap(_)) {
                let before_s = (target_start_s - cursor_s).max(0.0);
                let after_s = (end_s - target_start_s - child_duration_s).max(0.0);
                let mut replacement = Vec::new();
                if before_s > EPS {
                    replacement.push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                        before_s, rate,
                    )));
                }
                replacement.push(child);
                if after_s > EPS {
                    replacement.push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                        after_s, rate,
                    )));
                }
                track.children.splice(i..=i, replacement);
                return;
            }
            let insert_at = if target_start_s < cursor_s + dur_s / 2.0 {
                i
            } else {
                i + 1
            };
            track.children.insert(insert_at, child);
            return;
        }
        cursor_s = end_s;
    }
    if target_start_s > cursor_s + EPS {
        track
            .children
            .push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                target_start_s - cursor_s,
                rate,
            )));
    }
    track.children.push(child);
}

fn merge_adjacent_gaps(children: &mut Vec<TrackChild>, rate: f64) {
    let mut merged = Vec::with_capacity(children.len());
    for child in std::mem::take(children) {
        if let TrackChild::Gap(gap) = child {
            let dur_s = gap.source_range.duration.to_seconds();
            if dur_s <= 0.001 {
                continue;
            }
            if let Some(TrackChild::Gap(prev)) = merged.last_mut() {
                let total_s = prev.source_range.duration.to_seconds() + dur_s;
                *prev = montage_proto::otio::Gap::of_duration(total_s, rate);
            } else {
                merged.push(TrackChild::Gap(gap));
            }
        } else {
            merged.push(child);
        }
    }
    *children = merged;
}

/// Insert a b-roll clip near an anchor, either replacing footage in
/// place (`Replace`) or layering on a higher video track (`Overlay`).
///
/// In v1 the broll always lands at the *anchor clip's start* position
/// (no in-clip offset field). The op carries `duration_s`, the broll's
/// length on the timeline.
///
/// `Replace`: the anchor clip's leading `duration_s` window is swapped
/// for the broll. The anchor clip's residual tail (if any) stays on
/// the same track immediately after the broll. If `duration_s` is
/// >= the anchor's full duration, the broll fully consumes the anchor
/// — no tail piece is reinserted.
///
/// `Overlay`: a new video track (named `V<N>` for the next free
/// number) is created if no video track besides the anchor's exists,
/// and the broll lands on it. v1 places the broll at the anchor
/// clip's track-time start; nothing on the lower track is mutated.
fn apply_insert_broll(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    asset: &str,
    duration_s: f64,
    position: super::op::BRollPosition,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    use montage_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, TimeRange, Track, TrackKind,
    };
    let _ = (anchor, ctx);

    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("broll duration {duration_s} must be > 0"),
        });
    }

    let locator = required_locator(index, locator)?;

    // Snapshot the anchor clip's source range + name + rate before
    // mutating anything — we'll need them in both branches and
    // referencing back into `working` via locator after a structural
    // change is fragile.
    let (anchor_name, rate, anchor_source_start, anchor_source_dur) = {
        let StackChild::Track(track) = &working.tracks.children[locator.track_index] else {
            return Err(ApplyError::Invalid {
                index,
                message: "broll: anchor resolved to a non-track stack child".into(),
            });
        };
        let TrackChild::Clip(clip) = &track.children[locator.child_index] else {
            return Err(ApplyError::Invalid {
                index,
                message: "broll: anchor resolved to a non-clip track child".into(),
            });
        };
        let range = clip
            .source_range
            .as_ref()
            .ok_or_else(|| ApplyError::Invalid {
                index,
                message: "broll: anchor clip has no source_range".into(),
            })?;
        (
            clip.name.clone(),
            range.start_time.rate,
            range.start_time.to_seconds(),
            range.duration.to_seconds(),
        )
    };

    // Build the broll clip — same shape as apply_insert_clip's clip
    // construction.
    let mut broll = Clip::empty(format!("broll-from-{anchor_name}"));
    broll.media_reference = MediaReference::External(ExternalReference::new(asset.to_string()));
    broll.source_range = Some(TimeRange::new(
        RationalTime::new(0.0, rate),
        RationalTime::new(duration_s * rate, rate),
    ));
    stamp_fresh_clip_uuid(&mut broll);

    match position {
        super::op::BRollPosition::Replace => {
            // Compute the residual tail BEFORE we touch the track —
            // we'll only insert it if the broll doesn't fully consume
            // the anchor clip.
            let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
                return Err(ApplyError::Invalid {
                    index,
                    message: "broll: anchor resolved to a non-track stack child".into(),
                });
            };

            // Pull the anchor clip out as the template for the
            // residual tail.
            let TrackChild::Clip(orig) = track.children.remove(locator.child_index) else {
                return Err(ApplyError::Invalid {
                    index,
                    message: "broll: anchor resolved to a non-clip child".into(),
                });
            };
            track
                .children
                .insert(locator.child_index, TrackChild::Clip(broll));

            if duration_s < anchor_source_dur {
                let residual_start = anchor_source_start + duration_s;
                let residual_dur = anchor_source_dur - duration_s;
                let mut tail = orig;
                tail.name = format!("{anchor_name}-tail");
                tail.source_range = Some(TimeRange::new(
                    RationalTime::new(residual_start * rate, rate),
                    RationalTime::new(residual_dur * rate, rate),
                ));
                stamp_fresh_clip_uuid(&mut tail);
                track
                    .children
                    .insert(locator.child_index + 1, TrackChild::Clip(tail));
            }

            Ok(format!(
                "inserted b-roll over {anchor_name:?} on track {}: \
                 asset={asset:?} duration={duration_s:.3}s (replace)",
                locator.track_index,
            ))
        }
        super::op::BRollPosition::Overlay => {
            // Anchor's track-time = sum of preceding child durations.
            // We need this so the agent can place the overlay
            // correctly even when the lower track has lots of
            // earlier clips. Insert into a free gap at that exact
            // time; if every existing overlay track is occupied
            // there, create another overlay track instead of
            // appending to the end and silently moving the b-roll.
            let anchor_track_time =
                track_time_at(working, locator.track_index, locator.child_index);

            let mut broll = Some(broll);
            let mut target_name = None;
            for track_idx in 0..working.tracks.children.len() {
                if track_idx == locator.track_index {
                    continue;
                }
                let StackChild::Track(target) = &mut working.tracks.children[track_idx] else {
                    continue;
                };
                if !matches!(target.kind, TrackKind::Video) {
                    continue;
                }
                let clip = broll.take().expect("broll clip available");
                match insert_overlay_clip_at(
                    target,
                    TrackChild::Clip(clip),
                    anchor_track_time,
                    duration_s,
                    rate,
                ) {
                    Ok(()) => {
                        target_name = Some(target.name.clone());
                        break;
                    }
                    Err(clip) => {
                        let TrackChild::Clip(clip) = *clip else {
                            unreachable!("insert_overlay_clip_at returned non-clip")
                        };
                        broll = Some(clip);
                    }
                }
            }
            if target_name.is_none() {
                let next_name = next_video_track_name(working);
                let mut track = Track::empty(next_name.clone(), TrackKind::Video);
                let clip = broll.take().expect("broll clip available");
                insert_overlay_clip_at(
                    &mut track,
                    TrackChild::Clip(clip),
                    anchor_track_time,
                    duration_s,
                    rate,
                )
                .map_err(|_| ApplyError::Invalid {
                    index,
                    message: "broll: failed to insert on fresh overlay track".into(),
                })?;
                working.tracks.children.push(StackChild::Track(track));
                target_name = Some(next_name);
            }
            Ok(format!(
                "inserted b-roll over {anchor_name:?} on overlay track {}: \
                 asset={asset:?} duration={duration_s:.3}s (overlay)",
                target_name.unwrap_or_else(|| "V?".into()),
            ))
        }
    }
}

/// Insert a picture-in-picture clip on an upper video track.
fn apply_insert_pip(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    asset: &str,
    duration_s: f64,
    source_start_s: f64,
    corner: super::op::PiPCorner,
    scale: f64,
    margin_pct: f64,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    use montage_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, TimeRange, Track, TrackChild,
        TrackKind,
    };
    let _ = (anchor, ctx);

    validate_project_relative_asset(index, "insert_pip", asset)?;
    if !duration_s.is_finite() || duration_s <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("insert_pip: duration_s {duration_s} must be > 0"),
        });
    }
    if !source_start_s.is_finite() || source_start_s < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("insert_pip: source_start_s {source_start_s} must be >= 0"),
        });
    }
    if !scale.is_finite() || !(0.10..=0.60).contains(&scale) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("insert_pip: scale {scale} must be in range 0.10..=0.60"),
        });
    }
    if !margin_pct.is_finite() || !(0.0..=0.15).contains(&margin_pct) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("insert_pip: margin_pct {margin_pct} must be in range 0.0..=0.15"),
        });
    }

    let locator = required_locator(index, locator)?;
    let (anchor_name, rate) = {
        let StackChild::Track(track) = &working.tracks.children[locator.track_index] else {
            return Err(ApplyError::Invalid {
                index,
                message: "insert_pip: anchor resolved to a non-track stack child".into(),
            });
        };
        let TrackChild::Clip(clip) = &track.children[locator.child_index] else {
            return Err(ApplyError::Invalid {
                index,
                message: "insert_pip: anchor resolved to a non-clip track child".into(),
            });
        };
        let range = clip
            .source_range
            .as_ref()
            .ok_or_else(|| ApplyError::Invalid {
                index,
                message: "insert_pip: anchor clip has no source_range".into(),
            })?;
        (clip.name.clone(), range.start_time.rate)
    };

    let mut pip = Clip::empty(format!("pip-from-{anchor_name}"));
    pip.media_reference = MediaReference::External(ExternalReference::new(asset.to_string()));
    pip.source_range = Some(TimeRange::new(
        RationalTime::new(source_start_s * rate, rate),
        RationalTime::new(duration_s * rate, rate),
    ));
    stamp_fresh_clip_uuid(&mut pip);
    stamp_video_overlay_effect(&mut pip, "pip", Some(corner), Some(scale), Some(margin_pct));

    let target_idx = working
        .tracks
        .children
        .iter()
        .enumerate()
        .find_map(|(i, sc)| match sc {
            StackChild::Track(t)
                if matches!(t.kind, TrackKind::Video) && i != locator.track_index =>
            {
                Some(i)
            }
            _ => None,
        });
    let target_idx = match target_idx {
        Some(i) => i,
        None => {
            let next_name = next_video_track_name(working);
            let track = Track::empty(next_name, TrackKind::Video);
            working.tracks.children.push(StackChild::Track(track));
            working.tracks.children.len() - 1
        }
    };

    let anchor_track_time = track_time_at(working, locator.track_index, locator.child_index);
    let StackChild::Track(target) = &mut working.tracks.children[target_idx] else {
        return Err(ApplyError::Invalid {
            index,
            message: "insert_pip: overlay target resolved to a non-track stack child".into(),
        });
    };
    let target_cursor = track_cursor(target);
    if target_cursor < anchor_track_time {
        target
            .children
            .push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                anchor_track_time - target_cursor,
                rate,
            )));
    }
    target.children.push(TrackChild::Clip(pip));
    Ok(format!(
        "inserted PiP over {anchor_name:?} on overlay track {}: asset={asset:?} \
         source_start={source_start_s:.3}s duration={duration_s:.3}s corner={}",
        target.name,
        pip_corner_str(corner),
    ))
}

/// Effect name used for per-clip volume changes. Render reads this
/// in [`crates/render/src/timeline.rs`]'s FilterPlanner and emits
/// `volume=<value>` on the segment's audio stream.
const VOLUME_EFFECT_NAME: &str = montage_effects::VOLUME;

/// Effect name used for media overlays and picture-in-picture clips.
const VIDEO_OVERLAY_EFFECT_NAME: &str = montage_effects::VIDEO_OVERLAY;

/// Effect name used for per-clip audio fades.
const AUDIO_FADE_EFFECT_NAME: &str = montage_effects::AUDIO_FADE;

/// Effect name used for per-clip speed changes. Render reads this
/// and emits `setpts=<1/factor>*PTS` on video + `atempo=<factor>`
/// on audio (chained when factor is outside `[0.5, 2.0]`).
const SPEED_EFFECT_NAME: &str = montage_effects::SPEED;

/// Effect name used for clip-level color correction. Render reads the
/// optional numeric fields and emits FFmpeg color filters before speed.
const COLOR_CORRECTION_EFFECT_NAME: &str = montage_effects::COLOR_CORRECTION;

/// Effect name used for clip-level LUT application. Render maps the
/// project-relative `lut_path` to FFmpeg's `lut3d` filter.
const LUT_EFFECT_NAME: &str = montage_effects::LUT;
/// Effect name used for the atomic color pipeline. Schema is live;
/// render lowering is the next stage (see
/// `montage-color-pipeline-plan` memory).
const COLOR_PIPELINE_EFFECT_NAME: &str = montage_effects::COLOR_PIPELINE;
const SUPPORTED_LUT_EXTENSIONS: &[&str] = &["3dl", "cube", "dat", "m3d", "csp"];
const SUPPORTED_LUT_INTERPOLATIONS: &[&str] =
    &["nearest", "trilinear", "tetrahedral", "pyramid", "prism"];

/// Effect name stamped on title-clip synthesized clips. The metadata
/// holds text/position/font_size/color/font_weight/animation; render
/// walks the Titles track and emits drawtext filters per title.
const TITLE_EFFECT_NAME: &str = montage_effects::TITLE;

/// Track name used for the auto-created Titles track. Render walks
/// any video track whose `metadata["montage_track_role"]` is "titles"
/// (not just by name) so the user can rename it.
const TITLES_TRACK_NAME: &str = "Titles";

/// Track-metadata key flagging this track as the project's title
/// overlay track. `apply_insert_title` stamps it on track creation;
/// the render pipeline pattern-matches on it.
const TITLES_TRACK_ROLE_KEY: &str = "montage_track_role";

/// Track-metadata value for the titles track.
const TITLES_TRACK_ROLE_VALUE: &str = "titles";

const ANNOTATION_EFFECT_NAME: &str = "montage.annotation";
const ANNOTATIONS_TRACK_NAME: &str = "Annotations";
const ANNOTATIONS_TRACK_ROLE_VALUE: &str = "annotations";

/// Track metadata key holding first-class audio controls.
const AUDIO_TRACK_METADATA_KEY: &str = "montage_audio";

/// Clip effect storing waveform/timecode sync metadata.
const SYNC_GROUP_EFFECT_NAME: &str = "montage.sync_group";

/// Clip effect storing FFmpeg-native audio repair settings.
const AUDIO_FX_EFFECT_NAME: &str = "montage.audio_fx";

/// Stamp an montage.volume Effect on the anchored clip with `value`
/// (linear gain multiplier). Idempotent: any existing
/// montage.volume effect on the clip is removed first, so two
/// SetVolume ops in one envelope leave only the second's value.
fn apply_set_volume(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    value: f64,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    if !value.is_finite() || value < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_volume: value {value} must be finite and >= 0.0"),
        });
    }
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_volume: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_volume: anchor resolved to a non-clip track child".into(),
        });
    };
    clip.effects.retain(|e| e.effect_name != VOLUME_EFFECT_NAME);
    let mut effect = montage_proto::otio::Effect::new(VOLUME_EFFECT_NAME);
    effect
        .metadata
        .insert("value".to_string(), serde_json::json!(value));
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!(
        "set volume on clip {clip_name:?} to {value:.3} (linear gain)"
    ))
}

fn apply_mute_clip(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    muted: bool,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "mute_clip: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "mute_clip: anchor resolved to a non-clip track child".into(),
        });
    };
    let clip_name = clip.name.clone();
    let montage = clip
        .metadata
        .montage
        .get_or_insert_with(MontageClipMetadata::default);
    let overlay = montage
        .audio_override
        .get_or_insert_with(ClipAudioOverride::default);
    overlay.muted = muted;
    // Drop an override that no longer carries any state, keeping clip
    // metadata clean and the render on the coupled path when possible.
    if !overlay.muted && overlay.removed_ranges.is_empty() {
        montage.audio_override = None;
    }
    Ok(format!(
        "{} audio on clip {clip_name:?} (picture kept)",
        if muted { "muted" } else { "unmuted" }
    ))
}

#[allow(clippy::too_many_arguments)]
fn apply_remove_audio(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    start_s: Option<f64>,
    end_s: Option<f64>,
    clear: bool,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "remove_audio: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "remove_audio: anchor resolved to a non-clip track child".into(),
        });
    };
    let clip_name = clip.name.clone();
    let montage = clip
        .metadata
        .montage
        .get_or_insert_with(MontageClipMetadata::default);

    if clear {
        if let Some(overlay) = montage.audio_override.as_mut() {
            overlay.removed_ranges.clear();
            if !overlay.muted {
                montage.audio_override = None;
            }
        }
        return Ok(format!("cleared audio removals on clip {clip_name:?}"));
    }

    let (Some(start_s), Some(end_s)) = (start_s, end_s) else {
        return Err(ApplyError::Invalid {
            index,
            message: "remove_audio: start_s and end_s are required unless clear:true".into(),
        });
    };
    if !start_s.is_finite() || !end_s.is_finite() || start_s < 0.0 || end_s <= start_s {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "remove_audio: need finite 0 <= start_s < end_s (got {start_s}..{end_s})"
            ),
        });
    }

    let overlay = montage
        .audio_override
        .get_or_insert_with(ClipAudioOverride::default);
    overlay.removed_ranges.push(AudioRange { start_s, end_s });
    Ok(format!(
        "removed audio {start_s:.3}..{end_s:.3}s on clip {clip_name:?} (picture kept)"
    ))
}

fn apply_set_audio_fade(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    fade_in_s: Option<f64>,
    fade_out_s: Option<f64>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let fade_in = fade_in_s.unwrap_or(0.0);
    let fade_out = fade_out_s.unwrap_or(0.0);
    if !fade_in.is_finite() || !fade_out.is_finite() || fade_in < 0.0 || fade_out < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "set_audio_fade: fade_in_s {fade_in} and fade_out_s {fade_out} must be finite and >= 0.0"
            ),
        });
    }
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_audio_fade: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_audio_fade: anchor resolved to a non-clip track child".into(),
        });
    };
    clip.effects
        .retain(|e| e.effect_name != AUDIO_FADE_EFFECT_NAME);
    let mut effect = montage_proto::otio::Effect::new(AUDIO_FADE_EFFECT_NAME);
    effect
        .metadata
        .insert("fade_in_s".to_string(), serde_json::json!(fade_in));
    effect
        .metadata
        .insert("fade_out_s".to_string(), serde_json::json!(fade_out));
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!(
        "set audio fade on clip {clip_name:?}: in={fade_in:.3}s out={fade_out:.3}s"
    ))
}

fn apply_set_audio_lead(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    lead_s: f64,
    split_edit: Option<&SplitEditSpec>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    apply_split_edit_offset(
        working,
        index,
        anchor,
        locator,
        lead_s,
        SplitEditKind::AudioLead,
        split_edit,
        ctx,
    )
}

fn apply_set_audio_trail(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    trail_s: f64,
    split_edit: Option<&SplitEditSpec>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    apply_split_edit_offset(
        working,
        index,
        anchor,
        locator,
        trail_s,
        SplitEditKind::AudioTrail,
        split_edit,
        ctx,
    )
}

#[derive(Debug, Clone, Copy)]
enum SplitEditKind {
    AudioLead,
    AudioTrail,
}

fn apply_split_edit_offset(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    locator: Option<ClipLocator>,
    offset_s: f64,
    kind: SplitEditKind,
    split_edit: Option<&SplitEditSpec>,
    ctx: &AnchorContext,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    if !offset_s.is_finite() || offset_s < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("split edit offset {offset_s} must be finite and >= 0.0"),
        });
    }
    if let Some(confidence) = split_edit.and_then(|spec| spec.confidence)
        && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("split edit confidence {confidence} must be in [0, 1]"),
        });
    }
    let locator = required_locator(index, locator)?;
    let (clip_name, clip_id) =
        stamp_split_edit_on_clip(working, index, locator, kind, offset_s, split_edit)?;
    stamp_split_edit_cut_boundary(working, locator, &clip_id, kind, offset_s, split_edit);
    let label = match kind {
        SplitEditKind::AudioLead => "audio lead",
        SplitEditKind::AudioTrail => "audio trail",
    };
    Ok(format!("set {label} on clip {clip_name:?}: {offset_s:.3}s"))
}

fn stamp_split_edit_on_clip(
    working: &mut Timeline,
    index: usize,
    locator: ClipLocator,
    kind: SplitEditKind,
    offset_s: f64,
    split_edit: Option<&SplitEditSpec>,
) -> Result<(String, String), ApplyError> {
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "split edit: anchor resolved to a non-track stack child".into(),
        });
    };
    let audio_lead_from_clip_id = previous_clip_id(track, locator.child_index);
    let audio_trail_to_clip_id = next_clip_id(track, locator.child_index);
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "split edit: anchor resolved to a non-clip track child".into(),
        });
    };
    let clip_name = clip.name.clone();
    let clip_id = clip_uuid(clip).unwrap_or(clip.name.as_str()).to_string();
    let montage = clip
        .metadata
        .montage
        .get_or_insert_with(MontageClipMetadata::default);
    let mut spec = montage.split_edit.clone().unwrap_or_default();
    match kind {
        SplitEditKind::AudioLead => {
            spec.audio_lead_s = Some(offset_s);
            spec.audio_lead_from_clip_id = audio_lead_from_clip_id;
            if split_edit.and_then(|s| s.audio_trail_s).is_some() {
                spec.audio_trail_s = split_edit.and_then(|s| s.audio_trail_s);
                spec.audio_trail_to_clip_id = audio_trail_to_clip_id;
            }
        }
        SplitEditKind::AudioTrail => {
            spec.audio_trail_s = Some(offset_s);
            spec.audio_trail_to_clip_id = audio_trail_to_clip_id;
            if split_edit.and_then(|s| s.audio_lead_s).is_some() {
                spec.audio_lead_s = split_edit.and_then(|s| s.audio_lead_s);
                spec.audio_lead_from_clip_id = audio_lead_from_clip_id;
            }
        }
    }
    if let Some(reason) = split_edit.and_then(|s| s.reason.clone()) {
        spec.reason = Some(reason);
    }
    if let Some(confidence) = split_edit.and_then(|s| s.confidence) {
        spec.confidence = Some(confidence);
    }
    montage.split_edit = Some(spec);
    Ok((clip_name, clip_id))
}

fn stamp_split_edit_cut_boundary(
    working: &mut Timeline,
    locator: ClipLocator,
    clip_id: &str,
    kind: SplitEditKind,
    offset_s: f64,
    split_edit: Option<&SplitEditSpec>,
) {
    let Some((from_id, to_id)) = neighboring_boundary_ids(working, locator, clip_id, kind) else {
        return;
    };
    let key = cut_boundary_key(&from_id, &to_id);
    let (cut_type, audio_relation, intent) = match kind {
        SplitEditKind::AudioLead => (CutType::JCut, AudioRelation::AudioLeads, "audio_lead"),
        SplitEditKind::AudioTrail => (CutType::LCut, AudioRelation::AudioTrails, "audio_trail"),
    };
    timeline_montage_metadata(working).cut_boundaries.insert(
        key,
        SemanticCutSpec {
            cut_type,
            intent: intent.into(),
            energy: None,
            audio_relation,
            confidence: split_edit.and_then(|s| s.confidence),
            reason: split_edit.and_then(|s| s.reason.clone()).or_else(|| {
                Some(format!(
                    "{} {offset_s:.3}s",
                    match kind {
                        SplitEditKind::AudioLead => "Incoming audio leads picture by",
                        SplitEditKind::AudioTrail => "Outgoing audio trails picture by",
                    }
                ))
            }),
            extra: Default::default(),
        },
    );
}

fn neighboring_boundary_ids(
    timeline: &Timeline,
    locator: ClipLocator,
    clip_id: &str,
    kind: SplitEditKind,
) -> Option<(String, String)> {
    let StackChild::Track(track) = &timeline.tracks.children.get(locator.track_index)? else {
        return None;
    };
    match kind {
        SplitEditKind::AudioLead => {
            let prev = previous_clip_id(track, locator.child_index)?;
            Some((prev, clip_id.to_string()))
        }
        SplitEditKind::AudioTrail => {
            let next = next_clip_id(track, locator.child_index)?;
            Some((clip_id.to_string(), next))
        }
    }
}

fn previous_clip_id(track: &montage_proto::otio::Track, before_index: usize) -> Option<String> {
    track.children[..before_index]
        .iter()
        .rev()
        .find_map(track_child_clip_id)
}

fn next_clip_id(track: &montage_proto::otio::Track, after_index: usize) -> Option<String> {
    track
        .children
        .iter()
        .skip(after_index + 1)
        .find_map(track_child_clip_id)
}

fn next_clip_id_after_gaps(
    track: &montage_proto::otio::Track,
    after_index: usize,
) -> Option<String> {
    track
        .children
        .iter()
        .skip(after_index + 1)
        .find(|child| !matches!(child, TrackChild::Gap(_)))
        .and_then(track_child_clip_id)
}

fn track_child_clip_id(child: &TrackChild) -> Option<String> {
    let TrackChild::Clip(clip) = child else {
        return None;
    };
    Some(clip_uuid(clip).unwrap_or(clip.name.as_str()).to_string())
}

fn apply_set_track_audio(
    working: &mut Timeline,
    index: usize,
    track_name: &str,
    role: Option<&str>,
    volume: Option<f64>,
    muted: Option<bool>,
    solo: Option<bool>,
) -> Result<String, ApplyError> {
    if let Some(volume) = volume
        && (!volume.is_finite() || volume < 0.0)
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_track_audio: volume {volume} must be finite and >= 0.0"),
        });
    }
    let track = find_track_mut(working, track_name).ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!("set_track_audio: track {track_name:?} not found"),
    })?;
    if !matches!(track.kind, TrackKind::Audio) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_track_audio: track {track_name:?} is not an audio track"),
        });
    }
    let mut value = track
        .metadata
        .get(AUDIO_TRACK_METADATA_KEY)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let map = value.as_object_mut().ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!(
            "set_track_audio: existing {AUDIO_TRACK_METADATA_KEY} metadata is not an object"
        ),
    })?;
    if let Some(role) = role {
        map.insert("role".into(), serde_json::Value::String(role.to_string()));
    }
    if let Some(volume) = volume {
        map.insert("volume".into(), serde_json::json!(volume));
    }
    if let Some(muted) = muted {
        map.insert("muted".into(), serde_json::json!(muted));
    }
    if let Some(solo) = solo {
        map.insert("solo".into(), serde_json::json!(solo));
    }
    track
        .metadata
        .insert(AUDIO_TRACK_METADATA_KEY.to_string(), value);
    Ok(format!("set audio controls on track {track_name:?}"))
}

fn apply_set_ducking(
    working: &mut Timeline,
    index: usize,
    track_name: &str,
    enabled: Option<bool>,
    amount_db: Option<f64>,
    attack_ms: Option<f64>,
    release_ms: Option<f64>,
) -> Result<String, ApplyError> {
    let enabled = enabled.unwrap_or(true);
    let amount_db = amount_db.unwrap_or(-12.0);
    let attack_ms = attack_ms.unwrap_or(80.0);
    let release_ms = release_ms.unwrap_or(300.0);
    if !amount_db.is_finite()
        || !attack_ms.is_finite()
        || !release_ms.is_finite()
        || attack_ms < 0.0
        || release_ms < 0.0
    {
        return Err(ApplyError::Invalid {
            index,
            message: "set_ducking: amount_db, attack_ms, and release_ms must be finite; timings must be >= 0.0".into(),
        });
    }
    let track = find_track_mut(working, track_name).ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!("set_ducking: track {track_name:?} not found"),
    })?;
    if !matches!(track.kind, TrackKind::Audio) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_ducking: track {track_name:?} is not an audio track"),
        });
    }
    let mut value = track
        .metadata
        .get(AUDIO_TRACK_METADATA_KEY)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let map = value.as_object_mut().ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!(
            "set_ducking: existing {AUDIO_TRACK_METADATA_KEY} metadata is not an object"
        ),
    })?;
    map.insert(
        "ducking".into(),
        serde_json::json!({
            "enabled": enabled,
            "amount_db": amount_db,
            "attack_ms": attack_ms,
            "release_ms": release_ms,
        }),
    );
    track
        .metadata
        .insert(AUDIO_TRACK_METADATA_KEY.to_string(), value);
    Ok(format!(
        "set ducking on track {track_name:?}: enabled={enabled} amount={amount_db:.1}dB"
    ))
}

#[allow(clippy::too_many_arguments)]
fn apply_set_sync_group(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    sync_group_id: &str,
    offset_s: f64,
    speed_factor: Option<f64>,
    confidence: Option<f64>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    if sync_group_id.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "set_sync_group: sync_group_id must not be empty".into(),
        });
    }
    if !offset_s.is_finite() {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_sync_group: offset_s {offset_s} must be finite"),
        });
    }
    if let Some(factor) = speed_factor
        && (!factor.is_finite() || factor <= 0.0)
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_sync_group: speed_factor {factor} must be finite and > 0.0"),
        });
    }
    if let Some(confidence) = confidence
        && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_sync_group: confidence {confidence} must be in [0, 1]"),
        });
    }

    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_sync_group: anchor resolved to a non-track stack child".into(),
        });
    };

    let clip_name = {
        let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
            return Err(ApplyError::Invalid {
                index,
                message: "set_sync_group: anchor resolved to a non-clip track child".into(),
            });
        };
        clip.effects
            .retain(|e| e.effect_name != SYNC_GROUP_EFFECT_NAME);
        let mut effect = montage_proto::otio::Effect::new(SYNC_GROUP_EFFECT_NAME);
        effect
            .metadata
            .insert("sync_group_id".into(), serde_json::json!(sync_group_id));
        effect
            .metadata
            .insert("offset_s".into(), serde_json::json!(offset_s));
        if let Some(speed_factor) = speed_factor {
            effect
                .metadata
                .insert("speed_factor".into(), serde_json::json!(speed_factor));
            if (speed_factor - 1.0).abs() > 1e-9 {
                clip.effects.retain(|e| e.effect_name != SPEED_EFFECT_NAME);
                let mut speed = montage_proto::otio::Effect::new(SPEED_EFFECT_NAME);
                speed
                    .metadata
                    .insert("factor".into(), serde_json::json!(speed_factor));
                clip.effects.push(speed);
            }
        }
        if let Some(confidence) = confidence {
            effect
                .metadata
                .insert("confidence".into(), serde_json::json!(confidence));
        }
        clip.effects.push(effect);
        clip.name.clone()
    };

    let aligned_to_s = offset_s.max(0.0);
    if aligned_to_s > 0.0 {
        apply_move_clip_to_time(index, track, locator.child_index, aligned_to_s)?;
    }
    Ok(format!(
        "set sync group {sync_group_id:?} on clip {clip_name:?}: offset={offset_s:.3}s{}{}",
        speed_factor
            .map(|v| format!(" speed_factor={v:.6}"))
            .unwrap_or_default(),
        confidence
            .map(|v| format!(" confidence={v:.2}"))
            .unwrap_or_default()
    ))
}

fn apply_set_clip_audio_fx(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    fx: &AudioFxConfig,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    validate_audio_fx(index, "set_clip_audio_fx", fx)?;
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_clip_audio_fx: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_clip_audio_fx: anchor resolved to a non-clip track child".into(),
        });
    };
    clip.effects
        .retain(|e| e.effect_name != AUDIO_FX_EFFECT_NAME);
    let mut effect = montage_proto::otio::Effect::new(AUDIO_FX_EFFECT_NAME);
    effect.metadata = audio_fx_metadata(index, fx)?;
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!("set audio FX on clip {clip_name:?}"))
}

fn apply_set_track_audio_fx(
    working: &mut Timeline,
    index: usize,
    track_name: &str,
    fx: &AudioFxConfig,
) -> Result<String, ApplyError> {
    validate_audio_fx(index, "set_track_audio_fx", fx)?;
    let track = find_track_mut(working, track_name).ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!("set_track_audio_fx: track {track_name:?} not found"),
    })?;
    if !matches!(track.kind, TrackKind::Audio) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_track_audio_fx: track {track_name:?} is not an audio track"),
        });
    }
    let mut value = track
        .metadata
        .get(AUDIO_TRACK_METADATA_KEY)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let map = value.as_object_mut().ok_or_else(|| ApplyError::Invalid {
        index,
        message: format!(
            "set_track_audio_fx: existing {AUDIO_TRACK_METADATA_KEY} metadata is not an object"
        ),
    })?;
    map.insert(
        "fx".into(),
        serde_json::Value::Object(audio_fx_metadata(index, fx)?),
    );
    track
        .metadata
        .insert(AUDIO_TRACK_METADATA_KEY.to_string(), value);
    Ok(format!("set audio FX on track {track_name:?}"))
}

fn validate_audio_fx(index: usize, label: &str, fx: &AudioFxConfig) -> Result<(), ApplyError> {
    if fx.is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: format!("{label}: at least one audio FX parameter is required"),
        });
    }
    let nums = [
        ("high_pass_hz", fx.high_pass_hz),
        ("low_pass_hz", fx.low_pass_hz),
        ("compressor_threshold_db", fx.compressor_threshold_db),
        ("compressor_ratio", fx.compressor_ratio),
        ("limiter_limit_db", fx.limiter_limit_db),
        ("noise_gate_threshold_db", fx.noise_gate_threshold_db),
        ("hum_notch_hz", fx.hum_notch_hz),
        ("de_ess_hz", fx.de_ess_hz),
        ("de_ess_reduction_db", fx.de_ess_reduction_db),
        ("loudnorm_i", fx.loudnorm_i),
        ("loudnorm_tp", fx.loudnorm_tp),
    ];
    for (name, value) in nums {
        if let Some(value) = value
            && !value.is_finite()
        {
            return Err(ApplyError::Invalid {
                index,
                message: format!("{label}: {name} must be finite"),
            });
        }
    }
    for band in &fx.eq_bands {
        if !band.freq_hz.is_finite()
            || !band.gain_db.is_finite()
            || band.width_hz.is_some_and(|v| !v.is_finite())
        {
            return Err(ApplyError::Invalid {
                index,
                message: format!("{label}: EQ band values must be finite"),
            });
        }
    }
    Ok(())
}

fn audio_fx_metadata(
    index: usize,
    fx: &AudioFxConfig,
) -> Result<serde_json::Map<String, serde_json::Value>, ApplyError> {
    serde_json::to_value(fx)
        .map_err(|e| ApplyError::Invalid {
            index,
            message: format!("audio_fx: serialize config failed: {e}"),
        })?
        .as_object()
        .cloned()
        .ok_or_else(|| ApplyError::Invalid {
            index,
            message: "audio_fx: serialized config was not an object".into(),
        })
}

fn apply_set_effect(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    effect_id: &str,
    params: &serde_json::Map<String, serde_json::Value>,
    rationale: Option<&str>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let (definition, mut metadata) =
        montage_effects::normalize_params(effect_id, params).map_err(|e| ApplyError::Invalid {
            index,
            message: format!("set_effect: {e}"),
        })?;
    if definition.id == LUT_EFFECT_NAME {
        normalize_lut_metadata(index, &mut metadata, ctx)?;
    }
    if definition.id == COLOR_PIPELINE_EFFECT_NAME {
        normalize_color_pipeline_metadata(index, &mut metadata, ctx)?;
    }
    if !matches!(definition.scope, montage_effects::EffectScope::Clip) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_effect: effect {effect_id:?} is not a clip-scoped effect"),
        });
    }
    if let Some(rationale) = rationale
        && !rationale.trim().is_empty()
    {
        metadata.insert(
            "rationale".to_string(),
            serde_json::Value::String(rationale.to_string()),
        );
    }

    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_effect: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_effect: anchor resolved to a non-clip track child".into(),
        });
    };

    if matches!(definition.stack_policy, StackPolicy::ReplaceSameId) {
        clip.effects.retain(|e| e.effect_name != definition.id);
    }
    let mut effect = montage_proto::otio::Effect::new(definition.id);
    effect.name = definition.display_name.to_string();
    effect.metadata = metadata;
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!(
        "set effect {} on clip {clip_name:?}",
        definition.id
    ))
}

/// Stamp an montage.speed Effect on the anchored clip with `factor`
/// (playback rate multiplier). Idempotent: any existing
/// montage.speed effect on the clip is removed first.
fn apply_set_speed(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    factor: f64,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    if !factor.is_finite() || factor <= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_speed: factor {factor} must be finite and > 0.0"),
        });
    }
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_speed: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_speed: anchor resolved to a non-clip track child".into(),
        });
    };
    clip.effects.retain(|e| e.effect_name != SPEED_EFFECT_NAME);
    let mut effect = montage_proto::otio::Effect::new(SPEED_EFFECT_NAME);
    effect
        .metadata
        .insert("factor".to_string(), serde_json::json!(factor));
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!(
        "set speed on clip {clip_name:?} to {factor:.3}× (timeline duration scales by 1/factor)"
    ))
}

fn apply_set_time_remap(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    curve: &[serde_json::Value],
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let mut params = serde_json::Map::new();
    params.insert(
        "curve".to_string(),
        serde_json::Value::Array(curve.to_vec()),
    );
    apply_set_effect(
        working,
        index,
        anchor,
        montage_effects::TIME_REMAP,
        &params,
        None,
        ctx,
        locator,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_set_freeze(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    freeze_at_source_s: f64,
    duration_s: f64,
    freeze_position: Option<&str>,
    audio_behavior: Option<&str>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let mut params = serde_json::Map::new();
    params.insert(
        "freeze_at_source_s".to_string(),
        serde_json::json!(freeze_at_source_s),
    );
    params.insert("duration_s".to_string(), serde_json::json!(duration_s));
    if let Some(freeze_position) = freeze_position {
        params.insert(
            "freeze_position".to_string(),
            serde_json::Value::String(freeze_position.to_string()),
        );
    }
    if let Some(audio_behavior) = audio_behavior {
        params.insert(
            "audio_behavior".to_string(),
            serde_json::Value::String(audio_behavior.to_string()),
        );
    }
    apply_set_effect(
        working,
        index,
        anchor,
        montage_effects::FREEZE,
        &params,
        None,
        ctx,
        locator,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_set_color_correction(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    exposure_ev: Option<f64>,
    contrast: Option<f64>,
    saturation: Option<f64>,
    temperature: Option<f64>,
    tint: Option<f64>,
    shadows: Option<f64>,
    highlights: Option<f64>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let fields = [
        ("exposure_ev", exposure_ev, -4.0, 4.0),
        ("contrast", contrast, 0.0, 3.0),
        ("saturation", saturation, 0.0, 3.0),
        ("temperature", temperature, -1.0, 1.0),
        ("tint", tint, -1.0, 1.0),
        ("shadows", shadows, -1.0, 1.0),
        ("highlights", highlights, -1.0, 1.0),
    ];
    if fields.iter().all(|(_, value, _, _)| value.is_none()) {
        return Err(ApplyError::Invalid {
            index,
            message: "set_color_correction: provide at least one correction field".into(),
        });
    }
    for (name, value, min, max) in fields {
        if let Some(value) = value {
            if !value.is_finite() || value < min || value > max {
                return Err(ApplyError::Invalid {
                    index,
                    message: format!(
                        "set_color_correction: {name} {value} must be finite and within [{min}, {max}]"
                    ),
                });
            }
        }
    }

    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_color_correction: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_color_correction: anchor resolved to a non-clip track child".into(),
        });
    };

    clip.effects
        .retain(|e| e.effect_name != COLOR_CORRECTION_EFFECT_NAME);
    let mut effect = montage_proto::otio::Effect::new(COLOR_CORRECTION_EFFECT_NAME);
    for (name, value, _, _) in [
        ("exposure_ev", exposure_ev, -4.0, 4.0),
        ("contrast", contrast, 0.0, 3.0),
        ("saturation", saturation, 0.0, 3.0),
        ("temperature", temperature, -1.0, 1.0),
        ("tint", tint, -1.0, 1.0),
        ("shadows", shadows, -1.0, 1.0),
        ("highlights", highlights, -1.0, 1.0),
    ] {
        if let Some(value) = value {
            effect
                .metadata
                .insert(name.to_string(), serde_json::json!(value));
        }
    }
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!("set color correction on clip {clip_name:?}"))
}

fn apply_lut(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    lut_path: &str,
    interpolation: Option<&str>,
    strength: Option<f64>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = anchor;
    let lut_path = normalize_lut_path(index, lut_path)?;
    let interpolation = normalize_lut_interpolation(index, interpolation)?;
    let strength = normalize_lut_strength(index, strength)?;
    if let Some(root) = ctx.project_root() {
        validate_lut_contents(index, root, &lut_path, ExpectedLutKind::ThreeD)?;
    }
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "apply_lut: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "apply_lut: anchor resolved to a non-clip track child".into(),
        });
    };
    clip.effects.retain(|e| e.effect_name != LUT_EFFECT_NAME);
    let mut effect = montage_proto::otio::Effect::new(LUT_EFFECT_NAME);
    effect
        .metadata
        .insert("lut_path".to_string(), serde_json::json!(lut_path));
    if let Some(interpolation) = interpolation {
        effect.metadata.insert(
            "interpolation".to_string(),
            serde_json::json!(interpolation),
        );
    }
    if let Some(strength) = strength {
        effect
            .metadata
            .insert("strength".to_string(), serde_json::json!(strength));
    }
    let clip_name = clip.name.clone();
    clip.effects.push(effect);
    Ok(format!("applied LUT {lut_path:?} to clip {clip_name:?}"))
}

fn apply_remove_lut(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "remove_lut: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "remove_lut: anchor resolved to a non-clip track child".into(),
        });
    };
    let before = clip.effects.len();
    clip.effects.retain(|e| e.effect_name != LUT_EFFECT_NAME);
    let clip_name = clip.name.clone();
    if clip.effects.len() == before {
        Ok(format!("clip {clip_name:?} had no LUT effect"))
    } else {
        Ok(format!("removed LUT from clip {clip_name:?}"))
    }
}

fn normalize_lut_metadata(
    index: usize,
    metadata: &mut serde_json::Map<String, serde_json::Value>,
    ctx: &AnchorContext,
) -> Result<(), ApplyError> {
    let lut_path = metadata
        .get("lut_path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ApplyError::Invalid {
            index,
            message: "set_effect: montage.lut requires string lut_path".into(),
        })?;
    let normalized_path = normalize_lut_path(index, lut_path)?;
    // Mirror apply_lut: when the project root is known, parse the
    // file so a malformed `.cube` or `.3dl` fails at apply time
    // rather than mid-render. Without this, agents could route a
    // bad LUT through `Set Effect { effect: montage.lut }` and skip
    // the validation that `Apply LUT` runs.
    if let Some(root) = ctx.project_root() {
        validate_lut_contents(index, root, &normalized_path, ExpectedLutKind::ThreeD)?;
    }
    metadata.insert(
        "lut_path".to_string(),
        serde_json::Value::String(normalized_path),
    );
    let interpolation = metadata
        .get("interpolation")
        .and_then(serde_json::Value::as_str);
    match normalize_lut_interpolation(index, interpolation)? {
        Some(value) => {
            metadata.insert(
                "interpolation".to_string(),
                serde_json::Value::String(value),
            );
        }
        None => {
            metadata.remove("interpolation");
        }
    }
    let strength = metadata.get("strength").and_then(serde_json::Value::as_f64);
    match normalize_lut_strength(index, strength)? {
        Some(value) => {
            let Some(number) = serde_json::Number::from_f64(value) else {
                return Err(ApplyError::Invalid {
                    index,
                    message: format!("set_effect: montage.lut strength {value} is non-finite"),
                });
            };
            metadata.insert("strength".to_string(), serde_json::Value::Number(number));
        }
        None => {
            metadata.remove("strength");
        }
    }
    Ok(())
}

fn normalize_lut_path(index: usize, lut_path: &str) -> Result<String, ApplyError> {
    let trimmed = lut_path.trim();
    let path = std::path::Path::new(trimmed);
    if trimmed.is_empty()
        || trimmed.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(ApplyError::Invalid {
            index,
            message:
                "apply_lut: lut_path must be a non-empty project-relative path without '.', '..', absolute prefixes, or backslashes"
                    .into(),
        });
    }
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    if !extension
        .as_deref()
        .is_some_and(|ext| SUPPORTED_LUT_EXTENSIONS.contains(&ext))
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "apply_lut: lut_path must end in one of: {}",
                SUPPORTED_LUT_EXTENSIONS.join(", ")
            ),
        });
    }
    Ok(trimmed.to_string())
}

/// Validate the metadata for an `montage.color_pipeline` effect.
///
/// Beyond the per-param schema checks the effects registry already
/// runs, this layer enforces:
/// - color-space slot values are in [`montage_effects::KNOWN_COLOR_SPACES`]
/// - LUT path slots pass [`normalize_lut_path`] (project-relative,
///   supported extension)
/// - `look_interpolation` is a known interpolation mode
/// - cross-slot consistency (e.g., `look_strength` without `look_lut`
///   is meaningless and rejected)
/// - if the project root is known and the `look_lut` is a `.cube`
///   file that exists on disk, parse it via [`montage_lut::parse_cube_3d`]
fn normalize_color_pipeline_metadata(
    index: usize,
    metadata: &mut serde_json::Map<String, serde_json::Value>,
    ctx: &AnchorContext,
) -> Result<(), ApplyError> {
    for slot in [
        "clip_input_space",
        "working_space",
        "lut_input_space",
        "output_space",
    ] {
        let Some(value) = metadata.get(slot).and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !montage_effects::is_known_color_space(value) {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "set_effect: montage.color_pipeline {slot} {value:?} is not a known color space; \
                     expected one of {:?}",
                    montage_effects::KNOWN_COLOR_SPACES
                ),
            });
        }
    }

    // Each slot expects a particular LUT kind. Render lowers
    // `shaper_lut` with FFmpeg's `lut1d` and the others with
    // `lut3d`; validating the kind here means a `.cube` 3D LUT
    // stamped on `shaper_lut` is rejected at apply time instead of
    // crashing FFmpeg mid-render.
    let slot_kinds: &[(&str, ExpectedLutKind)] = &[
        ("input_transform_lut", ExpectedLutKind::ThreeD),
        ("shaper_lut", ExpectedLutKind::OneD),
        ("look_lut", ExpectedLutKind::ThreeD),
        ("output_transform_lut", ExpectedLutKind::ThreeD),
    ];
    for (slot, kind) in slot_kinds {
        let Some(value) = metadata.get(*slot).and_then(serde_json::Value::as_str) else {
            continue;
        };
        let normalized = normalize_lut_path(index, value).map_err(|err| {
            // Surface the slot name so the agent knows which path
            // was wrong when multiple LUTs are stamped on one
            // effect.
            ApplyError::Invalid {
                index,
                message: match err {
                    ApplyError::Invalid { message, .. } => {
                        format!("set_effect: montage.color_pipeline {slot}: {message}")
                    }
                    other => format!("set_effect: montage.color_pipeline {slot}: {other}"),
                },
            }
        })?;
        if let Some(root) = ctx.project_root() {
            validate_lut_contents(index, root, &normalized, *kind).map_err(|err| match err {
                ApplyError::Invalid { message, .. } => ApplyError::Invalid {
                    index,
                    message: format!("set_effect: montage.color_pipeline {slot}: {message}"),
                },
                other => other,
            })?;
        }
        metadata.insert(slot.to_string(), serde_json::Value::String(normalized));
    }

    // Mask is a different file kind from a LUT, so it gets its own
    // path normalizer instead of reusing `normalize_lut_path`'s
    // extension whitelist.
    if let Some(value) = metadata
        .get("mask_source")
        .and_then(serde_json::Value::as_str)
    {
        let normalized = normalize_mask_path(index, value)?;
        metadata.insert(
            "mask_source".to_string(),
            serde_json::Value::String(normalized),
        );
    }

    let interpolation = metadata
        .get("look_interpolation")
        .and_then(serde_json::Value::as_str);
    match normalize_lut_interpolation(index, interpolation)? {
        Some(value) => {
            metadata.insert(
                "look_interpolation".to_string(),
                serde_json::Value::String(value),
            );
        }
        None => {
            metadata.remove("look_interpolation");
        }
    }

    // Cross-slot consistency: any slot that customizes the look LUT
    // needs the look LUT itself to be present.
    let has_look_lut = metadata.contains_key("look_lut");
    for dependent in ["look_interpolation", "lut_input_space"] {
        if metadata.contains_key(dependent) && !has_look_lut {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "set_effect: montage.color_pipeline {dependent} requires look_lut to also be set"
                ),
            });
        }
    }
    if let Some(strength) = metadata
        .get("look_strength")
        .and_then(serde_json::Value::as_f64)
        && (strength - 1.0).abs() > f64::EPSILON
        && !has_look_lut
    {
        return Err(ApplyError::Invalid {
            index,
            message: "set_effect: montage.color_pipeline look_strength != 1.0 requires look_lut"
                .into(),
        });
    }

    // Look LUT content validation already ran above in the
    // per-slot loop. (The redundant pass that used to live here
    // would have invoked `validate_lut_contents` a second time.)
    Ok(())
}

/// Validate an `montage.color_pipeline.mask_source` path. Mirrors
/// `normalize_lut_path` (project-relative, no `..`, no backslashes)
/// but swaps the LUT extension whitelist for
/// [`montage_effects::SUPPORTED_MASK_EXTENSIONS`].
fn normalize_mask_path(index: usize, mask_source: &str) -> Result<String, ApplyError> {
    let trimmed = mask_source.trim();
    let path = std::path::Path::new(trimmed);
    if trimmed.is_empty()
        || trimmed.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(ApplyError::Invalid {
            index,
            message:
                "set_effect: montage.color_pipeline mask_source must be a non-empty project-relative path without '.', '..', absolute prefixes, or backslashes"
                    .into(),
        });
    }
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    if !extension
        .as_deref()
        .is_some_and(|ext| montage_effects::SUPPORTED_MASK_EXTENSIONS.contains(&ext))
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "set_effect: montage.color_pipeline mask_source must end in one of: {}",
                montage_effects::SUPPORTED_MASK_EXTENSIONS.join(", ")
            ),
        });
    }
    Ok(trimmed.to_string())
}

fn normalize_lut_strength(index: usize, strength: Option<f64>) -> Result<Option<f64>, ApplyError> {
    let Some(value) = strength else {
        return Ok(None);
    };
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(ApplyError::Invalid {
            index,
            message: format!("apply_lut: strength {value} must be finite and in 0.0..=1.0"),
        });
    }
    Ok(Some(value))
}

/// Which LUT kind a slot expects. The legacy `montage.lut` effect
/// and most `montage.color_pipeline` LUT slots are 3D; only the
/// `shaper_lut` slot wants a 1D table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedLutKind {
    /// 3D LUT (cube). Default for IDT/look/ODT slots.
    ThreeD,
    /// 1D LUT (per-channel response curve). Shaper slot only.
    OneD,
}

/// Extensions montage knows to always carry 3D data — listing one
/// of these in a 1D slot is rejected at apply time before render.
const THREE_D_ONLY_LUT_EXTENSIONS: &[&str] = &["3dl", "dat", "m3d"];

/// Parse the referenced LUT and surface structured errors at apply
/// time. Missing files are still allowed here — `render` catches
/// those with its own `MissingLut` error so the existing "stamp
/// now, render later" contract is preserved.
///
/// `expected` selects which kind of LUT the slot wants; mismatches
/// (e.g. a 3D `.cube` in a 1D slot) are rejected with a clear
/// message that tells the agent which slot and which kind. For
/// extensions montage doesn't yet parse (`.csp`, `.dat`, `.m3d`,
/// `.look`), the kind check is extension-only — `THREE_D_ONLY_LUT_EXTENSIONS`
/// gets rejected in 1D slots, everything else falls through.
fn validate_lut_contents(
    index: usize,
    project_root: &std::path::Path,
    lut_path: &str,
    expected: ExpectedLutKind,
) -> Result<(), ApplyError> {
    let full = project_root.join(lut_path);
    let extension = full
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    let Some(ext) = extension.as_deref() else {
        return Ok(());
    };
    // Extension-only kind check for formats we don't parse yet.
    if matches!(expected, ExpectedLutKind::OneD) && THREE_D_ONLY_LUT_EXTENSIONS.contains(&ext) {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "apply_lut: {lut_path:?} is a .{ext} (3D-only format) but the slot expects a 1D LUT"
            ),
        });
    }
    if !matches!(ext, "cube" | "3dl") {
        return Ok(());
    }
    let bytes = match std::fs::read(&full) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(ApplyError::Invalid {
                index,
                message: format!("apply_lut: failed to read {lut_path:?}: {err}"),
            });
        }
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(_) => {
            return Err(ApplyError::Invalid {
                index,
                message: format!("apply_lut: {lut_path:?} is not valid UTF-8"),
            });
        }
    };
    let parse_result = match (ext, expected) {
        ("cube", ExpectedLutKind::ThreeD) => montage_lut::parse_cube_3d(text).map(|_| ()),
        ("cube", ExpectedLutKind::OneD) => montage_lut::parse_cube_1d(text).map(|_| ()),
        ("3dl", ExpectedLutKind::ThreeD) => montage_lut::parse_3dl(text).map(|_| ()),
        // `.3dl` in a 1D slot was rejected above on extension; this
        // arm is unreachable but keeps the match exhaustive.
        ("3dl", ExpectedLutKind::OneD) => {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "apply_lut: {lut_path:?} is .3dl (always 3D) but the slot expects 1D"
                ),
            });
        }
        _ => return Ok(()),
    };
    parse_result.map_err(|err| ApplyError::Invalid {
        index,
        message: format!("apply_lut: {lut_path:?} is not a valid .{ext} LUT: {err}"),
    })?;
    Ok(())
}

fn normalize_lut_interpolation(
    index: usize,
    interpolation: Option<&str>,
) -> Result<Option<String>, ApplyError> {
    let Some(interpolation) = interpolation else {
        return Ok(None);
    };
    let interpolation = interpolation.trim().to_ascii_lowercase();
    if interpolation.is_empty() {
        return Ok(None);
    }
    if !SUPPORTED_LUT_INTERPOLATIONS.contains(&interpolation.as_str()) {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "apply_lut: interpolation must be one of: {}",
                SUPPORTED_LUT_INTERPOLATIONS.join(", ")
            ),
        });
    }
    Ok(Some(interpolation))
}

fn validate_project_relative_asset(
    index: usize,
    op_name: &str,
    asset: &str,
) -> Result<(), ApplyError> {
    let path = std::path::Path::new(asset);
    if asset.trim().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "{op_name}: asset must be a non-empty project-relative path without '..'"
            ),
        });
    }
    Ok(())
}

fn stamp_video_overlay_effect(
    clip: &mut montage_proto::otio::Clip,
    mode: &str,
    corner: Option<super::op::PiPCorner>,
    scale: Option<f64>,
    margin_pct: Option<f64>,
) {
    clip.effects
        .retain(|e| e.effect_name != VIDEO_OVERLAY_EFFECT_NAME);
    let mut effect = montage_proto::otio::Effect::new(VIDEO_OVERLAY_EFFECT_NAME);
    effect
        .metadata
        .insert("mode".to_string(), serde_json::json!(mode));
    if let Some(corner) = corner {
        effect.metadata.insert(
            "corner".to_string(),
            serde_json::json!(pip_corner_str(corner)),
        );
    }
    if let Some(scale) = scale {
        effect
            .metadata
            .insert("scale".to_string(), serde_json::json!(scale));
    }
    if let Some(margin_pct) = margin_pct {
        effect
            .metadata
            .insert("margin_pct".to_string(), serde_json::json!(margin_pct));
    }
    clip.effects.push(effect);
}

fn pip_corner_str(corner: super::op::PiPCorner) -> &'static str {
    match corner {
        super::op::PiPCorner::TopLeft => "top_left",
        super::op::PiPCorner::TopRight => "top_right",
        super::op::PiPCorner::BottomLeft => "bottom_left",
        super::op::PiPCorner::BottomRight => "bottom_right",
    }
}

/// Insert a title overlay onto the project's Titles track. The
/// titles track auto-creates on first call (Video kind, flagged via
/// track metadata). The overlay is structurally a Clip with a
/// MissingReference media_reference (no real media — drawtext
/// renders the title from the montage.title Effect's metadata) and
/// a source_range whose duration matches `end_s - start_s`.
///
/// Stamps a fresh clip_uuid on the synthesized clip so subsequent
/// `Set Title` ops can resolve via `Anchor::ClipUuid`.
#[allow(clippy::too_many_arguments)]
fn apply_insert_title(
    working: &mut Timeline,
    index: usize,
    start_s: f64,
    end_s: f64,
    text: &str,
    position: super::op::TitlePosition,
    font_size: u32,
    color: &str,
    font_weight: super::op::TitleWeight,
    animation: super::op::TitleAnimation,
    phases: Option<super::op::TitlePhases>,
    font_family: Option<&str>,
    font_path: Option<&str>,
) -> Result<String, ApplyError> {
    apply_insert_text_overlay(
        working,
        index,
        "title",
        None,
        None,
        font_overlay_metadata(font_family, font_path),
        start_s,
        end_s,
        text,
        position,
        font_size,
        color,
        font_weight,
        animation,
        phases,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_insert_rich_title(
    working: &mut Timeline,
    index: usize,
    start_s: f64,
    end_s: f64,
    segments: &[super::op::RichTextSegment],
    position: super::op::TitlePosition,
    font_size: u32,
    animation: super::op::TitleAnimation,
    phases: Option<super::op::TitlePhases>,
    font_family: Option<&str>,
    font_path: Option<&str>,
) -> Result<String, ApplyError> {
    if segments.is_empty() || segments.iter().all(|segment| segment.text.is_empty()) {
        return Err(ApplyError::Invalid {
            index,
            message: "insert_rich_title: segments_json must contain non-empty text".into(),
        });
    }
    let text = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    let mut extra_metadata = font_overlay_metadata(font_family, font_path).unwrap_or_default();
    extra_metadata.insert(
        "rich_segments".into(),
        serde_json::to_value(segments).map_err(|error| ApplyError::Invalid {
            index,
            message: format!("insert_rich_title: rich segments could not serialize: {error}"),
        })?,
    );
    apply_insert_text_overlay(
        working,
        index,
        "title",
        None,
        None,
        Some(extra_metadata),
        start_s,
        end_s,
        &text,
        position,
        font_size,
        "#FFFFFF",
        super::op::TitleWeight::Normal,
        animation,
        phases,
    )
}

/// Insert a caption overlay as a graph node. Captions intentionally
/// reuse the title render effect so the graph has one overlay track
/// while still preserving `role = "caption"` for downstream format
/// and audit tooling.
#[allow(clippy::too_many_arguments)]
fn apply_insert_caption(
    working: &mut Timeline,
    index: usize,
    start_s: f64,
    end_s: f64,
    text: &str,
    position: super::op::TitlePosition,
    font_size: u32,
    color: &str,
    safe_area: &str,
    word_timings: &[super::op::CaptionWordTiming],
    style_json: Option<&serde_json::Value>,
) -> Result<String, ApplyError> {
    if safe_area.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: "insert_caption: safe_area must be non-empty".into(),
        });
    }
    for word in word_timings {
        if word.text.trim().is_empty()
            || !word.start_s.is_finite()
            || !word.end_s.is_finite()
            || word.end_s < word.start_s
        {
            return Err(ApplyError::Invalid {
                index,
                message:
                    "insert_caption: word_timings must have non-empty text and finite ordered times"
                        .into(),
            });
        }
    }
    let extra_metadata = if word_timings.is_empty() && style_json.is_none() {
        None
    } else {
        let mut metadata = serde_json::Map::new();
        if !word_timings.is_empty() {
            metadata.insert(
                "word_timings".to_string(),
                serde_json::to_value(word_timings).map_err(|e| ApplyError::Invalid {
                    index,
                    message: format!("insert_caption: failed to serialize word_timings: {e}"),
                })?,
            );
        }
        if let Some(style) = style_json {
            metadata.insert("caption_style".to_string(), style.clone());
        }
        Some(metadata)
    };
    apply_insert_text_overlay(
        working,
        index,
        "caption",
        Some(safe_area),
        None,
        extra_metadata,
        start_s,
        end_s,
        text,
        position,
        font_size,
        color,
        super::op::TitleWeight::Bold,
        super::op::TitleAnimation::None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_insert_annotation(
    working: &mut Timeline,
    index: usize,
    start_s: f64,
    end_s: f64,
    kind: AnnotationKind,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    color: &str,
    stroke_width: u32,
    label: Option<&str>,
) -> Result<String, ApplyError> {
    use montage_proto::otio::{Clip, RationalTime, TimeRange};

    if !start_s.is_finite() || !end_s.is_finite() || end_s <= start_s {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "insert_annotation: invalid window [{start_s}..{end_s}]; end_s must be finite and > start_s"
            ),
        });
    }
    if !valid_normalized_region(x, y, width, height) {
        return Err(ApplyError::Invalid {
            index,
            message:
                "insert_annotation: x/y/width/height must be finite normalized values in 0..=1"
                    .into(),
        });
    }
    if !valid_graphic_color(color) {
        return Err(ApplyError::Invalid {
            index,
            message:
                "insert_annotation: color must be #RGB/#RGBA/#RRGGBB/#RRGGBBAA hex or an alphanumeric color name"
                    .into(),
        });
    }
    if stroke_width == 0 {
        return Err(ApplyError::Invalid {
            index,
            message: "insert_annotation: stroke_width must be greater than zero".into(),
        });
    }

    let annotations_idx = find_or_create_annotations_track(working);
    let StackChild::Track(track) = &mut working.tracks.children[annotations_idx] else {
        return Err(ApplyError::Invalid {
            index,
            message: "insert_annotation: annotations track resolved to a non-track child".into(),
        });
    };

    let rate = 24.0_f64;
    let mut clip = Clip::empty(format!(
        "annotation-{}-{:.3}-{:.3}",
        annotation_kind_str(kind),
        start_s,
        end_s
    ));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(start_s * rate, rate),
        RationalTime::new((end_s - start_s) * rate, rate),
    ));
    stamp_fresh_clip_uuid(&mut clip);

    let mut effect = montage_proto::otio::Effect::new(ANNOTATION_EFFECT_NAME);
    effect
        .metadata
        .insert("kind".into(), serde_json::json!(annotation_kind_str(kind)));
    effect.metadata.insert("x".into(), serde_json::json!(x));
    effect.metadata.insert("y".into(), serde_json::json!(y));
    effect
        .metadata
        .insert("width".into(), serde_json::json!(width));
    effect
        .metadata
        .insert("height".into(), serde_json::json!(height));
    effect
        .metadata
        .insert("start_s".into(), serde_json::json!(start_s));
    effect
        .metadata
        .insert("end_s".into(), serde_json::json!(end_s));
    effect
        .metadata
        .insert("color".into(), serde_json::json!(color));
    effect
        .metadata
        .insert("stroke_width".into(), serde_json::json!(stroke_width));
    if let Some(label) = label.filter(|label| !label.trim().is_empty()) {
        effect
            .metadata
            .insert("label".into(), serde_json::json!(label));
    }
    clip.effects.push(effect);

    let position_idx = title_insertion_index(track, start_s);
    track.children.insert(position_idx, TrackChild::Clip(clip));

    Ok(format!(
        "inserted {kind:?} annotation at [{start_s:.3}s..{end_s:.3}s]"
    ))
}

fn valid_normalized_region(x: f64, y: f64, width: f64, height: f64) -> bool {
    [x, y, width, height].into_iter().all(f64::is_finite)
        && x >= 0.0
        && y >= 0.0
        && width > 0.0
        && height > 0.0
        && x + width <= 1.0
        && y + height <= 1.0
}

fn annotation_kind_str(kind: AnnotationKind) -> &'static str {
    match kind {
        AnnotationKind::Rectangle => "rectangle",
        AnnotationKind::Circle => "circle",
        AnnotationKind::Arrow => "arrow",
        AnnotationKind::Bracket => "bracket",
        AnnotationKind::Blur => "blur",
    }
}

/// Build optional `font_family` / `font_path` effect metadata for a
/// title overlay. Returns `None` when neither is set so the render
/// layer falls back to its system-font probe (unchanged behavior).
fn font_overlay_metadata(
    font_family: Option<&str>,
    font_path: Option<&str>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let mut metadata = serde_json::Map::new();
    if let Some(family) = font_family.filter(|s| !s.trim().is_empty()) {
        metadata.insert("font_family".to_string(), serde_json::json!(family));
    }
    if let Some(path) = font_path.filter(|s| !s.trim().is_empty()) {
        metadata.insert("font_path".to_string(), serde_json::json!(path));
    }
    if metadata.is_empty() {
        None
    } else {
        Some(metadata)
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_insert_text_overlay(
    working: &mut Timeline,
    index: usize,
    role: &str,
    safe_area: Option<&str>,
    clip_uuid: Option<&str>,
    extra_metadata: Option<serde_json::Map<String, serde_json::Value>>,
    start_s: f64,
    end_s: f64,
    text: &str,
    position: super::op::TitlePosition,
    font_size: u32,
    color: &str,
    font_weight: super::op::TitleWeight,
    animation: super::op::TitleAnimation,
    phases: Option<super::op::TitlePhases>,
) -> Result<String, ApplyError> {
    use montage_proto::otio::{Clip, RationalTime, TimeRange};

    if !start_s.is_finite() || !end_s.is_finite() || end_s <= start_s {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "insert_{role}: invalid window [{start_s}..{end_s}]; end_s must be finite and > start_s"
            ),
        });
    }
    if text.is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: format!("insert_{role}: text must be non-empty"),
        });
    }

    // Find or create the Titles track. Match by metadata flag first
    // (the user may have renamed it), then by the canonical name as
    // a fallback.
    let titles_idx = find_or_create_titles_track(working);
    let StackChild::Track(track) = &mut working.tracks.children[titles_idx] else {
        return Err(ApplyError::Invalid {
            index,
            message: format!("insert_{role}: titles track resolved to a non-track stack child"),
        });
    };

    // The title's source_range is interpreted in *timeline-time*
    // here — the Titles track is treated as a top-level container,
    // not a media-bearing track. Render walks `track_role: titles`
    // tracks and reads start_s/end_s from each clip's source_range
    // for the drawtext `enable=between(t,...)` window.
    let rate = 24.0_f64;
    let duration_s = end_s - start_s;
    let mut clip = Clip::empty(format!("{role}-{:.3}-{:.3}", start_s, end_s));
    clip.source_range = Some(TimeRange::new(
        RationalTime::new(start_s * rate, rate),
        RationalTime::new(duration_s * rate, rate),
    ));
    if let Some(uuid) = clip_uuid {
        stamp_clip_uuid(&mut clip, uuid);
    } else {
        stamp_fresh_clip_uuid(&mut clip);
    }

    // Build the montage.title effect with all the styling.
    let mut effect = montage_proto::otio::Effect::new(TITLE_EFFECT_NAME);
    effect
        .metadata
        .insert("role".to_string(), serde_json::json!(role));
    effect
        .metadata
        .insert("text".to_string(), serde_json::json!(text));
    effect
        .metadata
        .insert("start_s".to_string(), serde_json::json!(start_s));
    effect
        .metadata
        .insert("end_s".to_string(), serde_json::json!(end_s));
    effect.metadata.insert(
        "position".to_string(),
        serde_json::json!(title_position_str(position)),
    );
    effect
        .metadata
        .insert("font_size".to_string(), serde_json::json!(font_size));
    effect
        .metadata
        .insert("color".to_string(), serde_json::json!(color));
    effect.metadata.insert(
        "font_weight".to_string(),
        serde_json::json!(title_weight_str(font_weight)),
    );
    effect.metadata.insert(
        "animation".to_string(),
        serde_json::json!(title_animation_str(animation)),
    );
    if let Some(phases) = phases {
        let serialized = serde_json::to_value(phases).map_err(|error| ApplyError::Invalid {
            index,
            message: format!("insert_{role}: phases could not serialize: {error}"),
        })?;
        effect.metadata.insert("phases".to_string(), serialized);
    }
    if let Some(profile) = safe_area {
        effect
            .metadata
            .insert("safe_area".to_string(), serde_json::json!(profile));
    }
    if let Some(extra_metadata) = extra_metadata {
        effect.metadata.extend(extra_metadata);
    }
    clip.effects.push(effect);

    // Insert the title in playback order so the Titles track stays
    // sorted by start_s. This makes render's left-to-right walk
    // produce drawtext filters in time order — a small nicety for
    // anyone reading the generated argv.
    let position_idx = title_insertion_index(track, start_s);
    track.children.insert(position_idx, TrackChild::Clip(clip));

    Ok(format!(
        "inserted {role} {text:?} on Titles track at [{start_s:.3}s..{end_s:.3}s] \
         (position={position:?}, animation={animation:?})"
    ))
}

fn timeline_montage_metadata(working: &mut Timeline) -> &mut MontageTimelineMetadata {
    let meta = working
        .metadata
        .montage
        .get_or_insert_with(MontageTimelineMetadata::default);
    if meta.version.is_empty() {
        meta.version = montage_proto::MONTAGE_PROJECT_VERSION.to_string();
    }
    meta
}

fn replace_by_id<T, F>(items: &mut Vec<T>, new_item: T, id: F)
where
    F: Fn(&T) -> &str,
{
    let new_id = id(&new_item).to_string();
    if let Some(existing) = items.iter_mut().find(|item| id(item) == new_id) {
        *existing = new_item;
    } else {
        items.push(new_item);
    }
}

fn apply_set_output_format(
    working: &mut Timeline,
    index: usize,
    aspect_ratio: &str,
    platform: Option<&str>,
    safe_area: Option<&str>,
) -> Result<String, ApplyError> {
    const SUPPORTED: &[&str] = &["16:9", "9:16", "1:1", "4:5"];
    if !SUPPORTED.contains(&aspect_ratio) {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "set_output_format: aspect_ratio {aspect_ratio:?} must be one of {SUPPORTED:?}"
            ),
        });
    }
    let meta = timeline_montage_metadata(working);
    meta.extra.insert(
        "output_format".into(),
        serde_json::json!({
            "aspect_ratio": aspect_ratio,
            "platform": platform,
            "safe_area": safe_area,
        }),
    );
    Ok(format!(
        "set output format to aspect_ratio={aspect_ratio:?}, platform={:?}, safe_area={:?}",
        platform, safe_area
    ))
}

fn apply_set_loudness_target(
    working: &mut Timeline,
    index: usize,
    integrated_lufs: f64,
    true_peak_db: Option<f64>,
) -> Result<String, ApplyError> {
    if !integrated_lufs.is_finite() || integrated_lufs >= 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "set_loudness_target: integrated_lufs {integrated_lufs} must be finite and below 0"
            ),
        });
    }
    if let Some(peak) = true_peak_db
        && (!peak.is_finite() || peak > 0.0)
    {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_loudness_target: true_peak_db {peak} must be finite and <= 0"),
        });
    }
    let meta = timeline_montage_metadata(working);
    meta.extra.insert(
        "loudness_target".into(),
        serde_json::json!({
            "integrated_lufs": integrated_lufs,
            "true_peak_db": true_peak_db,
        }),
    );
    Ok(format!(
        "set loudness target to {integrated_lufs:.1} LUFS, true_peak_db={true_peak_db:?}"
    ))
}

fn apply_set_package_metadata(
    working: &mut Timeline,
    index: usize,
    platform: Option<&str>,
    title: Option<&str>,
    description: Option<&str>,
    tags: Option<&str>,
) -> Result<String, ApplyError> {
    if platform.is_none() && title.is_none() && description.is_none() && tags.is_none() {
        return Err(ApplyError::Invalid {
            index,
            message: "set_package_metadata: at least one field is required".into(),
        });
    }
    let meta = timeline_montage_metadata(working);
    meta.extra.insert(
        "package_metadata".into(),
        serde_json::json!({
            "platform": platform,
            "title": title,
            "description": description,
            "tags": tags,
        }),
    );
    Ok(format!(
        "set package metadata platform={:?}, title={:?}",
        platform, title
    ))
}

fn apply_set_broadcast_overlay(
    working: &mut Timeline,
    index: usize,
    config: &BroadcastOverlayConfig,
) -> Result<String, ApplyError> {
    validate_broadcast_overlay_config(index, config)?;
    let meta = timeline_montage_metadata(working);
    let mut config = config.clone();
    if let Some(kit) = &meta.brand_kit {
        config.apply_brand_kit(kit);
    }
    meta.broadcast_overlay = Some(config.clone());
    Ok(format!(
        "set broadcast overlay enabled={}, title={:?}, topics={}, chapters={}",
        config.enabled,
        config.episode_title,
        config.topics.len(),
        config.chapters.len(),
    ))
}

fn apply_set_brand_kit(
    working: &mut Timeline,
    index: usize,
    kit: &BrandKit,
) -> Result<String, ApplyError> {
    validate_brand_kit(index, kit)?;
    let meta = timeline_montage_metadata(working);
    meta.brand_kit = Some(kit.clone());
    // Backfill an already-stored overlay so an existing overlay also picks
    // up newly-set shared brand identity, not just overlays set afterwards.
    if let Some(overlay) = meta.broadcast_overlay.as_mut() {
        overlay.apply_brand_kit(kit);
    }
    Ok(format!(
        "set brand kit (logo={}, palette={}, social_handles={})",
        kit.logo_path.is_some(),
        kit.palette.len(),
        kit.social_handles.len(),
    ))
}

fn validate_brand_kit(index: usize, kit: &BrandKit) -> Result<(), ApplyError> {
    validate_optional_project_path(index, "logo_path", kit.logo_path.as_deref())?;
    validate_optional_project_path(index, "font_primary_path", kit.font_primary_path.as_deref())?;
    validate_optional_project_path(
        index,
        "font_secondary_path",
        kit.font_secondary_path.as_deref(),
    )?;
    validate_optional_project_path(index, "music_path", kit.music_path.as_deref())?;
    // Palette entries flow into overlay color options (e.g. gold_hex →
    // drawbox/drawtext `color=`), which are interpolated straight into the
    // FFmpeg filter graph. Reject anything that isn't a plain hex/named color
    // so a malformed or option-bearing value can't corrupt the filter string.
    for (i, color) in kit.palette.iter().enumerate() {
        if !valid_graphic_color(color) {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "set_brand_kit: palette[{i}] must be #RGB/#RGBA/#RRGGBB/#RRGGBBAA hex or an alphanumeric color name"
                ),
            });
        }
    }
    Ok(())
}

fn validate_broadcast_overlay_config(
    index: usize,
    config: &BroadcastOverlayConfig,
) -> Result<(), ApplyError> {
    validate_optional_project_path(index, "brand_logo_path", config.brand_logo_path.as_deref())?;
    validate_optional_project_path(
        index,
        "host_a.photo_path",
        config.host_a.photo_path.as_deref(),
    )?;
    validate_optional_project_path(
        index,
        "host_b.photo_path",
        config.host_b.photo_path.as_deref(),
    )?;
    validate_timed_entries(index, "topics", &config.topics)?;
    validate_timed_entries(index, "chapters", &config.chapters)?;
    validate_overlay_style(index, &config.style)?;
    Ok(())
}

fn validate_optional_project_path(
    index: usize,
    field: &str,
    value: Option<&str>,
) -> Result<(), ApplyError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.trim().is_empty() {
        return Err(ApplyError::Invalid {
            index,
            message: format!("set_broadcast_overlay: {field} cannot be empty"),
        });
    }
    let path = std::path::Path::new(value);
    if path.is_absolute() || value.split('/').any(|part| part == "..") {
        return Err(ApplyError::Invalid {
            index,
            message: format!(
                "set_broadcast_overlay: {field} must be project-relative and cannot contain '..'"
            ),
        });
    }
    Ok(())
}

fn validate_timed_entries(
    index: usize,
    field: &str,
    entries: &[BroadcastTimedEntry],
) -> Result<(), ApplyError> {
    for entry in entries {
        if !entry.time_seconds.is_finite() || entry.time_seconds < 0.0 {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "set_broadcast_overlay: {field} time_seconds must be finite and >= 0"
                ),
            });
        }
        if entry.text.trim().is_empty() {
            return Err(ApplyError::Invalid {
                index,
                message: format!("set_broadcast_overlay: {field} text cannot be empty"),
            });
        }
    }
    Ok(())
}

fn validate_overlay_style(index: usize, style: &BroadcastOverlayStyle) -> Result<(), ApplyError> {
    let finite_positive = [
        ("title_fade_in_end", style.title_fade_in_end),
        ("title_visible_end", style.title_visible_end),
        ("host_intro_end", style.host_intro_end),
        ("ticker_sponsor_duration", style.ticker_sponsor_duration),
        ("ticker_fade_duration", style.ticker_fade_duration),
        ("ticker_topic_duration", style.ticker_topic_duration),
        ("chapter_display_duration", style.chapter_display_duration),
        ("name_bar_height", style.name_bar_height),
        ("ticker_height", style.ticker_height),
        ("host_strip_height", style.host_strip_height),
    ];
    for (field, value) in finite_positive {
        if !value.is_finite() || value <= 0.0 {
            return Err(ApplyError::Invalid {
                index,
                message: format!("set_broadcast_overlay: style.{field} must be finite and > 0"),
            });
        }
    }
    if !style.title_fade_out_start.is_finite() || style.title_fade_out_start < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: "set_broadcast_overlay: style.title_fade_out_start must be finite and >= 0"
                .into(),
        });
    }
    if !style.host_intro_start.is_finite() || style.host_intro_start < 0.0 {
        return Err(ApplyError::Invalid {
            index,
            message: "set_broadcast_overlay: style.host_intro_start must be finite and >= 0".into(),
        });
    }
    if style.title_visible_end <= style.title_fade_in_end
        || style.title_fade_out_start > style.title_visible_end
        || style.host_intro_end <= style.host_intro_start
    {
        return Err(ApplyError::Invalid {
            index,
            message: "set_broadcast_overlay: style timing windows are inconsistent".into(),
        });
    }
    Ok(())
}

/// Update the montage.title effect on an anchored title clip.
/// All styling fields are optional — None leaves the existing value
/// alone. start_s / end_s adjust both the effect metadata AND the
/// underlying clip's source_range so the timeline window stays in
/// sync with the drawtext enable window.
#[allow(clippy::too_many_arguments)]
fn apply_set_title(
    working: &mut Timeline,
    index: usize,
    anchor: &Anchor,
    start_s: Option<f64>,
    end_s: Option<f64>,
    text: Option<&str>,
    position: Option<super::op::TitlePosition>,
    font_size: Option<u32>,
    color: Option<&str>,
    font_weight: Option<super::op::TitleWeight>,
    animation: Option<super::op::TitleAnimation>,
    phases: Option<super::op::TitlePhases>,
    font_family: Option<&str>,
    font_path: Option<&str>,
    ctx: &AnchorContext,
    locator: Option<ClipLocator>,
) -> Result<String, ApplyError> {
    let _ = (anchor, ctx);
    let locator = required_locator(index, locator)?;
    let StackChild::Track(track) = &mut working.tracks.children[locator.track_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_title: anchor resolved to a non-track stack child".into(),
        });
    };
    let TrackChild::Clip(clip) = &mut track.children[locator.child_index] else {
        return Err(ApplyError::Invalid {
            index,
            message: "set_title: anchor resolved to a non-clip child".into(),
        });
    };
    let effect = clip
        .effects
        .iter_mut()
        .find(|e| e.effect_name == TITLE_EFFECT_NAME)
        .ok_or_else(|| ApplyError::Invalid {
            index,
            message: format!(
                "set_title: anchored clip {:?} carries no montage.title effect — \
                 SetTitle only updates existing titles, use InsertTitle to create one",
                clip.name,
            ),
        })?;

    if let Some(t) = text {
        if t.is_empty() {
            return Err(ApplyError::Invalid {
                index,
                message: "set_title: text must be non-empty".into(),
            });
        }
        effect
            .metadata
            .insert("text".to_string(), serde_json::json!(t));
    }
    if let Some(p) = position {
        effect.metadata.insert(
            "position".to_string(),
            serde_json::json!(title_position_str(p)),
        );
    }
    if let Some(f) = font_size {
        effect
            .metadata
            .insert("font_size".to_string(), serde_json::json!(f));
    }
    if let Some(c) = color {
        effect
            .metadata
            .insert("color".to_string(), serde_json::json!(c));
    }
    if let Some(w) = font_weight {
        effect.metadata.insert(
            "font_weight".to_string(),
            serde_json::json!(title_weight_str(w)),
        );
    }
    if let Some(a) = animation {
        effect.metadata.insert(
            "animation".to_string(),
            serde_json::json!(title_animation_str(a)),
        );
    }
    if let Some(p) = phases {
        let serialized = serde_json::to_value(p).map_err(|error| ApplyError::Invalid {
            index,
            message: format!("set_title: phases could not serialize: {error}"),
        })?;
        effect.metadata.insert("phases".to_string(), serialized);
    }
    if let Some(family) = font_family.filter(|s| !s.trim().is_empty()) {
        effect
            .metadata
            .insert("font_family".to_string(), serde_json::json!(family));
        // Render resolves `font_path` before `font_family`, so a stale explicit
        // font file would shadow the requested family switch. Drop it here —
        // unless this same op also supplies a new path, which is re-inserted
        // immediately below.
        if font_path.filter(|s| !s.trim().is_empty()).is_none() {
            effect.metadata.remove("font_path");
        }
    }
    if let Some(path) = font_path.filter(|s| !s.trim().is_empty()) {
        effect
            .metadata
            .insert("font_path".to_string(), serde_json::json!(path));
    }

    // start_s / end_s require both the effect metadata AND the
    // clip's source_range to update so the timeline view + the
    // render's enable window agree.
    if start_s.is_some() || end_s.is_some() {
        let prior = clip
            .source_range
            .as_ref()
            .ok_or_else(|| ApplyError::Invalid {
                index,
                message: "set_title: title clip has no source_range to mutate".into(),
            })?;
        let rate = prior.start_time.rate;
        let prior_start = prior.start_time.to_seconds();
        let prior_end = prior_start + prior.duration.to_seconds();
        let new_start = start_s.unwrap_or(prior_start);
        let new_end = end_s.unwrap_or(prior_end);
        if !new_start.is_finite() || !new_end.is_finite() || new_end <= new_start {
            return Err(ApplyError::Invalid {
                index,
                message: format!(
                    "set_title: invalid window [{new_start}..{new_end}]; end_s must be > start_s"
                ),
            });
        }
        clip.source_range = Some(montage_proto::otio::TimeRange::new(
            montage_proto::otio::RationalTime::new(new_start * rate, rate),
            montage_proto::otio::RationalTime::new((new_end - new_start) * rate, rate),
        ));
        effect
            .metadata
            .insert("start_s".to_string(), serde_json::json!(new_start));
        effect
            .metadata
            .insert("end_s".to_string(), serde_json::json!(new_end));
    }

    Ok(format!("updated title clip {:?}", clip.name))
}

/// Find the existing Titles track or push a new one onto the
/// timeline. Returns the track's index in `tracks.children`. The
/// new track is flagged via `metadata["montage_track_role"] = "titles"`
/// so the render pipeline can route its clips into drawtext filters.
fn find_or_create_titles_track(working: &mut Timeline) -> usize {
    if let Some(idx) = working
        .tracks
        .children
        .iter()
        .position(|sc| matches!(sc, StackChild::Track(t) if is_titles_track(t)))
    {
        return idx;
    }
    let mut track = montage_proto::otio::Track::empty(
        TITLES_TRACK_NAME.to_string(),
        montage_proto::otio::TrackKind::Video,
    );
    track.metadata.insert(
        TITLES_TRACK_ROLE_KEY.to_string(),
        serde_json::json!(TITLES_TRACK_ROLE_VALUE),
    );
    working.tracks.children.push(StackChild::Track(track));
    working.tracks.children.len() - 1
}

fn find_or_create_annotations_track(working: &mut Timeline) -> usize {
    if let Some(idx) = working
        .tracks
        .children
        .iter()
        .position(|sc| matches!(sc, StackChild::Track(t) if is_annotations_track(t)))
    {
        return idx;
    }
    let mut track = montage_proto::otio::Track::empty(
        ANNOTATIONS_TRACK_NAME.to_string(),
        montage_proto::otio::TrackKind::Video,
    );
    track.metadata.insert(
        TITLES_TRACK_ROLE_KEY.to_string(),
        serde_json::json!(ANNOTATIONS_TRACK_ROLE_VALUE),
    );
    working.tracks.children.push(StackChild::Track(track));
    working.tracks.children.len() - 1
}

/// True iff `track` is the project's Titles track. Matches the
/// metadata flag first; falls back to the canonical name so a
/// hand-edited OTIO from before the metadata flag landed is still
/// recognized.
fn is_titles_track(track: &montage_proto::otio::Track) -> bool {
    if track
        .metadata
        .get(TITLES_TRACK_ROLE_KEY)
        .and_then(|v| v.as_str())
        == Some(TITLES_TRACK_ROLE_VALUE)
    {
        return true;
    }
    track.name == TITLES_TRACK_NAME
}

fn is_annotations_track(track: &montage_proto::otio::Track) -> bool {
    if track
        .metadata
        .get(TITLES_TRACK_ROLE_KEY)
        .and_then(|v| v.as_str())
        == Some(ANNOTATIONS_TRACK_ROLE_VALUE)
    {
        return true;
    }
    track.name == ANNOTATIONS_TRACK_NAME
}

/// Find the insertion index for a new title at `start_s` on the
/// Titles track. Walks children left-to-right and returns the first
/// position whose existing source_range start is >= start_s. Keeps
/// the track sorted by start_s for readable render output.
fn title_insertion_index(track: &montage_proto::otio::Track, start_s: f64) -> usize {
    for (i, child) in track.children.iter().enumerate() {
        let TrackChild::Clip(c) = child else { continue };
        let existing_start = c
            .source_range
            .as_ref()
            .map(|r| r.start_time.to_seconds())
            .unwrap_or(0.0);
        if existing_start > start_s {
            return i;
        }
    }
    track.children.len()
}

fn title_position_str(p: super::op::TitlePosition) -> &'static str {
    match p {
        super::op::TitlePosition::Top => "top",
        super::op::TitlePosition::Center => "center",
        super::op::TitlePosition::Bottom => "bottom",
    }
}

fn title_weight_str(w: super::op::TitleWeight) -> &'static str {
    match w {
        super::op::TitleWeight::Normal => "normal",
        super::op::TitleWeight::Bold => "bold",
    }
}

fn title_animation_str(a: super::op::TitleAnimation) -> &'static str {
    match a {
        super::op::TitleAnimation::None => "none",
        super::op::TitleAnimation::FadeIn => "fade_in",
        super::op::TitleAnimation::FadeOut => "fade_out",
        super::op::TitleAnimation::FadeInOut => "fade_in_out",
        super::op::TitleAnimation::SlideIn => "slide_in",
        super::op::TitleAnimation::SlideOut => "slide_out",
    }
}

fn find_track_mut<'a>(
    timeline: &'a mut Timeline,
    track_name: &str,
) -> Option<&'a mut montage_proto::otio::Track> {
    timeline.tracks.children.iter_mut().find_map(|sc| match sc {
        StackChild::Track(track) if track.name == track_name => Some(track),
        _ => None,
    })
}

/// Sum the durations of children before `child_index` on the given
/// track. Used by the overlay broll path to compute the anchor's
/// track-time start so the broll on V2 lines up with the underlying
/// clip on V1.
fn track_time_at(timeline: &Timeline, track_index: usize, child_index: usize) -> f64 {
    let StackChild::Track(track) = &timeline.tracks.children[track_index] else {
        return 0.0;
    };
    let mut t = 0.0;
    for (i, child) in track.children.iter().enumerate() {
        if i >= child_index {
            break;
        }
        t += child_duration(child);
    }
    t
}

/// Total duration of a track's children. Used to know where to
/// append on the overlay track without recomputing per-element.
fn track_cursor(track: &montage_proto::otio::Track) -> f64 {
    track.children.iter().map(child_duration).sum()
}

fn insert_overlay_clip_at(
    track: &mut montage_proto::otio::Track,
    clip: TrackChild,
    start_s: f64,
    duration_s: f64,
    rate: f64,
) -> Result<(), Box<TrackChild>> {
    let cursor = track_cursor(track);
    if cursor <= start_s + 1e-6 {
        if start_s > cursor + 1e-6 {
            track
                .children
                .push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                    start_s - cursor,
                    rate,
                )));
        }
        track.children.push(clip);
        return Ok(());
    }

    let mut t = 0.0;
    for idx in 0..track.children.len() {
        let child_dur = child_duration(&track.children[idx]);
        let child_start = t;
        let child_end = t + child_dur;
        if start_s >= child_start - 1e-6 && start_s + duration_s <= child_end + 1e-6 {
            if !matches!(track.children[idx], TrackChild::Gap(_)) {
                return Err(Box::new(clip));
            }
            let before = (start_s - child_start).max(0.0);
            let after = (child_end - start_s - duration_s).max(0.0);
            let mut replacement = Vec::new();
            if before > 1e-6 {
                replacement.push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                    before, rate,
                )));
            }
            replacement.push(clip);
            if after > 1e-6 {
                replacement.push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                    after, rate,
                )));
            }
            track.children.splice(idx..=idx, replacement);
            return Ok(());
        }
        if start_s < child_end - 1e-6 {
            return Err(Box::new(clip));
        }
        t = child_end;
    }

    Err(Box::new(clip))
}

fn child_duration(child: &TrackChild) -> f64 {
    match child {
        TrackChild::Clip(c) => c
            .source_range
            .as_ref()
            .map(|r| r.duration.to_seconds())
            .unwrap_or(0.0),
        TrackChild::Gap(g) => g.source_range.duration.to_seconds(),
        // Transitions overlap their neighbors in source-time terms
        // (their `in_offset + out_offset` is the *visual* duration,
        // not extra timeline length). For the cursor math here we
        // treat them as zero — they don't push later clips later.
        TrackChild::Transition(_) => 0.0,
        // Nested stacks aren't produced anywhere in the montage
        // pipeline today; treat as zero rather than panicking.
        TrackChild::Stack(_) => 0.0,
    }
}

/// Pick the next free `V<N>` name for a brand-new video track.
/// Walks existing track names that match `V<digits>` and returns the
/// max+1; falls back to `V2` if the parse misses (no `V1` in scope,
/// or all names are non-numeric).
fn next_video_track_name(timeline: &Timeline) -> String {
    let mut max_n: u32 = 1;
    for sc in &timeline.tracks.children {
        let StackChild::Track(t) = sc else { continue };
        if let Some(rest) = t.name.strip_prefix('V')
            && let Ok(n) = rest.parse::<u32>()
        {
            if n > max_n {
                max_n = n;
            }
        }
    }
    format!("V{}", max_n + 1)
}

/// Lightweight validate hook on Timeline. We reach into the proto crate's
/// validate machinery via [`montage_proto::project::Project`] indirectly —
/// but here we just round-trip through serde to catch shape-level errors.
trait ValidateForApply {
    fn validate_for_test(&self) -> Result<(), String>;
}

impl ValidateForApply for Timeline {
    fn validate_for_test(&self) -> Result<(), String> {
        // The OTIO crate's typed validators ride on `Timeline::validate`;
        // it's pub(crate). The cheapest portable check: serialize +
        // deserialize, which exercises the shape-level invariants of
        // the entire tree.
        let s = serde_json::to_string(self).map_err(|e| e.to_string())?;
        let _: Timeline = serde_json::from_str(&s).map_err(|e| e.to_string())?;
        // Then walk: every clip must have a positive duration, every
        // track child non-negative duration. We do this manually since
        // Timeline::validate is pub(crate).
        let mut warnings = Vec::new();
        for sc in &self.tracks.children {
            let StackChild::Track(track) = sc else {
                continue;
            };
            for tc in &track.children {
                if let TrackChild::Clip(c) = tc
                    && let Some(r) = &c.source_range
                {
                    if r.duration.value < 0.0 {
                        warnings.push(format!("clip {:?} has negative duration", c.name));
                    }
                    if r.duration.rate <= 0.0 || r.start_time.rate <= 0.0 {
                        warnings.push(format!("clip {:?} has non-positive time rate", c.name));
                    }
                }
            }
        }
        let _ = HashSet::<()>::new(); // reserved for future dup-uuid detection
        if warnings.is_empty() {
            Ok(())
        } else {
            Err(warnings.join("; "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edl::op::{MarkerSelector, ProfessionalTimelineEdit};
    use montage_proto::montage_meta::{
        Anchor as AwAnchor, AudioRelation, CutType, MontageClipMetadata, cut_boundary_key,
    };
    use montage_proto::otio::{
        Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
        Track, TrackChild, TrackKind,
    };
    use montage_proto::professional::{
        CoordinateSpace, ReframeSmoothing, SourceRange, TrackKind as ProfessionalTrackKind,
        TrackSample, TrackSidecar, TrackingPackage,
    };

    fn timeline_with_three_clips() -> Timeline {
        let mut tl = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        for (i, snip) in ["alpha snippet", "bravo snippet", "charlie snippet"]
            .iter()
            .enumerate()
        {
            let mut c = Clip::empty(format!("clip-{i}"));
            c.media_reference =
                MediaReference::External(ExternalReference::new(format!("raw/{i}.mp4")));
            c.source_range = Some(TimeRange::new(
                RationalTime::new(0.0, 24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
            c.metadata = ClipMetadata {
                montage: Some(MontageClipMetadata {
                    anchor: Some(AwAnchor {
                        transcript_snippet: Some((*snip).to_string()),
                        ..AwAnchor::default()
                    }),
                    ..MontageClipMetadata::default()
                }),
                ..ClipMetadata::default()
            };
            track.children.push(TrackChild::Clip(c));
        }
        tl.tracks.children.push(StackChild::Track(track));
        tl
    }

    fn linked_av_timeline() -> Timeline {
        let mut tl = Timeline::empty("linked");
        let mut video = Track::empty("Video 1", TrackKind::Video);
        let mut audio = Track::empty("A1", TrackKind::Audio);
        for (track, name, uuid) in [
            (&mut video, "video", "video-uuid"),
            (&mut audio, "audio", "audio-uuid"),
        ] {
            let mut extra = std::collections::HashMap::new();
            extra.insert("clip_uuid".into(), serde_json::json!(uuid));
            extra.insert("link_group_id".into(), serde_json::json!("lg-av"));
            let mut clip = Clip::empty(name);
            clip.media_reference =
                MediaReference::External(ExternalReference::new("raw/episode.mov"));
            clip.source_range = Some(TimeRange::new(
                RationalTime::new(10.0 * 24.0, 24.0),
                RationalTime::new(20.0 * 24.0, 24.0),
            ));
            clip.metadata = ClipMetadata {
                montage: Some(MontageClipMetadata {
                    extra,
                    ..MontageClipMetadata::default()
                }),
                ..ClipMetadata::default()
            };
            track.children.push(TrackChild::Clip(clip));
        }
        tl.tracks.children.push(StackChild::Track(video));
        tl.tracks.children.push(StackChild::Track(audio));
        tl
    }

    fn add_one_second_handles(tl: &mut Timeline) {
        let StackChild::Track(track) = &mut tl.tracks.children[0] else {
            panic!("expected track")
        };
        for child in &mut track.children {
            let TrackChild::Clip(clip) = child else {
                continue;
            };
            clip.media_reference =
                MediaReference::External(external_ref_with_available("raw/handled.mp4", 7.0));
            clip.source_range = Some(TimeRange::new(
                RationalTime::new(1.0 * 24.0, 24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
        }
    }

    fn external_ref_with_available(asset: &str, duration_s: f64) -> ExternalReference {
        let mut ext = ExternalReference::new(asset.to_string());
        ext.available_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(duration_s * 24.0, 24.0),
        ));
        ext
    }

    #[test]
    fn apply_trim_shortens_clip() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                start: None,
                end: Some(3.0),
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 1);
        assert_eq!(
            outcome.applied[0].locator,
            Some(ClipLocator {
                track_index: 0,
                child_index: 1,
            }),
            "trim proposals need the resolved locator for diff hints and drag handles",
        );
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &t.children[1] else {
            panic!()
        };
        let r = c.source_range.as_ref().unwrap();
        assert!((r.duration.to_seconds() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn apply_trim_start_only_shortens_from_current_end() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-1".into(),
                },
                start: Some(1.0),
                end: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &t.children[1] else {
            panic!()
        };
        let r = c.source_range.as_ref().unwrap();
        assert!((r.start_time.to_seconds() - 1.0).abs() < 1e-9);
        assert!((r.duration.to_seconds() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn professional_ripple_trim_lowers_to_real_timeline_trim() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::RippleTrim {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    new_end_s: 3.0,
                    ripple_tracks: vec!["V1".into()],
                },
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Clip(clip) = &track.children[1] else {
            panic!("expected clip")
        };
        let range = clip.source_range.as_ref().unwrap();

        assert!((range.duration.to_seconds() - 3.0).abs() < 1e-9);
        assert!(outcome.applied[0].description.contains("ripple trimmed"));
        assert_eq!(
            track.children.len(),
            3,
            "ripple trim should not insert a gap"
        );
    }

    #[test]
    fn professional_slip_clip_lowers_to_source_range_shift_without_timeline_move() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let env = EdlEnvelope {
            ops: vec![EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::SlipClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    delta_s: 1.0,
                },
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Clip(clip) = &track.children[1] else {
            panic!("expected clip")
        };
        let range = clip.source_range.as_ref().unwrap();

        assert_eq!(
            outcome.applied[0].locator,
            Some(ClipLocator {
                track_index: 0,
                child_index: 1,
            })
        );
        assert!((range.start_time.to_seconds() - 2.0).abs() < 1e-9);
        assert!((range.duration.to_seconds() - 5.0).abs() < 1e-9);
        assert_eq!(
            track.children.len(),
            3,
            "slip edit should preserve timeline structure"
        );
    }

    #[test]
    fn professional_add_marker_lowers_to_real_clip_marker() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::AddMarker {
                    at_s: 6.0,
                    label: "Review beat".into(),
                    category: Some("note".into()),
                },
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Clip(clip) = &track.children[1] else {
            panic!("expected clip")
        };
        let marker = clip.markers.first().expect("marker should be attached");

        assert!(outcome.applied[0].description.contains("added marker"));
        assert_eq!(marker.name, "Review beat");
        assert!((marker.marked_range.start_time.to_seconds() - 1.0).abs() < 1e-9);
        assert_eq!(
            marker
                .metadata
                .montage
                .as_ref()
                .unwrap()
                .category
                .as_deref(),
            Some("note")
        );
        assert_eq!(
            marker
                .metadata
                .montage
                .as_ref()
                .unwrap()
                .extra
                .get("id")
                .and_then(serde_json::Value::as_str),
            Some("marker-0-1-0")
        );
    }

    #[test]
    fn professional_update_marker_edits_unique_marker_by_id() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::AddMarker {
                        at_s: 6.0,
                        label: "Review beat".into(),
                        category: Some("note".into()),
                    },
                },
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::UpdateMarker {
                        selector: MarkerSelector {
                            marker_id: Some("marker-0-1-0".into()),
                            ..MarkerSelector::default()
                        },
                        label: Some("Use this quote".into()),
                        category: Some("select".into()),
                        note: Some("Strong pull quote".into()),
                        at_s: Some(6.5),
                        duration_s: Some(1.25),
                    },
                },
            ],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Clip(clip) = &track.children[1] else {
            panic!("expected clip")
        };
        let marker = clip.markers.first().expect("marker should remain attached");

        assert!(outcome.applied[1].description.contains("updated marker"));
        assert_eq!(marker.name, "Use this quote");
        assert_eq!(marker.comment.as_deref(), Some("Strong pull quote"));
        assert!((marker.marked_range.start_time.to_seconds() - 1.5).abs() < 1e-9);
        assert!((marker.marked_range.duration.to_seconds() - 1.25).abs() < 1e-9);
        assert_eq!(
            marker
                .metadata
                .montage
                .as_ref()
                .unwrap()
                .category
                .as_deref(),
            Some("select")
        );
    }

    #[test]
    fn professional_delete_marker_removes_unique_marker_by_label_and_anchor() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::AddMarker {
                        at_s: 1.0,
                        label: "Review beat".into(),
                        category: Some("note".into()),
                    },
                },
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::AddMarker {
                        at_s: 6.0,
                        label: "Review beat".into(),
                        category: Some("note".into()),
                    },
                },
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::DeleteMarker {
                        selector: MarkerSelector {
                            anchor: Some(Anchor::ClipUuid {
                                uuid: "clip-1".into(),
                            }),
                            label: Some("Review beat".into()),
                            ..MarkerSelector::default()
                        },
                    },
                },
            ],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Clip(first_clip) = &track.children[0] else {
            panic!("expected clip")
        };
        let TrackChild::Clip(second_clip) = &track.children[1] else {
            panic!("expected clip")
        };

        assert!(outcome.applied[2].description.contains("deleted marker"));
        assert_eq!(first_clip.markers.len(), 1);
        assert!(second_clip.markers.is_empty());
    }

    #[test]
    fn professional_update_marker_rejects_ambiguous_label_selector() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::AddMarker {
                        at_s: 1.0,
                        label: "Review beat".into(),
                        category: Some("note".into()),
                    },
                },
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::AddMarker {
                        at_s: 6.0,
                        label: "Review beat".into(),
                        category: Some("note".into()),
                    },
                },
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::UpdateMarker {
                        selector: MarkerSelector {
                            label: Some("Review beat".into()),
                            ..MarkerSelector::default()
                        },
                        label: Some("Ambiguous".into()),
                        category: None,
                        note: None,
                        at_s: None,
                        duration_s: None,
                    },
                },
            ],
        };

        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(format!("{err}").contains("matches 2 markers"));
    }

    #[test]
    fn professional_roll_edit_lowers_to_adjacent_source_range_shift() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let env = EdlEnvelope {
            ops: vec![EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::RollEdit {
                    between: super::super::op::TransitionBetween {
                        from: Anchor::ClipUuid {
                            uuid: "clip-0".into(),
                        },
                        to: Anchor::ClipUuid {
                            uuid: "clip-1".into(),
                        },
                    },
                    delta_s: 1.0,
                },
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Clip(from_clip) = &track.children[0] else {
            panic!("expected from clip")
        };
        let TrackChild::Clip(to_clip) = &track.children[1] else {
            panic!("expected to clip")
        };
        let from_range = from_clip.source_range.as_ref().unwrap();
        let to_range = to_clip.source_range.as_ref().unwrap();

        assert!(outcome.applied[0].description.contains("rolled edit"));
        assert!((from_range.start_time.to_seconds() - 1.0).abs() < 1e-9);
        assert!((from_range.duration.to_seconds() - 6.0).abs() < 1e-9);
        assert!((to_range.start_time.to_seconds() - 2.0).abs() < 1e-9);
        assert!((to_range.duration.to_seconds() - 4.0).abs() < 1e-9);
        assert_eq!(
            from_range.duration.to_seconds() + to_range.duration.to_seconds(),
            10.0,
            "roll edit should preserve combined timeline duration"
        );
    }

    #[test]
    fn professional_slide_clip_lowers_to_neighbor_source_range_shift() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let env = EdlEnvelope {
            ops: vec![EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::SlideClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    delta_s: 1.0,
                },
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Clip(previous_clip) = &track.children[0] else {
            panic!("expected previous clip")
        };
        let TrackChild::Clip(slid_clip) = &track.children[1] else {
            panic!("expected slid clip")
        };
        let TrackChild::Clip(next_clip) = &track.children[2] else {
            panic!("expected next clip")
        };
        let previous_range = previous_clip.source_range.as_ref().unwrap();
        let slid_range = slid_clip.source_range.as_ref().unwrap();
        let next_range = next_clip.source_range.as_ref().unwrap();

        assert!(outcome.applied[0].description.contains("slid clip"));
        assert!((previous_range.start_time.to_seconds() - 1.0).abs() < 1e-9);
        assert!((previous_range.duration.to_seconds() - 6.0).abs() < 1e-9);
        assert!((slid_range.start_time.to_seconds() - 1.0).abs() < 1e-9);
        assert!((slid_range.duration.to_seconds() - 5.0).abs() < 1e-9);
        assert!((next_range.start_time.to_seconds() - 2.0).abs() < 1e-9);
        assert!((next_range.duration.to_seconds() - 4.0).abs() < 1e-9);
        assert_eq!(
            previous_range.duration.to_seconds()
                + slid_range.duration.to_seconds()
                + next_range.duration.to_seconds(),
            15.0,
            "slide edit should preserve combined timeline duration"
        );
    }

    #[test]
    fn professional_lift_range_lowers_to_duration_preserving_gap() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::LiftRange {
                    range: SourceRange {
                        start_s: 3.0,
                        end_s: 8.0,
                    },
                    tracks: vec!["V1".into()],
                },
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };

        assert!(outcome.applied[0].description.contains("lifted range"));
        assert_eq!(track.children.len(), 4);
        let TrackChild::Clip(left) = &track.children[0] else {
            panic!("expected leading clip")
        };
        let TrackChild::Gap(gap) = &track.children[1] else {
            panic!("expected lifted gap")
        };
        let TrackChild::Clip(tail) = &track.children[2] else {
            panic!("expected trailing split clip")
        };
        let TrackChild::Clip(after) = &track.children[3] else {
            panic!("expected unaffected clip")
        };
        let left_range = left.source_range.as_ref().unwrap();
        let tail_range = tail.source_range.as_ref().unwrap();
        let after_range = after.source_range.as_ref().unwrap();

        assert_eq!(left.name, "clip-0");
        assert!((left_range.start_time.to_seconds() - 0.0).abs() < 1e-9);
        assert!((left_range.duration.to_seconds() - 3.0).abs() < 1e-9);
        assert!((gap.source_range.duration.to_seconds() - 5.0).abs() < 1e-9);
        assert_eq!(tail.name, "clip-1");
        assert!((tail_range.start_time.to_seconds() - 3.0).abs() < 1e-9);
        assert!((tail_range.duration.to_seconds() - 2.0).abs() < 1e-9);
        assert_eq!(after.name, "clip-2");
        assert!((after_range.duration.to_seconds() - 5.0).abs() < 1e-9);
        assert!((track_cursor(track) - 15.0).abs() < 1e-9);
    }

    #[test]
    fn professional_extract_range_lowers_to_close_gap_removal() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::ExtractRange {
                    range: SourceRange {
                        start_s: 3.0,
                        end_s: 8.0,
                    },
                    tracks: vec!["V1".into()],
                },
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };

        assert!(outcome.applied[0].description.contains("extracted range"));
        assert_eq!(track.children.len(), 3);
        let TrackChild::Clip(left) = &track.children[0] else {
            panic!("expected leading clip")
        };
        let TrackChild::Clip(tail) = &track.children[1] else {
            panic!("expected trailing split clip")
        };
        let TrackChild::Clip(after) = &track.children[2] else {
            panic!("expected pulled-up clip")
        };
        let left_range = left.source_range.as_ref().unwrap();
        let tail_range = tail.source_range.as_ref().unwrap();
        let after_range = after.source_range.as_ref().unwrap();

        assert_eq!(left.name, "clip-0");
        assert!((left_range.start_time.to_seconds() - 0.0).abs() < 1e-9);
        assert!((left_range.duration.to_seconds() - 3.0).abs() < 1e-9);
        assert_eq!(tail.name, "clip-1");
        assert!((tail_range.start_time.to_seconds() - 3.0).abs() < 1e-9);
        assert!((tail_range.duration.to_seconds() - 2.0).abs() < 1e-9);
        assert_eq!(after.name, "clip-2");
        assert!((after_range.duration.to_seconds() - 5.0).abs() < 1e-9);
        assert!((track_cursor(track) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn professional_replace_clip_lowers_to_asset_swap_preserving_slot() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::ReplaceClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    asset: "raw/replacement.mov".into(),
                    range: SourceRange {
                        start_s: 10.0,
                        end_s: 15.0,
                    },
                },
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Clip(replacement) = &track.children[1] else {
            panic!("expected replacement clip")
        };
        let range = replacement.source_range.as_ref().unwrap();

        assert!(outcome.applied[0].description.contains("replaced clip"));
        assert_eq!(track.children.len(), 3);
        assert_eq!(replacement.name, "clip-1");
        assert!(matches!(
            &replacement.media_reference,
            MediaReference::External(reference) if reference.target_url == "raw/replacement.mov"
        ));
        assert!((range.start_time.to_seconds() - 10.0).abs() < 1e-9);
        assert!((range.duration.to_seconds() - 5.0).abs() < 1e-9);
        assert!((track_cursor(track) - 15.0).abs() < 1e-9);
    }

    #[test]
    fn professional_append_lowers_to_track_tail_insert() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::Append {
                    track: "V1".into(),
                    asset: "raw/tail.mov".into(),
                    range: SourceRange {
                        start_s: 2.0,
                        end_s: 4.0,
                    },
                },
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Clip(appended) = &track.children[3] else {
            panic!("expected appended clip")
        };
        let range = appended.source_range.as_ref().unwrap();

        assert!(outcome.applied[0].description.contains("appended clip"));
        assert_eq!(track.children.len(), 4);
        // Default name now derives from the asset file stem.
        assert_eq!(appended.name, "tail");
        assert!(matches!(
            &appended.media_reference,
            MediaReference::External(reference) if reference.target_url == "raw/tail.mov"
        ));
        assert!((range.start_time.to_seconds() - 2.0).abs() < 1e-9);
        assert!((range.duration.to_seconds() - 2.0).abs() < 1e-9);
        assert!((track_cursor(track) - 17.0).abs() < 1e-9);
    }

    #[test]
    fn professional_overwrite_lowers_to_timeline_range_replacement() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::Overwrite {
                    track: "V1".into(),
                    asset: "raw/overwrite.mov".into(),
                    range: SourceRange {
                        start_s: 6.0,
                        end_s: 12.0,
                    },
                },
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };

        assert!(outcome.applied[0].description.contains("overwrote range"));
        assert_eq!(track.children.len(), 4);
        let TrackChild::Clip(first) = &track.children[0] else {
            panic!("expected first clip")
        };
        let TrackChild::Clip(lead) = &track.children[1] else {
            panic!("expected retained lead")
        };
        let TrackChild::Clip(overwrite) = &track.children[2] else {
            panic!("expected overwrite clip")
        };
        let TrackChild::Clip(tail) = &track.children[3] else {
            panic!("expected retained tail")
        };
        let first_range = first.source_range.as_ref().unwrap();
        let lead_range = lead.source_range.as_ref().unwrap();
        let overwrite_range = overwrite.source_range.as_ref().unwrap();
        let tail_range = tail.source_range.as_ref().unwrap();

        assert_eq!(first.name, "clip-0");
        assert!((first_range.duration.to_seconds() - 5.0).abs() < 1e-9);
        assert_eq!(lead.name, "clip-1");
        assert!((lead_range.start_time.to_seconds() - 0.0).abs() < 1e-9);
        assert!((lead_range.duration.to_seconds() - 1.0).abs() < 1e-9);
        assert!(matches!(
            &overwrite.media_reference,
            MediaReference::External(reference) if reference.target_url == "raw/overwrite.mov"
        ));
        assert!((overwrite_range.start_time.to_seconds() - 0.0).abs() < 1e-9);
        assert!((overwrite_range.duration.to_seconds() - 6.0).abs() < 1e-9);
        assert_eq!(tail.name, "clip-2");
        assert!((tail_range.start_time.to_seconds() - 2.0).abs() < 1e-9);
        assert!((tail_range.duration.to_seconds() - 3.0).abs() < 1e-9);
        assert!((track_cursor(track) - 15.0).abs() < 1e-9);
    }

    #[test]
    fn apply_trim_rejects_no_op_range() {
        let mut tl = timeline_with_three_clips();
        let StackChild::Track(t) = &mut tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &mut t.children[1] else {
            panic!()
        };
        c.source_range = Some(TimeRange::new(
            RationalTime::new(5.0 * 24.0, 24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));

        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-1".into(),
                },
                start: Some(5.0),
                end: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("no-op")),
            "got: {err:?}"
        );
    }

    #[test]
    fn apply_delete_removes_clip() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo".into(),
                },
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 1);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 3);
        // Timeline timing is preserved: alpha, lifted gap, charlie.
        assert!(
            matches!(&t.children[1], TrackChild::Gap(gap) if (gap.source_range.duration.to_seconds() - 5.0).abs() < 1e-9)
        );
        let TrackChild::Clip(c1) = &t.children[2] else {
            panic!()
        };
        assert_eq!(c1.name, "clip-2");
    }

    #[test]
    fn apply_delete_removes_cut_boundary_metadata_for_deleted_clip() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetCutIntent {
                    between: super::super::op::TransitionBetween {
                        from: Anchor::ClipUuid {
                            uuid: "clip-0".into(),
                        },
                        to: Anchor::ClipUuid {
                            uuid: "clip-1".into(),
                        },
                    },
                    spec: SemanticCutSpec {
                        cut_type: CutType::CutOnAction,
                        intent: "hide_action_continuity".into(),
                        energy: None,
                        audio_relation: AudioRelation::Sync,
                        confidence: Some(0.9),
                        reason: Some("motion carries through the boundary".into()),
                        extra: Default::default(),
                    },
                },
                EdlOp::DeleteClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                },
            ],
        };

        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();

        let meta = new_tl.metadata.montage.as_ref().expect("montage metadata");
        assert!(
            meta.cut_boundaries.is_empty(),
            "cut metadata referring to deleted clips must not remain: {:?}",
            meta.cut_boundaries
        );
    }

    #[test]
    fn apply_delete_removes_audio_lead_when_source_boundary_is_removed() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetAudioLead {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-2".into(),
                    },
                    lead_s: 0.35,
                    split_edit: Some(SplitEditSpec {
                        audio_lead_s: Some(0.35),
                        audio_lead_from_clip_id: None,
                        audio_trail_s: None,
                        audio_trail_to_clip_id: None,
                        reason: Some("let clip two lead under clip one".into()),
                        confidence: Some(0.86),
                    }),
                },
                EdlOp::DeleteClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                },
            ],
        };

        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected video track");
        };
        let TrackChild::Clip(clip_two) = &track.children[2] else {
            panic!("expected clip two after delete");
        };

        assert!(
            clip_two
                .metadata
                .montage
                .as_ref()
                .and_then(|metadata| metadata.split_edit.as_ref())
                .and_then(|split| split.audio_lead_s)
                .is_none(),
            "audio lead should not survive after its source boundary was removed"
        );
    }

    #[test]
    fn apply_delete_removes_adjacent_transitions() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::InsertTransition {
                    between: super::super::op::TransitionBetween {
                        from: Anchor::TranscriptSnippet {
                            text: "alpha snippet".into(),
                        },
                        to: Anchor::TranscriptSnippet {
                            text: "bravo snippet".into(),
                        },
                    },
                    kind: "SMPTE_Dissolve".into(),
                    duration_s: 0.3,
                    alignment: None,
                    in_offset_s: None,
                    out_offset_s: None,
                    spec: None,
                },
                EdlOp::InsertTransition {
                    between: super::super::op::TransitionBetween {
                        from: Anchor::TranscriptSnippet {
                            text: "bravo snippet".into(),
                        },
                        to: Anchor::TranscriptSnippet {
                            text: "charlie snippet".into(),
                        },
                    },
                    kind: "SMPTE_Dissolve".into(),
                    duration_s: 0.3,
                    alignment: None,
                    in_offset_s: None,
                    out_offset_s: None,
                    spec: None,
                },
            ],
        };
        let (tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();

        let (new_tl, outcome) = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::DeleteClip {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                }],
            },
            &AnchorContext::empty(),
        )
        .unwrap();

        assert!(
            outcome.applied[0]
                .description
                .contains("removed 2 adjacent transition")
        );
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 3);
        assert!(matches!(&t.children[0], TrackChild::Clip(_)));
        assert!(matches!(&t.children[1], TrackChild::Gap(_)));
        assert!(matches!(&t.children[2], TrackChild::Clip(_)));
        assert!(
            t.children
                .iter()
                .all(|child| !matches!(child, TrackChild::Transition(_)))
        );
    }

    #[test]
    fn apply_split_partitions_clip_at_timestamp() {
        let tl = timeline_with_three_clips();
        // bravo's source_range is [0, 5s]. Split at 2.0s into two
        // pieces: [0, 2s] and [2s, 5s].
        let env = EdlEnvelope {
            ops: vec![EdlOp::SplitClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo".into(),
                },
                at_s: 2.0,
                snap: None,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 1);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // alpha, bravo (left), bravo-b (right), charlie. 4 clips.
        assert_eq!(t.children.len(), 4);
        let TrackChild::Clip(left) = &t.children[1] else {
            panic!()
        };
        let TrackChild::Clip(right) = &t.children[2] else {
            panic!()
        };
        assert_eq!(left.name, "clip-1");
        assert_eq!(right.name, "clip-1-b");
        let lr = left.source_range.as_ref().unwrap();
        let rr = right.source_range.as_ref().unwrap();
        assert!((lr.duration.to_seconds() - 2.0).abs() < 1e-9);
        assert!((rr.duration.to_seconds() - 3.0).abs() < 1e-9);
        assert!((rr.start_time.to_seconds() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn apply_split_cascades_to_linked_av_sibling() {
        let tl = linked_av_timeline();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SplitClip {
                anchor: Anchor::ClipUuid {
                    uuid: "video-uuid".into(),
                },
                at_s: 18.0,
                snap: None,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0]
                .description
                .contains("1 linked sibling also split"),
            "got {:?}",
            outcome.applied[0].description
        );
        for stack_child in &new_tl.tracks.children {
            let StackChild::Track(track) = stack_child else {
                panic!()
            };
            assert_eq!(track.children.len(), 2);
            let TrackChild::Clip(left) = &track.children[0] else {
                panic!()
            };
            let TrackChild::Clip(right) = &track.children[1] else {
                panic!()
            };
            assert_eq!(
                clip_source_range_s(left).map(|(_, end)| (end * 1000.0).round() / 1000.0),
                Some(18.0)
            );
            assert_eq!(
                clip_source_range_s(right).map(|(start, _)| (start * 1000.0).round() / 1000.0),
                Some(18.0)
            );
        }
    }

    #[test]
    fn apply_trim_cascades_to_linked_av_sibling() {
        let tl = linked_av_timeline();
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: Anchor::ClipUuid {
                    uuid: "audio-uuid".into(),
                },
                start: Some(12.0),
                end: Some(28.0),
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0]
                .description
                .contains("1 linked sibling also trimmed"),
            "got {:?}",
            outcome.applied[0].description
        );
        for stack_child in &new_tl.tracks.children {
            let StackChild::Track(track) = stack_child else {
                panic!()
            };
            let TrackChild::Clip(clip) = &track.children[0] else {
                panic!()
            };
            assert_eq!(clip_source_range_s(clip), Some((12.0, 28.0)));
        }
    }

    fn extract_clip_uuid(clip: &Clip) -> Option<String> {
        clip.metadata
            .montage
            .as_ref()
            .and_then(|m| m.extra.get("clip_uuid"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    #[test]
    fn apply_split_stamps_distinct_clip_uuids_on_each_piece() {
        // The right piece is built via clip.clone() — without
        // explicit re-stamping it would inherit the parent's
        // clip_uuid. That breaks Anchor::ClipUuid resolution because
        // two clips would share the same uuid. Verify the right
        // piece has a fresh uuid distinct from the left.
        let mut tl = timeline_with_three_clips();
        // Pre-stamp the parent so we can confirm the right piece
        // diverges from a known starting uuid.
        let StackChild::Track(t) = &mut tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &mut t.children[1] else {
            panic!()
        };
        c.metadata
            .montage
            .get_or_insert_with(MontageClipMetadata::default)
            .extra
            .insert(
                "clip_uuid".into(),
                serde_json::Value::String("parent-uuid".into()),
            );

        let env = EdlEnvelope {
            ops: vec![EdlOp::SplitClip {
                anchor: Anchor::ClipUuid {
                    uuid: "parent-uuid".into(),
                },
                at_s: 2.0,
                snap: None,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(left) = &t.children[1] else {
            panic!()
        };
        let TrackChild::Clip(right) = &t.children[2] else {
            panic!()
        };
        let left_uuid = extract_clip_uuid(left).unwrap();
        let right_uuid = extract_clip_uuid(right).unwrap();
        assert_eq!(left_uuid, "parent-uuid");
        assert_ne!(left_uuid, right_uuid);
        assert!(right_uuid.starts_with("c-"));
        assert!(outcome.applied[0].description.contains(&right_uuid));
        assert!(outcome.applied[0].description.contains("anchor=clip_uuid="));
    }

    #[test]
    fn apply_split_transfers_outgoing_cut_boundary_metadata_to_right_piece() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetCutIntent {
                    between: super::super::op::TransitionBetween {
                        from: Anchor::ClipUuid {
                            uuid: "clip-1".into(),
                        },
                        to: Anchor::ClipUuid {
                            uuid: "clip-2".into(),
                        },
                    },
                    spec: SemanticCutSpec {
                        cut_type: CutType::LCut,
                        intent: "hold_expert_thought".into(),
                        energy: None,
                        audio_relation: AudioRelation::AudioTrails,
                        confidence: Some(0.88),
                        reason: Some("hold the answer under the reaction".into()),
                        extra: Default::default(),
                    },
                },
                EdlOp::SplitClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    at_s: 2.5,
                    snap: None,
                },
            ],
        };

        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected video track");
        };
        let TrackChild::Clip(right_piece) = &track.children[2] else {
            panic!("expected split right piece");
        };
        let right_uuid = extract_clip_uuid(right_piece).expect("right piece uuid");
        let meta = new_tl.metadata.montage.as_ref().expect("montage metadata");
        let key = cut_boundary_key(&right_uuid, "clip-2");
        let spec = meta
            .cut_boundaries
            .get(&key)
            .expect("outgoing cut intent should follow the right split piece");

        assert_eq!(spec.cut_type, CutType::LCut);
        assert_eq!(spec.intent, "hold_expert_thought");
        assert_eq!(spec.audio_relation, AudioRelation::AudioTrails);
        assert!(!meta.cut_boundaries.contains_key("clip-1::clip-2"));
    }

    #[test]
    fn apply_split_keeps_split_edit_offsets_on_the_boundary_pieces() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetAudioLead {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    lead_s: 0.35,
                    split_edit: Some(SplitEditSpec {
                        audio_lead_s: Some(0.35),
                        audio_lead_from_clip_id: None,
                        audio_trail_s: None,
                        audio_trail_to_clip_id: None,
                        reason: Some("incoming audio leads the picture".into()),
                        confidence: Some(0.86),
                    }),
                },
                EdlOp::SetAudioTrail {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    trail_s: 0.55,
                    split_edit: Some(SplitEditSpec {
                        audio_lead_s: None,
                        audio_lead_from_clip_id: None,
                        audio_trail_s: Some(0.55),
                        audio_trail_to_clip_id: None,
                        reason: Some("outgoing audio trails into the reaction".into()),
                        confidence: Some(0.82),
                    }),
                },
                EdlOp::SplitClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    at_s: 2.5,
                    snap: None,
                },
            ],
        };

        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected video track");
        };
        let TrackChild::Clip(left_piece) = &track.children[1] else {
            panic!("expected split left piece");
        };
        let TrackChild::Clip(right_piece) = &track.children[2] else {
            panic!("expected split right piece");
        };
        let left_split = left_piece
            .metadata
            .montage
            .as_ref()
            .and_then(|metadata| metadata.split_edit.as_ref())
            .expect("left piece split edit");
        let right_split = right_piece
            .metadata
            .montage
            .as_ref()
            .and_then(|metadata| metadata.split_edit.as_ref())
            .expect("right piece split edit");

        assert_eq!(left_split.audio_lead_s, Some(0.35));
        assert_eq!(left_split.audio_trail_s, None);
        assert_eq!(right_split.audio_lead_s, None);
        assert_eq!(right_split.audio_trail_s, Some(0.55));
        assert_eq!(
            right_split.reason.as_deref(),
            Some("outgoing audio trails into the reaction")
        );
    }

    #[test]
    fn apply_insert_clip_stamps_fresh_clip_uuid() {
        let mut tl = Timeline::empty("test");
        let mut track =
            montage_proto::otio::Track::empty("V1", montage_proto::otio::TrackKind::Video);
        // No existing clip — the insert must generate its own anchor.
        track
            .children
            .push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                0.0, 24.0,
            )));
        tl.tracks.children.push(StackChild::Track(track));

        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/x.mp4".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: None,
                at_s: None,
                start: Some(0.0),
                end: Some(3.0),
                name: None,
                link_group_id: None,
                snap: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(inserted) = &t.children[1] else {
            panic!()
        };
        let uuid = extract_clip_uuid(inserted).expect("insert should stamp a clip_uuid");
        assert!(uuid.starts_with("c-"), "got: {uuid}");
    }

    #[test]
    fn apply_split_rejects_at_s_outside_range() {
        let tl = timeline_with_three_clips();
        // bravo's range is [0, 5s]. Splitting at 7s is outside.
        let env = EdlEnvelope {
            ops: vec![EdlOp::SplitClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo".into(),
                },
                at_s: 7.0,
                snap: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        match err {
            ApplyError::Invalid { index, message } => {
                assert_eq!(index, 0);
                assert!(
                    message.contains("must lie strictly inside"),
                    "got: {message}"
                );
            }
            other => panic!("want Invalid, got {other:?}"),
        }
    }

    #[test]
    fn split_then_delete_in_one_envelope_lifts_middle_out() {
        // The headline editorial flow: isolate a phrase in the middle
        // of a clip by splitting twice and lifting the middle piece.
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SplitClip {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo".into(),
                    },
                    at_s: 2.0,
                    snap: None,
                },
                // After the first split, bravo-b ([2s, 5s]) is the
                // right piece. Split it again at 4s to isolate the
                // middle [2s, 4s] chunk.
                EdlOp::SplitClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1-b".into(),
                    },
                    at_s: 4.0,
                    snap: None,
                },
                // Lift the now-isolated middle, leaving timeline timing intact.
                EdlOp::DeleteClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1-b".into(),
                    },
                },
            ],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 3);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // alpha, bravo[0..2], lifted gap[2..4], bravo-b-b[4..5], charlie.
        assert_eq!(t.children.len(), 5);
        assert!(
            matches!(&t.children[2], TrackChild::Gap(gap) if (gap.source_range.duration.to_seconds() - 2.0).abs() < 1e-9)
        );
    }

    #[test]
    fn apply_untrim_widens_after_a_trim() {
        // Trim bravo to [0, 2s], then Untrim back to its original [0, 5s].
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::TrimClip {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo".into(),
                    },
                    start: None,
                    end: Some(2.0),
                },
                EdlOp::UntrimClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    start: None,
                    end: Some(5.0),
                },
            ],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 2);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &t.children[1] else {
            panic!()
        };
        let r = c.source_range.as_ref().unwrap();
        assert!((r.duration.to_seconds() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn apply_untrim_refuses_to_narrow() {
        // bravo's source range starts at [0, 5s]. Try to "untrim" to
        // [0, 3s] — that's narrowing, which Untrim refuses.
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::UntrimClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo".into(),
                },
                start: None,
                end: Some(3.0),
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        match err {
            ApplyError::Invalid { message, .. } => {
                assert!(
                    message.contains("Untrim only widens"),
                    "expected 'Untrim only widens' hint; got: {message}"
                );
            }
            other => panic!("want Invalid, got {other:?}"),
        }
    }

    #[test]
    fn apply_untrim_caps_to_available_range_when_known() {
        // Build a single-clip timeline whose external reference declares
        // available_range = [0, 4s]. Trim it to [0, 2s], then Untrim
        // asking for [0, 999s]. Result should cap at 4s.
        use montage_proto::otio::{
            Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild,
            TimeRange, Timeline as Tl, Track, TrackChild, TrackKind,
        };
        let mut tl = Tl::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("clip-0".to_string());
        let mut ext = ExternalReference::new("raw/x.mp4");
        ext.available_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(4.0 * 24.0, 24.0),
        ));
        clip.media_reference = MediaReference::External(ext);
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));
        clip.metadata = ClipMetadata::default();
        track.children.push(TrackChild::Clip(clip));
        tl.tracks.children.push(StackChild::Track(track));

        let env = EdlEnvelope {
            ops: vec![EdlOp::UntrimClip {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-0".into(),
                },
                start: None,
                end: Some(999.0),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &t.children[0] else {
            panic!()
        };
        let r = c.source_range.as_ref().unwrap();
        // Capped to available_range end (4s).
        assert!(
            (r.duration.to_seconds() - 4.0).abs() < 1e-9,
            "expected duration capped to 4s; got {}",
            r.duration.to_seconds()
        );
    }

    /// Regression test for a real bug found in a live run: untrim with
    /// only `end` specified used to default `start` to 0.0, wiping
    /// the existing trimmed-in point. Now omitted fields preserve
    /// the current value (matching apply_trim's contract).
    #[test]
    fn apply_untrim_preserves_start_when_only_end_specified() {
        // Build a clip with source_range = [10s, 20s] (10s duration
        // starting at 10s into the source media). Untrim with end=30
        // and start=None should produce [10s, 30s] — NOT [0s, 30s].
        let mut tl = montage_proto::otio::Timeline::empty("preserve");
        let mut track =
            montage_proto::otio::Track::empty("V1", montage_proto::otio::TrackKind::Video);
        let mut clip = montage_proto::otio::Clip::empty("clip-0".to_string());
        clip.media_reference = montage_proto::otio::MediaReference::External(
            montage_proto::otio::ExternalReference::new("raw/x.mp4"),
        );
        clip.source_range = Some(montage_proto::otio::TimeRange::new(
            montage_proto::otio::RationalTime::new(10.0 * 24.0, 24.0),
            montage_proto::otio::RationalTime::new(10.0 * 24.0, 24.0),
        ));
        track
            .children
            .push(montage_proto::otio::TrackChild::Clip(clip));
        tl.tracks
            .children
            .push(montage_proto::otio::StackChild::Track(track));

        let env = EdlEnvelope {
            ops: vec![EdlOp::UntrimClip {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-0".into(),
                },
                start: None,
                end: Some(30.0),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(c) = &t.children[0] else {
            panic!()
        };
        let r = c.source_range.as_ref().unwrap();
        assert!(
            (r.start_time.to_seconds() - 10.0).abs() < 1e-9,
            "start should be preserved at 10.0s, got {}",
            r.start_time.to_seconds()
        );
        assert!(
            (r.duration.to_seconds() - 20.0).abs() < 1e-9,
            "duration should be 30-10=20s, got {}",
            r.duration.to_seconds()
        );
    }

    #[test]
    fn apply_insert_clip_creates_track_when_missing() {
        // Empty timeline. Insert one clip — track gets created.
        use montage_proto::otio::Timeline as Tl;
        let tl = Tl::empty("test");
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/clip-1.MOV".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: None,
                at_s: None,
                start: Some(0.0),
                end: Some(56.47),
                name: None,
                link_group_id: None,
                snap: None,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 1);
        assert_eq!(new_tl.tracks.children.len(), 1, "track created");
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.name, "V1");
        assert_eq!(t.children.len(), 1);
        let TrackChild::Clip(c) = &t.children[0] else {
            panic!()
        };
        // Default clip name derives from the asset's file stem.
        assert_eq!(c.name, "clip-1");
        let r = c.source_range.as_ref().unwrap();
        assert!((r.duration.to_seconds() - 56.47).abs() < 1e-6);
    }

    #[test]
    fn apply_insert_clip_appends_to_existing_track() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/extra.mp4".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: None,
                at_s: None,
                start: Some(0.0),
                end: Some(2.0),
                name: Some("intro".into()),
                link_group_id: None,
                snap: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // 3 existing + 1 inserted at the end.
        assert_eq!(t.children.len(), 4);
        let TrackChild::Clip(c) = &t.children[3] else {
            panic!()
        };
        assert_eq!(c.name, "intro");
    }

    #[test]
    fn apply_insert_clip_at_position_inserts_in_middle() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/middle.mp4".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: Some(1),
                at_s: None,
                start: Some(0.0),
                end: Some(1.0),
                name: Some("inserted".into()),
                link_group_id: None,
                snap: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 4);
        let TrackChild::Clip(c) = &t.children[1] else {
            panic!()
        };
        assert_eq!(c.name, "inserted", "new clip lands at index 1");
    }

    #[test]
    fn apply_insert_clip_default_name_avoids_duplicate_when_inserted_in_middle() {
        // The fixture's existing clips are named `clip-0/1/2`. Reuse asset
        // `raw/0.mp4` so its file-stem (`0`) collides — verifies that the
        // default-name picker falls back through the dedup suffix when
        // the stem itself is taken. (Here `0` and `1` are not in the
        // existing names, so the inserted clip should get `0` and the
        // ordering should be preserved.)
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/middle.mp4".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: Some(1),
                at_s: None,
                start: Some(0.0),
                end: Some(1.0),
                name: None,
                link_group_id: None,
                snap: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let names = t
            .children
            .iter()
            .filter_map(|tc| match tc {
                TrackChild::Clip(c) => Some(c.name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        // Asset stem `middle` doesn't collide with the existing names →
        // the new clip lands as `middle` between `clip-0` and `clip-1`.
        assert_eq!(names, vec!["clip-0", "middle", "clip-1", "clip-2"]);
    }

    #[test]
    fn apply_insert_clip_default_name_dedupes_when_stem_already_taken() {
        // Build a track whose existing clip is already named "foo" — then
        // insert asset `raw/foo.mp4`. The new clip should land as `foo-2`
        // to keep the track's clip names unique.
        let mut tl = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut existing = Clip::empty("foo");
        existing.media_reference = MediaReference::External(ExternalReference::new("raw/foo.mp4"));
        existing.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(2.0 * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(existing));
        tl.tracks.children.push(StackChild::Track(track));

        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/foo.mp4".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: None,
                at_s: None,
                start: Some(0.0),
                end: Some(1.0),
                name: None,
                link_group_id: None,
                snap: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let names = t
            .children
            .iter()
            .filter_map(|tc| match tc {
                TrackChild::Clip(c) => Some(c.name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["foo", "foo-2"]);
    }

    #[test]
    fn apply_linked_audio_insert_pads_to_video_start() {
        use montage_proto::otio::{
            Clip, ExternalReference, MediaReference, RationalTime, TimeRange,
        };

        let mut tl = montage_proto::otio::Timeline::empty("test");
        let mut v1 = montage_proto::otio::Track::empty("Video 1", TrackKind::Video);
        let mut first = Clip::empty("first");
        first.media_reference = MediaReference::External(ExternalReference::new("raw/first.mp4"));
        first.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(48.0, 24.0),
        ));
        v1.children.push(TrackChild::Clip(first));
        tl.tracks.children.push(StackChild::Track(v1));

        let env = EdlEnvelope {
            ops: vec![
                EdlOp::InsertClip {
                    asset: "raw/second.mp4".into(),
                    track: "Video 1".into(),
                    track_kind: Some(InsertTrackKind::Video),
                    at_position: None,
                    at_s: None,
                    start: Some(0.0),
                    end: Some(5.0),
                    name: None,
                    link_group_id: Some("lg-1".into()),
                    snap: None,
                },
                EdlOp::InsertClip {
                    asset: "raw/second.mp4".into(),
                    track: "A1".into(),
                    track_kind: Some(InsertTrackKind::Audio),
                    at_position: None,
                    at_s: None,
                    start: Some(0.0),
                    end: Some(5.0),
                    name: None,
                    link_group_id: Some("lg-1".into()),
                    snap: None,
                },
            ],
        };

        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(a1) = &new_tl.tracks.children[1] else {
            panic!()
        };
        assert!(matches!(a1.children[0], TrackChild::Gap(_)));
        assert!((child_duration(&a1.children[0]) - 2.0).abs() < 0.001);
        assert!(matches!(a1.children[1], TrackChild::Clip(_)));
    }

    #[test]
    fn apply_insert_clip_rejects_zero_duration() {
        use montage_proto::otio::Timeline as Tl;
        let tl = Tl::empty("test");
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/x.mp4".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: None,
                at_s: None,
                start: Some(5.0),
                end: Some(5.0),
                name: None,
                link_group_id: None,
                snap: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        match err {
            ApplyError::Invalid { message, .. } => {
                assert!(message.contains("must be > start"), "got: {message}");
            }
            other => panic!("want Invalid, got {other:?}"),
        }
    }

    #[test]
    fn apply_insert_clip_can_create_audio_track() {
        use montage_proto::otio::Timeline as Tl;
        let tl = Tl::empty("test");
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/dialogue.wav".into(),
                track: "A1".into(),
                track_kind: Some(InsertTrackKind::Audio),
                at_position: None,
                at_s: None,
                start: Some(0.0),
                end: Some(10.0),
                name: None,
                link_group_id: Some("lg-test".into()),
                snap: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert!(matches!(t.kind, TrackKind::Audio));
        let TrackChild::Clip(c) = &t.children[0] else {
            panic!()
        };
        let link = c
            .metadata
            .montage
            .as_ref()
            .and_then(|m| m.extra.get("link_group_id"))
            .and_then(|v| v.as_str());
        assert_eq!(link, Some("lg-test"));
    }

    #[test]
    fn apply_audio_fade_and_track_audio_replace_metadata() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetAudioFade {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-0".into(),
                    },
                    fade_in_s: Some(0.25),
                    fade_out_s: Some(0.5),
                },
                EdlOp::SetTrackAudio {
                    track: "V1".into(),
                    role: Some("dialogue".into()),
                    volume: Some(0.8),
                    muted: Some(false),
                    solo: Some(true),
                },
            ],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            err.to_string().contains("is not an audio track"),
            "expected video-track rejection, got {err}"
        );

        let mut audio_tl = montage_proto::otio::Timeline::empty("audio");
        let mut track = montage_proto::otio::Track::empty("A1", TrackKind::Audio);
        let mut clip = montage_proto::otio::Clip::empty("clip-0");
        clip.media_reference = montage_proto::otio::MediaReference::External(
            montage_proto::otio::ExternalReference::new("raw/a.wav"),
        );
        clip.source_range = Some(montage_proto::otio::TimeRange::new(
            montage_proto::otio::RationalTime::zero(24.0),
            montage_proto::otio::RationalTime::new(10.0 * 24.0, 24.0),
        ));
        stamp_fresh_clip_uuid(&mut clip);
        track.children.push(TrackChild::Clip(clip));
        audio_tl.tracks.children.push(StackChild::Track(track));
        let uuid = match &audio_tl.tracks.children[0] {
            StackChild::Track(t) => match &t.children[0] {
                TrackChild::Clip(c) => extract_clip_uuid(c).unwrap(),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetAudioFade {
                    anchor: Anchor::ClipUuid { uuid },
                    fade_in_s: Some(0.25),
                    fade_out_s: Some(0.5),
                },
                EdlOp::SetTrackAudio {
                    track: "A1".into(),
                    role: Some("dialogue".into()),
                    volume: Some(0.8),
                    muted: Some(false),
                    solo: Some(true),
                },
            ],
        };
        let (new_tl, _) = apply(&audio_tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.metadata["montage_audio"]["volume"].as_f64().unwrap(), 0.8);
        let TrackChild::Clip(c) = &t.children[0] else {
            panic!()
        };
        let fade = c
            .effects
            .iter()
            .find(|e| e.effect_name == AUDIO_FADE_EFFECT_NAME)
            .unwrap();
        assert_eq!(fade.metadata["fade_in_s"].as_f64(), Some(0.25));
        assert_eq!(fade.metadata["fade_out_s"].as_f64(), Some(0.5));
    }

    #[test]
    fn apply_anchor_miss_is_anchor_miss_error() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "no such snippet".into(),
                },
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        match err {
            ApplyError::AnchorMiss { index, .. } => assert_eq!(index, 0),
            other => panic!("want AnchorMiss, got {other:?}"),
        }
    }

    #[test]
    fn apply_does_not_mutate_input_on_error() {
        let tl = timeline_with_three_clips();
        let snapshot = serde_json::to_string(&tl).unwrap();
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "missing".into(),
                },
            }],
        };
        let _ = apply(&tl, &env, &AnchorContext::empty());
        let after = serde_json::to_string(&tl).unwrap();
        assert_eq!(snapshot, after);
    }

    #[test]
    fn apply_multi_op_envelope_in_order() {
        // Trim alpha, then delete charlie. Both anchors resolve against
        // the post-prior-op state implicitly (re-resolution per op).
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::TrimClip {
                    anchor: Anchor::TranscriptSnippet {
                        text: "alpha".into(),
                    },
                    start: None,
                    end: Some(2.0),
                },
                EdlOp::DeleteClip {
                    anchor: Anchor::TranscriptSnippet {
                        text: "charlie".into(),
                    },
                },
            ],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 2);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // Alpha trimmed, bravo unchanged, charlie lifted into a timing gap.
        assert_eq!(t.children.len(), 3);
        let TrackChild::Clip(alpha) = &t.children[0] else {
            panic!()
        };
        assert!((alpha.source_range.as_ref().unwrap().duration.to_seconds() - 2.0).abs() < 1e-9);
        assert!(
            matches!(&t.children[2], TrackChild::Gap(gap) if (gap.source_range.duration.to_seconds() - 5.0).abs() < 1e-9)
        );
    }

    #[test]
    fn trim_with_end_less_than_start_is_invalid() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "alpha".into(),
                },
                start: Some(3.0),
                end: Some(1.0),
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { message, .. } if message.contains("must be >= start"))
        );
    }

    #[test]
    fn apply_insert_broll_replace_swaps_in_place_with_tail() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertBRoll {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                asset: "raw/broll.mp4".into(),
                duration_s: 2.0,
                position: super::super::op::BRollPosition::Replace,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // 3 originals → 4 children (broll inserted in place of bravo,
        // plus the bravo tail).
        assert_eq!(t.children.len(), 4);
        let TrackChild::Clip(broll) = &t.children[1] else {
            panic!("expected broll at idx 1, got {:?}", t.children[1])
        };
        assert!(broll.name.starts_with("broll-from-clip-1"));
        assert_eq!(
            broll
                .source_range
                .as_ref()
                .map(|r| r.duration.to_seconds())
                .unwrap_or(0.0),
            2.0,
        );
        let TrackChild::Clip(tail) = &t.children[2] else {
            panic!("expected tail at idx 2, got {:?}", t.children[2])
        };
        assert!(tail.name.starts_with("clip-1-tail"));
        // tail.source_range = [2.0, 5.0] → start 2s, duration 3s.
        let r = tail.source_range.as_ref().unwrap();
        assert!((r.start_time.to_seconds() - 2.0).abs() < 1e-9);
        assert!((r.duration.to_seconds() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn apply_insert_broll_replace_consumes_anchor_when_duration_meets_or_exceeds() {
        let tl = timeline_with_three_clips();
        // Anchor source duration is 5s; broll for 5s → no tail.
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertBRoll {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                asset: "raw/broll.mp4".into(),
                duration_s: 5.0,
                position: super::super::op::BRollPosition::Replace,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 3); // 3 originals, bravo replaced by broll.
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name.starts_with("broll-from")));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-2"));
    }

    #[test]
    fn apply_insert_broll_overlay_creates_v2_track_with_padding_gap() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertBRoll {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                asset: "raw/broll.mp4".into(),
                duration_s: 2.0,
                position: super::super::op::BRollPosition::Overlay,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        // V1 is unchanged.
        let StackChild::Track(v1) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(v1.children.len(), 3);
        assert_eq!(v1.name, "V1");
        // V2 was created with a 5s Gap + the broll clip (anchor's
        // track-time start = 5s).
        assert_eq!(new_tl.tracks.children.len(), 2);
        let StackChild::Track(v2) = &new_tl.tracks.children[1] else {
            panic!()
        };
        assert_eq!(v2.name, "V2");
        assert_eq!(v2.children.len(), 2);
        let TrackChild::Gap(g) = &v2.children[0] else {
            panic!("expected gap at v2 idx 0")
        };
        assert!((g.source_range.duration.to_seconds() - 5.0).abs() < 1e-9);
        let TrackChild::Clip(broll) = &v2.children[1] else {
            panic!("expected broll at v2 idx 1")
        };
        assert!(broll.name.starts_with("broll-from-clip-1"));
    }

    #[test]
    fn apply_insert_broll_overlay_inserts_into_existing_gap_instead_of_appending() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::InsertBRoll {
                    anchor: Anchor::TranscriptSnippet {
                        text: "charlie snippet".into(),
                    },
                    asset: "raw/charlie-broll.mp4".into(),
                    duration_s: 1.0,
                    position: super::super::op::BRollPosition::Overlay,
                },
                EdlOp::InsertBRoll {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    asset: "raw/bravo-broll.mp4".into(),
                    duration_s: 1.0,
                    position: super::super::op::BRollPosition::Overlay,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(v2) = &new_tl.tracks.children[1] else {
            panic!()
        };
        assert_eq!(v2.name, "V2");
        assert_eq!(v2.children.len(), 4);
        assert!(
            matches!(&v2.children[0], TrackChild::Gap(g) if (g.source_range.duration.to_seconds() - 5.0).abs() < 1e-9)
        );
        assert!(
            matches!(&v2.children[1], TrackChild::Clip(c) if matches!(&c.media_reference, MediaReference::External(r) if r.target_url == "raw/bravo-broll.mp4"))
        );
        assert!(
            matches!(&v2.children[2], TrackChild::Gap(g) if (g.source_range.duration.to_seconds() - 4.0).abs() < 1e-9)
        );
        assert!(
            matches!(&v2.children[3], TrackChild::Clip(c) if matches!(&c.media_reference, MediaReference::External(r) if r.target_url == "raw/charlie-broll.mp4"))
        );
    }

    #[test]
    fn apply_insert_pip_creates_overlay_track_and_stamps_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertPiP {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                asset: "raw/pip.mp4".into(),
                duration_s: 2.0,
                source_start_s: 1.0,
                corner: super::super::op::PiPCorner::BottomRight,
                scale: 0.28,
                margin_pct: 0.035,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(v2) = &new_tl.tracks.children[1] else {
            panic!()
        };
        assert_eq!(v2.name, "V2");
        assert_eq!(v2.children.len(), 2);
        let TrackChild::Gap(g) = &v2.children[0] else {
            panic!("expected gap at v2 idx 0")
        };
        assert!((g.source_range.duration.to_seconds() - 5.0).abs() < 1e-9);
        let TrackChild::Clip(pip) = &v2.children[1] else {
            panic!("expected pip clip")
        };
        let range = pip.source_range.as_ref().unwrap();
        assert!((range.start_time.to_seconds() - 1.0).abs() < 1e-9);
        assert!((range.duration.to_seconds() - 2.0).abs() < 1e-9);
        let effect = pip
            .effects
            .iter()
            .find(|e| e.effect_name == VIDEO_OVERLAY_EFFECT_NAME)
            .expect("video overlay effect");
        assert_eq!(
            effect.metadata.get("mode").and_then(|v| v.as_str()),
            Some("pip")
        );
        assert_eq!(
            effect.metadata.get("corner").and_then(|v| v.as_str()),
            Some("bottom_right")
        );
    }

    #[test]
    fn apply_insert_broll_rejects_zero_duration() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertBRoll {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                asset: "raw/broll.mp4".into(),
                duration_s: 0.0,
                position: super::super::op::BRollPosition::Replace,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("must be > 0")),
            "expected duration error, got {err:?}",
        );
    }

    #[test]
    fn apply_insert_transition_lands_between_adjacent_clips() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 1.0,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 1);

        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // 3 clips + 1 transition = 4 children; transition sits at index 1.
        assert_eq!(t.children.len(), 4);
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-0"));
        let TrackChild::Transition(tr) = &t.children[1] else {
            panic!("expected transition at index 1, got {:?}", t.children[1])
        };
        assert_eq!(tr.transition_type, "SMPTE_Dissolve");
        // symmetric: half the duration on each side.
        assert!((tr.in_offset.to_seconds() - 0.5).abs() < 1e-9);
        assert!((tr.out_offset.to_seconds() - 0.5).abs() < 1e-9);
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-1"));
    }

    #[test]
    fn apply_set_cut_intent_persists_boundary_metadata() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetCutIntent {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                spec: SemanticCutSpec {
                    cut_type: CutType::CutOnAction,
                    intent: "hide_action_continuity".into(),
                    energy: Some(0.7),
                    audio_relation: AudioRelation::Sync,
                    confidence: Some(0.9),
                    reason: Some("motion carries across the boundary".into()),
                    extra: Default::default(),
                },
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0].description.contains("set cut intent"),
            "unexpected outcome: {:?}",
            outcome.applied
        );
        let meta = new_tl.metadata.montage.as_ref().expect("montage metadata");
        let key = cut_boundary_key("clip-0", "clip-1");
        let spec = meta
            .cut_boundaries
            .get(&key)
            .expect("stored cut boundary spec");
        assert_eq!(spec.cut_type, CutType::CutOnAction);
        assert_eq!(spec.intent, "hide_action_continuity");
        assert_eq!(spec.audio_relation, AudioRelation::Sync);
        assert_eq!(
            spec.reason.as_deref(),
            Some("motion carries across the boundary")
        );
    }

    #[test]
    fn apply_set_cut_intent_allows_lifted_gap_between_boundary_clips() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::DeleteClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                },
                EdlOp::SetCutIntent {
                    between: super::super::op::TransitionBetween {
                        from: Anchor::ClipUuid {
                            uuid: "clip-0".into(),
                        },
                        to: Anchor::ClipUuid {
                            uuid: "clip-2".into(),
                        },
                    },
                    spec: SemanticCutSpec {
                        cut_type: CutType::JCut,
                        intent: "lifted_gap_handoff".into(),
                        energy: None,
                        audio_relation: AudioRelation::AudioLeads,
                        confidence: Some(0.88),
                        reason: Some("deleted reset take leaves timing gap".into()),
                        extra: Default::default(),
                    },
                },
            ],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(outcome.applied.len(), 2);
        let meta = new_tl.metadata.montage.as_ref().expect("montage metadata");
        let key = cut_boundary_key("clip-0", "clip-2");
        assert_eq!(
            meta.cut_boundaries.get(&key).map(|spec| spec.cut_type),
            Some(CutType::JCut)
        );
    }

    #[test]
    fn apply_audio_lead_and_trail_stamp_split_edit_metadata() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetAudioLead {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    lead_s: 0.45,
                    split_edit: Some(SplitEditSpec {
                        audio_lead_s: Some(0.45),
                        audio_lead_from_clip_id: None,
                        audio_trail_s: None,
                        audio_trail_to_clip_id: None,
                        reason: Some("let next speaker enter before picture".into()),
                        confidence: Some(0.87),
                    }),
                },
                EdlOp::SetAudioTrail {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    trail_s: 0.60,
                    split_edit: Some(SplitEditSpec {
                        audio_lead_s: None,
                        audio_lead_from_clip_id: None,
                        audio_trail_s: Some(0.60),
                        audio_trail_to_clip_id: None,
                        reason: Some("hold answer under reaction".into()),
                        confidence: Some(0.82),
                    }),
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        let split = clip
            .metadata
            .montage
            .as_ref()
            .and_then(|m| m.split_edit.as_ref())
            .expect("split edit metadata");
        assert_eq!(split.audio_lead_s, Some(0.45));
        assert_eq!(split.audio_trail_s, Some(0.60));
        assert_eq!(split.reason.as_deref(), Some("hold answer under reaction"));
        assert_eq!(split.confidence, Some(0.82));

        let meta = new_tl.metadata.montage.as_ref().expect("montage metadata");
        let j_key = cut_boundary_key("clip-0", "clip-1");
        assert_eq!(
            meta.cut_boundaries
                .get(&j_key)
                .map(|spec| (spec.cut_type, spec.audio_relation)),
            Some((CutType::JCut, AudioRelation::AudioLeads))
        );
        let l_key = cut_boundary_key("clip-1", "clip-2");
        assert_eq!(
            meta.cut_boundaries
                .get(&l_key)
                .map(|spec| (spec.cut_type, spec.audio_relation)),
            Some((CutType::LCut, AudioRelation::AudioTrails))
        );
    }

    #[test]
    fn apply_insert_transition_replaces_existing_transition_between_same_clips() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let insert_env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 0.3,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let (tl_with_transition, _) = apply(&tl, &insert_env, &AnchorContext::empty()).unwrap();

        let replace_env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 2.0,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let (new_tl, outcome) =
            apply(&tl_with_transition, &replace_env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0]
                .description
                .contains("replaced transition"),
            "expected replacement outcome, got {:?}",
            outcome.applied,
        );

        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 4);
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-0"));
        let TrackChild::Transition(tr) = &t.children[1] else {
            panic!("expected transition at index 1, got {:?}", t.children[1])
        };
        assert_eq!(tr.transition_type, "SMPTE_Dissolve");
        assert!((tr.in_offset.to_seconds() - 1.0).abs() < 1e-9);
        assert!((tr.out_offset.to_seconds() - 1.0).abs() < 1e-9);
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-1"));
        assert!(matches!(&t.children[3], TrackChild::Clip(c) if c.name == "clip-2"));
    }

    #[test]
    fn apply_delete_transition_removes_existing_transition() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let insert_env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 0.3,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let (tl_with_transition, _) = apply(&tl, &insert_env, &AnchorContext::empty()).unwrap();

        let delete_env = EdlEnvelope {
            ops: vec![EdlOp::DeleteTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
            }],
        };
        let (new_tl, outcome) =
            apply(&tl_with_transition, &delete_env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0]
                .description
                .contains("deleted transition"),
            "expected delete outcome, got {:?}",
            outcome.applied,
        );
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 3);
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-0"));
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-1"));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-2"));
    }

    #[test]
    fn apply_insert_transition_persists_semantic_metadata() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "montage.slide_left".into(),
                duration_s: 0.28,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: Some(montage_proto::transitions::SemanticTransitionSpec {
                    id: "montage.slide_left".into(),
                    family: Some("slide".into()),
                    intent: Some("hide_motion_jump".into()),
                    energy: Some(0.7),
                    direction: Some("left".into()),
                    params: serde_json::Map::new(),
                    composition: None,
                }),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Transition(tr) = &t.children[1] else {
            panic!("expected transition")
        };
        assert_eq!(tr.transition_type, "montage.slide_left");
        let meta = tr
            .metadata
            .get("montage_transition")
            .and_then(serde_json::Value::as_object)
            .unwrap();
        assert_eq!(
            meta.get("id").and_then(|v| v.as_str()),
            Some("montage.slide_left")
        );
        assert_eq!(
            meta.get("intent").and_then(|v| v.as_str()),
            Some("hide_motion_jump")
        );
        let composition = meta
            .get("composition")
            .and_then(serde_json::Value::as_object)
            .unwrap();
        let primitives = composition
            .get("primitives")
            .and_then(serde_json::Value::as_array)
            .unwrap();
        assert!(matches!(
            primitives
                .first()
                .and_then(|primitive| primitive.get("op"))
                .and_then(serde_json::Value::as_str),
            Some("push")
        ));
    }

    #[test]
    fn apply_insert_transition_accepts_agent_composite_recipe() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let composition = montage_proto::transitions::TransitionComposition {
            version: 1,
            primitives: vec![
                montage_proto::transitions::TransitionPrimitive {
                    start: 0.0,
                    end: 1.0,
                    easing: montage_proto::transitions::TransitionEasing::EaseOutExpo,
                    op: montage_proto::transitions::TransitionPrimitiveOp::Push {
                        direction: "left".into(),
                        distance: 0.9,
                    },
                },
                montage_proto::transitions::TransitionPrimitive {
                    start: 0.35,
                    end: 0.55,
                    easing: montage_proto::transitions::TransitionEasing::EaseInOut,
                    op: montage_proto::transitions::TransitionPrimitiveOp::Flash {
                        color: "#ffffff".into(),
                        peak: 0.25,
                    },
                },
            ],
        };
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "montage.composite".into(),
                duration_s: 0.55,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: Some(montage_proto::transitions::SemanticTransitionSpec {
                    id: "montage.composite".into(),
                    family: Some("custom".into()),
                    intent: Some("beat_hit_motion_cover".into()),
                    energy: Some(0.85),
                    direction: Some("left".into()),
                    params: serde_json::Map::new(),
                    composition: Some(composition),
                }),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Transition(tr) = &t.children[1] else {
            panic!("expected transition")
        };
        assert_eq!(tr.transition_type, "montage.composite");
        let meta = tr
            .metadata
            .get("montage_transition")
            .and_then(serde_json::Value::as_object)
            .unwrap();
        assert_eq!(
            meta.get("intent").and_then(|v| v.as_str()),
            Some("beat_hit_motion_cover")
        );
        assert!(meta.get("composition").is_some());
    }

    #[test]
    fn apply_insert_transition_rejects_composite_without_recipe() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "montage.composite".into(),
                duration_s: 0.55,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("composition_json")),
            "expected missing composition error, got {err:?}",
        );
    }

    #[test]
    fn apply_insert_transition_rejects_missing_handles_before_render() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 1.0,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("incoming handle") && message.contains("Untrim Clip")),
            "expected apply-time handle error, got {err:?}",
        );
    }

    #[test]
    fn apply_insert_transition_persists_asymmetric_offsets() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 1.0,
                alignment: None,
                in_offset_s: Some(0.25),
                out_offset_s: Some(0.75),
                spec: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Transition(tr) = &t.children[1] else {
            panic!("expected transition")
        };
        assert!((tr.in_offset.to_seconds() - 0.25).abs() < 1e-9);
        assert!((tr.out_offset.to_seconds() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn apply_insert_transition_alignment_maps_to_offsets() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 0.75,
                alignment: Some(TransitionAlignment::StartAtCut),
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Transition(tr) = &t.children[1] else {
            panic!("expected transition")
        };
        assert!((tr.in_offset.to_seconds() - 0.0).abs() < 1e-9);
        assert!((tr.out_offset.to_seconds() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn apply_insert_transition_rejects_id_kind_mismatch() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "montage.cross_dissolve".into(),
                duration_s: 0.3,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: Some(montage_proto::transitions::SemanticTransitionSpec {
                    id: "montage.slide_left".into(),
                    family: Some("slide".into()),
                    intent: None,
                    energy: None,
                    direction: None,
                    params: serde_json::Map::new(),
                    composition: None,
                }),
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("must match kind")),
            "expected id/kind mismatch error, got {err:?}",
        );
    }

    #[test]
    fn apply_insert_transition_rejects_raw_ffmpeg_kind() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "slideleft".into(),
                duration_s: 0.3,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("raw FFmpeg")),
            "expected raw transition rejection, got {err:?}",
        );
    }

    #[test]
    fn apply_insert_transition_rejects_non_adjacent_anchors() {
        let tl = timeline_with_three_clips();
        // "alpha" (idx 0) and "charlie" (idx 2) are 2 apart, not adjacent.
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "charlie snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 1.0,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("not adjacent")),
            "expected adjacency error, got {err:?}",
        );
    }

    #[test]
    fn apply_insert_transition_rejects_anchors_on_different_tracks() {
        let mut tl = timeline_with_three_clips();
        // Add a second track with one clip whose snippet "delta snippet"
        // resolves on the new track only.
        let mut track2 = Track::empty("V2", TrackKind::Video);
        let mut c = Clip::empty("delta-clip".to_string());
        c.media_reference =
            MediaReference::External(ExternalReference::new("raw/delta.mp4".to_string()));
        c.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));
        c.metadata = ClipMetadata {
            montage: Some(MontageClipMetadata {
                anchor: Some(AwAnchor {
                    transcript_snippet: Some("delta snippet".to_string()),
                    ..AwAnchor::default()
                }),
                ..MontageClipMetadata::default()
            }),
            ..ClipMetadata::default()
        };
        track2.children.push(TrackChild::Clip(c));
        tl.tracks.children.push(StackChild::Track(track2));

        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "delta snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 1.0,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("different tracks")),
            "expected cross-track error, got {err:?}",
        );
    }

    #[test]
    fn apply_insert_transition_rejects_zero_duration() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 0.0,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("must be > 0")),
            "expected duration error, got {err:?}",
        );
    }

    #[test]
    fn apply_insert_transition_rejects_duration_outside_registry_bounds() {
        let mut tl = timeline_with_three_clips();
        add_one_second_handles(&mut tl);
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "alpha snippet".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "montage.flash_white".into(),
                duration_s: 1.0,
                alignment: Some(TransitionAlignment::EndAtCut),
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("outside supported range")),
            "expected duration bounds error, got {err:?}",
        );
    }

    #[test]
    fn apply_move_clip_to_start_reorders_track() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::MoveClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "charlie snippet".into(),
                },
                to_position: 0,
                at_s: None,
                snap: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-2"));
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-0"));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-1"));
    }

    #[test]
    fn apply_move_removes_cut_boundary_metadata_when_boundary_changes() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetCutIntent {
                    between: super::super::op::TransitionBetween {
                        from: Anchor::ClipUuid {
                            uuid: "clip-0".into(),
                        },
                        to: Anchor::ClipUuid {
                            uuid: "clip-1".into(),
                        },
                    },
                    spec: SemanticCutSpec {
                        cut_type: CutType::CutOnAction,
                        intent: "hide_action_continuity".into(),
                        energy: None,
                        audio_relation: AudioRelation::Sync,
                        confidence: Some(0.9),
                        reason: Some("motion carries through the boundary".into()),
                        extra: Default::default(),
                    },
                },
                EdlOp::MoveClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "clip-1".into(),
                    },
                    to_position: 2,
                    at_s: None,
                    snap: None,
                },
            ],
        };

        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();

        let meta = new_tl.metadata.montage.as_ref().expect("montage metadata");
        assert!(
            meta.cut_boundaries.is_empty(),
            "cut metadata for changed boundaries must not remain: {:?}",
            meta.cut_boundaries
        );
    }

    #[test]
    fn apply_move_clip_to_end_clamps_to_last_position() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::MoveClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "alpha snippet".into(),
                },
                to_position: 99, // way past the end — clamp.
                at_s: None,
                snap: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // alpha (was at 0) should now be at 2 (last position).
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-1"));
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-2"));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-0"));
    }

    #[test]
    fn apply_move_clip_same_position_is_a_no_op() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::MoveClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                to_position: 1, // already there.
                at_s: None,
                snap: None,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0]
                .description
                .contains("already at position"),
            "expected no-op description, got {:?}",
            outcome.applied[0].description,
        );
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-0"));
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-1"));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-2"));
    }

    #[test]
    fn apply_move_clip_to_time_leaves_gap_and_places_at_target() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::MoveClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "alpha snippet".into(),
                },
                to_position: 0,
                at_s: Some(20.0),
                snap: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert!(matches!(&t.children[0], TrackChild::Gap(_)));
        assert!((child_duration(&t.children[0]) - 5.0).abs() < 0.001);
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-1"));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-2"));
        assert!(matches!(&t.children[3], TrackChild::Gap(_)));
        assert!((child_duration(&t.children[3]) - 5.0).abs() < 0.001);
        assert!(matches!(&t.children[4], TrackChild::Clip(c) if c.name == "clip-0"));
    }

    #[test]
    fn apply_move_clip_snaps_timeline_time_to_clip_edge() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::MoveClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "alpha snippet".into(),
                },
                to_position: 0,
                at_s: Some(5.04),
                snap: Some(SnapOptions {
                    enabled: true,
                    tolerance_s: 0.1,
                    targets: vec![SnapTargetKind::ClipEdge],
                }),
            }],
        };

        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };

        assert!(matches!(&track.children[0], TrackChild::Gap(_)));
        assert!((child_duration(&track.children[0]) - 5.0).abs() < 0.001);
        assert!(matches!(&track.children[1], TrackChild::Clip(c) if c.name == "clip-0"));
    }

    #[test]
    fn apply_ripple_move_shifts_clip_forward_via_leading_gap() {
        // Three 5s clips, move the first by +3s. Expect: a 3s gap, then
        // clip-0, clip-1, clip-2 — the move preserves clip-0/1/2's
        // relative spacing because we ripple by resizing the *leading
        // gap* of the moved clip rather than removing it.
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::RippleMove {
                anchor: Anchor::TranscriptSnippet {
                    text: "alpha snippet".into(),
                },
                delta_s: 3.0,
                snap: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert!(matches!(&t.children[0], TrackChild::Gap(_)));
        assert!((child_duration(&t.children[0]) - 3.0).abs() < 0.001);
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-0"));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-1"));
        assert!(matches!(&t.children[3], TrackChild::Clip(c) if c.name == "clip-2"));
    }

    #[test]
    fn apply_ripple_move_shrinks_preceding_gap_on_negative_delta() {
        // Start: gap(5) | clip-0(5) | clip-1(5). Ripple-move clip-0 by
        // -3s. Expect: gap(2) | clip-0 | clip-1.
        use montage_proto::otio::{
            Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
            Timeline as Tl, Track, TrackChild, TrackKind,
        };
        let mut tl = Tl::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        track
            .children
            .push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                5.0, 24.0,
            )));
        for (i, name) in ["clip-0", "clip-1"].iter().enumerate() {
            let mut c = Clip::empty((*name).to_string());
            c.media_reference =
                MediaReference::External(ExternalReference::new(format!("raw/{i}.mp4")));
            c.source_range = Some(TimeRange::new(
                RationalTime::new(0.0, 24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
            track.children.push(TrackChild::Clip(c));
        }
        tl.tracks.children.push(StackChild::Track(track));

        let env = EdlEnvelope {
            ops: vec![EdlOp::RippleMove {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-0".into(),
                },
                delta_s: -3.0,
                snap: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert!(matches!(&t.children[0], TrackChild::Gap(_)));
        assert!(
            (child_duration(&t.children[0]) - 2.0).abs() < 0.001,
            "leading gap should shrink from 5 to 2; got {}",
            child_duration(&t.children[0])
        );
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-0"));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-1"));
    }

    #[test]
    fn apply_ripple_delete_closes_gap_and_shifts_downstream() {
        // Three 5s clips. Ripple-delete clip-1. Expect: clip-0,
        // clip-2 — total length 10s, no gap.
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::RippleDelete {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 2);
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-0"));
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-2"));
    }

    #[test]
    fn apply_ripple_delete_removes_matching_split_linked_sibling() {
        use montage_proto::montage_meta::MontageClipMetadata;
        use montage_proto::otio::{
            Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild,
            TimeRange, Timeline as Tl, Track, TrackChild, TrackKind,
        };

        let mut tl = Tl::empty("test");
        let mut montage_meta = MontageClipMetadata::default();
        montage_meta
            .extra
            .insert("link_group_id".into(), serde_json::json!("g0"));

        let build_clip = |name: &str| {
            let mut c = Clip::empty(name.to_string());
            c.media_reference = MediaReference::External(ExternalReference::new("raw/0.mp4"));
            c.source_range = Some(TimeRange::new(
                RationalTime::new(0.0, 24.0),
                RationalTime::new(60.0 * 24.0, 24.0),
            ));
            c.metadata = ClipMetadata {
                montage: Some(montage_meta.clone()),
                ..ClipMetadata::default()
            };
            c
        };

        let mut video = Track::empty("V1", TrackKind::Video);
        video.children.push(TrackChild::Clip(build_clip("v0")));
        let mut audio = Track::empty("A1", TrackKind::Audio);
        audio.children.push(TrackChild::Clip(build_clip("a0")));
        tl.tracks.children.push(StackChild::Track(video));
        tl.tracks.children.push(StackChild::Track(audio));

        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SplitClip {
                    anchor: Anchor::ClipUuid { uuid: "v0".into() },
                    at_s: 10.0,
                    snap: None,
                },
                EdlOp::SplitClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "v0-b".into(),
                    },
                    at_s: 20.0,
                    snap: None,
                },
                EdlOp::RippleDelete {
                    anchor: Anchor::ClipUuid {
                        uuid: "v0-b".into(),
                    },
                },
            ],
        };

        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(v) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let StackChild::Track(a) = &new_tl.tracks.children[1] else {
            panic!()
        };
        assert!(matches!(&v.children[0], TrackChild::Clip(c) if c.name == "v0"));
        assert!(matches!(&v.children[1], TrackChild::Clip(c) if c.name == "v0-b-b"));
        assert!(matches!(&a.children[0], TrackChild::Clip(c) if c.name == "a0"));
        assert!(matches!(&a.children[1], TrackChild::Clip(c) if c.name == "a0-b-b"));
        assert!(
            a.children.iter().all(|child| !matches!(
                child,
                TrackChild::Clip(c) if c.name == "a0-b"
            )),
            "ripple delete should remove the linked audio split matching source[10..20]"
        );
    }

    #[test]
    fn apply_ripple_trim_end_shifts_downstream_right() {
        // Three 5s clips, source_range [0..5]. Ripple-trim clip-0's
        // end to 7s (lengthen by 2s). Expect clip-0 duration 7s, and
        // clip-1 + clip-2 stay adjacent (so timeline grows by 2s).
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::RippleTrim {
                anchor: Anchor::TranscriptSnippet {
                    text: "alpha snippet".into(),
                },
                edge: RippleTrimEdge::End,
                value_s: 7.0,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // clip-0 should now have duration 7s.
        let TrackChild::Clip(c0) = &t.children[0] else {
            panic!()
        };
        assert!((c0.source_range.as_ref().unwrap().duration.to_seconds() - 7.0).abs() < 0.001);
        // clip-1 should follow immediately after — no leading gap.
        // (The downstream ripple grew the trailing gap of clip-0 by
        // +2s; we don't have one, so a new gap was inserted at
        // child_index+1. Both shapes are valid: either no gap
        // between them or a 0-length one. Verify children layout.)
        let total_children = t.children.len();
        assert!(total_children >= 3);
    }

    #[test]
    fn apply_delete_gap_removes_preceding_gap_before_anchor_clip() {
        // Three 5s clips + a leading 3s gap on V1. Delete the gap
        // sitting before clip-0. Expect: clip-0 | clip-1 | clip-2.
        use montage_proto::otio::{
            Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
            Timeline as Tl, Track, TrackChild, TrackKind,
        };
        let mut tl = Tl::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        track
            .children
            .push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                3.0, 24.0,
            )));
        for (i, name) in ["clip-0", "clip-1", "clip-2"].iter().enumerate() {
            let mut c = Clip::empty((*name).to_string());
            c.media_reference =
                MediaReference::External(ExternalReference::new(format!("raw/{i}.mp4")));
            c.source_range = Some(TimeRange::new(
                RationalTime::new(0.0, 24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
            track.children.push(TrackChild::Clip(c));
        }
        tl.tracks.children.push(StackChild::Track(track));
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteGap {
                anchor: Anchor::ClipUuid {
                    uuid: "clip-0".into(),
                },
                side: crate::edl::op::GapSide::Before,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 3);
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-0"));
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-1"));
        assert!(matches!(&t.children[2], TrackChild::Clip(c) if c.name == "clip-2"));
    }

    #[test]
    fn apply_delete_gap_after_is_a_no_op_when_no_trailing_gap() {
        // clip-0 has nothing after it on the track. Asking to delete
        // the trailing gap should leave the timeline unchanged with
        // a descriptive message.
        use montage_proto::otio::{
            Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
            Timeline as Tl, Track, TrackChild, TrackKind,
        };
        let mut tl = Tl::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut c = Clip::empty("solo".to_string());
        c.media_reference = MediaReference::External(ExternalReference::new("raw/solo.mp4"));
        c.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(5.0 * 24.0, 24.0),
        ));
        track.children.push(TrackChild::Clip(c));
        tl.tracks.children.push(StackChild::Track(track));
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteGap {
                anchor: Anchor::ClipUuid {
                    uuid: "solo".into(),
                },
                side: crate::edl::op::GapSide::After,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(outcome.applied[0].description.contains("no trailing gap"));
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 1);
    }

    #[test]
    fn apply_trim_track_tail_strips_trailing_gaps() {
        // Two 5s clips + a 10s trailing gap. Trimming the tail should
        // leave clip-0 | clip-1 and report 10s removed.
        use montage_proto::otio::{
            Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
            Timeline as Tl, Track, TrackChild, TrackKind,
        };
        let mut tl = Tl::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        for (i, name) in ["clip-0", "clip-1"].iter().enumerate() {
            let mut c = Clip::empty((*name).to_string());
            c.media_reference =
                MediaReference::External(ExternalReference::new(format!("raw/{i}.mp4")));
            c.source_range = Some(TimeRange::new(
                RationalTime::new(0.0, 24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
            track.children.push(TrackChild::Clip(c));
        }
        track
            .children
            .push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                10.0, 24.0,
            )));
        tl.tracks.children.push(StackChild::Track(track));
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimTrackTail { track: "V1".into() }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.children.len(), 2);
        assert!(matches!(&t.children[1], TrackChild::Clip(c) if c.name == "clip-1"));
        assert!(outcome.applied[0].description.contains("10."));
    }

    #[test]
    fn apply_delete_gap_cascades_to_link_group_siblings() {
        // V1 + A1 each carry one clip whose link_group_id matches.
        // Both tracks have a leading gap. DeleteGap(side=before) on
        // V1's clip should remove the leading gap on A1 too.
        use montage_proto::montage_meta::MontageClipMetadata;
        use montage_proto::otio::{
            Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild,
            TimeRange, Timeline as Tl, Track, TrackChild, TrackKind,
        };
        let mut tl = Tl::empty("test");
        let mut montage_meta = MontageClipMetadata::default();
        montage_meta
            .extra
            .insert("link_group_id".into(), serde_json::json!("g0"));
        let build = |name: &str, kind: TrackKind, clip_name: &str| {
            let mut t = Track::empty(name, kind);
            t.children
                .push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                    3.0, 24.0,
                )));
            let mut c = Clip::empty(clip_name.to_string());
            c.media_reference = MediaReference::External(ExternalReference::new("raw/0.mp4"));
            c.source_range = Some(TimeRange::new(
                RationalTime::new(0.0, 24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
            c.metadata = ClipMetadata {
                montage: Some(montage_meta.clone()),
                ..ClipMetadata::default()
            };
            t.children.push(TrackChild::Clip(c));
            t
        };
        tl.tracks
            .children
            .push(StackChild::Track(build("V1", TrackKind::Video, "v0")));
        tl.tracks
            .children
            .push(StackChild::Track(build("A1", TrackKind::Audio, "a0")));

        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteGap {
                anchor: Anchor::ClipUuid { uuid: "v0".into() },
                side: crate::edl::op::GapSide::Before,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0].description.contains("cascaded"),
            "expected cascade message; got {:?}",
            outcome.applied[0].description,
        );
        let StackChild::Track(v) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let StackChild::Track(a) = &new_tl.tracks.children[1] else {
            panic!()
        };
        assert!(matches!(&v.children[0], TrackChild::Clip(c) if c.name == "v0"));
        assert!(matches!(&a.children[0], TrackChild::Clip(c) if c.name == "a0"));
    }

    #[test]
    fn split_right_half_b_alias_resolves_within_envelope() {
        // Real projects anchor by montage.clip_uuid (≠ clip name), and
        // batch compilers reference a split's right half as
        // `{left_uuid}-b` — but the engine stamps a FRESH uuid on the
        // right half. The envelope-scoped alias must bridge that:
        // split/split/ripple-delete carving [4s..6s] out of one clip.
        use montage_proto::montage_meta::MontageClipMetadata;
        use montage_proto::otio::{
            Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild,
            TimeRange, Timeline as Tl, Track, TrackChild, TrackKind,
        };
        let mut tl = Tl::empty("test");
        let mut t = Track::empty("V1", TrackKind::Video);
        let mut montage_meta = MontageClipMetadata::default();
        montage_meta
            .extra
            .insert("clip_uuid".into(), serde_json::json!("c-111"));
        let mut c = Clip::empty("episode".to_string());
        c.media_reference = MediaReference::External(ExternalReference::new("raw/ep.mp4"));
        c.source_range = Some(TimeRange::new(
            RationalTime::new(0.0, 24.0),
            RationalTime::new(10.0 * 24.0, 24.0),
        ));
        c.metadata = ClipMetadata {
            montage: Some(montage_meta),
            ..ClipMetadata::default()
        };
        t.children.push(TrackChild::Clip(c));
        tl.tracks.children.push(StackChild::Track(t));

        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SplitClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "c-111".into(),
                    },
                    at_s: 4.0,
                    snap: None,
                },
                EdlOp::SplitClip {
                    anchor: Anchor::ClipUuid {
                        uuid: "c-111-b".into(),
                    },
                    at_s: 6.0,
                    snap: None,
                },
                EdlOp::RippleDelete {
                    anchor: Anchor::ClipUuid {
                        uuid: "c-111-b".into(),
                    },
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty())
            .expect("-b alias must resolve to the split's actual right half");
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let clips: Vec<_> = t
            .children
            .iter()
            .filter_map(|child| match child {
                TrackChild::Clip(c) => c.source_range.as_ref().map(|r| {
                    (
                        r.start_time.to_seconds(),
                        r.start_time.to_seconds() + r.duration.to_seconds(),
                    )
                }),
                _ => None,
            })
            .collect();
        assert_eq!(clips.len(), 2, "middle piece should be ripple-deleted");
        assert!((clips[0].0 - 0.0).abs() < 1e-6 && (clips[0].1 - 4.0).abs() < 1e-6);
        assert!((clips[1].0 - 6.0).abs() < 1e-6 && (clips[1].1 - 10.0).abs() < 1e-6);
    }

    #[test]
    fn apply_untrim_widens_link_group_siblings() {
        // Restoring a cut via Untrim must widen the linked audio
        // sibling too, or the A/V pair desyncs (podcast session trace).
        use montage_proto::montage_meta::MontageClipMetadata;
        use montage_proto::otio::{
            Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild,
            TimeRange, Timeline as Tl, Track, TrackChild, TrackKind,
        };
        let mut tl = Tl::empty("test");
        let mut montage_meta = MontageClipMetadata::default();
        montage_meta
            .extra
            .insert("link_group_id".into(), serde_json::json!("g0"));
        let build = |name: &str, kind: TrackKind, clip_name: &str| {
            let mut t = Track::empty(name, kind);
            let mut c = Clip::empty(clip_name.to_string());
            c.media_reference = MediaReference::External(ExternalReference::new("raw/0.mp4"));
            c.source_range = Some(TimeRange::new(
                RationalTime::new(1.0 * 24.0, 24.0),
                RationalTime::new(4.0 * 24.0, 24.0),
            ));
            c.metadata = ClipMetadata {
                montage: Some(montage_meta.clone()),
                ..ClipMetadata::default()
            };
            t.children.push(TrackChild::Clip(c));
            t
        };
        tl.tracks
            .children
            .push(StackChild::Track(build("V1", TrackKind::Video, "v0")));
        tl.tracks
            .children
            .push(StackChild::Track(build("A1", TrackKind::Audio, "a0")));

        let env = EdlEnvelope {
            ops: vec![EdlOp::UntrimClip {
                anchor: Anchor::ClipUuid { uuid: "v0".into() },
                start: None,
                end: Some(8.0),
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0].description.contains("linked sibling"),
            "expected linked-sibling note; got {:?}",
            outcome.applied[0].description,
        );
        let StackChild::Track(a) = &new_tl.tracks.children[1] else {
            panic!()
        };
        let TrackChild::Clip(audio) = &a.children[0] else {
            panic!()
        };
        let r = audio.source_range.as_ref().unwrap();
        assert!((r.start_time.to_seconds() - 1.0).abs() < 1e-6);
        assert!(
            (r.duration.to_seconds() - 7.0).abs() < 1e-6,
            "audio must widen to [1..8]"
        );
    }

    #[test]
    fn apply_trim_track_tail_cascades_to_link_group_siblings() {
        // V1 + A1 each end with a clip sharing link_group_id, then a
        // trailing gap. Trim Track Tail on V1 should also trim A1.
        use montage_proto::montage_meta::MontageClipMetadata;
        use montage_proto::otio::{
            Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild,
            TimeRange, Timeline as Tl, Track, TrackChild, TrackKind,
        };
        let mut tl = Tl::empty("test");
        let mut montage_meta = MontageClipMetadata::default();
        montage_meta
            .extra
            .insert("link_group_id".into(), serde_json::json!("g0"));
        let build = |name: &str, kind: TrackKind, clip_name: &str| {
            let mut t = Track::empty(name, kind);
            let mut c = Clip::empty(clip_name.to_string());
            c.media_reference = MediaReference::External(ExternalReference::new("raw/0.mp4"));
            c.source_range = Some(TimeRange::new(
                RationalTime::new(0.0, 24.0),
                RationalTime::new(5.0 * 24.0, 24.0),
            ));
            c.metadata = ClipMetadata {
                montage: Some(montage_meta.clone()),
                ..ClipMetadata::default()
            };
            t.children.push(TrackChild::Clip(c));
            t.children
                .push(TrackChild::Gap(montage_proto::otio::Gap::of_duration(
                    7.0, 24.0,
                )));
            t
        };
        tl.tracks
            .children
            .push(StackChild::Track(build("V1", TrackKind::Video, "v0")));
        tl.tracks
            .children
            .push(StackChild::Track(build("A1", TrackKind::Audio, "a0")));

        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimTrackTail { track: "V1".into() }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0]
                .description
                .contains("cascaded to linked tracks"),
            "expected cascade in description, got {:?}",
            outcome.applied[0].description,
        );
        let StackChild::Track(v) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let StackChild::Track(a) = &new_tl.tracks.children[1] else {
            panic!()
        };
        assert_eq!(v.children.len(), 1);
        assert_eq!(a.children.len(), 1);
    }

    #[test]
    fn apply_trim_track_tail_unknown_track_lists_available_names() {
        // Agent guessed a single-letter track name; the error message
        // should include the real track names so the next attempt can
        // self-correct without a separate view_timeline call.
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimTrackTail { track: "V".into() }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        let message = format!("{err}");
        assert!(
            message.contains("available tracks:"),
            "expected hint listing available tracks; got {message}"
        );
        assert!(
            message.contains("V1"),
            "expected V1 (the fixture's track name) in the hint; got {message}"
        );
    }

    #[test]
    fn apply_trim_track_tail_no_op_when_track_already_ends_in_clip() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::TrimTrackTail { track: "V1".into() }],
        };
        let (_, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0]
                .description
                .contains("already ends in a clip"),
            "expected no-op description, got {:?}",
            outcome.applied[0].description,
        );
    }

    #[test]
    fn apply_insert_track_appends_video_track_after_existing_video_tracks() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTrack {
                name: "V2".into(),
                kind: InsertTrackKind::Video,
                at_position: None,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(
            outcome.applied[0].description.contains("V2"),
            "got {:?}",
            outcome.applied[0].description,
        );
        let names: Vec<String> = new_tl
            .tracks
            .children
            .iter()
            .filter_map(|c| match c {
                StackChild::Track(t) => Some(t.name.clone()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"V2".to_string()), "names = {names:?}");
    }

    #[test]
    fn apply_insert_track_is_idempotent_on_same_name_and_kind() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTrack {
                name: "V1".into(),
                kind: InsertTrackKind::Video,
                at_position: None,
            }],
        };
        let before = tl.tracks.children.len();
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(
            new_tl.tracks.children.len(),
            before,
            "no-op should not change track count",
        );
        assert!(
            outcome.applied[0].description.contains("already exists"),
            "got {:?}",
            outcome.applied[0].description,
        );
    }

    #[test]
    fn apply_insert_track_errors_on_name_conflict_with_different_kind() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTrack {
                name: "V1".into(),
                kind: InsertTrackKind::Audio,
                at_position: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            err.to_string().contains("exists as"),
            "expected kind-conflict, got {err}",
        );
    }

    #[test]
    fn apply_delete_track_removes_empty_track() {
        // Two tracks: V1 with clips + V2 empty. Delete V2; V1 stays.
        use montage_proto::otio::{StackChild, Timeline as Tl, Track, TrackKind};
        let mut tl = timeline_with_three_clips();
        tl.tracks
            .children
            .push(StackChild::Track(Track::empty("V2", TrackKind::Video)));
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteTrack {
                name: "V2".into(),
                force: false,
            }],
        };
        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert!(outcome.applied[0].description.contains("removed"));
        assert_eq!(new_tl.tracks.children.len(), 1);
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        assert_eq!(t.name, "V1");
        let _ = Tl::empty("test"); // silence unused import in some configs
    }

    #[test]
    fn apply_delete_track_refuses_populated_track_without_force() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteTrack {
                name: "V1".into(),
                force: false,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            err.to_string().contains("has 3 clip"),
            "expected populated-track refusal, got {err}",
        );
    }

    #[test]
    fn apply_delete_track_with_force_drops_populated_track() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteTrack {
                name: "V1".into(),
                force: true,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(new_tl.tracks.children.len(), 0);
    }

    #[test]
    fn apply_delete_track_lists_available_names_on_miss() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::DeleteTrack {
                name: "VNope".into(),
                force: false,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("available tracks:") && message.contains("V1"),
            "expected hint listing real names; got {message}"
        );
    }

    #[test]
    fn apply_ripple_move_clamps_to_zero_on_excessive_negative_delta() {
        // Move the first clip by -999s. Expect the leading gap to
        // collapse to 0 (and be removed), with the clip pinned at
        // track start.
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::RippleMove {
                anchor: Anchor::TranscriptSnippet {
                    text: "alpha snippet".into(),
                },
                delta_s: -999.0,
                snap: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        // clip-0 is still first; no leading gap inserted.
        assert!(matches!(&t.children[0], TrackChild::Clip(c) if c.name == "clip-0"));
    }

    #[test]
    fn snap_resolution_uses_marker_targets_within_tolerance() {
        let mut tl = timeline_with_three_clips();
        let StackChild::Track(track) = &mut tl.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Clip(clip) = &mut track.children[1] else {
            panic!("expected clip")
        };
        clip.markers.push(montage_proto::otio::Marker::new(
            "snap here",
            TimeRange::new(
                RationalTime::new(1.0 * 24.0, 24.0),
                RationalTime::new(0.0, 24.0),
            ),
        ));

        let snapped = resolve_snap_time(
            &tl,
            0,
            5.96,
            Some(&SnapOptions {
                enabled: true,
                tolerance_s: 0.1,
                targets: vec![SnapTargetKind::Marker],
            }),
        )
        .unwrap();

        assert!((snapped - 6.0).abs() < 0.001);
    }

    #[test]
    fn apply_split_clip_snaps_at_s_to_marker_within_tolerance() {
        // Build a timeline with three 5s clips on V1. The middle clip
        // ("bravo snippet") spans timeline [5.0..10.0] and source
        // [0.0..5.0]. Stamp a marker at clip-local 2.0s so the timeline
        // position is 5.0 + 2.0 = 7.0s. The agent asks to split at
        // source-time 1.96s — the corresponding timeline time is
        // 6.96s, which is 0.04s from the marker. With a 0.1s
        // tolerance, snap should pull the split to source-time 2.0s.
        let mut tl = timeline_with_three_clips();
        let StackChild::Track(track) = &mut tl.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Clip(bravo) = &mut track.children[1] else {
            panic!("expected clip")
        };
        bravo.markers.push(montage_proto::otio::Marker::new(
            "snap here",
            TimeRange::new(
                RationalTime::new(2.0 * 24.0, 24.0),
                RationalTime::new(0.0, 24.0),
            ),
        ));

        let env = EdlEnvelope {
            ops: vec![EdlOp::SplitClip {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                at_s: 1.96,
                snap: Some(SnapOptions {
                    enabled: true,
                    tolerance_s: 0.1,
                    targets: vec![SnapTargetKind::Marker],
                }),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };
        // After splitting bravo at source 2.0s, the left piece keeps
        // [0.0..2.0] (2s duration) and the right piece holds [2.0..5.0]
        // (3s duration).
        let TrackChild::Clip(left) = &t.children[1] else {
            panic!("expected left split clip")
        };
        let TrackChild::Clip(right) = &t.children[2] else {
            panic!("expected right split clip")
        };
        let left_dur = left.source_range.as_ref().unwrap().duration.to_seconds();
        let right_dur = right.source_range.as_ref().unwrap().duration.to_seconds();
        assert!(
            (left_dur - 2.0).abs() < 0.001,
            "left duration should snap to 2s, got {left_dur}",
        );
        assert!(
            (right_dur - 3.0).abs() < 0.001,
            "right duration should snap to 3s, got {right_dur}",
        );
    }

    #[test]
    fn apply_insert_clip_snaps_at_s_to_clip_edge() {
        // Three 5s clips on V1 → timeline edges at 0, 5, 10, 15.
        // Insert with at_s = 4.96s and a 0.1s tolerance + ClipEdge
        // snap target → snaps to 5.0s, placing the new clip
        // immediately after clip-0 (position 1).
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertClip {
                asset: "raw/inserted.mp4".into(),
                track: "V1".into(),
                track_kind: None,
                at_position: None,
                at_s: Some(4.96),
                start: Some(0.0),
                end: Some(2.0),
                name: Some("snap-inserted".into()),
                link_group_id: None,
                snap: Some(SnapOptions {
                    enabled: true,
                    tolerance_s: 0.1,
                    targets: vec![SnapTargetKind::ClipEdge],
                }),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };
        // Expect: [clip-0 (5s), snap-inserted (2s), clip-1, clip-2].
        // The insert lands exactly at the clip-0/clip-1 boundary
        // because snap pulled 4.96 → 5.0; no gap is needed.
        let TrackChild::Clip(inserted) = &t.children[1] else {
            panic!(
                "expected inserted clip at position 1, got {:?}",
                t.children[1]
            )
        };
        assert_eq!(inserted.name, "snap-inserted");
        let TrackChild::Clip(c2) = &t.children[2] else {
            panic!("expected clip-1 at position 2")
        };
        assert_eq!(c2.name, "clip-1");
    }

    #[test]
    fn apply_multicam_plan_replaces_program_track_atomically() {
        let mut tl = timeline_with_three_clips();
        tl.tracks.children.push(StackChild::Track(Track::empty(
            "Program Video",
            TrackKind::Video,
        )));
        let env = EdlEnvelope {
            ops: vec![EdlOp::ApplyMulticamPlan {
                plan: MulticamApplyPlan {
                    program_track: "Program Video".into(),
                    decisions: vec![
                        MulticamDecision {
                            source_asset: "raw/cam-a.mov".into(),
                            start_s: 0.0,
                            end_s: 3.0,
                            // cam-a started 2s late on the shoot → its source
                            // for program 0..3 is source 2..5.
                            source_start_s: Some(2.0),
                            source_end_s: Some(5.0),
                            reason: Some("speaker A".into()),
                            speaker: Some("A".into()),
                            sync_group_id: Some("sync-1".into()),
                        },
                        MulticamDecision {
                            source_asset: "raw/cam-b.mov".into(),
                            start_s: 3.0,
                            end_s: 6.0,
                            reason: Some("reaction".into()),
                            speaker: None,
                            sync_group_id: Some("sync-1".into()),
                            ..Default::default()
                        },
                    ],
                },
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let program_tracks: Vec<&Track> = new_tl
            .tracks
            .children
            .iter()
            .filter_map(|child| match child {
                StackChild::Track(track) if track.name == "Program Video" => Some(track),
                _ => None,
            })
            .collect();
        let program = program_tracks.first().expect("program track");
        let TrackChild::Clip(first) = &program.children[0] else {
            panic!("expected first multicam clip")
        };

        assert_eq!(program_tracks.len(), 1);
        assert_eq!(program.children.len(), 2);
        assert!(outcome.applied[0].description.contains("multicam plan"));
        assert!(matches!(
            &first.media_reference,
            MediaReference::External(reference) if reference.target_url == "raw/cam-a.mov"
        ));
        let metadata = first.metadata.montage.as_ref().expect("montage metadata");
        assert_eq!(metadata.reasoning.as_deref(), Some("speaker A"));
        assert_eq!(
            metadata
                .extra
                .get("sync_group_id")
                .and_then(serde_json::Value::as_str),
            Some("sync-1")
        );
        // The clip reads the offset-corrected source IN point (2.0), not the
        // program start (0.0); timeline duration stays the program span (3.0).
        let range = first.source_range.as_ref().expect("source_range");
        assert!((range.start_time.to_seconds() - 2.0).abs() < 1e-6);
        assert!((range.duration.to_seconds() - 3.0).abs() < 1e-6);
    }

    #[test]
    fn apply_multicam_plan_rejects_invalid_decision_before_mutation() {
        let tl = timeline_with_three_clips();
        let before = serde_json::to_string(&tl).unwrap();
        let env = EdlEnvelope {
            ops: vec![EdlOp::ApplyMulticamPlan {
                plan: MulticamApplyPlan {
                    program_track: "Program Video".into(),
                    decisions: vec![
                        MulticamDecision {
                            source_asset: "raw/cam-a.mov".into(),
                            start_s: 0.0,
                            end_s: 5.0,
                            ..Default::default()
                        },
                        MulticamDecision {
                            source_asset: "raw/cam-b.mov".into(),
                            start_s: 4.0,
                            end_s: 6.0,
                            ..Default::default()
                        },
                    ],
                },
            }],
        };

        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        let after = serde_json::to_string(&tl).unwrap();

        assert!(format!("{err}").contains("before previous decision ends"));
        assert_eq!(before, after);
    }

    #[test]
    fn professional_wrap_as_nested_creates_track_child_stack() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::ProfessionalTimelineEdit {
                edit: ProfessionalTimelineEdit::WrapAsNested {
                    track: "V1".into(),
                    start_index: 0,
                    end_index: 2,
                    name: Some("Angle group".into()),
                },
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };
        let TrackChild::Stack(stack) = &track.children[0] else {
            panic!("expected nested stack")
        };
        let StackChild::Track(nested_track) = &stack.children[0] else {
            panic!("expected nested track")
        };

        assert!(outcome.applied[0].description.contains("nested stack"));
        assert_eq!(stack.name, "Angle group");
        assert_eq!(nested_track.children.len(), 2);
        assert!(matches!(&track.children[1], TrackChild::Clip(clip) if clip.name == "clip-2"));
    }

    #[test]
    fn professional_flatten_nested_restores_children() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::WrapAsNested {
                        track: "V1".into(),
                        start_index: 0,
                        end_index: 2,
                        name: Some("Angle group".into()),
                    },
                },
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::FlattenNested {
                        track: "V1".into(),
                        child_index: 0,
                    },
                },
            ],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(track) = &new_tl.tracks.children[0] else {
            panic!("expected track")
        };

        assert!(
            outcome.applied[1]
                .description
                .contains("flattened nested stack")
        );
        assert_eq!(track.children.len(), 3);
        assert!(matches!(&track.children[0], TrackChild::Clip(clip) if clip.name == "clip-0"));
        assert!(matches!(&track.children[1], TrackChild::Clip(clip) if clip.name == "clip-1"));
        assert!(matches!(&track.children[2], TrackChild::Clip(clip) if clip.name == "clip-2"));
    }

    #[test]
    fn apply_insert_transition_anchor_miss_surfaces_clearly() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTransition {
                between: super::super::op::TransitionBetween {
                    from: Anchor::TranscriptSnippet {
                        text: "no such phrase here".into(),
                    },
                    to: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
                kind: "SMPTE_Dissolve".into(),
                duration_s: 1.0,
                alignment: None,
                in_offset_s: None,
                out_offset_s: None,
                spec: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::AnchorMiss { .. }),
            "expected anchor miss, got something else",
        );
    }

    #[test]
    fn apply_set_volume_stamps_effect_with_value() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetVolume {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                value: 0.5,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        assert_eq!(clip.effects[0].effect_name, "montage.volume");
        let v = clip.effects[0]
            .metadata
            .get("value")
            .and_then(|v| v.as_f64())
            .unwrap();
        assert!((v - 0.5).abs() < 1e-9);
    }

    #[test]
    fn apply_mute_clip_sets_and_clears_override() {
        let tl = timeline_with_three_clips();
        let anchor = || Anchor::TranscriptSnippet {
            text: "bravo snippet".into(),
        };

        // Mute → audio_override.muted = true; no effects added (picture kept).
        let env = EdlEnvelope {
            ops: vec![EdlOp::MuteClip {
                anchor: anchor(),
                muted: true,
            }],
        };
        let (muted_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &muted_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert!(
            clip.metadata
                .montage
                .as_ref()
                .and_then(|m| m.audio_override.as_ref())
                .map(|o| o.muted)
                .unwrap_or(false),
            "clip audio must be muted"
        );
        assert!(
            clip.effects.is_empty(),
            "mute is metadata, not an effect — picture untouched"
        );

        // Unmute → override carries nothing else, so it is cleared entirely.
        let env = EdlEnvelope {
            ops: vec![EdlOp::MuteClip {
                anchor: anchor(),
                muted: false,
            }],
        };
        let (unmuted_tl, _) = apply(&muted_tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &unmuted_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert!(
            clip.metadata
                .montage
                .as_ref()
                .and_then(|m| m.audio_override.as_ref())
                .is_none(),
            "unmuting with no other override clears it"
        );
    }

    #[test]
    fn apply_remove_audio_appends_and_clears_ranges() {
        let tl = timeline_with_three_clips();
        let anchor = || Anchor::TranscriptSnippet {
            text: "bravo snippet".into(),
        };
        let removed = |tl: &Timeline| -> Vec<AudioRange> {
            let StackChild::Track(t) = &tl.tracks.children[0] else {
                panic!()
            };
            let TrackChild::Clip(clip) = &t.children[1] else {
                panic!()
            };
            clip.metadata
                .montage
                .as_ref()
                .and_then(|m| m.audio_override.as_ref())
                .map(|o| o.removed_ranges.clone())
                .unwrap_or_default()
        };

        // Two removals accumulate; picture untouched (no effects added).
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::RemoveAudio {
                    anchor: anchor(),
                    start_s: Some(1.0),
                    end_s: Some(2.0),
                    clear: false,
                },
                EdlOp::RemoveAudio {
                    anchor: anchor(),
                    start_s: Some(4.0),
                    end_s: Some(5.0),
                    clear: false,
                },
            ],
        };
        let (added_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let ranges = removed(&added_tl);
        assert_eq!(ranges.len(), 2);
        assert_eq!((ranges[0].start_s, ranges[0].end_s), (1.0, 2.0));
        assert_eq!((ranges[1].start_s, ranges[1].end_s), (4.0, 5.0));

        // clear:true drops all removals and (no mute) clears the override.
        let env = EdlEnvelope {
            ops: vec![EdlOp::RemoveAudio {
                anchor: anchor(),
                start_s: None,
                end_s: None,
                clear: true,
            }],
        };
        let (cleared_tl, _) = apply(&added_tl, &env, &AnchorContext::empty()).unwrap();
        assert!(removed(&cleared_tl).is_empty());
        let StackChild::Track(t) = &cleared_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert!(
            clip.metadata
                .montage
                .as_ref()
                .and_then(|m| m.audio_override.as_ref())
                .is_none(),
            "clearing the only override removes it"
        );
    }

    #[test]
    fn apply_remove_audio_rejects_inverted_span() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::RemoveAudio {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                start_s: Some(5.0),
                end_s: Some(3.0),
                clear: false,
            }],
        };
        assert!(apply(&tl, &env, &AnchorContext::empty()).is_err());
    }

    #[test]
    fn apply_set_volume_replaces_existing_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetVolume {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    value: 0.5,
                },
                EdlOp::SetVolume {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    value: 0.8,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        let v = clip.effects[0]
            .metadata
            .get("value")
            .and_then(|v| v.as_f64())
            .unwrap();
        assert!((v - 0.8).abs() < 1e-9);
    }

    #[test]
    fn apply_set_volume_rejects_negative_value() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetVolume {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                value: -0.1,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains(">= 0.0")),
            "expected validation error, got {err:?}",
        );
    }

    #[test]
    fn apply_set_effect_stamps_validated_metadata() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.color_correction".into(),
                params: serde_json::Map::from_iter([
                    ("contrast".into(), serde_json::json!(1.15)),
                    ("saturation".into(), serde_json::json!(0.9)),
                ]),
                rationale: Some("subtle correction for flat camera angle".into()),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        assert_eq!(clip.effects[0].effect_name, "montage.color_correction");
        assert_eq!(clip.effects[0].name, "Color Correction");
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("contrast")
                .and_then(|v| v.as_f64()),
            Some(1.15)
        );
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("rationale")
                .and_then(|v| v.as_str()),
            Some("subtle correction for flat camera angle")
        );
    }

    #[test]
    fn apply_set_effect_stamps_shake_defaults() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.shake".into(),
                params: serde_json::Map::new(),
                rationale: Some("add handheld emphasis".into()),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        let effect = clip
            .effects
            .iter()
            .find(|effect| effect.effect_name == "montage.shake")
            .expect("shake effect should be stamped");
        assert_eq!(effect.name, "Shake");
        assert_eq!(
            effect.metadata.get("intensity_px").and_then(|v| v.as_f64()),
            Some(8.0)
        );
        assert_eq!(
            effect.metadata.get("frequency_hz").and_then(|v| v.as_f64()),
            Some(12.0)
        );
        assert_eq!(
            effect.metadata.get("rationale").and_then(|v| v.as_str()),
            Some("add handheld emphasis")
        );
    }

    #[test]
    fn apply_set_effect_stamps_blur_defaults() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.blur".into(),
                params: serde_json::Map::new(),
                rationale: Some("soften distracting background".into()),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        let effect = clip
            .effects
            .iter()
            .find(|effect| effect.effect_name == "montage.blur")
            .expect("blur effect should be stamped");
        assert_eq!(effect.name, "Blur");
        assert_eq!(
            effect.metadata.get("radius_px").and_then(|v| v.as_f64()),
            Some(8.0)
        );
        assert_eq!(
            effect.metadata.get("rationale").and_then(|v| v.as_str()),
            Some("soften distracting background")
        );
    }

    #[test]
    fn apply_set_effect_stamps_warp_defaults() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.warp".into(),
                params: serde_json::Map::new(),
                rationale: Some("add stylized lens bend".into()),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        let effect = clip
            .effects
            .iter()
            .find(|effect| effect.effect_name == "montage.warp")
            .expect("warp effect should be stamped");
        assert_eq!(effect.name, "Warp");
        assert_eq!(
            effect.metadata.get("k1").and_then(|v| v.as_f64()),
            Some(-0.08)
        );
        assert_eq!(
            effect.metadata.get("k2").and_then(|v| v.as_f64()),
            Some(0.0)
        );
        assert_eq!(
            effect.metadata.get("center_x").and_then(|v| v.as_f64()),
            Some(0.5)
        );
        assert_eq!(
            effect.metadata.get("center_y").and_then(|v| v.as_f64()),
            Some(0.5)
        );
        assert_eq!(
            effect.metadata.get("rationale").and_then(|v| v.as_str()),
            Some("add stylized lens bend")
        );
    }

    #[test]
    fn apply_set_effect_replaces_same_id_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetEffect {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    effect: "montage.color_correction".into(),
                    params: serde_json::Map::from_iter([(
                        "contrast".into(),
                        serde_json::json!(1.15),
                    )]),
                    rationale: None,
                },
                EdlOp::SetEffect {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    effect: "montage.color_correction".into(),
                    params: serde_json::Map::from_iter([(
                        "saturation".into(),
                        serde_json::json!(0.8),
                    )]),
                    rationale: None,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        let color_effects: Vec<_> = clip
            .effects
            .iter()
            .filter(|effect| effect.effect_name == "montage.color_correction")
            .collect();
        assert_eq!(color_effects.len(), 1);
        assert!(!color_effects[0].metadata.contains_key("contrast"));
        assert_eq!(
            color_effects[0]
                .metadata
                .get("saturation")
                .and_then(|v| v.as_f64()),
            Some(0.8)
        );
    }

    #[test]
    fn apply_set_effect_preserves_different_effect_ids() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetEffect {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    effect: "montage.volume".into(),
                    params: serde_json::Map::from_iter([("value".into(), serde_json::json!(0.75))]),
                    rationale: None,
                },
                EdlOp::SetEffect {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    effect: "montage.color_correction".into(),
                    params: serde_json::Map::from_iter([(
                        "contrast".into(),
                        serde_json::json!(1.1),
                    )]),
                    rationale: None,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert!(
            clip.effects
                .iter()
                .any(|e| e.effect_name == "montage.volume")
        );
        assert!(
            clip.effects
                .iter()
                .any(|e| e.effect_name == "montage.color_correction")
        );
    }

    #[test]
    fn apply_set_parameter_animation_accepts_runtime_blur_effect_parameter() {
        let tl = timeline_with_three_clips();
        let animation = montage_proto::professional::ParameterAnimation {
            id: "anim-blur-radius".into(),
            target: montage_proto::professional::AnimationTarget::ClipParameter {
                clip_id: "clip-1".into(),
                parameter: "montage.blur.radius_px".into(),
            },
            keyframes: vec![
                montage_proto::professional::Keyframe::linear(0.0, 0.0),
                montage_proto::professional::Keyframe::linear(1.0, 12.0),
            ],
            pre_extrapolation: montage_proto::professional::ExtrapolationMode::Hold,
            post_extrapolation: montage_proto::professional::ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: None,
        };
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetParameterAnimation {
                animation: animation.clone(),
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();

        assert!(outcome.applied[0].description.contains("anim-blur-radius"));
        assert_eq!(
            new_tl
                .metadata
                .montage
                .as_ref()
                .unwrap()
                .parameter_animations,
            vec![animation]
        );
    }

    #[test]
    fn apply_set_parameter_animation_accepts_runtime_blur_effect_alias() {
        let tl = timeline_with_three_clips();
        let animation = montage_proto::professional::ParameterAnimation {
            id: "anim-blur-radius-alias".into(),
            target: montage_proto::professional::AnimationTarget::ClipParameter {
                clip_id: "clip-1".into(),
                parameter: "effects.montage.blur.params.radius_px".into(),
            },
            keyframes: vec![
                montage_proto::professional::Keyframe::linear(0.0, 0.0),
                montage_proto::professional::Keyframe::linear(1.0, 12.0),
            ],
            pre_extrapolation: montage_proto::professional::ExtrapolationMode::Hold,
            post_extrapolation: montage_proto::professional::ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: None,
        };
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetParameterAnimation {
                animation: animation.clone(),
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();

        assert!(
            outcome.applied[0]
                .description
                .contains("anim-blur-radius-alias")
        );
        assert_eq!(
            new_tl
                .metadata
                .montage
                .as_ref()
                .unwrap()
                .parameter_animations,
            vec![animation]
        );
    }

    #[test]
    fn apply_set_parameter_animation_accepts_runtime_shake_effect_parameter() {
        let tl = timeline_with_three_clips();
        let animation = montage_proto::professional::ParameterAnimation {
            id: "anim-shake-intensity".into(),
            target: montage_proto::professional::AnimationTarget::ClipParameter {
                clip_id: "clip-1".into(),
                parameter: "montage.shake.intensity_px".into(),
            },
            keyframes: vec![
                montage_proto::professional::Keyframe::linear(0.0, 0.0),
                montage_proto::professional::Keyframe::linear(1.0, 14.0),
            ],
            pre_extrapolation: montage_proto::professional::ExtrapolationMode::Hold,
            post_extrapolation: montage_proto::professional::ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: None,
        };
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetParameterAnimation {
                animation: animation.clone(),
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();

        assert!(
            outcome.applied[0]
                .description
                .contains("anim-shake-intensity")
        );
        assert_eq!(
            new_tl
                .metadata
                .montage
                .as_ref()
                .unwrap()
                .parameter_animations,
            vec![animation]
        );
    }

    #[test]
    fn apply_set_parameter_animation_accepts_runtime_warp_effect_parameter() {
        let tl = timeline_with_three_clips();
        let animation = montage_proto::professional::ParameterAnimation {
            id: "anim-warp-k1".into(),
            target: montage_proto::professional::AnimationTarget::ClipParameter {
                clip_id: "clip-1".into(),
                parameter: "montage.warp.k1".into(),
            },
            keyframes: vec![
                montage_proto::professional::Keyframe::linear(0.0, -0.15),
                montage_proto::professional::Keyframe::linear(1.0, 0.05),
            ],
            pre_extrapolation: montage_proto::professional::ExtrapolationMode::Hold,
            post_extrapolation: montage_proto::professional::ExtrapolationMode::Hold,
            motion_path: None,
            metadata_only: false,
            rationale: None,
        };
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetParameterAnimation {
                animation: animation.clone(),
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();

        assert!(outcome.applied[0].description.contains("anim-warp-k1"));
        assert_eq!(
            new_tl
                .metadata
                .montage
                .as_ref()
                .unwrap()
                .parameter_animations,
            vec![animation]
        );
    }

    #[test]
    fn apply_author_subject_reframe_from_track_stores_renderable_path() {
        let mut tl = timeline_with_three_clips();
        tl.metadata.montage.as_mut().unwrap().tracking_package = Some(TrackingPackage {
            tracks: vec![TrackSidecar {
                id: "speaker-track".into(),
                asset_id: "clip-1".into(),
                kind: ProfessionalTrackKind::Surface,
                coordinate_space: CoordinateSpace::Normalized,
                samples: vec![
                    TrackSample {
                        frame: 0,
                        points: vec![[0.20, 0.20], [0.40, 0.20], [0.40, 0.50], [0.20, 0.50]],
                        confidence: Some(0.91),
                    },
                    TrackSample {
                        frame: 30,
                        points: vec![[0.50, 0.30], [0.70, 0.30], [0.70, 0.60], [0.50, 0.60]],
                        confidence: Some(0.89),
                    },
                ],
                confidence: Some(0.9),
            }],
            ..TrackingPackage::default()
        });
        let env = EdlEnvelope {
            ops: vec![EdlOp::AuthorSubjectReframeFromTrack {
                track_id: "speaker-track".into(),
                clip_id: "clip-1".into(),
                aspect_ratio: "9:16".into(),
                source_width: 1920,
                source_height: 1080,
                target_width: 1080,
                target_height: 1920,
                frame_rate: 30.0,
                smoothing: ReframeSmoothing::Moderate,
                safe_area: Some("mobile".into()),
            }],
        };

        let (new_tl, outcome) = apply(&tl, &env, &AnchorContext::empty()).unwrap();

        assert!(
            outcome.applied[0]
                .description
                .contains("authored subject reframe path reframe-clip-1-9-16")
        );
        let package = new_tl
            .metadata
            .montage
            .as_ref()
            .unwrap()
            .tracking_package
            .as_ref()
            .unwrap();
        assert_eq!(package.reframe_paths.len(), 1);
        let path = &package.reframe_paths[0];
        assert_eq!(path.clip_id, "clip-1");
        assert_eq!(path.evidence_track_id.as_deref(), Some("speaker-track"));
        assert_eq!(path.keyframes.len(), 2);
        assert_eq!(path.keyframes[0].center, [0.3, 0.35]);
        assert_eq!(path.keyframes[1].center, [0.6, 0.45]);
        assert_eq!(path.keyframes[1].time_s, 1.0);
    }

    #[test]
    fn apply_set_effect_rejects_unknown_effect_before_writing() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.nope".into(),
                params: serde_json::Map::new(),
                rationale: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("unknown effect id")),
            "expected unknown effect validation error, got {err:?}"
        );
        let StackChild::Track(t) = &tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert!(clip.effects.is_empty());
    }

    #[test]
    fn apply_set_effect_rejects_out_of_range_params_before_writing() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.color_correction".into(),
                params: serde_json::Map::from_iter([("contrast".into(), serde_json::json!(4.0))]),
                rationale: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("contrast")),
            "expected range validation error, got {err:?}"
        );
        let StackChild::Track(t) = &tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert!(clip.effects.is_empty());
    }

    #[test]
    fn apply_set_speed_stamps_effect_with_factor() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetSpeed {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                factor: 2.0,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        assert_eq!(clip.effects[0].effect_name, "montage.speed");
        let f = clip.effects[0]
            .metadata
            .get("factor")
            .and_then(|v| v.as_f64())
            .unwrap();
        assert!((f - 2.0).abs() < 1e-9);
    }

    #[test]
    fn apply_set_time_remap_stamps_time_remap_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetTimeRemap {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                curve: vec![
                    serde_json::json!({"source_time_s": 0.0, "timeline_time_s": 0.0}),
                    serde_json::json!({"source_time_s": 2.0, "timeline_time_s": 3.0}),
                ],
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        assert_eq!(clip.effects[0].effect_name, "montage.time_remap");
        let curve = clip.effects[0]
            .metadata
            .get("curve")
            .and_then(|value| value.as_array())
            .unwrap();
        assert_eq!(curve.len(), 2);
        assert_eq!(curve[1]["source_time_s"], serde_json::json!(2.0));
        assert_eq!(curve[1]["timeline_time_s"], serde_json::json!(3.0));
    }

    #[test]
    fn apply_set_freeze_stamps_freeze_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetFreeze {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                freeze_at_source_s: 1.2,
                duration_s: 0.8,
                freeze_position: Some("at".into()),
                audio_behavior: Some("silence".into()),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        assert_eq!(clip.effects[0].effect_name, "montage.freeze");
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("freeze_at_source_s")
                .and_then(|value| value.as_f64()),
            Some(1.2)
        );
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("duration_s")
                .and_then(|value| value.as_f64()),
            Some(0.8)
        );
    }

    #[test]
    fn apply_set_speed_rejects_zero_factor() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetSpeed {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                factor: 0.0,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("> 0.0")),
            "expected validation error, got {err:?}",
        );
    }

    #[test]
    fn apply_set_color_correction_stamps_effect_with_fields() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetColorCorrection {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                exposure_ev: Some(0.25),
                contrast: Some(1.1),
                saturation: None,
                temperature: Some(-0.2),
                tint: None,
                shadows: None,
                highlights: Some(-0.3),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        assert_eq!(clip.effects[0].effect_name, "montage.color_correction");
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("exposure_ev")
                .and_then(|v| v.as_f64()),
            Some(0.25)
        );
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("temperature")
                .and_then(|v| v.as_f64()),
            Some(-0.2)
        );
        assert!(!clip.effects[0].metadata.contains_key("saturation"));
    }

    #[test]
    fn apply_set_color_correction_replaces_existing_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetColorCorrection {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    exposure_ev: Some(0.25),
                    contrast: None,
                    saturation: None,
                    temperature: None,
                    tint: None,
                    shadows: None,
                    highlights: None,
                },
                EdlOp::SetColorCorrection {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    exposure_ev: None,
                    contrast: Some(0.9),
                    saturation: Some(1.2),
                    temperature: None,
                    tint: None,
                    shadows: None,
                    highlights: None,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        let color_effects: Vec<_> = clip
            .effects
            .iter()
            .filter(|effect| effect.effect_name == "montage.color_correction")
            .collect();
        assert_eq!(color_effects.len(), 1);
        assert!(!color_effects[0].metadata.contains_key("exposure_ev"));
        assert_eq!(
            color_effects[0]
                .metadata
                .get("saturation")
                .and_then(|v| v.as_f64()),
            Some(1.2)
        );
    }

    #[test]
    fn apply_set_color_correction_rejects_invalid_field_value() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetColorCorrection {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                exposure_ev: Some(8.0),
                contrast: None,
                saturation: None,
                temperature: None,
                tint: None,
                shadows: None,
                highlights: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("exposure_ev")),
            "expected validation error, got {err:?}",
        );
    }

    #[test]
    fn apply_lut_stamps_effect_and_rejects_unsafe_paths() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::ApplyLut {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                lut_path: " luts/show-look.cube ".into(),
                interpolation: Some("TETRAHEDRAL".into()),
                strength: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(clip.effects.len(), 1);
        assert_eq!(clip.effects[0].effect_name, "montage.lut");
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("lut_path")
                .and_then(|v| v.as_str()),
            Some("luts/show-look.cube")
        );
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("interpolation")
                .and_then(|v| v.as_str()),
            Some("tetrahedral")
        );

        let err = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::ApplyLut {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    lut_path: "../secret.cube".into(),
                    interpolation: None,
                    strength: None,
                }],
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("project-relative")),
            "expected unsafe path validation error, got {err:?}",
        );

        let err = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::ApplyLut {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    lut_path: "luts/show-look.txt".into(),
                    interpolation: None,
                    strength: None,
                }],
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("must end in one of")),
            "expected LUT extension validation error, got {err:?}",
        );

        let err = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::ApplyLut {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    lut_path: "luts/show-look.cube".into(),
                    interpolation: Some("magic".into()),
                    strength: None,
                }],
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("interpolation")),
            "expected interpolation validation error, got {err:?}",
        );
    }

    #[test]
    fn apply_lut_records_strength_and_rejects_out_of_range() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::ApplyLut {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                lut_path: "luts/show-look.cube".into(),
                interpolation: None,
                strength: Some(0.6),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert_eq!(
            clip.effects[0]
                .metadata
                .get("strength")
                .and_then(|v| v.as_f64()),
            Some(0.6)
        );

        let err = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::ApplyLut {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    lut_path: "luts/show-look.cube".into(),
                    interpolation: None,
                    strength: Some(1.5),
                }],
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("strength")),
            "expected strength out-of-range error, got {err:?}",
        );
    }

    #[test]
    fn set_effect_lut_validates_cube_contents() {
        // P3: stamping `montage.lut` through `Set Effect` must run
        // the same content-parse as `Apply LUT`. Previously the
        // SetEffect path skipped `validate_lut_contents`, so a
        // malformed LUT could sit on a clip until render time.
        use crate::edl::op::EdlOp;
        let tl = timeline_with_three_clips();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("luts")).unwrap();
        std::fs::write(
            dir.path().join("luts/broken.cube"),
            "LUT_3D_SIZE 3\n0.0 0.0 0.0\n",
        )
        .unwrap();
        let ctx = AnchorContext::with_project_root(dir.path());

        let mut params = serde_json::Map::new();
        params.insert("lut_path".into(), serde_json::json!("luts/broken.cube"));
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.lut".into(),
                params,
                rationale: None,
            }],
        };
        let err = apply(&tl, &env, &ctx).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("not a valid .cube LUT")),
            "expected SetEffect path to surface .cube parse error, got {err:?}",
        );
    }

    #[test]
    fn color_pipeline_rejects_3d_cube_in_shaper_slot() {
        // P2: shaper_lut expects 1D — a 3D `.cube` must be rejected
        // at apply time rather than crashing FFmpeg's `lut1d`
        // filter mid-render.
        use crate::edl::op::EdlOp;
        let tl = timeline_with_three_clips();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("luts")).unwrap();
        // Minimal valid 2³ cube — succeeds as a 3D LUT.
        let cube_3d = "LUT_3D_SIZE 2\n\
                       0 0 0\n1 0 0\n0 1 0\n1 1 0\n\
                       0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        std::fs::write(dir.path().join("luts/shaper.cube"), cube_3d).unwrap();
        let ctx = AnchorContext::with_project_root(dir.path());

        let mut params = serde_json::Map::new();
        params.insert("clip_input_space".into(), serde_json::json!("arri_logc4"));
        params.insert("shaper_lut".into(), serde_json::json!("luts/shaper.cube"));
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.color_pipeline".into(),
                params,
                rationale: None,
            }],
        };
        let err = apply(&tl, &env, &ctx).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("shaper_lut")),
            "expected shaper_lut slot to reject 3D .cube, got {err:?}",
        );
    }

    #[test]
    fn color_pipeline_rejects_3dl_in_shaper_slot() {
        // P2: .3dl is always 3D, so it's rejected in the 1D slot
        // on extension alone (no file read needed).
        use crate::edl::op::EdlOp;
        let tl = timeline_with_three_clips();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("luts")).unwrap();
        // The file content doesn't matter — the extension check
        // catches this before parsing.
        std::fs::write(dir.path().join("luts/shaper.3dl"), b"").unwrap();
        let ctx = AnchorContext::with_project_root(dir.path());

        let mut params = serde_json::Map::new();
        params.insert("clip_input_space".into(), serde_json::json!("arri_logc4"));
        params.insert("shaper_lut".into(), serde_json::json!("luts/shaper.3dl"));
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.color_pipeline".into(),
                params,
                rationale: None,
            }],
        };
        let err = apply(&tl, &env, &ctx).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("3D-only format")),
            "expected .3dl to be rejected from shaper_lut on extension, got {err:?}",
        );
    }

    #[test]
    fn apply_lut_rejects_malformed_cube_at_apply_time() {
        let tl = timeline_with_three_clips();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("luts")).unwrap();
        // Declared size 3 but only one row of data — RowCountMismatch.
        std::fs::write(
            dir.path().join("luts/broken.cube"),
            "LUT_3D_SIZE 3\n0.0 0.0 0.0\n",
        )
        .unwrap();
        let ctx = AnchorContext::with_project_root(dir.path());
        let env = EdlEnvelope {
            ops: vec![EdlOp::ApplyLut {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                lut_path: "luts/broken.cube".into(),
                interpolation: None,
                strength: None,
            }],
        };
        let err = apply(&tl, &env, &ctx).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("not a valid .cube LUT")),
            "expected structured .cube parse error, got {err:?}",
        );
    }

    #[test]
    fn color_pipeline_rejects_unknown_color_space() {
        use crate::edl::op::EdlOp;
        let tl = timeline_with_three_clips();
        let mut params = serde_json::Map::new();
        params.insert(
            "clip_input_space".into(),
            serde_json::json!("Made_Up_Space"),
        );
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.color_pipeline".into(),
                params,
                rationale: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("not a known color space")),
            "expected unknown color space error, got {err:?}",
        );
    }

    #[test]
    fn color_pipeline_rejects_look_strength_without_look_lut() {
        use crate::edl::op::EdlOp;
        let tl = timeline_with_three_clips();
        let mut params = serde_json::Map::new();
        params.insert("clip_input_space".into(), serde_json::json!("rec709_g24"));
        params.insert("look_strength".into(), serde_json::json!(0.5));
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.color_pipeline".into(),
                params,
                rationale: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("look_strength")),
            "expected look_strength-without-look_lut error, got {err:?}",
        );
    }

    #[test]
    fn color_pipeline_accepts_mask_source_with_supported_extension() {
        use crate::edl::op::EdlOp;
        let tl = timeline_with_three_clips();
        let mut params = serde_json::Map::new();
        params.insert("clip_input_space".into(), serde_json::json!("rec709_g24"));
        params.insert("mask_source".into(), serde_json::json!("masks/face.png"));
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.color_pipeline".into(),
                params,
                rationale: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        let effect = clip
            .effects
            .iter()
            .find(|e| e.effect_name == "montage.color_pipeline")
            .expect("pipeline stamped");
        assert_eq!(
            effect.metadata.get("mask_source").and_then(|v| v.as_str()),
            Some("masks/face.png")
        );
    }

    #[test]
    fn color_pipeline_rejects_mask_source_with_disallowed_extension() {
        use crate::edl::op::EdlOp;
        let tl = timeline_with_three_clips();
        let mut params = serde_json::Map::new();
        params.insert("clip_input_space".into(), serde_json::json!("rec709_g24"));
        params.insert("mask_source".into(), serde_json::json!("masks/face.txt"));
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.color_pipeline".into(),
                params,
                rationale: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(err, ApplyError::Invalid { ref message, .. } if message.contains("mask_source must end in one of")),
            "expected mask-extension validation error, got {err:?}",
        );
    }

    #[test]
    fn color_pipeline_round_trip_with_log_clip_and_partial_strength() {
        use crate::edl::op::EdlOp;
        let tl = timeline_with_three_clips();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("luts")).unwrap();
        // Minimal valid 2³ cube so the parser is happy.
        let cube = "LUT_3D_SIZE 2\n\
                    0 0 0\n1 0 0\n0 1 0\n1 1 0\n\
                    0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        std::fs::write(dir.path().join("luts/look.cube"), cube).unwrap();
        let ctx = AnchorContext::with_project_root(dir.path());

        let mut params = serde_json::Map::new();
        params.insert("clip_input_space".into(), serde_json::json!("arri_logc4"));
        params.insert("lut_input_space".into(), serde_json::json!("rec709_g24"));
        params.insert("output_space".into(), serde_json::json!("rec709_g24"));
        params.insert("look_lut".into(), serde_json::json!("luts/look.cube"));
        params.insert("look_strength".into(), serde_json::json!(0.7));
        params.insert(
            "look_interpolation".into(),
            serde_json::json!("tetrahedral"),
        );

        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: "montage.color_pipeline".into(),
                params,
                rationale: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &ctx).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        let effect = clip
            .effects
            .iter()
            .find(|e| e.effect_name == "montage.color_pipeline")
            .expect("color_pipeline effect should be stamped");
        assert_eq!(
            effect
                .metadata
                .get("clip_input_space")
                .and_then(|v| v.as_str()),
            Some("arri_logc4")
        );
        assert_eq!(
            effect
                .metadata
                .get("look_strength")
                .and_then(|v| v.as_f64()),
            Some(0.7)
        );
        assert_eq!(
            effect
                .metadata
                .get("look_interpolation")
                .and_then(|v| v.as_str()),
            Some("tetrahedral")
        );
    }

    #[test]
    fn remove_lut_clears_only_lut_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::ApplyLut {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    lut_path: "luts/show-look.cube".into(),
                    interpolation: None,
                    strength: None,
                },
                EdlOp::SetVolume {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    value: 0.8,
                },
                EdlOp::RemoveLut {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        assert!(
            clip.effects
                .iter()
                .all(|e| e.effect_name != LUT_EFFECT_NAME),
            "LUT effect should have been removed: {:?}",
            clip.effects,
        );
        assert!(
            clip.effects
                .iter()
                .any(|e| e.effect_name == VOLUME_EFFECT_NAME),
            "non-LUT effects should remain: {:?}",
            clip.effects,
        );
    }

    #[test]
    fn set_effect_normalizes_lut_metadata() {
        let tl = timeline_with_three_clips();
        let mut params = serde_json::Map::new();
        params.insert(
            "lut_path".to_string(),
            serde_json::json!(" luts/generated-look.CUBE "),
        );
        params.insert("interpolation".to_string(), serde_json::json!("PRISM"));
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetEffect {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                effect: LUT_EFFECT_NAME.into(),
                params,
                rationale: Some("generated test look".into()),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        let effect = clip
            .effects
            .iter()
            .find(|effect| effect.effect_name == LUT_EFFECT_NAME)
            .expect("lut effect");
        assert_eq!(
            effect.metadata.get("lut_path").and_then(|v| v.as_str()),
            Some("luts/generated-look.CUBE")
        );
        assert_eq!(
            effect
                .metadata
                .get("interpolation")
                .and_then(|v| v.as_str()),
            Some("prism")
        );
        assert_eq!(
            effect.metadata.get("rationale").and_then(|v| v.as_str()),
            Some("generated test look")
        );
    }

    #[test]
    fn apply_insert_title_creates_titles_track_and_stamps_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTitle {
                start_s: 0.0,
                end_s: 3.0,
                text: "Welcome".into(),
                position: super::super::op::TitlePosition::Top,
                font_size: 72,
                color: "#FFAA00".into(),
                font_weight: super::super::op::TitleWeight::Bold,
                animation: super::super::op::TitleAnimation::FadeInOut,
                phases: None,
                font_family: None,
                font_path: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        // V1 (3 clips) + Titles track.
        assert_eq!(new_tl.tracks.children.len(), 2);
        let StackChild::Track(titles) = &new_tl.tracks.children[1] else {
            panic!("expected titles track at index 1")
        };
        assert_eq!(titles.name, "Titles");
        assert_eq!(
            titles
                .metadata
                .get("montage_track_role")
                .and_then(|v| v.as_str()),
            Some("titles"),
        );
        assert_eq!(titles.children.len(), 1);
        let TrackChild::Clip(title_clip) = &titles.children[0] else {
            panic!("expected title clip on titles track")
        };
        assert_eq!(title_clip.effects.len(), 1);
        let effect = &title_clip.effects[0];
        assert_eq!(effect.effect_name, "montage.title");
        assert_eq!(
            effect.metadata.get("text").and_then(|v| v.as_str()),
            Some("Welcome"),
        );
        assert_eq!(
            effect.metadata.get("position").and_then(|v| v.as_str()),
            Some("top"),
        );
        assert_eq!(
            effect.metadata.get("font_weight").and_then(|v| v.as_str()),
            Some("bold"),
        );
        assert_eq!(
            effect.metadata.get("animation").and_then(|v| v.as_str()),
            Some("fade_in_out"),
        );
        assert_eq!(
            effect.metadata.get("font_size").and_then(|v| v.as_u64()),
            Some(72),
        );
    }

    #[test]
    fn apply_insert_title_stamps_custom_font_metadata() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTitle {
                start_s: 0.0,
                end_s: 3.0,
                text: "Brand".into(),
                position: super::super::op::TitlePosition::Top,
                font_size: 72,
                color: "#FFFFFF".into(),
                font_weight: super::super::op::TitleWeight::Normal,
                animation: super::super::op::TitleAnimation::None,
                phases: None,
                font_family: Some("Brand Sans".into()),
                font_path: Some("/fonts/Brand.ttf".into()),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(titles) = &new_tl.tracks.children[1] else {
            panic!("expected titles track at index 1")
        };
        let TrackChild::Clip(title_clip) = &titles.children[0] else {
            panic!("expected title clip on titles track")
        };
        let effect = &title_clip.effects[0];
        assert_eq!(
            effect.metadata.get("font_path").and_then(|v| v.as_str()),
            Some("/fonts/Brand.ttf"),
        );
        assert_eq!(
            effect.metadata.get("font_family").and_then(|v| v.as_str()),
            Some("Brand Sans"),
        );
    }

    #[test]
    fn apply_insert_caption_creates_caption_overlay_node() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertCaption {
                start_s: 1.0,
                end_s: 2.5,
                text: "This changed everything".into(),
                position: super::super::op::TitlePosition::Bottom,
                font_size: 52,
                color: "#FFFFFF".into(),
                safe_area: "mobile".into(),
                word_timings: Vec::new(),
                style_json: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(titles) = &new_tl.tracks.children[1] else {
            panic!("expected titles track")
        };
        let TrackChild::Clip(caption_clip) = &titles.children[0] else {
            panic!("expected caption clip")
        };
        assert!(caption_clip.name.starts_with("caption-"));
        let effect = &caption_clip.effects[0];
        assert_eq!(effect.effect_name, "montage.title");
        assert_eq!(
            effect.metadata.get("role").and_then(|v| v.as_str()),
            Some("caption"),
        );
        assert_eq!(
            effect.metadata.get("safe_area").and_then(|v| v.as_str()),
            Some("mobile"),
        );
        assert_eq!(
            effect.metadata.get("position").and_then(|v| v.as_str()),
            Some("bottom"),
        );
    }

    #[test]
    fn apply_insert_caption_persists_word_timings_metadata() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertCaption {
                start_s: 1.0,
                end_s: 2.5,
                text: "This changed everything".into(),
                position: super::super::op::TitlePosition::Bottom,
                font_size: 52,
                color: "#FFFFFF".into(),
                safe_area: "mobile".into(),
                word_timings: vec![
                    super::super::op::CaptionWordTiming {
                        text: "This".into(),
                        start_s: 1.0,
                        end_s: 1.2,
                    },
                    super::super::op::CaptionWordTiming {
                        text: "changed".into(),
                        start_s: 1.24,
                        end_s: 1.5,
                    },
                ],
                style_json: None,
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(titles) = &new_tl.tracks.children[1] else {
            panic!("expected titles track")
        };
        let TrackChild::Clip(caption_clip) = &titles.children[0] else {
            panic!("expected caption clip")
        };
        let timings = caption_clip.effects[0]
            .metadata
            .get("word_timings")
            .and_then(|value| value.as_array())
            .expect("word_timings metadata");
        assert_eq!(timings.len(), 2);
        assert_eq!(
            timings[0].get("text").and_then(|value| value.as_str()),
            Some("This")
        );
    }

    #[test]
    fn insert_caption_stores_style_json_on_the_caption_effect() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertCaption {
                start_s: 1.0,
                end_s: 2.5,
                text: "AI changed everything".into(),
                position: super::super::op::TitlePosition::Bottom,
                font_size: 52,
                color: "#FFFFFF".into(),
                safe_area: "mobile".into(),
                word_timings: Vec::new(),
                style_json: Some(serde_json::json!({
                    "font_size": 52,
                    "weight": "bold",
                    "casing": "upper",
                    "primary_color": "#FFFFFF",
                    "highlight_color": "#FFD700",
                    "reveal": "active_word_pop",
                    "background": { "kind": "none" }
                })),
            }],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(titles) = &new_tl.tracks.children[1] else {
            panic!("expected titles track")
        };
        let TrackChild::Clip(caption_clip) = &titles.children[0] else {
            panic!("expected caption clip")
        };
        let style = caption_clip.effects[0]
            .metadata
            .get("caption_style")
            .expect("caption_style metadata must be present");
        assert_eq!(
            style.get("reveal").and_then(|v| v.as_str()),
            Some("active_word_pop"),
            "reveal must be stored as snake_case string"
        );
        assert_eq!(style.get("weight").and_then(|v| v.as_str()), Some("bold"),);
        assert_eq!(
            style.get("highlight_color").and_then(|v| v.as_str()),
            Some("#FFD700"),
        );
    }

    #[test]
    fn apply_output_loudness_and_package_metadata_live_on_timeline_graph() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetOutputFormat {
                    aspect_ratio: "9:16".into(),
                    platform: Some("youtube_shorts".into()),
                    safe_area: Some("mobile".into()),
                },
                EdlOp::SetLoudnessTarget {
                    integrated_lufs: -16.0,
                    true_peak_db: Some(-1.0),
                },
                EdlOp::SetPackageMetadata {
                    platform: Some("youtube_shorts".into()),
                    title: Some("Launch Risk".into()),
                    description: Some("A short clip about launch risk".into()),
                    tags: Some("launch,risk,clip".into()),
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let extra = &new_tl
            .metadata
            .montage
            .as_ref()
            .expect("montage metadata")
            .extra;
        assert_eq!(
            extra
                .get("output_format")
                .and_then(|v| v.get("aspect_ratio"))
                .and_then(|v| v.as_str()),
            Some("9:16"),
        );
        assert_eq!(
            extra
                .get("loudness_target")
                .and_then(|v| v.get("integrated_lufs"))
                .and_then(|v| v.as_f64()),
            Some(-16.0),
        );
        assert_eq!(
            extra
                .get("package_metadata")
                .and_then(|v| v.get("title"))
                .and_then(|v| v.as_str()),
            Some("Launch Risk"),
        );
    }

    #[test]
    fn apply_insert_title_reuses_existing_titles_track() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::InsertTitle {
                    start_s: 0.0,
                    end_s: 3.0,
                    text: "First".into(),
                    position: super::super::op::TitlePosition::Top,
                    font_size: 64,
                    color: "#FFFFFF".into(),
                    font_weight: super::super::op::TitleWeight::Normal,
                    animation: super::super::op::TitleAnimation::None,
                    phases: None,
                    font_family: None,
                    font_path: None,
                },
                EdlOp::InsertTitle {
                    start_s: 5.0,
                    end_s: 8.0,
                    text: "Second".into(),
                    position: super::super::op::TitlePosition::Bottom,
                    font_size: 48,
                    color: "#FFFFFF".into(),
                    font_weight: super::super::op::TitleWeight::Normal,
                    animation: super::super::op::TitleAnimation::None,
                    phases: None,
                    font_family: None,
                    font_path: None,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        // Still only 2 tracks (V1 + Titles), with two titles on the
        // Titles track in start_s order.
        assert_eq!(new_tl.tracks.children.len(), 2);
        let StackChild::Track(titles) = &new_tl.tracks.children[1] else {
            panic!()
        };
        assert_eq!(titles.children.len(), 2);
        let TrackChild::Clip(first) = &titles.children[0] else {
            panic!()
        };
        let TrackChild::Clip(second) = &titles.children[1] else {
            panic!()
        };
        assert_eq!(
            first.effects[0]
                .metadata
                .get("text")
                .and_then(|v| v.as_str()),
            Some("First"),
        );
        assert_eq!(
            second.effects[0]
                .metadata
                .get("text")
                .and_then(|v| v.as_str()),
            Some("Second"),
        );
    }

    #[test]
    fn apply_insert_title_rejects_invalid_window() {
        let tl = timeline_with_three_clips();
        // end_s <= start_s.
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTitle {
                start_s: 5.0,
                end_s: 3.0,
                text: "x".into(),
                position: super::super::op::TitlePosition::Center,
                font_size: 64,
                color: "#FFFFFF".into(),
                font_weight: super::super::op::TitleWeight::Normal,
                animation: super::super::op::TitleAnimation::None,
                phases: None,
                font_family: None,
                font_path: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("end_s must be"))
        );
    }

    #[test]
    fn apply_insert_title_rejects_empty_text() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::InsertTitle {
                start_s: 0.0,
                end_s: 3.0,
                text: "".into(),
                position: super::super::op::TitlePosition::Center,
                font_size: 64,
                color: "#FFFFFF".into(),
                font_weight: super::super::op::TitleWeight::Normal,
                animation: super::super::op::TitleAnimation::None,
                phases: None,
                font_family: None,
                font_path: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("non-empty"))
        );
    }

    #[test]
    fn apply_set_title_updates_only_specified_fields() {
        // Create a title, capture its uuid, then mutate its text.
        let tl = timeline_with_three_clips();
        let insert_env = EdlEnvelope {
            ops: vec![EdlOp::InsertTitle {
                start_s: 0.0,
                end_s: 3.0,
                text: "Original".into(),
                position: super::super::op::TitlePosition::Center,
                font_size: 64,
                color: "#FFFFFF".into(),
                font_weight: super::super::op::TitleWeight::Normal,
                animation: super::super::op::TitleAnimation::None,
                phases: None,
                font_family: None,
                font_path: None,
            }],
        };
        let (after_insert, _) = apply(&tl, &insert_env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(titles) = &after_insert.tracks.children[1] else {
            panic!()
        };
        let TrackChild::Clip(title_clip) = &titles.children[0] else {
            panic!()
        };
        let title_uuid = title_clip
            .metadata
            .montage
            .as_ref()
            .and_then(|m| m.extra.get("clip_uuid"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .expect("title clip should have a stamped clip_uuid");

        let set_env = EdlEnvelope {
            ops: vec![EdlOp::SetTitle {
                anchor: Anchor::ClipUuid {
                    uuid: title_uuid.clone(),
                },
                start_s: None,
                end_s: None,
                text: Some("Updated".into()),
                position: None,
                font_size: None,
                color: None,
                font_weight: Some(super::super::op::TitleWeight::Bold),
                animation: None,
                phases: None,
                font_family: None,
                font_path: None,
            }],
        };
        let (after_set, _) = apply(&after_insert, &set_env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(titles2) = &after_set.tracks.children[1] else {
            panic!()
        };
        let TrackChild::Clip(updated) = &titles2.children[0] else {
            panic!()
        };
        let effect = &updated.effects[0];
        // Updated fields.
        assert_eq!(
            effect.metadata.get("text").and_then(|v| v.as_str()),
            Some("Updated"),
        );
        assert_eq!(
            effect.metadata.get("font_weight").and_then(|v| v.as_str()),
            Some("bold"),
        );
        // Untouched fields preserve their original values.
        assert_eq!(
            effect.metadata.get("position").and_then(|v| v.as_str()),
            Some("center"),
        );
        assert_eq!(
            effect.metadata.get("font_size").and_then(|v| v.as_u64()),
            Some(64),
        );
    }

    #[test]
    fn apply_set_title_family_change_clears_stale_font_path() {
        // Insert a title with an explicit font_path, then SetTitle with only a
        // font_family. Render resolves font_path before font_family, so the
        // stale path must be cleared for the family switch to take effect.
        let tl = timeline_with_three_clips();
        let insert_env = EdlEnvelope {
            ops: vec![EdlOp::InsertTitle {
                start_s: 0.0,
                end_s: 3.0,
                text: "Original".into(),
                position: super::super::op::TitlePosition::Center,
                font_size: 64,
                color: "#FFFFFF".into(),
                font_weight: super::super::op::TitleWeight::Normal,
                animation: super::super::op::TitleAnimation::None,
                phases: None,
                font_family: None,
                font_path: Some("fonts/Custom.ttf".into()),
            }],
        };
        let (after_insert, _) = apply(&tl, &insert_env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(titles) = &after_insert.tracks.children[1] else {
            panic!()
        };
        let TrackChild::Clip(title_clip) = &titles.children[0] else {
            panic!()
        };
        let title_uuid = title_clip
            .metadata
            .montage
            .as_ref()
            .and_then(|m| m.extra.get("clip_uuid"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .expect("title clip should have a stamped clip_uuid");

        let set_env = EdlEnvelope {
            ops: vec![EdlOp::SetTitle {
                anchor: Anchor::ClipUuid { uuid: title_uuid },
                start_s: None,
                end_s: None,
                text: None,
                position: None,
                font_size: None,
                color: None,
                font_weight: None,
                animation: None,
                phases: None,
                font_family: Some("Helvetica".into()),
                font_path: None,
            }],
        };
        let (after_set, _) = apply(&after_insert, &set_env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(titles2) = &after_set.tracks.children[1] else {
            panic!()
        };
        let TrackChild::Clip(updated) = &titles2.children[0] else {
            panic!()
        };
        let effect = &updated.effects[0];
        assert_eq!(
            effect.metadata.get("font_family").and_then(|v| v.as_str()),
            Some("Helvetica"),
        );
        assert!(
            !effect.metadata.contains_key("font_path"),
            "stale font_path must be cleared when only font_family is set",
        );
    }

    #[test]
    fn apply_set_title_rejects_clip_without_title_effect() {
        // Anchor a clip-on-V1 (no montage.title effect) → error.
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetTitle {
                anchor: Anchor::TranscriptSnippet {
                    text: "bravo snippet".into(),
                },
                start_s: None,
                end_s: None,
                text: Some("nope".into()),
                position: None,
                font_size: None,
                color: None,
                font_weight: None,
                animation: None,
                phases: None,
                font_family: None,
                font_path: None,
            }],
        };
        let err = apply(&tl, &env, &AnchorContext::empty()).unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("montage.title"))
        );
    }

    #[test]
    fn apply_set_broadcast_overlay_stores_timeline_metadata() {
        let tl = timeline_with_three_clips();
        let mut config = BroadcastOverlayConfig {
            episode_title: "Ben Adams".into(),
            show_name: "Technologia Talks".into(),
            sponsors: vec!["LEARN-X".into(), "Throwly".into()],
            ..BroadcastOverlayConfig::default()
        };
        config.host_a.name = "Tadiwa Mbuwayesango".into();
        config.host_a.title = "Co-Host".into();
        config.host_a.photo_path = Some("branding/tadiwa.jpg".into());
        config.topics.push(BroadcastTimedEntry {
            time_seconds: 42.0,
            text: "Custom drones".into(),
        });

        let env = EdlEnvelope {
            ops: vec![EdlOp::SetBroadcastOverlay {
                config: config.clone(),
            }],
        };
        let (new_tl, applied) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(applied.applied.len(), 1);
        let stored = new_tl
            .metadata
            .montage
            .as_ref()
            .and_then(|m| m.broadcast_overlay.as_ref())
            .expect("overlay config should be stored");
        assert_eq!(stored.episode_title, "Ben Adams");
        assert_eq!(
            stored.host_a.photo_path.as_deref(),
            Some("branding/tadiwa.jpg")
        );
        assert_eq!(stored.topics[0].text, "Custom drones");
    }

    #[test]
    fn apply_set_broadcast_overlay_replaces_existing_config() {
        let tl = timeline_with_three_clips();
        let first = BroadcastOverlayConfig {
            episode_title: "First".into(),
            ..BroadcastOverlayConfig::default()
        };
        let second = BroadcastOverlayConfig {
            episode_title: "Second".into(),
            topics: vec![BroadcastTimedEntry {
                time_seconds: 5.0,
                text: "Replacement".into(),
            }],
            ..BroadcastOverlayConfig::default()
        };
        let (after_first, _) = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::SetBroadcastOverlay { config: first }],
            },
            &AnchorContext::empty(),
        )
        .unwrap();
        let (after_second, _) = apply(
            &after_first,
            &EdlEnvelope {
                ops: vec![EdlOp::SetBroadcastOverlay {
                    config: second.clone(),
                }],
            },
            &AnchorContext::empty(),
        )
        .unwrap();
        let stored = after_second
            .metadata
            .montage
            .as_ref()
            .and_then(|m| m.broadcast_overlay.as_ref())
            .unwrap();
        assert_eq!(stored.episode_title, "Second");
        assert_eq!(stored.topics.len(), 1);
    }

    #[test]
    fn apply_set_broadcast_overlay_rejects_unsafe_asset_paths() {
        let tl = timeline_with_three_clips();
        let mut config = BroadcastOverlayConfig::default();
        config.host_a.photo_path = Some("../secret.jpg".into());
        let err = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::SetBroadcastOverlay { config }],
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("project-relative"))
        );
    }

    #[test]
    fn apply_set_brand_kit_stores_timeline_metadata() {
        use montage_proto::montage_meta::SocialHandle;
        let tl = timeline_with_three_clips();
        let kit = BrandKit {
            logo_path: Some("branding/logo.png".into()),
            font_primary_path: Some("branding/primary.ttf".into()),
            palette: vec!["#FFD700".into(), "#00CED1".into()],
            music_path: Some("branding/theme.wav".into()),
            social_handles: vec![SocialHandle {
                platform: "youtube".into(),
                handle: "@montage".into(),
                url: Some("https://youtube.com/@montage".into()),
            }],
            ..BrandKit::default()
        };
        let env = EdlEnvelope {
            ops: vec![EdlOp::SetBrandKit { kit: kit.clone() }],
        };
        let (new_tl, applied) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        assert_eq!(applied.applied.len(), 1);
        let stored = new_tl
            .metadata
            .montage
            .as_ref()
            .and_then(|m| m.brand_kit.as_ref())
            .expect("brand kit should be stored");
        assert_eq!(stored.logo_path.as_deref(), Some("branding/logo.png"));
        assert_eq!(stored.palette, vec!["#FFD700", "#00CED1"]);
        assert_eq!(stored.social_handles[0].handle, "@montage");
    }

    #[test]
    fn apply_set_brand_kit_rejects_unsafe_paths() {
        let tl = timeline_with_three_clips();
        let kit = BrandKit {
            logo_path: Some("../secret.png".into()),
            ..BrandKit::default()
        };
        let err = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::SetBrandKit { kit }],
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("project-relative"))
        );
    }

    #[test]
    fn apply_set_brand_kit_rejects_malformed_palette_color() {
        let tl = timeline_with_three_clips();
        // A palette value carrying filter-option characters must be rejected so
        // it can't be interpolated raw into a drawbox/drawtext `color=` option.
        let kit = BrandKit {
            palette: vec!["#FFD700".into(), "red:foo@1".into()],
            ..BrandKit::default()
        };
        let err = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::SetBrandKit { kit }],
            },
            &AnchorContext::empty(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, ApplyError::Invalid { message, .. } if message.contains("palette[1]")),
            "expected palette[1] rejection, got {err:?}"
        );
    }

    #[test]
    fn broadcast_overlay_reads_brand_logo_from_kit_when_unset() {
        let tl = timeline_with_three_clips();
        // Store the brand kit first, then an overlay with no local logo.
        let kit = BrandKit {
            logo_path: Some("branding/logo.png".into()),
            palette: vec!["#112233".into()],
            ..BrandKit::default()
        };
        let overlay = BroadcastOverlayConfig {
            episode_title: "Episode 1".into(),
            ..BroadcastOverlayConfig::default()
        };
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetBrandKit { kit },
                EdlOp::SetBroadcastOverlay { config: overlay },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let stored = new_tl
            .metadata
            .montage
            .as_ref()
            .and_then(|m| m.broadcast_overlay.as_ref())
            .expect("overlay should be stored");
        // Logo and primary accent fell back to the brand kit.
        assert_eq!(stored.brand_logo_path.as_deref(), Some("branding/logo.png"));
        assert_eq!(stored.style.gold_hex, "#112233");
    }

    #[test]
    fn broadcast_overlay_keeps_local_logo_over_brand_kit() {
        let tl = timeline_with_three_clips();
        let kit = BrandKit {
            logo_path: Some("branding/kit_logo.png".into()),
            ..BrandKit::default()
        };
        let overlay = BroadcastOverlayConfig {
            brand_logo_path: Some("branding/local_logo.png".into()),
            ..BroadcastOverlayConfig::default()
        };
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetBrandKit { kit },
                EdlOp::SetBroadcastOverlay { config: overlay },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let stored = new_tl
            .metadata
            .montage
            .as_ref()
            .and_then(|m| m.broadcast_overlay.as_ref())
            .unwrap();
        assert_eq!(
            stored.brand_logo_path.as_deref(),
            Some("branding/local_logo.png")
        );
    }

    #[test]
    fn delete_primary_clip_preserves_broadcast_overlay_topics_and_chapters() {
        let mut tl = timeline_with_three_clips();
        let mut config = BroadcastOverlayConfig::default();
        config.topics = vec![
            BroadcastTimedEntry {
                time_seconds: 4.0,
                text: "Before cut".into(),
            },
            BroadcastTimedEntry {
                time_seconds: 12.0,
                text: "After cut".into(),
            },
        ];
        config.chapters = vec![BroadcastTimedEntry {
            time_seconds: 14.0,
            text: "Shifted chapter".into(),
        }];
        tl.metadata.montage = Some(MontageTimelineMetadata {
            broadcast_overlay: Some(config),
            ..MontageTimelineMetadata::default()
        });

        let (new_tl, _) = apply(
            &tl,
            &EdlEnvelope {
                ops: vec![EdlOp::DeleteClip {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                }],
            },
            &AnchorContext::empty(),
        )
        .unwrap();

        let overlay = new_tl
            .metadata
            .montage
            .as_ref()
            .and_then(|m| m.broadcast_overlay.as_ref())
            .unwrap();
        assert_eq!(overlay.topics[0].time_seconds, 4.0);
        assert_eq!(overlay.topics[1].time_seconds, 12.0);
        assert_eq!(overlay.chapters[0].time_seconds, 14.0);
    }

    #[test]
    fn apply_set_volume_then_set_speed_coexist_on_same_clip() {
        let tl = timeline_with_three_clips();
        let env = EdlEnvelope {
            ops: vec![
                EdlOp::SetVolume {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    value: 0.5,
                },
                EdlOp::SetSpeed {
                    anchor: Anchor::TranscriptSnippet {
                        text: "bravo snippet".into(),
                    },
                    factor: 1.5,
                },
            ],
        };
        let (new_tl, _) = apply(&tl, &env, &AnchorContext::empty()).unwrap();
        let StackChild::Track(t) = &new_tl.tracks.children[0] else {
            panic!()
        };
        let TrackChild::Clip(clip) = &t.children[1] else {
            panic!()
        };
        // Different effect names → both effects survive.
        assert_eq!(clip.effects.len(), 2);
        let names: Vec<&str> = clip
            .effects
            .iter()
            .map(|e| e.effect_name.as_str())
            .collect();
        assert!(names.contains(&"montage.volume"));
        assert!(names.contains(&"montage.speed"));
    }
}
