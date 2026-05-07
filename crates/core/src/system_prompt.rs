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
    Manual,
    Copilot,
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
        Err(_) => return ProjectFormat::Other { description: String::new() },
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => return ProjectFormat::Other { description: String::new() },
    };
    let raw = match value.pointer("/metadata/awidat/awidat_project_type") {
        Some(v) => v,
        None => return ProjectFormat::Other { description: String::new() },
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
        _ => ProjectFormat::Other { description: String::new() },
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
        ProjectFormat::Shorts => NEUTRAL_ADDENDUM,
        ProjectFormat::Tutorial => NEUTRAL_ADDENDUM,
        ProjectFormat::Other { .. } => NEUTRAL_ADDENDUM,
    }
}

/// Single line stamping the active permission mode. Short on
/// purpose — the agent doesn't need a paragraph explaining each
/// mode, just a clear declarative statement of which one's in
/// effect right now.
fn permission_line(mode: PromptPermissionMode) -> &'static str {
    match mode {
        PromptPermissionMode::Manual =>
            "**Permission mode: manual.** Every proposal needs explicit user approval. \
             Surface findings via Editorial Notes when asked; don't propose edits unprompted.",
        PromptPermissionMode::Copilot =>
            "**Permission mode: copilot.** Surface editorial findings as Notes proactively. \
             Don't issue apply_edl proposals unless the user explicitly asks (or clicks 'Fix' \
             on a Note); your role is to flag, the user's role is to act.",
        PromptPermissionMode::Autopilot =>
            "**Permission mode: autopilot.** When the user asks for cleanup, scan for findings \
             and bundle them into a single apply_edl envelope rather than emitting them \
             one-by-one. The user reviews the bundle in the ghost overlay and accepts or \
             rejects as a whole.",
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
\n- find_dead_air / find_filler_words / find_false_starts: editorial \
findings the user can review as Notes.\
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
\nMutating tools (apply_edl, start_render, start_indexing, bash) \
require user approval — you'll see the result come back as a \
tool_result, not a direct yes/no.\
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

/// Neutral addendum for shorts / tutorial / other. Doesn't claim
/// format-specific knowledge the agent doesn't have yet — Phase 4
/// fills these in with shorts-specific and tutorial-specific
/// prompts. Until then, the agent reasons from the user's project
/// description (when supplied) plus the base prompt.
const NEUTRAL_ADDENDUM: &str = "\
**Format: generic.** No format-specific defaults are loaded — use the \
base editorial tools (find_dead_air / find_filler_words / \
find_false_starts) when the user asks for cleanup, but treat any \
conventions (cadence, b-roll density, transition style) as \
project-specific rather than format-specific. Ask the user about \
their preferences before proposing aggressive cuts.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_podcast_with_manual_mode() {
        let prompt = assemble_system_prompt(&ProjectFormat::Podcast, PromptPermissionMode::Manual);
        assert!(prompt.contains("Discover before acting"));
        assert!(prompt.contains("long-form podcast cleanup"));
        assert!(prompt.contains("breath beats"));
        assert!(prompt.contains("Permission mode: manual"));
    }

    #[test]
    fn assembles_neutral_for_shorts_and_tutorial() {
        let shorts = assemble_system_prompt(&ProjectFormat::Shorts, PromptPermissionMode::Copilot);
        let tutorial =
            assemble_system_prompt(&ProjectFormat::Tutorial, PromptPermissionMode::Copilot);
        assert!(shorts.contains("**Format: generic.**"));
        assert!(tutorial.contains("**Format: generic.**"));
        // Both should NOT include the podcast-specific block.
        assert!(!shorts.contains("breath beats"));
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
    fn permission_line_varies_per_mode() {
        let manual =
            assemble_system_prompt(&ProjectFormat::Podcast, PromptPermissionMode::Manual);
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
