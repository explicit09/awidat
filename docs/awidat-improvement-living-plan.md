# Awidat Improvement Living Plan

This document is the running ledger for the Awidat improvement goal. Update it before and after each implementation slice so the current state, next action, and verification evidence stay visible.

Last updated: 2026-05-23, resumable preview worker pool slice complete

## Goal

Move the improvement research report from partial implementation to production-complete behavior in the repo:

- unify caption and subtitle handling into a real caption layout/render/verification path;
- make render backend selection deterministic, content-aware, and explainable;
- expose stream-copy/remux as a first-class fast path;
- strengthen rendered-output verification with evidence gates and boundary checks;
- improve proxy/preview caching from summaries into a practical refresh subsystem;
- wire capability metadata into planning, preflight, render manifests, and verification;
- run broad workspace verification after the covered-but-unfinished slices are completed.

## Current Position

The repo already contains substantial local implementation work across `crates/core`, `crates/render`, desktop Tauri commands, CLI plumbing, and tests. The worktree is intentionally dirty while this goal is active.

Covered but not finished:

| Area | Current state | Remaining completion gate |
|---|---|---|
| Caption/subtitle unification | Caption summaries, ASS/libass burn-in, sidecar fingerprints, layout metadata, verification gates, and render-preflight caption layout/readiness output exist. | Add actual rendered-frame occlusion/readability checks later under rendered-output verification. |
| Render backend dispatch | Timeline preflight and manifests explain stream-copy eligibility and blockers; focused closure tests are green. | Continue broad verification later. |
| Stream-copy/remux | `stream_remux` tool and simple timeline stream-copy fast path exist; manifest/report tests are green. | Continue broad verification later. |
| Render verification | Manifest, feature evidence, libass, caption, loudness, stream-remux, boundary, and caption rendered-output evidence gates exist. Caption rendered-output evidence is now produced by the frame-pixel scorer when ffmpeg is available, with libass-layout derivation as a named fallback. | Complete. |
| Preview/proxy cache | Cache summary, bounded refresh selection, desktop refresh command, preflight planning, and a real executable lifecycle (PreviewRefreshExecutor trait + ffmpeg-backed production impl + run_preview_cache_refresh tool with resume semantics) exist. | Cross-process file locking is still not implemented; in-process busy guard only. |
| Capability metadata | Capabilities mention render preflight, libass layout evidence, preview cache status, stream remux, and verification limits. | Keep metadata synchronized with each feature as it graduates. |

Not yet started or not yet production-complete:

| Area | Target |
|---|---|
| GPU compositor routing maturity | Raw-stream GPU routing now avoids mixed GPU/FFmpeg overclaiming; mixed transition sets fall back with explicit evidence. Future work: broader GPU effect coverage. |
| Broad workspace verification | Run after focused slices are complete: format, clippy, package-level tests, then workspace-level checks. |

## Execution Order

1. Finish covered-but-not-complete slices first.
2. For each slice:
   - write or update the failing focused test first;
   - run the focused test and confirm the expected failure;
   - implement the minimal production change;
   - run the focused test until green;
   - run relevant format/clippy checks;
   - update this document with status and verification evidence.
3. After covered slices are complete, implement not-yet-started production gaps in the same test-first loop.
4. Only then run broad workspace verification.

## Active Slice

Status: completed.

Slice: Preview/proxy cache executable refresh status.

Why this is first: preview/proxy cache already has summary and bounded planning, but still reads like planning rather than a production refresh lifecycle. Completing it tightens an already-covered area before moving to brand-new work.

Completion gate:

- agent-facing preview-cache status can report executable refresh intent without starting render jobs: done;
- render preflight reports the same refresh execution contract when preview-cache planning is included: done;
- desktop refresh planning can filter by project-relative `asset_id`, matching the agent/preflight asset-bounded selection contract: done;
- focused tests cover bounded selection, execution status fields, and desktop asset filtering: done;
- `cargo fmt --all -- --check`, focused Rust tests, and relevant clippy checks pass: done.

Verification:

- `cargo test -p awidat-core preview_cache_status`: passed.
- `cargo test -p awidat-core --test render_preflight_tool render_preflight_can_include_bounded_preview_cache_plan`: passed.
- `cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml preview_cache_refresh_plan`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p awidat-core --tests -- -D warnings`: passed.
- `cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --tests -- -D warnings`: passed. The desktop clippy path still prints existing `ts-rs` serde-attribute warnings from generated/type export handling, but the command exits cleanly.

## Completed Slice

Status: completed.

Slice: Caption/subtitle preflight and readability surfacing.

Why this is next: caption/subtitle unification is already heavily implemented through summaries, ASS/libass sidecars, manifests, and verification gates. The remaining gap is making caption layout/readability evidence visible before render and easier for agents to act on.

Completion gate:

- render preflight includes caption summary and caption layout/readability readiness when captions are present: done;
- caption warnings are carried into preflight output without requiring a render: done;
- focused tests cover caption overlays with safe-area and word timing metadata: done;
- focused tests, format, and relevant clippy checks pass: done.

Verification:

- `cargo test -p awidat-core --test render_preflight_tool`: passed.
- `cargo test -p awidat-core --test caption_summary`: passed.
- `cargo fmt --all -- --check`: passed after applying `cargo fmt --all`.
- `cargo clippy -p awidat-core --tests -- -D warnings`: passed.

## Completed Slice

Status: completed.

Slice: Rendered-output verification beyond metadata-only caption evidence.

Why this is next: verification is already covered by manifest, feature, backend, stream-remux, caption, libass, loudness, and boundary gates. The remaining quality gap is that caption readability is still inferred from metadata and sidecars instead of a concrete artifact-level readability/occlusion signal.

Completion gate:

- `verify_render` reports a separate caption rendered-output/readability gate when caption evidence is present: done;
- the gate can pass from explicit artifact evidence and fail when captions are present but no artifact-level evidence exists: done;
- focused tests cover both pass and fail paths without requiring heavyweight media renders: done;
- focused tests, format, and relevant clippy checks pass: done.

Verification:

- `cargo test -p awidat-core caption_rendered_output_gate`: passed.
- `cargo test -p awidat-core verify_render_reports_synthetic_render_gates`: passed.
- `cargo test -p awidat-core --test capability_manifest capability_manifest_adds_explicit_known_tool_metadata`: passed.
- `cargo fmt --all -- --check`: passed after applying `cargo fmt --all`.
- `cargo clippy -p awidat-core --tests -- -D warnings`: passed.

## Completed Slice

Status: completed by focused audit.

Slice: Backend dispatch and stream-copy/remux closure.

Why this is next: backend dispatch and stream-copy/remux are already covered by tools, manifests, preflight, and blocker metadata. Before moving into untouched production gaps, confirm these covered areas have focused green tests and no missing evidence fields.

Completion gate:

- stream-copy eligible timelines expose success evidence: done;
- noneligible timelines expose deterministic blocker evidence: done;
- `stream_remux` tool emits manifest/report evidence: done;
- focused tests pass: done.

Verification:

- `cargo test -p awidat-render single_clip_timeline`: passed.
- `cargo test -p awidat-core --test render_preflight_tool render_preflight_reports_stream_copy_blockers`: passed.
- `cargo test -p awidat-core --test stream_remux_tool`: passed.

## Completed Slice

Status: completed.

Slice: Automatic caption rendered-output evidence.

Why this is next: `verify_render` now requires caption rendered-output evidence, but the evidence still has to be supplied in manifest metadata. To move toward production completeness, verification should derive a first artifact-level evidence packet automatically from required ASS sidecars when explicit evidence is absent.

Completion gate:

- `verify_render` can pass caption rendered-output evidence when a caption render has required ASS sidecars with layout evidence, even if the manifest lacks explicit `caption_rendered_output_*` metadata: done;
- explicit failing evidence still fails: done;
- missing ASS/layout evidence still fails: done;
- focused tests, format, and relevant clippy checks pass: done.

Verification:

- `cargo test -p awidat-core caption_rendered_output_gate`: passed.
- `cargo test -p awidat-core verify_render_reports_synthetic_render_gates`: passed.
- `cargo fmt --all -- --check`: passed after applying `cargo fmt --all`.
- `cargo clippy -p awidat-core --tests -- -D warnings`: passed.

## Completed Slice

Status: completed.

Slice: Durable preview refresh lifecycle.

Why this is next: preview-cache planning and desktop execution now exist, but there is still no durable queue/status artifact that can survive a process boundary or make refresh lifecycle reproducible for agents and the desktop.

Completion gate:

- selected preview-cache refresh plans can be persisted as a project-local lifecycle artifact: done;
- persisted lifecycle records include selected task ids, aggregate work, status, and artifact policy: done;
- focused tests cover writing and reading the lifecycle artifact without generating media: done;
- focused tests, format, and relevant clippy checks pass: done.

Verification:

- `cargo test -p awidat-core preview_cache_status`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p awidat-core --tests -- -D warnings`: passed.

## Completed Slice

Status: completed.

Slice: GPU/backend routing maturity audit.

Why this is next: the remaining roadmap gap is less about adding another metadata field and more about whether backend selection honestly advertises GPU compositor limitations and avoids overclaiming support.

Completion gate:

- inspect current GPU compositor/backend routing tests and capability metadata: done;
- add missing focused tests if routing can overclaim GPU readiness: done;
- keep fallback to FFmpeg explicit when GPU-compatible evidence is absent: done;
- focused tests, format, and relevant clippy checks pass: done.

Verification:

- `cargo test -p awidat-render gpu_transitions`: passed.
- `cargo test -p awidat-render selected_timeline_backend_falls_back_for_mixed_gpu_and_ffmpeg_transitions`: passed.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy -p awidat-render --tests -- -D warnings`: passed.

## Completed Slice

Status: completed.

Slice: Broad verification and remaining production-hardening audit.

Why this is next: the covered and immediately actionable not-yet-started slices now have focused tests. Before claiming the overall goal complete, the repo still needs broad verification and a remaining-gap audit against the original roadmap, especially frame-pixel caption occlusion scoring and resumable preview worker execution.

Completion gate:

- run broad format, clippy, and workspace/package tests: done;
- inspect failures without reverting unrelated work: done, no failures observed;
- update this document with pass/fail evidence and remaining blockers: done;
- complete the remaining-gap audit against the original roadmap: done.

Verification:

- `cargo fmt --all -- --check`: passed (no output).
- `cargo clippy --workspace --all-targets -- -D warnings`: passed (exit 0). Pre-existing `ts-rs failed to parse this attribute` notes still print from the desktop crate's generated type exports — they are notes, not warnings, and do not fail `-D warnings`.
- `cargo test --workspace --no-run`: passed (exit 0).
- `cargo test --workspace`: passed (exit 0). Aggregate: 1838 passed, 0 failed, 8 ignored across 85 test binaries, plus 17 doc-test sections all green.

Remaining-gap audit (against the original roadmap):

- Frame-pixel caption occlusion scoring: still unimplemented in production form. `add_caption_rendered_output_gate` in `crates/core/src/tools/verify_render.rs` evaluates the gate from three sources only: explicit manifest fields (`caption_rendered_output_status/probe_count/safe_area_pass_count/occlusion_fail_count`), a libass-layout sidecar derivation (`libass_layout_supports_caption_rendered_output`), or a hard fail when neither is present. No decoder is invoked, no per-frame caption-box pixel sampling exists, and `caption_rendered_output_probe_count` is treated as caller-supplied or sidecar-derived rather than measured. Closing this gap requires a new scorer that decodes rendered frames, samples caption regions, and reports safe-area / occlusion counts back into the gate.
- Resumable preview worker pool: still unimplemented in production form. `write_preview_cache_refresh_lifecycle` in `crates/core/src/preview_cache.rs` persists a single `.awidat/preview-cache/refresh-plan.json` artifact with hardcoded `status: "planned"` and `artifact_policy: "no_render_job_started"`. There is no status machine, no worker, no progress reporting, and no resume API on top of the artifact. Closing this gap requires lifecycle status transitions, a background worker that consumes selected tasks, and resume semantics for partial progress.

Both gaps are explicitly listed as future work in the "Not yet started or not yet production-complete" table above and are out of scope for this slice; they are the next candidate slices once new implementation work is scheduled.

## Completed Slice

Status: completed.

Slice: Frame-pixel caption rendered-output scorer.

Why this is next: caption rendered-output evidence was still inferred from manifest fields and libass-layout sidecar counts. The original roadmap calls for a real frame-decoding scorer that measures safe-area and occlusion per caption event.

Completion gate:

- new `caption_rendered_output_scorer` module parses Dialogue lines, computes per-event bboxes against PlayRes + style margins + safe-area profile, samples the midpoint frame via a `CaptionFrameSampler` trait, and reports `probe_count`, `safe_area_pass_count`, and `occlusion_fail_count`: done;
- production `FfmpegFrameSampler` backed by a new `awidat_render::extract_frame_raw_gray` helper (raw grayscale, no PNG roundtrip, no new image-decode workspace dep): done;
- `verify_render_output` runs the scorer before its sync gate-builder and injects measured `caption_rendered_output_*` metadata; `add_caption_rendered_output_gate` reads the injected metadata and selects between `frame_pixel_scorer_passed`, `frame_pixel_scorer_failed`, and `frame_pixel_scorer_unavailable_fell_back_to_libass_layout` reasons: done;
- libass-layout sidecar derivation kept as a named fallback when the scorer is unavailable: done;
- `awidat_render::MediaProbe` learns `video_width` / `video_height` from ffprobe so the scorer knows real output dimensions: done;
- `crates/render/src/manifest.rs` records `libass_layout_sidecar_paths` so the scorer can find sidecars from manifest metadata: done;
- capability metadata + the capability_manifest test fixture advertise the new scorer surface: done;
- focused unit tests cover scorer pass, safe-area fail, occlusion fail, no-events, and partial-parse paths: done;
- integration tests cover scorer-pass, scorer-failed, and scorer-unavailable-fallback wiring through `verify_render_output`: done;
- broad workspace verification re-run cleanly after the slice landed: done.

Verification:

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed (exit 0; only pre-existing ts-rs notes from desktop generated exports).
- `cargo test --workspace`: passed. Aggregate: 1847 passed, 0 failed, 8 ignored across 85 test binaries — up from 1838 by the 9 new focused tests added in this slice (1 manifest, 5 scorer, 3 verify_render integration).

## Completed Slice

Status: completed.

Slice: Resumable preview worker pool.

Why this is next: `PreviewCacheRefreshLifecycle` was a planning artifact with hardcoded `status: "planned"`. The original roadmap calls for a running lifecycle with status transitions, background execution, and resume semantics.

Completion gate:

- `PreviewCacheRefreshLifecycle` is now a status machine with per-task `PreviewCacheRefreshTaskState` records (Pending/InProgress/Completed/Failed/Skipped) and aggregate status derivation: done;
- `PreviewRefreshExecutor` trait + `PreviewRefreshError` define the testable executor seam: done;
- production `FfmpegPreviewRefreshExecutor` dispatches per task kind to `awidat_render::transcode_proxy`, `generate_thumbnails`, and `generate_waveform` (writing the waveform sidecar to the desktop's existing schema): done;
- `run_preview_cache_refresh` driver iterates tasks, dispatches each Pending/InProgress one, persists state atomically (`tmp + rename`) between transitions, isolates failures (continues past a failed task), appends new tasks from a re-supplied selection, preserves original timestamps for completed tasks, and refuses to run when a fresh `in_progress` lifecycle is present (soft busy guard): done;
- new mutating tool `run_preview_cache_refresh` exposes the driver to agents with the same selection options as `preview_cache_status`: done;
- capability metadata (capabilities.rs + capability_metadata.rs + capability_manifest test fixture) advertises the new lifecycle + remaining cross-process file-locking gap: done;
- focused tests: 5 lifecycle scenarios (all-success / failure isolation / resume / append on rerun / busy guard), 1 production-executor smoke test (unknown kind), 2 tool schema/metadata tests: done;
- broad workspace verification re-run cleanly: done.

Verification:

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed (exit 0; only pre-existing ts-rs notes from desktop generated exports).
- `cargo test --workspace`: passed. Aggregate: 1855 passed, 0 failed, 8 ignored across 85 test binaries — up from 1847 by the 8 new focused tests added in this slice.

## Next Slice

Status: pending (optional production-hardening).

Slice: Cross-process file locking for the preview-cache refresh lifecycle.

Why this is next: the slice landed an in-process soft busy guard (5-minute window on `started_at_ms`), but a second process can still trample a running lifecycle. The remaining honest production gap is a real file lock (e.g. `fs2::FileExt::try_lock_exclusive` on a sibling `.lock` file) so the desktop, CLI, and agent paths can run safely on the same project.

Completion gate:

- introduce a portable file-lock helper (or adopt `fs2`/`fd-lock`) and wrap the lifecycle write path with try-lock-exclusive;
- treat lock contention as `PreviewRefreshError::Busy` so callers see the same surface as the soft guard;
- focused tests cover the contention case via two driver invocations against the same project root;
- run broad workspace verification.

Outside this slice, the remaining roadmap entry — broader GPU effect coverage — is a larger separate workstream rather than a hardening task and is not tracked under the active goal.

## Running Log

### 2026-05-22

- Created this living plan.
- Current goal remains active and incomplete.
- Completed preview-cache executable refresh status:
  - added shared core `PreviewCacheRefreshExecutionContract`;
  - exposed `refresh_execution` from `preview_cache_status`;
  - exposed `refresh_execution` from `render_preflight` preview-cache output;
  - added desktop `asset_id` filtering for refresh plans.
- Completed caption/subtitle preflight surfacing:
  - added render-preflight `caption_summary`;
  - added render-preflight `caption_layout_readiness`;
  - exposed caption warnings and layout readiness without starting a render.
- Completed rendered-output caption verification:
  - added `caption_rendered_output_readable` gate;
  - made caption renders fail verification when caption output evidence is missing;
  - accepted explicit manifest evidence for probe count, safe-area pass count, occlusion failure count, and status;
  - updated capability metadata to advertise the current limitation.
- Completed backend dispatch and stream-copy/remux closure audit:
  - stream-copy success/fallback focused tests pass;
  - render preflight blocker evidence test passes;
  - `stream_remux` tool manifest/report tests pass.
- Completed automatic caption rendered-output evidence:
  - `caption_rendered_output_readable` can derive passing evidence from libass layout sidecar metadata;
  - explicit caption output evidence remains authoritative;
  - missing caption output evidence still fails.
- Completed durable preview refresh lifecycle:
  - `preview_cache_status` accepts `persist_refresh_plan`;
  - the selected refresh plan persists to `.awidat/preview-cache/refresh-plan.json`;
  - the persisted artifact records selected task ids, aggregate work, lifecycle status, and artifact policy.
- Completed GPU/backend routing maturity audit:
  - raw-stream GPU dispatch now requires all transitions to be GPU-routable;
  - mixed GPU/FFmpeg transitions fall back to FFmpeg reencode with `mixed_gpu_ffmpeg_transitions` evidence;
  - focused GPU routing tests and render clippy pass.
- Completed broad verification and remaining-gap audit:
  - `cargo fmt --all -- --check` clean;
  - `cargo clippy --workspace --all-targets -- -D warnings` clean (only pre-existing ts-rs notes from desktop crate);
  - `cargo test --workspace --no-run` succeeded;
  - `cargo test --workspace` green: 1838 passed, 0 failed, 8 ignored across 85 test binaries, plus 17 doc-test sections;
  - confirmed frame-pixel caption occlusion scoring and resumable preview worker pool remain the only outstanding production gaps.
- Completed frame-pixel caption rendered-output scorer slice:
  - shipped a new `caption_rendered_output_scorer` module + `CaptionFrameSampler` trait + production `FfmpegFrameSampler` + test-only `InMemoryFrameSampler` / `AlwaysUnavailableFrameSampler`;
  - added `awidat_render::extract_frame_raw_gray` (raw grayscale single-frame extractor) and taught `MediaProbe` to expose `video_width` / `video_height`;
  - recorded `libass_layout_sidecar_paths` in the render manifest so the scorer can locate ASS sidecars from manifest metadata;
  - `verify_render_output` now runs the scorer before its sync gate-builder and injects measured `caption_rendered_output_*` metadata; the gate reason set expands to `frame_pixel_scorer_passed`, `frame_pixel_scorer_failed`, and `frame_pixel_scorer_unavailable_fell_back_to_libass_layout`;
  - capability notes (capabilities.rs + capability_metadata.rs + capability_manifest test) advertise the new scorer surface;
  - broad verification: `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace` green at 1847 passed / 0 failed / 8 ignored across 85 test binaries.

### 2026-05-23

- Completed resumable preview worker pool slice:
  - extended `PreviewCacheRefreshLifecycle` with per-task `PreviewCacheRefreshTaskState` records, status-machine aggregate, and atomic write-via-rename persistence;
  - shipped a `PreviewRefreshExecutor` trait, `PreviewRefreshError`, and `run_preview_cache_refresh` driver with resume semantics, failure isolation, soft busy guard, and append-on-rerun behavior;
  - added a production `FfmpegPreviewRefreshExecutor` that dispatches per task kind to `awidat_render::transcode_proxy`, `generate_thumbnails`, and `generate_waveform` (writing waveform sidecars in the desktop's existing schema);
  - exposed the driver to agents via a new mutating `run_preview_cache_refresh` tool that mirrors the `preview_cache_status` selection options;
  - capability metadata (capabilities.rs + capability_metadata.rs + capability_manifest test) advertises the new lifecycle and the remaining cross-process file-locking gap;
  - broad verification: `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace` green at 1855 passed / 0 failed / 8 ignored across 85 test binaries.
- Next action: optional production hardening — replace the soft busy guard with a real file lock so desktop / CLI / agent paths can run safely on the same project.
