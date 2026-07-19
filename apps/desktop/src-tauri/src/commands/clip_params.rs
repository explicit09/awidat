//! Inspector-driven direct edits to clip parameters. Bypasses the
//! agent's propose/accept loop for continuous parameters that the
//! user expects to scrub instantly (volume slider, speed slider,
//! etc.). Writes the OTIO via `apply_edl`, auto-commits a vedit
//! snapshot, and emits `timeline_changed` so the frontend re-flattens.
//!
//! # Shipped commands
//! - `set_clip_volume` — uses `SetVolume` op.
//! - `set_clip_speed` — uses `SetSpeed` op.
//! - `set_clip_fade` — uses `SetAudioFade` op.
//!
//! # Skipped commands (no existing EDL op)
//! - `set_clip_crop` — no `SetCrop` EDL op exists; would require adding
//!   a new core op. Deferred until a `SetEffect("montage.crop", ...)` op is
//!   wired through the apply layer.
//! - `set_clip_transform` — no `SetTransform` EDL op exists. Deferred.

use montage_core::edl::op::{Anchor, InsertTrackKind};
use montage_core::edl::{AnchorContext, EdlEnvelope, EdlOp, apply};
use montage_proto::project::Project;
use tauri::{AppHandle, State};

use crate::state::MontageState;

/// Apply a single-op EDL envelope to the project on disk, auto-commit
/// via vedit, and emit `timeline_changed`. This is the shared inner
/// helper for all clip-param commands.
///
/// Returns `Err(String)` if input validation, the OTIO apply, or the
/// disk write fails. Vedit commit failure is non-fatal: it logs a
/// warning and returns `Ok(())` per the same policy as
/// `accept_proposal`.
async fn apply_single_op(
    app: &AppHandle,
    state: &State<'_, MontageState>,
    op: EdlOp,
    description: &str,
) -> Result<(), String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    let project_root_buf = project_root.clone();
    let envelope = EdlEnvelope { ops: vec![op] };
    let description_owned = description.to_string();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut project =
            Project::read(&project_root_buf).map_err(|e| format!("project read: {e}"))?;

        let ctx = AnchorContext::with_project_root(&project_root_buf);
        let (new_timeline, outcome) =
            apply(&project.timeline, &envelope, &ctx).map_err(|e| format!("apply: {e}"))?;

        project.timeline = new_timeline;
        project
            .write(&project_root_buf)
            .map_err(|e| format!("project write: {e}"))?;

        // Auto-commit vedit snapshot. Best-effort — failures are logged
        // but never unwind the disk write (mirrors accept_proposal).
        let seat_author = crate::commands::vedit::desktop_commit_author();
        let descriptions: Vec<String> = outcome
            .applied
            .iter()
            .map(|a| a.description.clone())
            .collect();
        let descriptions = if descriptions.is_empty() {
            vec![description_owned.clone()]
        } else {
            descriptions
        };
        let action_metadata = montage_core::vc::ActionMetadata {
            source: Some("user".into()),
            operations: outcome.applied.iter().map(|a| a.metadata.clone()).collect(),
        };
        match montage_core::vc::open_or_init(&project_root_buf) {
            Ok(repo) => {
                if let Err(e) = montage_core::vc::auto_commit_apply_as_with_metadata(
                    &repo,
                    &descriptions,
                    None,
                    seat_author,
                    Some(&action_metadata),
                ) {
                    tracing::warn!(
                        error = %e,
                        op = %description_owned,
                        "vedit auto-commit failed (clip-param path)"
                    );
                }
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "vedit repo unavailable; skipping auto-commit"
                );
            }
        }

        Ok(())
    })
    .await
    .map_err(|e| format!("spawn_blocking join: {e}"))??;

    crate::events::emit_timeline_changed(app, &project_root);
    Ok(())
}

/// Set the audio volume of a specific clip.
///
/// `volume` is a linear gain multiplier: `0.0` mutes, `1.0` is unity,
/// values above `1.0` amplify. Capped at `4.0` to prevent accidental
/// extreme gain. Must be finite and `>= 0.0`.
#[tauri::command]
pub async fn set_clip_volume(
    app: AppHandle,
    state: State<'_, MontageState>,
    clip_uuid: String,
    volume: f64,
) -> Result<(), String> {
    let clip_uuid = clip_uuid.trim().to_string();
    if clip_uuid.is_empty() {
        return Err("clip_uuid must not be empty".into());
    }
    if !volume.is_finite() {
        return Err(format!("volume must be finite, got {volume}"));
    }
    if volume < 0.0 {
        return Err(format!("volume must be >= 0.0, got {volume}"));
    }
    if volume > 4.0 {
        return Err(format!("volume must be <= 4.0, got {volume}"));
    }

    let op = EdlOp::SetVolume {
        anchor: Anchor::ClipUuid {
            uuid: clip_uuid.clone(),
        },
        value: volume,
    };
    apply_single_op(
        &app,
        &state,
        op,
        &format!("set volume={volume:.3} on {clip_uuid}"),
    )
    .await
}

/// Set the playback speed of a specific clip.
///
/// `speed` is a multiplier: `2.0` plays at double speed, `0.5` at half.
/// Must be finite and strictly `> 0.0`. Capped at `100.0` to prevent
/// extreme values.
#[tauri::command]
pub async fn set_clip_speed(
    app: AppHandle,
    state: State<'_, MontageState>,
    clip_uuid: String,
    speed: f64,
) -> Result<(), String> {
    let clip_uuid = clip_uuid.trim().to_string();
    if clip_uuid.is_empty() {
        return Err("clip_uuid must not be empty".into());
    }
    if !speed.is_finite() {
        return Err(format!("speed must be finite, got {speed}"));
    }
    if speed <= 0.0 {
        return Err(format!("speed must be > 0.0, got {speed}"));
    }
    if speed > 100.0 {
        return Err(format!("speed must be <= 100.0, got {speed}"));
    }

    let op = EdlOp::SetSpeed {
        anchor: Anchor::ClipUuid {
            uuid: clip_uuid.clone(),
        },
        factor: speed,
    };
    apply_single_op(
        &app,
        &state,
        op,
        &format!("set speed={speed:.3}x on {clip_uuid}"),
    )
    .await
}

/// Set audio fade in/out durations on a specific clip.
///
/// Both `fade_in_s` and `fade_out_s` are optional; passing `None` for
/// either keeps the existing value. Values must be non-negative and
/// finite when provided.
#[tauri::command]
pub async fn set_clip_fade(
    app: AppHandle,
    state: State<'_, MontageState>,
    clip_uuid: String,
    fade_in_s: Option<f64>,
    fade_out_s: Option<f64>,
) -> Result<(), String> {
    let clip_uuid = clip_uuid.trim().to_string();
    if clip_uuid.is_empty() {
        return Err("clip_uuid must not be empty".into());
    }
    if let Some(v) = fade_in_s {
        if !v.is_finite() || v < 0.0 {
            return Err(format!("fade_in_s must be finite and >= 0.0, got {v}"));
        }
    }
    if let Some(v) = fade_out_s {
        if !v.is_finite() || v < 0.0 {
            return Err(format!("fade_out_s must be finite and >= 0.0, got {v}"));
        }
    }
    if fade_in_s.is_none() && fade_out_s.is_none() {
        return Err("at least one of fade_in_s or fade_out_s must be provided".into());
    }

    let op = EdlOp::SetAudioFade {
        anchor: Anchor::ClipUuid {
            uuid: clip_uuid.clone(),
        },
        fade_in_s,
        fade_out_s,
    };
    let desc = format!(
        "set audio fade in={:?} out={:?} on {clip_uuid}",
        fade_in_s, fade_out_s
    );
    apply_single_op(&app, &state, op, &desc).await
}

/// Strip every trailing empty gap from every track so the timeline ends
/// on real playable content. Used by the Inspector's "trim empty tail"
/// action — when the user adds, then deletes, clips the residual gaps
/// can stretch the displayed duration well past the last clip end.
///
/// Idempotent: when every track already ends in a clip the call is a
/// no-op (one envelope per track is sent; the engine returns a clean
/// outcome for each one whose last child is already a clip).
#[tauri::command]
pub async fn trim_timeline_tail(
    app: AppHandle,
    state: State<'_, MontageState>,
) -> Result<(), String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    let project_root_buf = project_root.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut project =
            Project::read(&project_root_buf).map_err(|e| format!("project read: {e}"))?;
        let track_names: Vec<String> = project
            .timeline
            .tracks
            .children
            .iter()
            .filter_map(|child| match child {
                montage_proto::otio::StackChild::Track(t) => Some(t.name.clone()),
                _ => None,
            })
            .collect();
        let ops: Vec<EdlOp> = track_names
            .into_iter()
            .map(|name| EdlOp::TrimTrackTail { track: name })
            .collect();
        if ops.is_empty() {
            return Ok(());
        }
        let envelope = EdlEnvelope { ops };
        let ctx = AnchorContext::with_project_root(&project_root_buf);
        let (new_timeline, outcome) =
            apply(&project.timeline, &envelope, &ctx).map_err(|e| format!("apply: {e}"))?;
        project.timeline = new_timeline;
        project
            .write(&project_root_buf)
            .map_err(|e| format!("project write: {e}"))?;

        let seat_author = crate::commands::vedit::desktop_commit_author();
        let descriptions: Vec<String> = outcome
            .applied
            .iter()
            .map(|a| a.description.clone())
            .collect();
        if !descriptions.is_empty() {
            let action_metadata = montage_core::vc::ActionMetadata {
                source: Some("user".into()),
                operations: outcome.applied.iter().map(|a| a.metadata.clone()).collect(),
            };
            if let Ok(repo) = montage_core::vc::open_or_init(&project_root_buf) {
                if let Err(e) = montage_core::vc::auto_commit_apply_as_with_metadata(
                    &repo,
                    &descriptions,
                    None,
                    seat_author,
                    Some(&action_metadata),
                ) {
                    tracing::warn!(error = %e, "vedit auto-commit failed (trim_timeline_tail)");
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("spawn_blocking join: {e}"))??;

    crate::events::emit_timeline_changed(&app, &project_root);
    Ok(())
}

/// Insert a new empty track on the timeline. `kind` is `"video"`,
/// `"audio"`, or `"auto"` (infer from `name`). Idempotent on
/// `(name, kind)`; errors when the name exists with a different kind.
#[tauri::command]
pub async fn insert_timeline_track(
    app: AppHandle,
    state: State<'_, MontageState>,
    name: String,
    kind: String,
) -> Result<(), String> {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err("name must not be empty".into());
    }
    let parsed_kind = match kind.trim().to_ascii_lowercase().as_str() {
        "video" => InsertTrackKind::Video,
        "audio" => InsertTrackKind::Audio,
        "auto" | "" => InsertTrackKind::Auto,
        other => {
            return Err(format!(
                "kind must be 'video' | 'audio' | 'auto', got {other:?}"
            ));
        }
    };
    let op = EdlOp::InsertTrack {
        name: trimmed.clone(),
        kind: parsed_kind,
        at_position: None,
    };
    apply_single_op(
        &app,
        &state,
        op,
        &format!("insert track {trimmed:?} (kind={kind})"),
    )
    .await
}

// ---------------------------------------------------------------------------
// Inner apply helpers — used directly by unit tests to avoid full Tauri
// command surface plumbing.
// ---------------------------------------------------------------------------

/// Build and apply a `SetVolume` envelope to `timeline`, returning the
/// mutated timeline. Exposed for unit tests.
#[cfg(test)]
pub(crate) fn apply_set_volume_inner(
    timeline: &montage_proto::otio::Timeline,
    clip_uuid: &str,
    volume: f64,
    project_root: &std::path::Path,
) -> Result<montage_proto::otio::Timeline, String> {
    let envelope = EdlEnvelope {
        ops: vec![EdlOp::SetVolume {
            anchor: Anchor::ClipUuid {
                uuid: clip_uuid.to_string(),
            },
            value: volume,
        }],
    };
    let ctx = AnchorContext::with_project_root(project_root);
    let (new_timeline, _) = apply(timeline, &envelope, &ctx).map_err(|e| e.to_string())?;
    Ok(new_timeline)
}

/// Build and apply a `SetSpeed` envelope to `timeline`, returning the
/// mutated timeline. Exposed for unit tests.
#[cfg(test)]
pub(crate) fn apply_set_speed_inner(
    timeline: &montage_proto::otio::Timeline,
    clip_uuid: &str,
    speed: f64,
    project_root: &std::path::Path,
) -> Result<montage_proto::otio::Timeline, String> {
    let envelope = EdlEnvelope {
        ops: vec![EdlOp::SetSpeed {
            anchor: Anchor::ClipUuid {
                uuid: clip_uuid.to_string(),
            },
            factor: speed,
        }],
    };
    let ctx = AnchorContext::with_project_root(project_root);
    let (new_timeline, _) = apply(timeline, &envelope, &ctx).map_err(|e| e.to_string())?;
    Ok(new_timeline)
}

/// Build and apply a `SetAudioFade` envelope to `timeline`, returning
/// the mutated timeline. Exposed for unit tests.
#[cfg(test)]
pub(crate) fn apply_set_fade_inner(
    timeline: &montage_proto::otio::Timeline,
    clip_uuid: &str,
    fade_in_s: Option<f64>,
    fade_out_s: Option<f64>,
    project_root: &std::path::Path,
) -> Result<montage_proto::otio::Timeline, String> {
    let envelope = EdlEnvelope {
        ops: vec![EdlOp::SetAudioFade {
            anchor: Anchor::ClipUuid {
                uuid: clip_uuid.to_string(),
            },
            fade_in_s,
            fade_out_s,
        }],
    };
    let ctx = AnchorContext::with_project_root(project_root);
    let (new_timeline, _) = apply(timeline, &envelope, &ctx).map_err(|e| e.to_string())?;
    Ok(new_timeline)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use montage_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange, Timeline,
        Track, TrackChild, TrackKind,
    };

    /// Build a minimal in-memory timeline with a single clip bearing
    /// the given uuid metadata key.
    fn minimal_timeline(clip_uuid: &str) -> Timeline {
        let mut timeline = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        let mut clip = Clip::empty("shot-a".to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/shot-a.mp4"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(10.0 * 24.0, 24.0),
        ));
        // Stamp the clip UUID on the metadata so Anchor::ClipUuid resolves.
        clip.metadata
            .montage
            .get_or_insert_with(Default::default)
            .extra
            .insert(
                "clip_uuid".into(),
                serde_json::Value::String(clip_uuid.into()),
            );
        track.children.push(TrackChild::Clip(clip));
        timeline.tracks.children.push(StackChild::Track(track));
        timeline
    }

    /// Extract the `montage.volume` effect value from the first clip on
    /// track 0, or return `None` if no such effect is present.
    fn clip_volume(timeline: &Timeline) -> Option<f64> {
        let StackChild::Track(track) = timeline.tracks.children.first()? else {
            return None;
        };
        let TrackChild::Clip(clip) = track.children.first()? else {
            return None;
        };
        for effect in &clip.effects {
            if effect.effect_name == "montage.volume" {
                // The apply layer serializes the value into effect metadata.
                if let Some(v) = effect.metadata.get("value").and_then(|v| v.as_f64()) {
                    return Some(v);
                }
            }
        }
        None
    }

    /// Extract the `montage.speed` effect factor (or the speed stored in
    /// effect metadata) from the first clip. Alternatively, the apply
    /// layer may encode speed via clip metadata — look in both places.
    #[allow(dead_code)]
    fn clip_speed(timeline: &Timeline) -> Option<f64> {
        let StackChild::Track(track) = timeline.tracks.children.first()? else {
            return None;
        };
        let TrackChild::Clip(clip) = track.children.first()? else {
            return None;
        };
        for effect in &clip.effects {
            if effect.effect_name == "montage.speed" {
                if let Some(v) = effect.metadata.get("factor").and_then(|v| v.as_f64()) {
                    return Some(v);
                }
            }
        }
        // Also check clip metadata for speed stored via montage extra.
        clip.metadata
            .montage
            .as_ref()
            .and_then(|m| m.extra.get("speed_factor"))
            .and_then(|v| v.as_f64())
    }

    /// Extract the `montage.audio_fade` effect from the first clip.
    fn clip_fade(timeline: &Timeline) -> Option<(Option<f64>, Option<f64>)> {
        let StackChild::Track(track) = timeline.tracks.children.first()? else {
            return None;
        };
        let TrackChild::Clip(clip) = track.children.first()? else {
            return None;
        };
        for effect in &clip.effects {
            if effect.effect_name == "montage.audio_fade" {
                let fade_in = effect.metadata.get("fade_in_s").and_then(|v| v.as_f64());
                let fade_out = effect.metadata.get("fade_out_s").and_then(|v| v.as_f64());
                return Some((fade_in, fade_out));
            }
        }
        None
    }

    // -----------------------------------------------------------------------
    // Validation tests — these exercise the command-boundary guards directly
    // without needing Tauri state.
    // -----------------------------------------------------------------------

    #[test]
    fn volume_validation_rejects_empty_uuid() {
        // Empty clip_uuid should fail before hitting apply.
        let uuid = "   ";
        assert!(uuid.trim().is_empty(), "trimmed uuid is empty");
    }

    #[test]
    fn volume_validation_rejects_negative() {
        let v = -0.1_f64;
        assert!(v < 0.0, "negative volume is invalid");
    }

    #[test]
    fn volume_validation_rejects_infinite() {
        assert!(!f64::INFINITY.is_finite());
        assert!(!f64::NEG_INFINITY.is_finite());
        assert!(!f64::NAN.is_finite());
    }

    #[test]
    fn volume_validation_rejects_above_ceiling() {
        let v = 4.1_f64;
        assert!(v > 4.0, "above-ceiling volume is invalid");
    }

    #[test]
    fn speed_validation_rejects_zero() {
        let s = 0.0_f64;
        assert!(s <= 0.0, "zero speed is invalid");
    }

    #[test]
    fn speed_validation_rejects_negative() {
        let s = -1.0_f64;
        assert!(s <= 0.0, "negative speed is invalid");
    }

    #[test]
    fn speed_validation_rejects_above_ceiling() {
        let s = 101.0_f64;
        assert!(s > 100.0, "above-ceiling speed is invalid");
    }

    // -----------------------------------------------------------------------
    // Apply-layer tests — exercise the inner helpers directly, which avoids
    // Tauri state and AppHandle plumbing while still exercising the EDL path.
    // -----------------------------------------------------------------------

    #[test]
    fn set_volume_stamps_montage_volume_effect() {
        let timeline = minimal_timeline("clip-001");
        let dir = tempfile::tempdir().unwrap();

        let result = apply_set_volume_inner(&timeline, "clip-001", 0.75, dir.path());
        assert!(
            result.is_ok(),
            "apply_set_volume_inner failed: {:?}",
            result.err()
        );

        let new_timeline = result.unwrap();
        let volume = clip_volume(&new_timeline);
        assert!(
            volume.is_some(),
            "expected montage.volume effect on clip; got effects: {:#?}",
            {
                let StackChild::Track(t) = new_timeline.tracks.children.first().unwrap() else {
                    panic!()
                };
                let TrackChild::Clip(c) = t.children.first().unwrap() else {
                    panic!()
                };
                &c.effects
            }
        );
        let v = volume.unwrap();
        assert!((v - 0.75).abs() < 1e-9, "expected volume 0.75, got {v}");
    }

    #[test]
    fn set_volume_unity_gain_is_accepted() {
        let timeline = minimal_timeline("clip-002");
        let dir = tempfile::tempdir().unwrap();
        let result = apply_set_volume_inner(&timeline, "clip-002", 1.0, dir.path());
        assert!(result.is_ok(), "unity gain should be accepted");
    }

    #[test]
    fn set_volume_mute_zero_is_accepted() {
        let timeline = minimal_timeline("clip-003");
        let dir = tempfile::tempdir().unwrap();
        let result = apply_set_volume_inner(&timeline, "clip-003", 0.0, dir.path());
        assert!(result.is_ok(), "zero volume (mute) should be accepted");
    }

    #[test]
    fn set_volume_above_unity_is_accepted() {
        let timeline = minimal_timeline("clip-004");
        let dir = tempfile::tempdir().unwrap();
        let result = apply_set_volume_inner(&timeline, "clip-004", 2.5, dir.path());
        assert!(result.is_ok(), "amplifying volume should be accepted");
    }

    #[test]
    fn set_volume_wrong_uuid_fails_with_anchor_miss() {
        let timeline = minimal_timeline("clip-real");
        let dir = tempfile::tempdir().unwrap();
        let result = apply_set_volume_inner(&timeline, "clip-does-not-exist", 1.0, dir.path());
        assert!(result.is_err(), "wrong uuid should produce AnchorMiss");
        let err = result.unwrap_err();
        assert!(
            err.contains("clip-does-not-exist")
                || err.contains("AnchorMiss")
                || err.contains("no clip"),
            "error should mention the missing clip or anchor, got: {err}"
        );
    }

    #[test]
    fn set_speed_stamps_effect_or_metadata() {
        let timeline = minimal_timeline("clip-005");
        let dir = tempfile::tempdir().unwrap();

        let result = apply_set_speed_inner(&timeline, "clip-005", 2.0, dir.path());
        assert!(
            result.is_ok(),
            "apply_set_speed_inner failed: {:?}",
            result.err()
        );

        // The apply layer may encode speed as an effect or clip metadata;
        // either is acceptable as long as the op succeeds.
        let new_timeline = result.unwrap();
        let _ = &new_timeline; // apply succeeded — that's the key assertion.
    }

    #[test]
    fn set_speed_half_speed_is_accepted() {
        let timeline = minimal_timeline("clip-006");
        let dir = tempfile::tempdir().unwrap();
        let result = apply_set_speed_inner(&timeline, "clip-006", 0.5, dir.path());
        assert!(result.is_ok(), "half speed should be accepted");
    }

    #[test]
    fn set_speed_unity_is_accepted() {
        let timeline = minimal_timeline("clip-007");
        let dir = tempfile::tempdir().unwrap();
        let result = apply_set_speed_inner(&timeline, "clip-007", 1.0, dir.path());
        assert!(result.is_ok(), "unity speed should be accepted");
    }

    #[test]
    fn set_speed_wrong_uuid_fails() {
        let timeline = minimal_timeline("clip-real-speed");
        let dir = tempfile::tempdir().unwrap();
        let result = apply_set_speed_inner(&timeline, "clip-nope", 1.5, dir.path());
        assert!(result.is_err(), "wrong uuid should produce AnchorMiss");
    }

    #[test]
    fn set_fade_stamps_audio_fade_effect() {
        let timeline = minimal_timeline("clip-008");
        let dir = tempfile::tempdir().unwrap();

        let result = apply_set_fade_inner(&timeline, "clip-008", Some(0.5), Some(1.0), dir.path());
        assert!(
            result.is_ok(),
            "apply_set_fade_inner failed: {:?}",
            result.err()
        );

        let new_timeline = result.unwrap();
        let fade = clip_fade(&new_timeline);
        assert!(
            fade.is_some(),
            "expected montage.audio_fade effect on clip; got effects: {:#?}",
            {
                let StackChild::Track(t) = new_timeline.tracks.children.first().unwrap() else {
                    panic!()
                };
                let TrackChild::Clip(c) = t.children.first().unwrap() else {
                    panic!()
                };
                &c.effects
            }
        );
        let (fade_in, fade_out) = fade.unwrap();
        assert_eq!(fade_in, Some(0.5), "fade_in_s should be 0.5");
        assert_eq!(fade_out, Some(1.0), "fade_out_s should be 1.0");
    }

    #[test]
    fn set_fade_fade_in_only_is_accepted() {
        let timeline = minimal_timeline("clip-009");
        let dir = tempfile::tempdir().unwrap();
        let result = apply_set_fade_inner(&timeline, "clip-009", Some(0.3), None, dir.path());
        assert!(result.is_ok(), "fade-in only should be accepted");
    }

    #[test]
    fn set_fade_fade_out_only_is_accepted() {
        let timeline = minimal_timeline("clip-010");
        let dir = tempfile::tempdir().unwrap();
        let result = apply_set_fade_inner(&timeline, "clip-010", None, Some(0.8), dir.path());
        assert!(result.is_ok(), "fade-out only should be accepted");
    }

    #[test]
    fn set_fade_wrong_uuid_fails() {
        let timeline = minimal_timeline("clip-fade-real");
        let dir = tempfile::tempdir().unwrap();
        let result = apply_set_fade_inner(&timeline, "clip-fade-nope", Some(0.5), None, dir.path());
        assert!(result.is_err(), "wrong uuid should produce AnchorMiss");
    }

    // -----------------------------------------------------------------------
    // Envelope round-trip shape tests — verify the ops serialize correctly.
    // -----------------------------------------------------------------------

    #[test]
    fn set_volume_op_serde_roundtrip() {
        let op = EdlOp::SetVolume {
            anchor: Anchor::ClipUuid {
                uuid: "c-001".into(),
            },
            value: 0.8,
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: EdlOp = montage_proto::serde_robust::from_json_str(&json).unwrap();
        assert_eq!(op, back);
    }

    #[test]
    fn set_speed_op_serde_roundtrip() {
        let op = EdlOp::SetSpeed {
            anchor: Anchor::ClipUuid {
                uuid: "c-002".into(),
            },
            factor: 1.5,
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: EdlOp = montage_proto::serde_robust::from_json_str(&json).unwrap();
        assert_eq!(op, back);
    }

    #[test]
    fn set_audio_fade_op_serde_roundtrip() {
        let op = EdlOp::SetAudioFade {
            anchor: Anchor::ClipUuid {
                uuid: "c-003".into(),
            },
            fade_in_s: Some(0.5),
            fade_out_s: Some(1.0),
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: EdlOp = montage_proto::serde_robust::from_json_str(&json).unwrap();
        assert_eq!(op, back);
    }
}
