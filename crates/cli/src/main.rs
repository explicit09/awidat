//! `awidat` binary entry point.
//!
//! Subcommands today: `init`, `validate`, `index`, `version`. Future
//! (`chat`, `render`, `skills`) land in later weeks per `PLAN.md` §15.

#![allow(clippy::expect_used, clippy::ptr_arg)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{self, Context, Result};
use awidat_proto::project::Project;
use awidat_proto::validate::{ValidationWarning, validate_project};
use clap::{Parser, Subcommand};

mod apply_edl_cmd;
mod chat_cmd;
mod index_cmd;
mod lessons_cmd;
mod new_cmd;
mod plan_clip;
mod plan_dead_air_edl_cmd;
mod plan_ranges;
mod plan_transcript_trim_edl_cmd;
mod render_cmd;
mod resume_cmd;
mod skills_cmd;
mod tui_cmd;
mod upgrade_cmd;

/// Top-level CLI.
#[derive(Parser, Debug)]
#[command(
    name = "awidat",
    version,
    about = "Terminal-first, agent-native video editing harness.",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Subcommands.
#[derive(Subcommand, Debug)]
enum Command {
    /// Create a new project at the given path.
    Init {
        /// Project directory to create. Must not exist or must be empty.
        path: PathBuf,
    },
    /// Create a new project with optional source import + auto-indexing.
    /// The "one command per video" UX: scaffold + acquire source +
    /// index + drop a starter AWIDAT.md.
    New {
        /// Project name. Used as the directory name under `cwd` (or
        /// under `--at`).
        name: String,
        /// Optional URL or local path to import as the first source.
        /// URLs go through yt-dlp; local paths are copied (or linked
        /// with `--link`) into `raw/`.
        #[arg(long)]
        import: Option<String>,
        /// Where to create the project dir. Defaults to the current
        /// working directory.
        #[arg(long)]
        at: Option<PathBuf>,
        /// Skip the post-creation `awidat index` run. Useful when
        /// dropping multiple assets before a single batch index pass.
        #[arg(long)]
        no_index: bool,
        /// Skip writing the starter `AWIDAT.md`.
        #[arg(long)]
        no_md: bool,
        /// Symlink instead of copying when `--import` is a local path.
        #[arg(long)]
        link: bool,
    },
    /// Validate an existing project: OTIO + awidat namespace + edit-plan +
    /// index manifest.
    Validate {
        /// Project directory.
        path: PathBuf,
    },
    /// Run footage indexers over the project's source assets and write
    /// sidecars under `<project>/index/`.
    Index {
        /// Project directory.
        path: PathBuf,
        /// Specific assets to index (one or more). If omitted, all files
        /// under `<project>/raw/` are indexed.
        #[arg(long = "asset")]
        assets: Vec<PathBuf>,
        /// Restrict to specific indexers by name. If omitted, every
        /// indexer-kinded server in config runs.
        #[arg(long = "indexer")]
        indexers: Vec<String>,
        /// Maximum concurrent (server × asset) pairs. 0 = unbounded.
        #[arg(long, default_value_t = 2)]
        concurrency: usize,
    },
    /// Apply an EDL envelope to `project.otio.json`.
    ApplyEdl {
        /// Project directory.
        path: PathBuf,
        /// Path to a text EDL envelope.
        edl: PathBuf,
    },
    /// Render the edited OTIO timeline to `<project>/renders/`.
    Render {
        /// Project directory.
        path: PathBuf,
    },
    /// Detect long silences in the primary source clip and emit a cleanup EDL.
    PlanDeadAirEdl {
        /// Project directory.
        path: PathBuf,
        /// Optional asset, clip name, clip uuid, file name, or file stem to plan against.
        #[arg(long)]
        asset: Option<String>,
        /// Minimum silence duration to remove, in seconds.
        #[arg(long, default_value_t = 0.8)]
        min_duration_s: f64,
        /// Silence threshold in dBFS. Must be negative.
        #[arg(long, default_value_t = -40.0, allow_hyphen_values = true)]
        silence_threshold_db: f64,
        /// Seconds to preserve at each side of a detected silence.
        #[arg(long, default_value_t = 0.0)]
        keep_padding_s: f64,
    },
    /// Find a transcript phrase in the primary source clip and emit a trim EDL.
    PlanTranscriptTrimEdl {
        /// Project directory.
        path: PathBuf,
        /// Transcript phrase whose segment should become the first kept content.
        #[arg(long)]
        keep_from_phrase: String,
        /// Optional asset, clip name, clip uuid, file name, or file stem to plan against.
        #[arg(long)]
        asset: Option<String>,
    },
    /// Detect the first actionable transcript segment and emit a leading-trim EDL.
    PlanTranscriptSetupEdl {
        /// Project directory.
        path: PathBuf,
        /// Optional asset, clip name, clip uuid, file name, or file stem to plan against.
        #[arg(long)]
        asset: Option<String>,
    },
    /// Remove transcript-marked spans and emit an internal-range EDL.
    PlanTranscriptRemoveEdl {
        /// Project directory.
        path: PathBuf,
        /// Transcript phrase whose containing span should be removed. Repeatable.
        #[arg(long = "remove-phrase", required = true)]
        remove_phrases: Vec<String>,
        /// Optional asset, clip name, clip uuid, file name, or file stem to plan against.
        #[arg(long)]
        asset: Option<String>,
    },
    /// Detect filler-heavy transcript spans and emit an internal cleanup EDL.
    PlanTranscriptCleanupEdl {
        /// Project directory.
        path: PathBuf,
        /// Optional asset, clip name, clip uuid, file name, or file stem to plan against.
        #[arg(long)]
        asset: Option<String>,
        /// Minimum filler/discourse-marker ratio for removing a transcript segment.
        #[arg(long, default_value_t = 0.35)]
        min_filler_ratio: f64,
        /// Minimum filler/discourse-marker token count for removing a transcript segment.
        #[arg(long, default_value_t = 2)]
        min_filler_tokens: usize,
    },
    /// Detect transcript restart markers and emit a false-start cleanup EDL.
    PlanFalseStartEdl {
        /// Project directory.
        path: PathBuf,
        /// Optional asset, clip name, clip uuid, file name, or file stem to plan against.
        #[arg(long)]
        asset: Option<String>,
    },
    /// Open a text-only REPL with the agent. Type a prompt; the agent
    /// streams a reply and may call tools (week 3 ships `bash`).
    /// Ctrl-D / EOF / `:quit` to exit.
    Chat {
        /// Project directory. Used today only as the agent's `cwd` for
        /// shell tools; week 5+ wires it into context injection.
        path: PathBuf,
        /// Override the default model id. Defaults to claude-sonnet-4-6.
        #[arg(long)]
        model: Option<String>,
    },
    /// Open a Ratatui chat against a project, with the full editorial
    /// tool registry mounted and approvals routed through a modal.
    /// Ctrl-C cancels the in-flight turn; Ctrl-D from an empty composer
    /// or `:q` from the modal exits.
    Tui {
        /// Project directory.
        path: PathBuf,
        /// Override the default model id. Defaults to claude-sonnet-4-6.
        #[arg(long)]
        model: Option<String>,
    },
    /// Stash the Anthropic API key in the OS keychain so `awidat chat`
    /// and `awidat tui` work without ANTHROPIC_API_KEY in the env.
    /// Reads the key from stdin to avoid shell history.
    SecretsSet {
        /// Account name. Defaults to `anthropic_api_key`.
        #[arg(default_value = "anthropic_api_key")]
        account: String,
    },
    /// List or invoke editorial skills (named workflows).
    Skills {
        /// What to do.
        #[command(subcommand)]
        action: SkillsAction,
    },
    /// Re-run the installer to upgrade in place. Without `--from`,
    /// fetches from AWIDAT_RELEASE_BASE env or the canonical GitHub
    /// Releases URL. Atomic-safe — never overwrites the running
    /// binary.
    Upgrade {
        /// Override the release URL or local path. Examples:
        ///   --from https://example.com/awidat/releases/latest
        ///   --from file:///path/to/dist/build
        ///   --from ./dist/build         (bare path is interpreted as file://)
        #[arg(long)]
        from: Option<String>,
        /// Don't actually install — print what would change.
        #[arg(long)]
        check: bool,
        /// Skip the post-install `uv sync` step (passthrough).
        #[arg(long)]
        skip_uv_sync: bool,
    },
    /// Distill or display learned style from past sessions.
    /// `awidat lessons learn` reads every captured editorial decision
    /// (recorded automatically when you Allow / Deny a modal in the
    /// TUI) and writes patterns to `~/.config/awidat/learned-style.md`.
    /// That file is auto-spliced into the system prompt at session
    /// start. `awidat lessons show` prints the current file.
    Lessons {
        /// What to do.
        #[command(subcommand)]
        action: LessonsAction,
    },
    /// List or resume prior sessions. With no args, lists the 20 most
    /// recent rollout logs (newest first). With `<selector>`, resumes
    /// that session in the TUI: `selector` may be a session id, a
    /// list-index ("1" = most recent), or an absolute path to a JSONL
    /// file.
    Resume {
        /// Session id, list-index, or path. Omit to list.
        selector: Option<String>,
        /// Override the default model id when resuming. Defaults to the
        /// model the original session ran on.
        #[arg(long)]
        model: Option<String>,
    },
    /// Print the version of the awidat binary.
    Version,
}

/// Sub-action for `awidat lessons`.
#[derive(Subcommand, Debug)]
enum LessonsAction {
    /// Read every captured editorial decision and rewrite
    /// `~/.config/awidat/learned-style.md` with the distilled patterns.
    Learn,
    /// Print the current `learned-style.md` body.
    Show,
}

/// Sub-action for `awidat skills`.
#[derive(Subcommand, Debug)]
enum SkillsAction {
    /// List installed skills (bundled + user-installed).
    List,
    /// Open the TUI with a pre-staged "use the <name> skill" first turn.
    /// The agent calls `load_skill(name=...)` itself on its first
    /// response to fetch the playbook.
    Run {
        /// Skill name (must match an entry in `awidat skills list`).
        name: String,
        /// Project directory.
        project: PathBuf,
        /// Override the default model id.
        #[arg(long)]
        model: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let res = match cli.command {
        Command::Init { path } => cmd_init(&path),
        Command::New {
            name,
            import,
            at,
            no_index,
            no_md,
            link,
        } => new_cmd::run(new_cmd::NewArgs {
            name,
            import,
            at,
            no_index,
            no_md,
            link,
        }),
        Command::Validate { path } => cmd_validate(&path),
        Command::Index {
            path,
            assets,
            indexers,
            concurrency,
        } => index_cmd::run(&path, assets, indexers, concurrency),
        Command::ApplyEdl { path, edl } => apply_edl_cmd::run(&path, &edl),
        Command::Render { path } => render_cmd::run(&path),
        Command::PlanDeadAirEdl {
            path,
            asset,
            min_duration_s,
            silence_threshold_db,
            keep_padding_s,
        } => plan_dead_air_edl_cmd::run(plan_dead_air_edl_cmd::PlanDeadAirEdlArgs {
            project_root: path,
            asset,
            min_duration_s,
            silence_threshold_db,
            keep_padding_s,
        }),
        Command::PlanTranscriptTrimEdl {
            path,
            keep_from_phrase,
            asset,
        } => plan_transcript_trim_edl_cmd::run(
            plan_transcript_trim_edl_cmd::PlanTranscriptTrimEdlArgs {
                project_root: path,
                keep_from_phrase,
                asset,
            },
        ),
        Command::PlanTranscriptSetupEdl { path, asset } => plan_transcript_trim_edl_cmd::run_setup(
            plan_transcript_trim_edl_cmd::PlanTranscriptSetupEdlArgs {
                project_root: path,
                asset,
            },
        ),
        Command::PlanTranscriptRemoveEdl {
            path,
            remove_phrases,
            asset,
        } => plan_transcript_trim_edl_cmd::run_remove(
            plan_transcript_trim_edl_cmd::PlanTranscriptRemoveEdlArgs {
                project_root: path,
                remove_phrases,
                asset,
            },
        ),
        Command::PlanTranscriptCleanupEdl {
            path,
            asset,
            min_filler_ratio,
            min_filler_tokens,
        } => plan_transcript_trim_edl_cmd::run_cleanup(
            plan_transcript_trim_edl_cmd::PlanTranscriptCleanupEdlArgs {
                project_root: path,
                asset,
                min_filler_ratio,
                min_filler_tokens,
            },
        ),
        Command::PlanFalseStartEdl { path, asset } => {
            plan_transcript_trim_edl_cmd::run_false_start(
                plan_transcript_trim_edl_cmd::PlanFalseStartEdlArgs {
                    project_root: path,
                    asset,
                },
            )
        }
        Command::Chat { path, model } => chat_cmd::run(&path, model.as_deref()),
        Command::Tui { path, model } => tui_cmd::run(&path, model.as_deref()),
        Command::SecretsSet { account } => cmd_secrets_set(&account),
        Command::Skills { action } => match action {
            SkillsAction::List => skills_cmd::list(skills_cmd::ListArgs),
            SkillsAction::Run {
                name,
                project,
                model,
            } => skills_cmd::run(skills_cmd::RunArgs {
                name,
                project,
                model,
            }),
        },
        Command::Lessons { action } => match action {
            LessonsAction::Learn => lessons_cmd::learn(),
            LessonsAction::Show => lessons_cmd::show(),
        },
        Command::Resume { selector, model } => {
            resume_cmd::run(selector.as_deref(), model.as_deref())
        }
        Command::Upgrade {
            from,
            check,
            skip_uv_sync,
        } => upgrade_cmd::run(upgrade_cmd::UpgradeArgs {
            from,
            check,
            skip_uv_sync,
        }),
        Command::Version => {
            print_version();
            Ok(())
        }
    };
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn cmd_secrets_set(account: &str) -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};
    let stdin = std::io::stdin();
    let value = if stdin.is_terminal() {
        // No rpassword dep — the value will echo when typed. Piping in
        // the key is the unechoed path.
        eprint!("API key: ");
        std::io::stderr().flush().ok();
        let mut buf = String::new();
        stdin.lock().read_line(&mut buf).context("read stdin")?;
        buf.trim().to_string()
    } else {
        let mut buf = String::new();
        stdin.lock().read_line(&mut buf).context("read stdin")?;
        buf.trim().to_string()
    };
    if value.is_empty() {
        return Err(anyhow::anyhow!("empty API key"));
    }
    awidat_secrets::set(account, &value)
        .with_context(|| format!("failed to store secret '{account}' in keychain"))?;
    eprintln!("stored '{account}' in keychain ({} chars)", value.len());
    Ok(())
}

fn print_version() {
    println!("awidat {}", env!("CARGO_PKG_VERSION"));
    println!(
        "supported OTIO schemas: {}",
        awidat_proto::project::supported_schema_summary()
    );
}

fn cmd_init(path: &std::path::Path) -> Result<()> {
    let project = Project::init(path)
        .with_context(|| format!("failed to initialize project at {}", path.display()))?;
    let learned_format = awidat_core::lessons::apply_learned_project_format_defaults(path)
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to apply learned project-format defaults at {}: {e}",
                path.display()
            )
        })?;
    println!("Initialized awidat project at {}", project.root.display());
    println!(
        "  - project.otio.json (Timeline.1, name = {:?})",
        project.timeline.name
    );
    if let Some(aspect_ratio) = learned_format.aspect_ratio.as_deref() {
        println!(
            "  - learned output format defaults: aspect_ratio={aspect_ratio}, platform={}, safe_area={}",
            learned_format.platform.as_deref().unwrap_or("none"),
            learned_format.safe_area.as_deref().unwrap_or("none")
        );
    }
    println!(
        "  - edit-plan.json    (version {})",
        project.edit_plan.version
    );
    println!("  - episode-notes.md");
    println!(
        "  - index/manifest.json ({} indexers)",
        project.manifest.as_ref().map_or(0, |m| m.indexers.len())
    );
    println!("  - renders/, .awidat/");
    println!();
    let cfg = awidat_config::Config::load(Some(path)).unwrap_or_default();
    let indexer_count = cfg.indexers().count();
    println!("Next steps:");
    println!("  1. Drop source media under {}/raw/", path.display());
    println!(
        "  2. Run `awidat index {}` — uses {} bundled indexer(s).",
        path.display(),
        indexer_count
    );
    println!("     Override or disable any default in:");
    println!("       ~/.config/awidat/config.toml          (global)");
    println!(
        "       {}/.awidat/config.toml      (per-project)",
        path.display()
    );
    println!("     Example overlay (turn off whisper for music-only assets):");
    println!("       [[mcp.servers]]");
    println!("       name = \"whisper\"");
    println!("       enabled = false");
    Ok(())
}

fn cmd_validate(path: &std::path::Path) -> Result<()> {
    let project = Project::read(path)
        .with_context(|| format!("failed to read project at {}", path.display()))?;
    let report = validate_project(&project)
        .with_context(|| format!("failed to validate project at {}", path.display()))?;

    let s = &report.summary;
    println!("Project at {} validates clean.", project.root.display());
    println!(
        "  Timeline:   {} tracks, {} clips, {} markers, {} effects",
        s.track_count, s.clip_count, s.marker_count, s.effect_count
    );
    println!("  Edit plan:  {} item(s)", s.plan_item_count);
    println!(
        "  Index:      {} indexer(s), {} sidecar(s)",
        s.indexer_count, s.sidecar_count
    );
    println!(
        "  Awidat metadata version: {}",
        project
            .timeline
            .metadata
            .awidat
            .as_ref()
            .map_or("(none)", |m| m.version.as_str())
    );

    if !report.schema_warnings.is_empty() {
        println!();
        println!("Schema warnings ({}):", report.schema_warnings.len());
        for w in &report.schema_warnings {
            println!(
                "  - {} at {}: schema {} (we support major {}, found {}); reading as supported major.",
                w.file, w.path, w.schema, w.expected_major, w.found_major
            );
        }
    }

    if !report.index_warnings.is_empty() {
        println!();
        println!("Index warnings ({}):", report.index_warnings.len());
        for w in &report.index_warnings {
            print_index_warning(w);
        }
    }

    if project.manifest.is_none() {
        println!();
        println!("Note: index/manifest.json not present — no indexers have run yet, this is fine.");
    }

    Ok(())
}

fn print_index_warning(w: &ValidationWarning) {
    match w {
        ValidationWarning::IndexerDirMissing { indexer } => {
            println!(
                "  - manifest claims indexer '{indexer}' has run, but index/{indexer}/ is missing"
            );
        }
        ValidationWarning::SidecarHeaderMismatch {
            path,
            found_indexer,
            expected_indexer,
        } => {
            println!(
                "  - {path}: header.indexer is '{found_indexer}', but file lives under '{expected_indexer}/'"
            );
        }
        ValidationWarning::SidecarAssetNotInManifest {
            indexer,
            asset,
            path,
        } => {
            println!("  - {path}: asset '{asset}' not listed for '{indexer}' in manifest");
        }
        ValidationWarning::SidecarMalformed { path, message } => {
            println!("  - {path}: malformed sidecar: {message}");
        }
        ValidationWarning::OrphanIndexerDir { indexer } => {
            println!(
                "  - index/{indexer}/ exists but is not in manifest.json — register it or remove it"
            );
        }
        ValidationWarning::InvalidIndexerName { indexer } => {
            println!(
                "  - indexer '{indexer}' is not a valid id — use lowercase letters, digits, and hyphens"
            );
        }
        ValidationWarning::UnsafeAssetId { asset, location } => {
            println!("  - asset id '{asset}' is unsafe: {location}");
        }
        ValidationWarning::SidecarPathMismatch {
            path,
            expected_path,
            asset,
        } => {
            println!("  - {path}: asset '{asset}' should live at {expected_path}");
        }
    }
}
