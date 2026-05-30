//! AGENTS.md read/write commands for the in-product editor (Wave 3 T2).
//!
//! The agent reads `<project_root>/AGENTS.md` at session start. The
//! Settings → Project section ships an "Edit AGENTS.md" button that
//! opens a modal mono-text editor; these commands back that flow.
//!
//! Path resolution: the caller passes the project root explicitly
//! (the frontend's `useProjectStore().current`). We refuse anything
//! that isn't an absolute path so a relative-path caller can't
//! escape via `..`. The file always lives at the root — subdirectory
//! AGENTS.md scoping is out of scope for the in-product editor.

use std::path::{Path, PathBuf};

use tokio::fs;

const AGENTS_MD_FILENAME: &str = "AGENTS.md";

/// Read `<project_path>/AGENTS.md`. Returns `None` when the file is
/// absent so the frontend can show a starter template without
/// conflating "no project" with "no AGENTS.md yet".
#[tauri::command]
pub async fn read_agents_md(project_path: String) -> Result<Option<String>, String> {
    let root = validate_project_root(&project_path)?;
    let file = root.join(AGENTS_MD_FILENAME);
    match fs::read_to_string(&file).await {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read AGENTS.md: {e}")),
    }
}

/// Write `<project_path>/AGENTS.md`. Creates the file if missing.
/// Idempotent — re-writing the same content twice is a no-op from
/// the agent's perspective (next session-start re-reads the same
/// bytes).
#[tauri::command]
pub async fn write_agents_md(project_path: String, content: String) -> Result<(), String> {
    let root = validate_project_root(&project_path)?;
    let file = root.join(AGENTS_MD_FILENAME);
    fs::write(&file, content)
        .await
        .map_err(|e| format!("write AGENTS.md: {e}"))?;
    Ok(())
}

/// Reject relative paths and non-directories. The frontend always
/// passes the absolute path from `useProjectStore().current`; this
/// guard catches accidental empty / relative inputs before they hit
/// the filesystem (where they'd silently resolve against the
/// desktop process's cwd).
fn validate_project_root(project_path: &str) -> Result<PathBuf, String> {
    if project_path.is_empty() {
        return Err("project_path is empty".into());
    }
    let buf = PathBuf::from(project_path);
    if !buf.is_absolute() {
        return Err(format!("project_path must be absolute: {project_path}"));
    }
    if !Path::new(&buf).is_dir() {
        return Err(format!("not a directory: {project_path}"));
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let out = read_agents_md(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn read_returns_content_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), b"# hello\n").unwrap();
        let out = read_agents_md(tmp.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(out.as_deref(), Some("# hello\n"));
    }

    #[tokio::test]
    async fn write_creates_file_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        write_agents_md(
            tmp.path().to_string_lossy().into_owned(),
            "# new\n".to_string(),
        )
        .await
        .unwrap();
        let on_disk = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
        assert_eq!(on_disk, "# new\n");
    }

    #[tokio::test]
    async fn write_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let body = "# repeat\n".to_string();
        write_agents_md(tmp.path().to_string_lossy().into_owned(), body.clone())
            .await
            .unwrap();
        write_agents_md(tmp.path().to_string_lossy().into_owned(), body.clone())
            .await
            .unwrap();
        let on_disk = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
        assert_eq!(on_disk, body);
    }

    #[tokio::test]
    async fn write_overwrites_existing_content() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("AGENTS.md"), b"old").unwrap();
        write_agents_md(tmp.path().to_string_lossy().into_owned(), "new".to_string())
            .await
            .unwrap();
        let on_disk = std::fs::read_to_string(tmp.path().join("AGENTS.md")).unwrap();
        assert_eq!(on_disk, "new");
    }

    #[tokio::test]
    async fn read_refuses_relative_paths() {
        let err = read_agents_md("relative/path".to_string())
            .await
            .unwrap_err();
        assert!(err.contains("must be absolute"), "got: {err}");
    }

    #[tokio::test]
    async fn write_refuses_relative_paths() {
        let err = write_agents_md("relative/path".to_string(), "x".into())
            .await
            .unwrap_err();
        assert!(err.contains("must be absolute"), "got: {err}");
    }

    #[tokio::test]
    async fn read_refuses_empty_path() {
        let err = read_agents_md(String::new()).await.unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[tokio::test]
    async fn read_refuses_non_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        let err = read_agents_md(file.to_string_lossy().into_owned())
            .await
            .unwrap_err();
        assert!(err.contains("not a directory"), "got: {err}");
    }
}
