# Montage Simplification Wave 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove source-proven dead weight and repair Montage's first-party development lane without changing the documented editing or publishing behavior.

**Architecture:** Keep the current crate boundaries and live import-to-verify flow. Wave 1 deletes configuration, dependencies, and professional orchestration that have no runtime effect or production callers; every cut is followed by the narrowest relevant compile/test gate.

**Tech Stack:** Rust 2024 workspace, Cargo, Make, Tauri 2, React/TypeScript, Python `uv` indexer workspace.

## Global Constraints

- Preserve the user's existing edits in `apps/desktop/src/App.css`, `crates/core/src/montage_mcp/tools/find_episode_start.rs`, and `crates/secrets/src/lib.rs`.
- Do not remove publishing behavior, compatibility readers, EDL safety, render verification, skill allowlists, or direct MCP tool exposure.
- Use `/Volumes/My Passport for Mac/awidat-build/main-target` for Rust build output.
- Stage and commit only files changed by this plan.
- Run the narrow check before each broader check.

---

### Task 1: Repair the first-party control plane

**Files:**
- Modify: `Makefile`
- Modify: `apps/desktop/src-tauri/Cargo.toml`
- Modify: `.gitignore`
- Modify: `crates/cli/Cargo.toml`
- Modify: `crates/cli/src/bin/montage-mcp-server.rs`

**Interfaces:**
- Consumes: current Cargo package names and the external Codex sidecar path.
- Produces: a resolvable `MONTAGE_APP_PACKAGES` list and no inert publishing feature flag.

- [ ] **Step 1: Capture the broken check-lane premise**

Run: `make fmt-app`

Expected before the change: failure naming `montage-tools` or `montage-sandboxing` as a non-member.

- [ ] **Step 2: Remove only stale package entries**

Delete `-p montage-tools` and `-p montage-sandboxing` from
`MONTAGE_APP_PACKAGES`. Do not move agent packages into the app lane.

- [ ] **Step 3: Remove the inert feature declaration**

Delete this entire block from `apps/desktop/src-tauri/Cargo.toml`:

```toml
[features]
default = ["legacy_local_publishing"]
legacy_local_publishing = []
```

Verify no source conditional exists:

```sh
rg -n 'legacy_local_publishing' apps crates
```

Expected after the change: no matches.

- [ ] **Step 4: Refresh stale MCP migration descriptions**

Replace the CLI manifest and binary comments that claim the MCP server exposes
one stub tool with wording that states it serves the current Montage tool
router through stdio. Do not change executable behavior.

- [ ] **Step 5: Verify local artifacts are ignored**

Run:

```sh
git check-ignore .worktrees/example .agent-loops/example .codex/example
```

Expected: all three paths print and the command succeeds.

- [ ] **Step 6: Verify the repaired lane**

Run: `CARGO_TARGET_DIR='/Volumes/My Passport for Mac/awidat-build/main-target' make fmt-app`

Expected: exit 0.

- [ ] **Step 7: Commit the control-plane repair**

```sh
git add .gitignore Makefile apps/desktop/src-tauri/Cargo.toml crates/cli/Cargo.toml crates/cli/src/bin/montage-mcp-server.rs
git commit -m "chore: repair first-party development lanes"
```

### Task 2: Remove unused direct dependencies

**Files:**
- Modify: `crates/core/Cargo.toml`
- Modify: `crates/render-gpu/Cargo.toml`
- Modify: `crates/index/Cargo.toml`
- Modify: `crates/cli/Cargo.toml`
- Modify: `crates/social/Cargo.toml`
- Modify: `crates/social-server/Cargo.toml`
- Modify: `crates/desktop-protocol/Cargo.toml`
- Modify: `apps/desktop/src-tauri/Cargo.toml`
- Modify: `Cargo.lock`

**Interfaces:**
- Consumes: existing workspace dependency versions.
- Produces: the same crate APIs with ten fewer direct dependency declarations.

- [ ] **Step 1: Reconfirm source absence**

Run identifier searches for `async_stream`, `eventsource_stream`, `rusqlite`,
`tokio_util`, `tracing`, `rand`, `thiserror`, `chrono`, and `anyhow` in the
specific package sources. Expected: only the manifest declarations listed in
the design, with no Rust source consumers.

- [ ] **Step 2: Remove the ten declarations**

Delete exactly the dependency entries enumerated in the design. Leave
workspace-level versions intact because other crates use them.

- [ ] **Step 3: Refresh the lockfile without upgrading packages**

Run:

```sh
CARGO_TARGET_DIR='/Volumes/My Passport for Mac/awidat-build/main-target' cargo metadata --format-version 1 --no-deps >/dev/null
```

Expected: exit 0; accept only dependency-edge changes in `Cargo.lock`.

- [ ] **Step 4: Compile every touched package**

Run:

```sh
CARGO_TARGET_DIR='/Volumes/My Passport for Mac/awidat-build/main-target' cargo check \
  -p montage-core -p montage-render-gpu -p montage-index -p montage-cli \
  -p montage-social -p montage-social-server -p montage-desktop-protocol \
  -p montage-desktop --all-targets
```

Expected: exit 0 with no missing-crate errors.

- [ ] **Step 5: Commit dependency hygiene**

```sh
git add Cargo.lock crates/core/Cargo.toml crates/render-gpu/Cargo.toml crates/index/Cargo.toml crates/cli/Cargo.toml crates/social/Cargo.toml crates/social-server/Cargo.toml crates/desktop-protocol/Cargo.toml apps/desktop/src-tauri/Cargo.toml
git commit -m "chore: remove unused direct dependencies"
```

### Task 3: Remove dormant professional orchestration

**Files:**
- Modify: `crates/core/src/professional.rs`
- Modify: `crates/core/tests/professional_orchestration.rs`

**Interfaces:**
- Consumes: `MontageTimelineMetadata`, `Timeline`, and professional protocol
  types required by current callers.
- Produces: unchanged signatures for `build_workflow_lens_snapshots`,
  `inspect_pre_autonomy_readiness`, and `derive_audio_finishing_state`.

- [ ] **Step 1: Record the live behavior baseline**

Run:

```sh
CARGO_TARGET_DIR='/Volumes/My Passport for Mac/awidat-build/main-target' \
  cargo test -p montage-core professional -- --nocapture
```

Expected baseline: all matching unit and integration tests pass.

- [ ] **Step 2: Reduce the module to the production dependency closure**

Retain the three public functions above, `WorkflowLensSnapshot`,
`LensCorrectionAction`, `OrchestrationInspection`, `PlannerPassEdge`, and all
private helpers required by those functions. Remove all other public items and
their private-only helper closure. Preserve function bodies instead of
rewriting their logic.

- [ ] **Step 3: Keep focused integration coverage**

In `professional_orchestration.rs`, retain tests for workflow-lens snapshots
and pre-autonomy inspection. Remove the `record_learning_signal` import and
test because that function has no production caller. Keep the existing audio
finishing unit test in `professional.rs`.

- [ ] **Step 4: Compile to discover accidental closure gaps**

Run:

```sh
CARGO_TARGET_DIR='/Volumes/My Passport for Mac/awidat-build/main-target' cargo check -p montage-core --all-targets
```

Expected: exit 0. If a removed private helper is required by a retained live
function, restore that helper unchanged; do not restore unrelated public APIs.

- [ ] **Step 5: Run focused professional tests**

Run the baseline command again. Expected: all remaining matching tests pass,
including professional EDL operations and the three live workflows.

- [ ] **Step 6: Confirm dormant symbols are absent**

Search for every removed public type/function from the design inventory.
Expected: no definitions and no call sites.

- [ ] **Step 7: Commit dormant-code removal**

```sh
git add crates/core/src/professional.rs crates/core/tests/professional_orchestration.rs
git commit -m "refactor: remove dormant professional orchestration"
```

### Task 4: Verify Wave 1 end to end

**Files:**
- Modify only if a verification failure identifies a regression in a Wave 1 file.

**Interfaces:**
- Consumes: completed Tasks 1-3.
- Produces: evidence that the first-party codebase is smaller and current product behavior still builds.

- [ ] **Step 1: Run Rust formatting and focused package tests**

```sh
CARGO_TARGET_DIR='/Volumes/My Passport for Mac/awidat-build/main-target' make fmt-app
CARGO_TARGET_DIR='/Volumes/My Passport for Mac/awidat-build/main-target' cargo test -p montage-core professional
CARGO_TARGET_DIR='/Volumes/My Passport for Mac/awidat-build/main-target' cargo test -p montage-cli -p montage-social -p montage-social-server -p montage-desktop-protocol
```

- [ ] **Step 2: Run frontend and Python gates**

```sh
pnpm --dir apps/desktop typecheck
python3 python/scripts/smoke_indexers.py --safe
```

- [ ] **Step 3: Audit the final diff**

Run `git diff main...HEAD --stat`, `git diff main...HEAD --check`, and
`git status --short`. Confirm the user's pre-existing modified files do not
appear in the branch diff.

- [ ] **Step 4: Record outcome metrics**

Report lines removed, direct dependencies removed, repaired commands, tests
executed, and any deferred or unproven areas. Do not claim the larger ongoing
simplification goal is complete after Wave 1.
