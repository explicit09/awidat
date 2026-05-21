# UI v2 — Autonomous Progress Log

Status doc updated continuously while building the v2 rewrite on `ui-v2`.

Goal: be the first thing you check in the morning. Tells you exactly what
got done, what failed, what's next.

## Branch state

- Branch: `ui-v2`
- Last clean checkpoint commit: `6629573` (Phase 2 part 1)
- All commits on `ui-v2` only. Main is untouched.

## Phase progress

- [x] Phase 1 — foundation (tokens, primitives, components, gallery)
- [ ] Phase 2 — app shell + Proposal Review end-to-end
  - [x] 2.1 stage + lens stores
  - [x] 2.2 app shell layout
  - [x] 2.3 stage indicator + lens nav
  - [x] 2.4 command rail
  - [ ] 2.5 preview / review surface — IN PROGRESS
  - [ ] 2.6 timeline / transcript hybrid
  - [ ] 2.7 proposal inspector
  - [ ] 2.8 backend protocol extension
  - [ ] 2.9 useProposalStore wiring
  - [ ] 2.10 stub other 5 stages
  - [ ] 2.11 App.tsx cutover
  - [ ] 2.12 end-to-end smoke
- [ ] Phase 3 — review lens + revise
- [ ] Phase 4 — indexing + deliver + system states
- [ ] Phase 5 — polish + retire legacy

## Loop rules

- Each sub-task must pass `pnpm exec tsc --noEmit` and `pnpm exec vite build`
  before committing.
- After each sub-task, screenshot via Playwright and check the shell preview
  visually before continuing.
- If anything fails, stop the loop, leave a "BLOCKED" entry below, do not
  commit broken state.
- Never push, never touch main, never run destructive git ops.

## Loop event log

| Time | Phase | Event |
|------|-------|-------|
| start | 2.5 | Beginning Preview/Review surface. |
