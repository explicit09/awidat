//! Agent-callable asset catalog and source review mutators.

use async_trait::async_trait;
use awidat_proto::professional::{SelectDecision, SourceRange, SourceSelect};
use awidat_proto::project::Project;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::media_catalog_mutation::{
    CatalogMutationError, create_bin, ensure_asset_catalog, ensure_awidat_metadata,
    move_asset_to_bin,
};
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Create a durable asset bin.
pub struct CreateBinTool;
/// Move an asset to a durable bin.
pub struct MoveToBinTool;
/// Set an asset display label.
pub struct RenameAssetTool;
/// Add or remove asset tags.
pub struct TagAssetTool;
/// Set an asset rating.
pub struct RateAssetTool;
/// Create or update a durable source select.
pub struct MarkSelectTool;

#[derive(Debug, Deserialize)]
struct CreateBinArgs {
    id: String,
    name: String,
    #[serde(default)]
    parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MoveToBinArgs {
    asset_id: String,
    #[serde(default)]
    bin_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RenameAssetArgs {
    asset_id: String,
    label: String,
}

#[derive(Debug, Deserialize)]
struct TagAssetArgs {
    asset_id: String,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RateAssetArgs {
    asset_id: String,
    rating: u8,
}

#[derive(Debug, Deserialize)]
struct MarkSelectArgs {
    asset_id: String,
    start_s: f64,
    end_s: f64,
    #[serde(default)]
    decision: Option<SelectDecision>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    notes: Vec<String>,
}

macro_rules! simple_mutating_tool {
    ($tool:ty, $name:literal, $description:literal, $schema:expr, $args:ty, $handler:ident, $op:literal) => {
        #[async_trait]
        impl ToolHandler for $tool {
            fn name(&self) -> &'static str {
                $name
            }

            fn schema(&self) -> ToolSchema {
                ToolSchema {
                    name: $name.into(),
                    description: $description.into(),
                    input_schema: $schema,
                    ..Default::default()
                }
            }

            fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
                true
            }

            fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
                vec![ApprovalKey::new(
                    self.name(),
                    format!("{}:{}", $op, invocation.args.to_string()),
                )]
            }

            async fn handle(
                &self,
                invocation: ToolInvocation,
                ctx: ToolContext,
            ) -> Result<ToolOutput, FunctionCallError> {
                let args: $args = serde_json::from_value(invocation.args).map_err(|e| {
                    FunctionCallError::RespondToModel(format!("{}: invalid args ({e})", $name))
                })?;
                $handler(args, ctx)
            }
        }
    };
}

simple_mutating_tool!(
    CreateBinTool,
    "create_bin",
    "Create a durable user/agent asset bin in the project catalog.",
    serde_json::json!({
        "type": "object",
        "required": ["id", "name"],
        "properties": {
            "id": {"type": "string"},
            "name": {"type": "string"},
            "parent_id": {"type": "string"}
        }
    }),
    CreateBinArgs,
    handle_create_bin,
    "bin"
);

simple_mutating_tool!(
    MoveToBinTool,
    "move_to_bin",
    "Move a catalog asset into a durable bin, or clear its bin.",
    serde_json::json!({
        "type": "object",
        "required": ["asset_id"],
        "properties": {
            "asset_id": {"type": "string"},
            "bin_id": {"type": "string"}
        }
    }),
    MoveToBinArgs,
    handle_move_to_bin,
    "asset"
);

simple_mutating_tool!(
    RenameAssetTool,
    "rename_asset",
    "Set the human-readable label for a catalog asset without renaming the source file.",
    serde_json::json!({
        "type": "object",
        "required": ["asset_id", "label"],
        "properties": {
            "asset_id": {"type": "string"},
            "label": {"type": "string"}
        }
    }),
    RenameAssetArgs,
    handle_rename_asset,
    "asset"
);

simple_mutating_tool!(
    TagAssetTool,
    "tag_asset",
    "Add or remove durable tags on a catalog asset.",
    serde_json::json!({
        "type": "object",
        "required": ["asset_id"],
        "properties": {
            "asset_id": {"type": "string"},
            "add": {"type": "array", "items": {"type": "string"}},
            "remove": {"type": "array", "items": {"type": "string"}}
        }
    }),
    TagAssetArgs,
    handle_tag_asset,
    "asset"
);

simple_mutating_tool!(
    RateAssetTool,
    "rate_asset",
    "Set an asset rating from 0 to 5.",
    serde_json::json!({
        "type": "object",
        "required": ["asset_id", "rating"],
        "properties": {
            "asset_id": {"type": "string"},
            "rating": {"type": "integer", "minimum": 0, "maximum": 5}
        }
    }),
    RateAssetArgs,
    handle_rate_asset,
    "asset"
);

simple_mutating_tool!(
    MarkSelectTool,
    "mark_select",
    "Create or update a durable source select for an asset range.",
    serde_json::json!({
        "type": "object",
        "required": ["asset_id", "start_s", "end_s"],
        "properties": {
            "asset_id": {"type": "string"},
            "start_s": {"type": "number"},
            "end_s": {"type": "number"},
            "decision": {"type": "string", "enum": ["select", "reject", "maybe", "favorite"]},
            "id": {"type": "string"},
            "reason": {"type": "string"},
            "notes": {"type": "array", "items": {"type": "string"}}
        }
    }),
    MarkSelectArgs,
    handle_mark_select,
    "select"
);

fn handle_create_bin(
    args: CreateBinArgs,
    ctx: ToolContext,
) -> Result<ToolOutput, FunctionCallError> {
    mutate_project(&ctx, |project| {
        let meta = ensure_awidat_metadata(&mut project.timeline);
        create_bin(meta, args.id, args.name, args.parent_id).map_err(catalog_error)?;
        Ok(serde_json::json!({"status": "created"}))
    })
}

fn handle_move_to_bin(
    args: MoveToBinArgs,
    ctx: ToolContext,
) -> Result<ToolOutput, FunctionCallError> {
    mutate_project(&ctx, |project| {
        let meta = ensure_awidat_metadata(&mut project.timeline);
        move_asset_to_bin(meta, &args.asset_id, args.bin_id).map_err(catalog_error)?;
        Ok(serde_json::json!({"status": "moved", "asset_id": args.asset_id}))
    })
}

fn handle_rename_asset(
    args: RenameAssetArgs,
    ctx: ToolContext,
) -> Result<ToolOutput, FunctionCallError> {
    if args.label.trim().is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "rename_asset: label must not be empty".into(),
        ));
    }
    mutate_project(&ctx, |project| {
        let meta = ensure_awidat_metadata(&mut project.timeline);
        let catalog = ensure_asset_catalog(meta);
        let asset = catalog
            .assets
            .iter_mut()
            .find(|asset| asset.id == args.asset_id)
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(format!("asset {} does not exist", args.asset_id))
            })?;
        asset.label = Some(args.label);
        Ok(serde_json::json!({"status": "renamed", "asset_id": args.asset_id}))
    })
}

fn handle_tag_asset(args: TagAssetArgs, ctx: ToolContext) -> Result<ToolOutput, FunctionCallError> {
    mutate_project(&ctx, |project| {
        let meta = ensure_awidat_metadata(&mut project.timeline);
        let catalog = ensure_asset_catalog(meta);
        let asset = catalog
            .assets
            .iter_mut()
            .find(|asset| asset.id == args.asset_id)
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(format!("asset {} does not exist", args.asset_id))
            })?;
        asset
            .tags
            .retain(|tag| !args.remove.iter().any(|remove| remove == tag));
        for tag in args.add {
            let tag = tag.trim();
            if !tag.is_empty() && !asset.tags.iter().any(|existing| existing == tag) {
                asset.tags.push(tag.to_string());
            }
        }
        asset.tags.sort();
        Ok(
            serde_json::json!({"status": "tagged", "asset_id": args.asset_id, "tags": asset.tags.clone()}),
        )
    })
}

fn handle_rate_asset(
    args: RateAssetArgs,
    ctx: ToolContext,
) -> Result<ToolOutput, FunctionCallError> {
    if args.rating > 5 {
        return Err(FunctionCallError::RespondToModel(
            "rate_asset: rating must be in 0..=5".into(),
        ));
    }
    mutate_project(&ctx, |project| {
        let meta = ensure_awidat_metadata(&mut project.timeline);
        let catalog = ensure_asset_catalog(meta);
        let asset = catalog
            .assets
            .iter_mut()
            .find(|asset| asset.id == args.asset_id)
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(format!("asset {} does not exist", args.asset_id))
            })?;
        asset.rating = Some(args.rating);
        Ok(serde_json::json!({"status": "rated", "asset_id": args.asset_id, "rating": args.rating}))
    })
}

fn handle_mark_select(
    args: MarkSelectArgs,
    ctx: ToolContext,
) -> Result<ToolOutput, FunctionCallError> {
    let range = SourceRange {
        start_s: args.start_s,
        end_s: args.end_s,
    };
    if !range.is_valid() {
        return Err(FunctionCallError::RespondToModel(
            "mark_select: start_s must be finite and before end_s".into(),
        ));
    }
    mutate_project(&ctx, |project| {
        let meta = ensure_awidat_metadata(&mut project.timeline);
        {
            let catalog = ensure_asset_catalog(meta);
            if !catalog.assets.iter().any(|asset| asset.id == args.asset_id) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "asset {} does not exist",
                    args.asset_id
                )));
            }
        }
        let decision = args.decision.unwrap_or_default();
        let id = args.id.unwrap_or_else(|| {
            format!(
                "select-{}-{:.3}-{:.3}-{:?}",
                slug(&args.asset_id),
                range.start_s,
                range.end_s,
                decision
            )
            .to_ascii_lowercase()
        });
        let select = SourceSelect {
            id: id.clone(),
            asset_id: args.asset_id.clone(),
            range,
            decision,
            reason: args.reason,
            notes: args.notes,
            ..SourceSelect::default()
        };
        match meta.selects.iter_mut().find(|select| select.id == id) {
            Some(existing) => *existing = select,
            None => meta.selects.push(select),
        }
        let catalog = ensure_asset_catalog(meta);
        if let Some(asset) = catalog
            .assets
            .iter_mut()
            .find(|asset| asset.id == args.asset_id)
            && !asset
                .usage
                .select_ids
                .iter()
                .any(|select_id| select_id == &id)
        {
            asset.usage.select_ids.push(id.clone());
            asset.usage.select_ids.sort();
        }
        Ok(serde_json::json!({"status": "marked", "select_id": id}))
    })
}

fn mutate_project(
    ctx: &ToolContext,
    mutate: impl FnOnce(&mut Project) -> Result<serde_json::Value, FunctionCallError>,
) -> Result<ToolOutput, FunctionCallError> {
    let mut project = Project::read(&ctx.project_root).map_err(|e| {
        FunctionCallError::RespondToModel(format!("manage_assets: unable to read project: {e}"))
    })?;
    let body = mutate(&mut project)?;
    project.write(&ctx.project_root).map_err(|e| {
        FunctionCallError::Fatal(format!("manage_assets: unable to write project: {e}"))
    })?;
    Ok(ToolOutput::text(body.to_string()))
}

fn catalog_error(error: CatalogMutationError) -> FunctionCallError {
    FunctionCallError::RespondToModel(error.to_string())
}

fn slug(input: &str) -> String {
    input
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use awidat_proto::professional::{AssetReadiness, AssetRecord, AssetRole, SelectDecision};
    use awidat_proto::project::Project;
    use tokio::sync::broadcast;

    use super::*;
    use crate::media_catalog_mutation::{ensure_awidat_metadata, upsert_asset};

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: awidat_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(awidat_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: std::sync::Arc::new(crate::skills::SkillRegistry::default()),
            subagent_return: None,
        }
    }

    fn invocation(name: &str, args: serde_json::Value) -> ToolInvocation {
        ToolInvocation {
            call_id: "c1".into(),
            name: name.into(),
            args,
        }
    }

    fn init_project_with_asset(root: &std::path::Path) {
        let mut project = Project::init(root).unwrap();
        let meta = ensure_awidat_metadata(&mut project.timeline);
        upsert_asset(
            meta,
            AssetRecord {
                id: "raw/a.mp4".into(),
                path: "raw/a.mp4".into(),
                role: AssetRole::Video,
                readiness: AssetReadiness::default(),
                ..AssetRecord::default()
            },
        );
        project.write(root).unwrap();
    }

    #[tokio::test]
    async fn create_bin_and_move_to_bin_persist_catalog_changes() {
        let dir = tempfile::tempdir().unwrap();
        init_project_with_asset(dir.path());

        CreateBinTool
            .handle(
                invocation(
                    "create_bin",
                    serde_json::json!({"id": "scene-1", "name": "Scene 1"}),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        MoveToBinTool
            .handle(
                invocation(
                    "move_to_bin",
                    serde_json::json!({"asset_id": "raw/a.mp4", "bin_id": "scene-1"}),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let project = Project::read(dir.path()).unwrap();
        let catalog = project
            .timeline
            .metadata
            .awidat
            .as_ref()
            .unwrap()
            .asset_catalog
            .as_ref()
            .unwrap();
        assert_eq!(catalog.bins[0].id, "scene-1");
        assert_eq!(catalog.assets[0].bin_id.as_deref(), Some("scene-1"));
    }

    #[tokio::test]
    async fn rename_tag_and_rate_asset_persist_metadata() {
        let dir = tempfile::tempdir().unwrap();
        init_project_with_asset(dir.path());

        RenameAssetTool
            .handle(
                invocation(
                    "rename_asset",
                    serde_json::json!({"asset_id": "raw/a.mp4", "label": "Hero take"}),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        TagAssetTool
            .handle(
                invocation(
                    "tag_asset",
                    serde_json::json!({
                        "asset_id": "raw/a.mp4",
                        "add": ["wide", "wide", "hero"],
                        "remove": ["missing"]
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        RateAssetTool
            .handle(
                invocation(
                    "rate_asset",
                    serde_json::json!({"asset_id": "raw/a.mp4", "rating": 5}),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();

        let project = Project::read(dir.path()).unwrap();
        let asset = &project
            .timeline
            .metadata
            .awidat
            .as_ref()
            .unwrap()
            .asset_catalog
            .as_ref()
            .unwrap()
            .assets[0];
        assert_eq!(asset.label.as_deref(), Some("Hero take"));
        assert_eq!(asset.tags, vec!["hero".to_string(), "wide".to_string()]);
        assert_eq!(asset.rating, Some(5));
    }

    #[tokio::test]
    async fn mark_select_persists_select_and_links_asset_usage() {
        let dir = tempfile::tempdir().unwrap();
        init_project_with_asset(dir.path());

        let output = MarkSelectTool
            .handle(
                invocation(
                    "mark_select",
                    serde_json::json!({
                        "asset_id": "raw/a.mp4",
                        "start_s": 1.0,
                        "end_s": 3.5,
                        "decision": "favorite",
                        "reason": "clean delivery",
                        "notes": ["good eye line"]
                    }),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        let select_id = body["select_id"].as_str().unwrap();

        let project = Project::read(dir.path()).unwrap();
        let meta = project.timeline.metadata.awidat.as_ref().unwrap();
        assert_eq!(meta.selects.len(), 1);
        assert_eq!(meta.selects[0].decision, SelectDecision::Favorite);
        assert_eq!(meta.selects[0].reason.as_deref(), Some("clean delivery"));
        assert_eq!(
            meta.asset_catalog.as_ref().unwrap().assets[0]
                .usage
                .select_ids,
            vec![select_id.to_string()]
        );
    }

    #[tokio::test]
    async fn rate_asset_rejects_out_of_range_rating() {
        let dir = tempfile::tempdir().unwrap();
        init_project_with_asset(dir.path());

        let err = RateAssetTool
            .handle(
                invocation(
                    "rate_asset",
                    serde_json::json!({"asset_id": "raw/a.mp4", "rating": 6}),
                ),
                ctx_at(dir.path()),
            )
            .await
            .unwrap_err();

        match err {
            FunctionCallError::RespondToModel(message) => {
                assert!(message.contains("0..=5"), "got: {message}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
