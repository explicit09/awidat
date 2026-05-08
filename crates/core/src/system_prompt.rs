//! Per-format system-prompt assembly (Phase 1.9).
//!
//! The agent's system prompt is no longer a single static const —
//! it composes from:
//!
//!   - **Base prompt**: every awidat session shares this. Discovery
//!     rules, tool overview, time semantics. Stable across formats.
//!
//!   - **Per-format addendum**: podcast / shorts / tutorial each get
//!     their own block of editorial defaults — silence thresholds,
//!     pacing rules, b-roll aggressiveness. `Other` falls back to a
//!     neutral baseline.
//!
//!   - **Project description**: only present when project type is
//!     `Other { description }`. Appended verbatim so the agent has
//!     *something* concrete to anchor its decisions on for projects
//!     the enum doesn't capture (compilation videos, mixed footage).
//!
//!   - **Permission-mode line**: a single sentence tells the agent
//!     how aggressive to be (manual / copilot / autopilot). Read at
//!     session start; users changing the mode mid-session get the
//!     new behavior on the next turn.
//!
//! Why this lives in core (not desktop-tauri): the CLI's `tui_cmd`
//! and `chat_cmd` paths build sessions too. Putting the assembly in
//! a shared crate means all three paths get the same per-format
//! tailoring without copy-paste drift.

use std::path::Path;

/// Project type, mirroring the protocol's variants but kept here in
/// core so this crate doesn't depend on `awidat-desktop-protocol`.
/// The CLI / desktop translate at the boundary.
#[derive(Debug, Clone)]
pub enum ProjectFormat {
    /// Long-form podcast — Phase 1's specialized mode.
    Podcast,
    /// Short-form vertical (60s, hook in first 3s, fast cuts).
    /// Phase 4 fills in the specialized prompt; v1 is neutral.
    Shorts,
    /// Tutorial / demo / screencast. Phase 4 specializes; v1 neutral.
    Tutorial,
    /// Anything else. The free-text description is appended to the
    /// prompt verbatim so the agent has user-supplied context.
    Other {
        /// User's project description (free text, may be empty).
        description: String,
    },
}

/// Permission mode, mirroring the protocol enum.
#[derive(Debug, Clone, Copy)]
pub enum PromptPermissionMode {
    /// Manual approval mode.
    Manual,
    /// Copilot approval mode.
    Copilot,
    /// Autopilot approval mode.
    Autopilot,
}

/// Assemble the final system prompt for a session.
///
/// The result is a single `String` ready to pass into `Session::new`.
/// Sections are joined with double newlines so the model perceives
/// them as related but distinct, matching how `claude-code` and
/// `codex` shape their own prompts.
pub fn assemble_system_prompt(
    format: &ProjectFormat,
    permission_mode: PromptPermissionMode,
) -> String {
    let mut out = String::new();
    out.push_str(BASE_PROMPT);
    out.push_str("\n\n");
    out.push_str(format_addendum(format));
    if let ProjectFormat::Other { description } = format
        && !description.trim().is_empty()
    {
        out.push_str("\n\n**Project description (user-supplied):** ");
        out.push_str(description.trim());
    }
    out.push_str("\n\n");
    out.push_str(permission_line(permission_mode));
    out
}

/// Convenience: load `<project>/.awidat/permission_mode` and the
/// project's OTIO metadata, build the prompt. Returns the base
/// prompt with neutral defaults when reads fail — never panics.
pub fn assemble_for_project(project_root: &Path) -> String {
    let format = read_project_format(project_root);
    let permission = read_permission_mode(project_root);
    assemble_system_prompt(&format, permission)
}

/// Read the project type from
/// `<project>/project.otio.json`'s `metadata.awidat.awidat_project_type`
/// slot (set by `init_project` in 1.3). Falls back to
/// `Other { description: "" }` on any read / parse failure.
fn read_project_format(project_root: &Path) -> ProjectFormat {
    let otio_path = project_root.join("project.otio.json");
    let bytes = match std::fs::read(&otio_path) {
        Ok(b) => b,
        Err(_) => {
            return ProjectFormat::Other {
                description: String::new(),
            };
        }
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            return ProjectFormat::Other {
                description: String::new(),
            };
        }
    };
    let raw = match value.pointer("/metadata/awidat/awidat_project_type") {
        Some(v) => v,
        None => {
            return ProjectFormat::Other {
                description: String::new(),
            };
        }
    };
    match raw.get("kind").and_then(|v| v.as_str()) {
        Some("podcast") => ProjectFormat::Podcast,
        Some("shorts") => ProjectFormat::Shorts,
        Some("tutorial") => ProjectFormat::Tutorial,
        Some("other") => ProjectFormat::Other {
            description: raw
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        },
        _ => ProjectFormat::Other {
            description: String::new(),
        },
    }
}

/// Read `<project>/.awidat/permission_mode`. Falls back to Manual
/// on any failure — same fallback as the desktop's
/// `commands::permission::read_mode`. Duplicated here so core
/// doesn't depend on the desktop crate.
fn read_permission_mode(project_root: &Path) -> PromptPermissionMode {
    let path = project_root.join(".awidat").join("permission_mode");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return PromptPermissionMode::Manual,
    };
    match text.trim() {
        "copilot" => PromptPermissionMode::Copilot,
        "autopilot" => PromptPermissionMode::Autopilot,
        _ => PromptPermissionMode::Manual,
    }
}

/// Per-format prompt addendum. Podcast gets concrete defaults;
/// shorts / tutorial / other get a neutral baseline that doesn't
/// claim format-specific knowledge the agent doesn't have yet.
fn format_addendum(format: &ProjectFormat) -> &'static str {
    match format {
        ProjectFormat::Podcast => PODCAST_ADDENDUM,
        // Phase 4C/4E refactor: format-specific playbooks live in
        // skills (`short-form`, `tutorial`) so the ~600-token bodies
        // only load when the agent calls `load_skill(...)` for that
        // format. Always-on prompt carries only the L1 stub via the
        // skills catalog injected per turn.
        ProjectFormat::Shorts => SHORTS_STUB,
        ProjectFormat::Tutorial => TUTORIAL_STUB,
        ProjectFormat::Other { .. } => NEUTRAL_ADDENDUM,
    }
}

/// Single line stamping the active permission mode. Short on
/// purpose — the agent doesn't need a paragraph explaining each
/// mode, just a clear declarative statement of which one's in
/// effect right now.
fn permission_line(mode: PromptPermissionMode) -> &'static str {
    match mode {
        PromptPermissionMode::Manual => {
            "**Permission mode: manual.** Every proposal needs explicit user approval. \
             Surface findings via Editorial Notes when asked; don't propose edits unprompted. \
             For dirty continuity verdicts, surface a continuity_warning Note quoting the \
             rule reasons rather than auto-bundling a transition — the user decides."
        }
        PromptPermissionMode::Copilot => {
            "**Permission mode: copilot.** Surface editorial findings as Notes proactively. \
             Don't issue apply_edl proposals unless the user explicitly asks (or clicks 'Fix' \
             on a Note); your role is to flag, the user's role is to act. For dirty continuity \
             verdicts, the Note's reasons array is the user's primary signal — they decide \
             whether to accept the bundled fix you suggested or cut anyway."
        }
        PromptPermissionMode::Autopilot => {
            "**Permission mode: autopilot.** When the user asks for cleanup, scan for findings \
             and bundle them into a single apply_edl envelope rather than emitting them \
             one-by-one. For dirty continuity verdicts, silently bundle a 0.3s SMPTE_Dissolve \
             via the bundle_with_dissolve pattern — don't ask, just include the transition \
             in the envelope. The user reviews the bundle in the ghost overlay and accepts \
             or rejects as a whole. For dirty cuts that read as visual jar (mid-motion, \
             speaker-switch mid-utterance), prefer bundling a `*** Insert BRoll` cover when \
             find_broll_opportunities surfaced a strong candidate at the same anchor — the \
             cutaway hides the jar without changing the audio at all."
        }
    }
}

/// The base prompt every session inherits. Lifted from the previous
/// hand-rolled `SYSTEM_PROMPT` constants in `apps/desktop/.../session.rs`
/// and the CLI tui/chat commands — same content, deduplicated here.
const BASE_PROMPT: &str = "\
You are awidat, a desktop agent for editing long-form spoken video. \
You operate inside a GUI: the user sees the chat, the timeline, and \
the video preview live. Be concise. Commit edits via apply_edl \
directly when you're confident.\
\n\n**Discover before acting.** Never guess asset paths or filenames. \
On the first turn of any session that touches assets, call \
view_episode (or list_assets) to learn the actual filenames. \
Asset paths in this project may be UUID-style (copy_F65206FA-…MOV), \
not human-readable like 'cast.mp4'. Guessing wastes tool calls and \
shows the user red error cards. The single discovery call is cheap \
and makes everything after it correct.\
\n\nKey tools:\
\n- view_episode: map of the project (assets + which indexers ran).\
\n- find_beat / find_moment / inspect_moment: editorial moment lookup.\
\n- find_episode_start: determine the publishable episode start; use \
this for podcast/interview top trims instead of guessing from the \
first transcript page.\
\n- find_dead_air / find_filler_words / find_false_starts: editorial \
findings the user can review as Notes.\
\n- assess_continuity(at_s, kind): BEFORE proposing any \
*** Trim Clip / *** Split Clip via apply_edl, call this with the \
proposed cut point. It returns `{ verdict, rules: [...] }` where \
verdict is `clean` / `risky` / `dirty` / `abstain`. Behavior:\
\n  • `clean`: propose the raw cut.\
\n  • `risky`: surface the rules array as a Note (kind: \
continuity_warning) describing the risk; let the user decide.\
\n  • `dirty`: do NOT propose the raw cut. You have THREE options, \
in order of preference: (a) bundle a 0.3s *** Insert Transition \
(SMPTE_Dissolve) at the cut point — works for most dirty cuts; \
(b) for visually-driven moments (mid-motion or speaker-switch \
mid-utterance), call `find_broll_opportunities` for the affected \
range and surface a `broll_suggestion` Note offering a b-roll \
cover instead — this is the right move when the cut would jar \
visually but the audio reads fine; (c) surface a continuity_warning \
Note quoting the rule reasons and let the user decide. Never \
silently emit a dirty cut.\
\n  • `abstain`: tell the user which sidecars are missing (the \
rules array shows `verdict: abstain` per missing input) and ask \
whether to proceed without the check.\
\n- apply_edl: cut/trim/delete/split/insert clips on the timeline. \
For `@@ anchor: clip_uuid=...`, use the clip anchor shown by \
view_timeline, usually the clip name like `clip-0`; never use the \
asset filename, proxy stem, or raw media basename as clip_uuid. \
Times are source-media seconds. view_timeline shows current \
`source=[start..end]`; to trim the first N seconds of the visible \
clip, set `start` to source start + N, and to trim the last N \
seconds, set `end` to source end - N.\
\n- start_render (scope='timeline'): render the edited timeline to mp4.\
\n- start_indexing: (re)run the configured indexers on raw/. Use when \
view_episode shows missing sidecars and the user asked for an \
operation that needs them. Imports auto-chain through indexing in \
the GUI's import flow, so this is the rare-case tool — don't \
proactively re-index already-indexed projects.\
\n\n**Edit graph is source of truth.** The agent must understand and \
mutate the OTIO timeline graph, not treat awidat as a chat wrapper \
around FFmpeg. Use scripts, indexers, and shell commands for analysis \
or verification only. Do not use bash/FFmpeg to cut, concatenate, \
caption, overlay, or produce the final edited artifact. Express \
editorial intent as EDL, apply it with `apply_edl`, inspect the \
resulting graph with `view_timeline`/`vedit_diff`, and export with \
`start_render(scope='timeline')`.\
\nMutating tools may be approval-gated depending on permission mode. \
Manual and Copilot are conservative; Autopilot lets routine \
editing/index/render tool calls proceed without approval cards. Bash \
can still be gated because it is arbitrary shell access. You'll see \
the result come back as a tool_result, not a direct yes/no.\
\n\nThe user's input may be prefixed with a metadata line like \
`[user is watching <stem> at MM:SS]`. That's the desktop's \
preview pane reporting where the user has the playhead. When \
the user says \"here\", \"this\", \"now\", or asks about the \
current moment, that timestamp is the answer to \"where.\" Use \
inspect_clip / view_frame / find_moment scoped to that time \
rather than guessing.";

/// Podcast-format defaults. Concrete numbers tuned for long-form
/// spoken-content cleanup — silence thresholds, breath-beat
/// preservation, conservative filler treatment, mid-sentence cut
/// avoidance.
const PODCAST_ADDENDUM: &str = "\
**Format: long-form podcast cleanup.** Editorial defaults:\
\n- Trim silences ≥ 2.0s but skip silences < 1.2s (breath beats — \
cutting them makes speech feel rushed).\
\n- Flag filler words ('um', 'uh') via find_filler_words but DON'T \
auto-trim every one — they're part of natural cadence. Surface \
the worst offenders (clusters, very long fillers) as Notes.\
\n- Never cut mid-sentence. The whisper transcript shows segment \
boundaries; cuts should land at sentence ends or natural pauses.\
\n- Preserve speaker rhythm: don't propose three or more cuts \
within 5 seconds of each other in the same vicinity.\
\n- B-roll suggestions on this format are reactive (covering an \
awkward cut) more than proactive — long-form podcasts don't need \
constant visual variety.\
\n\nWhen the user asks for 'a quick cleanup pass', the playbook is: \
find_dead_air → propose silence trims; offer to also scan filler \
words. Don't run all three editorial tools speculatively unless \
asked.";

/// Neutral addendum for `Other` projects. Shorts / tutorial used to
/// share this; Phase 4C / 4E split them out into dedicated
/// addenda below.
const NEUTRAL_ADDENDUM: &str = "\
**Format: generic.** No format-specific defaults are loaded — use the \
base editorial tools (find_dead_air / find_filler_words / \
find_false_starts) when the user asks for cleanup, but treat any \
conventions (cadence, b-roll density, transition style) as \
project-specific rather than format-specific. Ask the user about \
their preferences before proposing aggressive cuts.";

/// Stub line stamped into the always-on prompt when project type is
/// shorts. The full ~600-token playbook lives in
/// `skills/short-form/SKILL.md` — agent calls `load_skill('short-form')`
/// when the user asks for short-form work. Phase 4C-skills refactor.
const SHORTS_STUB: &str = "\
**Format: short-form vertical.** When the user asks for cleanup or \
short-form output, call `load_skill(name='short-form')` for the full \
editorial playbook (hook timing, cut cadence, caption pass, aspect \
ratio). The skill carries the concrete defaults so they don't burn \
context on sessions that aren't doing short-form work.";

/// Stub line for tutorial-format projects. The full playbook lives
/// in `skills/tutorial/SKILL.md`. Phase 4E-skills refactor.
const TUTORIAL_STUB: &str = "\
**Format: tutorial / screen recording.** When the user asks for \
cleanup or chaptering, call `load_skill(name='tutorial')` for the \
full editorial playbook (key-frame holds, no-cut-over-typing rule, \
chapter generation from topics). The skill carries the concrete \
defaults so they don't burn context on sessions that aren't doing \
tutorial work.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_podcast_with_manual_mode() {
        let prompt = assemble_system_prompt(&ProjectFormat::Podcast, PromptPermissionMode::Manual);
        assert!(prompt.contains("Discover before acting"));
        assert!(prompt.contains("Edit graph is source of truth"));
        assert!(prompt.contains("long-form podcast cleanup"));
        assert!(prompt.contains("breath beats"));
        assert!(prompt.contains("Permission mode: manual"));
    }

    #[test]
    fn shorts_format_emits_skill_stub() {
        // Phase 4C-skills refactor: the always-on prompt carries
        // only the stub pointing at `load_skill('short-form')`. The
        // full playbook lives in the SKILL.md and is loaded on
        // demand — keeps token cost low for sessions that aren't
        // doing short-form work.
        let shorts = assemble_system_prompt(&ProjectFormat::Shorts, PromptPermissionMode::Copilot);
        assert!(
            shorts.contains("**Format: short-form vertical.**"),
            "{shorts}"
        );
        assert!(shorts.contains("load_skill(name='short-form')"), "{shorts}");
        assert!(!shorts.contains("<skills_instructions>"));
        assert!(!shorts.contains("scripts/caption_plan.py"));
        // Concrete defaults must NOT appear in the stub — they live
        // in the SKILL.md body.
        assert!(!shorts.contains("hook lands in the first 3 seconds"));
        assert!(!shorts.contains("breath beats"));
    }

    #[test]
    fn tutorial_format_emits_skill_stub() {
        // Phase 4E-skills refactor: same shape as shorts.
        let tutorial =
            assemble_system_prompt(&ProjectFormat::Tutorial, PromptPermissionMode::Copilot);
        assert!(
            tutorial.contains("**Format: tutorial / screen recording.**"),
            "{tutorial}"
        );
        assert!(
            tutorial.contains("load_skill(name='tutorial')"),
            "{tutorial}"
        );
        assert!(!tutorial.contains("<skills_instructions>"));
        assert!(!tutorial.contains("scripts/"));
        // Concrete defaults must NOT appear in the stub.
        assert!(!tutorial.contains("Hold key frames longer"));
        assert!(!tutorial.contains("hook lands in the first 3 seconds"));
    }

    #[test]
    fn other_format_falls_back_to_neutral() {
        let other = assemble_system_prompt(
            &ProjectFormat::Other {
                description: String::new(),
            },
            PromptPermissionMode::Copilot,
        );
        assert!(other.contains("**Format: generic.**"));
    }

    #[test]
    fn other_with_description_appends_user_text_verbatim() {
        let prompt = assemble_system_prompt(
            &ProjectFormat::Other {
                description: "compilation of my best clips from 5 source videos".into(),
            },
            PromptPermissionMode::Manual,
        );
        assert!(prompt.contains("**Format: generic.**"));
        assert!(prompt.contains("Project description (user-supplied):"));
        assert!(prompt.contains("compilation of my best clips"));
    }

    #[test]
    fn other_with_empty_description_skips_the_section() {
        let prompt = assemble_system_prompt(
            &ProjectFormat::Other {
                description: "".into(),
            },
            PromptPermissionMode::Manual,
        );
        assert!(!prompt.contains("Project description (user-supplied):"));
    }

    #[test]
    fn other_with_whitespace_only_description_treats_as_empty() {
        let prompt = assemble_system_prompt(
            &ProjectFormat::Other {
                description: "   \n  ".into(),
            },
            PromptPermissionMode::Autopilot,
        );
        assert!(!prompt.contains("Project description (user-supplied):"));
    }

    #[test]
    fn base_prompt_documents_assess_continuity_workflow() {
        // The agent must know to call assess_continuity before
        // trims/splits, and must know what each verdict implies.
        // This test pins those instructions so future prompt
        // edits don't accidentally drop them.
        let prompt = assemble_system_prompt(&ProjectFormat::Podcast, PromptPermissionMode::Manual);
        assert!(prompt.contains("assess_continuity"));
        assert!(prompt.contains("dirty"));
        assert!(prompt.contains("Insert Transition"));
        assert!(prompt.contains("continuity_warning"));
    }

    #[test]
    fn permission_line_varies_per_mode() {
        let manual = assemble_system_prompt(&ProjectFormat::Podcast, PromptPermissionMode::Manual);
        let copilot =
            assemble_system_prompt(&ProjectFormat::Podcast, PromptPermissionMode::Copilot);
        let autopilot =
            assemble_system_prompt(&ProjectFormat::Podcast, PromptPermissionMode::Autopilot);
        assert!(manual.contains("manual"));
        assert!(copilot.contains("copilot"));
        assert!(autopilot.contains("autopilot"));
        // Each mode's line must be distinct.
        assert!(!manual.contains("autopilot"));
        assert!(!autopilot.contains("manual"));
    }

    #[test]
    fn assemble_for_project_falls_back_to_other_when_no_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = assemble_for_project(dir.path());
        // No otio file, no permission file → Other + Manual
        assert!(prompt.contains("**Format: generic.**"));
        assert!(prompt.contains("Permission mode: manual"));
    }

    #[test]
    fn assemble_for_project_reads_otio_and_permission() {
        let dir = tempfile::tempdir().unwrap();
        // Write a fake OTIO with the project_type slot.
        let otio = serde_json::json!({
            "OTIO_SCHEMA": "Timeline.1",
            "metadata": {
                "awidat": {
                    "version": "0.1",
                    "awidat_project_type": { "kind": "podcast" }
                }
            }
        });
        std::fs::write(
            dir.path().join("project.otio.json"),
            serde_json::to_vec_pretty(&otio).unwrap(),
        )
        .unwrap();
        // Write permission_mode.
        std::fs::create_dir_all(dir.path().join(".awidat")).unwrap();
        std::fs::write(dir.path().join(".awidat/permission_mode"), b"copilot").unwrap();

        let prompt = assemble_for_project(dir.path());
        assert!(prompt.contains("long-form podcast cleanup"));
        assert!(prompt.contains("Permission mode: copilot"));
    }
}
