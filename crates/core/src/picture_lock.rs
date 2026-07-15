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

/// Read lock state; missing file means unlocked.
pub fn read_picture_lock(project_root: &Path) -> PictureLock {
    let path = picture_lock_path(project_root);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| PictureLock {
            locked: false,
            reason: None,
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
}
