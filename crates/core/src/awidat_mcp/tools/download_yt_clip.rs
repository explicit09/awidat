//! `download_yt_clip` — fetch a YouTube/Vimeo clip via yt-dlp into
//! `raw/broll/` and return a ready-to-paste `*** Insert BRoll` EDL
//! fragment. Ported from `crates/core/src/tools/download_yt_clip.rs`
//! to the in-process MCP server.
//!
//! Mutating: writes to `raw/broll/` and records the user-acknowledged
//! caveat under `.awidat/yt_caveats.json`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::timeout;

use crate::awidat_mcp::context::McpToolCtx;

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

static DOWNLOAD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Arguments to `download_yt_clip`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct DownloadYtClipArgs {
    /// Source URL. Must match an entry in the allowed-host list.
    pub url: String,
    /// Where on the timeline the cutaway should land. Same anchor
    /// shape as `use_broll` and the EDL grammar.
    pub anchor: AnchorArg,
    /// Cutaway length in seconds (0.5–30).
    pub duration_s: f64,
    /// Optional: trim the source to a sub-window before downloading.
    #[serde(default)]
    pub source_start_s: Option<f64>,
    /// See `source_start_s`.
    #[serde(default)]
    pub source_end_s: Option<f64>,
    /// Position on the timeline. `overlay` (default) or `replace`.
    #[serde(default)]
    pub position: Option<String>,
    /// Copyright acknowledgment gate. The agent MUST set this to
    /// `true` AFTER explaining the licensing situation to the user
    /// AND getting explicit confirmation.
    pub acknowledged: bool,
}

/// Anchor shape (same as `use_broll`).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum AnchorArg {
    Transcript { transcript_snippet: String },
    Uuid { clip_uuid: String },
}

impl Default for AnchorArg {
    fn default() -> Self {
        AnchorArg::Uuid {
            clip_uuid: String::new(),
        }
    }
}

/// Run `download_yt_clip` against the project resolved from
/// [`McpToolCtx`]. Returns a JSON body as `Ok(String)`.
pub async fn run(args: DownloadYtClipArgs, ctx: McpToolCtx) -> Result<String, String> {
    if !args.acknowledged {
        return Err("download_yt_clip: refused — `acknowledged` is false. \
             Before setting acknowledged=true, you MUST explain to the user that \
             third-party clips have copyright implications they're responsible for \
             verifying, AND get their explicit confirmation. Once they confirm, \
             retry with acknowledged=true."
            .into());
    }

    let host = parse_host(&args.url).ok_or_else(|| {
        format!(
            "download_yt_clip: malformed URL '{}'. Pass a full https:// URL.",
            args.url
        )
    })?;
    if !ALLOWED_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
        return Err(format!(
            "download_yt_clip: host '{host}' not allowed. Allowed: {}.",
            ALLOWED_HOSTS.join(", ")
        ));
    }

    let position = match args.position.as_deref().unwrap_or("overlay") {
        "overlay" => "overlay",
        "replace" => "replace",
        other => {
            return Err(format!(
                "download_yt_clip: invalid position '{other}'. Use 'overlay' or 'replace'."
            ));
        }
    };

    if !(0.5..=30.0).contains(&args.duration_s) {
        return Err(format!(
            "download_yt_clip: duration_s={} out of range (0.5..=30.0).",
            args.duration_s
        ));
    }

    let prior = DOWNLOAD_COUNT.load(Ordering::SeqCst);
    if prior >= MAX_DOWNLOADS_PER_SESSION {
        return Err(format!(
            "download_yt_clip: per-session download budget reached ({MAX_DOWNLOADS_PER_SESSION}). \
             Restart the session if this is genuinely needed."
        ));
    }

    let clip_id = url_to_clip_id(&args.url);
    let asset_rel = format!("raw/broll/yt-{clip_id}.mp4");
    let dest = ctx.project_root.join(&asset_rel);
    let downloaded = !dest.exists();

    if downloaded {
        // Make sure raw/broll/ exists before yt-dlp tries to write.
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("download_yt_clip: create raw/broll/ failed: {e}"))?;
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
                    return Err(format!(
                        "download_yt_clip: yt-dlp exited {} for '{}': {}",
                        out.status.code().unwrap_or(-1),
                        args.url,
                        stderr,
                    ));
                }
            }
            Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err("download_yt_clip: yt-dlp not found on PATH. Install via \
                     `brew install yt-dlp` (macOS) or `pipx install yt-dlp` (cross-platform), \
                     then retry."
                    .into());
            }
            Ok(Err(e)) => {
                return Err(format!("download_yt_clip: spawning yt-dlp failed: {e}"));
            }
            Err(_) => {
                return Err(format!(
                    "download_yt_clip: yt-dlp timed out after {DOWNLOAD_TIMEOUT_SECS}s. \
                     Try a smaller source_start_s/source_end_s window or a slower-bitrate format."
                ));
            }
        }

        DOWNLOAD_COUNT.fetch_add(1, Ordering::SeqCst);
    }

    // Persist caveat acknowledgment after the download lands so a
    // failed download doesn't pollute the record.
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
    Ok(body.to_string())
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

pub const DESCRIPTION: &str = "\
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
to actually place the cutaway. Same flow as `use_broll`.";
