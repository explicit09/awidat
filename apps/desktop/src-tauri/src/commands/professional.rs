//! Professional workflow lens and orchestration review commands.

use awidat_core::professional::{
    OrchestrationInspection, WorkflowLensSnapshot, build_workflow_lens_snapshots,
    inspect_pre_autonomy_readiness,
};
use awidat_proto::project::Project;
use tauri::State;

use crate::state::AwidatState;

/// Read thin workflow lens snapshots for the loaded project.
#[tauri::command]
pub async fn read_professional_lenses(
    state: State<'_, AwidatState>,
) -> Result<Vec<WorkflowLensSnapshot>, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    tokio::task::spawn_blocking(move || {
        let project = Project::read(&project_root).map_err(|e| format!("project read: {e}"))?;
        let metadata = project.timeline.metadata.awidat.unwrap_or_default();
        Ok(build_workflow_lens_snapshots(&metadata))
    })
    .await
    .map_err(|e| format!("professional lenses join: {e}"))?
}

/// Read pre-autonomy readiness, planner pass edges, and conflicts.
#[tauri::command]
pub async fn read_pre_autonomy_inspection(
    state: State<'_, AwidatState>,
) -> Result<OrchestrationInspection, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    tokio::task::spawn_blocking(move || {
        let project = Project::read(&project_root).map_err(|e| format!("project read: {e}"))?;
        let metadata = project.timeline.metadata.awidat.unwrap_or_default();
        Ok(inspect_pre_autonomy_readiness(&metadata))
    })
    .await
    .map_err(|e| format!("pre-autonomy inspection join: {e}"))?
}
