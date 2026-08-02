# Desktop Publishing Consolidation Implementation Plan

> Execute in the existing `codex/montage-simplification` worktree. Keep changes limited to the dormant desktop-local publishing path and its direct frontend mirrors.

**Goal:** Delete the unused local publishing implementation and leave one server-backed publishing path without changing live publishing behavior.

**Architecture:** The desktop retains a read-only local disclosure command. All account, OAuth, scheduling, upload, retry, and status operations continue through `commands/social.rs`, `SocialClient`, and `montage-social-server`.

---

### Task 1: Lock the live-path contract with focused tests

**Files:**
- Modify: `apps/desktop/tests/campaign-publisher.test.ts`
- Modify: `apps/desktop/tests/upload-prefs.test.ts`

1. Retain campaign request and server publisher coverage.
2. Remove tests that exist solely for `startCampaignUploads` and `poll_upload_states`.
3. Replace backend-revision preference tests with local preference behavior coverage if the existing harness permits it.
4. Run the focused frontend test commands from `package.json`.

### Task 2: Remove frontend bridges to the dormant backend

**Files:**
- Modify: `apps/desktop/src/campaign/publisher.ts`
- Modify: `apps/desktop/src/state/uploadMetadata.ts`
- Modify: `apps/desktop/src/state/uploadPrefs.ts`
- Modify: `apps/desktop/src/App.tsx`
- Modify: `apps/desktop/src/app/useRenderQueueWorker.ts`
- Modify: `apps/desktop/src/app/renderQueue.ts`
- Modify: `apps/desktop/src/state/aiDisclosure.ts`

1. Delete `startCampaignUploads`, its local queue wire types, and poll helper.
2. Remove best-effort `set_upload_metadata` persistence; localStorage remains the store.
3. Remove backend preference persistence/hydration; localStorage remains authoritative.
4. Remove App’s preference hydrate effect.
5. Keep `compute_ai_disclosure` for visible local analysis, remove the nonfunctional auto-disclose control, and update the warning copy.
6. Run typecheck and focused frontend tests.

### Task 3: Collapse desktop publishing to disclosure detection

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands/publishing.rs`
- Modify: `apps/desktop/src-tauri/src/publishing/mod.rs`
- Delete: all `apps/desktop/src-tauri/src/publishing/*.rs` except `ai_disclosure.rs` and `mod.rs`
- Modify: `apps/desktop/src-tauri/src/state.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/src-tauri/src/commands/auth.rs`

1. Write/retain tests that `compute_ai_disclosure` returns project-derived disclosure and empty disclosure without a project.
2. Reduce the command module to that read-only function.
3. Remove `UploadQueue` state and unregister every deleted local publishing command.
4. Reduce the publishing module to the disclosure implementation.
5. Delete provider, OAuth, keychain, storage, upload, and queue modules.
6. Run desktop tests.

### Task 4: Remove dependencies and stale references

**Files:**
- Modify: `apps/desktop/src-tauri/Cargo.toml`
- Modify: `Cargo.lock`

1. Remove `async-trait` and target-specific `keyring` dependencies if no longer directly used.
2. Run `cargo check -p montage-desktop --all-targets` and dependency analysis.
3. Search for every deleted command/module name; require zero production references.

### Task 5: Verify and commit

1. Run focused frontend tests and `pnpm --dir apps/desktop typecheck`.
2. Run `cargo fmt --all -- --check`.
3. Run `cargo test -p montage-desktop`.
4. Run `cargo clippy -p montage-desktop --all-targets -- -D warnings`.
5. Run `cargo check --workspace --all-targets` because dependencies and Tauri registration changed.
6. Confirm `git diff --check`, branch status, deletion totals, and preserved main-checkout dirty files.
7. Commit the design/plan and implementation in reviewable units.
