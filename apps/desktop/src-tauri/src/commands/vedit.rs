//! Desktop-facing vedit history commands.

use serde::Serialize;
use tauri::{AppHandle, State};

use crate::events::emit_timeline_changed;
use crate::state::AwidatState;

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
    state: State<'_, AwidatState>,
    limit: Option<usize>,
) -> Result<Vec<VeditCommitEntry>, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let entries = awidat_core::vc::log(&repo, limit.unwrap_or(30).min(200))
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
    state: State<'_, AwidatState>,
    from_ref: Option<String>,
    to_ref: Option<String>,
) -> Result<VeditDiffResponse, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let diff = awidat_core::vc::diff_refs(&repo, from_ref.as_deref(), to_ref.as_deref())
        .map_err(|e| format!("read vedit diff: {e}"))?;
    diff_response(diff)
}

#[tauri::command]
pub async fn changed_vedit_clip_ids(
    state: State<'_, AwidatState>,
    from_ref: Option<String>,
    to_ref: Option<String>,
) -> Result<VeditChangedClipIdsResponse, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let diff = awidat_core::vc::diff_refs(&repo, from_ref.as_deref(), to_ref.as_deref())
        .map_err(|e| format!("read vedit changed clip ids: {e}"))?;
    Ok(changed_clip_ids_response(diff))
}

#[tauri::command]
pub async fn preflight_vedit_merge(
    state: State<'_, AwidatState>,
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
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let preflight = awidat_core::vc::merge_preflight(&repo, &source_ref, target_ref)
        .map_err(|e| format!("preflight vedit merge: {e}"))?;
    Ok(merge_preflight_response(preflight))
}

#[tauri::command]
pub async fn list_vedit_tags(state: State<'_, AwidatState>) -> Result<Vec<VeditTagEntry>, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let tags = awidat_core::vc::list_tags(&repo).map_err(|e| format!("read vedit tags: {e}"))?;
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
    state: State<'_, AwidatState>,
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
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let refstr = refstr.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let tag = awidat_core::vc::tag_ref(&repo, &name, refstr)
        .map_err(|e| format!("write vedit tag: {e}"))?;
    Ok(VeditTagEntry {
        name: tag.name,
        target: tag.target,
    })
}

#[tauri::command]
pub async fn list_vedit_branches(
    state: State<'_, AwidatState>,
) -> Result<Vec<VeditBranchEntry>, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let branches =
        awidat_core::vc::list_branches(&repo).map_err(|e| format!("read vedit branches: {e}"))?;
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
    state: State<'_, AwidatState>,
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
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let start_ref = start_ref
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let branch = awidat_core::vc::create_branch(&repo, &name, start_ref)
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
    state: State<'_, AwidatState>,
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
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let checkout = awidat_core::vc::checkout_branch(&repo, &branch)
        .map_err(|e| format!("checkout vedit branch: {e}"))?;
    emit_timeline_changed(&app, &project_root);
    Ok(VeditCheckoutResponse {
        branch: checkout.branch,
        commit_hash: checkout.commit_hash,
        timeline_hash: checkout.timeline_hash,
    })
}

#[tauri::command]
pub async fn show_vedit_commit(
    state: State<'_, AwidatState>,
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
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let details = awidat_core::vc::show_commit(&repo, &refstr)
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
    state: State<'_, AwidatState>,
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
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let limit = limit.unwrap_or(200).min(500);
    let entries = awidat_core::vc::blame_clip(&repo, &clip_id, start_ref.as_deref(), limit)
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
    state: State<'_, AwidatState>,
    refstr: String,
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
    let repo = awidat_core::vc::open_or_init(&project_root)
        .map_err(|e| format!("open vedit repo: {e}"))?;
    let restored = awidat_core::vc::restore_working_timeline(&repo, &refstr)
        .map_err(|e| format!("restore vedit ref: {e}"))?;
    let header = format!("Restore timeline to {}", short_hash(&restored.commit_hash));
    let audit = awidat_core::vc::commit_current_timeline(
        &repo,
        &header,
        Some("Restored project.otio.json from the desktop timeline history panel."),
    )
    .map_err(|e| format!("commit restore audit: {e}"))?;
    emit_timeline_changed(&app, &project_root);
    Ok(VeditRestoreResponse {
        restored_ref: restored.requested_ref,
        restored_commit_hash: restored.commit_hash,
        restored_timeline_hash: restored.timeline_hash,
        audit_commit_hash: Some(audit.commit_hash),
    })
}

fn short_hash(hash: &str) -> String {
    hash.strip_prefix("sha256:")
        .unwrap_or(hash)
        .chars()
        .take(7)
        .collect()
}

fn merge_preflight_response(
    preflight: awidat_core::vc::MergePreflight,
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

fn changed_clip_ids_response(diff: awidat_core::vc::CommittedDiff) -> VeditChangedClipIdsResponse {
    let changed_clip_ids = awidat_core::vc::changed_clip_ids(&diff)
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

fn diff_response(diff: awidat_core::vc::CommittedDiff) -> Result<VeditDiffResponse, String> {
    let change_count = diff.len();
    let changed_clip_ids = awidat_core::vc::changed_clip_ids(&diff)
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

#[cfg(test)]
mod tests {
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
        let repo = awidat_core::vc::open_or_init(dir.path()).unwrap();
        let base = awidat_core::vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        awidat_core::vc::create_branch(&repo, "alt-tight", Some(&base.commit_hash)).unwrap();

        write_otio(dir.path(), 120.0);
        awidat_core::vc::commit_current_timeline(&repo, "Trim shot-a on main", None).unwrap();

        awidat_core::vc::checkout_branch(&repo, "alt-tight").unwrap();
        write_otio(dir.path(), 180.0);
        awidat_core::vc::commit_current_timeline(&repo, "Trim shot-a on alternate", None).unwrap();
        let preflight = awidat_core::vc::merge_preflight(
            &repo,
            "alt-tight",
            Some(awidat_core::vc::DEFAULT_BRANCH),
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
        let repo = awidat_core::vc::open_or_init(dir.path()).unwrap();
        let first = awidat_core::vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        write_otio(dir.path(), 120.0);
        let second = awidat_core::vc::commit_current_timeline(&repo, "Trim shot-a", None).unwrap();
        let diff =
            awidat_core::vc::diff_refs(&repo, Some(&first.commit_hash), Some(&second.commit_hash))
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
        let repo = awidat_core::vc::open_or_init(dir.path()).unwrap();
        let first = awidat_core::vc::commit_current_timeline(&repo, "Initial", None).unwrap();
        write_otio(dir.path(), 120.0);
        let second = awidat_core::vc::commit_current_timeline(&repo, "Trim shot-a", None).unwrap();
        let diff =
            awidat_core::vc::diff_refs(&repo, Some(&first.commit_hash), Some(&second.commit_hash))
                .unwrap();

        let response = super::diff_response(diff).unwrap();

        assert_eq!(response.change_count, 1);
        assert_eq!(response.changed_clip_count, 2);
        assert_eq!(response.changed_clip_ids, ["raw/foo.mp4", "shot-a"]);
    }
}
