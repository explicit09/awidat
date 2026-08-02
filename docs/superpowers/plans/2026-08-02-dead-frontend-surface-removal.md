# Dead Frontend Surface Removal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete eight source-proven unmounted desktop component files while preserving all live v2 behavior and backend contracts.

**Architecture:** Treat the active `App.tsx` import graph and explicit `appGlue` ownership comments as the runtime boundary. Characterize the active app before deletion, delete only the zero-runtime-in-degree component closure, and verify the entire desktop frontend again rather than moving responsibilities.

**Tech Stack:** React 19, TypeScript 5.8, Vite 7, Node test scripts, Tauri 2.

## Global Constraints

- Do not edit `apps/desktop/src/App.css`.
- Preserve `app/EmptyState.tsx`, `notes/store.ts`, Tauri notes commands, dismissal persistence, and generated protocol.
- Delete only the eight paths enumerated in the design.
- Use the existing desktop test runner; do not add a source-absence change detector or a new test framework.

---

### Task 1: Capture the live frontend baseline

**Files:**
- Modify: none.

**Interfaces:**
- Consumes: the current mounted v2 desktop tree.
- Produces: fresh behavior evidence from the exact checks that will run after deletion.

- [ ] **Step 1: Run TypeScript typechecking**

Run: `pnpm --dir apps/desktop typecheck`

Expected: PASS.

- [ ] **Step 2: Run the complete frontend suite**

Run: `pnpm --dir apps/desktop test`

Expected: PASS, including desktop UI smoke and stage-harness verification.

### Task 2: Delete only the dormant component closure

**Files:**
- Delete: the eight component paths from the design.

**Interfaces:**
- Consumes: the fresh characterization baseline from Task 1.
- Produces: the same active v2 module graph with no retired source files.

- [ ] **Step 1: Remove the eight files**

Use an explicit patch containing one deletion per path. Do not remove the notes store, project commands, shared empty state, or CSS selectors.

- [ ] **Step 2: Verify no stale import remains**

Run an `rg` search for the eight exported component symbols under `apps/desktop/src`. Expected: no definition or import for those symbols; comments that merely mention `MediaPane` may remain until the separate CSS/state documentation wave.

### Task 3: Prove frontend behavior and checkout isolation

**Files:**
- Modify only if a failure is caused by a stale reference to one of the eight deleted files.

**Interfaces:**
- Consumes: the reduced frontend tree.
- Produces: fresh broad evidence that current desktop behavior is unchanged.

- [ ] **Step 1: Typecheck**

Run: `pnpm --dir apps/desktop typecheck`

Expected: PASS.

- [ ] **Step 2: Run the complete desktop frontend suite**

Run: `pnpm --dir apps/desktop test`

Expected: PASS, including the desktop UI smoke and stage harness checks.

- [ ] **Step 3: Audit the diff**

Run: `git diff --check`, `git status --short`, and a line-count diff. Confirm `App.css` is unchanged and no QA scaffold or unrelated checkout file is staged with the implementation.

- [ ] **Step 4: Commit**

Stage only the eight deletions and the reviewed design/plan updates. Commit with `refactor: remove dead frontend surfaces`.
