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

## Phase 5.5 — Merge ui-v2 to main (held for user approval)

Per the safety rules I set at the start of the loop, I do not push to or merge
into `main` unsupervised. `ui-v2` is at commit `2a1c6cb` at the time of this
write; everything is green:

- `cargo check -p awidat-desktop` clean (2 pre-existing unreachable-arm warnings, unchanged)
- `cargo test -p awidat-desktop-protocol --lib` 62 tests pass
- `pnpm exec tsc --noEmit` clean
- `pnpm exec vite build` builds main + gallery + shell entries
- `node tests/desktop-ui-smoke.mjs` 12/12 checks pass
- `node tests/perf-budget.mjs` all 5 budgets pass with significant headroom

To merge when you're ready:

```
git checkout main
git merge --no-ff ui-v2 -m "Merge ui-v2: foundation rewrite of Awidat desktop"
git push origin main
```

Or do a squash-merge via PR for a single tidy commit.

## Phase 5.4 — Visual QA against concept screens

Side-by-side comparison of each shipped stage vs the canonical concept PNGs:

| Stage    | Concept image (in ~/Downloads/Awidat UI Design Assets/) | Match |
|----------|----------------------------------------------------------|-------|
| Proposal | `podcast_editing_interface_with_ai_assist.png` | ✅ structurally complete: stage+lens nav, dual-cam viewer, transcript-aligned timeline lanes, Proposal Inspector with all blocks (intent/confidence ring+bar/risk/evidence/explanation/alternatives), Accept/Reject/Revise + Inspect deeper footer. |
| Review   | `podcast_editing_in_dark_mode_interface.png`   | ✅ transcript-as-edit-surface, evidence chips, keep/edit/note affordances, channel-lane strip ready. |
| Indexing | `media_indexing_dashboard_with_progress_indicators.png` | ✅ all 9 named indexing tasks render with status pills + "Ask agent for first cut" hand-off card. |
| Deliver  | `professional_media_delivery_interface_overview.png` | ✅ 6 platform targets, preflight severity filters, summary + confidence + 4 action buttons. |
| Intent / Revise | (Screen 8 + StageStub) | ✅ empty-state pattern from spec §8. |

Polish gaps (deferred to a supervised pass):

1. **Top chrome** — concept has window-traffic-lights + project picker
   ("Interview_A · Podcast Episode ▾") + a top-level nav (Project /
   Workspace / Agent / Media / Review / Deliver / Settings) + undo/redo /
   notifications / avatar / Share. Mine has logo + stage nav + agent
   status + share/settings icons. Functionally equivalent, visually thinner.
2. **Lens row** — concept's active lens is brand-blue, mine is brand-teal.
   Easy swap; defer to design-token alignment review.
3. **Command Rail** — concept uses numbered steps (1, 2, 3) for Plan;
   mine uses bullets + struck-through on complete. Easy refactor.
4. **Status footer** — concept has rich footer (model name, context
   window, autosave timestamp, render queue). Mine is just "Agent online
   · local · disk OK". Add when backend metrics surface.

These polish items don't affect Phase 5.5 (merge to main) — they're best
addressed as a focused design-polish pass with eyes on the actual UI.

## Phase 5.1 — Legacy code deletion (deferred to supervised pass)

The new App.tsx + shell + state + ui imports NONE of these legacy files:
- `src/agent/ChatStream.tsx`, `Composer.tsx`, `SessionBar.tsx`,
  `ApprovalCard.tsx`, `UserInputCard.tsx`, `JobCard.tsx`
- `src/app/ActionBar.tsx`, `EmptyState.tsx`, `ProjectBanner.tsx`
- `src/media/MediaPane.tsx`, `SegmentedVideoView.tsx`
  (SegmentedVideoView is referenced by media/usePlaySegments.ts which
  is itself not imported by the new shell — safe to delete the whole tree)
- `src/notes/NotesPanel.tsx`, `BrollNoteCard.tsx`
- `src/properties/PropertiesPane.tsx`, `MotionAnimationControl.tsx`
- `src/timeline/TimelinePane.tsx`, `ProposalActions.tsx`, `ProposalHandles.tsx`
- `src/transcript/TranscriptSidebar.tsx`, `TranscriptView.tsx`
- `src/vedit/VeditPanel.tsx`
- `src/App.css` (5,192 lines — only consumed by the separate `broadcast` Vite entry)

What CANNOT be deleted (still used by useAppGlue):
- `src/agent/store.ts`
- `src/app/state.ts`, `src/app/menuCommands.ts`
- `src/media/store.ts`, `src/media/mediaStreamUrl.ts`
- `src/notes/store.ts`
- `src/properties/store.ts`
- `src/timeline/store.ts`, `src/timeline/proposal.ts`

The deletion is mechanical but the blast radius is large (~30 files). Deferring
to a human-supervised pass to avoid breaking the build overnight. The new app
is fully functional with the legacy code sitting unused alongside it.

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
