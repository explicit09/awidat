# Agent Live-Preview Stage — Phase 1 Implementation Plan (Stage extraction + verification harness)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the preview overlay stack into a `Stage` compositor and build the deterministic
harness + gates that let an autonomous loop judge its own preview changes.

**Architecture:** Pure-move refactor of `SegmentedVideoView.tsx` overlay layers into
`apps/desktop/src/media/stage/`, then a dev-only harness route that mounts `Stage` with a frozen
clock, screenshot-gated by Playwright + ffmpeg SSIM, plus shared animation test vectors
(TS ↔ Rust). Spec: `docs/superpowers/specs/2026-07-07-agent-live-preview-stage-design.md`.

**Tech Stack:** React 19 + TypeScript + Vite (existing), node:assert tests via
`node --experimental-strip-types` (existing style), Playwright (already a devDependency,
pattern in `apps/desktop/tests/desktop-ui-smoke.mjs`), ffmpeg for SSIM compare.

## Global Constraints

- Cross-platform: no macOS-only APIs; harness must run in plain Chromium (no Tauri).
- All cargo commands go through `scripts/loop-cargo.sh` (Task 1) —
  `CARGO_TARGET_DIR="/Volumes/My Passport for Mac/awidat-build/target"`, halt if unmounted.
  The Passport drive drops under sustained write load: scope cargo to single crates (`-p`),
  never `--workspace` builds inside the loop.
- TS tests follow the existing node:assert style (`node --experimental-strip-types tests/x.test.ts`),
  registered as `test:<name>` in `apps/desktop/package.json` AND chained into the `test` script
  (CI only runs the chain — define ≠ run).
- Refactor tasks are **pure moves**: no behavior change, no renames beyond module paths.
- Commit after every green task; `git diff --check` before each commit.
- Repo hygiene at task end when Rust touched: `cargo fmt --check` + scoped clippy.

## Loop Protocol (how this file is consumed)

Each iteration: pick the top task with an unchecked box → do its steps in order → run its
**Verify** command(s) → all green: check the boxes, commit, next task. Any step red: fix and
re-verify. Same task red after 3 distinct fix attempts across iterations: STOP the loop and
surface the failure. Task 10 is a mandatory STOP.

---

### Task 1: Loop infrastructure — `scripts/loop-cargo.sh` + verify entrypoint

**Files:**
- Create: `scripts/loop-cargo.sh`
- Create: `scripts/stage-verify.sh`

**Interfaces:**
- Produces: `scripts/loop-cargo.sh <cargo-args…>` (runs cargo with external target dir, mount-guarded);
  `scripts/stage-verify.sh` (Phase-1 cumulative gate: desktop `npm test`; later tasks append).

- [ ] **Step 1: Write `scripts/loop-cargo.sh`**

```bash
#!/usr/bin/env bash
# Cargo wrapper for the autonomous loop. The internal disk cannot hold build
# artifacts; the external drive can, but drops under sustained write load —
# so: mount-guard before and after, and never run unscoped workspace builds.
set -euo pipefail
MOUNT_POINT="/Volumes/My Passport for Mac"
if [ ! -d "$MOUNT_POINT" ]; then
  echo "loop-cargo: Passport drive not mounted — halting (do NOT build on internal disk)" >&2
  exit 86
fi
EXT_TARGET="$MOUNT_POINT/awidat-build/target"
mkdir -p "$EXT_TARGET"
status=0
CARGO_TARGET_DIR="$EXT_TARGET" cargo "$@" || status=$?
if [ ! -d "$MOUNT_POINT" ]; then
  echo "loop-cargo: drive dropped during build — artifacts suspect, retry once from scratch" >&2
  exit 87
fi
exit $status
```

- [ ] **Step 2: Write `scripts/stage-verify.sh`**

```bash
#!/usr/bin/env bash
# Cumulative Phase-1 verification gate. Later tasks append lines; the loop
# runs this before every commit that claims a task done.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
( cd apps/desktop && npm test )
```

- [ ] **Step 3: Make both executable, run stage-verify to confirm green baseline**

Run: `chmod +x scripts/loop-cargo.sh scripts/stage-verify.sh && ./scripts/stage-verify.sh; echo EXIT=$?`
Expected: `EXIT=0` (baseline: 22 suites pass as of plan time)

- [ ] **Step 4: Commit**

```bash
git add scripts/loop-cargo.sh scripts/stage-verify.sh
git commit -m "loop: cargo mount-guard wrapper + cumulative stage verify gate"
```

---

### Task 2: `StageClock` — narrow clock interface with frozen mode

**Files:**
- Create: `apps/desktop/src/media/stage/stageClock.ts`
- Test: `apps/desktop/tests/stage-clock.test.ts`
- Modify: `apps/desktop/package.json` (add `test:stage-clock`, chain into `test`)

**Interfaces:**
- Produces:
  ```ts
  export type StageClock = {
    now(): number;            // timeline seconds
    isPlaying(): boolean;
    rate(): number;
  };
  export function frozenClock(t: number): StageClock;
  export function livePreviewClock(src: {
    now(): number; isPlaying(): boolean; rate(): number;
  }): StageClock;
  ```
  `frozenClock(t)` always reports `now() === t`, `isPlaying() === false`, `rate() === 0` —
  this is what the harness mounts. `livePreviewClock` adapts the existing `PreviewClock`
  accessors in `SegmentedVideoView.tsx` (see `previewClockNow`, lines ~156-190).

- [ ] **Step 1: Write the failing test** (`apps/desktop/tests/stage-clock.test.ts`)

```ts
import { strict as assert } from "node:assert";
import { frozenClock, livePreviewClock } from "../src/media/stage/stageClock.ts";

const f = frozenClock(12.5);
assert.equal(f.now(), 12.5);
assert.equal(f.isPlaying(), false);
assert.equal(f.rate(), 0);

let t = 3;
const live = livePreviewClock({ now: () => t, isPlaying: () => true, rate: () => 1.5 });
assert.equal(live.now(), 3);
t = 4;
assert.equal(live.now(), 4);
assert.equal(live.isPlaying(), true);
assert.equal(live.rate(), 1.5);

console.log("stage-clock: OK");
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd apps/desktop && node --experimental-strip-types tests/stage-clock.test.ts`
Expected: FAIL (module not found)

- [ ] **Step 3: Implement `stageClock.ts`** (types + two constructors exactly as in Interfaces; `livePreviewClock` returns the accessors pass-through)

- [ ] **Step 4: Register + chain script, run test**

In `apps/desktop/package.json`: add `"test:stage-clock": "node --experimental-strip-types tests/stage-clock.test.ts"`, append `&& npm run test:stage-clock` to the `test` chain (before the smoke test entry).
Run: `cd apps/desktop && npm run test:stage-clock`
Expected: `stage-clock: OK`

- [ ] **Step 5: Verify + commit**

Run: `./scripts/stage-verify.sh`
```bash
git add apps/desktop/src/media/stage/stageClock.ts apps/desktop/tests/stage-clock.test.ts apps/desktop/package.json
git commit -m "stage: StageClock interface with frozen mode for the harness"
```

---

### Task 3: Extract transition layers to `stage/transitions.tsx` (pure move)

**Files:**
- Create: `apps/desktop/src/media/stage/transitions.tsx`
- Modify: `apps/desktop/src/media/SegmentedVideoView.tsx`

**Interfaces:**
- Produces (moved, exported): `TimelineTransitionOverlay`, `TimelineTransitionColorOverlay`,
  `GpuTransitionPreview` and every helper they close over — currently
  `SegmentedVideoView.tsx:1058-1361` (`transitionProgress`, `baseTransitionOpacity`,
  `transitionOpacity`, `transitionVisualStyle`, `transitionSideProgress`,
  `isDissolveTransition` … `isPixelize`). Props unchanged.
- Consumes: `useGpuTransitionPreview` import moves with `GpuTransitionPreview`.

- [ ] **Step 1: Move lines 1058-1361 verbatim into `stage/transitions.tsx`; export the three components; move the imports they need; import them back into `SegmentedVideoView.tsx`**

- [ ] **Step 2: Typecheck + full verify**

Run: `cd apps/desktop && npx tsc --noEmit` then, from repo root, `./scripts/stage-verify.sh`
Expected: clean tsc, all suites green

- [ ] **Step 3: Commit**

```bash
git add apps/desktop/src/media/stage/transitions.tsx apps/desktop/src/media/SegmentedVideoView.tsx
git commit -m "stage: extract transition overlay layers (pure move)"
```

---

### Task 4: Extract titles + motion-scene layers to `stage/titles.tsx` and `stage/motionScene.tsx` (pure move)

**Files:**
- Create: `apps/desktop/src/media/stage/titles.tsx`
- Create: `apps/desktop/src/media/stage/motionScene.tsx`
- Modify: `apps/desktop/src/media/SegmentedVideoView.tsx`

**Interfaces:**
- `titles.tsx` exports: `TimelineTitleOverlays`, `activeTitleOverlays`, `titleOverlayBox`,
  `titleAlign`, type `PreviewTitleOverlay` (currently `SegmentedVideoView.tsx:1921-1946,
  1989-2061, 2153-2181` plus `titleOverlayStyle`/`titleRevealText` helpers ~2287+).
- `motionScene.tsx` exports: `TimelineMotionSceneOverlays`, `activeMotionSceneOverlays`,
  `motionShapeOverlayStyle`, `motionImageOverlayStyle`, types `PreviewMotionShapeOverlay`,
  `PreviewMotionImageOverlay`, `PreviewMotionSceneOverlay` (currently `1948-1987, 2063-2145,
  2183-2279`).
- Shared helper `projectAssetUrl` (line 1914) moves to `motionScene.tsx` and is re-exported
  if titles/broadcast need it.

- [ ] **Step 1: Move title pieces to `stage/titles.tsx`, motion-scene pieces to `stage/motionScene.tsx`; fix imports both directions**
- [ ] **Step 2: Typecheck + full verify** — Run: `cd apps/desktop && npx tsc --noEmit` then `./scripts/stage-verify.sh` from root. Expected: green.
- [ ] **Step 3: Commit** — `git commit -m "stage: extract title and motion-scene layers (pure move)"`

---

### Task 5: Extract video overlays + broadcast chrome to `stage/videoOverlays.tsx` and `stage/broadcast.tsx` (pure move)

**Files:**
- Create: `apps/desktop/src/media/stage/videoOverlays.tsx` (`TimelineVideoOverlays`,
  `TimelineVideoOverlay`, `videoOverlayStyle` — lines 1363-1474)
- Create: `apps/desktop/src/media/stage/broadcast.tsx` (`TimelineBroadcastOverlay` and every
  `Broadcast*` component + helpers — lines 1476-1919, keep the existing `export` on
  `TimelineBroadcastOverlay`; update its external importers)
- Modify: `apps/desktop/src/media/SegmentedVideoView.tsx`

- [ ] **Step 1: Move both groups; `grep -rn "TimelineBroadcastOverlay" apps/desktop/src` and update all import sites**
- [ ] **Step 2: Typecheck + full verify** — expected green.
- [ ] **Step 3: Commit** — `git commit -m "stage: extract video-overlay and broadcast layers (pure move)"`

---

### Task 6: `Stage.tsx` — compose the program-frame layer stack

**Files:**
- Create: `apps/desktop/src/media/stage/Stage.tsx`
- Modify: `apps/desktop/src/media/SegmentedVideoView.tsx` (program-frame block, lines ~972-1009)

**Interfaces:**
- Produces:
  ```tsx
  export type StageProps = {
    clock: StageClock;
    programFrameCss: CSSProperties;
    programFrameSize: { width: number; height: number };
    projectRoot: string | null;
    videoOverlays: VideoOverlaySegment[];
    transition: PreviewTransition | null;
    renderTransitionOnGpu: boolean;
    titles: PreviewTitleOverlay[];
    motionSceneLayers: PreviewMotionSceneOverlay[];
    broadcastOverlay: TimelineSnapshot["broadcast_overlay"];
    showGap: boolean;
  };
  export function Stage(props: StageProps): JSX.Element;
  ```
  `Stage` renders the exact `.timeline-program-frame` stack currently inline in
  `SegmentedPlayer` (video overlays → gap → transition → transition-color → gpu-transition →
  titles → motion scenes → broadcast), deriving `timelineTime`/`isPlaying` from `clock`.
- Consumes: everything Tasks 3-5 exported; `StageClock` from Task 2.

- [ ] **Step 1: Write `Stage.tsx`; replace the inline block in `SegmentedPlayer` with `<Stage …/>` passing `livePreviewClock(...)`**
- [ ] **Step 2: Typecheck + full verify + smoke** — `npx tsc --noEmit`, `./scripts/stage-verify.sh` (chain ends in `tests/desktop-ui-smoke.mjs`, which exercises the real page). Expected green.
- [ ] **Step 3: Commit** — `git commit -m "stage: Stage compositor owns the program-frame layer stack"`

---

### Task 7: Fixture clip + stage harness route

**Files:**
- Create: `apps/desktop/public/fixtures/stage/clip.mp4` (checked in, ≤2 MB)
- Create: `apps/desktop/public/fixtures/stage/scene-basic.json`
- Create: `apps/desktop/src/media/stage/StageHarness.tsx`
- Modify: `apps/desktop/src/App.tsx` (mount harness when `window.location.pathname === "/stage-harness"`, same pattern as the `/design/concept` check at `App.tsx:341`)

**Steps:**

- [ ] **Step 1: Cut the fixture clip.** Editorial corpus proxies live on the Passport drive; if mounted, cut 3 s: `ffmpeg -ss <t> -t 3 -i <corpus-proxy> -vf scale=1280:720 -c:v libx264 -g 1 -crf 28 -an apps/desktop/public/fixtures/stage/clip.mp4`. If NOT mounted, fall back to deterministic synthetic: `ffmpeg -f lavfi -i "testsrc2=size=1280x720:rate=30:duration=3" -c:v libx264 -g 1 -crf 28 -an clip.mp4` and note in the commit that a corpus clip should replace it. Confirm size `< 2MB`.

- [ ] **Step 2: Write `scene-basic.json`** — one title, one rect, one image layer with keyframed opacity/position exercising today's vocabulary (shape mirrors the `PreviewTitleOverlay` / `PreviewMotionShapeOverlay` / `PreviewMotionImageOverlay` types from Task 4).

- [ ] **Step 3: Write `StageHarness.tsx`.** Reads `?t=<seconds>&scene=<url>` from `location.search`; renders a fixed 1280×720 `.timeline-program-frame` containing a paused `<video src="/fixtures/stage/clip.mp4">` seeked to `t`, and `<Stage clock={frozenClock(t)} …/>` with overlay models parsed from the scene JSON; sets `document.title = "stage-harness-ready"` only after the video `seeked` event AND fonts ready (`document.fonts.ready`) — Playwright waits on the title.

- [ ] **Step 4: Manual check** — `cd apps/desktop && npx vite --port 1420` then fetch `http://127.0.0.1:1420/stage-harness?t=1.0&scene=/fixtures/stage/scene-basic.json` — expect harness DOM (verify via curl for 200 + Playwright in Task 8).

- [ ] **Step 5: Verify + commit** — `./scripts/stage-verify.sh`; `git commit -m "stage: deterministic harness route with checked-in fixture clip"`

---

### Task 8: Harness screenshot gate (Playwright + ffmpeg SSIM)

**Files:**
- Create: `apps/desktop/tests/stage-harness.mjs` (follow the server-boot pattern of `tests/desktop-ui-smoke.mjs:27-58`)
- Create: `apps/desktop/tests/fixtures/stage-golden/` (goldens, per-platform filename suffix: `scene-basic-t1.0-darwin.png`)
- Create: `scripts/ssim-compare.sh`
- Modify: `apps/desktop/package.json` (`test:stage-harness`, chained), `scripts/stage-verify.sh` (append harness gate)

**Steps:**

- [ ] **Step 1: Write `scripts/ssim-compare.sh`**

```bash
#!/usr/bin/env bash
# ssim-compare.sh <a.png> <b.png> <min-ssim>  — exits 1 if SSIM(All) < min
set -euo pipefail
score=$(ffmpeg -hide_banner -i "$1" -i "$2" -lavfi ssim -f null - 2>&1 \
  | grep -oE "All:[0-9.]+" | cut -d: -f2)
echo "SSIM=$score (min $3)"
awk -v s="$score" -v m="$3" 'BEGIN { exit (s+0 >= m+0) ? 0 : 1 }'
```

- [ ] **Step 2: Write `tests/stage-harness.mjs`** — boot/reuse dev server (smoke-test pattern), `chromium.launch()`, viewport 1280×720, `deviceScaleFactor: 1`, load `/stage-harness?t=1.0&scene=/fixtures/stage/scene-basic.json`, wait for title `stage-harness-ready`, screenshot to `tests/smoke/stage-harness-t1.0.png`; assert expected overlay DOM (title text node, one shape div, one img) via selectors; if a golden for this platform exists, run `scripts/ssim-compare.sh <shot> <golden> 0.98`; if absent, write the shot AS the golden and print `golden bootstrapped` (first run self-seeds).

- [ ] **Step 3: Run twice** — first run bootstraps golden, second must pass compare. Expected: `SSIM=1.000000` on identical config.

- [ ] **Step 4: Chain it** — package.json `test:stage-harness` + append `&& npm run test:stage-harness` to `test`; append to `scripts/stage-verify.sh`.

- [ ] **Step 5: Verify + commit** — `./scripts/stage-verify.sh`; commit goldens too: `git commit -m "stage: harness screenshot gate with SSIM compare"`

---

### Task 9: Shared animation test vectors (TS ↔ Rust)

**Files:**
- Create: `crates/eval/fixtures/animation-vectors.json`
- Create: `apps/desktop/tests/animation-vectors.test.ts` (+ `test:animation-vectors`, chained)
- Create: `crates/eval/tests/animation_vectors.rs`
- Modify: `scripts/stage-verify.sh` (append the TS side; Rust side runs via loop-cargo)

**Steps:**

- [ ] **Step 1: Generate vectors FROM the TS evaluator** — script evaluates `evaluateAnimations` (`apps/desktop/src/timeline/animation.ts`) across the parameter list at `animation.ts:30-49`: for each of ~20 cases (param, keyframes incl. bezier + spring + extrapolation, sample times) record `{param, keyframes, t, expected}` to `crates/eval/fixtures/animation-vectors.json` with 1e-6 precision.
- [ ] **Step 2: TS test** replays every vector through `evaluateAnimations`, asserts `|actual-expected| < 1e-6`. Run; expected trivially green (self-generated) — this pins today's behavior.
- [ ] **Step 3: Rust test** in `crates/eval/tests/animation_vectors.rs` parses the same JSON and replays through the Rust evaluator (`crates/render/src/animation.rs`; add `montage-render` as dev-dependency of `crates/eval` if absent). Assert within 1e-4. Run: `./scripts/loop-cargo.sh test -p montage-eval --test animation_vectors`. Expected: PASS — **any mismatch is a real preview/export divergence: fix the RUST side only if it's provably wrong, otherwise record the divergence in the task notes and STOP for review (do not silently loosen tolerances).**
- [ ] **Step 4: Hygiene** — `./scripts/loop-cargo.sh fmt --check` + `./scripts/loop-cargo.sh clippy -p montage-eval`; chain TS side; verify; commit `"eval: shared animation vectors pin TS↔Rust evaluator parity"`.

---

### Task 10: Phase-1 review stop (MANDATORY HALT)

- [ ] **Step 1: Assemble evidence** — harness screenshot(s), `stage-verify` output, animation-vector results, `SegmentedVideoView.tsx` line-count before/after, commit list.
- [ ] **Step 2: STOP the loop and present evidence to the human.** Phase 2 (IR + templates + preview renderer) gets planned only after this review, on top of the extracted architecture.
