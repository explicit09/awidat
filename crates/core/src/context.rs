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
/// codex's because awidat skills are smaller in scope (editorial
/// workflows, not arbitrary code-gen recipes).
const SKILLS_HOW_TO_USE: &str = "  - Trigger: if the user's request matches a skill's description \
(e.g. \"tighten this interview\" → interview-tightener), call \
`load_skill(name=...)` BEFORE doing the work. Following the playbook \
produces better cuts than improvising.\n  - Progressive disclosure: \
the L1 catalog above is everything you see by default. The full L2 \
body comes back from `load_skill`. L3 — bundled scripts — runs via \
the `bash` tool against paths the L2 body references.\n  - Multiple \
skills: if the request spans domains (tighten THEN suggest b-roll), \
load both, in order, and announce which you're using.\n  - Skip \
gracefully: if no skill applies, just do the work with the regular \
tools. Skills are a shortcut, not a gate.\n  - Edit graph first: \
skills may use scripts to score/analyze, but all editorial changes \
must become graph edits through `apply_edl`, then be inspected with \
`view_timeline`/`vedit_diff` and rendered with `start_render(scope=\"timeline\")`. \
Do not use `bash`, FFmpeg, or sidecar scripts as an alternate editor.";

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
}
