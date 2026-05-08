//! `awidat upgrade` — re-run install.sh against the same or a new
//! release URL.
//!
//! This is intentionally a thin wrapper around `dist/install.sh` —
//! the installer already knows how to detect platform, fetch a
//! tarball, swap the symlink atomically, and run `uv sync`. Pulling
//! that logic into Rust would duplicate ~150 lines of bash for no
//! benefit, and bash is the right language for "extract a tarball,
//! swap a symlink" anyway.
//!
//! What this Rust wrapper adds:
//!   - Resolves the install.sh location automatically (looks in the
//!     installed tree under `~/.local/share/awidat/current/share/`)
//!   - Surfaces the install with one command instead of three
//!     env-var prefixes
//!   - `--from <url-or-path>` passthrough that handles both `file://`
//!     URLs and bare filesystem paths
//!   - `--check` flag that doesn't actually install — fetches the
//!     remote VERSION (or hashes a local tarball) and prints diff
//!   - Tells the user "you can't `awidat upgrade` from `cargo run`"
//!     when called via the dev binary, since `target/release/awidat`
//!     isn't on the install symlink path.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

/// CLI args. Wired into clap in main.rs.
pub struct UpgradeArgs {
    /// Override the release URL or local path. Without this, falls
    /// back to AWIDAT_RELEASE_BASE env var, then the hardcoded
    /// GitHub Releases URL.
    pub from: Option<String>,
    /// Don't actually install — just print what would change.
    pub check: bool,
    /// Skip the `uv sync` step (passthrough to install.sh).
    pub skip_uv_sync: bool,
}

/// Default release base URL when neither --from nor AWIDAT_RELEASE_BASE
/// is set. Wired to GitHub Releases for the explicit09/awidat repo.
/// Once Phase 3 lands a real CI pipeline this URL will resolve to
/// platform tarballs; until then `awidat upgrade` without --from
/// fails with a clear "no release URL" message.
const DEFAULT_RELEASE_BASE: &str = "https://github.com/explicit09/awidat/releases/latest/download";

/// Entry point.
pub fn run(args: UpgradeArgs) -> Result<()> {
    // Resolve the install layout. We expect the *installed* binary
    // tree (~/.local/share/awidat/current/) to contain the install
    // script under share/awidat/install.sh — that's where package.sh
    // bundles it.
    let installed_root =
        find_installed_root().context("failed to locate the awidat install tree")?;
    let install_script = installed_root.join("share/awidat/install.sh");
    if !install_script.exists() {
        bail!(
            "install script not found at {} — was awidat installed via dist/install.sh? \
             If you're running from `cargo run`, upgrade isn't applicable; rebuild + run \
             dist/install.sh manually.",
            install_script.display()
        );
    }

    // Resolve the source. Priority: --from > AWIDAT_RELEASE_BASE env > default.
    let release_base = args
        .from
        .clone()
        .or_else(|| std::env::var("AWIDAT_RELEASE_BASE").ok())
        .unwrap_or_else(|| DEFAULT_RELEASE_BASE.to_string());
    let release_base = normalize_release_base(&release_base);

    if args.check {
        // --check semantics: in a real release flow, fetch the
        // remote VERSION file and compare to installed. With our
        // current file:// + GitHub Releases setup we don't have a
        // stable VERSION endpoint separate from the tarball — so
        // we just print what's installed and where, and tell the
        // user to run upgrade to actually install. Once Phase 3
        // CI lands a release.json or similar, plumb the comparison
        // here. Tracked as Chunk 4 followup.
        return run_check_lite(&release_base, &installed_root);
    }

    println!("Upgrading awidat from {release_base}...");
    let mut cmd = Command::new("bash");
    cmd.arg(&install_script);
    cmd.env("AWIDAT_RELEASE_BASE", &release_base);
    if args.skip_uv_sync {
        cmd.env("AWIDAT_SKIP_UV_SYNC", "1");
    }
    let status = cmd
        .status()
        .with_context(|| format!("failed to spawn {}", install_script.display()))?;
    if !status.success() {
        bail!(
            "install.sh exited with {} — see output above",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "<signal>".into())
        );
    }
    println!();
    println!("Upgrade complete. New version is active for any new shells.");
    println!(
        "(Your current shell may still hold the old binary in memory until you re-run awidat.)"
    );
    Ok(())
}

/// Locate the install tree by canonicalizing the binary path (to
/// resolve any PATH symlinks like ~/.local/bin/awidat → versions/<sha>/
/// bin/awidat) and walking up looking for the `share/awidat/
/// install.sh` marker the bundler ships.
fn find_installed_root() -> Result<PathBuf> {
    let raw_exe = std::env::current_exe().context("std::env::current_exe failed")?;
    // current_exe() on macOS does NOT auto-resolve symlinks (it
    // returns the path used to invoke the binary). canonicalize()
    // walks the chain. Without this, the installed binary at
    // ~/.local/bin/awidat would walk up to ~/.local/, never finding
    // the install tree under ~/.local/share/awidat/current/.
    let exe = raw_exe.canonicalize().unwrap_or_else(|_| raw_exe.clone());
    let mut cur: Option<&Path> = Some(&exe);
    while let Some(dir) = cur {
        if dir.join("share/awidat/install.sh").exists() {
            return Ok(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    Err(anyhow!(
        "could not locate the awidat install tree (no share/awidat/install.sh \
         found walking up from {}). If you're running from `cargo run` or a \
         dev build, `awidat upgrade` only works against an installed binary.",
        exe.display()
    ))
}

/// Normalize `--from`: bare filesystem paths get the `file://` prefix,
/// URLs are passed through unchanged. Also expands `~`.
fn normalize_release_base(raw: &str) -> String {
    let s = raw.trim();
    if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("file://") {
        return s.to_string();
    }
    // Bare path: `~/foo` or `./bar` or `/abs/path` or `relative/path`.
    let expanded = expand_tilde(s);
    let abs = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&expanded))
            .unwrap_or(expanded)
    };
    format!("file://{}", abs.display())
}

fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(p)
}

/// `--check` mode (lite): print what's installed, where, and the
/// release base that `awidat upgrade` would pull from. Doesn't
/// actually fetch — once Phase 3 CI lands a stable VERSION endpoint
/// we'll add a real diff.
fn run_check_lite(release_base: &str, installed_root: &Path) -> Result<()> {
    let local_version_file = installed_root.join("VERSION");
    let local = std::fs::read_to_string(&local_version_file)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| format!("awidat {} (no VERSION file)", env!("CARGO_PKG_VERSION")));
    println!("Currently installed:");
    for line in local.lines() {
        println!("  {line}");
    }
    println!("  path: {}", installed_root.display());
    println!();
    println!("Would upgrade from:");
    println!("  {release_base}");
    println!();
    println!("Run `awidat upgrade` (without --check) to actually install.");
    Ok(())
}
