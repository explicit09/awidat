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

fn default_subagents() -> Vec<SubAgent> {
    vec![
        SubAgent {
            name: "episode-explorer",
            description: "Read-only sub-agent that produces a structured \
                          overview of an episode: speakers, topics, \
                          high-energy beats, indexed channels. Use when \
                          the parent needs a quick map before picking a \
                          direction. Returns a one-screen summary.",
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
        },
        SubAgent {
            name: "cut-scout",
            description: "Read-only sub-agent that scouts the strongest \
                          editorial moments by kind (hook, punchline, \
                          cta, emotional_peak). Returns a ranked list \
                          with timestamps, scores, dependencies, and a \
                          recommended cut order.",
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
        },
        SubAgent {
            name: "b-roll-hunter",
            description: "Read-only sub-agent that hunts visual cutaways \
                          for a spoken passage. Cross-references shot \
                          type, frame quality, and CLIP semantic match. \
                          Returns 3-5 candidates with scores and reasons.",
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
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_ships_three_subagents() {
        let r = SubAgentRegistry::defaults();
        assert_eq!(r.len(), 3);
        assert!(r.get("episode-explorer").is_some());
        assert!(r.get("cut-scout").is_some());
        assert!(r.get("b-roll-hunter").is_some());
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
