//! Sub-agents — restricted-tool research agents the parent agent can
//! delegate isolated questions to.
//!
//! Per the cross-harness survey + `harnesses/cline/src/core/task/tools/subagent/SubagentBuilder.ts:14-23`:
//! a sub-agent is a fresh `Session` with a *narrowed* tool registry, a
//! task-specific system suffix, and exactly ONE way to return data to
//! the parent — the `attempt_completion(result=<str>)` tool. Cline's
//! pattern is research-only (read tools + bash), and we adopt the same
//! constraint for V1: sub-agents do not call `apply_edl`, do not call
//! `start_render`, do not call mutating tools at all. Mutating
//! sub-agents are deferred to #153 (parallel persona reviewers) once
//! V1 ships.
//!
//! V1 ships 3 research sub-agents:
//! - `episode-explorer`: "what's in this episode?" — view + read tools
//! - `cut-scout`: "find me the strongest 5 X" — find_beat / inspect_moment
//! - `b-roll-hunter`: "find cutaways for this passage" — vision tools
//!
//! Each is defined declaratively below. Adding a 4th sub-agent =
//! one entry in `default_subagents()` — no code changes elsewhere.

use std::sync::Arc;

/// Declarative spec for one sub-agent.
#[derive(Debug, Clone)]
pub struct SubAgent {
    /// Stable id used as the `name` argument of `delegate`. Lowercase
    /// kebab so it reads in the parent's tool-call args naturally.
    pub name: &'static str,
    /// One-line description surfaced in the parent's tool catalog.
    pub description: &'static str,
    /// Appended to the parent's base system prompt for this run. Tells
    /// the sub-agent what it's for and reminds it to call
    /// `attempt_completion`.
    pub system_suffix: &'static str,
    /// Allow-list of tool names. The parent's full registry is
    /// filtered to this subset. `attempt_completion` is auto-mounted
    /// on top of this list.
    pub allowed_tools: &'static [&'static str],
    /// Hard cap on outer-loop iterations for this sub-agent. Sub-agents
    /// are research-bounded; long sub-sessions are usually a sign of a
    /// confused prompt, not a real research need. 16 is conservative.
    pub max_iterations: usize,
    /// What kind of sub-agent this is. Filters which `delegate*` tool
    /// can spawn it.
    pub kind: SubAgentKind,
}

/// Distinguishes "research" sub-agents (single-shot, called by
/// `delegate`) from "persona reviewers" (multi-shot parallel, called
/// by `delegate_all`). Per #153: parallel personas are the substrate
/// for "what would the editor / fact-checker / brand-voice think of
/// this cut?" — building blocks exist across harnesses but no one
/// runs them in parallel as a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubAgentKind {
    /// Single-shot read-only research. Called by `delegate(name, task)`.
    Research,
    /// Persona reviewer for a draft cut. Multiple run in parallel via
    /// `delegate_all(personas, task)` and each one's
    /// `attempt_completion` becomes a section of the consolidated
    /// review.
    Persona,
}

/// In-memory directory of sub-agents discoverable via `delegate`.
#[derive(Debug, Clone)]
pub struct SubAgentRegistry {
    agents: Vec<Arc<SubAgent>>,
}

impl SubAgentRegistry {
    /// Built-in V1 directory.
    pub fn defaults() -> Self {
        Self {
            agents: default_subagents().into_iter().map(Arc::new).collect(),
        }
    }

    /// Empty registry — for tests and the case where the operator
    /// wants `delegate` available but no sub-agents installed.
    pub fn empty() -> Self {
        Self { agents: Vec::new() }
    }

    /// Look up by name.
    pub fn get(&self, name: &str) -> Option<Arc<SubAgent>> {
        self.agents.iter().find(|a| a.name == name).cloned()
    }

    /// Number of sub-agents available.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// True iff no sub-agents are installed.
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    /// Iterate sub-agents in registration order. Used to render the
    /// catalog inside the `delegate` tool's description so the parent
    /// agent knows what's available.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<SubAgent>> {
        self.agents.iter()
    }

    /// Iterate sub-agents whose `kind` matches the filter. Used by
    /// `delegate` (Research) and `delegate_all` (Persona) to scope
    /// their catalog so each tool's description only lists what it
    /// can actually spawn.
    pub fn iter_kind(&self, kind: SubAgentKind) -> impl Iterator<Item = &Arc<SubAgent>> {
        self.agents.iter().filter(move |a| a.kind == kind)
    }
}

impl Default for SubAgentRegistry {
    fn default() -> Self {
        Self::defaults()
    }
}

const RESEARCH_SUFFIX: &str = "\n\n# Sub-agent execution mode\n\
You are running as a research sub-agent. Your job is to gather \
information, not to commit edits. Read the tools you have, gather \
what's needed, and report back.\n\
\n\
You have a constrained tool surface — no apply_edl, no start_render, \
no bash. If you need a tool you don't have, surface that in your \
final answer; don't try to work around it.\n\
\n\
**Return path: `attempt_completion(result=<str>)`.** That string is \
sent verbatim to the parent agent — make it the answer the parent \
needs. Keep it concise: lead with the answer, then bullet the \
supporting evidence (file paths, timestamps, scores). The parent \
will read every line, so don't pad.\n\
\n\
You are done when you've called `attempt_completion`. Calling it \
ends your turn. Don't call other tools after.";

const PERSONA_REVIEW_SUFFIX: &str = "\n\n# Persona-review mode\n\
You are running as a persona reviewer. Your job is to review the \
parent's draft cut from a SPECIFIC editorial lens — your persona, \
described above. Stay strictly in that lens. Don't critique outside \
your area; another persona is doing that in parallel.\n\
\n\
Tool surface is read-only — you cannot edit, render, or shell out. \
Read the timeline, inspect clips and moments, look at the \
transcript, then report.\n\
\n\
**Return format.** Lead with a one-line summary verdict (LGTM / \
concerns / blocker). Then a bullet list of specific findings, each \
with a timestamp and a one-sentence reason. Cite the data you saw, \
don't paraphrase your reaction. If your lens has nothing to say on \
this cut, say so explicitly — the parent prefers a clean LGTM to \
fabricated criticism.\n\
\n\
**Return path: `attempt_completion(result=<str>)`.** That string is \
sent verbatim back, alongside other personas' returns. Keep it under \
~200 words; the parent reads every persona.\n\
\n\
You are done when you've called `attempt_completion`. Calling it \
ends your turn. Don't call other tools after.";

fn default_subagents() -> Vec<SubAgent> {
    vec![
        SubAgent {
            name: "episode-explorer",
            description: "Read-only research sub-agent that produces a \
                          structured overview of an episode: speakers, \
                          topics, high-energy beats, indexed channels. \
                          Use when the parent needs a quick map before \
                          picking a direction. Returns a one-screen \
                          summary.",
            system_suffix: RESEARCH_SUFFIX,
            allowed_tools: &[
                "view_episode",
                "view_timeline",
                "list_assets",
                "find_beat",
                "inspect_moment",
                "read_index",
                "shot_summary",
                "attempt_completion",
            ],
            max_iterations: 16,
            kind: SubAgentKind::Research,
        },
        SubAgent {
            name: "cut-scout",
            description: "Read-only research sub-agent that scouts the \
                          strongest editorial moments by kind (hook, \
                          punchline, cta, emotional_peak). Returns a \
                          ranked list with timestamps, scores, \
                          dependencies, and a recommended cut order.",
            system_suffix: RESEARCH_SUFFIX,
            allowed_tools: &[
                "find_beat",
                "inspect_moment",
                "find_moment",
                "view_episode",
                "read_index",
                "attempt_completion",
            ],
            max_iterations: 16,
            kind: SubAgentKind::Research,
        },
        SubAgent {
            name: "b-roll-hunter",
            description: "Read-only research sub-agent that hunts \
                          visual cutaways for a spoken passage. \
                          Cross-references shot type, frame quality, \
                          and CLIP semantic match. Returns 3-5 \
                          candidates with scores and reasons.",
            system_suffix: RESEARCH_SUFFIX,
            allowed_tools: &[
                "broll_candidates",
                "clip_search",
                "shot_summary",
                "find_speaker_oncam",
                "view_frame",
                "inspect_clip",
                "view_timeline",
                "attempt_completion",
            ],
            max_iterations: 16,
            kind: SubAgentKind::Research,
        },
        // ---- Personas (#153) -------------------------------------
        SubAgent {
            name: "editor",
            description: "Persona reviewer focused on PACING and \
                          ATTENTION. Watches for: dead air kept that \
                          shouldn't be, cuts that land mid-thought, \
                          tangents that lose the audience, hook that \
                          isn't hook-shaped. Cites timestamps; doesn't \
                          rewrite the cut.",
            system_suffix: PERSONA_REVIEW_SUFFIX,
            allowed_tools: &[
                "view_timeline",
                "view_episode",
                "inspect_clip",
                "inspect_moment",
                "find_beat",
                "find_moment",
                "read_index",
                "attempt_completion",
            ],
            max_iterations: 12,
            kind: SubAgentKind::Persona,
        },
        SubAgent {
            name: "fact-checker",
            description: "Persona reviewer focused on CLAIMS and \
                          ATTRIBUTIONS. Watches for: confident-sounding \
                          numbers without sources, names mispronounced \
                          or wrong, dates and stats that look off. \
                          Doesn't try to verify externally; flags the \
                          claim and where to verify.",
            system_suffix: PERSONA_REVIEW_SUFFIX,
            allowed_tools: &[
                "view_timeline",
                "view_episode",
                "inspect_clip",
                "inspect_moment",
                "find_moment",
                "read_index",
                "attempt_completion",
            ],
            max_iterations: 12,
            kind: SubAgentKind::Persona,
        },
        SubAgent {
            name: "brand-voice",
            description: "Persona reviewer focused on TONE and \
                          ON-BRAND-NESS. Watches for: filler that \
                          undermines authority, off-brand language, \
                          register shifts (casual where formal is \
                          warranted, formal where casual lands better). \
                          Cites the moment, names the tone problem.",
            system_suffix: PERSONA_REVIEW_SUFFIX,
            allowed_tools: &[
                "view_timeline",
                "view_episode",
                "inspect_clip",
                "inspect_moment",
                "find_moment",
                "read_index",
                "attempt_completion",
            ],
            max_iterations: 12,
            kind: SubAgentKind::Persona,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_ships_research_and_persona_subagents() {
        let r = SubAgentRegistry::defaults();
        // 3 research + 3 personas
        assert_eq!(r.len(), 6);
        // Research
        assert!(r.get("episode-explorer").is_some());
        assert!(r.get("cut-scout").is_some());
        assert!(r.get("b-roll-hunter").is_some());
        // Personas
        assert!(r.get("editor").is_some());
        assert!(r.get("fact-checker").is_some());
        assert!(r.get("brand-voice").is_some());
    }

    #[test]
    fn personas_have_persona_kind() {
        let r = SubAgentRegistry::defaults();
        for name in ["editor", "fact-checker", "brand-voice"] {
            assert_eq!(
                r.get(name).unwrap().kind,
                SubAgentKind::Persona,
                "{name} must be marked as Persona"
            );
        }
    }

    #[test]
    fn research_agents_have_research_kind() {
        let r = SubAgentRegistry::defaults();
        for name in ["episode-explorer", "cut-scout", "b-roll-hunter"] {
            assert_eq!(
                r.get(name).unwrap().kind,
                SubAgentKind::Research,
                "{name} must be marked as Research"
            );
        }
    }

    #[test]
    fn no_subagent_includes_mutating_tools() {
        let mutating = ["apply_edl", "start_render", "bash"];
        for a in SubAgentRegistry::defaults().iter() {
            for m in mutating {
                assert!(
                    !a.allowed_tools.contains(&m),
                    "sub-agent '{}' must not have access to '{}'",
                    a.name,
                    m
                );
            }
        }
    }

    #[test]
    fn every_subagent_has_attempt_completion() {
        for a in SubAgentRegistry::defaults().iter() {
            assert!(
                a.allowed_tools.contains(&"attempt_completion"),
                "sub-agent '{}' must have attempt_completion (its only return path)",
                a.name
            );
        }
    }

    #[test]
    fn unknown_name_returns_none() {
        let r = SubAgentRegistry::defaults();
        assert!(r.get("not-a-real-agent").is_none());
    }
}
