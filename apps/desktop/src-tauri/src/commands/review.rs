use awidat_core::review::LocalReviewPackage;
use serde::Deserialize;
use tauri::State;

use crate::state::AwidatState;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalReviewPackageResponse {
    /// Absolute or project-relative path to the render used to produce this package.
    pub render_path: String,
    /// RFC3339 UTC timestamp for when the package was generated.
    pub generated_at: String,
    /// Commit hash of the referenced vedit revision.
    pub vedit_commit: String,
    /// Timeline hash for the referenced commit.
    pub timeline_hash: String,
    /// Tags applied by the caller at package generation time.
    pub tags: Vec<String>,
    /// Parsed header from the referenced vedit commit.
    pub commit_header: String,
    /// Parsed body from the vedit commit when present.
    pub reasoning_body: String,
    /// Absolute path to the persisted package JSON file.
    pub package_path: String,
}

impl From<LocalReviewPackage> for LocalReviewPackageResponse {
    fn from(package: LocalReviewPackage) -> Self {
        Self {
            render_path: package.render_path,
            generated_at: package.generated_at,
            vedit_commit: package.vedit_commit,
            timeline_hash: package.timeline_hash,
            tags: package.tags,
            commit_header: package.commit_header,
            reasoning_body: package.reasoning_body,
            package_path: package.package_path,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthorLocalReviewPackageArgs {
    /// Render asset to reference. Can be absolute or project-relative.
    pub render_path: String,
    /// Optional tags for discoverability and filtering.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Build a local review package for a rendered output path.
#[tauri::command]
pub async fn author_local_review_package(
    state: State<'_, AwidatState>,
    args: AuthorLocalReviewPackageArgs,
) -> Result<LocalReviewPackageResponse, String> {
    if args.render_path.trim().is_empty() {
        return Err("render_path cannot be empty".into());
    }

    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    let package = awidat_core::review::build_local_review_package(
        &project_root,
        &args.render_path,
        args.tags,
    )
    .map_err(|e| format!("author_local_review_package: {e}"))?;

    Ok(package.into())
}
