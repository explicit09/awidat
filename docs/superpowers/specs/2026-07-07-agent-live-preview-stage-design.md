# Agent Live-Preview Stage — Design

**Date:** 2026-07-07
**Status:** Approved direction (Approach C), implementation via autonomous loop
**Branch:** `worktree-agent-live-preview` (based on `eval-gates`)

## Problem

The preview frame (`apps/desktop/src/media/SegmentedVideoView.tsx`, ~2,500 lines) must render
AI-agent edits — text, animated graphics, overlays — in real time. Four gaps:

1. **Narrow graphics vocabulary.** MotionScene supports only text, rect, and image layers with
   basic keyframes. No groups, no enter/exit presets, no springs-as-vocabulary, no templates
   (lower-thirds, kinetic type, callouts).
2. **Preview/export drift.** Preview composites overlays with DOM/CSS; export lowers the same
   timeline to an FFmpeg filtergraph (`crates/render/src/timeline.rs`). Nothing enforces that
   they agree.
3. **Refresh granularity.** Preview refreshes only when a mutating tool *completes*
   (`montage://timeline-changed` → `read_timeline`). No streaming/optimistic updates while the
   agent works.
4. **Frame architecture.** `SegmentedVideoView.tsx` mixes clock, double-buffered video slots,
   and seven overlay layer types in one file; every addition raises the cost of the next.

## Constraints

- **Cross-platform: macOS + Windows.** All choices must work in both WKWebView and WebView2,
  and the export path must not grow platform-specific dependencies. No headless Chromium in
  the render pipeline. No GPU-texture sharing with the webview (no common path across the two
  webviews today).
- Follow the grain of existing decisions: `docs/branded-motion-graphics-plan.md` and
  `docs/motion-scene-remotion-backend.md` chose "one MotionScene document, multiple backends"
  and deferred Remotion.
- Preview must keep playing smoothly during agent edits (double-buffer + PreviewClock stay).

## Decision: Approach C — one document, two lowerings, parity-gated

The **MotionScene IR is the contract**. Two independent lowerings:

- **Preview:** IR → DOM/CSS layers inside a new `Stage` compositor (extracted from
  `SegmentedVideoView`). Animations evaluated analytically at time *t* by the existing
  evaluator (`apps/desktop/src/timeline/animation.ts`), extended as the IR grows.
- **Export:** IR → FFmpeg filters / ASS subtitles in `crates/render/src/timeline.rs`
  (drawtext/libass, drawbox, overlay), as today.

Drift is controlled **empirically, not architecturally**: an SSIM parity gate renders the same
document at the same timestamp through both paths and fails when they diverge past threshold.
This reuses the picture-gates + golden-fixture machinery from `crates/eval`.

Rejected alternatives:
- **Web-first (Remotion / headless browser as the one true renderer):** perfect parity by
  construction, but drags Chromium into the Windows+macOS export pipeline and contradicts the
  recorded deferral decision.
- **Native-GPU-first (render-gpu wgpu as single renderer):** right possible end-state, but
  real-time preview requires streaming GPU frames into the webview with no cross-platform
  transport; vocabulary growth gated on Rust+WGSL. The IR contract keeps this door open.

## Architecture

### MotionScene IR (extended)

Additions to the existing MotionScene document (proto in `crates/proto`, TS types generated
via ts-rs into `apps/desktop/src/protocol/generated/`):

- **Groups**: layers nest under a group with its own transform/opacity; children animate
  relative to the group.
- **Enter/exit presets**: named reveal/dismiss animations (`fade`, `slide-<dir>`, `pop`,
  `typewriter`, `wipe`) with duration + easing, compiled to keyframes at plan time so both
  lowerings consume plain keyframes.
- **Easing/spring library**: named curves (standard cubic-beziers + spring presets) referenced
  by id; both evaluators implement them from one spec.
- **Templates**: agent-facing compound builders — `lower_third`, `kinetic_text`, `callout`,
  `highlight_box`, `progress_bar` — that expand into groups+layers+presets inside the planning
  tool (`plan_motion_scene`), so renderers only ever see core primitives.

Templates expand at plan time; the IR stays small. Renderers implement primitives only.

### Stage compositor (preview)

Extract from `SegmentedVideoView.tsx` into `apps/desktop/src/media/stage/`:

- `Stage.tsx` — owns the layer stack; takes `{clock, snapshot-derived overlay models, videoSlot}`.
- One module per layer: video slots, video overlays (PiP), transitions, titles, motion scenes,
  broadcast chrome, grade canvas. Each layer is a pure function of `(model, t)`.
- `StageClock` — the existing `PreviewClock` behind a narrow interface, plus a **frozen mode**
  (fixed *t*) for the harness.
- `SegmentedVideoView` shrinks to: clock + video slot management + `<Stage …/>`.

No behavior change in the refactor phase; existing overlay rendering moves, not rewrites.

### Verification harness (what makes the loop self-judging)

- **Stage harness page**: dev-only Vite route (`/#/stage-harness`) that mounts `Stage` in a
  plain browser — no Tauri. Inputs via query/JSON fixture: fixture video file (served
  statically by the dev server), MotionScene document, frozen clock at *t*, fixed viewport
  (1280×720), animations off free-run. Deterministic screenshot target for Playwright on both
  OSes.
- **Parity gate**: for each gate case (doc, fixture clip, timestamps): render frame via export
  path (FFmpeg) → PNG; screenshot harness at same *t* → PNG; SSIM-compare with per-case
  thresholds; region masks allow excluding video content when judging overlay-only cases.
  Implemented in `crates/eval` alongside the existing picture gates; invocable as one command
  (`cargo run -p montage-eval -- stage-parity …` or a make target).
- **Evaluator parity vectors**: one JSON file of animation test vectors
  (`crates/eval/fixtures/animation-vectors.json`: params, keyframes, easings, t → expected)
  consumed by Rust evaluator tests and TS `animation.ts` tests (node:assert, matching the
  existing TS test style).
- **Fixture clips**: 2–5 s cuts from the editorial-study corpus proxies, checked in small
  (short, 720p, high-keyframe) under `crates/eval/fixtures/media/`.

### Streaming/optimistic updates (phase 4)

Emit overlay-delta events while a mutating tool is still in flight: codex-bridge already sees
tool call arguments; for `plan_motion_scene`/`apply_edl` ops that only add/modify overlays,
emit `montage://overlay-preview` with the proposed layers. The Stage renders them in a
"pending" layer (subtle affordance), reconciled/replaced when `timeline-changed` lands.
Full `read_timeline` remains the source of truth; optimistic layers are additive and dropped
on reconcile. This keeps the existing correctness path untouched.

## Implementation phases (loop task queue order)

1. **Phase 1 — Stage extraction + harness.** Refactor to `stage/`; harness page; Playwright
   screenshot smoke test; evaluator vector file bootstrapped from current `animation.ts`
   behavior. Gate for this phase: harness screenshots match pre-refactor goldens (pixel/SSIM),
   all existing TS + Rust tests pass.
2. **Phase 2 — IR + preview renderer.** Groups, presets, easing/spring library, templates in
   `plan_motion_scene`; preview rendering for all of it; agent-visible tool docs updated.
   Gate: harness goldens for each template + evaluator vectors green. (Demo win lives here.)
3. **Phase 3 — Export lowering + parity gates.** FFmpeg/ASS lowering for the new IR pieces;
   stage-parity gate cases for every template and primitive; thresholds tuned; gates wired
   into the standard test chain (`test:*` scripts chained into `test`, per CI convention).
4. **Phase 4 — Streaming updates.** Optimistic overlay events end-to-end; latency measurement
   (tool-start → first paint) recorded by the harness.

Each phase ends with a **stop-for-review task** — the loop halts and surfaces evidence
(screenshots, gate results, latency numbers) for human approval before the next phase.

## Loop mechanics

- Runs in worktree `worktree-agent-live-preview`.
- Task queue: `docs/superpowers/plans/2026-07-07-agent-live-preview-stage-plan.md` — checklist;
  every task carries its own verification command(s).
- Per iteration: pick top unchecked task → implement → run that task's verification → fix
  until green → run repo hygiene (`cargo fmt --check`, clippy on touched crates,
  `git diff --check`) → commit → check off → next.
- Stop conditions: queue empty; phase-boundary review task reached; or the same task fails
  verification 3 consecutive iterations (halt and surface, don't thrash).
- Self-improvement requirement: a task is only "done" when its gate passes; gates accumulate
  (later tasks re-run earlier phases' gates), so regressions fail loudly.

## Error handling

- Harness nondeterminism (font rendering, colorspace differences across OSes): per-case SSIM
  thresholds + region masks rather than exact pixel equality; goldens are per-platform if
  cross-OS deltas exceed threshold.
- FFmpeg/Playwright missing on a machine: verification commands probe and fail with an
  actionable message; the loop surfaces this instead of retrying.
- Disk pressure (known ENOSPC risk): all cargo builds in the loop set
  `CARGO_TARGET_DIR="/Volumes/My Passport for Mac/awidat-build/target"` (external HFS+ drive;
  the main checkout's `target/` is already a symlink there). If the drive is unmounted the
  loop halts and surfaces rather than falling back to the internal disk.

## Non-goals

- No Remotion dependency, no headless-browser rendering in export.
- No GPU-compositor preview transport (render-gpu stays for transitions/export raw-stream).
- No changes to audio pipeline, color pipeline, or the media stream server.
- No Windows CI setup in this effort (design keeps Windows compatible; CI wiring is separate).
