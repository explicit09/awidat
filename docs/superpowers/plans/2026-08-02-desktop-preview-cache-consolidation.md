# Desktop Preview Cache Consolidation Plan

**Goal:** Remove the unused duplicate desktop cache API while preserving the active core/MCP lifecycle.

### Task 1: Remove the dead desktop surface

- Delete `apps/desktop/src-tauri/src/commands/preview_cache.rs`.
- Remove its command module and both Tauri handler registrations.
- Verify the deleted command names and module path have no desktop references.

### Task 2: Prove the retained path

- Run the focused `montage_core::preview_cache` and `preview_refresh_executor` tests.
- Confirm the MCP `run_preview_cache_refresh` tool still compiles against the shared core implementation.
- Leave proxy, thumbnail, waveform, import, and project-load backfills untouched.

### Task 3: Verify the boundary

- Run `cargo fmt --all -- --check`.
- Run desktop all-target compile and strict clippy.
- Run the desktop library suite and final workspace checks.
