# Dead Desktop Adapter Removal

**Date:** 2026-08-02

## Decision

Remove four legacy Tauri surfaces that have no frontend production or test callers:

- Standalone dismissal list/add/remove commands.
- The desktop local-review-package wrapper.
- The raw motion-sidecar reader.
- The static social-provider registry command.

## Retained behavior

- Notes still persist dismissal buckets through the live `set_note_status` command.
- Local review packages remain available through the active MCP tool backed by `montage_core::review`.
- Motion generation remains part of import/indexing; the UI reads the richer `read_motion_regions` evidence endpoint.
- Every server-backed social account, scheduling, upload, publish, polling, retry, cancel, and audit command remains registered.

## Evidence

- Exact command-name searches return no desktop TypeScript or test references.
- The dismissal and review command modules have no Rust callers outside Tauri registration.
- `read_motion` is not called; its module's generation helpers have live import/index callers and remain.
- `social_providers` is isolated from the live server-backed social command set.

## Verification

- Require zero references to the four removed IPC surfaces.
- Run formatting, desktop all-target compile, full desktop library tests, and strict clippy.
- Re-run frontend typecheck and the notes/social focused tests.
