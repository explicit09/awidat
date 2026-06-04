//! `awidat new` — one-command project creation.
//!
//! Combines `init` + asset acquisition + (optional) indexing + a
//! starter `AGENTS.md`. The Cursor / Claude Code / npm playbook for
//! "user installs one thing, gets a working project."
//!
//! Modes:
//! - `--import <URL>` — yt-dlp downloads the video into `raw/`.
//! - `--import <PATH>` — copies (or symlinks) the local file into `raw/`.
//! - `--import-channel <URL>` — yt-dlp downloads the latest N items
//!   from a playlist/channel into `raw/`. Bound with `--limit <N>`
//!   and/or `--after <YYYY-MM-DD>`.
//! - omit `--import`/`--import-channel` — same as `awidat init`, but
//!   with the friendlier next-step output and a starter AGENTS.md.
//!
//! Default behavior runs `awidat index` synchronously after the
//! source lands, so the user sees the wall time honestly. Pass
//! `--no-index` to skip indexing (useful when dropping multiple
//! assets before a single batch index pass).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use awidat_proto::awidat_meta::AwidatClipMetadata;
use awidat_proto::otio::{
    Clip, ClipMetadata, ExternalReference, MediaReference, RationalTime, StackChild, TimeRange,
    Track, TrackChild, TrackKind,
};
use awidat_proto::project::Project;

/// CLI args for `awidat new`. Wired into the clap subcommand in
/// `main.rs`.
pub struct NewArgs {
    /// Project name. Used as the directory name under `cwd` (or
    /// under `--at` if specified).
    pub name: String,
    /// Optional URL or local path to import as the first source.
    pub import: Option<String>,
    /// Optional playlist/channel URL to batch-import the latest items
    /// from. Mutually exclusive with `import`.
    pub import_channel: Option<String>,
    /// Max number of items to import in channel mode (yt-dlp
    /// `--playlist-end`). Defaults applied in the importer.
    pub limit: Option<u32>,
    /// Only import items uploaded on/after this date (`YYYY-MM-DD` or
    /// `YYYYMMDD`). Channel mode only.
    pub after: Option<String>,
    /// Where to create the project dir. Defaults to the current
    /// working directory.
    pub at: Option<PathBuf>,
    /// Skip the post-creation `awidat index` run.
    pub no_index: bool,
    /// Skip writing a starter `AGENTS.md`.
    pub no_md: bool,
    /// Use a symlink instead of a copy when importing a local path.
    /// Saves disk on large files; not applicable to URL imports.
    pub link: bool,
}

/// Entry point. Returns Ok on success; the caller in main.rs prints
/// the error and exits.
pub fn run(args: NewArgs) -> Result<()> {
    let parent = match args.at {
        Some(p) => p,
        None => std::env::current_dir().context("failed to read current dir")?,
    };
    let project_dir = parent.join(&args.name);
    if project_dir.exists() {
        let mut entries = std::fs::read_dir(&project_dir)
            .with_context(|| format!("failed to inspect {}", project_dir.display()))?;
        if entries.next().is_some() {
            bail!(
                "target directory exists and is not empty: {}",
                project_dir.display()
            );
        }
    }

    println!("Creating awidat project at {}", project_dir.display());
    let project = Project::init(&project_dir)
        .with_context(|| format!("failed to init project at {}", project_dir.display()))?;
    println!("  ✓ Initialized OTIO timeline, edit-plan, manifest, raw/, renders/, .awidat/");
    let learned_format = awidat_core::lessons::apply_learned_project_format_defaults(&project_dir)
        .map_err(|e| {
            anyhow!(
                "failed to apply learned project-format defaults at {}: {e}",
                project_dir.display()
            )
        })?;
    if let Some(aspect_ratio) = learned_format.aspect_ratio.as_deref() {
        println!(
            "  ✓ Applied learned output format defaults: aspect_ratio={aspect_ratio}, platform={}, safe_area={}",
            learned_format.platform.as_deref().unwrap_or("none"),
            learned_format.safe_area.as_deref().unwrap_or("none")
        );
    }

    if !args.no_md {
        let md_path = project_dir.join("AGENTS.md");
        std::fs::write(&md_path, AGENTS_MD_TEMPLATE).with_context(|| {
            format!("failed to write starter AGENTS.md at {}", md_path.display())
        })?;
        println!("  ✓ Wrote starter AGENTS.md (delete or edit to taste)");
    }

    if args.import.is_some() && args.import_channel.is_some() {
        bail!("pass either --import or --import-channel, not both");
    }
    if args.import_channel.is_none() && (args.limit.is_some() || args.after.is_some()) {
        bail!("--limit/--after only apply to --import-channel");
    }

    // Channel mode can yield many assets; single import yields one.
    let imported_assets: Vec<PathBuf> = match (&args.import, &args.import_channel) {
        (Some(src), _) if is_url(src) => vec![import_url(src, &project_dir)?],
        (Some(src), _) => vec![import_local(Path::new(src), &project_dir, args.link)?],
        (None, Some(channel)) => {
            import_channel(channel, args.limit, args.after.as_deref(), &project_dir)?
        }
        (None, None) => Vec::new(),
    };
    // The first imported asset is attached to V1 so the timeline opens
    // with content; the rest land in raw/ for the user to arrange.
    if let Some(p) = imported_assets.first() {
        attach_imported_asset_to_timeline(&project_dir, p)?;
        println!(
            "  ✓ Imported source: {} ({})",
            p.file_name().unwrap_or_default().to_string_lossy(),
            human_size(p)
        );
        println!("  ✓ Added imported source to timeline track V1");
        for extra in imported_assets.iter().skip(1) {
            println!(
                "  ✓ Imported source: {} ({})",
                extra.file_name().unwrap_or_default().to_string_lossy(),
                human_size(extra)
            );
        }
        if imported_assets.len() > 1 {
            println!(
                "  ✓ Imported {} sources from channel into raw/",
                imported_assets.len()
            );
        }
    }
    println!();
    println!("Project ready: {}", project.root.display());

    if args.no_index || imported_assets.is_empty() {
        println!();
        println!("Next:");
        if imported_assets.is_empty() {
            println!("  • Drop source media under {}/raw/", project_dir.display());
        }
        println!(
            "  • awidat index {} — runs the bundled 10 indexers",
            project_dir.display()
        );
        println!(
            "  • awidat tui {}      — chat with the editor",
            project_dir.display()
        );
        return Ok(());
    }

    println!();
    println!("Running indexers (this can take 20+ minutes for hour-long video)...");
    println!(
        "Press Ctrl-C to stop and resume later with `awidat index {}`.",
        project_dir.display()
    );
    println!();
    crate::index_cmd::run(&project_dir, imported_assets, Vec::new(), 4)
        .context("indexing failed")?;

    println!();
    println!("All done. Open the editor:");
    println!("  awidat tui {}", project_dir.display());
    Ok(())
}

/// Heuristic: does this look like a URL we should hand to yt-dlp?
fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Download the URL via yt-dlp. Returns the path of the resulting file.
/// We let yt-dlp pick the best format up to 1080p so podcasts stay a
/// reasonable size; users who want the full bitrate can re-download
/// via `yt-dlp` directly.
fn import_url(url: &str, project_dir: &Path) -> Result<PathBuf> {
    if which_yt_dlp().is_none() {
        bail!(
            "yt-dlp is not installed. Install it (`brew install yt-dlp` or \
             `pip install -U yt-dlp`) and rerun, or use `--import <local-path>` \
             with a file you've already downloaded."
        );
    }
    let raw_dir = project_dir.join("raw");
    std::fs::create_dir_all(&raw_dir).context("failed to create raw/ dir")?;
    let output_template = raw_dir.join("yt-%(id)s.%(ext)s");
    println!("  → Downloading {url} via yt-dlp (1080p max, mp4 merge)...");

    let argv = build_new_yt_dlp_argv(&output_template.to_string_lossy(), url, &None);
    let out = Command::new("yt-dlp")
        .args(&argv)
        .output()
        .context("failed to spawn yt-dlp")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("yt-dlp failed: {stderr}");
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let path_str = stdout.lines().last().unwrap_or("").trim();
    if path_str.is_empty() {
        bail!("yt-dlp succeeded but printed no filepath; aborting");
    }
    let path = PathBuf::from(path_str);
    if !path.exists() {
        bail!("yt-dlp reported {path_str} but file is not on disk");
    }
    Ok(path)
}

/// Bounds for a channel/playlist import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelBounds {
    /// Max items to import (yt-dlp `--playlist-end`).
    limit: u32,
    /// Optional `--dateafter` value, normalized to `YYYYMMDD`.
    dateafter: Option<String>,
}

/// Default cap when no `--limit` is supplied in channel mode, so a
/// huge channel does not download in full by accident.
pub const DEFAULT_CHANNEL_LIMIT: u32 = 10;

/// Import the latest N items from a playlist/channel URL. Returns the
/// resolved paths in playlist order. Shares format/merge defaults with
/// single-URL import via [`build_new_yt_dlp_argv`].
fn import_channel(
    url: &str,
    limit: Option<u32>,
    after: Option<&str>,
    project_dir: &Path,
) -> Result<Vec<PathBuf>> {
    if which_yt_dlp().is_none() {
        bail!(
            "yt-dlp is not installed. Install it (`brew install yt-dlp` or \
             `pip install -U yt-dlp`) and rerun."
        );
    }
    let bounds = channel_bounds(limit, after)?;
    let raw_dir = project_dir.join("raw");
    std::fs::create_dir_all(&raw_dir).context("failed to create raw/ dir")?;
    let output_template = raw_dir.join("yt-%(id)s.%(ext)s");
    match &bounds.dateafter {
        Some(d) => println!(
            "  → Downloading up to {} item(s) from channel {url} (after {d}) via yt-dlp...",
            bounds.limit
        ),
        None => println!(
            "  → Downloading up to {} item(s) from channel {url} via yt-dlp...",
            bounds.limit
        ),
    }

    let argv = build_new_yt_dlp_argv(&output_template.to_string_lossy(), url, &Some(bounds));
    let out = Command::new("yt-dlp")
        .args(&argv)
        .output()
        .context("failed to spawn yt-dlp")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("yt-dlp failed: {stderr}");
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let paths = parse_channel_filepaths(&stdout);
    if paths.is_empty() {
        bail!(
            "yt-dlp succeeded but reported no downloaded files; check the channel URL / date bound"
        );
    }
    for p in &paths {
        if !p.exists() {
            bail!("yt-dlp reported {} but file is not on disk", p.display());
        }
    }
    Ok(paths)
}

/// Validate `--limit`/`--after` and normalize into [`ChannelBounds`].
fn channel_bounds(limit: Option<u32>, after: Option<&str>) -> Result<ChannelBounds> {
    if let Some(0) = limit {
        bail!("--limit must be >= 1");
    }
    let dateafter = match after {
        Some(raw) => Some(
            normalize_date_yyyymmdd(raw)
                .ok_or_else(|| anyhow!("--after must be YYYY-MM-DD or YYYYMMDD"))?,
        ),
        None => None,
    };
    Ok(ChannelBounds {
        limit: limit.unwrap_or(DEFAULT_CHANNEL_LIMIT),
        dateafter,
    })
}

/// Normalize a date to yt-dlp's `YYYYMMDD`. Accepts `YYYY-MM-DD` or
/// `YYYYMMDD`; `None` if not a plausible 8-digit date.
fn normalize_date_yyyymmdd(raw: &str) -> Option<String> {
    if raw.chars().any(|c| !c.is_ascii_digit() && c != '-') {
        return None;
    }
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    (digits.len() == 8).then_some(digits)
}

/// Build the yt-dlp argv shared by single and channel imports. Pure /
/// deterministic for unit testing. `--print after_move:filepath` makes
/// yt-dlp emit one final resolved path per item. Single mode pins
/// `--no-playlist`; channel mode opts into `--yes-playlist` plus the
/// bound flags.
pub fn build_new_yt_dlp_argv(
    output_template: &str,
    url: &str,
    channel: &Option<ChannelBounds>,
) -> Vec<String> {
    let mut argv = vec![
        "--print".to_string(),
        "after_move:filepath".to_string(),
        "-f".to_string(),
        "bv*[height<=1080]+ba/b[height<=1080]".to_string(),
        "--merge-output-format".to_string(),
        "mp4".to_string(),
    ];
    match channel {
        Some(bounds) => {
            argv.push("--yes-playlist".to_string());
            argv.push("--playlist-end".to_string());
            argv.push(bounds.limit.to_string());
            if let Some(dateafter) = &bounds.dateafter {
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

/// Parse the per-item filepaths from `--print after_move:filepath`
/// stdout (one path per non-empty line), preserving order.
fn parse_channel_filepaths(stdout: &str) -> Vec<PathBuf> {
    stdout
        .lines()
        .map(|line| line.trim_end_matches('\r').trim())
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Bring a local file into the project's raw/ dir. Symlink when
/// `--link` is set, copy otherwise. Symlinks are faster + save disk
/// for big assets; copies make the project portable (e.g. you can
/// move the project dir to another machine without re-resolving
/// the source).
fn import_local(src: &Path, project_dir: &Path, link: bool) -> Result<PathBuf> {
    if !src.exists() {
        bail!("source file not found: {}", src.display());
    }
    let raw_dir = project_dir.join("raw");
    std::fs::create_dir_all(&raw_dir).context("failed to create raw/ dir")?;
    let filename = src
        .file_name()
        .ok_or_else(|| anyhow!("source path has no filename: {}", src.display()))?;
    let dst = raw_dir.join(filename);
    if dst.exists() {
        bail!(
            "a file named {} already exists in raw/",
            filename.to_string_lossy()
        );
    }
    if link {
        // Use absolute path for the symlink so it survives moving
        // the project dir. (If the user wants a relative symlink they
        // can mklink themselves.)
        let abs = src
            .canonicalize()
            .with_context(|| format!("failed to resolve absolute path of {}", src.display()))?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&abs, &dst)
            .with_context(|| format!("symlink {} -> {} failed", dst.display(), abs.display()))?;
        #[cfg(not(unix))]
        std::fs::copy(&abs, &dst).with_context(|| {
            format!(
                "copy {} -> {} failed (symlinks unsupported on this platform)",
                abs.display(),
                dst.display()
            )
        })?;
    } else {
        std::fs::copy(src, &dst)
            .with_context(|| format!("copy {} -> {} failed", src.display(), dst.display()))?;
    }
    Ok(dst)
}

fn attach_imported_asset_to_timeline(project_dir: &Path, imported_asset: &Path) -> Result<()> {
    let mut project = Project::read(project_dir)
        .with_context(|| format!("failed to read project at {}", project_dir.display()))?;
    let asset_id = imported_asset
        .strip_prefix(project_dir)
        .with_context(|| {
            format!(
                "imported asset {} is not inside project {}",
                imported_asset.display(),
                project_dir.display()
            )
        })?
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let duration_s = probe_duration_s(imported_asset)?;
    let available_range = TimeRange::new(
        RationalTime::zero(30.0),
        RationalTime::new(duration_s * 30.0, 30.0),
    );
    let mut reference = ExternalReference::new(asset_id.clone());
    reference.available_range = Some(available_range);

    let clip_name = imported_asset
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("imported-source")
        .to_string();
    let mut clip = Clip::empty(clip_name.clone());
    clip.source_range = Some(available_range);
    clip.media_reference = MediaReference::External(reference);
    clip.metadata = ClipMetadata {
        awidat: Some(AwidatClipMetadata {
            reasoning: Some("Initial clip created from awidat new --import.".to_string()),
            extra: [(
                "clip_uuid".to_string(),
                serde_json::Value::String(slug_clip_uuid(&clip_name)),
            )]
            .into_iter()
            .collect(),
            ..AwidatClipMetadata::default()
        }),
        ..ClipMetadata::default()
    };

    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.push(TrackChild::Clip(clip));
    project.timeline.tracks.children = vec![StackChild::Track(track)];
    if let Some(metadata) = project.timeline.metadata.awidat.as_mut()
        && !metadata
            .source_assets
            .iter()
            .any(|asset| asset == &asset_id)
    {
        metadata.source_assets.push(asset_id);
    }
    project
        .write(project_dir)
        .with_context(|| format!("failed to write project at {}", project_dir.display()))?;
    Ok(())
}

fn probe_duration_s(asset: &Path) -> Result<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(asset)
        .output()
        .with_context(|| format!("failed to spawn ffprobe for {}", asset.display()))?;
    if !output.status.success() {
        bail!(
            "ffprobe failed for {}: {}",
            asset.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let duration_s = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .with_context(|| format!("ffprobe returned invalid duration for {}", asset.display()))?;
    if !duration_s.is_finite() || duration_s <= 0.0 {
        bail!(
            "ffprobe returned unusable duration {duration_s} for {}",
            asset.display()
        );
    }
    Ok(duration_s)
}

fn slug_clip_uuid(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let compact = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if compact.is_empty() {
        "clip-imported-source".to_string()
    } else {
        format!("clip-{compact}")
    }
}

fn which_yt_dlp() -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in path_var.split(':') {
        let p = PathBuf::from(dir).join("yt-dlp");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn human_size(p: &Path) -> String {
    let Ok(meta) = std::fs::metadata(p) else {
        return "unknown size".into();
    };
    let bytes = meta.len();
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

const AGENTS_MD_TEMPLATE: &str = "\
# Project conventions

This file is read by awidat at session start and added to the agent's \
system prompt. Use it to record editorial conventions, ground rules, \
and per-episode constraints. Edit freely; remove sections you don't \
need. Subdirectories may also have their own `AGENTS.md` for narrower \
scope.

## Speakers

- Speaker A: <name / role>
- Speaker B: <name / role>

## Style

- Cut breath buffer: 200ms before / 100ms after each take
- Cross-talk: prefer the speaker who finishes their thought
- Filler removal: aggressive on um/uh, conservative on 'like'/'you know'

## Avoid

- Don't trim hooks below 5s
- Don't render with hard cuts mid-laugh
- Don't move the CTA out of the closing third

## Render targets

- Master: 1080p, ProRes 422 (or full-quality H.264 if no ProRes pipeline)
- Social: 1080×1920 (vertical) crops of standalone moments
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_argv_keeps_no_playlist_and_format() {
        let argv = build_new_yt_dlp_argv("raw/yt-%(id)s.%(ext)s", "https://x/v", &None);
        assert!(argv.contains(&"--no-playlist".to_string()));
        assert!(!argv.contains(&"--yes-playlist".to_string()));
        assert!(!argv.iter().any(|a| a == "--playlist-end"));
        assert!(argv.contains(&"bv*[height<=1080]+ba/b[height<=1080]".to_string()));
        assert_eq!(argv.last().unwrap(), "https://x/v");
        let print_idx = argv.iter().position(|a| a == "--print").unwrap();
        assert_eq!(argv[print_idx + 1], "after_move:filepath");
    }

    #[test]
    fn channel_argv_has_playlist_end_and_dateafter() {
        let bounds = channel_bounds(Some(3), Some("2025-02-01")).unwrap();
        let argv = build_new_yt_dlp_argv("raw/yt-%(id)s.%(ext)s", "https://x/chan", &Some(bounds));
        assert!(argv.contains(&"--yes-playlist".to_string()));
        assert!(!argv.contains(&"--no-playlist".to_string()));
        let pe_idx = argv.iter().position(|a| a == "--playlist-end").unwrap();
        assert_eq!(argv[pe_idx + 1], "3");
        let da_idx = argv.iter().position(|a| a == "--dateafter").unwrap();
        assert_eq!(argv[da_idx + 1], "20250201");
        assert_eq!(argv.last().unwrap(), "https://x/chan");
    }

    #[test]
    fn channel_bounds_defaults_limit_and_validates() {
        let b = channel_bounds(None, None).unwrap();
        assert_eq!(b.limit, DEFAULT_CHANNEL_LIMIT);
        assert_eq!(b.dateafter, None);
        assert!(channel_bounds(Some(0), None).is_err());
        assert!(channel_bounds(Some(2), Some("garbage")).is_err());
        assert!(channel_bounds(Some(2), Some("2025/02/01")).is_err());
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
        assert_eq!(normalize_date_yyyymmdd("nope"), None);
    }

    #[test]
    fn parse_channel_filepaths_preserves_order_and_skips_blanks() {
        let stdout = "raw/yt-a.mp4\n\nraw/yt-b.mp4\r\n";
        let paths = parse_channel_filepaths(stdout);
        assert_eq!(
            paths,
            vec![PathBuf::from("raw/yt-a.mp4"), PathBuf::from("raw/yt-b.mp4")]
        );
    }
}
