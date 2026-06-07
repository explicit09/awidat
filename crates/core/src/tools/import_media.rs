//! Agent-callable media ingest tools.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use montage_proto::professional::{
    AssetProvenance, AssetReadiness, AssetRecord, AssetRole, ReadinessState,
};
use montage_proto::project::Project;
use serde::Deserialize;
use tokio::process::Command;

use crate::FunctionCallError;
use crate::media_catalog_mutation::{ensure_montage_metadata, upsert_asset};
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
    /// When set, import up to N items from a playlist/channel URL
    /// (yt-dlp `--playlist-end N`). When absent, single-item import
    /// behaviour is preserved (yt-dlp `--no-playlist`).
    #[serde(default)]
    limit: Option<u32>,
    /// Optional lower bound on the upload date (inclusive). Accepts
    /// `YYYY-MM-DD` or `YYYYMMDD`; forwarded to yt-dlp `--dateafter`.
    #[serde(default)]
    after: Option<String>,
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
            None,
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
            description: "Download a URL into the project's raw/ directory via yt-dlp and record it in the durable asset catalog. Supports batch import of the latest N items from a playlist/channel URL via `limit` and an optional `after` date bound.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["url"],
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "URL to download with yt-dlp. May be a single video, or a playlist/channel URL when `limit` is set."
                    },
                    "destination_name": {
                        "type": "string",
                        "description": "Optional safe output file name under raw/. When omitted, yt-dlp chooses a restricted title-based name. Ignored in batch mode (when `limit` is set)."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Import up to N items from a playlist/channel URL (yt-dlp --playlist-end). When omitted, only a single item is imported."
                    },
                    "after": {
                        "type": "string",
                        "description": "Only import items uploaded on/after this date. Accepts YYYY-MM-DD or YYYYMMDD (yt-dlp --dateafter)."
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
        let batch = parse_batch_options(args.limit, args.after.as_deref(), "import_url")?;
        let raw_dir = ctx.project_root.join("raw");
        std::fs::create_dir_all(&raw_dir).map_err(|e| {
            FunctionCallError::Fatal(format!("import_url: unable to create raw/: {e}"))
        })?;
        // In batch mode each item must land on its own file name, so we
        // ignore destination_name and use a deterministic id-based
        // template. Single imports honour destination_name as before.
        let output_template = if batch.is_some() {
            raw_dir.join("yt-%(id)s.%(ext)s")
        } else {
            match args.destination_name.as_deref() {
                Some(name) => raw_dir.join(safe_file_name(name, "import_url")?),
                None => raw_dir.join("%(title).200B.%(ext)s"),
            }
        };

        let argv = build_import_url_argv(&output_template.to_string_lossy(), &args.url, &batch);
        let out = Command::new("yt-dlp")
            .args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "import_url: unable to run yt-dlp ({e}); install yt-dlp or use import_local"
                ))
            })?;
        if !out.status.success() {
            return Err(FunctionCallError::RespondToModel(format!(
                "import_url: yt-dlp exited with status {}",
                out.status
            )));
        }

        let stdout = String::from_utf8_lossy(&out.stdout);
        let items = parse_imported_items(&stdout);
        if items.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "import_url: yt-dlp finished but reported no downloaded files".into(),
            ));
        }

        let mut imported = Vec::with_capacity(items.len());
        for item in &items {
            let path = PathBuf::from(&item.filepath);
            if !path.exists() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "import_url: yt-dlp reported {} but the file is not on disk",
                    item.filepath
                )));
            }
            let rel_path = project_relative_path(&ctx.project_root, &path)?;
            let size_bytes = path.metadata().map(|meta| meta.len()).unwrap_or(0);
            record_imported_asset(
                &ctx.project_root,
                &rel_path,
                Some(args.url.clone()),
                item.upload_date.clone(),
                "import_url",
            )?;
            imported.push(serde_json::json!({
                "asset_id": rel_path,
                "path": rel_path,
                "size_bytes": size_bytes,
                "upload_date": item.upload_date,
            }));
        }

        // Preserve the single-item response shape for backwards
        // compatibility; add an `items` array for batch callers.
        let first = imported[0].clone();
        Ok(ToolOutput::text(
            serde_json::json!({
                "status": "imported",
                "asset_id": first["asset_id"],
                "path": first["path"],
                "size_bytes": first["size_bytes"],
                "upload_date": first["upload_date"],
                "imported_count": imported.len(),
                "items": imported,
            })
            .to_string(),
        ))
    }
}

/// Bounds for a batch (playlist/channel) import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchOptions {
    /// Maximum number of items to import (yt-dlp `--playlist-end`).
    limit: u32,
    /// Optional `--dateafter` value, normalized to `YYYYMMDD`.
    dateafter: Option<String>,
}

/// Validate and normalize batch options. Returns `Ok(None)` when no
/// batch flags were supplied (single-item import).
pub(crate) fn parse_batch_options(
    limit: Option<u32>,
    after: Option<&str>,
    tool_name: &str,
) -> Result<Option<BatchOptions>, FunctionCallError> {
    let dateafter = match after {
        Some(raw) => Some(normalize_date_yyyymmdd(raw).ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "{tool_name}: `after` must be a date in YYYY-MM-DD or YYYYMMDD form"
            ))
        })?),
        None => None,
    };
    match limit {
        Some(0) => Err(FunctionCallError::RespondToModel(format!(
            "{tool_name}: `limit` must be >= 1"
        ))),
        Some(limit) => Ok(Some(BatchOptions { limit, dateafter })),
        None => {
            if dateafter.is_some() {
                // `after` alone still implies a batch (channel) import.
                Ok(Some(BatchOptions {
                    limit: DEFAULT_BATCH_LIMIT,
                    dateafter,
                }))
            } else {
                Ok(None)
            }
        }
    }
}

/// Default cap when an `after` bound is given without an explicit limit.
pub(crate) const DEFAULT_BATCH_LIMIT: u32 = 50;

/// Normalize a date string to yt-dlp's `YYYYMMDD` form. Accepts
/// `YYYY-MM-DD` or `YYYYMMDD`; returns `None` if it is not a plausible
/// 8-digit calendar-ish date.
pub(crate) fn normalize_date_yyyymmdd(raw: &str) -> Option<String> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 8 {
        return None;
    }
    // Reject anything that had non-digit, non-dash separators.
    if raw.chars().any(|c| !c.is_ascii_digit() && c != '-') {
        return None;
    }
    Some(digits)
}

/// Build the yt-dlp argv. Pure and deterministic so it can be unit
/// tested without spawning a process. `--no-playlist` keeps single
/// imports from accidentally fanning out across a playlist URL; batch
/// mode opts in with `--yes-playlist` plus the bound flags.
pub(crate) fn build_import_url_argv(
    output_template: &str,
    url: &str,
    batch: &Option<BatchOptions>,
) -> Vec<String> {
    let mut argv = vec![
        "--restrict-filenames".to_string(),
        "--no-part".to_string(),
        // Print the upload date and final path for each downloaded
        // item, one line per item, after the file is moved into place.
        "--print".to_string(),
        "after_move:%(upload_date)s\t%(filepath)s".to_string(),
    ];
    match batch {
        Some(opts) => {
            argv.push("--yes-playlist".to_string());
            argv.push("--playlist-end".to_string());
            argv.push(opts.limit.to_string());
            if let Some(dateafter) = &opts.dateafter {
                argv.push("--dateafter".to_string());
                argv.push(dateafter.clone());
            }
        }
        None => argv.push("--no-playlist".to_string()),
    }
    argv.push("-o".to_string());
    argv.push(output_template.to_string());
    argv.push(url.to_string());
    argv
}

/// A single item parsed from yt-dlp `--print` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImportedItem {
    filepath: String,
    upload_date: Option<String>,
}

/// Parse the tab-separated `upload_date\tfilepath` lines emitted by
/// `--print after_move:...`. yt-dlp prints `NA` for unknown upload
/// dates; those become `None`. The upload date is normalized to
/// `YYYY-MM-DD` for storage.
pub(crate) fn parse_imported_items(stdout: &str) -> Vec<ImportedItem> {
    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            if line.trim().is_empty() {
                return None;
            }
            let (date_raw, filepath) = match line.split_once('\t') {
                Some((d, p)) => (d.trim(), p.trim()),
                // Defensive: a line with no tab is treated as a bare path.
                None => ("", line.trim()),
            };
            if filepath.is_empty() {
                return None;
            }
            Some(ImportedItem {
                filepath: filepath.to_string(),
                upload_date: pretty_upload_date(date_raw),
            })
        })
        .collect()
}

/// Convert yt-dlp's `YYYYMMDD` (or `NA`) upload date to `YYYY-MM-DD`,
/// or `None` when unknown.
pub(crate) fn pretty_upload_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("NA") || raw.eq_ignore_ascii_case("none") {
        return None;
    }
    if raw.len() == 8 && raw.chars().all(|c| c.is_ascii_digit()) {
        Some(format!("{}-{}-{}", &raw[0..4], &raw[4..6], &raw[6..8]))
    } else {
        // Already formatted or unexpected; keep as-is.
        Some(raw.to_string())
    }
}

fn record_imported_asset(
    project_root: &Path,
    rel_path: &str,
    imported_from: Option<String>,
    upload_date: Option<String>,
    created_by: &str,
) -> Result<(), FunctionCallError> {
    let mut project = Project::read(project_root).map_err(|e| {
        FunctionCallError::RespondToModel(format!("import_media: unable to read project: {e}"))
    })?;
    let meta = ensure_montage_metadata(&mut project.timeline);
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
                upload_date,
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
    use montage_proto::project::Project;
    use tokio::sync::broadcast;

    use super::*;

    fn ctx_at(root: &std::path::Path) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: root.to_path_buf(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: montage_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(montage_mcp::ClientInfo {
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
    fn single_import_argv_uses_no_playlist_and_print() {
        let argv = build_import_url_argv("raw/%(title).200B.%(ext)s", "https://x/v", &None);
        assert!(argv.contains(&"--no-playlist".to_string()));
        assert!(!argv.contains(&"--yes-playlist".to_string()));
        assert!(!argv.iter().any(|a| a == "--playlist-end"));
        assert!(!argv.iter().any(|a| a == "--dateafter"));
        // print spec then the template then the url, in order.
        let print_idx = argv.iter().position(|a| a == "--print").unwrap();
        assert_eq!(
            argv[print_idx + 1],
            "after_move:%(upload_date)s\t%(filepath)s"
        );
        assert_eq!(argv.last().unwrap(), "https://x/v");
        let o_idx = argv.iter().position(|a| a == "-o").unwrap();
        assert_eq!(argv[o_idx + 1], "raw/%(title).200B.%(ext)s");
    }

    #[test]
    fn batch_import_argv_has_playlist_end_and_dateafter() {
        let batch = parse_batch_options(Some(5), Some("2025-01-31"), "import_url")
            .unwrap()
            .unwrap();
        let argv = build_import_url_argv("raw/yt-%(id)s.%(ext)s", "https://x/chan", &Some(batch));
        assert!(argv.contains(&"--yes-playlist".to_string()));
        assert!(!argv.contains(&"--no-playlist".to_string()));
        let pe_idx = argv.iter().position(|a| a == "--playlist-end").unwrap();
        assert_eq!(argv[pe_idx + 1], "5");
        let da_idx = argv.iter().position(|a| a == "--dateafter").unwrap();
        assert_eq!(argv[da_idx + 1], "20250131");
        assert_eq!(argv.last().unwrap(), "https://x/chan");
    }

    #[test]
    fn after_alone_implies_batch_with_default_limit() {
        let batch = parse_batch_options(None, Some("20240101"), "import_url")
            .unwrap()
            .unwrap();
        let argv = build_import_url_argv("raw/yt-%(id)s.%(ext)s", "https://x/chan", &Some(batch));
        let pe_idx = argv.iter().position(|a| a == "--playlist-end").unwrap();
        assert_eq!(argv[pe_idx + 1], DEFAULT_BATCH_LIMIT.to_string());
        assert!(argv.iter().any(|a| a == "--dateafter"));
    }

    #[test]
    fn no_bounds_means_no_batch() {
        assert_eq!(parse_batch_options(None, None, "import_url").unwrap(), None);
    }

    #[test]
    fn parse_batch_options_rejects_bad_input() {
        assert!(parse_batch_options(Some(0), None, "import_url").is_err());
        assert!(parse_batch_options(Some(3), Some("nope"), "import_url").is_err());
        assert!(parse_batch_options(Some(3), Some("2025/01/01"), "import_url").is_err());
    }

    #[test]
    fn normalize_date_accepts_both_forms() {
        assert_eq!(
            normalize_date_yyyymmdd("2025-06-03").as_deref(),
            Some("20250603")
        );
        assert_eq!(
            normalize_date_yyyymmdd("20250603").as_deref(),
            Some("20250603")
        );
        assert_eq!(normalize_date_yyyymmdd("2025-6-3"), None);
        assert_eq!(normalize_date_yyyymmdd(""), None);
    }

    #[test]
    fn parse_imported_items_handles_multiple_and_na() {
        let stdout = "20250101\t/proj/raw/yt-a.mp4\nNA\t/proj/raw/yt-b.mp4\n\n";
        let items = parse_imported_items(stdout);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].filepath, "/proj/raw/yt-a.mp4");
        assert_eq!(items[0].upload_date.as_deref(), Some("2025-01-01"));
        assert_eq!(items[1].filepath, "/proj/raw/yt-b.mp4");
        assert_eq!(items[1].upload_date, None);
    }

    #[test]
    fn pretty_upload_date_formats_and_drops_na() {
        assert_eq!(
            pretty_upload_date("20231225").as_deref(),
            Some("2023-12-25")
        );
        assert_eq!(pretty_upload_date("NA"), None);
        assert_eq!(pretty_upload_date(""), None);
    }

    #[test]
    fn record_imported_asset_stores_upload_date() {
        let project_dir = tempfile::tempdir().unwrap();
        Project::init(project_dir.path()).unwrap();
        // Create the raw file so role detection / catalog write is realistic.
        std::fs::create_dir_all(project_dir.path().join("raw")).unwrap();
        std::fs::write(project_dir.path().join("raw/yt-a.mp4"), b"x").unwrap();

        record_imported_asset(
            project_dir.path(),
            "raw/yt-a.mp4",
            Some("https://x/chan".into()),
            Some("2025-01-31".into()),
            "import_url",
        )
        .unwrap();

        let project = Project::read(project_dir.path()).unwrap();
        let meta = project.timeline.metadata.montage.as_ref().unwrap();
        let asset = &meta.asset_catalog.as_ref().unwrap().assets[0];
        let prov = asset.provenance.as_ref().unwrap();
        assert_eq!(prov.upload_date.as_deref(), Some("2025-01-31"));
        assert_eq!(prov.imported_from.as_deref(), Some("https://x/chan"));
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
        let meta = project.timeline.metadata.montage.as_ref().unwrap();
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
