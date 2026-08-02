# Dead Desktop Adapter Removal Plan

**Goal:** Remove unreachable desktop IPC wrappers without removing the capabilities behind their live replacement paths.

### Task 1: Delete isolated adapters

- Delete the desktop dismissal and local-review command modules.
- Remove their module declarations and Tauri registrations.
- Remove only the `read_motion` and `social_providers` wrappers, their unused imports, and wrapper-only tests.

### Task 2: Prove replacements remain

- Confirm notes still call `set_note_status` with dismissal buckets.
- Confirm local-review packaging remains registered in MCP.
- Confirm motion import/index generation and `read_motion_regions` remain.
- Confirm the server-backed social command set remains unchanged.

### Task 3: Verify

- Run Rust formatting, all-target compile, library tests, and strict clippy.
- Run desktop TypeScript typecheck and focused notes/social tests.
- Finish with workspace-wide checks.
