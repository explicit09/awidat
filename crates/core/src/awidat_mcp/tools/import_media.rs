//! `import_media` — ingest local files or URLs into the project's
//! `raw/` directory and record them in the durable asset catalog.
//! Ported from `crates/core/src/tools/import_media.rs` to the
//! in-process MCP server.
//!
//! The source file exposes two distinct mutating tools — `import_local`
//! (copy/symlink a local file) and `import_url` (download via yt-dlp).
//! Both are ported here as separate `run_local` / `run_url` entry
//! points with their own arg structs and registered as two methods on
//! `AwidatMcpServer`.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use awidat_proto::professional::{
    AssetProvenance, AssetReadiness, AssetRecord, AssetRole, ReadinessState,
};
use awidat_proto::project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::awidat_mcp::context::McpToolCtx;
use crate::media_catalog_mutation::{ensure_awidat_metadata, upsert_asset};

/// Arguments to `import_local`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ImportLocalArgs {
    /// Absolute path to an existing local media file.
    pub source_path: String,
    /// Optional safe file name to use under `raw/`. Defaults to the
    /// source file name.
    #[serde(default)]
    pub destination_name: Option<String>,
    /// Create a symlink instead of copying. Default false.
    #[serde(default)]
    pub link: bool,
}

/// Arguments to `import_url`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ImportUrlArgs {
    /// URL to download with yt-dlp.
    pub url: String,
    /// Optional safe output file name under `raw/`. When omitted,
    /// yt-dlp chooses a restricted title-based name.
    #[serde(default)]
    pub destination_name: Option<String>,
}

/// Run `import_local`. Returns a JSON status body as `Ok(String)`.
pub async fn run_local(args: ImportLocalArgs, ctx: McpToolCtx) -> Result<String, String> {
    let source = PathBuf::from(&args.source_path);
    if !source.is_absolute() {
        return Err("import_local: source_path must be absolute".into());
    }
    if !source.is_file() {
        return Err(format!(
            "import_local: source_path is not a file: {}",
            source.display()
        ));
    }

    let raw_dir = ctx.project_root.join("raw");
    std::fs::create_dir_all(&raw_dir)
        .map_err(|e| format!("import_local: unable to create raw/: {e}"))?;
    let file_name = safe_destination_name(
        args.destination_name.as_deref(),
        source.file_name().and_then(|name| name.to_str()),
        "import_local",
    )?;
    let destination = raw_dir.join(file_name);
    if destination.exists() {
        return Err(format!(
            "import_local: destination already exists: {}",
            destination.display()
        ));
    }

    if args.link {
        symlink_file(&source, &destination)
            .map_err(|e| format!("import_local: symlink failed: {e}"))?;
    } else {
        std::fs::copy(&source, &destination)
            .map_err(|e| format!("import_local: copy failed: {e}"))?;
    }

    let rel_path = project_relative_path(&ctx.project_root, &destination)?;
    let size_bytes = destination.metadata().map(|meta| meta.len()).unwrap_or(0);
    record_imported_asset(
        &ctx.project_root,
        &rel_path,
        Some(source.to_string_lossy().to_string()),
        "import_local",
    )?;

    Ok(serde_json::json!({
        "status": "imported",
        "asset_id": rel_path,
        "path": rel_path,
        "size_bytes": size_bytes,
        "linked": args.link
    })
    .to_string())
}

/// Run `import_url`. Returns a JSON status body as `Ok(String)`.
pub async fn run_url(args: ImportUrlArgs, ctx: McpToolCtx) -> Result<String, String> {
    if !(args.url.starts_with("http://") || args.url.starts_with("https://")) {
        return Err("import_url: url must start with http:// or https://".into());
    }
    let raw_dir = ctx.project_root.join("raw");
    std::fs::create_dir_all(&raw_dir)
        .map_err(|e| format!("import_url: unable to create raw/: {e}"))?;
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
            format!("import_url: unable to run yt-dlp ({e}); install yt-dlp or use import_local")
        })?;
    if !status.success() {
        return Err(format!("import_url: yt-dlp exited with status {status}"));
    }

    let imported = new_files(&raw_dir, &before)
        .map_err(|e| format!("import_url: unable to inspect raw/: {e}"))?;
    let destination = imported
        .into_iter()
        .next()
        .ok_or_else(|| "import_url: yt-dlp finished but no new file appeared under raw/".to_string())?;
    let rel_path = project_relative_path(&ctx.project_root, &destination)?;
    let size_bytes = destination.metadata().map(|meta| meta.len()).unwrap_or(0);
    record_imported_asset(&ctx.project_root, &rel_path, Some(args.url), "import_url")?;

    Ok(serde_json::json!({
        "status": "imported",
        "asset_id": rel_path,
        "path": rel_path,
        "size_bytes": size_bytes
    })
    .to_string())
}

fn record_imported_asset(
    project_root: &Path,
    rel_path: &str,
    imported_from: Option<String>,
    created_by: &str,
) -> Result<(), String> {
    let mut project = Project::read(project_root)
        .map_err(|e| format!("import_media: unable to read project: {e}"))?;
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
    project
        .write(project_root)
        .map_err(|e| format!("import_media: unable to write project: {e}"))
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
) -> Result<String, String> {
    match requested.or(fallback) {
        Some(name) => safe_file_name(name, tool_name),
        None => Err(format!(
            "{tool_name}: unable to determine destination file name"
        )),
    }
}

fn safe_file_name(name: &str, tool_name: &str) -> Result<String, String> {
    let path = Path::new(name);
    if name.trim().is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || name.contains("..")
        || name.contains('/')
        || name.contains('\\')
    {
        return Err(format!(
            "{tool_name}: destination_name must be a single safe file name"
        ));
    }
    Ok(name.to_string())
}

fn project_relative_path(project_root: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(project_root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| {
            format!(
                "import_media: imported path escaped project root: {}",
                path.display()
            )
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

pub const DESCRIPTION_LOCAL: &str = "\
Import a local media file into the project's raw/ directory and record \
it in the durable asset catalog. Pass an absolute `source_path`, an \
optional `destination_name` (a single safe file name under raw/), and \
optional `link: true` to create a symlink instead of copying.";

pub const DESCRIPTION_URL: &str = "\
Download a URL into the project's raw/ directory via yt-dlp and record \
it in the durable asset catalog. Pass an http(s) `url` and an optional \
`destination_name`. Requires `yt-dlp` on PATH.";
