//! Agent-callable media ingest tools.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use awidat_proto::professional::{
    AssetProvenance, AssetReadiness, AssetRecord, AssetRole, ReadinessState,
};
use awidat_proto::project::Project;
use serde::Deserialize;
use tokio::process::Command;

use crate::FunctionCallError;
use crate::media_catalog_mutation::{ensure_awidat_metadata, upsert_asset};
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// Import a local file into `raw/`.
pub struct ImportLocalTool;

/// Import a URL into `raw/` via `yt-dlp`.
pub struct ImportUrlTool;

#[derive(Debug, Deserialize)]
struct ImportLocalArgs {
    source_path: String,
    #[serde(default)]
    destination_name: Option<String>,
    #[serde(default)]
    link: bool,
}

#[derive(Debug, Deserialize)]
struct ImportUrlArgs {
    url: String,
    #[serde(default)]
    destination_name: Option<String>,
}

#[async_trait]
impl ToolHandler for ImportLocalTool {
    fn name(&self) -> &'static str {
        "import_local"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "import_local".into(),
            description: "Import a local media file into the project's raw/ directory and record it in the durable asset catalog.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["source_path"],
                "properties": {
                    "source_path": {
                        "type": "string",
                        "description": "Absolute path to an existing local media file."
                    },
                    "destination_name": {
                        "type": "string",
                        "description": "Optional safe file name to use under raw/. Defaults to the source file name."
                    },
                    "link": {
                        "type": "boolean",
                        "description": "Create a symlink instead of copying. Default false."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        vec![ApprovalKey::new(
            self.name(),
            format!(
                "source:{}",
                invocation.args["source_path"].as_str().unwrap_or("")
            ),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: ImportLocalArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "import_local: invalid args ({e}). Expected source_path, optional destination_name, optional link."
            ))
        })?;
        let source = PathBuf::from(args.source_path);
        if !source.is_absolute() {
            return Err(FunctionCallError::RespondToModel(
                "import_local: source_path must be absolute".into(),
            ));
        }
        if !source.is_file() {
            return Err(FunctionCallError::RespondToModel(format!(
                "import_local: source_path is not a file: {}",
                source.display()
            )));
        }

        let raw_dir = ctx.project_root.join("raw");
        std::fs::create_dir_all(&raw_dir).map_err(|e| {
            FunctionCallError::Fatal(format!("import_local: unable to create raw/: {e}"))
        })?;
        let file_name = safe_destination_name(
            args.destination_name.as_deref(),
            source.file_name().and_then(|name| name.to_str()),
            "import_local",
        )?;
        let destination = raw_dir.join(file_name);
        if destination.exists() {
            return Err(FunctionCallError::RespondToModel(format!(
                "import_local: destination already exists: {}",
                destination.display()
            )));
        }

        if args.link {
            symlink_file(&source, &destination).map_err(|e| {
                FunctionCallError::Fatal(format!("import_local: symlink failed: {e}"))
            })?;
        } else {
            std::fs::copy(&source, &destination)
                .map_err(|e| FunctionCallError::Fatal(format!("import_local: copy failed: {e}")))?;
        }

        let rel_path = project_relative_path(&ctx.project_root, &destination)?;
        let size_bytes = destination.metadata().map(|meta| meta.len()).unwrap_or(0);
        record_imported_asset(
            &ctx.project_root,
            &rel_path,
            Some(source.to_string_lossy().to_string()),
            "import_local",
        )?;

        Ok(ToolOutput::text(
            serde_json::json!({
                "status": "imported",
                "asset_id": rel_path,
                "path": rel_path,
                "size_bytes": size_bytes,
                "linked": args.link
            })
            .to_string(),
        ))
    }
}

#[async_trait]
impl ToolHandler for ImportUrlTool {
    fn name(&self) -> &'static str {
        "import_url"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "import_url".into(),
            description: "Download a URL into the project's raw/ directory via yt-dlp and record it in the durable asset catalog.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to download with yt-dlp."
                    },
                    "destination_name": {
                        "type": "string",
                        "description": "Optional safe output file name under raw/. When omitted, yt-dlp chooses a restricted title-based name."
                    }
                }
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        true
    }

    fn approval_keys(&self, invocation: &ToolInvocation) -> Vec<ApprovalKey> {
        vec![ApprovalKey::new(
            self.name(),
            format!("url:{}", invocation.args["url"].as_str().unwrap_or("")),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: ImportUrlArgs = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "import_url: invalid args ({e}). Expected url and optional destination_name."
            ))
        })?;
        if !(args.url.starts_with("http://") || args.url.starts_with("https://")) {
            return Err(FunctionCallError::RespondToModel(
                "import_url: url must start with http:// or https://".into(),
            ));
        }
        let raw_dir = ctx.project_root.join("raw");
        std::fs::create_dir_all(&raw_dir).map_err(|e| {
            FunctionCallError::Fatal(format!("import_url: unable to create raw/: {e}"))
        })?;
        let before = existing_files(&raw_dir);
        let output_template = match args.destination_name.as_deref() {
            Some(name) => raw_dir.join(safe_file_name(name, "import_url")?),
            None => raw_dir.join("%(title).200B.%(ext)s"),
        };

        let status = Command::new("yt-dlp")
            .arg("--restrict-filenames")
            .arg("--no-part")
            .arg("-o")
            .arg(&output_template)
            .arg(&args.url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "import_url: unable to run yt-dlp ({e}); install yt-dlp or use import_local"
                ))
            })?;
        if !status.success() {
            return Err(FunctionCallError::RespondToModel(format!(
                "import_url: yt-dlp exited with status {status}"
            )));
        }

        let imported = new_files(&raw_dir, &before).map_err(|e| {
            FunctionCallError::Fatal(format!("import_url: unable to inspect raw/: {e}"))
        })?;
        let destination = imported.into_iter().next().ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "import_url: yt-dlp finished but no new file appeared under raw/".into(),
            )
        })?;
        let rel_path = project_relative_path(&ctx.project_root, &destination)?;
        let size_bytes = destination.metadata().map(|meta| meta.len()).unwrap_or(0);
        record_imported_asset(&ctx.project_root, &rel_path, Some(args.url), "import_url")?;

        Ok(ToolOutput::text(
            serde_json::json!({
                "status": "imported",
                "asset_id": rel_path,
                "path": rel_path,
                "size_bytes": size_bytes
            })
            .to_string(),
        ))
    }
}

fn record_imported_asset(
    project_root: &Path,
    rel_path: &str,
    imported_from: Option<String>,
    created_by: &str,
) -> Result<(), FunctionCallError> {
    let mut project = Project::read(project_root).map_err(|e| {
        FunctionCallError::RespondToModel(format!("import_media: unable to read project: {e}"))
    })?;
    let meta = ensure_awidat_metadata(&mut project.timeline);
    upsert_asset(
        meta,
        AssetRecord {
            id: rel_path.to_string(),
            path: rel_path.to_string(),
            role: asset_role_for_path(rel_path),
            readiness: AssetReadiness {
                proxy: ReadinessState::Pending,
                index: ReadinessState::Pending,
                online: ReadinessState::Ready,
            },
            provenance: Some(AssetProvenance {
                imported_from,
                checksum: None,
                created_by: Some(created_by.to_string()),
            }),
            ..AssetRecord::default()
        },
    );
    project.write(project_root).map_err(|e| {
        FunctionCallError::Fatal(format!("import_media: unable to write project: {e}"))
    })
}

fn asset_role_for_path(path: &str) -> AssetRole {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("wav" | "aif" | "aiff" | "mp3" | "m4a" | "flac" | "ogg") => AssetRole::Audio,
        Some("png" | "jpg" | "jpeg" | "tif" | "tiff" | "webp" | "heic") => AssetRole::Still,
        Some("psd" | "ai" | "svg") => AssetRole::Graphic,
        Some("srt" | "vtt" | "itt" | "stl") => AssetRole::Caption,
        Some("cube" | "cdl" | "json" | "xml" | "otio" | "edl" | "ale") => AssetRole::Support,
        _ => AssetRole::Video,
    }
}

fn safe_destination_name(
    requested: Option<&str>,
    fallback: Option<&str>,
    tool_name: &str,
) -> Result<String, FunctionCallError> {
    match requested.or(fallback) {
        Some(name) => safe_file_name(name, tool_name),
        None => Err(FunctionCallError::RespondToModel(format!(
            "{tool_name}: unable to determine destination file name"
        ))),
    }
}

fn safe_file_name(name: &str, tool_name: &str) -> Result<String, FunctionCallError> {
    let path = Path::new(name);
    if name.trim().is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || name.contains("..")
        || name.contains('/')
        || name.contains('\\')
    {
        return Err(FunctionCallError::RespondToModel(format!(
            "{tool_name}: destination_name must be a single safe file name"
        )));
    }
    Ok(name.to_string())
}

fn project_relative_path(project_root: &Path, path: &Path) -> Result<String, FunctionCallError> {
    path.strip_prefix(project_root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| {
            FunctionCallError::Fatal(format!(
                "import_media: imported path escaped project root: {}",
                path.display()
            ))
        })
}

fn existing_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file())
                .collect()
        })
        .unwrap_or_default()
}

fn new_files(dir: &Path, before: &[PathBuf]) -> std::io::Result<Vec<PathBuf>> {
    let mut files = std::fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && !before.contains(path))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

#[cfg(unix)]
fn symlink_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, destination)
}

#[cfg(windows)]
fn symlink_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(source, destination)
}

#[cfg(test)]
mod tests {
    use awidat_proto::project::Project;
    use tokio::sync::broadcast;

    use super::*;

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

    #[test]
    fn safe_file_name_rejects_paths() {
        assert!(safe_file_name("../x.mp4", "test").is_err());
        assert!(safe_file_name("nested/x.mp4", "test").is_err());
        assert!(safe_file_name("/tmp/x.mp4", "test").is_err());
    }

    #[test]
    fn asset_role_for_path_detects_audio_and_support() {
        assert_eq!(asset_role_for_path("raw/a.wav"), AssetRole::Audio);
        assert_eq!(asset_role_for_path("raw/a.ale"), AssetRole::Support);
    }

    #[tokio::test]
    async fn import_local_copies_file_and_updates_catalog() {
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("camera.mov");
        std::fs::write(&source, b"media").unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        Project::init(project_dir.path()).unwrap();

        let output = ImportLocalTool
            .handle(
                invocation(
                    "import_local",
                    serde_json::json!({
                        "source_path": source,
                        "destination_name": "take-1.mov"
                    }),
                ),
                ctx_at(project_dir.path()),
            )
            .await
            .unwrap();

        let body: serde_json::Value = serde_json::from_str(&output.content).unwrap();
        assert_eq!(body["path"], "raw/take-1.mov");
        assert_eq!(
            std::fs::read(project_dir.path().join("raw/take-1.mov")).unwrap(),
            b"media"
        );
        let project = Project::read(project_dir.path()).unwrap();
        let meta = project.timeline.metadata.awidat.as_ref().unwrap();
        assert_eq!(meta.source_assets, vec!["raw/take-1.mov".to_string()]);
        let asset = &meta.asset_catalog.as_ref().unwrap().assets[0];
        assert_eq!(asset.id, "raw/take-1.mov");
        assert_eq!(
            asset.provenance.as_ref().unwrap().created_by.as_deref(),
            Some("import_local")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn import_local_can_symlink_file() {
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("camera.wav");
        std::fs::write(&source, b"audio").unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        Project::init(project_dir.path()).unwrap();

        ImportLocalTool
            .handle(
                invocation(
                    "import_local",
                    serde_json::json!({
                        "source_path": source,
                        "destination_name": "linked.wav",
                        "link": true
                    }),
                ),
                ctx_at(project_dir.path()),
            )
            .await
            .unwrap();

        let imported = project_dir.path().join("raw/linked.wav");
        assert_eq!(std::fs::read(&imported).unwrap(), b"audio");
        assert!(
            std::fs::symlink_metadata(imported)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[tokio::test]
    async fn import_local_rejects_unsafe_destination_name() {
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("camera.mov");
        std::fs::write(&source, b"media").unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        Project::init(project_dir.path()).unwrap();

        let err = ImportLocalTool
            .handle(
                invocation(
                    "import_local",
                    serde_json::json!({
                        "source_path": source,
                        "destination_name": "../escape.mov"
                    }),
                ),
                ctx_at(project_dir.path()),
            )
            .await
            .unwrap_err();

        match err {
            FunctionCallError::RespondToModel(message) => {
                assert!(message.contains("single safe file name"), "got: {message}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
