# UI Concept vs. Current Desktop — Gap Analysis & Rewrite Plan

**Date:** 2026-05-21
**Status:** Draft v2 — aligned to canonical design spec
**Owner:** TBD
**Scope:** `apps/desktop` (Tauri + React frontend). Backend crates out of scope; we reuse what's there.

## Canonical references

The design system is fully specified in two places. Treat them as authoritative; this doc is a rewrite plan, not a re-specification.

1. **`~/Downloads/Awidat UI Design Concept.md`** — the design system: brand, color, typography, spacing, elevation, motion, icons, component specs, interaction rules, design token JSON.
2. **`~/Downloads/Awidat UI Design Assets/`** — the 9 concept PNGs + SVG logo files. The PNGs are direction; the markdown is the spec.

When in doubt, the markdown spec wins over the PNGs. When the markdown is silent, follow the PNGs.

---

## Why this document exists

The user has 9 concept screens + a comprehensive design spec describing a polished, agent-native, evidence-backed video-editing cockpit. The current desktop UI is a generic three-column editor with no stage model, no real design system, and a proposal model that doesn't surface confidence/risk/alternatives.

User decision: **rewrite the frontend from scratch, foundation-first**, treating the design spec as ground truth, with performance as a hard requirement. Premise: code quality bounds what AI agents working in this repo can produce — invest in the foundation before chasing visuals.

This doc inventories the gap and proposes a phased rewrite that ships something usable at each phase.

---

## Product framing (from the design spec — internalize this)

- **Product name:** Awidat. Sentence-case in body copy; all-caps only in tiny decorative labels. *(Earlier drafts of mine misspelled it "Avidat" — that was wrong.)*
- **Category:** Agent-native professional video editor. **Not** a Premiere/Resolve clone, not a magic-button AI, not a chatbot wrapper.
- **Core UX model:** *Intent → Indexing → Proposal → Review → Revise → Deliver.* Six stages, the human-in-the-loop loop.
- **User role:** Directs an expert editor and reviews its work.
- **Agent role:** Proposes, explains, gathers evidence, waits for approval.
- **Voice:** Calm, precise, transparent, technical, trustworthy.

### Hard product rules (from §10 of the spec)

These shape the UI architecture; the rewrite must honor them:

1. No destructive changes without acceptance.
2. Every proposed edit must have a visible reason.
3. Every accepted/rejected/pending state must be visually distinct.
4. The user must always know whether they're seeing *current timeline*, *proposed timeline*, or *render output*.
5. Evidence must be inspectable (transcript, audio, speaker, visual, scene, confidence).
6. Advanced detail is progressively disclosed through "Inspect deeper."
7. **Review-first, not manual-edit-first.**
8. **Evidence-first, not magic-first.**
9. The command rail is a production command interface, not a chatbot.

### Anti-patterns the rewrite must avoid (from §12 of the spec)

- Full manual toolbar as the core UI.
- Dense color wheels/scopes as primary workspace.
- Track-heavy timeline dominating every screen.
- Chatbot taking over the whole product.
- "Generate video" magic app framing.

---

## Information architecture

Two navigation axes, both visible in the top chrome:

**Axis 1 — Stages (the loop):** Intent, Indexing, Proposal, Review, Revise, Deliver. These represent *where in the work the user is*. State, not navigation.

**Axis 2 — Workflow lenses (the surface):** Import, Index, Selects, Assembly, Review, Captions, Audio, Color, Delivery. These represent *which view the user is working in*. Navigation, not state.

A user can be in the *Proposal* stage while looking at the *Review* lens, or in the *Deliver* stage while looking at the *Color* lens. Stages and lenses are orthogonal.

### Spatial model (recurring across screens)

1. **Top chrome** (44px) — logo, stage indicator, lens nav, project switcher, agent status, share, settings.
2. **Workflow lens row** (44px) — the active lens highlighted, others clickable.
3. **Left Agent / Command Rail** (320px) — natural-language commands, plan, task progress, activity log, suggested next actions.
4. **Center Preview / Review Surface** — video + selected-cut overlay.
5. **Bottom Timeline / Transcript Hybrid** — transcript as first-class editing surface, synchronized waveform/timeline lanes.
6. **Right Proposal Inspector** (320px) — the selected cut/proposal's detail: time range, type, intent, confidence, risk, evidence, alternatives.
7. **Status footer** (32px) — local-first, terminal-aware status.

The same spatial model recurs across most screens with different content; not every screen renders all panes (e.g., Ingest has no Proposal Inspector).

---

## Current state inventory

Inventoried earlier. Condensed: the rewrite assumes the frontend is greenfield. The codebase contributes **reusable backend (19 Tauri command modules)** and a few non-trivial widgets (SegmentedVideoView, MotionAnimationControl, transcript loader) whose internal logic is salvageable. **Every other piece of UI is discarded.**

### What we keep (in some form)

- Tauri 2.x + Vite + React 19 + Zustand. Stack stays.
- Zustand stores survive in shape but are reorganized: `useProjectStore`, `useAgentStore`, `useTimelineStore`, `useMediaStore`, `useTranscriptStore`, `useNotesStore`, `useProposalStore`. New: `useStageStore`, `useLensStore`, `useSelectionStore` (replaces `useTimelineSelectionStore`).
- All 19 Tauri command modules. Protocol extensions needed for the Proposal model — see "Backend extensions."
- `SegmentedVideoView` playback core (wrap in new chrome).
- `MotionAnimationControl` keyframe editor (wrap in new chrome).
- Transcript loader (`useTranscriptStore.byStem`) — keep; rebuild the view.

### What we delete

- `App.tsx` (rebuilt from scratch).
- `App.css` (5,192 lines — replaced with token-driven system).
- Every pane container (PropertiesPane, TimelinePane chrome, MediaPane chrome, NotesPanel chrome, VeditPanel chrome, ChatStream chrome, ActionBar, SessionBar, ProjectBanner). Their internal logic informs the rebuild but the components are rewritten.
- The current Playwright smoke test (selectors break; rewritten alongside the UI).

---

## Design foundation — confirmed values

Pulled from the canonical spec. These become the Phase-1 token files.

**Brand:** Awidat. Triangular A mark + teal/green palette + small blue node. SVGs in `Awidat UI Design Assets/`.

**Color tokens:** Full surface, border, text, brand, semantic, status-pill, and data-viz palettes are specified with exact hex values in §2 of the design spec. I'm not duplicating them here — Phase 1 generates `tokens.css` directly from that section.

**Typography:**
- Primary UI: **Inter** (`@fontsource/inter`)
- Monospace: **JetBrains Mono** (`@fontsource/jetbrains-mono`)
- Type scale: Display 32 / H1 24 / H2 18 / H3 15 / Body 14 / Body-small 13 / Label 12 / Caption 11 / Micro 10 / Timecode 13 mono
- Sentence case for actions; uppercase only for compact labels.

**Spacing:** 4px base grid, ramp 0/4/8/12/16/20/24/32/40/48/64.

**Layout sizing:** Top chrome 44, lens row 44, left rail 320, right inspector 320, main panel min 720, footer 32, primary button 36, compact button 28, pill 22, timeline row 32, transcript row 48 min.

**Radius:** xs 4 / sm 6 / md 8 / lg 12 / xl 16 / full 999.

**Elevation:** Shadow-light. Mostly border-driven; glow tokens for active/warning/danger states.

**Motion:** Fast 120 / Medium 180 / Slow 260 / Progress 400 / Attention 900ms. Two signature easings: `cubic-bezier(0.2,0,0,1)` for state changes, `cubic-bezier(0.16,1,0.3,1)` for panels/modals.

**Icons:** Lucide React. Sizes 12/16/20/24/32 at 1.75px stroke (1.5px at 32). Stage and agent-role glyphs use specific Lucide icons listed in §7 of the spec.

---

## The gap, by screen

**Legend:** ✅ exists and reusable · 🟡 exists but wrong shape · ❌ missing · 🔧 needs backend extension

### Screen 1 — Product concept & IA
- 🟡 Six-stage loop in code — code is one-page, concept is six-stage with lens nav. Stage + lens models both need building.
- ❌ Workflow lens nav.
- ❌ Brand surface (proper Awidat wordmark + mark in top chrome).

### Screen 2 — Main desktop workspace (Proposal Review)
- ✅ Video playback core (SegmentedVideoView).
- ✅ Timeline rendering machinery (current TimelinePane logic).
- 🟡 Transcript/timeline hybrid — transcript exists, click-to-seek exists, but transcript is **not** a first-class editing surface today.
- ❌ "Proposed Timeline" vs "Current Timeline" visual distinction.
- ❌ Numbered jump chips for pending changes.
- ❌🔧 Proposal Inspector (right pane) — current ProposalActions is a two-button overlay. Needs full inspector with intent, confidence, risk, evidence, alternatives.
- ❌ "Inspect deeper" progressive disclosure.

### Screen 3 — Agent proposal review (batch)
- ❌ Batch proposal model entirely. Current `useProposalStore` holds one active proposal; concept needs a list with per-row + batch actions.
- ❌🔧 Before/after preview surface.
- ❌ Agent command history + active request UI.
- ❌ Evidence highlights panel.

### Screen 4 — Timeline/transcript hybrid (Review lens)
- ✅ Transcript data + waveform commands.
- 🟡 Sentence-level selection — exists for delete; needs richer affordances ("Keep this pause," "Edit around selection").
- ❌🔧 Speaker confidence + per-evidence tags (filler phrase, pause duration, etc.) — protocol extension.
- ❌ Current/Proposed timeline toggle.

### Screen 5 — Cut/proposal inspector (single cut deep-dive)
- Same gaps as Screen 2 inspector plus:
- ❌ Alternatives variant cards.
- ❌ "Compare alternatives" + "Agent repair" actions.
- ❌ Current/proposed/render-output context switcher.

### Screen 6 — Import/indexing
- 🟡 Import + indexing commands exist; UI is fragmented JobCards. Needs unified indexing dashboard.
- ❌ Named indexing pipeline (the spec lists nine: Transcripts, Scenes, Audio Analysis, Face Detection, Motion Analysis, Color Analysis, Silence Detection, Speaker Diarization, Caption Readiness). Backend likely covers a subset.
- ❌ Extracted structure preview (duration, scenes, speaker segments).
- ❌ System status (local processing, disk, temperature) — "terminal-aware status footer."
- ❌ "Ask agent for first cut" CTA.

### Screen 7 — Delivery/preflight
- 🟡 Render command exists; no preflight, no platform targets, no preset row.
- ❌🔧 Per-target preflight checklist (pass/warning/failure filters).
- ❌🔧 Safe-area preview guides.
- ❌ Issue inspector with problem/impact/suggested fix/agent repair.

### Screen 8 — Empty/loading/error states
- ❌ All three. Current empty state is functional but undesigned. Critical foundation work — these are the most-seen screens before users become productive.

### Screen 9 — Component system
- ❌ Every named component. The keystone of foundation-first. List from §9 of the design spec:
  - Proposal card
  - Timeline change markers
  - Transcript segment
  - Agent status
  - Evidence chips
  - Confidence/risk indicators
  - Accept/reject/revise control group
  - Media/indexing status
  - Render/preflight findings

---

## Backend extensions required

Phase ordering assumes additive, optional fields — UI degrades gracefully if backend hasn't caught up.

**Proposal protocol (`ProposedEdit`):** add optional fields
- `confidence: number` (0–1)
- `risk: "low" | "medium" | "high" | "very_high"`
- `risk_flags: string[]`
- `evidence: EvidenceTag[]` (each: kind, label, confidence?)
- `alternatives: ProposedEdit[]`
- `culmination_notes: string`
- `intent: string`

**Proposal store:** support a list of pending proposals, not just one active. Backend already emits items; aggregation is frontend.

**Transcript:** speaker confidence + per-word evidence tags may need to be computed (currently unclear what's available). Defer to phase 3 to investigate.

**Delivery preflight:** new backend surface. Per-target findings + safe-area metadata. Out of scope until phase 4.

---

## Phased plan

Each phase ships something the user can open and use. No phase is infrastructure-only.

### Phase 0 — Decisions (done; recording them here)

These were decided in conversation and apply going forward:

- **Token philosophy:** semantic (`--surface-card`, `--text-primary`). Already specified in design spec §2.
- **CSS approach:** Tailwind v4 + CSS variables for semantic tokens. Tauri/Vite + Tailwind v4 is one config line.
- **Component library base:** shadcn/ui (Radix + Tailwind, copy-in pattern). Already use Radix primitives; shadcn extends. Product-specific components built on top.
- **Anchor screen for Phase 2:** Proposal Review (Screen 2 + 5). Core editing loop.
- **Branch strategy:** long-lived `ui-v2` branch. Not feature-flagged on main. Rewrite touches every UI file; flags don't help.
- **Theme:** dark only at launch. Concept is dark; building light in parallel doubles QA.
- **Platform priority:** macOS first through phase 5. Tauri ships everywhere but we test/polish on macOS.

### Phase 1 — Foundation (1–2 weeks, no user-visible change)

Goal: every component built after this phase composes from primitives. No bespoke CSS per pane.

- **Tokens.** `apps/desktop/src/ui/tokens.css` generated from design spec §2 (colors), §3 (typography), §4 (spacing/sizing/radius/borders), §5 (elevation), §6 (motion). Plus Tailwind v4 config that exposes the tokens as utility classes.
- **Fonts.** `@fontsource/inter` + `@fontsource/jetbrains-mono` installed and imported.
- **Icon library.** `lucide-react` installed.
- **Primitives** under `apps/desktop/src/ui/`. The list comes from the spec, not invention:
  - `Button`, `IconButton` (Accept / Reject / Revise / AgentRepair variants — spec §9)
  - `Pill` (Proposed / Pending / Accepted / Rejected / Reviewing / Processing / Ready / Failed / Warning / Missing / Revised)
  - `EvidenceChip`
  - `ConfidenceMeter` (label + score, never color-alone)
  - `RiskIndicator` (label + dots, never color-alone)
  - `AgentStatusBadge`
  - `Card` (base for ProposalCard, TranscriptSegment, etc.)
  - `Stack`, `Inline`, `Divider`
  - `Tooltip`, `Popover`, `Dialog` (Radix-backed, restyled)
  - `Tabs`, `Toast`
- **Product components** (built on primitives, also Phase 1 because they're foundational):
  - `ProposalCard`
  - `TranscriptSegment`
  - `TimelineMarker` (Accepted / Rejected / Pending / Warning / Info / Skipped / Selected variants)
  - `MediaStatusRow`
  - `PreflightFindingRow`
- **Brand assets.** `Awidat UI Design Assets/awidat-mark.svg` and `awidat-wordmark.svg` copied into the repo. (The on-disk SVGs differ slightly from the inline ones in the design doc; defer to the design-doc versions when there's a conflict — log this as a brand-cleanup task.)
- **Storybook-style dev harness.** Extend `apps/desktop/tests/ui-harness.html` so every primitive and product component renders in every state. Playwright asserts no visual regression.
- **Performance baseline.** Measure TimelinePane FPS, video scrub latency, transcript render time. These become regression bars enforced from Phase 2 onward.

Ship criteria: every primitive + every product component renders in the harness; CI runs harness smoke; old UI still works untouched.

### Phase 2 — App shell + Proposal Review end-to-end (2–3 weeks)

Goal: launch the new shell with **one stage fully implemented** — Proposal Review.

- **App shell.** New `App.tsx` (small, just composition). Mounts:
  - Top chrome (44px) — logo, stage indicator, lens nav, project switcher, agent status, settings.
  - Lens row (44px) — Review lens active; others present.
  - Left Agent / Command Rail (320px).
  - Center Preview / Review Surface.
  - Bottom Timeline / Transcript Hybrid.
  - Right Proposal Inspector (320px).
  - Status footer (32px) — terminal-aware status (local processing, disk, project state).
- **State:** `useStageStore` (current stage), `useLensStore` (current lens). Both default to first item for new projects; persisted per project.
- **Screen 2 + Screen 5 implemented in full.** Proposal Inspector with intent, confidence, risk, evidence, alternatives, Accept/Reject/Revise/Inspect-deeper. Numbered jump chips. Current vs Proposed timeline toggle. Selected-cut overlay on preview.
- **Backend extension:** ship `ProposedEdit` optional fields listed under "Backend extensions." UI renders them when present; degrades gracefully when absent so agent updates can land progressively.
- **Other stages:** disabled-but-visible in stage indicator. Clicking them routes to a stub "Coming soon" screen, not the classic UI. **No classic-UI fallback** — the rewrite replaces, doesn't bridge.

Ship criteria: open project → see new shell → agent proposes → user reviews with full inspector → accept/reject lands on backend.

### Phase 3 — Review lens + Revise (2–3 weeks)

- **Review lens (Screen 4):** transcript-as-first-class editing surface. Speaker labels, evidence tags, sentence selection, "Keep this pause" / "Edit around selection" affordances, current/proposed toggle.
- **Agent proposal review (Screen 3):** batch view. `useProposalStore` extended to hold a list. Agent command history + active request. Accept-all / Accept-selected / Reject / Revise-with-prompt / Inspect-deeper.
- **Revise stage:** integrates the existing vedit machinery (commits, diffs, restore) but reframed: "Revise" is the agent-mediated correction stage, not raw history browsing.

### Phase 4 — Indexing + Deliver + System states (2–3 weeks)

- **Indexing (Screen 6):** unified pipeline dashboard. Map the spec's 9 indexing tasks to actual backend commands; surface what's missing. "Ask agent for first cut" hand-off.
- **Deliver (Screen 7):** preflight checklist, target presets, safe-area preview. Needs backend work — scope tight: ship YouTube + TikTok + Instagram presets first; Captions + Cover + Custom as fast-follow.
- **System states (Screen 8):** empty / loading / error for every stage. Real copy, real recovery actions ("Repair with agent," "Retry"). Not a single generic empty state.

### Phase 5 — Polish + retire legacy code

- Remove all dead code from `apps/desktop/src/` that the rewrite obsoleted.
- Performance pass against Phase 1 baselines.
- Visual QA against the 9 concept screens.
- Rewrite Playwright smoke against new selectors.
- Brand-cleanup task (reconcile on-disk SVGs with design-doc SVGs).

---

## Risks

1. **Backend protocol additions slip.** Mitigation: every new field is optional; UI renders conditionally. Already designed in.
2. **Agent doesn't produce confidence/risk/alternatives yet.** Separate workstream. UI ships ready; agent catches up.
3. **Performance regressions during refactor.** Phase 1 sets baselines, Phase 5 enforces. Measure continuously.
4. **Scope creep on components.** Build what the spec names. Don't invent.
5. **Indexing pipeline mismatch.** Spec lists 9 indexing tasks; backend probably covers a subset. Phase 4 investigates and surfaces gaps — don't block Phase 1–3 on this.
6. **The on-disk SVGs differ from the design-doc SVGs.** Minor — defer to design doc; fix in Phase 5 brand pass.

---

## Open questions for the user

These don't block Phase 1 but should be answered before Phase 4:

1. **Indexing tasks** — of the 9 named (Transcripts, Scenes, Audio Analysis, Face Detection, Motion Analysis, Color Analysis, Silence Detection, Speaker Diarization, Caption Readiness), which exist today? Which are aspirational? (Will check the codebase in Phase 3.)
2. **Delivery preset scope** — v1 ships how many of the 6 platform targets? (YouTube, TikTok, Instagram, Captions, Cover, Custom.)
3. **Hi-res crops** — the four zoomed crops referenced in §11 of the design doc (`screen1_bottom_strip_2x.png`, etc.) aren't in the assets folder. If they get generated, they help; if not, we work from the originals.
4. **Final brand pass** — current SVGs are noted as placeholders. Is a brand-identity exercise scheduled, or do the placeholders ship?

---

## Next step

Phase 1 starts. First commit on `ui-v2` branch sets up tokens, fonts, icons, and the primitive scaffolding.
