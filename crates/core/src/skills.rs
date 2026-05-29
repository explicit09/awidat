//! Skills — named, prompt-engineered editorial workflows.
//!
//! Per `PLAN.md` §11. The Anthropic Skills shape with three-level
//! progressive disclosure:
//!
//! - **L1**: name + description in a per-turn contextual fragment.
//!   The agent sees a one-line catalog of available skills without
//!   loading full playbooks into the system prompt.
//! - **L2**: full `SKILL.md` body loaded only when relevant — the
//!   agent self-selects via a `load_skill(name)` tool. Even
//!   `awidat skills run <name>` only stages a first-turn prompt that
//!   tells the agent to call `load_skill`.
//! - **L3**: bundled scripts under `<skill>/scripts/`, run via the
//!   existing `bash` tool. The agent doesn't try to do embedding
//!   match in its head — it calls scripts/embedding_search.py.
//!
//! Discovery hierarchy (highest priority first):
//! 1. user skill roots, e.g. `~/Library/Application Support/awidat/skills`
//!    and `~/.config/awidat/skills`
//! 2. `<install_root>/share/awidat/skills/<name>/SKILL.md` (bundled)
//! 3. Walk-up dev fallback: `<repo>/skills/<name>/SKILL.md`
//!
//! User-scoped skills override bundled by `name`. New names append.
//! Same overlay rule as `[[mcp.servers]]` in `awidat-config`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One discovered skill. Owns the parsed frontmatter + the raw L2
/// body text + the directory it came from (for resolving scripts/).
#[derive(Debug, Clone)]
pub struct Skill {
    /// Frontmatter — what the loader reads to build the L1 catalog.
    pub meta: SkillMeta,
    /// Full body text from SKILL.md, frontmatter stripped. This is
    /// what gets returned by `load_skill` at L2.
    pub body: String,
    /// Directory holding this skill's `SKILL.md` + `scripts/` +
    /// `templates/`. Skills can reference these via relative paths
    /// in their body; the agent resolves them through `bash` calls.
    pub root: PathBuf,
}

/// YAML frontmatter at the top of `SKILL.md`. Required: name +
/// description. Everything else optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMeta {
    /// Stable id. Used in `awidat skills run <name>` and in the
    /// L1 catalog line. Must match the directory name.
    pub name: String,
    /// One-sentence summary of what this skill does. Shown to the
    /// agent at L1 ("here's the menu of editorial workflows").
    pub description: String,
    /// Semver-ish; not enforced. Bumped manually when the skill's
    /// behavior changes meaningfully so users can tell different
    /// versions apart.
    #[serde(default = "default_version")]
    pub version: String,
    /// Optional tool name allowlist. When set, the agent only sees
    /// these tools for the duration of this skill turn — prevents
    /// the model from going off-script (e.g. running `bash` mid
    /// b-roll-suggester). Empty/missing = no restriction.
    #[serde(default)]
    pub tools_allowlist: Vec<String>,
    /// Optional grouping tag for `awidat skills list` (editorial /
    /// technical / creative). Free-form.
    #[serde(default)]
    pub tier: Option<String>,
    /// Optional minimum Awidat core version required to run this skill.
    /// Used as a compatibility gate — if the host Awidat is older than
    /// the declared minimum, the skill is skipped from the catalog
    /// with a warning. Same shape as `version` (`MAJOR.MINOR.PATCH`).
    /// Empty / missing = no compatibility check.
    #[serde(default)]
    pub awidat_min_version: Option<String>,
}

fn default_version() -> String {
    "0.1.0".into()
}

/// Errors loading a skill or building the registry.
#[derive(Debug, Error)]
pub enum SkillError {
    /// IO reading SKILL.md.
    #[error("I/O reading skill '{name}' at {path}: {source}")]
    Io {
        /// Skill name (from path).
        name: String,
        /// File that failed.
        path: String,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// YAML frontmatter parse error.
    #[error("malformed frontmatter in skill '{name}': {message}")]
    Frontmatter {
        /// Skill name (from path).
        name: String,
        /// Diagnostic.
        message: String,
    },
    /// SKILL.md doesn't have the `--- ... ---` frontmatter delimiters.
    #[error("skill '{name}' missing YAML frontmatter (expected `---` fenced block at top)")]
    NoFrontmatter {
        /// Skill name (from path).
        name: String,
    },
    /// Frontmatter `name` field doesn't match the directory name.
    #[error("skill at {path} has frontmatter name '{declared}' but directory is '{dir}'")]
    NameMismatch {
        /// Path to the skill dir.
        path: String,
        /// What the frontmatter said.
        declared: String,
        /// What the directory is called.
        dir: String,
    },
}

/// Registry of discovered skills, keyed by name. Built once at
/// session start; cheap (millisecond-range) for any reasonable
/// number of bundled + user skills.
#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    skills: BTreeMap<String, Skill>,
}

impl SkillRegistry {
    /// Discover skills from the standard hierarchy:
    ///
    /// 1. `<bundled_root>/skills/` — bundled with the install. Pass
    ///    the dir resolved by `awidat_config::defaults::python_root`
    ///    (or its `share/awidat/skills` sibling, depending on the
    ///    install layout) — we don't pin the exact location here so
    ///    the caller can pass dev-tree paths during local iteration.
    /// 2. `~/.config/awidat/skills/` — user overrides.
    ///
    /// User entries win on name conflicts. Returns the registry +
    /// any non-fatal load errors (so `awidat skills list` can
    /// surface "this skill is malformed, fix it" without
    /// preventing the rest from loading).
    pub fn discover(
        bundled_root: Option<&Path>,
        user_root: Option<&Path>,
    ) -> (Self, Vec<SkillError>) {
        Self::discover_many(bundled_root, user_root.into_iter())
    }

    /// Discover skills from bundled root plus any number of user roots.
    /// Later user roots win on name conflicts.
    pub fn discover_many<'a>(
        bundled_root: Option<&Path>,
        user_roots: impl IntoIterator<Item = &'a Path>,
    ) -> (Self, Vec<SkillError>) {
        let mut skills: BTreeMap<String, Skill> = BTreeMap::new();
        let mut errors = Vec::new();
        let scan = |root: &Path, errors: &mut Vec<SkillError>| -> Vec<Skill> {
            let mut out = Vec::new();
            let Ok(entries) = std::fs::read_dir(root) else {
                return out;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                match load_skill_dir(&path) {
                    Ok(skill) => out.push(skill),
                    Err(e) => errors.push(e),
                }
            }
            out
        };
        if let Some(root) = bundled_root {
            for s in scan(root, &mut errors) {
                skills.insert(s.meta.name.clone(), s);
            }
        }
        for root in user_roots {
            for s in scan(root, &mut errors) {
                // User overrides bundled by name.
                skills.insert(s.meta.name.clone(), s);
            }
        }
        (Self { skills }, errors)
    }

    /// All discovered skills in name-asc order.
    pub fn all(&self) -> impl Iterator<Item = &Skill> {
        self.skills.values()
    }

    /// Look up by name.
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    /// Build the per-turn fragment carrying the L1 catalog. Returns
    /// `None` when no skills are installed — caller skips emission.
    /// Otherwise returns one `crate::context::AvailableSkillsFragment`
    /// ready to materialize as a tagged user-role message.
    pub fn l1_fragment(&self) -> Option<crate::context::AvailableSkillsFragment> {
        if self.skills.is_empty() {
            return None;
        }
        let skill_lines: Vec<String> = self
            .skills
            .values()
            .map(|s| format!("  - {}: {}", s.meta.name, s.meta.description))
            .collect();
        Some(crate::context::AvailableSkillsFragment { skill_lines })
    }

    /// **Deprecated** — kept temporarily to avoid breaking the few
    /// existing tests that call it. Prefer `l1_fragment()` for new
    /// code; the system prompt no longer carries the catalog.
    #[deprecated(note = "use l1_fragment() — catalog is now per-turn, not in system prompt")]
    pub fn l1_catalog(&self) -> String {
        self.l1_fragment()
            .map(|f| {
                use crate::context::ContextualUserFragment;
                f.render()
            })
            .unwrap_or_default()
    }
}

/// Load one skill from a directory. The directory must contain a
/// `SKILL.md` with YAML frontmatter; the directory's basename is
/// used to cross-check the declared `name`.
fn load_skill_dir(dir: &Path) -> Result<Skill, SkillError> {
    let md_path = dir.join("SKILL.md");
    let dir_name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<unknown>");
    let raw = std::fs::read_to_string(&md_path).map_err(|e| SkillError::Io {
        name: dir_name.to_string(),
        path: md_path.display().to_string(),
        source: e,
    })?;
    let (meta, body) = parse_skill_md(&raw, dir_name)?;
    if meta.name != dir_name {
        return Err(SkillError::NameMismatch {
            path: dir.display().to_string(),
            declared: meta.name,
            dir: dir_name.to_string(),
        });
    }
    Ok(Skill {
        meta,
        body: body.to_string(),
        root: dir.to_path_buf(),
    })
}

/// Parse SKILL.md = YAML frontmatter + body. Frontmatter is fenced
/// by `---` at the start and end. Returns (meta, body_text).
fn parse_skill_md<'a>(raw: &'a str, dir_name: &str) -> Result<(SkillMeta, &'a str), SkillError> {
    let s = raw.trim_start_matches('\u{feff}'); // strip BOM if present
    let s = s
        .strip_prefix("---\n")
        .or_else(|| s.strip_prefix("---\r\n"))
        .ok_or_else(|| SkillError::NoFrontmatter {
            name: dir_name.to_string(),
        })?;
    // Find the closing `---` line.
    let mut end_offset: Option<usize> = None;
    let mut cursor = 0;
    for line in s.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" {
            end_offset = Some(cursor + line.len());
            break;
        }
        cursor += line.len();
    }
    let Some(end) = end_offset else {
        return Err(SkillError::NoFrontmatter {
            name: dir_name.to_string(),
        });
    };
    let frontmatter_text = &s[..cursor]; // before the closing ---
    let body = &s[end..];
    let meta: SkillMeta =
        serde_yaml::from_str(frontmatter_text).map_err(|e| SkillError::Frontmatter {
            name: dir_name.to_string(),
            message: e.to_string(),
        })?;
    Ok((meta, body.trim_start_matches('\n')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(dir: &Path, name: &str, frontmatter: &str, body: &str) {
        let skill_dir = dir.join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\n{frontmatter}\n---\n{body}"),
        )
        .unwrap();
    }

    #[test]
    fn loads_a_minimal_skill() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "podcast-episode-producer",
            "name: podcast-episode-producer\ndescription: produce a published episode from raw recording",
            "# Body\n\nDo the thing.",
        );
        let (reg, errs) = SkillRegistry::discover(Some(&dir.path().join("skills")), None);
        assert!(errs.is_empty(), "expected no errors; got {errs:?}");
        let s = reg.get("podcast-episode-producer").expect("loaded");
        assert_eq!(s.meta.name, "podcast-episode-producer");
        assert_eq!(s.meta.version, "0.1.0", "default version applied");
        assert!(s.body.contains("Do the thing."));
    }

    #[test]
    fn user_overrides_bundled_by_name() {
        let bundled = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        write_skill(
            bundled.path(),
            "interview-tightener",
            "name: interview-tightener\ndescription: bundled version",
            "bundled body",
        );
        write_skill(
            user.path(),
            "interview-tightener",
            "name: interview-tightener\ndescription: user override",
            "user body",
        );
        let (reg, errs) = SkillRegistry::discover(
            Some(&bundled.path().join("skills")),
            Some(&user.path().join("skills")),
        );
        assert!(errs.is_empty());
        let s = reg.get("interview-tightener").unwrap();
        assert_eq!(s.meta.description, "user override");
        assert!(s.body.contains("user body"));
    }

    #[test]
    fn later_user_roots_override_earlier_user_roots() {
        let bundled = tempfile::tempdir().unwrap();
        let xdg_user = tempfile::tempdir().unwrap();
        let platform_user = tempfile::tempdir().unwrap();
        write_skill(
            bundled.path(),
            "technologia-podcast-brand",
            "name: technologia-podcast-brand\ndescription: bundled fallback",
            "bundled body",
        );
        write_skill(
            xdg_user.path(),
            "technologia-podcast-brand",
            "name: technologia-podcast-brand\ndescription: xdg copy",
            "xdg body",
        );
        write_skill(
            platform_user.path(),
            "technologia-podcast-brand",
            "name: technologia-podcast-brand\ndescription: platform copy",
            "platform body",
        );

        let user_roots = [
            xdg_user.path().join("skills"),
            platform_user.path().join("skills"),
        ];
        let (reg, errs) = SkillRegistry::discover_many(
            Some(&bundled.path().join("skills")),
            user_roots.iter().map(PathBuf::as_path),
        );
        assert!(errs.is_empty());
        let s = reg.get("technologia-podcast-brand").unwrap();
        assert_eq!(s.meta.description, "platform copy");
        assert!(s.body.contains("platform body"));
    }

    #[test]
    fn name_mismatch_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "wrong-dir-name",
            "name: declared-different\ndescription: x",
            "body",
        );
        let (_reg, errs) = SkillRegistry::discover(Some(&dir.path().join("skills")), None);
        assert_eq!(errs.len(), 1);
        match &errs[0] {
            SkillError::NameMismatch { dir, declared, .. } => {
                assert_eq!(dir, "wrong-dir-name");
                assert_eq!(declared, "declared-different");
            }
            other => panic!("expected NameMismatch; got {other:?}"),
        }
    }

    #[test]
    fn missing_frontmatter_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("skills/no-frontmatter");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "just a body, no frontmatter").unwrap();
        let (_reg, errs) = SkillRegistry::discover(Some(&dir.path().join("skills")), None);
        assert_eq!(errs.len(), 1);
        assert!(matches!(errs[0], SkillError::NoFrontmatter { .. }));
    }

    #[test]
    fn l1_fragment_lists_skills_alphabetically_with_markers() {
        use crate::context::ContextualUserFragment;
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "b-roll-suggester",
            "name: b-roll-suggester\ndescription: visual cutaway hunting",
            "body",
        );
        write_skill(
            dir.path(),
            "podcast-episode-producer",
            "name: podcast-episode-producer\ndescription: end-to-end episode",
            "body",
        );
        let (reg, _) = SkillRegistry::discover(Some(&dir.path().join("skills")), None);
        let frag = reg.l1_fragment().expect("non-empty registry → fragment");
        let rendered = frag.render();
        assert!(rendered.starts_with("<skills_instructions>"));
        assert!(rendered.ends_with("</skills_instructions>"));
        assert!(rendered.contains("- b-roll-suggester: visual cutaway hunting"));
        assert!(rendered.contains("- podcast-episode-producer: end-to-end episode"));
        // Alpha order: b- comes before p-.
        let b_pos = rendered.find("b-roll-suggester").unwrap();
        let p_pos = rendered.find("podcast-episode-producer").unwrap();
        assert!(b_pos < p_pos);
    }

    #[test]
    fn empty_registry_emits_no_fragment() {
        let reg = SkillRegistry::default();
        assert!(reg.l1_fragment().is_none());
    }

    #[test]
    fn awidat_min_version_round_trips_when_set() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "future-skill",
            "name: future-skill\ndescription: x\nawidat_min_version: \"0.2.0\"",
            "body",
        );
        let (reg, errs) = SkillRegistry::discover(Some(&dir.path().join("skills")), None);
        assert!(errs.is_empty());
        let s = reg.get("future-skill").unwrap();
        assert_eq!(s.meta.awidat_min_version.as_deref(), Some("0.2.0"));
    }

    #[test]
    fn awidat_min_version_absent_when_not_declared() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "current-skill",
            "name: current-skill\ndescription: x",
            "body",
        );
        let (reg, errs) = SkillRegistry::discover(Some(&dir.path().join("skills")), None);
        assert!(errs.is_empty());
        let s = reg.get("current-skill").unwrap();
        assert!(s.meta.awidat_min_version.is_none());
    }

    #[test]
    fn tools_allowlist_round_trips_when_set() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "sample",
            "name: sample\ndescription: x\ntools_allowlist:\n  - find_beat\n  - apply_edl",
            "body",
        );
        let (reg, errs) = SkillRegistry::discover(Some(&dir.path().join("skills")), None);
        assert!(errs.is_empty());
        let s = reg.get("sample").unwrap();
        assert_eq!(s.meta.tools_allowlist, vec!["find_beat", "apply_edl"]);
    }
}
