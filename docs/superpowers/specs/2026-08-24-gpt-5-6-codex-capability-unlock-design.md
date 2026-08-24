# GPT-5.6 Codex Capability Unlock Design

**Date:** 2026-08-24

## Goal

Let Montage benefit from GPT-5.6 Codex's visual reasoning, long-horizon
context, model tiers, and focused tool orchestration without weakening the
proposal, validation, approval, or render-verification boundaries that protect
the edit graph.

## Scope

This change has five deliverables:

1. Send extracted source and program frames to Codex as image content rather
   than base64 embedded in text.
2. Add explicit Balanced and Deep Edit agent profiles.
3. Replace the global direct-exposure workaround with a tested, configurable
   tool-exposure strategy and a focused default surface.
4. Make compaction thresholds GPT-5.6-aware while staying below the current
   long-context price step by default.
5. Let the agent perform reversible analysis and proposal work autonomously
   while preserving approval for edit acceptance, export, and publication.

Native video input is out of scope. GPT-5.6 accepts image input, so Montage
continues to decompose video into transcript, audio/vision indexes, selected
frames, and rendered review frames.

## Capability Profiles

Montage exposes two profiles in the desktop chat surface and persists the
selection per project.

| Profile | Model | Effort | Intended work |
| --- | --- | --- | --- |
| Balanced | `gpt-5.6-terra` | `medium` | routine cleanup, mechanical execution, normal chat |
| Deep Edit | `gpt-5.6-sol` | `high` | rough cuts, story judgment, montage, B-roll, transitions, visual QA |

Balanced remains the default for existing and new projects. Max and ultra are
not automatic profile settings. They may be offered as explicit one-turn
actions for unusually difficult work because ultra can coordinate multiple
agents and materially changes cost and execution behavior.

Changing profile updates subsequent turns in the current Codex thread using
the app-server's model and effort overrides. Resumed threads retain their
existing settings until the user changes the profile.

## Visual Evidence Transport

`view_frame` and `view_program_frame` return an MCP `CallToolResult` containing:

- a short text block with asset, source/timeline time, dimensions, and cache
  provenance;
- one MCP image-content block with the JPEG or PNG bytes and MIME type.

The tools must not place base64 inside a text JSON field. Preview detail remains
bounded to a 768-pixel longest edge; original detail remains explicit.

The first implementation supports one image per tool call. Contact-sheet
selection is a separate read-only tool that composes a bounded grid from
ranked frame candidates and returns it as image content. The model chooses
individual original-detail frames only when the grid reveals a decision that
needs closer inspection.

Every visually sensitive proposal must be reviewable against a program frame
or render-derived contact sheet. Image transport never grants edit authority.

## Tool Exposure

The current bridge forces `supports_search_tool=false`, exposing all 133
Montage tools directly. Removing that override without evidence is unsafe
because an earlier Codex build failed to search and edited OTIO directly.

The replacement is staged:

1. Introduce `direct` and `native-search` exposure modes behind a typed bridge
   setting. Existing projects initially resolve to `direct`.
2. Add an evaluation fixture that proves an organic edit request discovers
   `view_episode`, the appropriate skill, and `apply_edl` without direct OTIO
   mutation.
3. Make `native-search` the default only after that fixture passes with
   GPT-5.6 Terra and Sol.
4. In direct mode, expose a small bootstrap surface plus the active skill's
   allowlist instead of all tools. Always-visible tools cover discovery,
   project instructions, skill loading, planning, user input, completion, and
   edit proposals.

Skill allowlists control both advertised and executable tools. Execution-time
checking remains the final enforcement layer. Direct filesystem mutation of
`project.otio.json` is never an allowed substitute for `apply_edl`.

## Context and Compaction

The fixed 200,000-token threshold is replaced with a model-aware policy:

- GPT-5.6 profiles compact at 250,000 tokens by default.
- Explicit max/ultra work may choose a higher threshold only when the user
  accepts the long-context cost implication.
- Unknown and legacy models retain the 200,000-token fallback.

The 250,000 default uses more of GPT-5.6's working context while remaining
below the current 272,000-token API price step. The custom video-production
checkpoint remains, but its summary records image evidence references,
profile, active skills, proposal state, and remaining verification.

## Permission Boundary

The product distinguishes reversible agent work from consequential actions.

### Autonomous in every mode

- inspect indexes, frames, timelines, and render results;
- load or switch skills;
- create Editorial Notes;
- construct and revise ghost-overlay proposals;
- render local previews for verification.

### Approval required

- accept a proposal into the edit graph in manual or copilot mode;
- overwrite or export user-selected deliverables;
- publish or upload media;
- install paid/generated media or incur a disclosed external charge.

Autopilot may bundle requested cleanup into one proposal, but it does not
silently publish. Existing validation, timestamp mapping, continuity checks,
`vedit_diff`, cancellation, and render verification remain mandatory.

## Skill Guidance

Skills retain domain knowledge and hard editorial invariants, but procedural
micromanagement is reduced:

- state goals, evidence requirements, hard constraints, approval boundaries,
  and completion criteria;
- avoid prescribing every tool call when Codex can choose the sequence;
- allow related skills to compose through phase-scoped allowlists;
- request clarification only for materially different creative meanings or
  consequential actions, not for reversible proposal generation.

The thematic-montage distinction remains, but Codex may surface a labeled
thematic option without asking first. Applying that interpretation still
requires normal proposal review.

## Evaluation and Rollout

A checked-in GPT-5.6 Codex evaluation matrix covers podcast cleanup, rough-cut
assembly, retakes, B-roll, thematic montage, multicam, pacing, transitions,
reframing, color review, short-form extraction, and final visual QA.

Each case records:

- required tool and skill discovery;
- editorial acceptance or correction;
- invalid edit attempts and direct-OTIO mutations;
- tool calls, approval turns, tokens, and elapsed time;
- render or program-frame verification.

Rollout gates:

1. Image-content transport passes focused bridge and MCP tests.
2. Balanced and Deep profiles persist and update model plus effort correctly.
3. Direct mode advertises only bootstrap plus active-skill tools.
4. Native search becomes default only after the organic-prompt safety fixture
   passes on both Terra and Sol.
5. Packaged desktop proof confirms profile selection, visible frame review,
   proposal approval, save, quit/reopen, and on-disk persistence.

## Non-Goals

- Native ingestion of whole video files by GPT-5.6.
- Automatic ultra or multi-agent execution.
- Removing transactional proposals or validation.
- Rewriting all editorial skills in one change.
- Changing rendering, indexing, or publishing providers unrelated to the
  Codex capability boundary.
