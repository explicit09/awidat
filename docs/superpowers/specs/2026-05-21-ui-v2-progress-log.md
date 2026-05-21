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

## Phase 4.2 — Indexing pipeline backend audit

Of the 9 indexing tasks the design spec names, the current backend has:

| Task                  | Backend status                                                |
|-----------------------|----------------------------------------------------------------|
| Transcripts           | ✅ Wired (`whisper` indexer in `crates/index/src/lib.rs`)       |
| Scenes                | ✅ Wired (`scenedetect`)                                        |
| Audio analysis        | 🟡 Partial — used by `find_dead_air`, `find_filler_words` (no top-level indexer) |
| Face detection        | 🟡 Used by `find_speaker_oncam`, `find_eye_contact` (no top-level indexer) |
| Motion analysis       | ❌ No first-class indexer                                       |
| Color analysis        | 🟡 Has tools (`color_scopes.rs`, `list_looks.rs`) — no indexer  |
| Silence detection     | 🟡 Used by `find_dead_air` — no top-level indexer               |
| Speaker diarization   | 🟡 Schema exists (`TranscriptSpeaker`) — pipeline TBD          |
| Caption readiness     | 🟡 Derivable from transcript completeness — no explicit signal  |

The IndexingDashboard component renders all 9 rows. Missing ones surface as
`status: "missing"` with "Pending — not yet computed" copy so the UX is honest
about what isn't computed yet. The named-indexer additions are out of scope for
Phase 4 and tracked separately.

## Phase 4.4 — Delivery preflight backend (deferred)

The DeliverySurface (Phase 4.3) renders preflight findings + render summary
from a shape the backend will populate. Today there's only `start_timeline_render`
producing one mp4; per-target packaging + preflight checks need:

- Per-target preset metadata (aspect ratio, codec, captions, cover frame export).
- Preflight checker (loudness target, caption length, safe-area, etc.).
- Multi-output render queue (one render → many target artifacts).

These are real backend tasks but they're not on the foundation-rewrite critical
path. The UI is ready when they ship. Logged here and skipped to keep the loop
moving toward Phase 5.

## Loop event log

| Time | Phase | Event |
|------|-------|-------|
| start | 2.5 | Beginning Preview/Review surface. |
