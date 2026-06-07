//! Tauri commands for the per-project editorial notes file.
//! Phase 1.7 wires the NotesPanel UI to the persistent storage in
//! `<project>/.montage/notes.json` (shape defined in
//! `montage_core::notes::NotesFile`).
//!
//! Dismiss is special — the command also stamps the matching coarse
//! pattern into `dismissed_patterns.json` so the agent's editorial-
//! finding tools (find_dead_air etc.) respect the dismissal on next
//! scan. That coupling lives here at the desktop layer rather than
//! in core; core just owns the load/save shape.

use montage_core::dismissal::{
    DismissalBucket, DismissalFile, dismissal_file_path, load_dismissals, save_dismissals,
};
use montage_core::notes::{NotesFile, PersistedNote, load_notes, save_notes};
use montage_proto::otio::{StackChild, Timeline, TrackChild, TrackKind};
use montage_proto::project::Project;
use serde::Deserialize;
use tauri::State;

use crate::state::MontageState;

/// Read the current notes file. Returns empty when no project is
/// loaded — same shape so the frontend doesn't have to special-case
/// the unloaded state.
#[tauri::command]
pub async fn list_notes(state: State<'_, MontageState>) -> Result<NotesFile, String> {
    let project_root = match state.project_root.lock().await.clone() {
        Some(p) => p,
        None => return Ok(NotesFile::empty()),
    };
    let mut file = load_notes(&project_root);
    if hydrate_missing_broll_anchors_from_project(&project_root, &mut file)? {
        save_notes(&project_root, &file)?;
    }
    Ok(file)
}

/// Append-or-replace a single note. Idempotent: same id → in-place
/// update so multiple agent emissions of the same note don't grow
/// duplicates. Returns the updated file.
#[tauri::command]
pub async fn upsert_note(
    state: State<'_, MontageState>,
    note: PersistedNote,
) -> Result<NotesFile, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let mut file = load_notes(&project_root);
    file.upsert(note);
    hydrate_missing_broll_anchors_from_project(&project_root, &mut file)?;
    save_notes(&project_root, &file)?;
    Ok(file)
}

/// Mutate a note's status (resolved / dismissed). When the new
/// status is `dismissed` AND a dismissal bucket is supplied, also
/// stamp the bucket into `dismissed_patterns.json` so the agent
/// won't re-surface the same kind on next scan. The bucket comes
/// from the frontend because only the UI knows which threshold
/// (e.g. "silence under 2s") to dismiss; the note record alone
/// doesn't carry the bucket.
///
/// Returns `(updated notes, updated dismissals)` so the UI can
/// reflect both writes without round-tripping a separate
/// `list_dismissals` call.
#[tauri::command]
pub async fn set_note_status(
    state: State<'_, MontageState>,
    args: SetNoteStatusArgs,
) -> Result<(NotesFile, DismissalFile), String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    let mut notes = load_notes(&project_root);
    if !notes.set_status(&args.id, &args.status) {
        return Err(format!("note id {:?} not found", args.id));
    }
    save_notes(&project_root, &notes)?;

    let mut dismissals = load_dismissals(&project_root);
    if args.status == "dismissed"
        && let Some(bucket) = args.dismiss_bucket
    {
        dismissals.add(bucket);
        save_dismissals(&project_root, &dismissals)?;
    }
    Ok((notes, dismissals))
}

#[derive(Debug, Deserialize)]
pub struct SetNoteStatusArgs {
    /// Note id to mutate.
    pub id: String,
    /// New status: `"open"`, `"resolved"`, or `"dismissed"`.
    pub status: String,
    /// Optional bucket to dismiss alongside (only honored when
    /// `status == "dismissed"`). When `None`, only the individual
    /// note flips status; the agent will still re-surface the same
    /// kind on next scan.
    #[serde(default)]
    pub dismiss_bucket: Option<DismissalBucket>,
}

/// Drop a note record entirely. Used by the panel's "clear all
/// resolved" affordance and by edge cases where a stale note
/// should disappear (e.g. the agent emitted a note pointing at a
/// clip the user deleted; the desktop garbage-collects it). v1
/// has no UI for this beyond the Tauri command — wired for future
/// use.
#[tauri::command]
pub async fn delete_note(state: State<'_, MontageState>, id: String) -> Result<NotesFile, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let mut file = load_notes(&project_root);
    file.notes.retain(|n| n.id != id);
    save_notes(&project_root, &file)?;
    Ok(file)
}

/// Defensive util: surface where the dismissals file lives. Used
/// by the desktop's "show in finder" affordance (not wired to UI
/// in Phase 1, but cheap to expose now).
#[tauri::command]
pub async fn dismissals_path(state: State<'_, MontageState>) -> Result<Option<String>, String> {
    Ok(state
        .project_root
        .lock()
        .await
        .as_ref()
        .map(|p| dismissal_file_path(p).to_string_lossy().into_owned()))
}

fn hydrate_missing_broll_anchors_from_project(
    project_root: &std::path::Path,
    file: &mut NotesFile,
) -> Result<bool, String> {
    if !file.notes.iter().any(needs_broll_anchor_hydration) {
        return Ok(false);
    }
    let project = Project::read(project_root).map_err(|e| format!("read project: {e}"))?;
    Ok(hydrate_missing_broll_anchors(
        &project.timeline,
        &mut file.notes,
    ))
}

fn hydrate_missing_broll_anchors(timeline: &Timeline, notes: &mut [PersistedNote]) -> bool {
    let mut changed = false;
    for note in notes {
        if !needs_broll_anchor_hydration(note) {
            continue;
        }
        if let Some(uuid) = clip_uuid_at_timeline_time(timeline, note.anchor_at_s) {
            note.broll_anchor = Some(serde_json::json!({
                "kind": "clip_uuid",
                "uuid": uuid,
            }));
            changed = true;
        }
    }
    changed
}

fn needs_broll_anchor_hydration(note: &PersistedNote) -> bool {
    note.kind == "broll_suggestion" && note.broll_anchor.is_none()
}

fn clip_uuid_at_timeline_time(timeline: &Timeline, at_s: f64) -> Option<String> {
    if !at_s.is_finite() || at_s < 0.0 {
        return None;
    }

    for stack_child in &timeline.tracks.children {
        let StackChild::Track(track) = stack_child else {
            continue;
        };
        if !matches!(track.kind, TrackKind::Video) {
            continue;
        }
        if track
            .metadata
            .get("montage_track_role")
            .and_then(|v| v.as_str())
            == Some("titles")
        {
            continue;
        }

        let mut cursor_s = 0.0;
        for child in &track.children {
            let duration_s = match child {
                TrackChild::Clip(clip) => clip
                    .source_range
                    .as_ref()
                    .map(|r| r.duration.to_seconds())
                    .unwrap_or(0.0),
                TrackChild::Gap(gap) => gap.source_range.duration.to_seconds(),
                TrackChild::Transition(_) | TrackChild::Stack(_) => 0.0,
            };
            let end_s = cursor_s + duration_s;
            if at_s >= cursor_s
                && at_s < end_s
                && let TrackChild::Clip(clip) = child
            {
                return Some(clip_uuid_or_name(clip));
            }
            cursor_s = end_s;
        }
    }
    None
}

fn clip_uuid_or_name(clip: &montage_proto::otio::Clip) -> String {
    clip.metadata
        .montage
        .as_ref()
        .and_then(|m| m.extra.get("clip_uuid"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| clip.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use montage_proto::otio::{
        Clip, ExternalReference, MediaReference, RationalTime, TimeRange, Track,
    };

    fn note(id: &str, kind: &str, anchor_at_s: f64) -> PersistedNote {
        PersistedNote {
            id: id.into(),
            kind: kind.into(),
            status: "open".into(),
            anchor_at_s,
            summary: "test".into(),
            suggested_proposal: None,
            continuity_verdict: None,
            continuity_reasons: None,
            broll_query: None,
            broll_previews: None,
            broll_anchor: None,
            author: None,
            comment_text: None,
        }
    }

    fn clip(name: &str, uuid: &str, duration_s: f64) -> Clip {
        let mut clip = Clip::empty(name.to_string());
        clip.media_reference = MediaReference::External(ExternalReference::new("raw/a.mp4"));
        clip.source_range = Some(TimeRange::new(
            RationalTime::zero(24.0),
            RationalTime::new(duration_s * 24.0, 24.0),
        ));
        clip.metadata
            .montage
            .get_or_insert_with(Default::default)
            .extra
            .insert("clip_uuid".into(), serde_json::Value::String(uuid.into()));
        clip
    }

    fn timeline_with_two_clips() -> Timeline {
        let mut timeline = Timeline::empty("test");
        let mut track = Track::empty("V1", TrackKind::Video);
        track
            .children
            .push(TrackChild::Clip(clip("clip-0", "u0", 5.0)));
        track
            .children
            .push(TrackChild::Clip(clip("clip-1", "u1", 5.0)));
        timeline.tracks.children.push(StackChild::Track(track));
        timeline
    }

    #[test]
    fn hydrates_legacy_broll_note_anchor_from_timeline_time() {
        let timeline = timeline_with_two_clips();
        let mut notes = vec![note("n1", "broll_suggestion", 6.0)];

        assert!(hydrate_missing_broll_anchors(&timeline, &mut notes));
        assert_eq!(
            notes[0].broll_anchor,
            Some(serde_json::json!({"kind": "clip_uuid", "uuid": "u1"}))
        );
    }

    #[test]
    fn leaves_non_broll_and_unmatched_notes_unchanged() {
        let timeline = timeline_with_two_clips();
        let mut notes = vec![
            note("n1", "silence_trim", 1.0),
            note("n2", "broll_suggestion", 99.0),
        ];

        assert!(!hydrate_missing_broll_anchors(&timeline, &mut notes));
        assert!(notes.iter().all(|note| note.broll_anchor.is_none()));
    }

    #[test]
    fn preserves_existing_broll_anchor() {
        let timeline = timeline_with_two_clips();
        let mut notes = vec![note("n1", "broll_suggestion", 6.0)];
        notes[0].broll_anchor = Some(serde_json::json!({
            "kind": "transcript_snippet",
            "text": "already anchored",
        }));

        assert!(!hydrate_missing_broll_anchors(&timeline, &mut notes));
        assert_eq!(
            notes[0].broll_anchor,
            Some(serde_json::json!({
                "kind": "transcript_snippet",
                "text": "already anchored",
            }))
        );
    }
}
