//! `awidat skills` — list and invoke named editorial workflows.
//!
//! Two subcommands:
//!  - `awidat skills list` — print the L1 catalog (name, description,
//!    tier, source dir) of every discovered skill.
//!  - `awidat skills run <name> <project>` — open the TUI with a
//!    pre-staged "please use the <name> skill" prompt so the agent
//!    loads the skill body on its first turn.
//!
//! `run` deliberately does NOT inject the skill body directly into
//! the session prompt. We let the agent call `load_skill(name=...)`
//! through the regular tool path so:
//!  - The L1 → L2 progressive disclosure works the same whether the
//!    skill was invoked by the user OR self-selected by the agent.
//!  - Compaction-aware fragment markers stay coherent (skill bodies
//!    arrive as ToolResult blocks, not as hand-injected user text).
//!  - One code path, two triggers.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use awidat_core::skills::SkillRegistry;

/// Args for `awidat skills list`.
pub struct ListArgs;

/// Args for `awidat skills run`.
pub struct RunArgs {
    /// Skill name to invoke (must match an entry in the L1 catalog).
    pub name: String,
    /// Project directory to open the TUI against.
    pub project: PathBuf,
    /// Override the model id (passed through to `awidat tui`).
    pub model: Option<String>,
}

/// `awidat skills list`. Prints the discovered skill catalog with
/// source paths so the user can edit them.
pub fn list(_args: ListArgs) -> Result<()> {
    let (registry, errors) = discover_registry()?;
    for err in &errors {
        eprintln!("warning: {err}");
    }
    let mut skills: Vec<_> = registry.all().collect();
    skills.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
    if skills.is_empty() {
        println!("No skills installed.");
        println!();
        println!("Drop a skill folder under one of:");
        for root in awidat_config::defaults::user_skills_roots() {
            println!("  {}/<name>/SKILL.md      (user)", root.display());
        }
        if let Some(b) = awidat_config::defaults::skills_root() {
            println!("  {}/<name>/SKILL.md      (bundled)", b.display());
        }
        return Ok(());
    }
    println!("Installed skills:");
    println!();
    for s in skills {
        let tier = s
            .meta
            .tier
            .as_deref()
            .map(|t| format!(" [{t}]"))
            .unwrap_or_default();
        println!("  {}{}  v{}", s.meta.name, tier, s.meta.version);
        println!("    {}", s.meta.description);
        println!("    {}", s.root.display());
        println!();
    }
    println!("Run `awidat skills run <name> <project>` to use one.");
    Ok(())
}

/// `awidat skills run <name> <project>`. Validates the skill exists,
/// then opens the TUI with a stage-1 user message asking the agent
/// to use that skill. The TUI flow is the standard one — no special
/// session shape — but the first prompt is pre-filled so the user
/// only has to hit enter.
pub fn run(args: RunArgs) -> Result<()> {
    if !args.project.is_dir() {
        return Err(anyhow!(
            "project directory not found: {}",
            args.project.display()
        ));
    }
    let (registry, _errors) = discover_registry()?;
    if registry.get(&args.name).is_none() {
        let mut available: Vec<&str> = registry.all().map(|s| s.meta.name.as_str()).collect();
        available.sort();
        let suggestions = if available.is_empty() {
            "no skills installed; run `awidat skills list` to see how to install one".to_string()
        } else {
            format!("available skills: {}", available.join(", "))
        };
        return Err(anyhow!("no skill named {:?}. {suggestions}", args.name));
    }

    println!("Opening TUI with the {} skill staged.", args.name);
    println!(
        "(The agent will call `load_skill(name={:?})` on its first turn to fetch the playbook.)",
        args.name
    );
    println!();

    crate::tui_cmd::run_with_initial_prompt(
        &args.project,
        args.model.as_deref(),
        Some(initial_prompt_for_skill(&args.name)),
    )
}

/// The pre-filled first-turn user message. Concise: tell the agent
/// the user wants this skill, and let load_skill do the rest.
fn initial_prompt_for_skill(name: &str) -> String {
    format!(
        "Use the `{name}` skill for this session. Start by calling \
         `load_skill(name=\"{name}\")` to fetch the full playbook, \
         then follow it. After loading, summarize the playbook in \
         3-5 bullets so I can confirm we're aligned before you start \
         editing."
    )
}

/// Resolve the same registry the runtime uses. Mirrors what
/// `Session::new` does internally; lifted here so the `list` /
/// `run` commands operate against identical state.
fn discover_registry() -> Result<(SkillRegistry, Vec<awidat_core::skills::SkillError>)> {
    let bundled = awidat_config::defaults::skills_root();
    let user_roots = awidat_config::defaults::user_skills_roots();
    let (reg, errs) =
        SkillRegistry::discover_many(bundled.as_deref(), user_roots.iter().map(PathBuf::as_path));
    Ok((reg, errs))
}

// Silence the unused-import lint on the path import in some builds.
#[allow(dead_code)]
fn _unused(_p: &Path) {}
