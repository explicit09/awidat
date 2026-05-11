//! `awidat new` — one-command project creation.
//!
//! Combines `init` + asset acquisition + (optional) indexing + a
//! starter `AWIDAT.md`. The Cursor / Claude Code / npm playbook for
//! "user installs one thing, gets a working project."
//!
//! Modes:
//! - `--import <URL>` — yt-dlp downloads the video into `raw/`.
//! - `--import <PATH>` — copies (or symlinks) the local file into `raw/`.
//! - omit `--import` — same as `awidat init`, but with the friendlier
//!   next-step output and a starter AWIDAT.md.
//!
//! Default behavior runs `awidat index` synchronously after the
//! source lands, so the user sees the wall time honestly. Pass
//! `--no-index` to skip indexing (useful when dropping multiple
//! assets before a single batch index pass).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use awidat_proto::project::Project;

/// CLI args for `awidat new`. Wired into the clap subcommand in
/// `main.rs`.
pub struct NewArgs {
    /// Project name. Used as the directory name under `cwd` (or
    /// under `--at` if specified).
    pub name: String,
    /// Optional URL or local path to import as the first source.
    pub import: Option<String>,
    /// Where to create the project dir. Defaults to the current
    /// working directory.
    pub at: Option<PathBuf>,
    /// Skip the post-creation `awidat index` run.
    pub no_index: bool,
    /// Skip writing a starter `AWIDAT.md`.
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

    if !args.no_md {
        let md_path = project_dir.join("AWIDAT.md");
        std::fs::write(&md_path, AWIDAT_MD_TEMPLATE).with_context(|| {
            format!("failed to write starter AWIDAT.md at {}", md_path.display())
        })?;
        println!("  ✓ Wrote starter AWIDAT.md (delete or edit to taste)");
    }

    let imported_asset = match args.import.as_deref() {
        Some(src) if is_url(src) => Some(import_url(src, &project_dir)?),
        Some(src) => Some(import_local(Path::new(src), &project_dir, args.link)?),
        None => None,
    };
    if let Some(p) = &imported_asset {
        println!(
            "  ✓ Imported source: {} ({})",
            p.file_name().unwrap_or_default().to_string_lossy(),
            human_size(p)
        );
    }

    println!();
    println!("Project ready: {}", project.root.display());

    if args.no_index || imported_asset.is_none() {
        println!();
        println!("Next:");
        if imported_asset.is_none() {
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
    let assets = vec![
        imported_asset
            .as_ref()
            .expect("import path is some when we reach here")
            .clone(),
    ];
    crate::index_cmd::run(&project_dir, assets, Vec::new(), 4).context("indexing failed")?;

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

    // `--print after_move:filepath` makes yt-dlp print the final
    // resolved path on its own stdout line, which is the only
    // reliable way to know what filename it landed on (id-derived
    // names have unpredictable extensions before merge).
    let out = Command::new("yt-dlp")
        .args([
            "--print",
            "after_move:filepath",
            "-f",
            "bv*[height<=1080]+ba/b[height<=1080]",
            "--merge-output-format",
            "mp4",
            "-o",
            &output_template.to_string_lossy(),
            url,
        ])
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

const AWIDAT_MD_TEMPLATE: &str = "\
# Project conventions

This file is read by awidat at session start and added to the agent's \
system prompt. Use it to record editorial conventions, ground rules, \
and per-episode constraints. Edit freely; remove sections you don't \
need. Subdirectories may also have their own `AWIDAT.md` for narrower \
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
