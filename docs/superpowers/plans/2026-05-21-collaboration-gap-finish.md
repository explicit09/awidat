# Collaboration Gap Finish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the first local-first collaboration gap slice by adding named vedit checkpoints, commit show, blame projection, branch alternates with checkout, and corrected version-control docs.

**Architecture:** Keep `vedit_core` behind `awidat_core::vc`. Add narrow wrapper APIs, then expose them through focused tool modules and runtime registries. Avoid merge and external review-service integration in this slice.

**Tech Stack:** Rust 2024, `vedit-core`, serde JSON tool responses, existing Awidat `ToolHandler` pattern.

---

### Task 1: Core vedit wrapper APIs

**Files:**
- Modify: `crates/core/src/vc/mod.rs`

- [x] Add `tag_ref(repo, name, refstr)` and `list_tags(repo)`.
- [x] Add `show_commit(repo, refstr)` returning commit metadata plus `CommittedDiff`.
- [x] Add `blame_clip(repo, clip_id, start_ref, limit)` returning matching commit/change entries.
- [x] Add unit tests in `vc::tests`.

### Task 2: Tool modules

**Files:**
- Create: `crates/core/src/tools/vedit_tag.rs`
- Create: `crates/core/src/tools/vedit_show.rs`
- Create: `crates/core/src/tools/vedit_blame.rs`
- Modify: `crates/core/src/tools/mod.rs`

- [x] Implement argument parsing and JSON responses for each tool.
- [x] Add focused tool tests using temporary minimal OTIO projects.

### Task 3: Runtime registration

**Files:**
- Modify: `crates/cli/src/chat_cmd.rs`
- Modify: `crates/cli/src/tui_cmd.rs`
- Modify: `apps/desktop/src-tauri/src/session.rs`

- [x] Import and register `VeditTagTool`, `VeditShowTool`, and `VeditBlameTool`.
- [x] Import and register `VeditBranchTool` and `VeditCheckoutTool`.
- [x] Update system-prompt collaboration tool lists only where they enumerate tools.

### Task 3.5: Desktop command surface

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/vedit.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`

- [x] Expose `list_vedit_tags` and `tag_vedit_ref`.
- [x] Expose `show_vedit_commit`.
- [x] Expose `blame_vedit_clip`.
- [x] Include animation-change arrays in desktop diff responses.
- [x] Expose `list_vedit_branches`, `create_vedit_branch`, and `checkout_vedit_branch`.

### Task 4: Docs cleanup

**Files:**
- Modify: `skills/version-control/SKILL.md`

- [x] Remove stale text saying apply auto-commit is future work.
- [x] Document `vedit_tag`, `vedit_show`, `vedit_blame`, `vedit_branch`, and `vedit_checkout`.
- [x] Keep merge language explicit: branch alternates exist, bounded merge remains roadmap work.

### Task 5: Verification

**Commands:**
- `cargo fmt --all -- --check`
- `cargo test -p awidat-core vc::`
- `cargo test -p awidat-core vedit_`
- `make check`

Expected: all pass when disk space allows build artifacts. If the machine still has no free space, report the exact `df -h` evidence and the blocked commands.

Current evidence:

- [x] `cargo fmt --all -- --check` passes.
- [x] `git diff --check` passes.
- [x] `cargo test -p awidat-core vc::` passes.
- [x] `cargo test -p awidat-core vedit_` passes.
- [x] `cargo check -p awidat-cli` passes.
- [x] `cargo check -p awidat-desktop` passes.
- [x] `pnpm exec tsc --noEmit` passes in `apps/desktop`.
- [x] `pnpm test` passes in `apps/desktop`.
- [x] `make check` passes.

Latest disk evidence:

```text
Filesystem      Size    Used   Avail Capacity  Mounted on
/dev/disk3s5   228Gi   180Gi   946Mi   100%    /System/Volumes/Data
```
