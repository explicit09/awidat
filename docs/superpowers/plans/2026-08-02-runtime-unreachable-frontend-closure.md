# Runtime-Unreachable Frontend Closure

## Goal

Remove the remaining frontend modules that cannot be reached from any shipped
desktop entry point, together with tests and a dependency that exist only for
that dead code.

## Evidence boundary

- A TypeScript-resolved import graph starts at `main.tsx`, `gallery.tsx`,
  `glassShowcase.tsx`, `shellPreview.tsx`, `studioMockup.tsx`, and
  `broadcast/render-main.tsx`.
- The graph reaches 309 of 326 non-test TypeScript modules. The other 17 form
  the deletion set below and have no dynamic import, barrel, HTML, or test
  harness route into production.
- Current export, chat history, social scheduling, timeline, and transcript
  behavior is owned by reachable replacements; backend commands remain intact.
- Fresh baseline: typecheck passes, the complete desktop test chain passes,
  desktop UI smoke is 22/22, and seven stage goldens are SSIM 1.000000.

## Delete

- Legacy components: `Composer`, `JobCard`, `SessionBar`, `SocialSchedule`,
  `ScopeDock`, `Dialog`, `ShellModeToggle`, and `TranscriptSidebar`.
- Legacy hooks/state: `useExportJob`, its watchdog, and `shellMode`.
- Test-only helpers: `publishingBridge`, `proxyBlobUrl`,
  `segmentedVideoSlotLoad`, `jobsSummary`, `cacheStrip`, and `moveDraft`.
- Tests that exist only for the deleted helpers and the unused
  `@radix-ui/react-dialog` direct dependency.

## Preserve

- Reachable replacement flows and all Tauri command handlers.
- Generated protocol, `App.css`, shared stores, and unrelated worktree state.

## Verification

1. Re-run TypeScript reachability; no production-unreachable TypeScript module
   should remain.
2. Run desktop typecheck and the revised complete test chain.
3. Run desktop UI smoke and stage visual goldens through that chain.
4. Run `git diff --check` and review the exact deletion boundary.

