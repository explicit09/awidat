//! Bundled indexer registry.
//!
//! Montage ships with canonical Python indexers pre-registered. New
//! projects work with zero `[[mcp.servers]]` config — the user's
//! `.montage/config.toml` (and `~/.config/montage/config.toml`) become
//! purely *additive overlays*: add custom indexers, swap a model,
//! or `enabled = false` to disable a default.
//!
//! Consumer release policy: the downloadable desktop app bundles the
//! Python workspace and `uv` sidecar so indexers work out of the box.
//!
//! ## Resolution model
//!
//! Two paths need to be discovered at default-load time:
//!
//! - **Python workspace root** — where the bundled `<name>-mcp`
//!   packages live. Resolved in priority order:
//!     1. `MONTAGE_PYTHON_ROOT` env var (explicit override; dev use)
//!     2. Bundled Tauri resource next to the app executable
//!     3. Walk up from the montage binary looking for `python/`
//!     4. Documented install location: `<prefix>/share/montage/python`
//!     5. Fall back to a literal-empty path; defaults are still
//!        registered, but launching them will fail with a clear error
//!        (`uv` won't find the package). The user can fix by setting
//!        the env var or installing from a packaged release.
//!
//! - **`uv` executable** — bundled sibling sidecar first, then `which uv`,
//!   then `~/.local/bin/uv` (uv's documented install path on macOS/Linux),
//!   else fall back to the literal `"uv"` (assumes PATH at launch).
//!
//! ## Why this lives in montage-config and not montage-cli
//!
//! Multiple consumers need the registry: the CLI's `montage index`,
//! the TUI command's session creation, and any future MCP-host
//! integration test fixture. Putting it under config keeps the data-
//! not-code rule intact (the engine is still data-driven; we just
//! provide better defaults *as data* every install gets).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{HooksConfig, IndexerGroup, IndexerResourceClass, McpConfig, McpServer, McpServerKind};

/// Resolve the python workspace root per the priority model in the
/// module doc. Returns `None` only when every fallback misses; in
/// that case the defaults still register but launching them errors.
pub fn python_root() -> Option<PathBuf> {
    if let Ok(env) = std::env::var("MONTAGE_PYTHON_ROOT") {
        let p = PathBuf::from(env);
        if p.exists() {
            return Some(p);
        }
        // The user explicitly set the var. Trust them — don't fall
        // back. They'll get a clear "package not found" if it's
        // wrong, which is better than silently using a different
        // python tree.
        return Some(p);
    }
    if let Some(found) = bundled_python_root() {
        return Some(found);
    }
    // Walk up from the binary location, then from the current dir.
    // Either should land on the montage repo root in a dev install.
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
        && let Some(found) = walk_up_for_python(parent)
    {
        return Some(found);
    }
    if let Ok(cwd) = std::env::current_dir()
        && let Some(found) = walk_up_for_python(&cwd)
    {
        return Some(found);
    }
    // Documented install locations. Try Homebrew prefix first since
    // that's where `brew install montage` would land it.
    for candidate in [
        "/opt/homebrew/share/montage/python",
        "/usr/local/share/montage/python",
    ] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Some(p);
        }
    }
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".local/share/montage/python");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn bundled_python_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    bundled_python_root_from_exe(&exe)
}

fn bundled_python_root_from_exe(exe: &Path) -> Option<PathBuf> {
    let exe_dir = exe.parent()?;
    for candidate in [
        exe_dir.join("python"),
        exe_dir.join("../Resources/python"),
        exe_dir.join("../Resources/_up_/_up_/_up_/python"),
        exe_dir.join("../resources/python"),
        exe_dir.join("../resources/_up_/_up_/_up_/python"),
    ] {
        if python_workspace_marker_exists(&candidate) {
            return Some(candidate.canonicalize().unwrap_or(candidate));
        }
    }
    None
}

/// Walk up from `start` looking for a directory containing a
/// `python/packages/montage-mcp/pyproject.toml` — the unambiguous
/// marker of the montage python workspace.
fn walk_up_for_python(start: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start);
    while let Some(dir) = cur {
        let candidate = dir.join("python");
        if python_workspace_marker_exists(&candidate) {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

fn python_workspace_marker_exists(candidate: &Path) -> bool {
    candidate
        .join("packages/montage-mcp/pyproject.toml")
        .exists()
}

/// Resolve the bundled-skills directory. Mirrors `python_root` —
/// env override → dev walk-up → documented install paths. Returns
/// `None` only when every fallback misses; the skill loader treats
/// that as "no bundled skills" which is fine (user-installed skills
/// still load via `~/.config/montage/skills/`).
///
/// The dev-tree marker is `<repo>/skills/.bundled-marker` (an empty
/// file we ship to disambiguate the bundled tree from any random
/// directory called `skills/`).
pub fn skills_root() -> Option<PathBuf> {
    if let Ok(env) = std::env::var("MONTAGE_SKILLS_ROOT") {
        return Some(PathBuf::from(env));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
        && let Some(found) = walk_up_for_skills(parent)
    {
        return Some(found);
    }
    if let Ok(cwd) = std::env::current_dir()
        && let Some(found) = walk_up_for_skills(&cwd)
    {
        return Some(found);
    }
    for candidate in [
        "/opt/homebrew/share/montage/skills",
        "/usr/local/share/montage/skills",
    ] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Some(p);
        }
    }
    if let Some(home) = dirs::home_dir() {
        // Tarball installs land bundled skills under
        // `~/.local/share/montage/current/share/montage/skills` (per
        // the layout written by dist/install.sh: bundled assets
        // live next to the binary under `share/montage/`).
        for rel in [
            ".local/share/montage/current/share/montage/skills",
            ".local/share/montage/skills",
        ] {
            let p = home.join(rel);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

fn walk_up_for_skills(start: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start);
    while let Some(dir) = cur {
        let candidate = dir.join("skills");
        if candidate.join(".bundled-marker").exists() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

/// Primary user-scoped skills dir. On macOS this is usually
/// `~/Library/Application Support/montage/skills`; on Linux this is
/// usually `~/.config/montage/skills`.
pub fn user_skills_root() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("montage").join("skills"))
}

/// User-scoped skill directories, ordered from lower to higher
/// priority. We include the XDG-style `~/.config/montage/skills`
/// fallback even on macOS so skills copied there by older docs or
/// hand-written installers still load.
pub fn user_skills_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".config/montage/skills"));
    }
    if let Some(primary) = user_skills_root()
        && !roots.iter().any(|p| p == &primary)
    {
        roots.push(primary);
    }
    roots
}

/// Persistent state root: where session logs (`sessions/<date>/...jsonl`)
/// live. Override with `MONTAGE_STATE_ROOT` (used in tests). Defaults to
/// `~/.local/share/montage`, sibling of the install layout so a user
/// wiping the install doesn't lose conversation history.
pub fn state_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MONTAGE_STATE_ROOT")
        && !p.is_empty()
    {
        return Some(PathBuf::from(p));
    }
    dirs::home_dir().map(|h| h.join(".local/share/montage"))
}

/// Resolve the `uv` executable path. Returns the absolute path when
/// findable; falls back to the literal `"uv"` (assumes PATH).
pub fn uv_command() -> String {
    if let Some(path) = bundled_uv_command() {
        return path.to_string_lossy().into_owned();
    }
    // `which uv` — the canonical resolver. We don't shell out to
    // `which`; we walk PATH ourselves to keep startup deterministic
    // and avoid a process spawn on every Config::load.
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            let candidate = PathBuf::from(dir).join("uv");
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        let candidate = home.join(".local/bin/uv");
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    "uv".to_string()
}

fn bundled_uv_command() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    bundled_uv_command_from_exe(&exe)
}

fn bundled_uv_command_from_exe(exe: &Path) -> Option<PathBuf> {
    let exe_dir = exe.parent()?;
    for file_name in ["uv", "uv.exe"] {
        let candidate = exe_dir.join(file_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// One bundled indexer recipe.
struct IndexerRecipe {
    name: &'static str,
    package: &'static str,
    /// Optional baseline env vars (e.g. `WHISPER_MODEL = "small.en"`).
    env: &'static [(&'static str, &'static str)],
    /// Names of other indexers whose sidecars this one reads. The
    /// dispatcher will not launch this indexer until every name
    /// here has produced its sidecar successfully.
    depends_on: &'static [&'static str],
    resource_class: IndexerResourceClass,
    group: IndexerGroup,
}

/// The canonical list. Adding a new indexer to the montage install
/// is a one-line edit here, no engine changes — the data-not-code
/// rule per `INDEX_SCHEMA.md`. Order is the dispatch order
/// (whisper before topic before editorial-moments because of the
/// dependency chain; visual indexers after).
const RECIPES: &[IndexerRecipe] = &[
    IndexerRecipe {
        name: "audio-energy",
        package: "audio-energy-mcp",
        env: &[],
        depends_on: &[],
        resource_class: IndexerResourceClass::Light,
        group: IndexerGroup::Navigation,
    },
    IndexerRecipe {
        name: "beats",
        package: "beats-mcp",
        env: &[],
        depends_on: &[],
        resource_class: IndexerResourceClass::Light,
        group: IndexerGroup::Navigation,
    },
    IndexerRecipe {
        name: "face",
        package: "face-mcp",
        env: &[],
        depends_on: &[],
        resource_class: IndexerResourceClass::Vision,
        group: IndexerGroup::Vision,
    },
    IndexerRecipe {
        name: "whisper",
        package: "whisper-mcp",
        // distil-large-v3 is a drop-in replacement for large-v3 in
        // every Whisper library — same JSON shape, same wav2vec2
        // alignment, same pyannote diarization path. ~6× faster than
        // large-v3 on CPU with ~1% WER loss, which beats small.en's
        // accuracy AND beats large-v3-turbo's wall-clock on Apple
        // Silicon (no MPS for CTranslate2, so int8/CPU is the only
        // path and large-v3-turbo runs ~0.5× realtime on a 1h source).
        //
        // Users override via global/project config or env directly:
        //   [mcp.servers.env]
        //   WHISPER_MODEL = "large-v3-turbo"   # last 1% of WER
        //   WHISPER_MODEL = "small.en"         # weak hardware
        env: &[("WHISPER_MODEL", "distil-large-v3")],
        depends_on: &[],
        resource_class: IndexerResourceClass::Exclusive,
        group: IndexerGroup::Navigation,
    },
    IndexerRecipe {
        name: "topic",
        package: "topic-mcp",
        env: &[],
        depends_on: &["whisper"],
        resource_class: IndexerResourceClass::Embedding,
        group: IndexerGroup::Navigation,
    },
    IndexerRecipe {
        name: "editorial-moments",
        package: "editorial-moments-mcp",
        env: &[],
        depends_on: &["whisper", "topic"],
        resource_class: IndexerResourceClass::Network,
        group: IndexerGroup::Editorial,
    },
    IndexerRecipe {
        name: "clip",
        package: "clip-mcp",
        env: &[],
        depends_on: &[],
        resource_class: IndexerResourceClass::Embedding,
        group: IndexerGroup::Vision,
    },
    IndexerRecipe {
        name: "color-analysis",
        package: "color-analysis-mcp",
        env: &[],
        depends_on: &[],
        resource_class: IndexerResourceClass::Light,
        group: IndexerGroup::Quality,
    },
    IndexerRecipe {
        name: "frame-quality",
        package: "frame-quality-mcp",
        env: &[],
        depends_on: &[],
        resource_class: IndexerResourceClass::Light,
        group: IndexerGroup::Quality,
    },
    IndexerRecipe {
        name: "scenedetect",
        package: "scenedetect-mcp",
        env: &[],
        depends_on: &[],
        resource_class: IndexerResourceClass::Vision,
        group: IndexerGroup::Navigation,
    },
    IndexerRecipe {
        name: "gaze",
        package: "gaze-mcp",
        env: &[],
        depends_on: &["face"],
        resource_class: IndexerResourceClass::Vision,
        group: IndexerGroup::Vision,
    },
    IndexerRecipe {
        name: "shot",
        package: "shot-mcp",
        env: &[],
        depends_on: &["scenedetect", "face", "gaze"],
        resource_class: IndexerResourceClass::Vision,
        group: IndexerGroup::Vision,
    },
];

/// Materialize the bundled defaults into a [`crate::Config`]. Called
/// from [`crate::Config::load`].
///
/// Resolves `uv` and the python workspace root *once* per call. If
/// the python root can't be found, the entries are still registered
/// (so disable-via-overlay still works); their `cwd` is left unset
/// and launches will fail with a clear "package not found" error.
pub fn with_defaults() -> crate::Config {
    let cmd = uv_command();
    let python_cwd = python_root();
    let uv_project_environment = uv_project_environment_for_python_root(
        python_cwd.as_deref(),
        bundled_python_root().as_deref(),
        state_root().as_deref(),
    );

    let servers = RECIPES
        .iter()
        .map(|recipe| {
            let mut env: HashMap<String, String> = recipe
                .env
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect();
            if let Some(path) = &uv_project_environment {
                env.insert("UV_PROJECT_ENVIRONMENT".into(), path.clone());
            }
            McpServer {
                name: recipe.name.into(),
                command: cmd.clone(),
                args: vec![
                    "run".into(),
                    "--package".into(),
                    recipe.package.into(),
                    recipe.package.into(),
                ],
                env,
                cwd: python_cwd.clone(),
                kind: McpServerKind::Indexer,
                enabled: true,
                depends_on: recipe.depends_on.iter().map(|s| (*s).into()).collect(),
                resource_class: recipe.resource_class,
                indexer_group: Some(recipe.group),
            }
        })
        .collect();

    crate::Config {
        mcp: McpConfig { servers },
        hooks: HooksConfig::default(),
    }
}

fn uv_project_environment_for_python_root(
    python_cwd: Option<&Path>,
    bundled_python_cwd: Option<&Path>,
    state_root: Option<&Path>,
) -> Option<String> {
    if python_cwd? != bundled_python_cwd? {
        return None;
    }
    let state_root = state_root?;
    Some(
        state_root
            .join("python")
            .join(".venv")
            .to_string_lossy()
            .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_default_indexers_registered() {
        let cfg = with_defaults();
        assert_eq!(cfg.mcp.servers.len(), 12);
        let names: Vec<&str> = cfg.mcp.servers.iter().map(|s| s.name.as_str()).collect();
        // Spot-check that the headline indexers are present.
        assert!(names.contains(&"whisper"));
        assert!(names.contains(&"beats"));
        assert!(names.contains(&"clip"));
        assert!(names.contains(&"editorial-moments"));
        assert!(names.contains(&"face"));
        assert!(names.contains(&"color-analysis"));
    }

    #[test]
    fn defaults_are_all_enabled() {
        let cfg = with_defaults();
        assert!(cfg.mcp.servers.iter().all(|s| s.enabled));
    }

    #[test]
    fn defaults_prioritize_measured_non_transcription_critical_path() {
        let cfg = with_defaults();
        let names: Vec<&str> = cfg.mcp.servers.iter().map(|s| s.name.as_str()).collect();

        assert_order(&names, "beats", "face");
        assert_order(&names, "face", "clip");
        assert_order(&names, "clip", "color-analysis");
        assert_order(&names, "color-analysis", "frame-quality");
        assert_order(&names, "frame-quality", "scenedetect");
        assert_order(&names, "scenedetect", "gaze");
        assert_order(&names, "gaze", "shot");
    }

    #[test]
    fn defaults_are_indexer_kind() {
        let cfg = with_defaults();
        assert!(
            cfg.mcp
                .servers
                .iter()
                .all(|s| s.kind == McpServerKind::Indexer)
        );
    }

    #[test]
    fn defaults_include_resource_classes_and_groups() {
        let cfg = with_defaults();
        assert_eq!(
            cfg.find_server("whisper").unwrap().resource_class,
            IndexerResourceClass::Exclusive
        );
        assert_eq!(
            cfg.find_server("clip").unwrap().resource_class,
            IndexerResourceClass::Embedding
        );
        assert_eq!(
            cfg.find_server("face").unwrap().resource_class,
            IndexerResourceClass::Vision
        );
        assert_eq!(
            cfg.find_server("audio-energy").unwrap().resource_class,
            IndexerResourceClass::Light
        );
        assert_eq!(
            cfg.find_server("beats").unwrap().resource_class,
            IndexerResourceClass::Light
        );
        assert_eq!(
            cfg.find_server("topic").unwrap().resource_class,
            IndexerResourceClass::Embedding
        );
        assert_eq!(
            cfg.find_server("editorial-moments").unwrap().resource_class,
            IndexerResourceClass::Network
        );
        assert_eq!(
            cfg.find_server("frame-quality").unwrap().indexer_group,
            Some(IndexerGroup::Quality)
        );
    }

    #[test]
    fn whisper_default_env_includes_small_model() {
        let cfg = with_defaults();
        let whisper = cfg.find_server("whisper").unwrap();
        assert_eq!(
            whisper.env.get("WHISPER_MODEL").map(String::as_str),
            Some("distil-large-v3")
        );
    }

    #[test]
    fn dep_graph_matches_python_indexer_contract() {
        // Known producer→consumer relationships. Locking
        // these in the registry means the dispatcher's topo
        // schedule matches what the python indexers expect to find
        // on disk; mismatch surfaces as the
        // "missing prerequisite sidecar" failure we hit on the
        // first 44-min real-video run.
        let cfg = with_defaults();
        let topic = cfg.find_server("topic").unwrap();
        assert_eq!(topic.depends_on, vec!["whisper".to_string()]);
        let em = cfg.find_server("editorial-moments").unwrap();
        assert_eq!(em.depends_on, vec!["whisper", "topic"]);
        let shot = cfg.find_server("shot").unwrap();
        assert_eq!(shot.depends_on, vec!["scenedetect", "face", "gaze"]);
        let gaze = cfg.find_server("gaze").unwrap();
        assert_eq!(gaze.depends_on, vec!["face"]);
        // No-dep indexers: spot-check.
        for name in [
            "audio-energy",
            "beats",
            "scenedetect",
            "whisper",
            "clip",
            "face",
            "frame-quality",
            "color-analysis",
        ] {
            assert!(
                cfg.find_server(name).unwrap().depends_on.is_empty(),
                "{name} should have no dependencies"
            );
        }
    }

    #[test]
    fn args_use_uv_run_package_pattern() {
        let cfg = with_defaults();
        let clip = cfg.find_server("clip").unwrap();
        assert_eq!(clip.args, vec!["run", "--package", "clip-mcp", "clip-mcp"]);
    }

    #[test]
    fn bundled_uv_command_from_exe_prefers_required_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let uv = dir.path().join("uv");
        std::fs::write(&uv, b"").unwrap();

        let resolved = bundled_uv_command_from_exe(&dir.path().join("montage-desktop"));

        assert_eq!(resolved.as_deref(), Some(uv.as_path()));
    }

    #[test]
    fn bundled_uv_command_from_exe_accepts_windows_sidecar_name() {
        let dir = tempfile::tempdir().unwrap();
        let uv = dir.path().join("uv.exe");
        std::fs::write(&uv, b"").unwrap();

        let resolved = bundled_uv_command_from_exe(&dir.path().join("montage-desktop.exe"));

        assert_eq!(resolved.as_deref(), Some(uv.as_path()));
    }

    #[test]
    fn bundled_python_root_from_exe_checks_tauri_resource_layout() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("MacOS")).unwrap();
        let resources = dir.path().join("Resources");
        let python = resources.join("python");
        std::fs::create_dir_all(python.join("packages/montage-mcp")).unwrap();
        std::fs::write(
            python.join("packages/montage-mcp/pyproject.toml"),
            b"[project]\nname = \"montage-mcp\"\n",
        )
        .unwrap();

        let resolved = bundled_python_root_from_exe(&dir.path().join("MacOS/montage-desktop"));

        let expected = python.canonicalize().unwrap();
        assert_eq!(resolved.as_deref(), Some(expected.as_path()));
    }

    #[test]
    fn uv_project_environment_uses_user_state_for_bundled_python() {
        let python = Path::new("/Applications/Montage.app/Contents/Resources/python");
        let state = Path::new("/Users/example/.local/share/montage");

        let resolved =
            uv_project_environment_for_python_root(Some(python), Some(python), Some(state));

        assert_eq!(
            resolved.as_deref(),
            Some("/Users/example/.local/share/montage/python/.venv")
        );
    }

    #[test]
    fn uv_project_environment_leaves_dev_python_workspace_alone() {
        let dev_python = Path::new("/repo/python");
        let bundled_python = Path::new("/Applications/Montage.app/Contents/Resources/python");
        let state = Path::new("/Users/example/.local/share/montage");

        let resolved = uv_project_environment_for_python_root(
            Some(dev_python),
            Some(bundled_python),
            Some(state),
        );

        assert_eq!(resolved, None);
    }

    // Note: `python_root()` reads `MONTAGE_PYTHON_ROOT` directly. We
    // don't have a unit test that mutates that env var because the
    // workspace forbids `unsafe { std::env::set_var(...) }` and
    // `set_var` is unsafe in 2024-edition Rust. The integration test
    // for the resolution path runs as part of the CLI smoke (where
    // we can set the env in the spawned process).

    fn assert_order(names: &[&str], before: &str, after: &str) {
        let before_index = names.iter().position(|name| *name == before).unwrap();
        let after_index = names.iter().position(|name| *name == after).unwrap();
        assert!(
            before_index < after_index,
            "expected {before:?} before {after:?} in {names:?}"
        );
    }
}
