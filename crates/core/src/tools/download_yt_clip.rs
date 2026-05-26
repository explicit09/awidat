//! `download_yt_clip` tool — fetch a YouTube / Vimeo clip via yt-dlp
//! into the project's `raw/broll/` directory, returning a ready-to-
//! paste `*** Insert BRoll` EDL fragment.
//!
//! Phase 4A (safe slice). Distinct from `use_broll` (Pexels): this
//! tool fetches user-supplied URLs from third-party platforms whose
//! licensing surface is murky. The agent MUST surface the
//! copyright caveat to the user and the user MUST acknowledge it
//! per-URL before the download runs.
//!
//! ## Caveat record
//!
//! Acknowledged URLs persist at `<project>/.awidat/yt_caveats.json`:
//!
//! ```json
//! {
//!   "schema": 1,
//!   "acknowledged": [
//!     { "url": "https://...", "acknowledged_at": "2026-05-07T..." }
//!   ]
//! }
//! ```
//!
//! v1: the file is informational. The user is supposed to verify
//! rights before each `acknowledged: true` call. A future export-
//! gating pass can read the file to enforce per-clip acknowledgment
//! before exporting; this tool deliberately does NOT block export
//! — that's a separate UX decision worth reviewing.
//!
//! ## Sandbox
//!
//! Runs `yt-dlp` directly via `tokio::process::Command`, NOT through
//! the bash tool. yt-dlp is a network-egress process and the bash
//! sandbox denies network egress by default — going through bash
//! would either fail (current state) or require lifting the bash
//! sandbox (a security regression). A dedicated tool is the right
//! shape: same pattern as `pexels::Client::download_to`, just with
//! a subprocess instead of reqwest.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::timeout;

use crate::FunctionCallError;
use crate::tool_schema::Tool as ToolSchema;
use crate::tool::{ApprovalKey, ToolContext, ToolHandler, ToolInvocation, ToolOutput};

/// Per-download timeout. yt-dlp + a fast network can finish a 720p
/// clip in 10s; slow networks or larger files can take a minute or
/// two. Five minutes covers the long tail without hanging the agent
/// loop forever.
const DOWNLOAD_TIMEOUT_SECS: u64 = 300;

/// Hosts the agent is allowed to fetch from. Modest allowlist —
/// expand only with explicit user policy review. Everything else
/// returns a clear "host not allowed" error rather than handing
/// yt-dlp an arbitrary URL.
const ALLOWED_HOSTS: &[&str] = &[
    "youtube.com",
    "www.youtube.com",
    "m.youtube.com",
    "youtu.be",
    "vimeo.com",
    "www.vimeo.com",
];

/// Per-session download cap, mirroring the Pexels tool. Resets on
/// process restart. Friction, not policy.
pub const MAX_DOWNLOADS_PER_SESSION: usize = 10;

use std::sync::atomic::{AtomicUsize, Ordering};
static DOWNLOAD_COUNT: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
fn reset_download_count() {
    DOWNLOAD_COUNT.store(0, Ordering::SeqCst);
}

/// The `download_yt_clip` tool.
pub struct DownloadYtClipTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Source URL. Must match an entry in [`ALLOWED_HOSTS`].
    url: String,
    /// Where on the timeline the cutaway should land. Same anchor
    /// shape as `use_broll` and the EDL grammar.
    anchor: AnchorArg,
    /// Cutaway length in seconds (0.5–30).
    duration_s: f64,
    /// Optional: trim the source to a sub-window before downloading.
    /// `start_s` and `end_s` map to yt-dlp's `--download-sections`.
    /// Useful when the source is a 60-minute video and you only
    /// need a 4-second slice — saves bandwidth and disk.
    #[serde(default)]
    source_start_s: Option<f64>,
    /// See `source_start_s`. End cap is yt-dlp's
    /// `--download-sections "*S-E"` (E inclusive).
    #[serde(default)]
    source_end_s: Option<f64>,
    /// Position on the timeline. `overlay` (default) or `replace`.
    #[serde(default)]
    position: Option<String>,
    /// Copyright acknowledgment gate. The agent MUST set this to
    /// `true` AFTER explaining the licensing situation to the user
    /// AND getting explicit confirmation. Recorded in the caveat
    /// file. Calls without this flag are refused.
    acknowledged: bool,
}

/// Anchor shape (same as `use_broll`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum AnchorArg {
    Transcript { transcript_snippet: String },
    Uuid { clip_uuid: String },
}

#[async_trait]
impl ToolHandler for DownloadYtClipTool {
    fn name(&self) -> &'static str {
        "download_yt_clip"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "download_yt_clip".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["url", "anchor", "duration_s", "acknowledged"],
                "properties": {
                    "url": { "type": "string", "description": "YouTube or Vimeo URL." },
                    "anchor": {
                        "oneOf": [
                            { "type": "object", "required": ["transcript_snippet"],
                              "properties": { "transcript_snippet": { "type": "string" } } },
                            { "type": "object", "required": ["clip_uuid"],
                              "properties": { "clip_uuid": { "type": "string" } } }
                        ]
                    },
                    "duration_s": { "type": "number", "minimum": 0.5, "maximum": 30.0 },
                    "source_start_s": { "type": "number", "minimum": 0.0 },
                    "source_end_s": { "type": "number", "minimum": 0.0 },
                    "position": { "type": "string", "enum": ["overlay", "replace"] },
                    "acknowledged": {
                        "type": "boolean",
                        "description": "REQUIRED. Set true ONLY after explaining the copyright caveat to the user and getting explicit confirmation. The user is responsible for rights verification."
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
        let url = invocation
            .args
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing-url>");
        let source_window = match (
            invocation.args.get("source_start_s"),
            invocation.args.get("source_end_s"),
        ) {
            (Some(start), Some(end)) => format!("{start}-{end}"),
            _ => "full".to_string(),
        };
        vec![ApprovalKey::new(
            "download_yt_clip",
            format!("{url}:{source_window}:dest=raw/broll"),
        )]
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "download_yt_clip: invalid args ({e}). Required: url, anchor, duration_s, acknowledged."
            ))
        })?;

        if !args.acknowledged {
            return Err(FunctionCallError::RespondToModel(
                "download_yt_clip: refused — `acknowledged` is false. \
                 Before setting acknowledged=true, you MUST explain to the user that \
                 third-party clips have copyright implications they're responsible for \
                 verifying, AND get their explicit confirmation. Once they confirm, \
                 retry with acknowledged=true."
                    .into(),
            ));
        }

        let host = parse_host(&args.url).ok_or_else(|| {
            FunctionCallError::RespondToModel(format!(
                "download_yt_clip: malformed URL '{}'. Pass a full https:// URL.",
                args.url
            ))
        })?;
        if !ALLOWED_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
            return Err(FunctionCallError::RespondToModel(format!(
                "download_yt_clip: host '{host}' not allowed. Allowed: {}.",
                ALLOWED_HOSTS.join(", ")
            )));
        }

        let position = match args.position.as_deref().unwrap_or("overlay") {
            "overlay" => "overlay",
            "replace" => "replace",
            other => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "download_yt_clip: invalid position '{other}'. Use 'overlay' or 'replace'."
                )));
            }
        };

        if !(0.5..=30.0).contains(&args.duration_s) {
            return Err(FunctionCallError::RespondToModel(format!(
                "download_yt_clip: duration_s={} out of range (0.5..=30.0).",
                args.duration_s
            )));
        }

        let prior = DOWNLOAD_COUNT.load(Ordering::SeqCst);
        if prior >= MAX_DOWNLOADS_PER_SESSION {
            return Err(FunctionCallError::RespondToModel(format!(
                "download_yt_clip: per-session download budget reached ({MAX_DOWNLOADS_PER_SESSION}). \
                 Restart the session if this is genuinely needed."
            )));
        }

        let clip_id = url_to_clip_id(&args.url);
        let asset_rel = format!("raw/broll/yt-{clip_id}.mp4");
        let dest = ctx.project_root.join(&asset_rel);
        let downloaded = !dest.exists();

        if downloaded {
            // Make sure raw/broll/ exists before yt-dlp tries to write.
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    FunctionCallError::RespondToModel(format!(
                        "download_yt_clip: create raw/broll/ failed: {e}"
                    ))
                })?;
            }

            let argv = build_yt_dlp_argv(&args.url, &dest, args.source_start_s, args.source_end_s);
            let result = timeout(
                Duration::from_secs(DOWNLOAD_TIMEOUT_SECS),
                Command::new(&argv[0]).args(&argv[1..]).output(),
            )
            .await;

            match result {
                Ok(Ok(out)) => {
                    if !out.status.success() {
                        let stderr = truncate(&String::from_utf8_lossy(&out.stderr), 800);
                        return Err(FunctionCallError::RespondToModel(format!(
                            "download_yt_clip: yt-dlp exited {} for '{}': {}",
                            out.status.code().unwrap_or(-1),
                            args.url,
                            stderr,
                        )));
                    }
                }
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(FunctionCallError::RespondToModel(
                        "download_yt_clip: yt-dlp not found on PATH. Install via \
                         `brew install yt-dlp` (macOS) or `pipx install yt-dlp` (cross-platform), \
                         then retry."
                            .into(),
                    ));
                }
                Ok(Err(e)) => {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "download_yt_clip: spawning yt-dlp failed: {e}"
                    )));
                }
                Err(_) => {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "download_yt_clip: yt-dlp timed out after {DOWNLOAD_TIMEOUT_SECS}s. \
                         Try a smaller source_start_s/source_end_s window or a slower-bitrate format."
                    )));
                }
            }

            DOWNLOAD_COUNT.fetch_add(1, Ordering::SeqCst);
        }

        // Persist caveat acknowledgment after the download lands so
        // a failed download doesn't pollute the record.
        if let Err(e) = persist_caveat(&ctx.project_root, &args.url) {
            tracing::warn!(error = %e, "download_yt_clip: failed to persist caveat record");
        }

        let edl_fragment = build_edl_fragment(&asset_rel, &args.anchor, args.duration_s, position);
        let body = serde_json::json!({
            "asset_path": asset_rel,
            "absolute_path": dest.display().to_string(),
            "downloaded": downloaded,
            "source_url": args.url,
            "edl_fragment": edl_fragment,
            "downloads_remaining_this_session":
                MAX_DOWNLOADS_PER_SESSION.saturating_sub(DOWNLOAD_COUNT.load(Ordering::SeqCst)),
            "next_step": "Hand the edl_fragment to apply_edl to place the cutaway. \
                          The user has acknowledged the copyright caveat for this URL.",
        });
        Ok(ToolOutput::text(body.to_string()))
    }
}

/// Caveat record persisted at `<project>/.awidat/yt_caveats.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CaveatFile {
    /// Schema version. Currently 1.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Acknowledged URLs in arrival order.
    #[serde(default)]
    pub acknowledged: Vec<CaveatRecord>,
}

fn default_version() -> u32 {
    1
}

/// One caveat record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaveatRecord {
    /// Source URL the user acknowledged.
    pub url: String,
    /// ISO-8601 timestamp.
    pub acknowledged_at: String,
}

/// Path: `<project>/.awidat/yt_caveats.json`.
pub fn caveat_file_path(project_root: &Path) -> PathBuf {
    project_root.join(".awidat").join("yt_caveats.json")
}

fn persist_caveat(project_root: &Path, url: &str) -> std::io::Result<()> {
    let path = caveat_file_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file: CaveatFile = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CaveatFile::default(),
        Err(e) => return Err(e),
    };
    if file.version == 0 {
        file.version = 1;
    }
    if !file.acknowledged.iter().any(|r| r.url == url) {
        file.acknowledged.push(CaveatRecord {
            url: url.to_string(),
            acknowledged_at: chrono::Utc::now().to_rfc3339(),
        });
    }
    let bytes = serde_json::to_vec_pretty(&file)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&path, bytes)
}

fn build_yt_dlp_argv(
    url: &str,
    dest: &Path,
    source_start_s: Option<f64>,
    source_end_s: Option<f64>,
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "yt-dlp".into(),
        // Cap quality at 1080p mp4 — anything higher wastes disk for
        // a cutaway.
        "-f".into(),
        "bv*[height<=1080][ext=mp4]+ba[ext=m4a]/b[height<=1080][ext=mp4]/b".into(),
        // Merge to a single mp4 container.
        "--merge-output-format".into(),
        "mp4".into(),
        // Be quiet on stdout; we only care about the exit code.
        "--no-progress".into(),
        "--quiet".into(),
        "--no-warnings".into(),
        // Skip playlists that share the same id.
        "--no-playlist".into(),
        "-o".into(),
        dest.display().to_string(),
    ];
    // Optional sub-window. yt-dlp accepts `*<start>-<end>` syntax.
    if let (Some(s), Some(e)) = (source_start_s, source_end_s)
        && e > s
    {
        argv.push("--download-sections".into());
        argv.push(format!("*{s:.2}-{e:.2}"));
        // Tell yt-dlp to actually trim, not just record the metadata.
        argv.push("--force-keyframes-at-cuts".into());
    }
    argv.push(url.to_string());
    argv
}

fn build_edl_fragment(
    asset_rel: &str,
    anchor: &AnchorArg,
    duration_s: f64,
    position: &str,
) -> String {
    let anchor_line = match anchor {
        AnchorArg::Transcript { transcript_snippet } => {
            let escaped = transcript_snippet.replace('"', "\\\"");
            format!("@@ anchor: transcript_snippet=\"{escaped}\"")
        }
        AnchorArg::Uuid { clip_uuid } => {
            format!("@@ anchor: clip_uuid={clip_uuid}")
        }
    };
    format!(
        "*** Begin EDL\n\
         *** Insert BRoll\n\
         {anchor_line}\n\
         + asset: {asset_rel}\n\
         + duration_s: {duration_s}\n\
         + position: {position}\n\
         *** End EDL\n"
    )
}

fn parse_host(url: &str) -> Option<String> {
    // Cheap manual parser — avoids pulling in `url`. Format:
    // scheme://host[:port]/path
    let after_scheme = url.split_once("://")?.1;
    let host_with_path = after_scheme;
    let host = host_with_path.split('/').next().unwrap_or(host_with_path);
    let host_no_port = host.split(':').next().unwrap_or(host);
    if host_no_port.is_empty() {
        None
    } else {
        Some(host_no_port.to_lowercase())
    }
}

fn url_to_clip_id(url: &str) -> String {
    // Stable, filesystem-safe hash that captures the URL. Using a
    // simple stripped-and-hashed form keeps the same URL going to the
    // same on-disk path (idempotent re-runs).
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(url.as_bytes());
    // First 12 hex chars; collision probability is negligible per
    // project at our scale.
    let mut s = String::with_capacity(12);
    for b in &hash[..6] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

const DESCRIPTION: &str = "\
Download a YouTube or Vimeo clip via yt-dlp into the project's \
raw/broll/ directory and return a ready-to-paste *** Insert BRoll \
EDL fragment.\
\n\nGated on `acknowledged: true`. Before setting that flag, you \
MUST explain to the user that third-party clips have copyright \
implications they're responsible for verifying, AND get their \
explicit confirmation. Acknowledged URLs persist in \
`<project>/.awidat/yt_caveats.json`.\
\n\nAllowed hosts: youtube.com, m.youtube.com, youtu.be, vimeo.com. \
Anything else is refused with a clear error.\
\n\nIdempotent on file existence: a previously-downloaded URL hits \
the same on-disk path (sha-keyed by URL) and skips the re-download.\
\n\nOptional: pass `source_start_s` + `source_end_s` to fetch only a \
sub-window of a long source — saves bandwidth and disk. yt-dlp's \
--download-sections handles this with --force-keyframes-at-cuts.\
\n\nPer-session cap: 10 downloads. yt-dlp must be on PATH; \
absent-binary case surfaces a clear `brew install yt-dlp` prompt.\
\n\nReturns: { asset_path, absolute_path, downloaded, source_url, \
edl_fragment, downloads_remaining_this_session, next_step }.\
\n\nDoes NOT call apply_edl — you hand the edl_fragment to apply_edl \
to actually place the cutaway. Same flow as `use_broll`.\
";

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_youtube_host() {
        assert_eq!(
            parse_host("https://www.youtube.com/watch?v=abc"),
            Some("www.youtube.com".to_string())
        );
        assert_eq!(
            parse_host("https://youtu.be/abc"),
            Some("youtu.be".to_string())
        );
        assert_eq!(
            parse_host("https://m.youtube.com/watch?v=abc"),
            Some("m.youtube.com".to_string())
        );
    }

    #[test]
    fn rejects_non_url_strings() {
        assert!(parse_host("not a url").is_none());
        assert!(parse_host("youtube.com/foo").is_none());
    }

    #[test]
    fn url_to_clip_id_is_stable_and_short() {
        let a = url_to_clip_id("https://youtu.be/abc");
        let b = url_to_clip_id("https://youtu.be/abc");
        assert_eq!(a, b, "must be stable for the same URL");
        assert_eq!(a.len(), 12);
        assert_ne!(a, url_to_clip_id("https://youtu.be/xyz"));
    }

    #[test]
    fn build_argv_includes_url_and_output_path() {
        let dest = std::path::Path::new("/tmp/foo.mp4");
        let argv = build_yt_dlp_argv("https://youtu.be/abc", dest, None, None);
        assert_eq!(argv[0], "yt-dlp");
        assert!(argv.contains(&"-o".to_string()));
        assert!(argv.iter().any(|s| s == "/tmp/foo.mp4"));
        assert_eq!(
            argv.last().map(String::as_str),
            Some("https://youtu.be/abc")
        );
        // No sub-window flags when not provided.
        assert!(!argv.iter().any(|s| s == "--download-sections"));
    }

    #[test]
    fn build_argv_appends_sub_window_when_provided() {
        let dest = std::path::Path::new("/tmp/foo.mp4");
        let argv = build_yt_dlp_argv("https://youtu.be/abc", dest, Some(10.0), Some(13.5));
        assert!(argv.iter().any(|s| s == "--download-sections"));
        assert!(argv.iter().any(|s| s == "*10.00-13.50"));
        assert!(argv.iter().any(|s| s == "--force-keyframes-at-cuts"));
    }

    #[test]
    fn build_argv_skips_sub_window_when_invalid_range() {
        // end <= start: skip the sub-window flags entirely (yt-dlp
        // would reject them) rather than passing a bad value.
        let dest = std::path::Path::new("/tmp/foo.mp4");
        let argv = build_yt_dlp_argv("https://youtu.be/abc", dest, Some(10.0), Some(5.0));
        assert!(!argv.iter().any(|s| s == "--download-sections"));
    }

    #[test]
    fn fragment_with_transcript_anchor() {
        let frag = build_edl_fragment(
            "raw/broll/yt-deadbeef.mp4",
            &AnchorArg::Transcript {
                transcript_snippet: "the city skyline".into(),
            },
            2.5,
            "overlay",
        );
        assert!(frag.contains("Insert BRoll"));
        assert!(frag.contains("transcript_snippet=\"the city skyline\""));
        assert!(frag.contains("+ asset: raw/broll/yt-deadbeef.mp4"));
        assert!(frag.contains("+ duration_s: 2.5"));
    }

    #[test]
    fn fragment_escapes_quotes() {
        let frag = build_edl_fragment(
            "raw/broll/yt-1.mp4",
            &AnchorArg::Transcript {
                transcript_snippet: r#"he said "yes""#.into(),
            },
            2.0,
            "overlay",
        );
        assert!(
            frag.contains(r#"transcript_snippet="he said \"yes\"""#),
            "{frag}"
        );
    }

    #[test]
    fn caveat_record_appends_unique() {
        let dir = tempdir().unwrap();
        persist_caveat(dir.path(), "https://youtu.be/abc").unwrap();
        persist_caveat(dir.path(), "https://youtu.be/abc").unwrap(); // duplicate
        persist_caveat(dir.path(), "https://youtu.be/xyz").unwrap();

        let bytes = std::fs::read(caveat_file_path(dir.path())).unwrap();
        let file: CaveatFile = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(file.version, 1);
        assert_eq!(file.acknowledged.len(), 2);
        assert_eq!(file.acknowledged[0].url, "https://youtu.be/abc");
        assert_eq!(file.acknowledged[1].url, "https://youtu.be/xyz");
        // Timestamps are non-empty ISO-8601 strings (sanity check).
        assert!(!file.acknowledged[0].acknowledged_at.is_empty());
    }

    #[test]
    fn caveat_round_trips_existing_file() {
        let dir = tempdir().unwrap();
        persist_caveat(dir.path(), "https://youtu.be/first").unwrap();
        // Second call reads + appends; result should have both.
        persist_caveat(dir.path(), "https://vimeo.com/2").unwrap();
        let file: CaveatFile =
            serde_json::from_slice(&std::fs::read(caveat_file_path(dir.path())).unwrap()).unwrap();
        assert_eq!(file.acknowledged.len(), 2);
    }

    #[test]
    fn download_count_resets_in_tests() {
        DOWNLOAD_COUNT.store(5, Ordering::SeqCst);
        reset_download_count();
        assert_eq!(DOWNLOAD_COUNT.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn truncate_handles_unicode() {
        let out = truncate("héllo wörld", 4);
        assert!(out.ends_with('…'));
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }
}
