# Category 11 Collaboration Roadmap Plan

> This plan coordinates the larger collaboration gap. Execute each phase
> as its own implementation plan after the previous phase has compiled,
> tested, and been reviewed. Do not stack unverified Rust behavior unless
> the blocker is explicitly accepted.

## Current Status

- [x] Isolated worktree created: `codex/collaboration-gap-finish`.
- [x] Category 11 markdown and HTML analysis re-read from
  `/Users/explicit/Projects/awidat/.reference-research/pro-editing-gap-analysis/`.
- [x] Phase 1 local vedit attribution implemented in code.
- [x] Phase 1 docs updated for current auto-commit behavior.
- [x] Remaining backlog decomposed in
  `docs/superpowers/specs/2026-05-21-collaboration-category-11-backlog.md`.
- [x] Phase 1 targeted Rust tests pass.
- [x] CLI/TUI registration compiles through `cargo check -p awidat-cli`.
- [x] Desktop Rust/frontend checks pass.
- [x] Branch/create/list/checkout implemented and verified.
- [x] Workspace check passes.
- [x] Open product decisions documented in
  `docs/superpowers/specs/2026-05-21-collaboration-open-decisions.md`.
- [x] Requirement-level audit documented in
  `docs/superpowers/specs/2026-05-21-collaboration-completion-audit.md`.
- [x] Remaining product blockers converted into an explicit approval
  packet:
  `docs/superpowers/specs/2026-05-21-collaboration-decision-packet.md`.
- [x] Changed-clip-id extraction primitive implemented and tested for
  future bounded merge planning.
- [x] Read-only `vedit_changed_clip_ids` tool implemented and
  registered for agent-side overlap inspection.
- [x] Desktop `changed_vedit_clip_ids` command implemented for local
  review UI preflight data.
- [x] Desktop vedit diff responses include changed clip ids for
  single-call review rows.
- [x] Read-only bounded merge preflight implemented for core, agent
  tool, and desktop command surfaces.
- [ ] Bounded merge product rule approved.
- [ ] Authored review-note storage decision approved.

## Phase 1: Finish Local Vedit Attribution

Plan:

- [x] Add named checkpoint refs through `vc::tag_ref` and `vc::list_tags`.
- [x] Add `vc::show_commit` for commit metadata plus parent diff.
- [x] Add `vc::blame_clip` for first-parent clip attribution.
- [x] Expose `vedit_tag`, `vedit_show`, and `vedit_blame` tools.
- [x] Register tools in CLI, TUI, and desktop agent sessions.
- [x] Expose matching desktop Tauri commands.
- [x] Include animation changes in desktop diff responses.
- [x] Update `skills/version-control/SKILL.md`.
- [x] Run `cargo fmt --all -- --check`.
- [x] Run `git diff --check`.
- [x] Run targeted `awidat-core` tests.
- [x] Run `cargo check -p awidat-cli`.
- [x] Run `cargo check -p awidat-desktop`.
- [x] Run `pnpm exec tsc --noEmit` in `apps/desktop`.
- [x] Run `pnpm test` in `apps/desktop`.
- [x] Run `make check` after disk space is available.

Blocker:

- Cleared. Disk space became available and full workspace verification
  passed.

Targeted verification that passed:

- `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p awidat-core vc::`
- `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p awidat-core vedit_`
- `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo check -p awidat-cli`
- `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo check -p awidat-desktop`
- `pnpm exec tsc --noEmit`
- `pnpm test`
- `make check`

## Phase 2: Branches and Bounded Merge

Entry criteria:

- Phase 1 tests pass.
- User approves the bounded merge constraint: only merge refs whose
  changed clip ids do not overlap; otherwise return a conflict requiring
  a human decision.
- Decision source:
  `docs/superpowers/specs/2026-05-21-collaboration-open-decisions.md`.
- Copy/paste approval packet:
  `docs/superpowers/specs/2026-05-21-collaboration-decision-packet.md`.

Implementation tasks:

- [x] Add branch list/create/switch wrappers under `awidat_core::vc`.
- [x] Add changed-clip-id extraction from `CommittedDiff`.
- [x] Expose changed-clip-id extraction through read-only
  `vedit_changed_clip_ids`.
- [x] Expose changed-clip-id extraction through desktop
  `changed_vedit_clip_ids`.
- [x] Include changed clip ids in desktop `diff_vedit_refs` and commit
  show diff responses.
- [x] Add read-only bounded merge preflight that refuses overlapping
  clip ids in its report.
- [x] Add `vedit_branch` and `vedit_checkout` tools.
- [ ] Add `vedit_merge` after bounded merge behavior is approved.
- [x] Register tools in CLI, TUI, and desktop sessions.
- [x] Add desktop commands after the tool behavior is verified.
- [x] Update version-control skill docs with branch/merge constraints.

Verification:

- [x] Unit tests for branch wrappers.
- [x] Unit tests for changed clip id extraction.
- [x] Tool tests for read-only changed clip id reporting.
- [x] Desktop command response test for changed clip id reporting.
- [x] Desktop diff response test for changed clip id reporting.
- [x] Unit tests for merge allowed preflight cases.
- [x] Tool tests for refused merge preflight conflicts.
- [x] Desktop response tests for refused merge preflight conflicts.
- [x] Tool tests for branch/checkout behavior.
- [x] `cargo test -p awidat-core vc::`
- [x] `cargo test -p awidat-core vedit_`

## Phase 3: User-Authored Notes and Rich Anchors

Entry criteria:

- User decides whether authored review notes share the current
  `notes.json` store or use a separate store.
- Existing notes migration strategy is accepted.
- Decision source:
  `docs/superpowers/specs/2026-05-21-collaboration-open-decisions.md`.
- Copy/paste approval packet:
  `docs/superpowers/specs/2026-05-21-collaboration-decision-packet.md`.

Implementation tasks:

- [ ] Add author and anchor models in core notes.
- [ ] Preserve load compatibility for current `anchor_at_s` notes.
- [ ] Add creation/update commands for user-authored notes.
- [ ] Update desktop store deserialization.
- [ ] Update NotesPanel only where needed for author and anchor display.
- [ ] Keep dismissal memory scoped to agent-authored notes.

Verification:

- [ ] Core notes migration tests.
- [ ] Tauri notes command tests where practical.
- [ ] Desktop type checks.
- [ ] UI harness check for existing NotesPanel behavior.

## Phase 4: Desktop Diff and Review View

Entry criteria:

- Phase 1 desktop commands are compiled and stable.
- User accepts the view scope: local history/review UI, not a hosted
  review page.

Implementation tasks:

- [ ] Add a desktop history/detail view using `list_vedit_commits` and
  `show_vedit_commit`.
- [ ] Add tag listing and tag creation affordance.
- [ ] Add clip blame drawer or panel.
- [ ] Render structural and animation changes as editor-readable rows.

Verification:

- [ ] Desktop type checks.
- [ ] UI harness update.
- [ ] Browser screenshot verification if the dev app can run.

## Phase 5: Render-to-Review Handoff

Entry criteria:

- User accepts local package handoff as the first review-link substitute.
- Review manifest schema is accepted.
- Decision source:
  `docs/superpowers/specs/2026-05-21-collaboration-open-decisions.md`.
- Copy/paste approval packet:
  `docs/superpowers/specs/2026-05-21-collaboration-decision-packet.md`.

Implementation tasks:

- [ ] Add review manifest type.
- [ ] Add manifest writer tied to a completed timeline render.
- [ ] Include vedit commit, timeline hash, tag names, commit header, and
  reasoning body.
- [ ] Add desktop command to create or reveal a review package.

Verification:

- [ ] Manifest serialization tests.
- [ ] Command tests using fake render artifacts.
- [ ] Optional ffmpeg smoke only when disk and fixtures allow.

## Phase 6: Third-Party Comment Ingest

Entry criteria:

- Authored note schema exists.
- Provider is selected.
- API credentials or representative payload fixtures are available.

Implementation tasks:

- [ ] Add provider payload parser.
- [ ] Map comments into authored anchored notes.
- [ ] Add idempotency for repeated webhook/import payloads.
- [ ] Add opt-in command or integration entrypoint.

Verification:

- [ ] Fixture parser tests.
- [ ] Idempotency tests.
- [ ] No live network dependency in default test suite.

## Deferred: Multi-User Awareness and Cloud Sync

These remain outside the local-first scope until the product direction is
settled. They require a coordination model beyond local `.vedit` refs and
should not be implemented as lock-file patches in this worktree.
