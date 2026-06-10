//! Contextual fragments — typed, marker-tagged content we inject into
//! per-turn message history.
//!
//! Originally lifted from `harnesses/codex/codex-rs/core/src/context/fragment.rs`.
//! Now that the legacy in-process agent loop is gone (step 8e/W), the
//! consumer is the codex subprocess — we just need to render the fragment
//! to a marker-tagged string that codex's prompt assembly can splice in.
//! No `anthropic::Message` shape is required at this layer.
//!
//! Why fragments instead of system-prompt mutation:
//!
//!   - System prompt lives in tier-1 cache. Mutating it on every turn
//!     blows the cache. Fragments live per-turn — cheap.
//!   - Skill catalogs can change without mutating the static system
//!     prompt. System-prompt-only loading would force prompt rebuilds.
//!   - Compaction needs to identify-and-strip injected context to
//!     produce a clean handoff summary. Markers are how it tells.

/// Trait for types that materialize as a marked text fragment in the
/// per-turn history. Implementations own the markers. `body()` is the
/// raw payload without markers. `render()` concatenates them.
pub trait ContextualUserFragment {
    /// Opening tag, e.g. `<skills_instructions>`. Used by both
    /// rendering and the matches-text check (so compaction can find
    /// fragments later).
    const START_MARKER: &'static str;
    /// Closing tag, e.g. `</skills_instructions>`.
    const END_MARKER: &'static str;

    /// The fragment's payload, without markers.
    fn body(&self) -> String;

    /// True iff `text` looks like a previously-rendered fragment of
    /// this type (starts with START_MARKER and ends with END_MARKER,
    /// after trim). Lets compaction recognize and strip injected
    /// context. Empty markers always return false.
    fn matches_text(text: &str) -> bool
    where
        Self: Sized,
    {
        if Self::START_MARKER.is_empty() || Self::END_MARKER.is_empty() {
            return false;
        }
        let trimmed = text.trim();
        trimmed.starts_with(Self::START_MARKER) && trimmed.ends_with(Self::END_MARKER)
    }

    /// Render to the wire form: `START_MARKER + body + END_MARKER`.
    /// No automatic separators; the body owns its own whitespace.
    fn render(&self) -> String {
        if Self::START_MARKER.is_empty() && Self::END_MARKER.is_empty() {
            return self.body();
        }
        format!("{}{}{}", Self::START_MARKER, self.body(), Self::END_MARKER)
    }
}

/// L1 catalog fragment — the list of available skills with one-line
/// descriptions. Rendered as a marker-tagged block the codex subprocess
/// can splice into the per-turn context (not the static system prompt).
///
/// Format (matches codex's shape):
///
/// ```text
/// <skills_instructions>
/// ## Skills
/// A skill is a set of local instructions ...
/// ### Available skills
///   - <name>: <description>
///   - ...
/// ### How to use skills
///   - Trigger rules: ...
///   - Progressive disclosure: ...
/// </skills_instructions>
/// ```
pub struct AvailableSkillsFragment {
    /// One line per skill, format `- <name>: <description>`.
    pub skill_lines: Vec<String>,
}

impl ContextualUserFragment for AvailableSkillsFragment {
    const START_MARKER: &'static str = "<skills_instructions>";
    const END_MARKER: &'static str = "</skills_instructions>";

    fn body(&self) -> String {
        if self.skill_lines.is_empty() {
            // Empty bodies still render with the markers so compaction
            // recognizes them. But the catalog should never be emitted
            // with zero skills — caller's responsibility.
            return "\n## Skills\n\nNo skills available in this session.\n".to_string();
        }
        let mut s = String::from("\n## Skills\n");
        s.push_str(SKILLS_INTRO);
        s.push_str("\n### Available skills\n");
        for line in &self.skill_lines {
            s.push_str(line);
            s.push('\n');
        }
        s.push_str("### How to use skills\n");
        s.push_str(SKILLS_HOW_TO_USE);
        s.push('\n');
        // Rationale contract — always-on editorial rule that lands on
        // every turn alongside the skills catalog. Lives here (in the
        // L1 fragment) rather than per-skill body so the agent absorbs
        // it whether or not it calls `load_skill`. Wave 4 task W4.2.
        s.push_str(RATIONALE_CONTRACT);
        s.push('\n');
        s
    }
}

/// Intro paragraph above the skill list. Lifted from codex's
/// SKILLS_INTRO_WITH_ABSOLUTE_PATHS but tightened for our scope.
const SKILLS_INTRO: &str = "\nA skill is a named editorial workflow with a full playbook \
in a SKILL.md file. Each entry below shows the skill name and a \
one-line description. Use `load_skill(name=...)` to fetch the full \
playbook when a skill matches the user's request.\n";

/// Trigger + use rules block. Same purpose as codex's
/// `SKILLS_HOW_TO_USE_WITH_ABSOLUTE_PATHS` — the prose tells the
/// model when to use a skill and how to follow it. Tighter than
/// codex's because montage skills are smaller in scope (editorial
/// workflows, not arbitrary code-gen recipes).
const SKILLS_HOW_TO_USE: &str = "  - Trigger: if the user's request matches a skill's description \
(e.g. \"tighten this interview\" → interview-tightener), call \
`load_skill(name=...)` BEFORE doing the work. Following the playbook \
produces better cuts than improvising.\n  - Progressive disclosure: \
the L1 catalog above is everything you see by default. The full L2 \
body comes back from `load_skill`. L3 — bundled scripts — runs via \
the `bash` tool against paths the L2 body references.\n  - Multiple \
skills: if the request spans domains (tighten THEN suggest b-roll), \
load both, in order, and announce which you're using. For production \
requests, load the generic producer skill first, then any brand/private \
skill that matches the project or show; brand/private skills compose \
with generic producers, not replace them.\n  - Workflow gates: if the \
request asks you to produce, prepare, finish, render, package, make \
shorts, or otherwise complete an output and a matching producer/director \
skill exists, load it before planning or editing. If a brand/private \
skill is missing, continue with the matching generic producer and say \
which brand layer was unavailable.\n  - Skip gracefully: if no skill \
applies, just do the work with the regular tools. Skills are optional \
only when no matching workflow exists.\n  - Edit graph first: \
skills may use scripts to score/analyze, but all editorial changes \
must become graph edits through `apply_edl`, then be inspected with \
`view_timeline`/`vedit_diff` and rendered with `start_render(scope=\"timeline\")`. \
Do not use `bash`, FFmpeg, or sidecar scripts as an alternate editor.";

/// Always-on rationale contract. Lives in the L1 catalog body so the
/// agent sees it on every turn — even when no skill is loaded — and
/// so it appears exactly once across the entire context (not once per
/// SKILL.md body, which would drift). Wave 4 task W4.2.
///
/// The contract teaches the agent to fill the `rationale` field on
/// every proposal it emits via `apply_edl`. Empty rationale is a
/// review failure; the Brief, History, Inspector, and timeline ghost
/// overlay all surface this string to the user as the trust signal
/// that justifies accepting the edit.
const RATIONALE_CONTRACT: &str = "\n## Rationale contract\n\n\
Every proposal you emit MUST include a one-sentence rationale. Pass it \
on `apply_edl` as `reasoning: \"<one short sentence>\"`; the desktop \
surfaces it as the `rationale` on the resulting proposal pill, Brief \
row, History entry, and timeline ghost overlay.\n\n\
The sentence answers \"why\" in plain editorial terms:\n\n\
  - Reference the threshold or principle from AGENTS.md, the indexer \
signal, or the skill that triggered the suggestion.\n  - Keep it under \
~120 characters when possible. The user reads it on a single line in \
the Brief and on timeline ghost-clips.\n  - Don't restate the action. \
\"Trimmed 0.42s of silence\" is the title; the rationale is \
\"Silence > 300ms exceeded the podcast-cleanup threshold from AGENTS.md\".\n  \
- For B-roll insertions, the rationale must name the prompt and provider \
in compliance with disclosure requirements: \"Generated by replicate/sd-3 \
from prompt 'sunset over coastline' — disclosure auto-added.\"\n  - For \
color/audio/transition proposals, name the audited measurement: \
\"Loudness spike at -4 LUFS exceeded broadcast safe of -16 LUFS.\"\n  - \
Do not leave `rationale` null. A missing rationale fails the editorial \
review.\n";

/// L1 fragment — the user's most recent proposal rejections, surfaced
/// to the agent so the next turn doesn't repeat patterns the user just
/// turned down. Mirrors `AvailableSkillsFragment`'s shape: marker-tagged
/// block the bridge prepends to every `start_turn` input.
///
/// Wave 5 task C3. The desktop builds this from
/// `<project>/.montage/feedback.jsonl` (the rejection log Wave 5 C2 ships)
/// at session-launch time, capped at [`MAX_FEEDBACK_ENTRIES`] entries
/// newest-first.
///
/// Format:
///
/// ```text
/// <recent_feedback>
/// ## Recent feedback from the user
///
/// The user has rejected the following proposals recently. Avoid
/// repeating these patterns:
///
///   - cut "Trim 0:12 — 0:12.42" — reason: Too aggressive
///   - broll "Insert B-roll at 0:14" — reason: Off-topic
///   - color (no reason given)
///
/// Read these as constraints, not absolutes. The user may want similar
/// proposals in different contexts — use the reasons to refine WHY you
/// propose, not as a blanket ban.
/// </recent_feedback>
/// ```
///
/// Empty `lines` → caller skips rendering entirely (see
/// `codex_session::render_recent_feedback`); we never emit an empty
/// section, just no section at all.
pub struct RecentFeedbackFragment {
    /// One line per rejection, format matches the `format_feedback_line`
    /// helper in the desktop crate. Newest-first.
    pub lines: Vec<String>,
}

/// Hard cap on rejection rows surfaced in the per-turn L1 fragment.
/// Keep prompt budget honest: 15 short bullets at ~100 chars apiece is
/// ~1.5 KB of prompt, well under the per-turn fragment budget. The full
/// 200-entry log on disk stays available for History; this only governs
/// what the agent sees on the next turn.
pub const MAX_FEEDBACK_ENTRIES: usize = 15;

impl ContextualUserFragment for RecentFeedbackFragment {
    const START_MARKER: &'static str = "<recent_feedback>";
    const END_MARKER: &'static str = "</recent_feedback>";

    fn body(&self) -> String {
        // Empty bodies should not reach `body()` — the caller filters
        // them out so the agent's prompt doesn't carry an empty section.
        // If one slips through anyway, emit nothing inside the markers
        // (cheap; matches AvailableSkillsFragment's "marker survival"
        // pattern so compaction can still find + strip the block).
        if self.lines.is_empty() {
            return "\n## Recent feedback from the user\n\nNo recent rejections.\n".to_string();
        }
        let mut s = String::from("\n## Recent feedback from the user\n");
        s.push_str(FEEDBACK_INTRO);
        for line in &self.lines {
            s.push_str(line);
            s.push('\n');
        }
        s.push_str(FEEDBACK_OUTRO);
        s
    }
}

/// Intro paragraph above the rejection bullet list. Explicitly behavioural
/// — "avoid repeating these patterns" — so the agent treats the fragment
/// as a constraint catalog, not a notification feed.
const FEEDBACK_INTRO: &str = "\nThe user has rejected the following proposals recently. \
Avoid repeating these patterns:\n\n";

/// Outro paragraph below the rejection bullet list. Softens "avoid" so
/// the agent doesn't read it as an absolute ban — the user may want a
/// similar proposal in a different context. The "reasons" tell the
/// agent WHY a previous attempt missed, not that the entire medium is
/// off-limits.
const FEEDBACK_OUTRO: &str = "\nRead these as constraints, not absolutes. The user may want \
similar proposals in different contexts — use the reasons to refine WHY you propose, \
not as a blanket ban.\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_lines_render_with_markers() {
        let frag = AvailableSkillsFragment {
            skill_lines: vec![
                "  - interview-tightener: tighten an interview by 20-30%".to_string(),
                "  - b-roll-suggester: visual cutaway hunting".to_string(),
            ],
        };
        let rendered = frag.render();
        assert!(rendered.starts_with("<skills_instructions>"));
        assert!(rendered.ends_with("</skills_instructions>"));
        assert!(rendered.contains("interview-tightener"));
        assert!(rendered.contains("b-roll-suggester"));
        assert!(rendered.contains("How to use skills"));
        assert!(rendered.contains("Edit graph first"));
        assert!(rendered.contains("Do not use `bash`, FFmpeg"));
    }

    /// Wave 4 W4.2 — the rationale contract is part of the always-on
    /// L1 fragment so the agent absorbs it on every turn. The contract
    /// lives exactly once in the rendered catalog (not per SKILL.md
    /// body) to avoid drift.
    #[test]
    fn rationale_contract_lands_in_l1_fragment() {
        let frag = AvailableSkillsFragment {
            skill_lines: vec!["  - cut-director: orchestrate cleanup".to_string()],
        };
        let rendered = frag.render();
        // Heading is the agent-facing signpost — the assertion the
        // task description asks for ("does the rendered L1 catalog
        // string contain the phrase 'Rationale contract'?").
        assert!(
            rendered.contains("## Rationale contract"),
            "L1 fragment must include the Rationale contract heading; got:\n{rendered}"
        );
        // Key behavioural rules the contract enforces.
        assert!(rendered.contains("Every proposal you emit MUST include"));
        assert!(rendered.contains("reasoning: \"<one short sentence>\""));
        assert!(rendered.contains("A missing rationale fails the editorial review"));
        // The contract appears exactly once even when many skills are
        // listed — no per-skill body duplication.
        let occurrences = rendered.matches("## Rationale contract").count();
        assert_eq!(
            occurrences, 1,
            "rationale contract must render exactly once"
        );
    }

    #[test]
    fn matches_text_recognizes_rendered_form() {
        let frag = AvailableSkillsFragment {
            skill_lines: vec!["  - test: x".to_string()],
        };
        let rendered = frag.render();
        assert!(AvailableSkillsFragment::matches_text(&rendered));
        // Negative: arbitrary text doesn't match.
        assert!(!AvailableSkillsFragment::matches_text(
            "just a normal message"
        ));
        assert!(!AvailableSkillsFragment::matches_text(
            "<skill>different marker</skill>"
        ));
    }

    /// Wave 5 C3 — `RecentFeedbackFragment` renders the expected shape:
    /// marker-tagged block, behavioural heading + intro, one bullet per
    /// line, and the "constraints, not absolutes" outro that keeps the
    /// agent from reading the list as a blanket ban.
    #[test]
    fn recent_feedback_fragment_renders_with_markers_and_lines() {
        let frag = RecentFeedbackFragment {
            lines: vec![
                "  - cut \"Trim 0:12 — 0:12.42\" — reason: Too aggressive".to_string(),
                "  - broll \"Insert B-roll at 0:14\" — reason: Off-topic".to_string(),
                "  - color (no reason given)".to_string(),
            ],
        };
        let rendered = frag.render();
        assert!(rendered.starts_with("<recent_feedback>"), "{rendered}");
        assert!(rendered.ends_with("</recent_feedback>"), "{rendered}");
        assert!(rendered.contains("## Recent feedback from the user"));
        // Intro: behavioural "avoid repeating" framing, not "FYI".
        assert!(
            rendered.contains("Avoid repeating these patterns"),
            "intro must be behavioural: {rendered}"
        );
        // All three rows surfaced.
        assert!(rendered.contains("Trim 0:12 — 0:12.42"));
        assert!(rendered.contains("Insert B-roll at 0:14"));
        assert!(rendered.contains("(no reason given)"));
        // Outro: softens "avoid" so the agent doesn't read it as a ban.
        assert!(
            rendered.contains("constraints, not absolutes"),
            "outro must soften: {rendered}"
        );
    }

    /// Empty `lines` should not occur in production (the caller filters
    /// before constructing the fragment), but if one slips through we
    /// keep the markers so compaction can still find + strip the block.
    /// The body itself just says "no recent rejections" rather than the
    /// behavioural intro — there's nothing to constrain.
    #[test]
    fn recent_feedback_fragment_with_empty_lines_renders_marker_only() {
        let frag = RecentFeedbackFragment { lines: vec![] };
        let rendered = frag.render();
        assert!(rendered.starts_with("<recent_feedback>"));
        assert!(rendered.ends_with("</recent_feedback>"));
        assert!(rendered.contains("No recent rejections"));
        // Empty fragment must NOT emit the behavioural intro — there's
        // nothing to avoid repeating.
        assert!(
            !rendered.contains("Avoid repeating these patterns"),
            "empty fragment leaked the behavioural intro: {rendered}"
        );
    }

    #[test]
    fn recent_feedback_fragment_matches_text() {
        let frag = RecentFeedbackFragment {
            lines: vec!["  - cut x".to_string()],
        };
        let rendered = frag.render();
        assert!(RecentFeedbackFragment::matches_text(&rendered));
        // Negative — skills-fragment-tagged text must NOT match.
        let skills_frag = AvailableSkillsFragment {
            skill_lines: vec!["  - test: x".to_string()],
        };
        let skills_rendered = skills_frag.render();
        assert!(!RecentFeedbackFragment::matches_text(&skills_rendered));
    }

    /// The 15-entry cap is part of the public surface — the desktop
    /// honours it on the read side, so the constant must stay stable
    /// across releases (or the prompt budget assumption breaks).
    #[test]
    fn max_feedback_entries_is_fifteen() {
        assert_eq!(MAX_FEEDBACK_ENTRIES, 15);
    }
}
