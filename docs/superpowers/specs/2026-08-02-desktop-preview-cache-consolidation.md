# Desktop Preview Cache Consolidation

**Date:** 2026-08-02

## Decision

Delete the unused desktop-only preview-cache command module. Its two Tauri endpoints have no frontend production or test callers, and the module is referenced only by its own registration and tests.

Keep the active core/MCP path:

- `montage_core::preview_cache` owns the shared summary, selection, and persisted lifecycle model.
- `run_preview_cache_refresh` uses the ffmpeg-backed executor, resumes pending work, and skips completed tasks.
- Existing desktop import, proxy, thumbnail, waveform, and background-backfill paths remain unchanged.

## Evidence

- Neither `preview_cache_summary` nor `preview_cache_refresh` appears in desktop TypeScript or tests.
- No Rust production module calls `commands::preview_cache`; only the Tauri handler list exposes it.
- The desktop module duplicates summary types, task selection, artifact checks, and execution already implemented in the core path.
- The core implementation has a stronger lifecycle contract, including persistence, resume semantics, and a busy guard.

## Verification

- Require zero desktop-command references after deletion.
- Run formatting, desktop all-target compile, desktop tests, and strict clippy.
- Re-run focused core preview-cache and executor tests to prove the retained path.
