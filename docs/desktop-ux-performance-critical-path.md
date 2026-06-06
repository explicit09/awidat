# Desktop UX Performance Critical Path

Awidat desktop should feel ready before all project data is hydrated. The
performance contract is:

- Paint the editor shell first.
- Load the minimum project, timeline, and media state needed to show preview.
- Defer history, generated media, readiness dashboards, skill/indexer config,
  waveform/thumbnail decoration, publishing state, and other secondary panels.
- Keep stage and tool switching local to React state; do not make navigation
  wait on disk, media, subprocess, or network work.

## Startup and Open Project

Backend project open (`set_project_root`) validates the project sentinel,
switches state, tears down the old Codex session, starts the generated-media
watcher, clears the media server cache, updates recents, allows project asset
directories, and starts proxy/sidecar backfill in the background.

Frontend startup now lets `useAppGlue` perform the project-root refresh only.
The project-change effect owns the first timeline and media refresh because
those are preview-critical. The previous extra boot-level timeline/media
refresh and delayed retry were removed so the preview path is not competing
with duplicate reads.

## Preview Readiness

The first visible preview depends on:

- `current_project_root` / `recent_projects`
- `read_timeline`
- `list_source_media` and `list_proxies`
- `media_url_for_path` for the selected preview media
- timeline playable paths consumed by `SegmentedVideoView`

These remain eager. If a timeline has no playable clip yet, proxy backfill
continues in the background and the preview reports that it is generating.

## Deferred Hydration

Non-critical hydration is scheduled through
`deferNonCriticalHydration()` in `apps/desktop/src/app/startupHydration.ts`.
It gives the first shell/preview/menu interaction window a 500 ms head start,
then waits for `requestIdleCallback` when available, with a timeout so work
still progresses. Effects cancel their deferred work on project switch so stale
data cannot land in the next project.

Deferred work includes:

- upload preference hydration
- generated media registry refresh
- initial chat history and chat session list refresh
- synthetic intro agent turn
- transcript sidecar reads until a transcript surface is visible
- thumbnail frame and waveform decoration reads
- indexer config reads
- permission-mode chip hydration
- index readiness, episode summary, media readiness, and running-job polling
- scheduler publishing account hydration
- skills catalog and user skills folder hydration on first Skills tab mount

## Menu and Panel Switching

`StageShell` keeps the preview stage mounted and overlays destination sheets.
Switching stage/tool state is local React state with stable performance hooks
on dock buttons. The stage and timeline stay mounted while destination sheets
appear, and destination cleanup happens after the Stage has painted again.
Heavy destination contents should either mount only when selected or hydrate
their data through the deferred path above.

## Verification

Focused regression coverage:

```bash
cd apps/desktop
npm run test:startup-hydration
npm run test:perf-budget
pnpm exec tsc --noEmit
```

Broader checks for this area:

```bash
cd apps/desktop
npm run test:play-segments
npm run test
pnpm exec vite build
```
