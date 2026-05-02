//! `awidat` binary entry point.
//!
//! Week 1 surface: `init`, `validate`, `version`. Future subcommands
//! (`index`, `chat`, `render`, `skills`) land in later weeks per
//! `PLAN.md` §15.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use awidat_proto::project::Project;
use awidat_proto::validate::{ValidationWarning, validate_project};
use clap::{Parser, Subcommand};

/// Top-level CLI. Subcommands match `PLAN.md` §15 Week 1.
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
    /// Validate an existing project: OTIO + awidat namespace + edit-plan +
    /// index manifest.
    Validate {
        /// Project directory.
        path: PathBuf,
    },
    /// Print the version of the awidat binary.
    Version,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let res = match cli.command {
        Command::Init { path } => cmd_init(&path),
        Command::Validate { path } => cmd_validate(&path),
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
    println!("Initialized awidat project at {}", project.root.display());
    println!(
        "  - project.otio.json (Timeline.1, name = {:?})",
        project.timeline.name
    );
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
    println!(
        "Next: edit project.otio.json, or run `awidat validate {}`.",
        path.display()
    );
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
