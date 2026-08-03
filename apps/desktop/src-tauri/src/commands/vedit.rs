//! Desktop-facing vedit history commands.

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::events::emit_timeline_changed;
use crate::state::MontageState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditCommitEntry {
    /// Commit hash.
    pub commit_hash: String,
    /// Timeline content hash.
    pub timeline_hash: String,
    /// ISO timestamp.
    pub timestamp: String,
    /// First line of the commit message.
    pub header: String,
    /// Full commit message.
    pub full_message: String,
    /// Parent commit hashes.
    pub parents: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditDiffResponse {
    pub from_ref: String,
    pub to_ref: String,
    pub change_count: usize,
    pub changed_clip_count: usize,
    pub changed_clip_ids: Vec<String>,
    pub changes: serde_json::Value,
    pub animation_changes: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditChangedClipIdsResponse {
    pub from_ref: String,
    pub to_ref: String,
    pub changed_clip_count: usize,
    pub changed_clip_ids: Vec<String>,
    pub structural_change_count: usize,
    pub animation_change_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditMergePreflightResponse {
    pub source_ref: String,
    pub target_ref: String,
    pub source_commit: String,
    pub target_commit: String,
    pub merge_base: String,
    pub is_mergeable: bool,
    pub source_changed_clip_ids: Vec<String>,
    pub target_changed_clip_ids: Vec<String>,
    pub overlapping_clip_ids: Vec<String>,
    pub source_change_count: usize,
    pub target_change_count: usize,
    pub next_step: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditRestoreResponse {
    pub restored_ref: String,
    pub restored_commit_hash: String,
    pub restored_timeline_hash: String,
    pub restored_parent_hash: Option<String>,
    pub audit_commit_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditTagEntry {
    pub name: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditBranchEntry {
    pub name: String,
    pub target: String,
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditCheckoutResponse {
    pub branch: String,
    pub commit_hash: String,
    pub timeline_hash: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditShowResponse {
    pub commit_hash: String,
    pub timeline_hash: String,
    pub timestamp: String,
    pub header: String,
    pub full_message: String,
    pub parents: Vec<String>,
    pub diff: VeditDiffResponse,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VeditBlameEntry {
    pub commit_hash: String,
    pub timeline_hash: String,
    pub timestamp: String,
    pub header: String,
    pub full_message: String,
    pub parents: Vec<String>,
    pub changes: serde_json::Value,
    pub animation_changes: serde_json::Value,
}

#[tauri::command]
pub async fn list_vedit_commits(
    state: State<'_, MontageState>,
    limit: Option<usize>,
) -> Result<Vec<VeditCommitEntry>, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = montage_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let entries = montage_core::vc::log(&repo, limit.unwrap_or(30).min(200))
        .map_err(|e| format!("read vedit log: {e}"))?;
    Ok(entries
        .into_iter()
        .map(|entry| VeditCommitEntry {
            commit_hash: entry.commit_hash,
            timeline_hash: entry.timeline_hash,
            timestamp: entry.timestamp,
            header: entry.header,
            full_message: entry.full_message,
            parents: entry.parents,
        })
        .collect())
}

#[tauri::command]
pub async fn diff_vedit_refs(
    state: State<'_, MontageState>,
    from_ref: Option<String>,
    to_ref: Option<String>,
) -> Result<VeditDiffResponse, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = montage_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let diff = montage_core::vc::diff_refs(&repo, from_ref.as_deref(), to_ref.as_deref())
        .map_err(|e| format!("read vedit diff: {e}"))?;
    diff_response(diff)
}

#[tauri::command]
pub async fn changed_vedit_clip_ids(
    state: State<'_, MontageState>,
    from_ref: Option<String>,
    to_ref: Option<String>,
) -> Result<VeditChangedClipIdsResponse, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = montage_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let diff = montage_core::vc::diff_refs(&repo, from_ref.as_deref(), to_ref.as_deref())
        .map_err(|e| format!("read vedit changed clip ids: {e}"))?;
    Ok(changed_clip_ids_response(diff))
}

#[tauri::command]
pub async fn preflight_vedit_merge(
    state: State<'_, MontageState>,
    source_ref: String,
    target_ref: Option<String>,
) -> Result<VeditMergePreflightResponse, String> {
    let source_ref = source_ref.trim().to_string();
    if source_ref.is_empty() {
        return Err("source ref cannot be empty".into());
    }
    let target_ref = target_ref
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = montage_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let preflight = montage_core::vc::merge_preflight(&repo, &source_ref, target_ref)
        .map_err(|e| format!("preflight vedit merge: {e}"))?;
    Ok(merge_preflight_response(preflight))
}

#[tauri::command]
pub async fn list_vedit_tags(state: State<'_, MontageState>) -> Result<Vec<VeditTagEntry>, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = montage_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let tags = montage_core::vc::list_tags(&repo).map_err(|e| format!("read vedit tags: {e}"))?;
    Ok(tags
        .into_iter()
        .map(|tag| VeditTagEntry {
            name: tag.name,
            target: tag.target,
        })
        .collect())
}

#[tauri::command]
pub async fn tag_vedit_ref(
    state: State<'_, MontageState>,
    name: String,
    refstr: Option<String>,
) -> Result<VeditTagEntry, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("tag name cannot be empty".into());
    }
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = montage_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let refstr = refstr.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let tag = montage_core::vc::tag_ref(&repo, &name, refstr)
        .map_err(|e| format!("write vedit tag: {e}"))?;
    Ok(VeditTagEntry {
        name: tag.name,
        target: tag.target,
    })
}

#[tauri::command]
pub async fn list_vedit_branches(
    state: State<'_, MontageState>,
) -> Result<Vec<VeditBranchEntry>, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = montage_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let branches =
        montage_core::vc::list_branches(&repo).map_err(|e| format!("read vedit branches: {e}"))?;
    Ok(branches
        .into_iter()
        .map(|branch| VeditBranchEntry {
            name: branch.name,
            target: branch.target,
            is_current: branch.is_current,
        })
        .collect())
}

#[tauri::command]
pub async fn create_vedit_branch(
    state: State<'_, MontageState>,
    name: String,
    start_ref: Option<String>,
) -> Result<VeditBranchEntry, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("branch name cannot be empty".into());
    }
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = montage_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let start_ref = start_ref
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let branch = montage_core::vc::create_branch(&repo, &name, start_ref)
        .map_err(|e| format!("create vedit branch: {e}"))?;
    Ok(VeditBranchEntry {
        name: branch.name,
        target: branch.target,
        is_current: branch.is_current,
    })
}

#[tauri::command]
pub async fn checkout_vedit_branch(
    app: AppHandle,
    state: State<'_, MontageState>,
    branch: String,
) -> Result<VeditCheckoutResponse, String> {
    let branch = branch.trim().to_string();
    if branch.is_empty() {
        return Err("branch name cannot be empty".into());
    }
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let project_root_for_task = project_root.clone();
    let checkout = tokio::task::spawn_blocking(move || {
        let _mutation = montage_core::vc::lock_timeline_mutation(&project_root_for_task)
            .map_err(|e| format!("lock timeline mutation: {e}"))?;
        let repo = montage_core::vc::open_or_init(&project_root_for_task)
            .map_err(|e| format!("open vedit repo: {e}"))?;
        montage_core::vc::checkout_branch(&repo, &branch)
            .map_err(|e| format!("checkout vedit branch: {e}"))
    })
    .await
    .map_err(|e| format!("checkout vedit branch join: {e}"))??;
    emit_timeline_changed(&app, &project_root);
    Ok(VeditCheckoutResponse {
        branch: checkout.branch,
        commit_hash: checkout.commit_hash,
        timeline_hash: checkout.timeline_hash,
    })
}

#[tauri::command]
pub async fn show_vedit_commit(
    state: State<'_, MontageState>,
    refstr: String,
) -> Result<VeditShowResponse, String> {
    let refstr = refstr.trim().to_string();
    if refstr.is_empty() {
        return Err("show ref cannot be empty".into());
    }
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = montage_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let details = montage_core::vc::show_commit(&repo, &refstr)
        .map_err(|e| format!("show vedit commit: {e}"))?;
    let diff = diff_response(details.diff)?;
    Ok(VeditShowResponse {
        commit_hash: details.commit_hash,
        timeline_hash: details.timeline_hash,
        timestamp: details.timestamp,
        header: details.header,
        full_message: details.full_message,
        parents: details.parents,
        diff,
    })
}

#[tauri::command]
pub async fn blame_vedit_clip(
    state: State<'_, MontageState>,
    clip_id: String,
    start_ref: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<VeditBlameEntry>, String> {
    let clip_id = clip_id.trim().to_string();
    if clip_id.is_empty() {
        return Err("clip id cannot be empty".into());
    }
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = montage_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let limit = limit.unwrap_or(200).min(500);
    let entries = montage_core::vc::blame_clip(&repo, &clip_id, start_ref.as_deref(), limit)
        .map_err(|e| format!("blame vedit clip: {e}"))?;
    entries
        .into_iter()
        .map(|entry| {
            let changes = serde_json::to_value(&entry.changes)
                .map_err(|e| format!("serialize vedit blame changes: {e}"))?;
            let animation_changes = serde_json::to_value(&entry.animation_changes)
                .map_err(|e| format!("serialize vedit blame animation changes: {e}"))?;
            Ok(VeditBlameEntry {
                commit_hash: entry.commit_hash,
                timeline_hash: entry.timeline_hash,
                timestamp: entry.timestamp,
                header: entry.header,
                full_message: entry.full_message,
                parents: entry.parents,
                changes,
                animation_changes,
            })
        })
        .collect()
}

#[tauri::command]
pub async fn restore_vedit_ref(
    app: AppHandle,
    state: State<'_, MontageState>,
    refstr: String,
    expected_current: String,
) -> Result<VeditRestoreResponse, String> {
    let refstr = refstr.trim().to_string();
    if refstr.is_empty() {
        return Err("restore ref cannot be empty".into());
    }
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let project_root_for_task = project_root.clone();
    let response = tokio::task::spawn_blocking(move || {
        let _mutation = montage_core::vc::lock_timeline_mutation(&project_root_for_task)
            .map_err(|e| format!("lock timeline mutation: {e}"))?;
        let repo = montage_core::vc::open_or_init(&project_root_for_task)
            .map_err(|e| format!("open vedit repo: {e}"))?;
        ensure_expected_current(&repo, &expected_current)?;
        let restored = montage_core::vc::restore_working_timeline(&repo, &refstr)
            .map_err(|e| format!("restore vedit ref: {e}"))?;
        let restored_parent_hash = montage_core::vc::commit_parents(&repo, &restored.commit_hash)
            .map_err(|e| format!("read restored commit parents: {e}"))?
            .into_iter()
            .next();
        let header = format!("Restore timeline to {}", short_hash(&restored.commit_hash));
        let reasoning = format!(
            "Montage-Restored-Ref: {}\nMontage-Restored-Parent: {}\n\nRestored project.otio.json from the desktop timeline history panel.",
            restored.commit_hash,
            restored_parent_hash.as_deref().unwrap_or("none"),
        );
        // Restore is user-initiated from the desktop history panel — stamp
        // the seat-holder on the audit commit so blame views show who
        // rolled the timeline back.
        let audit = montage_core::vc::commit_current_timeline_as(
            &repo,
            &header,
            Some(&reasoning),
            desktop_commit_author(),
        )
        .map_err(|e| format!("commit restore audit: {e}"))?;
        Ok::<_, String>(VeditRestoreResponse {
            restored_ref: restored.requested_ref,
            restored_commit_hash: restored.commit_hash,
            restored_timeline_hash: restored.timeline_hash,
            restored_parent_hash,
            audit_commit_hash: Some(audit.commit_hash),
        })
    })
    .await
    .map_err(|e| format!("restore vedit ref join: {e}"))??;
    emit_timeline_changed(&app, &project_root);
    Ok(response)
}

fn ensure_expected_current(
    repo: &montage_core::vc::Repo,
    expected_current: &str,
) -> Result<(), String> {
    let expected = expected_current.trim();
    if expected.is_empty() {
        return Err("expected current vedit ref cannot be empty".into());
    }
    let actual = montage_core::vc::log(repo, 1)
        .map_err(|e| format!("read current vedit ref: {e}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "timeline history is empty".to_string())?;
    if actual.commit_hash != expected {
        return Err(format!(
            "timeline changed before restore (expected {}, found {})",
            short_hash(expected),
            short_hash(&actual.commit_hash),
        ));
    }
    let working_hash = montage_core::vc::working_timeline_hash(repo)
        .map_err(|e| format!("hash current working timeline: {e}"))?;
    if working_hash != actual.timeline_hash {
        return Err("timeline changed before restore (working copy has uncommitted edits)".into());
    }
    Ok(())
}

fn short_hash(hash: &str) -> String {
    hash.strip_prefix("sha256:")
        .unwrap_or(hash)
        .chars()
        .take(7)
        .collect()
}

fn merge_preflight_response(
    preflight: montage_core::vc::MergePreflight,
) -> VeditMergePreflightResponse {
    let next_step = if preflight.is_mergeable {
        "A future bounded merge can proceed under the non-overlapping clip-id rule.".to_string()
    } else {
        "Resolve overlapping clip ids manually before merging.".to_string()
    };
    VeditMergePreflightResponse {
        source_ref: preflight.source_ref,
        target_ref: preflight.target_ref,
        source_commit: preflight.source_commit,
        target_commit: preflight.target_commit,
        merge_base: preflight.merge_base,
        is_mergeable: preflight.is_mergeable,
        source_changed_clip_ids: preflight.source_changed_clip_ids,
        target_changed_clip_ids: preflight.target_changed_clip_ids,
        overlapping_clip_ids: preflight.overlapping_clip_ids,
        source_change_count: preflight.source_change_count,
        target_change_count: preflight.target_change_count,
        next_step,
    }
}

fn changed_clip_ids_response(diff: montage_core::vc::CommittedDiff) -> VeditChangedClipIdsResponse {
    let changed_clip_ids = montage_core::vc::changed_clip_ids(&diff)
        .into_iter()
        .collect::<Vec<_>>();
    VeditChangedClipIdsResponse {
        from_ref: diff.from_ref,
        to_ref: diff.to_ref,
        changed_clip_count: changed_clip_ids.len(),
        changed_clip_ids,
        structural_change_count: diff.changes.len(),
        animation_change_count: diff.animation_changes.len(),
    }
}

fn diff_response(diff: montage_core::vc::CommittedDiff) -> Result<VeditDiffResponse, String> {
    let change_count = diff.len();
    let changed_clip_ids = montage_core::vc::changed_clip_ids(&diff)
        .into_iter()
        .collect::<Vec<_>>();
    let changes =
        serde_json::to_value(&diff.changes).map_err(|e| format!("serialize vedit diff: {e}"))?;
    let animation_changes = serde_json::to_value(&diff.animation_changes)
        .map_err(|e| format!("serialize vedit animation diff: {e}"))?;
    Ok(VeditDiffResponse {
        from_ref: diff.from_ref,
        to_ref: diff.to_ref,
        change_count,
        changed_clip_count: changed_clip_ids.len(),
        changed_clip_ids,
        changes,
        animation_changes,
    })
}

/// Resolve the [`montage_core::vc::CommitAuthor`] to stamp on a
/// desktop-initiated commit.
///
/// Desktop sessions are real-user sessions: the seat-holder is the
/// person on the keyboard. Until we wire a richer in-process identity
/// (Tauri state, profile, etc.) the env vars `MONTAGE_USER_NAME` /
/// `MONTAGE_USER_EMAIL` are the only signal — `git`-style configuration
/// for the seat. When neither is set the caller passes `None` and the
/// `*_as` variants fall back to the "montage agent" default, matching
/// pre-slice behavior.
///
/// Kept in `vedit.rs` so all desktop call sites (the apply_edl write
/// path in `proposal.rs`, the auto-insert path in `auto_insert.rs`,
/// the restore-audit commit here) share one decision point — DRY rule:
/// every piece of logic should have a single, unambiguous,
/// authoritative representation.
pub(crate) fn desktop_commit_author() -> Option<montage_core::vc::CommitAuthor> {
    montage_core::vc::CommitAuthor::from_env()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    fn write_otio(project_root: &std::path::Path, duration: f64) {
        let value = serde_json::json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": "test",
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "name": "tracks",
                "children": [{
                    "OTIO_SCHEMA": "Track.1",
                    "name": "V1",
                    "kind": "Video",
                    "children": [{
                        "OTIO_SCHEMA": "Clip.2",
                        "name": "shot-a",
                        "source_range": {
                            "OTIO_SCHEMA": "TimeRange.1",
                            "start_time": {"OTIO_SCHEMA": "RationalTime.1", "value": 0.0, "rate": 24.0},
                            "duration": {"OTIO_SCHEMA": "RationalTime.1", "value": duration, "rate": 24.0}
                        },
                        "media_reference": {
                            "OTIO_SCHEMA": "ExternalReference.1",
                            "target_url": "raw/foo.mp4"
                        }
                    }]
                }]
            }
        });
        std::fs::write(
            project_root.join("project.otio.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn merge_preflight_response_reports_overlap_without_merging() {
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0);
        let repo = montage_core::vc::open_or_init(dir.path()).unwrap();
        let base = montage_core::vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        montage_core::vc::create_branch(&repo, "alt-tight", Some(&base.commit_hash)).unwrap();

        write_otio(dir.path(), 120.0);
        montage_core::vc::commit_current_timeline(&repo, "Trim shot-a on main", None).unwrap();

        montage_core::vc::checkout_branch(&repo, "alt-tight").unwrap();
        write_otio(dir.path(), 180.0);
        montage_core::vc::commit_current_timeline(&repo, "Trim shot-a on alternate", None).unwrap();
        let preflight = montage_core::vc::merge_preflight(
            &repo,
            "alt-tight",
            Some(montage_core::vc::DEFAULT_BRANCH),
        )
        .unwrap();

        let response = super::merge_preflight_response(preflight);

        assert!(!response.is_mergeable);
        assert_eq!(response.overlapping_clip_ids, ["raw/foo.mp4", "shot-a"]);
        assert_eq!(
            response.next_step,
            "Resolve overlapping clip ids manually before merging."
        );
    }

    #[test]
    fn changed_clip_ids_response_sorts_ids_and_counts_diff_shapes() {
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0);
        let repo = montage_core::vc::open_or_init(dir.path()).unwrap();
        let first = montage_core::vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        write_otio(dir.path(), 120.0);
        let second = montage_core::vc::commit_current_timeline(&repo, "Trim shot-a", None).unwrap();
        let diff =
            montage_core::vc::diff_refs(&repo, Some(&first.commit_hash), Some(&second.commit_hash))
                .unwrap();

        let response = super::changed_clip_ids_response(diff);

        assert_eq!(response.changed_clip_count, 2);
        assert_eq!(response.changed_clip_ids, ["raw/foo.mp4", "shot-a"]);
        assert_eq!(response.structural_change_count, 1);
        assert_eq!(response.animation_change_count, 0);
    }

    #[test]
    fn diff_response_includes_changed_clip_ids_for_review_rows() {
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0);
        let repo = montage_core::vc::open_or_init(dir.path()).unwrap();
        let first = montage_core::vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        write_otio(dir.path(), 120.0);
        let second = montage_core::vc::commit_current_timeline(&repo, "Trim shot-a", None).unwrap();
        let diff =
            montage_core::vc::diff_refs(&repo, Some(&first.commit_hash), Some(&second.commit_hash))
                .unwrap();

        let response = super::diff_response(diff).unwrap();

        assert_eq!(response.change_count, 1);
        assert_eq!(response.changed_clip_count, 2);
        assert_eq!(response.changed_clip_ids, ["raw/foo.mp4", "shot-a"]);
    }

    #[test]
    fn restore_rejects_a_stale_expected_head() {
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0);
        let repo = montage_core::vc::open_or_init(dir.path()).unwrap();
        let first = montage_core::vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        write_otio(dir.path(), 120.0);
        montage_core::vc::commit_current_timeline(&repo, "Trim shot-a", None).unwrap();

        let error = super::ensure_expected_current(&repo, &first.commit_hash).unwrap_err();

        assert!(error.contains("timeline changed before restore"));
    }

    #[test]
    fn restore_rejects_uncommitted_working_timeline_changes() {
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0);
        let repo = montage_core::vc::open_or_init(dir.path()).unwrap();
        let current = montage_core::vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        write_otio(dir.path(), 120.0);

        let error = super::ensure_expected_current(&repo, &current.commit_hash).unwrap_err();

        assert!(error.contains("working copy has uncommitted edits"));
    }

    // ---- desktop author attribution -----------------------------------
    // Regression guard for the follow-up to A3: the desktop apply_edl
    // write paths (and the restore-audit commit) must stamp the
    // env-configured seat-holder on the commit, NOT the "montage agent"
    // default. We exercise the author-resolution helper directly +
    // the `_as` commit entry point — the same code path the handlers
    // use after this slice.

    #[test]
    fn desktop_commit_author_resolves_from_env_callback() {
        let env = |k: &str| match k {
            "MONTAGE_USER_NAME" => Some("Alice".to_string()),
            "MONTAGE_USER_EMAIL" => Some("alice@example.com".to_string()),
            _ => None,
        };
        let author = montage_core::vc::CommitAuthor::from_env_with(env)
            .expect("env-resolved author must be present");
        assert_eq!(author.name, "Alice");
        assert_eq!(author.email, "alice@example.com");
    }

    #[test]
    fn desktop_commit_author_is_none_when_env_unset() {
        let env = |_: &str| None::<String>;
        assert!(montage_core::vc::CommitAuthor::from_env_with(env).is_none());
    }

    #[test]
    fn desktop_commit_author_is_none_when_env_partial() {
        // Half-configured env (only the name) must NOT pair a real
        // name with a missing email — mirror resolver semantics.
        let half_env = |k: &str| match k {
            "MONTAGE_USER_NAME" => Some("Alice".to_string()),
            _ => None,
        };
        assert!(montage_core::vc::CommitAuthor::from_env_with(half_env).is_none());
    }

    #[test]
    fn auto_commit_apply_as_attributes_to_env_seat_holder_not_agent() {
        // Mirror what the desktop write path does after this slice:
        // resolve the seat author from env, then call the `_as` entry
        // point so the commit is stamped with the user, not the agent.
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0);
        let repo = montage_core::vc::open_or_init(dir.path()).unwrap();

        let env = |k: &str| match k {
            "MONTAGE_USER_NAME" => Some("Alice".to_string()),
            "MONTAGE_USER_EMAIL" => Some("alice@example.com".to_string()),
            _ => None,
        };
        let seat_author = montage_core::vc::CommitAuthor::from_env_with(env);
        assert!(seat_author.is_some(), "env should resolve to an author");

        let descriptions = vec!["Trim shot-a by 1.0s".to_string()];
        let outcome = montage_core::vc::auto_commit_apply_as(
            &repo,
            &descriptions,
            Some("desktop apply_edl test"),
            seat_author,
        )
        .expect("auto_commit_apply_as");

        let entries = montage_core::vc::log(&repo, 1).unwrap();
        let head = entries.first().expect("at least one commit");
        assert_eq!(head.commit_hash, outcome.commit_hash);
        assert_eq!(
            head.author.name, "Alice",
            "desktop apply_edl must attribute to the seat-holder, not 'montage agent'"
        );
        assert_eq!(head.author.email, "alice@example.com");
    }

    #[test]
    fn auto_commit_apply_as_falls_back_to_default_when_no_seat_author() {
        // Negative control: when env is empty the helper returns None,
        // and the `_as` resolver chain falls back to the default.
        let dir = tempfile::tempdir().unwrap();
        write_otio(dir.path(), 240.0);
        let repo = montage_core::vc::open_or_init(dir.path()).unwrap();

        let none_env = |_: &str| None::<String>;
        let seat_author = montage_core::vc::CommitAuthor::from_env_with(none_env);
        assert!(seat_author.is_none());

        montage_core::vc::auto_commit_apply_as(
            &repo,
            &["Trim shot-a by 0.5s".to_string()],
            None,
            seat_author,
        )
        .unwrap();
        let head = montage_core::vc::log(&repo, 1).unwrap().pop().unwrap();
        assert_eq!(head.author.name, "montage agent");
        assert_eq!(head.author.email, "agent@montage.local");
    }
}
