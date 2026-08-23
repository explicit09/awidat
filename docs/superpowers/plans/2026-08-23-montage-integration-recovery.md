# Montage Integration Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve every meaningful Awidat working-tree change, integrate the verified Montage lifecycle fixes with the published performance branch, reverify the product, and establish exact local/GitHub parity without force-pushing.

**Architecture:** Split the current dirty `main` state into an audit-derived lifecycle commit and a separate preservation branch for three unrelated edits. Build a clean integration branch from `codex/montage-performance-optimization`, which already contains `codex/montage-simplification`, then cherry-pick and reconcile the lifecycle commit before testing and fast-forwarding `main`.

**Tech Stack:** Git, Rust/Cargo, Tauri, React/TypeScript, pnpm, macOS app packaging

**Spec:** `docs/reviews/montage-memory-retention-audit-2026-08-07.html`

## Global Constraints

- Preserve all current source changes and all linked worktrees.
- Never force-push, hard-reset, or delete a worktree.
- Do not commit `.playwright-mcp/` logs or root `target` symlinks.
- Keep unrelated `App.css`, `find_episode_start.rs`, and `crates/secrets/src/lib.rs` edits out of the lifecycle integration commit.
- Treat `codex/montage-performance-optimization` as the integration base because it contains `codex/montage-simplification`.
- Rerun current verification; prior August 7 results are evidence of intent, not proof of the integrated tree.

---

### Task 1: Preserve the lifecycle audit change set

**Files:**
- Modify/Add: the exact audit-derived paths enumerated by the August 7 rollout, including desktop project/transcript state, media lifecycle modules, cache modules, focused tests, `apps/desktop/package.json`, and `docs/reviews/montage-memory-retention-audit-2026-08-07.html`
- Exclude: `.playwright-mcp/**`, `target`, `apps/desktop/src/App.css`, `crates/core/src/montage_mcp/tools/find_episode_start.rs`, `crates/secrets/src/lib.rs`

**Interfaces:**
- Consumes: dirty `main` at `f7c610f3`
- Produces: one commit on `codex/montage-memory-lifecycle-20260823`

- [ ] **Step 1:** Create and switch to `codex/montage-memory-lifecycle-20260823` without altering the working tree.
- [ ] **Step 2:** Add `.playwright-mcp/` and `/target` to the shared local Git exclude file; verify only source/review changes remain visible.
- [ ] **Step 3:** Stage only the audit-derived file list and inspect `git diff --cached --stat` plus `git diff --cached --check`.
- [ ] **Step 4:** Run the focused frontend lifecycle tests and targeted Rust desktop tests covering project/transcript cache changes.
- [ ] **Step 5:** Commit as `fix(desktop): bound project media lifecycle`.

### Task 2: Preserve unrelated existing edits separately

**Files:**
- Modify: `apps/desktop/src/App.css`
- Modify: `crates/core/src/montage_mcp/tools/find_episode_start.rs`
- Modify: `crates/secrets/src/lib.rs`

**Interfaces:**
- Consumes: the remaining tracked edits after Task 1
- Produces: `codex/awidat-unrelated-preservation-20260823` with a separate preservation commit

- [ ] **Step 1:** Create and switch to `codex/awidat-unrelated-preservation-20260823` from the lifecycle commit.
- [ ] **Step 2:** Stage exactly the three unrelated paths and inspect their full cached diff.
- [ ] **Step 3:** Run `cargo fmt --all -- --check` and focused checks for the affected Rust crates when available.
- [ ] **Step 4:** Commit as `chore: preserve pre-existing awidat edits`.
- [ ] **Step 5:** Push the preservation branch non-force and verify its remote commit ID.

### Task 3: Create the clean Montage integration branch

**Files:**
- Create worktree: `.worktrees/montage-integration-20260823`
- Add: `docs/superpowers/plans/2026-08-23-montage-integration-recovery.md`
- Add: `docs/superpowers/plans/2026-08-23-montage-integration-recovery.html`

**Interfaces:**
- Consumes: published `codex/montage-performance-optimization` and lifecycle commit from Task 1
- Produces: `codex/montage-integration-20260823`

- [ ] **Step 1:** Create the integration worktree from `codex/montage-performance-optimization`.
- [ ] **Step 2:** Verify simplification is an ancestor of the performance tip.
- [ ] **Step 3:** Cherry-pick the lifecycle commit.
- [ ] **Step 4:** Resolve conflicts by preserving the performance branch’s newer structure while reapplying lifecycle invariants: project-scoped caches, stale-result suppression, bounded preview work, explicit media release, and persistent preview-host reuse.
- [ ] **Step 5:** Run `git diff --check` and inspect every resolved file against the lifecycle tests.
- [ ] **Step 6:** Add this plan and its HTML companion, then commit the plan separately.

### Task 4: Verify the integrated source

**Files:**
- Test: `apps/desktop/tests/*lifecycle*`, `apps/desktop/tests/latest-preview-queue.test.ts`, `apps/desktop/tests/preview-media-release.test.ts`, `apps/desktop/tests/recent-project-preview.test.ts`, `apps/desktop/tests/persistent-preview-host.test.mjs`
- Test: Rust desktop project/transcript tests

**Interfaces:**
- Consumes: resolved integration branch
- Produces: fresh focused and full verification evidence

- [ ] **Step 1:** Run focused frontend lifecycle/cache tests; require zero failures.
- [ ] **Step 2:** Run targeted Rust tests for project and transcript commands; require zero failures.
- [ ] **Step 3:** Run the complete desktop frontend suite once on the stable candidate.
- [ ] **Step 4:** Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo check --workspace --all-targets`.
- [ ] **Step 5:** Run `cargo test -p montage-desktop`; require zero failures.

### Task 5: Verify the packaged Montage application

**Files:**
- Build output: external Cargo target configured by the worktree or `/Volumes/My Passport for Mac/awidat-build`

**Interfaces:**
- Consumes: green integrated source
- Produces: fresh exact-package startup/lifecycle evidence

- [ ] **Step 1:** Build with `pnpm tauri build --debug --bundles app` from `apps/desktop`.
- [ ] **Step 2:** Launch the newly built `Montage.app`, not an older installed copy.
- [ ] **Step 3:** Exercise project open, switch, preview, close, quit, and reopen using a disposable fixture.
- [ ] **Step 4:** Verify expected content and confirm main/GPU/network/WebContent processes exit on native Quit.

### Task 6: Review and publish

**Files:**
- Review: full integration diff from `origin/main`

**Interfaces:**
- Consumes: fully verified integration branch
- Produces: exact local/remote parity for integration and `main`

- [ ] **Step 1:** Run default-P0 autoreview once; verify every finding before editing and stop after at most two review-triggered fix cycles.
- [ ] **Step 2:** Push `codex/montage-integration-20260823` non-force and compare exact local/remote commit IDs.
- [ ] **Step 3:** Fast-forward local `main` to the verified integration tip.
- [ ] **Step 4:** Push `main` non-force, fetch again, and compare `main`, `origin/main`, and `git ls-remote` commit IDs.
- [ ] **Step 5:** Confirm all linked worktrees and preservation branches remain intact.
