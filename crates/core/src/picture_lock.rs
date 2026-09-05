//! Picture-lock gate: after picture pass, block picture-mutating EDL ops.
//!
//! Real post houses lock picture before sound/color/graphics. Montage
//! stores lock state at `<project>/.montage/picture_lock.json`. When
//! locked, [`check_envelope`] rejects ops that move, trim, split, or
//! re-time picture so later departments cannot silently reopen the cut.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::edl::{EdlEnvelope, EdlOp};

/// Project-relative picture-lock file.
pub const PICTURE_LOCK_REL: &str = ".montage/picture_lock.json";

/// Persisted picture-lock state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PictureLock {
    /// Whether picture is locked.
    pub locked: bool,
    /// Optional human reason (profile, gate results, user request).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When the lock was set or cleared.
    pub updated_at: DateTime<Utc>,
}

/// Path to the picture-lock file.
pub fn picture_lock_path(project_root: &Path) -> PathBuf {
    project_root.join(PICTURE_LOCK_REL)
}

/// Read lock state; missing file means unlocked. A file that exists
/// but fails to parse fails CLOSED (treated as locked) rather than
/// silently reopening picture — a corrupt lock file should never be
/// mistaken for an unlocked project.
pub fn read_picture_lock(project_root: &Path) -> PictureLock {
    let path = picture_lock_path(project_root);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| PictureLock {
            locked: true,
            reason: Some(
                "picture_lock.json is corrupt — treating as locked; fix or delete the file"
                    .to_string(),
            ),
            updated_at: Utc::now(),
        }),
        Err(_) => PictureLock {
            locked: false,
            reason: None,
            updated_at: Utc::now(),
        },
    }
}

/// Set or clear picture lock.
pub fn set_picture_lock(
    project_root: &Path,
    locked: bool,
    reason: Option<String>,
) -> Result<PictureLock, String> {
    let path = picture_lock_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("picture_lock: create {}: {e}", parent.display()))?;
    }
    let state = PictureLock {
        locked,
        reason,
        updated_at: Utc::now(),
    };
    let body =
        serde_json::to_vec_pretty(&state).map_err(|e| format!("picture_lock: serialize: {e}"))?;
    std::fs::write(&path, body)
        .map_err(|e| format!("picture_lock: write {}: {e}", path.display()))?;
    Ok(state)
}

/// Whether this op mutates picture structure (forbidden under lock).
pub fn op_mutates_picture(op: &EdlOp) -> bool {
    match op {
        // Sound / loudness / levels — allowed after lock.
        EdlOp::SetVolume { .. }
        | EdlOp::MuteClip { .. }
        | EdlOp::RemoveAudio { .. }
        | EdlOp::SetAudioFade { .. }
        | EdlOp::SetTrackAudio { .. }
        | EdlOp::SetDucking { .. }
        | EdlOp::SetClipAudioFx { .. }
        | EdlOp::SetTrackAudioFx { .. }
        | EdlOp::SetLoudnessTarget { .. }
        | EdlOp::SetAudioFinishing { .. }
        // Color finishing — allowed after lock.
        | EdlOp::SetColorCorrection { .. }
        | EdlOp::ApplyLut { .. }
        | EdlOp::RemoveLut { .. }
        | EdlOp::SetColorFinishing { .. }
        // Package / metadata / markers / titles / captions / graphics overlays.
        | EdlOp::InsertTitle { .. }
        | EdlOp::InsertRichTitle { .. }
        | EdlOp::SetTitle { .. }
        | EdlOp::InsertCaption { .. }
        | EdlOp::InsertAnnotation { .. }
        | EdlOp::InstantiateMotionTemplate { .. }
        | EdlOp::SetMotionTemplate { .. }
        | EdlOp::SetMotionScene { .. }
        | EdlOp::SetOutputFormat { .. }
        | EdlOp::SetPackageMetadata { .. }
        | EdlOp::SetBroadcastOverlay { .. }
        | EdlOp::SetAssetCatalog { .. }
        | EdlOp::SetSourceReview { .. }
        | EdlOp::AddProposalPackage { .. }
        | EdlOp::SetParameterAnimation { .. }
        | EdlOp::AttachComposition { .. }
        | EdlOp::SetTrackingPackage { .. }
        | EdlOp::AuthorSubjectReframeFromTrack { .. }
        | EdlOp::SelectDeliveryProfile { .. }
        | EdlOp::AddPreflightReport { .. }
        | EdlOp::SetWorkflowLens { .. }
        | EdlOp::SetPipelineReadiness { .. }
        | EdlOp::SetBrandKit { .. }
        | EdlOp::SetSyncGroup { .. }
        | EdlOp::SetEffect { .. }
        | EdlOp::InsertTrack { .. }
        | EdlOp::DeleteTrack { .. } => false,
        // Markers recorded through the professional edit contract are
        // metadata-only and allowed under lock.
        EdlOp::ProfessionalTimelineEdit {
            edit:
                crate::edl::op::ProfessionalTimelineEdit::AddMarker { .. }
                | crate::edl::op::ProfessionalTimelineEdit::UpdateMarker { .. }
                | crate::edl::op::ProfessionalTimelineEdit::DeleteMarker { .. },
        } => false,
        // Everything else restructures or retimes picture.
        _ => true,
    }
}

/// Fail if picture is locked and the envelope contains picture ops.
pub fn check_envelope(project_root: &Path, envelope: &EdlEnvelope) -> Result<(), String> {
    let lock = read_picture_lock(project_root);
    if !lock.locked {
        return Ok(());
    }
    let blocked: Vec<usize> = envelope
        .ops
        .iter()
        .enumerate()
        .filter_map(|(i, op)| op_mutates_picture(op).then_some(i))
        .collect();
    if blocked.is_empty() {
        return Ok(());
    }
    Err(format!(
        "picture is locked{} — refusing picture-mutating EDL op(s) at index {:?}. \
         Sound/color/graphics ops that do not retime or restructure the cut may \
         still apply. Unlock only via `set_picture_lock` with locked=false after \
         an explicit user request to reopen picture.",
        lock.reason
            .as_deref()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default(),
        blocked
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edl::parse as edl_parse;

    #[test]
    fn allows_when_unlocked() {
        let dir = tempfile::tempdir().unwrap();
        let env = edl_parse(
            "*** Begin EDL\n*** Split Clip\n@@ anchor: clip_uuid=c0\n+ at_s: 1.0\n*** End EDL\n",
        )
        .unwrap();
        assert!(check_envelope(dir.path(), &env).is_ok());
    }

    #[test]
    fn blocks_picture_ops_when_locked() {
        let dir = tempfile::tempdir().unwrap();
        set_picture_lock(dir.path(), true, Some("gates passed".into())).unwrap();
        let env = edl_parse(
            "*** Begin EDL\n*** Split Clip\n@@ anchor: clip_uuid=c0\n+ at_s: 1.0\n*** End EDL\n",
        )
        .unwrap();
        let err = check_envelope(dir.path(), &env).unwrap_err();
        assert!(err.contains("picture is locked"), "{err}");
    }

    #[test]
    fn read_missing_file_is_unlocked() {
        let dir = tempfile::tempdir().unwrap();
        let lock = read_picture_lock(dir.path());
        assert!(!lock.locked);
        assert!(lock.reason.is_none());
    }

    #[test]
    fn read_corrupt_file_fails_closed_as_locked() {
        let dir = tempfile::tempdir().unwrap();
        let path = picture_lock_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not valid json{{{").unwrap();

        let lock = read_picture_lock(dir.path());
        assert!(lock.locked, "corrupt lock file must fail closed");
        let reason = lock.reason.as_deref().unwrap_or_default();
        assert!(reason.contains("corrupt"), "{reason}");
        assert!(reason.contains("treating as locked"), "{reason}");
    }

    #[test]
    fn read_valid_locked_file() {
        let dir = tempfile::tempdir().unwrap();
        set_picture_lock(dir.path(), true, Some("gates passed".into())).unwrap();
        let lock = read_picture_lock(dir.path());
        assert!(lock.locked);
        assert_eq!(lock.reason.as_deref(), Some("gates passed"));
    }

    #[test]
    fn read_valid_unlocked_file() {
        let dir = tempfile::tempdir().unwrap();
        set_picture_lock(dir.path(), true, None).unwrap();
        set_picture_lock(dir.path(), false, None).unwrap();
        let lock = read_picture_lock(dir.path());
        assert!(!lock.locked);
    }

    /// Table test over `op_mutates_picture`: for every category the
    /// module explicitly allowlists as post-lock-safe, a representative
    /// op must return `false`; representative picture-mutating ops must
    /// return `true`. Does not change the match — only asserts its
    /// existing classification, so a future accidental reclassification
    /// of any of these ops trips this test.
    #[test]
    fn op_mutates_picture_table() {
        use crate::edl::op::{Anchor, EdlOp, ProfessionalTimelineEdit};
        use montage_proto::montage_meta::BrandKit;
        use montage_proto::professional::{
            AssetCatalog, AudioFinishingState, ColorFinishingState, DeliveryProfile,
            PipelineReadinessReport, PreflightReport, ProposalPackage, TrackingPackage,
            WorkflowLens,
        };

        fn anchor(uuid: &str) -> Anchor {
            Anchor::ClipUuid {
                uuid: uuid.to_string(),
            }
        }

        // --- Allowlisted as safe post-lock (must return false) ---
        let safe: Vec<(&str, EdlOp)> = vec![
            (
                "SetVolume",
                EdlOp::SetVolume {
                    anchor: anchor("c0"),
                    value: 1.0,
                },
            ),
            (
                "MuteClip",
                EdlOp::MuteClip {
                    anchor: anchor("c0"),
                    muted: true,
                },
            ),
            (
                "RemoveAudio",
                EdlOp::RemoveAudio {
                    anchor: anchor("c0"),
                    start_s: Some(0.0),
                    end_s: Some(1.0),
                    clear: false,
                },
            ),
            (
                "SetAudioFade",
                EdlOp::SetAudioFade {
                    anchor: anchor("c0"),
                    fade_in_s: Some(0.5),
                    fade_out_s: None,
                },
            ),
            (
                "SetLoudnessTarget",
                EdlOp::SetLoudnessTarget {
                    integrated_lufs: -14.0,
                    true_peak_db: Some(-1.0),
                },
            ),
            (
                "SetAudioFinishing",
                EdlOp::SetAudioFinishing {
                    state: AudioFinishingState::default(),
                },
            ),
            (
                "SetColorCorrection",
                EdlOp::SetColorCorrection {
                    anchor: anchor("c0"),
                    exposure_ev: Some(0.5),
                    contrast: None,
                    saturation: None,
                    temperature: None,
                    tint: None,
                    shadows: None,
                    highlights: None,
                },
            ),
            (
                "RemoveLut",
                EdlOp::RemoveLut {
                    anchor: anchor("c0"),
                },
            ),
            (
                "SetColorFinishing",
                EdlOp::SetColorFinishing {
                    state: ColorFinishingState::default(),
                },
            ),
            (
                "InsertTitle",
                EdlOp::InsertTitle {
                    start_s: 0.0,
                    end_s: 1.0,
                    text: "hello".into(),
                    position: crate::edl::op::TitlePosition::Bottom,
                    font_size: 48,
                    color: "#FFFFFF".into(),
                    font_weight: crate::edl::op::TitleWeight::Normal,
                    animation: crate::edl::op::TitleAnimation::None,
                    phases: None,
                    font_family: None,
                    font_path: None,
                },
            ),
            (
                "InsertCaption",
                EdlOp::InsertCaption {
                    start_s: 0.0,
                    end_s: 1.0,
                    text: "caption".into(),
                    position: crate::edl::op::TitlePosition::Bottom,
                    font_size: 32,
                    color: "#FFFFFF".into(),
                    safe_area: "standard".into(),
                    word_timings: Vec::new(),
                    style_json: None,
                },
            ),
            (
                "InsertAnnotation",
                EdlOp::InsertAnnotation {
                    start_s: 0.0,
                    end_s: 1.0,
                    kind: crate::edl::op::AnnotationKind::Rectangle,
                    x: 0.1,
                    y: 0.1,
                    width: 0.2,
                    height: 0.2,
                    color: "#FFCC00".into(),
                    stroke_width: 2,
                    label: None,
                },
            ),
            (
                "SetOutputFormat",
                EdlOp::SetOutputFormat {
                    aspect_ratio: "16:9".into(),
                    platform: None,
                    safe_area: None,
                    width: None,
                    height: None,
                    frame_rate: None,
                },
            ),
            (
                "SetPackageMetadata",
                EdlOp::SetPackageMetadata {
                    platform: None,
                    title: None,
                    description: None,
                    tags: None,
                },
            ),
            (
                "SetAssetCatalog",
                EdlOp::SetAssetCatalog {
                    catalog: AssetCatalog::default(),
                },
            ),
            (
                "SetSourceReview",
                EdlOp::SetSourceReview {
                    selects: Vec::new(),
                    stringouts: Vec::new(),
                },
            ),
            (
                "AddProposalPackage",
                EdlOp::AddProposalPackage {
                    package: ProposalPackage::default(),
                },
            ),
            (
                "SetTrackingPackage",
                EdlOp::SetTrackingPackage {
                    package: TrackingPackage::default(),
                },
            ),
            (
                "SelectDeliveryProfile",
                EdlOp::SelectDeliveryProfile {
                    profile: DeliveryProfile::youtube_1080p(),
                },
            ),
            (
                "AddPreflightReport",
                EdlOp::AddPreflightReport {
                    report: PreflightReport::default(),
                },
            ),
            (
                "SetWorkflowLens",
                EdlOp::SetWorkflowLens {
                    lens: WorkflowLens::Preflight,
                },
            ),
            (
                "SetPipelineReadiness",
                EdlOp::SetPipelineReadiness {
                    readiness: PipelineReadinessReport::default(),
                    planner_passes: Vec::new(),
                },
            ),
            (
                "SetBrandKit",
                EdlOp::SetBrandKit {
                    kit: BrandKit::default(),
                },
            ),
            (
                "InsertTrack",
                EdlOp::InsertTrack {
                    name: "V9".into(),
                    kind: crate::edl::op::InsertTrackKind::Video,
                    at_position: None,
                },
            ),
            (
                "DeleteTrack",
                EdlOp::DeleteTrack {
                    name: "V9".into(),
                    force: false,
                },
            ),
            (
                "ProfessionalTimelineEdit::AddMarker",
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::AddMarker {
                        at_s: 1.0,
                        label: "beat".into(),
                        category: None,
                    },
                },
            ),
            (
                "ProfessionalTimelineEdit::UpdateMarker",
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::UpdateMarker {
                        selector: crate::edl::op::MarkerSelector::default(),
                        label: Some("beat2".into()),
                        category: None,
                        note: None,
                        at_s: None,
                        duration_s: None,
                    },
                },
            ),
            (
                "ProfessionalTimelineEdit::DeleteMarker",
                EdlOp::ProfessionalTimelineEdit {
                    edit: ProfessionalTimelineEdit::DeleteMarker {
                        selector: crate::edl::op::MarkerSelector::default(),
                    },
                },
            ),
        ];
        for (name, op) in &safe {
            assert!(
                !op_mutates_picture(op),
                "{name} is allowlisted as post-lock-safe but op_mutates_picture returned true"
            );
        }

        // --- Picture-mutating (must return true) ---
        let mutating: Vec<(&str, EdlOp)> = vec![
            (
                "SplitClip",
                EdlOp::SplitClip {
                    anchor: anchor("c0"),
                    at_s: 1.0,
                    snap: None,
                },
            ),
            (
                "TrimClip",
                EdlOp::TrimClip {
                    anchor: anchor("c0"),
                    start: Some(0.5),
                    end: None,
                },
            ),
            (
                "UntrimClip",
                EdlOp::UntrimClip {
                    anchor: anchor("c0"),
                    start: Some(0.0),
                    end: None,
                },
            ),
            (
                "MoveClip",
                EdlOp::MoveClip {
                    anchor: anchor("c0"),
                    to_position: 2,
                    at_s: None,
                    snap: None,
                },
            ),
            (
                "InsertClip",
                EdlOp::InsertClip {
                    asset: "raw/a.mp4".into(),
                    track: "V1".into(),
                    track_kind: None,
                    at_position: None,
                    at_s: None,
                    start: Some(0.0),
                    end: Some(1.0),
                    name: None,
                    link_group_id: None,
                    snap: None,
                },
            ),
            (
                "DeleteClip",
                EdlOp::DeleteClip {
                    anchor: anchor("c0"),
                },
            ),
            (
                "RippleDelete",
                EdlOp::RippleDelete {
                    anchor: anchor("c0"),
                },
            ),
            (
                "RippleMove",
                EdlOp::RippleMove {
                    anchor: anchor("c0"),
                    delta_s: 1.0,
                    snap: None,
                },
            ),
        ];
        for (name, op) in &mutating {
            assert!(
                op_mutates_picture(op),
                "{name} restructures/retimes picture but op_mutates_picture returned false"
            );
        }
    }
}
