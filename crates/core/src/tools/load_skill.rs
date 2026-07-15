//! `load_skill` — L2 progressive-disclosure tool.
//!
//! The agent sees the L1 catalog of skills (one line per skill) as a
//! per-turn contextual fragment. When the user's request maps to a
//! skill (e.g. "tighten this interview" → `interview-tightener`), the
//! agent calls `load_skill(name)` to receive the skill's full
//! `SKILL.md` body — the editorial style, step-by-step playbook, and
//! any references to bundled scripts under `scripts/`.
//!
//! Returning the body as a tool result rather than mutating the
//! system prompt is intentional: it keeps the tier-1 cache stable
//! (system prompt doesn't change mid-session) and lets the agent
//! "consult" multiple skills in one turn if needed (e.g. start with
//! interview-tightener, then escalate to b-roll-suggester for the
//! visual side).

use async_trait::async_trait;
use serde::Deserialize;

use crate::FunctionCallError;
use crate::tool::{ToolContext, ToolHandler, ToolInvocation, ToolOutput};
use crate::tool_schema::Tool as ToolSchema;

/// The `load_skill` tool.
pub struct LoadSkillTool;

#[derive(Debug, Deserialize)]
struct Args {
    /// Skill name (matches the directory name + frontmatter `name`).
    name: String,
}

#[async_trait]
impl ToolHandler for LoadSkillTool {
    fn name(&self) -> &'static str {
        "load_skill"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "load_skill".into(),
            description: DESCRIPTION.into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Skill name from the L1 catalog (e.g. 'interview-tightener')."
                    }
                },
                "required": ["name"]
            }),
            ..Default::default()
        }
    }

    fn is_mutating(&self, _invocation: &ToolInvocation) -> bool {
        false
    }

    async fn handle(
        &self,
        invocation: ToolInvocation,
        ctx: ToolContext,
    ) -> Result<ToolOutput, FunctionCallError> {
        let args: Args = serde_json::from_value(invocation.args).map_err(|e| {
            FunctionCallError::RespondToModel(format!(
                "load_skill: invalid args ({e}). Required: {{ \"name\": <skill> }}."
            ))
        })?;
        let Some(skill) = ctx.skills.get(&args.name) else {
            // Build a "did you mean?" hint with the available list.
            let mut available: Vec<&str> = ctx.skills.all().map(|s| s.meta.name.as_str()).collect();
            available.sort();
            let suggestions = if available.is_empty() {
                "no skills are installed; drop a skill folder under \
                 ~/.config/montage/skills/<name>/SKILL.md or check the \
                 bundled skills location"
                    .to_string()
            } else {
                format!("available skills: {}", available.join(", "))
            };
            return Err(FunctionCallError::RespondToModel(format!(
                "load_skill: no skill named {:?}. {}",
                args.name, suggestions
            )));
        };

        let allowlist = skill.meta.tools_allowlist.clone();
        let active = crate::skill_session::set_active_skill(
            &ctx.project_root,
            skill.meta.name.clone(),
            allowlist,
        )
        .map_err(FunctionCallError::RespondToModel)?;

        // Return the full L2 body, with a frontmatter-stripped header
        // so the model sees the skill name + version up top before
        // diving into the body. We deliberately do NOT include the
        // root path — the agent shouldn't need to know where skills
        // live on disk; bundled scripts are referenced relatively
        // from inside the body.
        let header = format!(
            "Skill loaded: {name} v{version}\n\
             Description: {desc}\n\n\
             --- SKILL.md body ---\n",
            name = skill.meta.name,
            version = skill.meta.version,
            desc = skill.meta.description,
        );
        let mut out = String::with_capacity(header.len() + skill.body.len() + 256);
        out.push_str(&header);
        out.push_str(&skill.body);
        // Tell the agent where to find the skill's bundled scripts +
        // templates. They reference these via relative paths in the
        // body, but the absolute path lets the bash tool find them.
        out.push_str(&format!(
            "\n\n--- Skill resources ---\n\
             Bundled scripts/templates for this skill live at:\n  {}\n\
             Reference them in `bash` calls via that absolute path.",
            skill.root.display()
        ));
        match active {
            Some(active) => out.push_str(&format!(
                "\n\n--- Tool allowlist (enforced) ---\n\
                 Active skill `{}` restricts tools to:\n  {}\n\
                 Other tools will fail until you load a different skill.",
                active.name,
                active.tools_allowlist.join(", ")
            )),
            None => out.push_str(
                "\n\n--- Tool allowlist ---\n\
                 This skill has no tools_allowlist; full tool surface remains available.",
            ),
        }
        Ok(ToolOutput::text(out))
    }
}

const DESCRIPTION: &str = "\
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{Skill, SkillMeta, SkillRegistry};
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn ctx_with_skills(reg: SkillRegistry) -> ToolContext {
        let (tx, _) = broadcast::channel(8);
        ToolContext {
            project_root: std::env::temp_dir(),
            events_tx: tx,
            user_input_tx: None,
            job_manager: montage_render::JobManager::new(),
            approval_tx: None,
            sandbox_mode: crate::tool::SandboxMode::Default,
            mcp_host: crate::mcp_host::McpHost::new(montage_mcp::ClientInfo {
                name: "test".into(),
                version: "0.0.0".into(),
            }),
            skills: Arc::new(reg),
            subagent_return: None,
        }
    }

    fn registry_with(name: &str, body: &str) -> SkillRegistry {
        // Build a registry by hand without filesystem IO. Public-API
        // path requires `discover()` which scans dirs, but for unit
        // tests we just construct the inner state directly via a
        // temp-dir round-trip — keeps tests honest about the actual
        // discovery flow.
        let dir = tempfile::tempdir().unwrap();
        // Leak the tempdir so the on-disk path survives the test
        // (Drop would unlink it before assertions run). std::mem::
        // forget is the cleanest way; tests are short-lived.
        std::fs::create_dir_all(dir.path().join(name)).unwrap();
        std::fs::write(
            dir.path().join(name).join("SKILL.md"),
            format!("---\nname: {name}\ndescription: a test skill\n---\n{body}"),
        )
        .unwrap();
        let path = dir.path().to_path_buf();
        std::mem::forget(dir);
        let (reg, errs) = SkillRegistry::discover(Some(&path), None);
        assert!(errs.is_empty(), "{errs:?}");
        reg
    }

    #[tokio::test]
    async fn returns_skill_body_when_name_matches() {
        let reg = registry_with("test-skill", "# Body\n\nDo the editorial thing.");
        let ctx = ctx_with_skills(reg);
        let out = LoadSkillTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "load_skill".into(),
                    args: serde_json::json!({"name": "test-skill"}),
                },
                ctx,
            )
            .await
            .unwrap();
        assert!(out.content.contains("Skill loaded: test-skill"));
        assert!(out.content.contains("Do the editorial thing."));
        assert!(out.content.contains("Skill resources"));
    }

    #[tokio::test]
    async fn unknown_skill_returns_did_you_mean() {
        let reg = registry_with("real-skill", "body");
        let ctx = ctx_with_skills(reg);
        let err = LoadSkillTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "load_skill".into(),
                    args: serde_json::json!({"name": "wrong-name"}),
                },
                ctx,
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("no skill named"));
                assert!(
                    msg.contains("real-skill"),
                    "should suggest known skills: {msg}"
                );
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_registry_says_no_skills_installed() {
        let ctx = ctx_with_skills(SkillRegistry::default());
        let err = LoadSkillTool
            .handle(
                ToolInvocation {
                    call_id: "c1".into(),
                    name: "load_skill".into(),
                    args: serde_json::json!({"name": "anything"}),
                },
                ctx,
            )
            .await
            .unwrap_err();
        match err {
            FunctionCallError::RespondToModel(msg) => {
                assert!(msg.contains("no skills are installed"));
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    // Suppress unused-import warning when the only usage is the
    // tempfile-leak helper in registry_with.
    #[allow(dead_code)]
    fn _unused_skill(_meta: SkillMeta, _s: Skill) {}
}
