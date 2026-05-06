//! Asset import: pulls media into the project's `raw/` dir.
//!
//! Two flavors:
//! - [`import_local`] — copy or symlink an existing file
//! - [`import_url`] — `yt-dlp` download via the bundled sidecar
//!
//! Both surface as [`Item::Job`] in the chat stream (Started → Delta →
//! Completed) so the user sees progress in the same timeline as agent
//! activity. Both honour the per-job [`CancellationToken`] held in
//! `AwidatState::jobs`, so the frontend can call `cancel_job(id)` to
//! abort a long download.

use std::path::PathBuf;
use std::process::Stdio;

use awidat_desktop_protocol::{Id, JobKind};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_shell::ShellExt;
use tauri_plugin_shell::process::CommandEvent;
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::events::JobEmitter;
use crate::state::{AwidatState, JobHandle};

/// Copy or symlink an existing local file into `<project>/raw/`.
/// Emits an `Item::Job` (kind = `LocalImport`) for visibility, even
/// though the operation is fast — keeps the chat-as-activity-log
/// model consistent.
///
/// `link` = `false` copies (default); `link` = `true` symlinks.
/// Symlinks save disk for big files but make the project not
/// portable across machines.
///
/// Returns the absolute path of the file inside the project's `raw/`.
#[tauri::command]
pub async fn import_local(
    app: AppHandle,
    state: State<'_, AwidatState>,
    src_path: String,
    link: bool,
) -> Result<String, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;

    let job_id = Id::new(format!(
        "import-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let cancel = register_job(&state, &job_id).await;
    let emitter = JobEmitter::start(
        app.clone(),
        job_id.clone(),
        JobKind::LocalImport,
        format!("importing {src_path}"),
    );

    let result = run_local_import(&project_root, &src_path, link, &cancel).await;
    unregister_job(&state, &job_id).await;

    match result {
        Ok(dst) => {
            let summary = format!(
                "imported {}",
                dst.file_name().unwrap_or_default().to_string_lossy()
            );
            emitter.ok(Some(summary));
            // Fire the auto-chain (transcode → index) in the
            // background so the command returns promptly. Each step
            // emits its own job card; failures don't roll back
            // earlier steps (a transcoded file with no index is
            // still useful — the user can retry indexing).
            spawn_post_import_chain(app.clone(), project_root.clone(), dst.clone());
            Ok(dst.to_string_lossy().into_owned())
        }
        Err(_e) if cancel.is_cancelled() => {
            emitter.cancelled();
            Err("cancelled".into())
        }
        Err(e) => {
            emitter.err(e.clone());
            Err(e)
        }
    }
}

/// Spawn the post-import auto-chain (proxy transcode → auto-insert
/// → thumbnails → indexer). Runs as a background tokio task so the
/// originating import command returns immediately. Each step emits
/// its own job card; the chain stops at the first hard error but the
/// partial progress survives — a transcoded-but-not-yet-indexed asset
/// is still scrubbable in the preview pane.
///
/// Thumbnail generation is sequenced *after* auto-insert (rather than
/// before) so the user sees the clip on the timeline as a coloured
/// rect immediately; the filmstrip flips in once the
/// [`JobKind::Thumbnails`] job completes via a second
/// `timeline-changed` event.
fn spawn_post_import_chain(app: AppHandle, project_root: PathBuf, asset: PathBuf) {
    tokio::spawn(async move {
        let state = app.state::<AwidatState>();
        // Transcode the just-imported asset.
        if let Err(e) = crate::commands::transcode::transcode_single_asset_in_project(
            &app,
            &state,
            &project_root,
            &asset,
        )
        .await
        {
            tracing::warn!(error = %e, asset = %asset.display(), "auto-transcode failed; skipping auto-insert + index");
            return;
        }

        // Auto-insert this asset onto the timeline if (and only if)
        // the timeline has no video clips yet. First-asset-on-fresh-
        // project case — users expect to see what they just
        // imported, not have to ask the agent to add it.
        match awidat_render::probe_duration_s(&asset).await {
            Ok(Some(duration_s)) => {
                match crate::commands::auto_insert::auto_insert_if_empty(
                    &project_root,
                    &asset,
                    duration_s,
                )
                .await
                {
                    Ok(true) => {
                        crate::events::emit_timeline_changed(&app, &project_root);
                        tracing::info!(
                            asset = %asset.display(),
                            duration_s,
                            "auto-inserted first asset onto empty timeline",
                        );
                    }
                    Ok(false) => {
                        // Non-empty timeline; user is arranging.
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "auto-insert failed");
                    }
                }
            }
            Ok(None) => {
                tracing::warn!(
                    asset = %asset.display(),
                    "auto-insert: ffprobe couldn't determine duration; skipping",
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "auto-insert: probe failed");
            }
        }

        // Generate filmstrip thumbnails for the just-imported asset.
        // Idempotent (mtime-checked) so re-running the chain on an
        // already-thumbed asset is a no-op. We fire `timeline-changed`
        // afterwards so the frontend's re-fetch picks up the now-
        // populated `thumbnail_dir` field on the clip.
        match crate::commands::thumbnail::generate_thumbnails_for_asset_in_project(
            &app,
            &state,
            &project_root,
            &asset,
        )
        .await
        {
            Ok(_) => {
                crate::events::emit_timeline_changed(&app, &project_root);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    asset = %asset.display(),
                    "auto-thumbnails failed; clip stays as coloured rect",
                );
            }
        }

        // Index the whole project. The indexer is sha-keyed so this
        // is idempotent for already-indexed assets — only the new
        // asset's pairs do real work.
        if let Err(e) =
            crate::commands::index::index_project_at_root(&app, &state, project_root.clone()).await
        {
            tracing::warn!(error = %e, "auto-index failed");
        }
    });
}

/// Implementation of the local-import logic. Separated so the
/// command body can map errors / cancellation cleanly.
async fn run_local_import(
    project_root: &std::path::Path,
    src_path: &str,
    link: bool,
    cancel: &CancellationToken,
) -> Result<PathBuf, String> {
    if cancel.is_cancelled() {
        return Err("cancelled".into());
    }
    let src = PathBuf::from(src_path);
    if !src.is_file() {
        return Err(format!("source is not a file: {src_path}"));
    }
    let raw = project_root.join("raw");
    fs::create_dir_all(&raw)
        .await
        .map_err(|e| format!("create raw/: {e}"))?;
    let filename = src
        .file_name()
        .ok_or_else(|| format!("source path has no filename: {src_path}"))?;
    let dst = raw.join(filename);
    if dst.exists() {
        return Err(format!(
            "a file named {} already exists in raw/",
            filename.to_string_lossy()
        ));
    }
    if link {
        let abs = src
            .canonicalize()
            .map_err(|e| format!("resolve absolute path: {e}"))?;
        #[cfg(unix)]
        {
            let dst_for_link = dst.clone();
            tokio::task::spawn_blocking(move || std::os::unix::fs::symlink(&abs, &dst_for_link))
                .await
                .map_err(|e| format!("symlink join: {e}"))?
                .map_err(|e| format!("symlink: {e}"))?;
        }
        #[cfg(not(unix))]
        {
            let _ = abs;
            return Err("symlinks not supported on this platform; pass link=false".into());
        }
    } else {
        // tokio::fs::copy streams in 64KB chunks — fine for big
        // media. Cancellation polled around the call.
        tokio::select! {
            _ = cancel.cancelled() => return Err("cancelled".into()),
            r = fs::copy(&src, &dst) => {
                r.map_err(|e| format!("copy: {e}"))?;
            }
        }
    }
    Ok(dst)
}

/// Download a URL via the bundled `yt-dlp` sidecar. Streams progress
/// out of yt-dlp's `--newline` mode; cancellable via `cancel_job(id)`.
///
/// Returns the absolute path of the downloaded file inside `raw/`.
#[tauri::command]
pub async fn import_url(
    app: AppHandle,
    state: State<'_, AwidatState>,
    url: String,
) -> Result<String, String> {
    let project_root = state
        .project_root
        .lock()
        .await
        .clone()
        .ok_or_else(|| "no project loaded".to_string())?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(format!("not an http(s) url: {url}"));
    }

    let job_id = Id::new(format!(
        "ytdlp-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let cancel = register_job(&state, &job_id).await;
    let emitter = JobEmitter::start(
        app.clone(),
        job_id.clone(),
        JobKind::UrlImport,
        format!("downloading {url}"),
    );

    let result = run_url_import(&app, &project_root, &url, &emitter, &cancel).await;
    unregister_job(&state, &job_id).await;

    match result {
        Ok(path) => {
            let summary = format!(
                "downloaded {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            emitter.ok(Some(summary));
            spawn_post_import_chain(app.clone(), project_root.clone(), path.clone());
            Ok(path.to_string_lossy().into_owned())
        }
        Err(_e) if cancel.is_cancelled() => {
            emitter.cancelled();
            Err("cancelled".into())
        }
        Err(e) => {
            emitter.err(e.clone());
            Err(e)
        }
    }
}

async fn run_url_import(
    app: &AppHandle,
    project_root: &std::path::Path,
    url: &str,
    emitter: &JobEmitter,
    cancel: &CancellationToken,
) -> Result<PathBuf, String> {
    let raw = project_root.join("raw");
    fs::create_dir_all(&raw)
        .await
        .map_err(|e| format!("create raw/: {e}"))?;
    let output_template = raw.join("yt-%(id)s.%(ext)s");

    // `--newline` makes yt-dlp emit one progress line per percent
    // (vs. carriage-returned in-place updates), which is what
    // `CommandEvent::Stdout` gives us per-line.
    // `--print after_move:filepath` makes the very last stdout line
    // be the final filepath — we capture and parse it out.
    let cmd = app
        .shell()
        .sidecar("yt-dlp")
        .map_err(|e| format!("locate yt-dlp sidecar: {e}"))?
        .args([
            "--newline",
            "--print",
            "after_move:filepath",
            "-f",
            "bv*[height<=1080]+ba/b[height<=1080]",
            "--merge-output-format",
            "mp4",
            "-o",
            &output_template.to_string_lossy(),
            url,
        ]);
    let _ = (Stdio::null(),); // silence unused-import on some cfgs

    let (mut rx, child) = cmd.spawn().map_err(|e| format!("spawn yt-dlp: {e}"))?;

    let mut last_filepath: Option<String> = None;
    let mut error_lines: Vec<String> = Vec::new();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = child.kill();
                return Err("cancelled".into());
            }
            ev = rx.recv() => {
                let Some(ev) = ev else { break };
                match ev {
                    CommandEvent::Stdout(buf) => {
                        let line = String::from_utf8_lossy(&buf).trim().to_string();
                        if line.is_empty() { continue; }
                        if let Some(p) = parse_progress(&line) {
                            emitter.progress(Some(p.percent), p.status);
                        } else if line.starts_with('/') || line.contains(":\\") {
                            // Heuristic: `--print after_move:filepath`
                            // emits an absolute path. Pick the last
                            // such line as the final file location.
                            last_filepath = Some(line);
                        } else {
                            // Status / log line. Surface as the
                            // current status without a percent.
                            emitter.progress(None, line);
                        }
                    }
                    CommandEvent::Stderr(buf) => {
                        let line = String::from_utf8_lossy(&buf).trim().to_string();
                        if !line.is_empty() {
                            error_lines.push(line);
                        }
                    }
                    CommandEvent::Error(e) => {
                        return Err(format!("yt-dlp error: {e}"));
                    }
                    CommandEvent::Terminated(payload) => {
                        if payload.code != Some(0) {
                            let detail = if error_lines.is_empty() {
                                format!("exit {:?}", payload.code)
                            } else {
                                error_lines.join("\n")
                            };
                            return Err(format!("yt-dlp failed: {detail}"));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    let path = last_filepath.ok_or_else(|| "yt-dlp produced no filepath".to_string())?;
    let pb = PathBuf::from(&path);
    if !pb.exists() {
        return Err(format!("yt-dlp reported {path} but file is not on disk"));
    }
    Ok(pb)
}

/// Parse one yt-dlp `--newline` progress line. Format example:
/// `[download]  45.2% of  120.00MiB at  3.20MiB/s ETA 00:24`.
/// Returns `None` for any other line.
struct ParsedProgress {
    percent: u8,
    status: String,
}

fn parse_progress(line: &str) -> Option<ParsedProgress> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("[download]") {
        return None;
    }
    // Find "<num>%" anywhere on the line; that's the only stable
    // anchor across yt-dlp versions.
    let pct_idx = trimmed.find('%')?;
    // Walk back to the start of the percent number.
    let head = &trimmed[..pct_idx];
    let num_start = head
        .rfind(|c: char| c.is_whitespace())
        .map(|i| i + 1)
        .unwrap_or(0);
    let pct_str = &head[num_start..];
    let pct: f32 = pct_str.parse().ok()?;
    let pct_u8 = pct.round().clamp(0.0, 100.0) as u8;
    Some(ParsedProgress {
        percent: pct_u8,
        status: trimmed.to_string(),
    })
}

async fn register_job(state: &State<'_, AwidatState>, id: &Id) -> CancellationToken {
    let token = CancellationToken::new();
    state.jobs.lock().await.insert(
        id.0.clone(),
        JobHandle {
            cancel: token.clone(),
        },
    );
    token
}

async fn unregister_job(state: &State<'_, AwidatState>, id: &Id) {
    state.jobs.lock().await.remove(&id.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_progress_extracts_percent() {
        let line = "[download]  45.2% of  120.00MiB at  3.20MiB/s ETA 00:24";
        let p = parse_progress(line).unwrap();
        assert_eq!(p.percent, 45);
        assert!(p.status.contains("45.2%"));
    }

    #[test]
    fn parse_progress_handles_full() {
        let p = parse_progress("[download] 100% of 50.0MiB in 12s").unwrap();
        assert_eq!(p.percent, 100);
    }

    #[test]
    fn parse_progress_rejects_non_download_lines() {
        assert!(parse_progress("[info] bla").is_none());
        assert!(parse_progress("yt-dlp 2024.x").is_none());
    }
}
