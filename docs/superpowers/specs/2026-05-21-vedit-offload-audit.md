# 2026-05-21: Vedit Offload Audit and Issue Plan

You asked to move core ownership to `vedit` where possible and file issues instead of maintaining local copies.

## Scope decision

We should keep Awidat-owned work in this repo and defer the following to `vedit`.

### Defer to `vedit` (create upstream issues)

1. **Expose non-overlap clip-id merge policy at merge time**
   - Current behavior in Awidat wrapper: preflight + conflict check by changed clip IDs + overlay of changed clips.
   - Vedit currently only has track-level merge behavior in core.
   - Request to `vedit`:
     - add API (or strategy/config) to return changed clip IDs for a merge preflight,
     - support an opt-in non-overlapping clip-id merge mode,
     - expose a structured merge result payload with source/target/parents/changed ids.

2. **Expose mutable review-package metadata for commit-backed review artifacts**
   - Awidat currently needs review object fields aligned with package provenance and timeline identity.
   - If `vedit` is intended to own this surface, request:
     - schema extension for review artifacts (render path, generated-at, vedit commit id, timeline hash, tags, commit header, reasoning body),
     - stable JSON serialization contract + protocol export for clients.

3. **Merge API ergonomics for command-layer callers**
   - Awidat CLI/TUI/desktop currently needs a single stable `vedit merge` command surface.
   - Request:
     - standardized command input shape (source ref, optional target ref defaulting to checked-out commit/branch),
     - output including timing and parent/changed-id summary for downstream UIs.
   - GitHub issue: https://github.com/explicit09/vedit/issues/5

### Keep local in Awidat

1. **Review notes persistence in `.awidat/notes.json`**
   - Existing Awidat schema already owns persistence lifecycle, status transitions, and frontend coupling.

2. **Local review package authoring/presentation policy**
   - UI and command exposure in desktop/agent tools is specific to Awidat product workflows.

3. **Third-party ingest / multi-user sync / cloud sync**
   - Already deferred until credentials/providers are available.

## Suggested issue titles for upstream `vedit`

- `vedit-core: add changed-clip-id merge preflight and non-overlap-only merge mode`
- `vedit-core: expose clip-level merge result metadata for downstream UI`
- `vedit-core: add merge command contract for source+optional target with changed-id summary`

Created:
- https://github.com/explicit09/vedit/issues/3
- https://github.com/explicit09/vedit/issues/4
- https://github.com/explicit09/vedit/issues/5
