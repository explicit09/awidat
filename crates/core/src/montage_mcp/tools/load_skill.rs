//! `load_skill` — L2 progressive-disclosure tool. Loads the full
//! `SKILL.md` body of a named editorial skill into the current turn's
//! context. Ported from `crates/core/src/tools/load_skill.rs`.
//!
//! The original `ToolHandler` reads from `ctx.skills`, an
//! `Arc<SkillRegistry>` constructed once per session. The MCP
//! short-lived process model has no session, so this port rediscovers
//! the registry on every call using the same hierarchy
//! (`montage_config::defaults::skills_root()` for bundled skills +
//! `user_skills_roots()` for user overrides). Discovery is cheap
//! (filesystem reads + frontmatter parsing) and bounded by the user's
//! skill count.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;

/// Arguments to `load_skill`.
#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct LoadSkillArgs {
    /// Skill name from the L1 catalog (e.g. 'interview-tightener').
    pub name: String,
}

/// Run `load_skill`. Returns the assembled L2 body as `Ok(String)`,
/// or an error message including a "did you mean?" hint.
pub fn run(args: LoadSkillArgs, _ctx: McpToolCtx) -> Result<String, String> {
    let bundled = montage_config::defaults::skills_root();
    let user_roots = montage_config::defaults::user_skills_roots();
    let (registry, _errors) = crate::skills::SkillRegistry::discover_many(
        bundled.as_deref(),
        user_roots.iter().map(std::path::PathBuf::as_path),
    );

    let Some(skill) = registry.get(&args.name) else {
        let mut available: Vec<&str> = registry.all().map(|s| s.meta.name.as_str()).collect();
        available.sort();
        let suggestions = if available.is_empty() {
            "no skills are installed; drop a skill folder under \
             ~/.config/montage/skills/<name>/SKILL.md or check the \
             bundled skills location"
                .to_string()
        } else {
            format!("available skills: {}", available.join(", "))
        };
        return Err(format!(
            "load_skill: no skill named {:?}. {}",
            args.name, suggestions
        ));
    };

    // Return the full L2 body, with a frontmatter-stripped header so
    // the model sees the skill name + version up top before diving
    // into the body. We deliberately do NOT include the root path —
    // the agent shouldn't need to know where skills live on disk;
    // bundled scripts are referenced relatively from inside the body.
    let header = format!(
        "Skill loaded: {name} v{version}\n\
         Description: {desc}\n\n\
         --- SKILL.md body ---\n",
        name = skill.meta.name,
        version = skill.meta.version,
        desc = skill.meta.description,
    );
    let mut out = String::with_capacity(header.len() + skill.body.len());
    out.push_str(&header);
    out.push_str(&skill.body);
    out.push_str(&format!(
        "\n\n--- Skill resources ---\n\
         Bundled scripts/templates for this skill live at:\n  {}\n\
         Reference them in `bash` calls via that absolute path.",
        skill.root.display()
    ));
    Ok(out)
}

pub const DESCRIPTION: &str = "\
Load the full L2 body of a named editorial skill into the current \
turn's context. Use this when the user's request maps to one of the \
skills listed in the L1 catalog. The \
returned text contains the skill's editorial style, step-by-step \
playbook, and references to bundled scripts you can run via `bash`.\
\n\n\
Examples:\
\n  load_skill(name='interview-tightener') — when the user asks to \
tighten an interview\
\n  load_skill(name='b-roll-suggester')   — when the user asks for \
visual cutaway suggestions\
\n  load_skill(name='podcast-episode-producer') — for the canonical \
end-to-end episode flow\
\n\n\
You can call multiple skills in a single turn if the request spans \
their domains (e.g. tighten THEN suggest b-roll).\
";
