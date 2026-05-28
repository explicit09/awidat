# Awidat Desktop UI/UX Redesign — Design Spec

**Date:** 2026-05-28
**Status:** Approved for plan
**Surface:** `apps/desktop/` (React 19 + Tailwind 4 + Radix + Zustand)
**Owner:** explicit09
**Prior art:** `apps/desktop/src/ui/tokens.css` (existing design system, mostly retained); `~/Downloads/Awidat UI Design Concept.md` (referenced by tokens.css comments)

---

## 1. Why we're redoing this

A live walkthrough surfaced six concrete UX failures:

1. **Black voids.** Source review with no playable media shows a black rectangle and nothing else. No skeleton, no slate, no acknowledgment that an asset exists or is processing.
2. **The "Missing × 9" wall.** The Index rail repeats "Waiting for local indexer · Missing" nine times. The repetition is pure noise — the user reads it as broken, not "in progress."
3. **CTA accent inconsistency.** "Import media" is mint, "Create" is blue, "Export now" is mint. No single primary action color.
4. **Chrome reads as debug.** Edit/Deliver tabs are weak, the brand wordmark duplicates with the macOS window title, the bottom status bar looks like a log line.
5. **Empty Agent rail looks unfinished.** A naked input and a single greeting; no path forward for a first-time user.
6. **Pill taxonomy sprawl.** Eleven semantic pill states (`proposed`/`pending`/`accepted`/`rejected`/`reviewing`/`processing`/`ready`/`failed`/`warning`/`missing`/`revised`) with overlapping meanings.

The bones of the existing design system (charcoal palette, Inter + JetBrains Mono, layout sizing tokens) are sound. The execution at the screen level — defaults, hierarchy, status semantics, empty states — is where the gap is. This spec re-grounds the brand and the screen-level patterns; the token file gets reshaped but not replaced.

## 2. Direction — "Studio Pro"

Single sentence: **Awidat reads as a real editing tool, not a SaaS dashboard.**

| Quality | Resolution |
|---|---|
| Personality | Technical, dense, confident, keyboard-first |
| Reference apps | DaVinci Resolve × Linear × Cursor |
| Tone of voice | Precise, terse, no hand-holding text |
| Color disposition | Near-black surfaces; one bright orange action accent used sparingly; cyan for selection/playhead; green/red for outcomes |
| Type disposition | Inter throughout the UI; JetBrains Mono for the wordmark, timecodes, paths, and machine-readable values |

This direction is the *default expression*. The product still serves two audiences (pro editors and solo creators); they share the direction but differ in density — see §4.

## 3. Brand

### 3.1 Mark

A precise equilateral triangle (▲) on a dark plinth. The triangle reads simultaneously as **play**, as the letter **A**, and as a **direction marker on a film slate**. Geometric, not illustrative.

- **Plinth:** `#0F1110` background, `#2A2C2B` 1px border, 10–16px corner radius depending on size.
- **Triangle:** `#FF7A18` orange, with a drop-shadow glow at hero sizes only (`drop-shadow(0 0 24px rgba(255,122,24,0.30))`).
- **Sizes:** generated for 32 / 128 / 256 / 512 / 1024. At 32px the triangle simplifies (no glow, lighter weight) so the icon survives in a favicon slot.

### 3.2 Wordmark

`AWIDAT` in JetBrains Mono SemiBold, all caps, `letter-spacing: 0.16em`, color `#E5E7EB`. Always pairs with the mark in chrome (mark left, wordmark right, 8–10px gap). The mono treatment ties the wordmark to the timecodes and the indexer chips inside the app — the brand and the system speak the same language.

### 3.3 Voice

| Do | Don't |
|---|---|
| "5 of 9 signals ready · ETA ~2 min" | "Waiting for local indexer · Missing" |
| "Building proxy… 67%" | "Loading…" |
| "Drop a file to start" | "No project open yet" |
| "Trim filler & long silences" | "Run AI cleanup workflow" |

Lead with the system state. Don't apologize for emptiness — explain it.

## 4. Mode system — two skins, one shell

A single user-level setting `mode: 'creator' | 'pro'` (persisted via the existing settings surface). The 3-pane shell layout is identical in both modes. The toggle only changes panel *defaults*:

| Surface | Pro default | Creator default |
|---|---|---|
| Inspector rail | All sections expanded (Identity, Visual, Audio, Timing, Track Mix, Timing Metadata, Danger Zone) | Header + Volume/Speed/Trim only; "More" disclosure reveals the rest |
| Index rail | Dense grid (4-stat + grouped 3-col signal grid + indexers strip) | Summary card (progress + ETA + 2 stats) with "Show signal details · advanced" disclosure |
| Transcript / Vedit tabs | Reachable | Reachable |
| LUT / Scopes / Track Mix controls | Visible inline | Hidden until "Show advanced" is opened |
| Top chrome | Mode pill shows `Pro` active | Mode pill shows `Creator` active |
| Agent rail | Same | Same (visually the rail is unchanged across modes; the difference is panel content) |

### Implementation rule

There is **one** component tree. Each collapsible panel accepts a `defaultExpandedIn={ creator?: boolean; pro?: boolean }` prop or a `revealLevel: 'always' | 'pro' | 'advanced'` flag, whichever fits the panel primitive better. Mode toggling **does not** unmount or remount anything; it only flips initial state. Users can override per panel (their override sticks across mode changes within a session).

**No second-route tree, no `<CreatorView />` vs `<ProView />` split.**

## 5. Token revisions (`apps/desktop/src/ui/tokens.css`)

Most existing tokens stay. Targeted revisions:

### 5.1 Brand accent

```diff
- --color-brand: #20C997;              /* mint — DEMOTED */
- --color-brand-hover: #2EE6AE;
- --color-brand-active: #17A77E;
+ --color-brand: #FF7A18;              /* studio orange — primary action */
+ --color-brand-hover: #FF8B33;
+ --color-brand-active: #E5641A;
+
+ --color-accent-mint: #20C997;        /* kept for "agent online" dot + ready states */
+ --color-accent-mint-hover: #2EE6AE;
+ --color-accent-mint-active: #17A77E;
```

The mint is not deleted — it survives as `--color-accent-mint`, **scoped to liveness indicators only**: the "agent online" dot in chrome, the "daemon connected" indicators, and similar "this connection / process is alive" signals. Mint is **not** used for job completion (that's `--color-job-ready` green) or accepted proposals (that's `--color-proposal-accepted` green). Mint **never** appears on buttons; orange is the only primary CTA color.

**Migration:** every existing `var(--color-brand)` consumer in the codebase needs a per-site decision: keep as orange (it's a primary action), move to `--color-accent-mint` (it's a liveness indicator), or move to `--color-success` / `--color-job-ready` (it's a completion state). The foundation PR sweeps the codebase; deferred surfaces (§11) are part of the sweep so they don't visually lag.

### 5.2 Pill family collapse

Replace the eleven `--color-pill-*` triplets with **two families**:

**Job lifecycle** (used by indexers, jobs, proxy builds, exports, anything machine-driven):

| Token suffix | Color | Use |
|---|---|---|
| `--color-job-idle` | `#3F3F46` dot / `#1F2123` fill / `#7A7A82` text | Not started, queued |
| `--color-job-running` | `#FF7A18` dot / `rgba(255,122,24,0.10)` fill / `#FCA67A` text | In progress (with optional `%` in the pill) |
| `--color-job-ready` | `#22C55E` dot / `rgba(34,197,94,0.10)` fill / `#86EFAC` text | Complete, usable |
| `--color-job-failed` | `#EF4444` dot / `rgba(239,68,68,0.12)` fill / `#FCA5A5` text | Errored, needs attention |

**Proposal lifecycle** (used by agent-generated edits, suggestions, anything human-arbitrated):

| Token suffix | Color | Use |
|---|---|---|
| `--color-proposal-proposed` | `#38BDF8` dot / `rgba(56,189,248,0.12)` fill / `#7DD3FC` text | Agent has proposed, awaiting human |
| `--color-proposal-accepted` | `#22C55E` dot / `rgba(34,197,94,0.10)` fill / `#86EFAC` text | Human accepted, applied |
| `--color-proposal-rejected` | `#EF4444` dot / `rgba(239,68,68,0.12)` fill / `#FCA5A5` text | Human rejected |
| `--color-proposal-revised` | `#A855F7` dot / `rgba(168,85,247,0.12)` fill / `#D8B4FE` text | Human accepted a modified version |

**Disabled** is a global visual modifier (`opacity: 0.42`, no separate pill family) — not a state.

### 5.3 Pill geometry

All pills follow the same chrome: 18px height, `border-radius: 999px`, 1px border, 6px dot left, 11px SemiBold label, 2px / 8px padding. The status pill primitive accepts `{ family: 'job' | 'proposal', state, label?, percent?, dotOnly? }`. A `dotOnly` variant exists for inline tables where a 6px dot is enough.

### 5.4 Footer / chrome typography

The footer status strings move from `text-body-sm` (Inter) to `text-micro` (JetBrains Mono 10.5px). Footer reads as a state line, not as prose.

## 6. Chrome — top + bottom

### 6.1 Top chrome (two rows)

**Row 1 — Identity (height 36px)**
```
[traffic] | [▲ mark, 18px] [AWIDAT, mono 11px] · [Demo, sans 13px bold] [~/Downloads/Demo, mono 11px muted]
                                                                                              … [Pro|Creator pill] [↗ share] [⚙ settings]
```

**Row 2 — Workspace (height 30px)**
```
       | [Edit][Deliver][History][Skills]                                                        [00:00:12:08 / 00:00:38:17, mono]
         underline indicator on active tab, orange (#FF7A18, 2px)
```

- Mode pill: oval, two segments (`Pro` / `Creator`), 11px SemiBold, active segment uses `--color-surface-selected`, 1px inset border on active.
- Share + settings icons: 26×26 ghost buttons, hover reveals `--color-surface-hover` and 1px border.
- Tabs: underline-style, `color-text-muted` inactive, `color-text-primary` active, 2px orange underline.
- Live timecode right-aligned in row 2, `--text-timecode` (JetBrains Mono 12px), updates at preview frame rate.

The macOS window title is suppressed (`titleBarStyle: 'Overlay'` in `apps/desktop/src-tauri/tauri.conf.json`) so the brand never duplicates with the OS window title. The top chrome's row 1 must declare `data-tauri-drag-region` on the non-interactive background so the user can still drag the window from the chrome — interactive elements (mark, wordmark, project pill, mode pill, icons) opt out via `data-tauri-drag-region={false}`.

### 6.2 Bottom chrome (footer, height 26px)

Two semantic groups, both mono:

**Left group — project state:**
- `● indexing 56%` (running pill color, `--color-job-running`) — replaced by `● ready` (`--color-job-ready` green) when the indexing job completes
- `5 / 9 signals` — only shown while indexing or partial
- A separate `● agent online` mint dot lives in chrome row 1 (agent rail header), not in the footer — agent liveness and project readiness are two different signals.

**Right group — system state:**
- `Autosaved · 12:58`
- `render <b>0</b>` — bold count of items in render queue
- 4-bar activity glyph (existing throughput indicator, kept)
- `disk <b>22GB</b>` — bold free space

Removed from the footer: `Model: Awidat Pro 1.2`, `Context window: local`. Those move to Settings (model is a deployment detail, not a per-session display).

## 7. Index rail — kill the "Missing × 9" wall

### 7.1 Pro default (dense grid)

**Header**
- `Index readiness` title + job pill on the right (`Indexing 56%` while running, `Ready` when complete)
- One-line meta: `5 of 9 ready · 4 queued · ETA ~2 min` (mono)
- 4px progress bar (orange gradient)

**4-stat block** (2×2 grid of cards)
- Duration · Scenes · Segments · Transcript-length
- Each card: mono value (13px SemiBold) + uppercase 10px label
- `—` placeholder when unknown (kept low-contrast, not styled as error)

**Signal grid** (grouped by domain)
- Three groups: **Speech** (Transcript, Captions, Diarization), **Visuals** (Scenes, Faces, Color, Motion), **Audio** (Loudness, Silence)
- Each group: small uppercase label + `n / m ready` mono summary, then a 3-column tile grid
- Each tile (signal): name (11px SemiBold) + status dot + state word + optional `%` for running signals
- Tile background `--color-surface-card`, 1px gap between tiles produced by parent grid `gap: 1px` on `--color-border-subtle` background
- A row with fewer than 3 signals fills the remaining columns with low-opacity `—` placeholders so the grid never reflows on signal availability changes

**Indexers strip** (bottom)
- Single line: `12 indexers active` + first 3 indexer name chips + `+9` overflow chip
- Click expands the strip into the full per-indexer list (today's screenshot 6 contents, slightly tightened)

### 7.2 Creator default (summary first)

**Single summary card** replaces the entire dense grid:
- Large title: `Indexing your media…` (changes to `Ready to edit` when complete)
- Right-side job pill with `%`
- 4px progress bar
- One-line subtext: `5 of 9 signals ready · ETA ~2 min · works offline`

**2-stat block** (Duration · Detected-scenes-count) — half the information of Pro

**Single body paragraph:** "Awidat is reading your media. As soon as it's done, the agent can propose cleanup edits."

**Disclosure button:** `▾ Show signal details · advanced` — expands to the Pro grid in place, no navigation.

### 7.3 State transitions

- Idle (no media) → entire rail hidden; replaced with an "Add files to begin" empty surface (see §10.4).
- Indexing → progress UI, no error styling.
- Failed → header pill turns red, failing signals show `failed` state with a hover tooltip carrying the error string. Other signals continue independently.
- Ready → header pill mint, progress bar disappears, "Re-run indexers" link becomes the primary affordance.

## 8. Inspector rail (right side)

The Inspector content from screenshot 7 (Identity / Visual / Audio / Timing / Track Mix / Timing Metadata / Danger Zone) is **kept** — all its data is real and meaningful for Pro users. The redesign work here is **organization and density**, not feature changes:

### 8.1 Pro default

- All sections expanded.
- Section headers: uppercase 10px tracked label + section-level inline collapse caret.
- Slider rows tighten: label (left, 11px SemiBold), full-width track, value badge right (mono 11px).
- Sliders use orange thumb and a faint orange fill from the zero point. Selected/active track stem uses `--color-brand-secondary` (kept cyan) so action (orange) and selection (cyan) never collide.
- `LUT` field gets a 22px swatch on the left of the input showing the current LUT's first-frame color average (already computed by the color pipeline; surface it).
- Danger Zone moves below a divider with a 12px gap and a `--color-text-muted` heading. Delete button keeps its current red border treatment.

### 8.2 Creator default

- Sections collapsed except: **Identity** (read-only summary), **Audio → Volume**, **Timing → Speed**, **Audio → Fades**. Four controls total.
- `Show advanced editing controls` disclosure below; opens everything Pro shows by default.
- Danger Zone hidden until disclosure opens.

### 8.3 Behavior

- No clip selected → rail shows the empty Inspector pattern: muted "Select a clip to edit" line + a small ghost of the clip-icon. Not a blank rectangle.
- Multiple clips selected → headers say `n clips`, identity fields show `—`, sliders show the common value or a `mixed` chip if values differ.

## 9. Status pill primitive

A single React component replaces the existing status pill implementations:

```tsx
<StatusPill
  family="job" | "proposal"
  state={"idle" | "running" | "ready" | "failed"
       | "proposed" | "accepted" | "rejected" | "revised"}
  label?: string         // override default label string
  percent?: number       // appended to label as ` · 56%`; only valid for running
  dotOnly?: boolean      // 6px dot only, for inline tables
  size?: 'sm' | 'md'     // default md (18px); sm = 16px for inline use
/>
```

- Default labels are localized via a single map (no inline strings in callsites).
- The component asserts at build time (TS) that `percent` is only allowed on `state==='running'`.
- All existing call sites of pill components are migrated; deprecated `<Pill kind="missing">` etc. removed.

## 10. Empty + loading states

**DNA rule:** every empty surface is replaced with either (a) progress, (b) a clear next move, or (c) explicit acknowledgment of the asset that exists but isn't ready. Never a black rectangle.

### 10.1 No-project landing (screenshot 1 replacement)

- Centered hero: 48px ▲ in orange with glow, 13px AWIDAT mono caps below, both centered horizontally.
- Heading: `Open a project to start editing` (16px SemiBold).
- Body (max 38ch): `Awidat needs a project to index your media and run the agent. Drop a file anywhere in the window, or pick one of these.`
- Action row, in order:
  1. Primary: `＋ New project ⌘N` (orange filled button)
  2. Secondary: `Import media ⌘I`
  3. Secondary: `Open… ⌘O`
  4. Tertiary muted: `Try example`
- Muted `recent ·` line at bottom listing last 3 projects (mono 10px, hover underline).
- The whole canvas is a single drop-target; on drag-over, the body background fades to `rgba(255,122,24,0.06)` with a 1px dashed orange inset border.

### 10.2 Source review while media loads (screenshot 3 fix)

Replaces the black rectangle with a **film slate** standing in for the preview:

- Slate background: 45° striped charcoal pattern (`repeating-linear-gradient(45deg, #0F1112 0 6px, #0A0B0B 6px 12px)`), 16:9 framed inside the preview area, 1px charcoal border.
- Top-left mono caption: `<filename>` · Top-right: `<width>×<height> · <codec>`
- Center overlay (Inter): `Building proxy…` (14px SemiBold orange) · `5 of 9 indexers ready · transcript at 67%` (11px secondary) · `~2 min · runs locally` (10px mono muted)
- Bottom-left: `<size> · <duration>` · Bottom-right: `<audio codec/rate>`
- 3px progress bar pinned to the bottom edge of the slate (orange gradient).
- Transport bar stays live; scrub thumb is disabled until first proxy frames are decodable, then activates without remount.
- When proxy is ready, slate cross-fades out (200ms) and `<video>` reveals.

### 10.3 Agent rail (empty conversation) (screenshot 4 fix)

- One opener card from the agent at the top:
  - Avatar/glyph + `Awidat · read AGENTS.md · podcast mode` micro-header
  - Message body: `I've indexed your <duration> clip — speech, scenes, color, audio all ready. Tell me how you want this cut, or pick a starting move below.`
  - When the index is not yet ready, the opener becomes: `Reading your media now — 5 of 9 signals done. You can still send a message; I'll work on it once indexing finishes.`
- Suggestion list (4 starter prompts, project-type aware). The initial implementation hard-codes a small map keyed on the project's `project_type` (e.g., podcast, interview, highlight). Future iteration may source suggestions from skill metadata — that's out of scope for this spec.
  - Podcast: `Trim filler & long silences` · `Cut to the punchline (≤ 90s)` · `Find a YouTube-ready highlight` · `Apply podcast cleanup defaults`
  - Fallback when `project_type` is unknown: `Trim long silences` · `Summarize what's in this clip` · `Suggest a starting edit` · `Show me the loudest moments`
- Each suggestion: orange `▸` glyph + label, 7px×10px padded, hover lifts to `--color-surface-card-hover`.
- Input pinned to the bottom of the rail with the @ attach hint inline, send affordance is a quiet orange `⏎`.
- The footer of the rail surfaces model + mode in mono micro (e.g., `● awidat-pro · manual mode`) — same scale as the app footer, distinct from the agent card.

### 10.4 Indexers running, no clips on timeline yet

- Timeline area shows the existing `no tracks yet + Track` button row + a single-line empty hint below: `Drop clips here, or ask the agent for a starting cut.` (12px muted, centered, no illustration).
- Generated-media panel keeps its current `No generated media yet` card pattern but adopts the new chrome.

## 11. Out of scope for the spec (defer to plan or follow-up)

These surfaces use the new tokens and chrome automatically (they live inside the shell) but their detailed redesigns are **not** in this spec and will be designed in-flight during implementation or in a follow-up spec:

- **Inspector visual treatment of individual sliders** — token-driven; matches §8.1 directionally but pixel-perfect spec is implementation-time.
- **Timeline clip cards, track lanes, thumbnail strips, audio waveform colors** — receive the new tokens; deeper redesign deferred.
- **Deliver view** (Targets / Preflight / Render queue / Safe-area legend) — receives the new chrome, pill system, and accent; full layout redesign deferred to a follow-up spec.
- **New Project modal** (screenshot 2) — receives the new modal chrome (already mostly tokenized); copy and field grouping kept as-is.
- **Settings surface** — gets the model + context display moved to it; not redesigned beyond that move.
- **Full keyboard shortcut map** — the chrome locks `⌘N / ⌘I / ⌘O / ⌘K / ⌘1 / ⌘2`; the rest of the shortcut surface is implementation-time.

The plan that consumes this spec **must** sequence the work so foundation (tokens, chrome, pill primitive, empty states, Index rail) ships before any of the deferred surfaces are touched. Otherwise the deferred surfaces will visually lag the rest of the app during the migration window.

## 12. Non-goals

- **Light mode** is not added in this redesign. Tokens stay dark-only. Light mode is a separate project.
- **Brand pivot beyond Studio.** The mark and accent are locked; future brand evolution is out of scope.
- **Renaming product surfaces** (e.g., calling "Deliver" something else) is not in scope. Copy fixes for status strings and empty surfaces are in scope.
- **Restructuring the underlying state stores** (Zustand stores in `apps/desktop/src/state/`) is not required. The redesign reads existing state shapes; if a new surface needs new state it gets called out in the implementation plan.

## 13. Acceptance criteria

A reasonable reviewer should be able to confirm:

- The mint `--color-brand` is no longer the primary CTA color anywhere; orange `#FF7A18` is the only filled primary button color in the app.
- The Index rail at the moment of "no indexers run yet" no longer reads `Missing` anywhere.
- Source review never shows a bare black rectangle when a source media item exists — the slate is present whenever there is an asset and no decoded proxy frame.
- Status pills app-wide instantiate `<StatusPill family={'job'|'proposal'} state=…>` — there is no surviving `kind="missing"` or `kind="reviewing"` pill in the codebase.
- The macOS window does not show the "Awidat" title duplicating the in-app wordmark.
- The landing surface contains exactly one filled accent-color button; all other actions are ghost or muted styles. (Objective replacement for "user can identify the primary action quickly.")
- A mode toggle in the chrome flips Index rail and Inspector rail densities without unmounting the underlying panels — verified by a React profiler trace showing no panel-level mount/unmount on toggle.
- The Tauri window has no native title bar on macOS; dragging the top chrome (away from interactive elements) moves the window.

## 14. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Orange as primary accent fights with the bright UI Awidat already has (proposal blues, accepted greens) | Orange is **only** for primary actions and the "running" job state. Selection stays cyan; ready stays mint/green. The action role is reserved. |
| Mode toggle confuses users ("which mode am I in?") | Mode pill is always visible in chrome row 1; current state is the active segment. No hidden mode switching from menus. |
| Migrating every pill call site is a long tail | Build the new `StatusPill` primitive and a temporary shim that maps old `kind` props to new `family + state`. Remove shim once call sites are migrated. |
| Film-slate empty state competes visually with real first frames | Slate uses charcoal stripes (no color) and cross-fades out in 200ms when the proxy is ready. The slate is intentionally dimmer than any real frame. |
| Deferred surfaces (Inspector / Timeline / Deliver) look broken during migration | Plan must land the foundation in one PR before any deferred surface ships; deferred surfaces get the new tokens at the foundation step, so they "absorb" the new palette and chrome without per-surface rework. |

## 15. References

- Brainstorm session artefacts: `.superpowers/brainstorm/33222-1779948202/content/`
  - `direction.html` — three design directions; chose Studio Pro
  - `index-rail.html` — Pro grid vs Creator summary
  - `chrome.html` — three chrome treatments; chose two-row editor
  - `empty-states.html` — three empty-state redesigns
  - `brand-mark.html` — three brand marks; chose M1 Studio
- Existing tokens: `apps/desktop/src/ui/tokens.css`
- Existing design concept (referenced by tokens.css): `~/Downloads/Awidat UI Design Concept.md`
- Screens audited (live walkthrough screenshots, 2026-05-28): landing, new-project modal, indexing source review, agent rail narrow, fully-indexed Edit view, Index panel close-up, Inspector panel close-up, Deliver view.
