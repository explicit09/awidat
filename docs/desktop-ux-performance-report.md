# Desktop UX Performance Report

Generated: 2026-06-06

This pass measured the Tauri desktop renderer through the local React harness
with mocked Tauri IPC and a real MP4 preview fixture. Native process start to
renderer-ready still needs a native WebView/WebDriver trace; this harness
records that field as not measured.

## Baseline vs Final

| Metric | Target | Baseline | Final | Status |
| --- | ---: | ---: | ---: | --- |
| Process start to renderer ready | TBD | not measured | not measured | native trace needed |
| Renderer ready to shell interactive | n/a | 280 ms | 269 ms | measured |
| Warm startup to usable shell | <= 2500 ms | 280 ms | 269 ms | pass |
| Cold startup to usable shell | <= 4000 ms | 1316 ms | 1301 ms | pass |
| Open fixture project to interactive timeline | <= 2000 ms | 311 ms | 291 ms | pass |
| Open fixture project to first preview frame | <= 1500 ms | 550 ms | 503 ms | pass |
| Preview engine initialization | n/a | not measured | 247.1 ms | measured |
| Menu/tab switch p95 | <= 120 ms | 385.8 ms | 5.9 ms | pass |
| Max startup/open long task | <= 50 ms | 59 ms | 0 ms | pass |

## Critical Path Findings

- Startup shell was already fast, but noncritical hydration was competing with
  first preview and menu switching.
- The first preview path now stays focused on project root, timeline, media
  list/proxy list, and `media_url_for_path`.
- The synthetic intro agent turn and transcript sidecar read were entering the
  open-project critical path; both are now deferred until after the first
  interaction window or until a transcript surface is visible.
- Destination menu switching was visually slow because the benchmark was
  timing text-inferred panel transitions and the shell had extra stage
  animation work. The shell now exposes stable switch state hooks, keeps the
  Stage mounted, avoids destination-switch animation work, and cleans up hidden
  sheets after the Stage paints.

## Commands Run

```bash
cd apps/desktop
npm run test:perf-full
npm run test:perf-budget
npm run test:startup-hydration
pnpm exec tsc --noEmit
npm test
pnpm exec vite build
git diff --check
```

## Files Changed

- `apps/desktop/src/app/startupHydration.ts`
- `apps/desktop/src/App.tsx`
- `apps/desktop/src/app/scheduler/SchedulerWorkspace.tsx`
- `apps/desktop/src/shell/SkillsSurface.tsx`
- `apps/desktop/src/shell/StageShell.tsx`
- `apps/desktop/src/state/appGlue.ts`
- `apps/desktop/src/transcript/TranscriptView.tsx`
- `apps/desktop/src/transcript/store.ts`
- `apps/desktop/tests/fixtures/perf-preview.mp4`
- `apps/desktop/tests/perf-budget.mjs`
- `apps/desktop/tests/perf-full.mjs`
- `apps/desktop/tests/startup-hydration.test.ts`
- `apps/desktop/tests/ui-harness.html`
- `docs/desktop-ux-performance-critical-path.md`
- `docs/desktop-ux-performance-report.md`

## Risks and Remaining Bottlenecks

- Native process start to renderer-ready is still unmeasured; add a native
  desktop trace before claiming full process startup coverage.
- The final benchmark is a renderer harness, not an installed app run. It is
  good for regression budgets and critical-path IPC shape, but native WebView
  startup can still differ.
- `read_timeline` still appears twice on the mocked open-project path. It no
  longer causes budget misses, but it is the next cleanup target if startup
  work regresses.
- Background deferred work still needs project-switch cancellation discipline;
  new startup effects should use `deferNonCriticalHydration()` unless they are
  required for shell, timeline, or first preview correctness.
