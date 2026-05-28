# Desktop UI/UX Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the Studio Pro redesign defined in `docs/superpowers/specs/2026-05-28-desktop-ui-redesign-design.md`: collapse the 11-state pill taxonomy to two job/proposal families, demote mint to a liveness-only token and promote orange `#FF7A18` to the single primary action accent, rebuild the top chrome as a two-row editor strip, replace every black-void empty state with progress or a clear next move, refactor the 776-line `IndexingDashboard.tsx` into Pro-grid + Creator-summary surfaces driven by a new mode store, and ship a new `▲ + AWIDAT mono` brand mark at every icon size.

**Architecture:**
- One cohesive sequence, not parallel tracks: token + pill primitive land first so every downstream surface compiles against the new contract. Mode store + collapsible-panel `revealLevel` API land before the rails so the rails can wire to them in a single pass.
- No second component tree for Creator vs Pro — one shell, one set of panels, mode flips `defaultExpanded` only.
- No new test framework introduced. New pure-logic tests use the existing `node --experimental-strip-types` pattern from `apps/desktop/tests/*.test.ts`. UI surfaces are guarded by extending the existing Playwright `tests/desktop-ui-smoke.mjs` smoke (which already drives the dev shell at `:1420`).

**Tech Stack:** React 19, Tailwind 4 (with `@theme` token block), Zustand 5, Radix UI primitives, Tauri v2, class-variance-authority for component variants, Playwright (smoke only). All existing — no new deps.

---

## Worktree

Before Task 1, create an isolated worktree using `superpowers:using-git-worktrees`. All work happens on a single branch (default name: `redesign/studio-pro`). The plan does not split into multiple branches; the user decides PR boundaries when reviewing (a natural split is Phase A foundation → one PR, Phases B–E → a second PR).

---

## File Structure

**New files to be created:**
- `apps/desktop/src/ui/primitives/StatusPill.tsx` — replacement pill primitive with `family + state` API
- `apps/desktop/src/state/mode.ts` — Zustand `useMode()` store, persisted
- `apps/desktop/src/ui/primitives/CollapsiblePanel.tsx` — shared panel primitive with `revealLevel` API (extracted from existing inline collapsibles if any)
- `apps/desktop/src/shell/chrome/TopChrome.tsx` — new two-row chrome (split out of `AppShell.tsx`)
- `apps/desktop/src/shell/chrome/IdentityRow.tsx` — row 1: mark, wordmark, project pill, path, mode pill, share, settings
- `apps/desktop/src/shell/chrome/WorkspaceRow.tsx` — row 2: tabs + live timecode
- `apps/desktop/src/shell/chrome/Footer.tsx` — rebuilt footer (replaces `JobsStatusBar.tsx`)
- `apps/desktop/src/shell/empty/Landing.tsx` — no-project landing surface
- `apps/desktop/src/shell/empty/FilmSlate.tsx` — loading slate for `PreviewSurface`
- `apps/desktop/src/agent/EmptyConversation.tsx` — opener card + starter prompts when chat is empty
- `apps/desktop/src/agent/starterPrompts.ts` — `project_type → prompts[]` map (pure data + lookup)
- `apps/desktop/src/shell/IndexRailPro.tsx` — Pro mode dense Index rail (extracted from `IndexingDashboard.tsx`)
- `apps/desktop/src/shell/IndexRailCreator.tsx` — Creator mode summary Index rail
- `apps/desktop/src/shell/IndexRail.tsx` — thin selector that picks Pro vs Creator based on `useMode()`
- `apps/desktop/src/brand/awidat-mark.svg` — new triangle mark master
- `apps/desktop/src-tauri/icons/iconset/<sizes>.png` — regenerated icon set
- `apps/desktop/tests/status-pill.test.ts` — pure-logic tests for label/percent rendering
- `apps/desktop/tests/mode-store.test.ts` — pure-logic tests for mode toggle + persistence
- `apps/desktop/tests/starter-prompts.test.ts` — project_type lookup tests
- `apps/desktop/tests/tokens-presence.mjs` — node script asserting the renamed tokens exist in `tokens.css`

**Existing files to be modified:**
- `apps/desktop/src/ui/tokens.css` — rebrand + pill family rewrite (§5 of spec)
- `apps/desktop/src/ui/primitives/Pill.tsx` — deleted at end of Task 4 once call sites have migrated
- `apps/desktop/src/shell/AppShell.tsx` — extracted chrome moves out, replaced with `<TopChrome />`
- `apps/desktop/src/shell/JobsStatusBar.tsx` — deleted in favor of new `Footer.tsx`
- `apps/desktop/src/shell/IndexingDashboard.tsx` — split into IndexRailPro/Creator (the 776-line file is the largest in the area and exceeds the 300–500 LOC guideline)
- `apps/desktop/src/shell/PreviewSurface.tsx` — surface the `<FilmSlate>` when no decoded proxy frame yet
- `apps/desktop/src/agent/ChatStream.tsx` — render `<EmptyConversation />` when message list is empty
- `apps/desktop/src/inspector/ClipInspector.tsx` — wire sections through `<CollapsiblePanel revealLevel="advanced">` so Creator mode collapses them
- `apps/desktop/src-tauri/tauri.conf.json` — add `titleBarStyle: "Overlay"` for macOS, refresh icon manifest
- `apps/desktop/tests/desktop-ui-smoke.mjs` — extend with assertions for the new surfaces
- `apps/desktop/package.json` — register the new test scripts under the `test:*` family

---

## Task 1: Branch, baseline, and verify the build

**Files:**
- Modify: none (verification only)

- [ ] **Step 1: Create the worktree / branch**

Use `superpowers:using-git-worktrees` to create an isolated worktree on branch `redesign/studio-pro` from `main`. All subsequent file paths in this plan are relative to that worktree's `apps/desktop/` directory unless absolute.

- [ ] **Step 2: Verify the baseline desktop dev build still works**

Run from the worktree root:
```bash
cd apps/desktop && pnpm install
make desktop &        # backgrounded; ~5 min cold cargo build
```

Wait for the Tauri window to open. Visually confirm: chrome with mint `▲`, Edit/Deliver tabs, the dashboard you screenshotted on 2026-05-28. Quit the app (`⌘Q`) once verified.

Expected: the dev shell opens, no console errors, screenshots match the "before" state in the spec.

- [ ] **Step 3: Run all existing tests as a baseline**

```bash
cd apps/desktop && pnpm test
```

Record which tests pass. They must all still pass at the end of each subsequent task — if one breaks, that's the task's regression, not preexisting.

Expected: existing `test:animation`, `test:editor`, `test:timeline`, `test:play-segments`, and `desktop-ui-smoke.mjs` complete successfully.

- [ ] **Step 4: Commit the worktree setup (if anything was generated)**

Most likely nothing changed; skip if `git status` is clean.

---

## Task 2: Token revisions (`tokens.css`)

**Files:**
- Modify: `apps/desktop/src/ui/tokens.css`
- Create: `apps/desktop/tests/tokens-presence.mjs`
- Modify: `apps/desktop/package.json` (add `test:tokens` script, wire into `test`)

This is the foundation. Every later task assumes these tokens exist. After this task lands, the visual app will look subtly different everywhere (mint primary becomes orange wherever `var(--color-brand)` is consumed) — that's expected and will be fully reconciled in Task 5.

- [ ] **Step 1: Write the failing token-presence test**

Create `apps/desktop/tests/tokens-presence.mjs`:

```javascript
#!/usr/bin/env node
/**
 * Asserts that the renamed brand + pill family tokens are present in
 * tokens.css after the redesign migration. Fails loudly if a token is
 * dropped without an explicit replacement.
 */
import { readFileSync } from "node:fs";
import { strict as assert } from "node:assert";

const css = readFileSync(new URL("../src/ui/tokens.css", import.meta.url), "utf8");

const requiredTokens = [
  // brand
  "--color-brand: #FF7A18",
  "--color-brand-hover: #FF8B33",
  "--color-brand-active: #E5641A",
  "--color-accent-mint: #20C997",
  // job family
  "--color-job-idle-dot",
  "--color-job-running-dot",
  "--color-job-ready-dot",
  "--color-job-failed-dot",
  "--color-job-idle-fill",
  "--color-job-running-fill",
  "--color-job-ready-fill",
  "--color-job-failed-fill",
  "--color-job-idle-text",
  "--color-job-running-text",
  "--color-job-ready-text",
  "--color-job-failed-text",
  // proposal family
  "--color-proposal-proposed-dot",
  "--color-proposal-accepted-dot",
  "--color-proposal-rejected-dot",
  "--color-proposal-revised-dot",
];
const removedTokens = [
  // the dropped 11-family triplets — must NOT appear
  "--color-pill-proposed-fill",
  "--color-pill-pending-fill",
  "--color-pill-reviewing-fill",
  "--color-pill-missing-fill",
];

for (const t of requiredTokens) {
  assert.ok(css.includes(t), `tokens.css missing required token: ${t}`);
}
for (const t of removedTokens) {
  assert.ok(!css.includes(t), `tokens.css still contains removed token: ${t}`);
}

console.log(`tokens-presence: OK (${requiredTokens.length} required, ${removedTokens.length} removed verified)`);
```

- [ ] **Step 2: Wire the new test into `package.json`**

Edit `apps/desktop/package.json`. Find the `scripts` block and add:

```json
"test:tokens": "node tests/tokens-presence.mjs",
```

Update the `test` aggregate script to include `test:tokens` before the smoke step. Example:

```json
"test": "npm run test:tokens && npm run test:animation && npm run test:editor && npm run test:timeline && npm run test:play-segments && node tests/desktop-ui-smoke.mjs",
```

- [ ] **Step 3: Run the test to confirm it fails**

```bash
cd apps/desktop && npm run test:tokens
```

Expected: assertion failure on the first required token (e.g. `tokens.css missing required token: --color-brand: #FF7A18`).

- [ ] **Step 4: Apply the token revisions to `tokens.css`**

Open `apps/desktop/src/ui/tokens.css`. Apply the following edits inside the existing `@theme { … }` block:

**Replace the brand block** (currently lines around `--color-brand: #20C997;`):

```css
  /* brand — Studio orange is the single primary action accent. Mint
     is demoted to liveness indicators only (agent online, daemon
     connected) and lives at --color-accent-mint. See spec §5.1. */
  --color-brand: #FF7A18;
  --color-brand-hover: #FF8B33;
  --color-brand-active: #E5641A;

  --color-accent-mint: #20C997;
  --color-accent-mint-hover: #2EE6AE;
  --color-accent-mint-active: #17A77E;
```

**Delete the entire `/* status pills — fill / border / text / dot */` block** (the 11 families: proposed/pending/accepted/rejected/reviewing/processing/ready/failed/warning/missing/revised). Replace with:

```css
  /* status pills — two families × 4 states. See spec §5.2.
     Job lifecycle: indexers, jobs, proxy builds, exports.
     Proposal lifecycle: agent-generated edits, human-arbitrated. */

  /* job × idle */
  --color-job-idle-fill: rgba(75, 75, 82, 0.20);
  --color-job-idle-border: rgba(120, 120, 128, 0.30);
  --color-job-idle-text: #B4B4B8;
  --color-job-idle-dot: #3F3F46;

  /* job × running (orange = the action accent) */
  --color-job-running-fill: rgba(255, 122, 24, 0.10);
  --color-job-running-border: rgba(255, 122, 24, 0.40);
  --color-job-running-text: #FCA67A;
  --color-job-running-dot: #FF7A18;

  /* job × ready (true green, distinct from mint) */
  --color-job-ready-fill: rgba(34, 197, 94, 0.10);
  --color-job-ready-border: rgba(34, 197, 94, 0.40);
  --color-job-ready-text: #86EFAC;
  --color-job-ready-dot: #22C55E;

  /* job × failed */
  --color-job-failed-fill: rgba(239, 68, 68, 0.12);
  --color-job-failed-border: rgba(239, 68, 68, 0.45);
  --color-job-failed-text: #FCA5A5;
  --color-job-failed-dot: #EF4444;

  /* proposal × proposed (cyan = awaiting human) */
  --color-proposal-proposed-fill: rgba(56, 189, 248, 0.12);
  --color-proposal-proposed-border: rgba(56, 189, 248, 0.45);
  --color-proposal-proposed-text: #7DD3FC;
  --color-proposal-proposed-dot: #38BDF8;

  /* proposal × accepted (shares green with job-ready: a positive completion) */
  --color-proposal-accepted-fill: rgba(34, 197, 94, 0.10);
  --color-proposal-accepted-border: rgba(34, 197, 94, 0.40);
  --color-proposal-accepted-text: #86EFAC;
  --color-proposal-accepted-dot: #22C55E;

  /* proposal × rejected */
  --color-proposal-rejected-fill: rgba(239, 68, 68, 0.12);
  --color-proposal-rejected-border: rgba(239, 68, 68, 0.45);
  --color-proposal-rejected-text: #FCA5A5;
  --color-proposal-rejected-dot: #EF4444;

  /* proposal × revised (purple — human modified) */
  --color-proposal-revised-fill: rgba(168, 85, 247, 0.12);
  --color-proposal-revised-border: rgba(168, 85, 247, 0.45);
  --color-proposal-revised-text: #D8B4FE;
  --color-proposal-revised-dot: #A855F7;
```

- [ ] **Step 5: Run the token test to confirm it passes**

```bash
cd apps/desktop && npm run test:tokens
```

Expected: `tokens-presence: OK (… required, … removed verified)`.

- [ ] **Step 6: Type-check the project**

```bash
cd apps/desktop && pnpm tsc --noEmit
```

Expected: tsc passes. (CSS errors don't surface here, but any TS that referenced a CSS module string would.)

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/ui/tokens.css apps/desktop/tests/tokens-presence.mjs apps/desktop/package.json
git commit -m "redesign(tokens): demote mint, promote orange, collapse pill families"
```

---

## Task 3: New `StatusPill` primitive

**Files:**
- Create: `apps/desktop/src/ui/primitives/StatusPill.tsx`
- Create: `apps/desktop/tests/status-pill.test.ts`
- Modify: `apps/desktop/package.json` (add `test:status-pill`)

- [ ] **Step 1: Write the failing test**

Create `apps/desktop/tests/status-pill.test.ts`:

```typescript
/**
 * Pure-logic tests for the StatusPill primitive. We don't render the
 * React tree — we exercise the label/percent computation that drives it.
 */
import { strict as assert } from "node:assert";
import { resolveStatusLabel } from "../src/ui/primitives/StatusPill.ts";

// Default labels per family/state
assert.equal(resolveStatusLabel({ family: "job", state: "idle" }), "Idle");
assert.equal(resolveStatusLabel({ family: "job", state: "running" }), "Running");
assert.equal(resolveStatusLabel({ family: "job", state: "ready" }), "Ready");
assert.equal(resolveStatusLabel({ family: "job", state: "failed" }), "Failed");
assert.equal(resolveStatusLabel({ family: "proposal", state: "proposed" }), "Proposed");
assert.equal(resolveStatusLabel({ family: "proposal", state: "accepted" }), "Accepted");
assert.equal(resolveStatusLabel({ family: "proposal", state: "rejected" }), "Rejected");
assert.equal(resolveStatusLabel({ family: "proposal", state: "revised" }), "Revised");

// Custom label override wins
assert.equal(
  resolveStatusLabel({ family: "job", state: "running", label: "Indexing" }),
  "Indexing",
);

// Percent appends to running label
assert.equal(
  resolveStatusLabel({ family: "job", state: "running", percent: 56 }),
  "Running · 56%",
);
assert.equal(
  resolveStatusLabel({ family: "job", state: "running", label: "Indexing", percent: 56 }),
  "Indexing · 56%",
);

// Percent on non-running is a type error at compile time AND ignored at runtime
// (we test runtime; the type system enforces the rest)
assert.equal(
  resolveStatusLabel({ family: "job", state: "ready", percent: 100 } as any),
  "Ready",
  "percent must be ignored when state !== running",
);

// Percent clamping
assert.equal(resolveStatusLabel({ family: "job", state: "running", percent: -5 }), "Running · 0%");
assert.equal(resolveStatusLabel({ family: "job", state: "running", percent: 1000 }), "Running · 100%");
assert.equal(resolveStatusLabel({ family: "job", state: "running", percent: 56.7 }), "Running · 57%");

console.log("status-pill: OK");
```

Wire into `package.json`:

```json
"test:status-pill": "node --experimental-strip-types tests/status-pill.test.ts",
```

And add it to the `test` aggregate alongside `test:tokens`.

- [ ] **Step 2: Run test to verify it fails**

```bash
cd apps/desktop && npm run test:status-pill
```

Expected: `Error [ERR_MODULE_NOT_FOUND]: Cannot find module '…/StatusPill.ts'` (or similar — the file doesn't exist yet).

- [ ] **Step 3: Implement the primitive**

Create `apps/desktop/src/ui/primitives/StatusPill.tsx`:

```tsx
import type { HTMLAttributes } from "react";
import { cva } from "class-variance-authority";
import { cn } from "../cn";

export type JobState = "idle" | "running" | "ready" | "failed";
export type ProposalState = "proposed" | "accepted" | "rejected" | "revised";

/**
 * Discriminated union so `percent` is only valid on `family: 'job', state: 'running'`.
 * TypeScript enforces this at every call site; runtime double-checks (see resolveStatusLabel).
 */
export type StatusPillProps =
  | {
      family: "job";
      state: "running";
      label?: string;
      percent?: number;
      dotOnly?: boolean;
      size?: "sm" | "md";
    }
  | {
      family: "job";
      state: Exclude<JobState, "running">;
      label?: string;
      percent?: never;
      dotOnly?: boolean;
      size?: "sm" | "md";
    }
  | {
      family: "proposal";
      state: ProposalState;
      label?: string;
      percent?: never;
      dotOnly?: boolean;
      size?: "sm" | "md";
    };

const DEFAULT_LABELS: Record<"job" | "proposal", Record<string, string>> = {
  job: { idle: "Idle", running: "Running", ready: "Ready", failed: "Failed" },
  proposal: { proposed: "Proposed", accepted: "Accepted", rejected: "Rejected", revised: "Revised" },
};

/** Pure label/percent resolution — testable without rendering. */
export function resolveStatusLabel(
  opts: { family: "job" | "proposal"; state: string; label?: string; percent?: number },
): string {
  const base = opts.label ?? DEFAULT_LABELS[opts.family]?.[opts.state] ?? opts.state;
  if (opts.state !== "running" || opts.percent === undefined) return base;
  const clamped = Math.max(0, Math.min(100, Math.round(opts.percent)));
  return `${base} · ${clamped}%`;
}

const pill = cva(
  [
    "inline-flex items-center gap-1.5",
    "rounded-full border",
    "font-semibold",
    "whitespace-nowrap",
  ],
  {
    variants: {
      family: { job: "", proposal: "" },
      state: {
        idle: "",
        running: "",
        ready: "",
        failed: "",
        proposed: "",
        accepted: "",
        rejected: "",
        revised: "",
      },
      size: {
        sm: "h-4 px-1.5 text-[10px] leading-none",
        md: "h-[18px] px-2 text-[11px] leading-none",
      },
    },
    compoundVariants: [
      { family: "job", state: "idle", className: "bg-[var(--color-job-idle-fill)] border-[var(--color-job-idle-border)] text-[var(--color-job-idle-text)]" },
      { family: "job", state: "running", className: "bg-[var(--color-job-running-fill)] border-[var(--color-job-running-border)] text-[var(--color-job-running-text)]" },
      { family: "job", state: "ready", className: "bg-[var(--color-job-ready-fill)] border-[var(--color-job-ready-border)] text-[var(--color-job-ready-text)]" },
      { family: "job", state: "failed", className: "bg-[var(--color-job-failed-fill)] border-[var(--color-job-failed-border)] text-[var(--color-job-failed-text)]" },
      { family: "proposal", state: "proposed", className: "bg-[var(--color-proposal-proposed-fill)] border-[var(--color-proposal-proposed-border)] text-[var(--color-proposal-proposed-text)]" },
      { family: "proposal", state: "accepted", className: "bg-[var(--color-proposal-accepted-fill)] border-[var(--color-proposal-accepted-border)] text-[var(--color-proposal-accepted-text)]" },
      { family: "proposal", state: "rejected", className: "bg-[var(--color-proposal-rejected-fill)] border-[var(--color-proposal-rejected-border)] text-[var(--color-proposal-rejected-text)]" },
      { family: "proposal", state: "revised", className: "bg-[var(--color-proposal-revised-fill)] border-[var(--color-proposal-revised-border)] text-[var(--color-proposal-revised-text)]" },
    ],
    defaultVariants: { size: "md", family: "job", state: "idle" },
  },
);

const dot = cva("h-1.5 w-1.5 shrink-0 rounded-full", {
  variants: {
    family: { job: "", proposal: "" },
    state: {
      idle: "", running: "", ready: "", failed: "",
      proposed: "", accepted: "", rejected: "", revised: "",
    },
  },
  compoundVariants: [
    { family: "job", state: "idle", className: "bg-[var(--color-job-idle-dot)]" },
    { family: "job", state: "running", className: "bg-[var(--color-job-running-dot)] shadow-[0_0_6px_rgba(255,122,24,0.6)]" },
    { family: "job", state: "ready", className: "bg-[var(--color-job-ready-dot)]" },
    { family: "job", state: "failed", className: "bg-[var(--color-job-failed-dot)]" },
    { family: "proposal", state: "proposed", className: "bg-[var(--color-proposal-proposed-dot)]" },
    { family: "proposal", state: "accepted", className: "bg-[var(--color-proposal-accepted-dot)]" },
    { family: "proposal", state: "rejected", className: "bg-[var(--color-proposal-rejected-dot)]" },
    { family: "proposal", state: "revised", className: "bg-[var(--color-proposal-revised-dot)]" },
  ],
});

export function StatusPill(props: StatusPillProps & HTMLAttributes<HTMLSpanElement>) {
  const { family, state, label, percent, dotOnly = false, size = "md", className, ...rest } = props as any;
  const text = resolveStatusLabel({ family, state, label, percent });

  if (dotOnly) {
    return <span className={cn(dot({ family, state }), className)} aria-label={text} {...rest} />;
  }
  return (
    <span className={cn(pill({ family, state, size }), className)} {...rest}>
      <span className={dot({ family, state })} aria-hidden />
      {text}
    </span>
  );
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cd apps/desktop && npm run test:status-pill
```

Expected: `status-pill: OK`.

- [ ] **Step 5: Type-check**

```bash
cd apps/desktop && pnpm tsc --noEmit
```

Expected: pass. The discriminated union should compile without errors.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/ui/primitives/StatusPill.tsx apps/desktop/tests/status-pill.test.ts apps/desktop/package.json
git commit -m "redesign(pill): add StatusPill primitive with job+proposal families"
```

---

## Task 4: Migrate every `<Pill>` call site to `<StatusPill>` and delete the old primitive

**Files:**
- Modify: every file using `<Pill>` from `apps/desktop/src/ui/primitives/Pill.tsx`
- Delete: `apps/desktop/src/ui/primitives/Pill.tsx`

The mapping from old `status=` values to new `family + state`:

| Old `status` | New `family` | New `state` | Notes |
|---|---|---|---|
| `proposed` | proposal | proposed | |
| `pending` | job | idle | "Pending" was a softer "idle" — fold in |
| `accepted` | proposal | accepted | |
| `rejected` | proposal | rejected | |
| `reviewing` | proposal | proposed | Reviewing = "awaiting human" = proposed |
| `processing` | job | running | Add `percent` if call site has one |
| `ready` | job | ready | |
| `failed` | job | failed | |
| `warning` | job | failed | Lossy: warning becomes failed visually. If a call site truly means "warn not fail," use `state="running"` with `label="Warning"` — discuss in PR review if any survive. |
| `missing` | job | idle | "Missing" → `state="idle"`, optional `label="Not yet run"` |
| `revised` | proposal | revised | |
| `neutral` | — | — | Replace with plain text or `dotOnly` per call site |

- [ ] **Step 1: List every call site**

```bash
cd apps/desktop && grep -rEln '\bPill\b|from .*primitives/Pill' src/ | sort -u
```

Record the resulting file list. There should be a handful (per scouting, at least `src/ui/components/ProposalCard.tsx`, `src/ui/components/MediaStatusRow.tsx`, plus likely uses in shell + agent).

- [ ] **Step 2: For each file, migrate inline**

For each file from Step 1:
1. Open the file
2. Replace `import { Pill } from "…/Pill";` with `import { StatusPill } from "…/StatusPill";`
3. For each JSX usage, apply the mapping table above. Example:

   ```tsx
   /* before */ <Pill status="missing">Missing</Pill>
   /* after  */ <StatusPill family="job" state="idle" label="Not yet run" />
   ```
   
   ```tsx
   /* before */ <Pill status="processing">Indexing 56%</Pill>
   /* after  */ <StatusPill family="job" state="running" percent={56} label="Indexing" />
   ```

4. Save.

- [ ] **Step 3: Type-check after each file**

```bash
cd apps/desktop && pnpm tsc --noEmit
```

Expected: pass after each file. If a call site can't be expressed cleanly in the new API, comment the migration with `// TODO(redesign): clarify — original status="warning"` and proceed; collect for review at the end.

- [ ] **Step 4: Delete the old primitive**

Once `grep -rEln 'from .*primitives/Pill[^A-Za-z]' src/` returns no results:

```bash
git rm apps/desktop/src/ui/primitives/Pill.tsx
```

- [ ] **Step 5: Run full tests**

```bash
cd apps/desktop && pnpm test
```

Expected: all tests pass. The smoke test will catch any visual regressions in the rendered chrome.

- [ ] **Step 6: Commit**

```bash
git add -u apps/desktop/src
git commit -m "redesign(pill): migrate all call sites to StatusPill; remove old Pill"
```

---

## Task 5: Brand-color sweep — audit every `var(--color-brand)` consumer

After Task 2, `--color-brand` evaluates to orange `#FF7A18` everywhere. This task makes that change deliberate per call site. Some sites should keep orange (true CTAs), some should switch to `--color-accent-mint` (liveness), some to `--color-job-ready-dot` (completion).

**Files:**
- Modify: any file in `apps/desktop/src/**/*.{ts,tsx,css}` referencing `--color-brand` (without the `-secondary`, `-purple`, `-hover`, etc. suffix — those are intentional separate tokens)

- [ ] **Step 1: List every `--color-brand` consumer**

```bash
cd apps/desktop && grep -rEn 'var\(--color-brand[^-]' src/ | tee /tmp/brand-sweep.txt
```

Read `/tmp/brand-sweep.txt`. For each line, decide one of three actions:
- **Keep orange** — it's a primary action (buttons, primary CTAs)
- **Move to mint** — it's a liveness indicator ("agent online" dot, daemon-connected glyph)
- **Move to success green** — it's a completion state ("ready" pills, accepted checkmarks)

- [ ] **Step 2: For each file with a relevant match, apply the decision**

Apply edits manually per call site. Examples of common patterns:

```tsx
/* "agent online" dot — currently brand, should be mint */
- <span className="bg-[var(--color-brand)]" />
+ <span className="bg-[var(--color-accent-mint)]" />

/* "ready" status pill — currently brand, should be green */
- <span className="text-[var(--color-brand)]">Ready</span>
+ <StatusPill family="job" state="ready" />   /* if it's the StatusPill, already handled in Task 4 */

/* primary "Save" button — currently brand, stays brand (orange) */
- <button className="bg-[var(--color-brand)] hover:bg-[var(--color-brand-hover)]">
+ /* no change — orange is correct here */
```

- [ ] **Step 3: Audit the design surfaces visually**

Boot the dev shell:

```bash
make desktop &
```

Walk through each main surface (Edit empty, Edit with media, Deliver, modal). For each, confirm:
- Buttons that should be primary actions are orange.
- "Agent online" indicator is mint.
- "Ready" / "Accepted" states are green (not orange, not mint).
- No surface accidentally turns orange because of an undecided `var(--color-brand)` use.

Take a screenshot of each surface; save under `/tmp/redesign-task5/<surface>.png` for the PR description.

- [ ] **Step 4: Run all tests**

```bash
cd apps/desktop && pnpm test
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add -u apps/desktop/src
git commit -m "redesign(brand): per-site reclassification of --color-brand consumers"
```

---

## Task 6: Mode store (`useMode`)

**Files:**
- Create: `apps/desktop/src/state/mode.ts`
- Create: `apps/desktop/tests/mode-store.test.ts`
- Modify: `apps/desktop/package.json` (add `test:mode-store`)

- [ ] **Step 1: Write the failing test**

Create `apps/desktop/tests/mode-store.test.ts`:

```typescript
import { strict as assert } from "node:assert";
import { createModeStore, type Mode } from "../src/state/mode.ts";

// fresh store defaults to "pro" (existing users are editors; the spec
// defines pro as the heritage default, creator as the opinionated alt)
{
  const store = createModeStore({ persist: false });
  assert.equal(store.get(), "pro");
}

// toggle flips
{
  const store = createModeStore({ persist: false });
  store.set("creator");
  assert.equal(store.get(), "creator");
  store.toggle();
  assert.equal(store.get(), "pro");
  store.toggle();
  assert.equal(store.get(), "creator");
}

// persistence round-trips through the provided storage adapter
{
  const fake: Record<string, string> = {};
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: (k: string, v: string) => { fake[k] = v; },
    removeItem: (k: string) => { delete fake[k]; },
  };
  const a = createModeStore({ persist: true, storage });
  a.set("creator");
  // simulate process restart — new store reads from same storage
  const b = createModeStore({ persist: true, storage });
  assert.equal(b.get(), "creator");
}

// invalid persisted value falls back to default
{
  const fake: Record<string, string> = { "awidat:mode": "garbage" };
  const storage = {
    getItem: (k: string) => fake[k] ?? null,
    setItem: () => {},
    removeItem: () => {},
  };
  const s = createModeStore({ persist: true, storage });
  assert.equal(s.get(), "pro");
}

// subscribe receives updates
{
  const store = createModeStore({ persist: false });
  const received: Mode[] = [];
  const unsub = store.subscribe((m) => received.push(m));
  store.set("creator");
  store.set("pro");
  unsub();
  store.set("creator");          // not received after unsubscribe
  assert.deepEqual(received, ["creator", "pro"]);
}

console.log("mode-store: OK");
```

Wire into `package.json`:

```json
"test:mode-store": "node --experimental-strip-types tests/mode-store.test.ts",
```

Add to `test` aggregate.

- [ ] **Step 2: Run test to verify it fails**

```bash
cd apps/desktop && npm run test:mode-store
```

Expected: module-not-found.

- [ ] **Step 3: Implement the store**

Create `apps/desktop/src/state/mode.ts`:

```typescript
import { create } from "zustand";

export type Mode = "pro" | "creator";
export const STORAGE_KEY = "awidat:mode";

const isValid = (v: unknown): v is Mode => v === "pro" || v === "creator";

interface StorageAdapter {
  getItem(k: string): string | null;
  setItem(k: string, v: string): void;
  removeItem(k: string): void;
}

interface ModeStore {
  get: () => Mode;
  set: (next: Mode) => void;
  toggle: () => void;
  subscribe: (cb: (mode: Mode) => void) => () => void;
}

interface CreateOpts {
  persist?: boolean;
  storage?: StorageAdapter;
  defaultMode?: Mode;
}

/**
 * Framework-agnostic mode store. The Zustand React hook wraps this so the
 * core logic is testable under plain node.
 */
export function createModeStore(opts: CreateOpts = {}): ModeStore {
  const persist = opts.persist ?? true;
  const storage: StorageAdapter | null = persist
    ? opts.storage ?? (typeof localStorage !== "undefined" ? localStorage : null)
    : null;
  const defaultMode: Mode = opts.defaultMode ?? "pro";

  let current: Mode = defaultMode;
  if (storage) {
    const raw = storage.getItem(STORAGE_KEY);
    if (raw && isValid(raw)) current = raw;
  }

  const subscribers = new Set<(m: Mode) => void>();

  return {
    get: () => current,
    set: (next) => {
      if (!isValid(next) || next === current) return;
      current = next;
      storage?.setItem(STORAGE_KEY, next);
      subscribers.forEach((cb) => cb(next));
    },
    toggle: function () {
      this.set(current === "pro" ? "creator" : "pro");
    },
    subscribe: (cb) => {
      subscribers.add(cb);
      return () => { subscribers.delete(cb); };
    },
  };
}

/** React-facing Zustand hook. */
interface UseModeState { mode: Mode; setMode: (m: Mode) => void; toggle: () => void; }
const _store = createModeStore();
export const useMode = create<UseModeState>((set) => ({
  mode: _store.get(),
  setMode: (m) => { _store.set(m); set({ mode: _store.get() }); },
  toggle:   () => { _store.toggle();  set({ mode: _store.get() }); },
}));
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cd apps/desktop && npm run test:mode-store
```

Expected: `mode-store: OK`.

- [ ] **Step 5: Type-check**

```bash
cd apps/desktop && pnpm tsc --noEmit
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/state/mode.ts apps/desktop/tests/mode-store.test.ts apps/desktop/package.json
git commit -m "redesign(mode): add useMode store with persistence"
```

---

## Task 7: Tauri title bar overlay + drag region scaffolding

**Files:**
- Modify: `apps/desktop/src-tauri/tauri.conf.json`

- [ ] **Step 1: Edit the window config**

Open `apps/desktop/src-tauri/tauri.conf.json`. Inside `app.windows[0]`, add:

```json
"titleBarStyle": "Overlay",
"hiddenTitle": true,
"trafficLightPosition": { "x": 12, "y": 14 }
```

So the windows entry looks like (preserving other fields):

```json
{
  "title": "Awidat",
  "width": 1280,
  "height": 800,
  "titleBarStyle": "Overlay",
  "hiddenTitle": true,
  "trafficLightPosition": { "x": 12, "y": 14 }
}
```

- [ ] **Step 2: Restart the dev shell to pick up the change**

```bash
make desktop-stop && make desktop &
```

When the Tauri window opens, confirm:
- The macOS title bar is gone (no "Awidat" caption at the top).
- The red/yellow/green traffic lights are positioned where you set them.
- The window can still be dragged — there are no drag-region declarations yet, so this may currently feel broken. That is expected. Task 8 attaches `data-tauri-drag-region` to the new top chrome.

- [ ] **Step 3: Commit**

```bash
git add apps/desktop/src-tauri/tauri.conf.json
git commit -m "redesign(chrome): enable Tauri overlay title bar on macOS"
```

---

## Task 8: Top chrome row 1 — `IdentityRow`

**Files:**
- Create: `apps/desktop/src/shell/chrome/IdentityRow.tsx`
- Create: `apps/desktop/src/shell/chrome/TopChrome.tsx` (skeleton — row 2 lands in Task 9)
- Modify: `apps/desktop/src/shell/AppShell.tsx` (mount `<TopChrome />` in place of the existing chrome region)

- [ ] **Step 1: Create the `IdentityRow` component**

Create `apps/desktop/src/shell/chrome/IdentityRow.tsx`:

```tsx
import wordmark from "../../brand/awidat-wordmark.svg";       // existing import path used elsewhere
import mark from "../../brand/awidat-mark.svg";              // NEW file — placeholder OK, finalized in Task 17
import { useMode } from "../../state/mode";
import { useProject } from "../../state";                     // existing project state — adjust if the export is named differently
import { Share2, Settings } from "lucide-react";

export function IdentityRow() {
  const { mode, toggle } = useMode();
  const project = useProject((s) => s.current);  // expected shape: { name: string, path: string } | null

  return (
    <div
      data-tauri-drag-region
      className="flex items-center justify-between gap-3 h-9 pl-[88px] pr-3 select-none"
      // pl-[88px] reserves space for the macOS traffic lights at x:12 + ~70px gap
    >
      <div className="flex items-center gap-2 min-w-0" data-tauri-drag-region={false}>
        <img src={mark} alt="" width={18} height={18} />
        <span className="font-mono font-semibold tracking-[0.08em] text-[11px] text-[var(--color-text-primary)]">
          AWIDAT
        </span>
        {project && (
          <>
            <span className="text-[var(--color-text-disabled)]">·</span>
            <span className="font-semibold text-[13px] text-[var(--color-text-primary)] truncate">
              {project.name}
            </span>
            <span className="font-mono text-[11px] text-[var(--color-text-muted)] truncate">
              {project.path}
            </span>
          </>
        )}
      </div>
      <div className="flex items-center gap-2" data-tauri-drag-region={false}>
        <ModePill mode={mode} onToggle={toggle} />
        <IconBtn label="Share"><Share2 size={14} /></IconBtn>
        <IconBtn label="Settings"><Settings size={14} /></IconBtn>
      </div>
    </div>
  );
}

function ModePill({ mode, onToggle }: { mode: "pro" | "creator"; onToggle: () => void }) {
  return (
    <button
      onClick={onToggle}
      className="inline-flex items-center gap-0.5 px-1 py-0.5 rounded-full border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] text-[11px] font-semibold text-[var(--color-text-muted)]"
      aria-label={`Switch to ${mode === "pro" ? "Creator" : "Pro"} mode`}
    >
      <span className={pillSegment(mode === "pro")}>Pro</span>
      <span className={pillSegment(mode === "creator")}>Creator</span>
    </button>
  );
}
const pillSegment = (active: boolean) =>
  active
    ? "px-2 py-0.5 rounded-full bg-[var(--color-surface-selected)] text-[var(--color-text-primary)] shadow-[inset_0_0_0_1px_var(--color-border)]"
    : "px-2 py-0.5 rounded-full";

function IconBtn({ children, label }: { children: React.ReactNode; label: string }) {
  return (
    <button
      aria-label={label}
      className="grid place-items-center w-6 h-6 rounded border border-transparent text-[var(--color-text-muted)] hover:border-[var(--color-border-subtle)] hover:bg-[var(--color-surface-hover)] hover:text-[var(--color-text-primary)]"
    >
      {children}
    </button>
  );
}
```

If `useProject` does not export with this exact shape, find the actual project store in `apps/desktop/src/state/` and adapt the destructuring. The component MUST handle `project === null` gracefully (just the brand half on the left).

- [ ] **Step 2: Create a placeholder mark SVG**

Create `apps/desktop/src/brand/awidat-mark.svg`. Simple equilateral triangle, finalized in Task 17 — for now:

```xml
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40" width="40" height="40">
  <rect width="40" height="40" rx="10" fill="#0F1110" stroke="#2A2C2B" stroke-width="1"/>
  <polygon points="20,10 30,28 10,28" fill="#FF7A18"/>
</svg>
```

- [ ] **Step 3: Create the `TopChrome` skeleton**

Create `apps/desktop/src/shell/chrome/TopChrome.tsx`:

```tsx
import { IdentityRow } from "./IdentityRow";

export function TopChrome() {
  return (
    <div className="flex flex-col border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-app)]">
      <IdentityRow />
      {/* WorkspaceRow lands in Task 9 */}
    </div>
  );
}
```

- [ ] **Step 4: Mount `<TopChrome />` in `AppShell.tsx`**

Open `apps/desktop/src/shell/AppShell.tsx`. Find the current chrome JSX (the brand wordmark image + the Edit/Deliver tabs + the right-side status). Replace it with:

```tsx
import { TopChrome } from "./chrome/TopChrome";
…
<TopChrome />
```

Leave the rest of the shell layout intact for now; the workspace row is wired in Task 9, the footer in Task 10.

- [ ] **Step 5: Boot the dev shell and verify**

```bash
make desktop-stop && make desktop &
```

When the window opens:
- The new identity row should be visible: traffic lights at x=12, the ▲ mark, the AWIDAT wordmark in mono caps, a project name + path if a project is open, mode pill on the right with `Pro` highlighted, share + settings ghost icons.
- Drag the window from the empty space in the identity row — it should move (the `data-tauri-drag-region` is working).
- Clicking the mark, wordmark, or mode pill should NOT drag the window (drag opt-out is working).
- Clicking the mode pill should toggle the highlighted segment between Pro and Creator. Restart the app; the choice should persist.

Take screenshots; save to `/tmp/redesign-task8/`.

- [ ] **Step 6: Run tests**

```bash
cd apps/desktop && pnpm test
```

The smoke test will likely fail because it checks for the OLD chrome strings. Update `tests/desktop-ui-smoke.mjs` `check(…)` blocks that match the old chrome (e.g., `assert.ok(page.locator('text=Awidat'))`) to look for `AWIDAT` (caps) or the new `data-testid="top-chrome"` if you choose to add one. Re-run until the smoke passes.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/shell/chrome/ apps/desktop/src/brand/awidat-mark.svg apps/desktop/src/shell/AppShell.tsx apps/desktop/tests/desktop-ui-smoke.mjs
git commit -m "redesign(chrome): identity row with mark, wordmark, project pill, mode toggle"
```

---

## Task 9: Top chrome row 2 — `WorkspaceRow` (tabs + live timecode)

**Files:**
- Create: `apps/desktop/src/shell/chrome/WorkspaceRow.tsx`
- Modify: `apps/desktop/src/shell/chrome/TopChrome.tsx` (mount the new row)
- Modify: `apps/desktop/src/shell/AppShell.tsx` (remove the old tab strip and pass workspace state down)

- [ ] **Step 1: Identify where the current Edit/Deliver tab state lives**

```bash
cd apps/desktop && grep -rEn 'workspace|"Edit"|"Deliver"' src/shell/AppShell.tsx src/state/
```

Identify the state hook that drives the active workspace and the live timecode. They likely live in `state/index.ts` or `state/appGlue.ts`. Read the relevant store to learn the exact prop names. (Do not rename them.)

- [ ] **Step 2: Create `WorkspaceRow`**

Create `apps/desktop/src/shell/chrome/WorkspaceRow.tsx`:

```tsx
import { useWorkspace, usePreviewClock } from "../../state";   // replace with the actual hooks discovered in Step 1
import { cn } from "../../ui/cn";

const TABS = [
  { id: "edit",     label: "Edit",     kbd: "⌘1" },
  { id: "deliver",  label: "Deliver",  kbd: "⌘2" },
  // disabled placeholders kept visible to communicate scope:
  { id: "history",  label: "History",  kbd: "",   disabled: true },
  { id: "skills",   label: "Skills",   kbd: "",   disabled: true },
] as const;

export function WorkspaceRow() {
  const { active, setActive } = useWorkspace();          // expected: { active: 'edit'|'deliver', setActive(id) }
  const { current, total } = usePreviewClock();          // expected: { current: string, total: string } — both pre-formatted timecodes

  return (
    <div className="flex items-center justify-between h-[30px] px-3 border-t border-[var(--color-border-subtle)]">
      <div className="flex items-center gap-4">
        {TABS.map((t) => (
          <button
            key={t.id}
            disabled={t.disabled}
            onClick={() => !t.disabled && setActive(t.id)}
            className={cn(
              "relative h-[30px] -mb-px text-[12px] font-semibold",
              t.disabled
                ? "text-[var(--color-text-disabled)] cursor-not-allowed"
                : active === t.id
                ? "text-[var(--color-text-primary)]"
                : "text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]"
            )}
          >
            {t.label}
            {active === t.id && (
              <span className="absolute -bottom-px left-0 right-0 h-[2px] bg-[var(--color-brand)]" />
            )}
          </button>
        ))}
      </div>
      <div className="font-mono text-[12px] text-[var(--color-text-muted)] tabular-nums">
        {current} <span className="text-[var(--color-text-disabled)]">/</span> {total}
      </div>
    </div>
  );
}
```

If `usePreviewClock` doesn't exist with that exact shape, find the existing source of `00:00:00:00 / 00:00:38:17` in the codebase (likely in `src/timeline/` or `src/media/`) and wire it. If the timecode is not centrally available, expose a memoized hook that reads from whatever store currently feeds the transport bar.

- [ ] **Step 3: Mount in `TopChrome`**

Edit `apps/desktop/src/shell/chrome/TopChrome.tsx`:

```tsx
import { IdentityRow } from "./IdentityRow";
import { WorkspaceRow } from "./WorkspaceRow";

export function TopChrome() {
  return (
    <div className="flex flex-col border-b border-[var(--color-border-subtle)] bg-[var(--color-surface-app)]">
      <IdentityRow />
      <WorkspaceRow />
    </div>
  );
}
```

- [ ] **Step 4: Remove the leftover Edit/Deliver tab strip from `AppShell.tsx`**

Open `apps/desktop/src/shell/AppShell.tsx`. Find any remaining inline render of Edit/Deliver tabs (there may still be one above the body grid). Remove it. The chrome lives entirely in `<TopChrome />` now.

- [ ] **Step 5: Boot, verify, screenshot**

```bash
make desktop-stop && make desktop &
```

Open a project (or load the example) so a timeline exists. Verify:
- The workspace row sits directly under the identity row, 30px tall.
- "Edit" has the orange 2px underline; "Deliver" is muted.
- "History" and "Skills" are visible but disabled — confirm the spec is comfortable showing future scope; if not, remove them now.
- The live timecode updates at preview framerate.
- Clicking "Deliver" switches the workspace; "Edit" loses the underline and "Deliver" gains it.

- [ ] **Step 6: Update the smoke test**

Add to `tests/desktop-ui-smoke.mjs` a check that `Edit` and `Deliver` tabs exist as buttons and the timecode container is present.

- [ ] **Step 7: Run all tests**

```bash
cd apps/desktop && pnpm test
```

Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/src/shell/chrome/WorkspaceRow.tsx apps/desktop/src/shell/chrome/TopChrome.tsx apps/desktop/src/shell/AppShell.tsx apps/desktop/tests/desktop-ui-smoke.mjs
git commit -m "redesign(chrome): workspace row with tabs and live timecode"
```

---

## Task 10: Footer redesign (`Footer.tsx`)

**Files:**
- Create: `apps/desktop/src/shell/chrome/Footer.tsx`
- Modify: `apps/desktop/src/shell/AppShell.tsx` (swap `<JobsStatusBar />` → `<Footer />`)
- Delete: `apps/desktop/src/shell/JobsStatusBar.tsx`

- [ ] **Step 1: Read the current footer**

```bash
cd apps/desktop && cat src/shell/JobsStatusBar.tsx
```

Note what state hooks it reads (indexing progress, render queue, disk, autosave, throughput bars). The new footer reads the same state — same data, different layout.

- [ ] **Step 2: Build `Footer.tsx`**

Create `apps/desktop/src/shell/chrome/Footer.tsx`:

```tsx
import { StatusPill } from "../../ui/primitives/StatusPill";
import { useIndexing, useRender, useDisk, useAutosave } from "../../state";   // adapt to actual hooks

export function Footer() {
  const { running, ready, total, percent } = useIndexing();
  const { queueDepth } = useRender();
  const { freeBytes } = useDisk();
  const { lastSavedAt } = useAutosave();

  const indexing = running > 0;

  return (
    <div className="flex items-center justify-between h-[26px] px-3 text-[10.5px] text-[var(--color-text-muted)] font-mono bg-[var(--color-surface-app)] border-t border-[var(--color-border-subtle)]">
      <div className="flex items-center gap-4">
        {indexing ? (
          <>
            <StatusPill family="job" state="running" percent={percent} label="indexing" size="sm" />
            <span>{ready} / {total} signals</span>
          </>
        ) : (
          <StatusPill family="job" state="ready" label="ready" size="sm" />
        )}
      </div>
      <div className="flex items-center gap-4">
        <span>Autosaved · {formatClock(lastSavedAt)}</span>
        <span>render <b className="text-[var(--color-text-secondary)]">{queueDepth}</b></span>
        <ThroughputBars />
        <span>disk <b className="text-[var(--color-text-secondary)]">{formatGB(freeBytes)}</b></span>
      </div>
    </div>
  );
}

function formatClock(d: Date | null): string {
  if (!d) return "—";
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
function formatGB(bytes: number): string {
  return `${Math.round(bytes / 1e9)}GB`;
}

function ThroughputBars() {
  // 4 bars, currently a fixed visual; later wire to the existing
  // throughput state if there's a hook. Keep the static visual for now.
  return (
    <span className="inline-flex items-end gap-px h-2">
      <i className="block w-px h-[30%] bg-[var(--color-text-disabled)]" />
      <i className="block w-px h-[55%] bg-[var(--color-text-disabled)]" />
      <i className="block w-px h-[80%] bg-[var(--color-text-disabled)]" />
      <i className="block w-px h-[100%] bg-[var(--color-text-disabled)]" />
    </span>
  );
}
```

If the hooks above don't match what's actually exported, re-read the existing `JobsStatusBar.tsx` and copy its imports — those are guaranteed to compile.

- [ ] **Step 3: Mount and delete the old footer**

In `apps/desktop/src/shell/AppShell.tsx`, replace `<JobsStatusBar />` with `<Footer />` and update the import.

```bash
git rm apps/desktop/src/shell/JobsStatusBar.tsx
cd apps/desktop && grep -rEn 'JobsStatusBar' src/ tests/ || echo "no surviving refs"
```

Expected: no surviving refs after the import is updated.

- [ ] **Step 4: Boot and verify**

```bash
make desktop-stop && make desktop &
```

Verify the footer is one row, 26px tall, with two semantic groups (state left, system right). Indexing → orange running pill with `%`; once stable → green ready pill. No `Model: …` or `Context window: …` strings should appear in the footer; if they do, search the old `JobsStatusBar.tsx` references and clear them.

- [ ] **Step 5: Run all tests**

```bash
cd apps/desktop && pnpm test
```

- [ ] **Step 6: Commit**

```bash
git add -u apps/desktop/src apps/desktop/src/shell/chrome/Footer.tsx
git commit -m "redesign(chrome): footer with two semantic mono groups; remove JobsStatusBar"
```

---

## Task 11: `Landing.tsx` — no-project empty state

**Files:**
- Create: `apps/desktop/src/shell/empty/Landing.tsx`
- Modify: `apps/desktop/src/shell/AppShell.tsx` (render `<Landing />` when there is no project)

- [ ] **Step 1: Identify how "no project" is currently detected**

```bash
cd apps/desktop && grep -rEn 'No project|no project|noProject' src/shell/ src/state/
```

Find the boolean. Note its exact selector.

- [ ] **Step 2: Build `Landing.tsx`**

Create `apps/desktop/src/shell/empty/Landing.tsx`:

```tsx
import mark from "../../brand/awidat-mark.svg";
import { useRecentProjects, useProjectActions } from "../../state";  // adapt selectors if names differ

export function Landing() {
  const recents = useRecentProjects((s) => s.list);                  // expected: { name, path }[]
  const { newProject, importMedia, openProject } = useProjectActions();

  return (
    <div className="flex flex-col items-center justify-center text-center px-6 py-12 flex-1 min-h-0
                    bg-[radial-gradient(ellipse_at_50%_40%,#14110E_0%,var(--color-surface-page)_60%)]">
      <img src={mark} alt="" width={64} height={64}
           className="drop-shadow-[0_0_24px_rgba(255,122,24,0.30)] mb-3" />
      <div className="font-mono text-[13px] tracking-[0.16em] text-[var(--color-text-primary)] mb-5">
        AWIDAT
      </div>
      <h1 className="text-[16px] font-bold text-[var(--color-text-primary)] mb-1">
        Open a project to start editing
      </h1>
      <p className="text-[12px] text-[var(--color-text-muted)] max-w-[38ch] mb-5">
        Awidat needs a project to index your media and run the agent.
        Drop a file anywhere in the window, or pick one of these.
      </p>
      <div className="flex flex-wrap gap-2 justify-center">
        <LandingBtn primary kbd="⌘N" onClick={newProject}>＋ New project</LandingBtn>
        <LandingBtn kbd="⌘I" onClick={importMedia}>Import media</LandingBtn>
        <LandingBtn kbd="⌘O" onClick={openProject}>Open…</LandingBtn>
        <LandingBtn muted onClick={() => openProject("example")}>Try example</LandingBtn>
      </div>
      {recents.length > 0 && (
        <div className="mt-5 font-mono text-[10px] text-[var(--color-text-disabled)]">
          recent ·{" "}
          {recents.slice(0, 3).map((r, i) => (
            <span key={r.path}>
              <button
                onClick={() => openProject(r.path)}
                className="text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
              >
                {r.name}
              </button>
              {i < Math.min(recents.length, 3) - 1 && " · "}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

function LandingBtn({
  children, primary, muted, kbd, onClick,
}: { children: React.ReactNode; primary?: boolean; muted?: boolean; kbd?: string; onClick?: () => void }) {
  return (
    <button
      onClick={onClick}
      className={
        primary
          ? "h-7 inline-flex items-center gap-2 px-3 rounded-md border border-[var(--color-brand)] bg-[var(--color-brand)] text-[var(--color-text-inverse)] text-[11px] font-semibold hover:bg-[var(--color-brand-hover)]"
          : muted
          ? "h-7 inline-flex items-center gap-2 px-3 rounded-md text-[var(--color-text-muted)] text-[11px] font-semibold"
          : "h-7 inline-flex items-center gap-2 px-3 rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] text-[var(--color-text-primary)] text-[11px] font-semibold hover:bg-[var(--color-surface-card-hover)]"
      }
    >
      {children}
      {kbd && (
        <span className={primary
          ? "font-mono text-[9px] px-1 py-px rounded border border-[rgba(10,11,11,0.25)] text-[rgba(10,11,11,0.55)]"
          : "font-mono text-[9px] px-1 py-px rounded border border-[var(--color-border-subtle)] text-[var(--color-text-disabled)]"}
        >{kbd}</span>
      )}
    </button>
  );
}
```

If `useRecentProjects`/`useProjectActions` don't exist with these names, find the actual project state in `src/state/` and adapt. The component must render with `recents = []` (just hide the row).

- [ ] **Step 3: Render `<Landing />` when no project is open**

In `apps/desktop/src/shell/AppShell.tsx`, in the area between `<TopChrome />` and `<Footer />`, branch:

```tsx
{hasProject
  ? <ExistingEditOrDeliverContent />
  : <Landing />}
```

(`hasProject` is whatever boolean you identified in Step 1.)

- [ ] **Step 4: Wire drop-anywhere**

If the app already supports drag-and-drop import (likely — Tauri windows often do), confirm the existing handler still works when `<Landing />` is showing. If not, wrap the landing in a `<DropZone />` (using the existing pattern from elsewhere in the app — search `onDragOver|tauri.*drop`).

- [ ] **Step 5: Boot and verify**

Quit any open project; the landing should appear. Confirm:
- Hero mark + AWIDAT wordmark centered, mark has a subtle orange glow.
- One filled orange button ("＋ New project"); three ghost/muted siblings.
- Recents row visible iff there are recent projects.
- Drag-over the window dims the background slightly (if you implemented the visual cue).
- Clicking "New project" opens the existing new-project dialog (the modal itself is out of scope for this redesign).

- [ ] **Step 6: Tests**

```bash
cd apps/desktop && pnpm test
```

Update `desktop-ui-smoke.mjs` to include a check for `Landing` when no project is open (depends on how the smoke currently boots).

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/shell/empty/Landing.tsx apps/desktop/src/shell/AppShell.tsx apps/desktop/tests/desktop-ui-smoke.mjs
git commit -m "redesign(empty): no-project landing surface"
```

---

## Task 12: `FilmSlate.tsx` — source review loading state

**Files:**
- Create: `apps/desktop/src/shell/empty/FilmSlate.tsx`
- Modify: `apps/desktop/src/shell/PreviewSurface.tsx`

- [ ] **Step 1: Build `FilmSlate.tsx`**

Create `apps/desktop/src/shell/empty/FilmSlate.tsx`:

```tsx
interface FilmSlateProps {
  filename: string;
  resolution?: string;         // "1920×1080"
  codec?: string;              // "h264"
  audio?: string;              // "aac 48k"
  sizeBytes?: number;
  durationSec?: number;
  ready: number;               // 0..1
  status: string;              // "Building proxy…"
  detail: string;              // "5 of 9 indexers ready · transcript at 67%"
  eta?: string;                // "~2 min"
}

export function FilmSlate({
  filename, resolution, codec, audio, sizeBytes, durationSec, ready, status, detail, eta,
}: FilmSlateProps) {
  return (
    <div className="flex items-center justify-center w-full h-full bg-black">
      <div className="relative w-[60%] aspect-video border border-[var(--color-border-subtle)] rounded
                      flex flex-col justify-between p-3 text-[10px] text-[var(--color-text-muted)] font-mono
                      bg-[repeating-linear-gradient(45deg,#0F1112_0_6px,#0A0B0B_6px_12px)]">
        <div className="flex justify-between">
          <span>{filename}</span>
          {resolution && <span>{resolution}{codec ? ` · ${codec}` : ""}</span>}
        </div>
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-1.5 font-sans">
          <div className="text-[14px] font-bold text-[var(--color-brand)]">{status}</div>
          <div className="text-[11px] text-[var(--color-text-secondary)]">{detail}</div>
          {eta && (
            <div className="text-[10px] text-[var(--color-text-muted)] font-mono">
              {eta} · runs locally
            </div>
          )}
        </div>
        <div className="flex justify-between">
          {sizeBytes !== undefined && durationSec !== undefined && (
            <span>{Math.round(sizeBytes / 1e6)} MB · {durationSec.toFixed(1)}s</span>
          )}
          {audio && <span>{audio}</span>}
        </div>
        <div className="absolute inset-x-0 bottom-0 h-[3px] bg-[var(--color-surface-input)] overflow-hidden">
          <div
            className="h-full bg-gradient-to-r from-[var(--color-brand)] to-[#FCA67A] transition-[width] duration-300"
            style={{ width: `${Math.max(0, Math.min(1, ready)) * 100}%` }}
          />
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Surface the slate from `PreviewSurface`**

Open `apps/desktop/src/shell/PreviewSurface.tsx`. Find the render path that currently produces the black rectangle when there is a `sourceMedia` but no decoded proxy frame yet. Replace that branch with `<FilmSlate {...props} />`, mapping the existing state into the slate's props.

A typical patch:

```tsx
import { FilmSlate } from "./empty/FilmSlate";
…
const hasProxyFrame = /* existing predicate */;
const sourceMedia = /* existing accessor */;
const indexing    = /* existing accessor */;

if (sourceMedia && !hasProxyFrame) {
  return (
    <FilmSlate
      filename={sourceMedia.name}
      resolution={sourceMedia.resolution}
      codec={sourceMedia.codec}
      audio={sourceMedia.audio}
      sizeBytes={sourceMedia.sizeBytes}
      durationSec={sourceMedia.durationSec}
      ready={indexing.percent / 100}
      status={indexing.percent < 100 ? "Building proxy…" : "Decoding first frame…"}
      detail={`${indexing.ready} of ${indexing.total} indexers ready${
        indexing.transcript ? ` · transcript at ${indexing.transcript}%` : ""
      }`}
      eta={indexing.etaText}
    />
  );
}
```

Adapt field names to what the existing source-media model actually exposes.

- [ ] **Step 3: Cross-fade**

When `hasProxyFrame` becomes true, the surface should not jump cut. Add a 200ms opacity transition on the slate's outermost wrapper, or render both for the transition window. Simplest approach: wrap in a `<div className="transition-opacity duration-200" style={{ opacity: hasProxyFrame ? 0 : 1 }}>` and let the underlying `<video>` reveal beneath.

- [ ] **Step 4: Boot and verify**

```bash
make desktop-stop && make desktop &
```

Import a media file. While the proxy is building, the preview area should show the film slate with progress, not a black rectangle. Once the proxy is ready, the slate fades out and the video plays.

- [ ] **Step 5: Tests**

```bash
cd apps/desktop && pnpm test
```

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/shell/empty/FilmSlate.tsx apps/desktop/src/shell/PreviewSurface.tsx
git commit -m "redesign(empty): film slate replaces source-review black void"
```

---

## Task 13: Agent rail empty + starter prompts

**Files:**
- Create: `apps/desktop/src/agent/EmptyConversation.tsx`
- Create: `apps/desktop/src/agent/starterPrompts.ts`
- Create: `apps/desktop/tests/starter-prompts.test.ts`
- Modify: `apps/desktop/src/agent/ChatStream.tsx`
- Modify: `apps/desktop/package.json` (add `test:starter-prompts`)

- [ ] **Step 1: Write the failing starter-prompts test**

Create `apps/desktop/tests/starter-prompts.test.ts`:

```typescript
import { strict as assert } from "node:assert";
import { getStarterPrompts } from "../src/agent/starterPrompts.ts";

const podcast = getStarterPrompts("podcast");
assert.equal(podcast.length, 4);
assert.ok(podcast.every((p) => typeof p === "string" && p.length > 0));

const unknown = getStarterPrompts("totally_unknown_type_42");
assert.equal(unknown.length, 4);
assert.ok(unknown.includes("Suggest a starting edit"));

console.log("starter-prompts: OK");
```

Wire into `package.json`:

```json
"test:starter-prompts": "node --experimental-strip-types tests/starter-prompts.test.ts",
```

Add to `test` aggregate.

- [ ] **Step 2: Implement starter-prompts**

Create `apps/desktop/src/agent/starterPrompts.ts`:

```typescript
const PROMPTS_BY_TYPE: Record<string, string[]> = {
  podcast: [
    "Trim filler & long silences",
    "Cut to the punchline (≤ 90s)",
    "Find a YouTube-ready highlight",
    "Apply podcast cleanup defaults",
  ],
  interview: [
    "Remove cross-talk & dead air",
    "Pull the strongest 3 quotes",
    "Make a 60-second teaser",
    "Apply interview cleanup defaults",
  ],
  highlight: [
    "Find the biggest moments",
    "Cut a 30-second teaser",
    "Tighten reaction beats",
    "Apply highlight defaults",
  ],
};

const FALLBACK: string[] = [
  "Trim long silences",
  "Summarize what's in this clip",
  "Suggest a starting edit",
  "Show me the loudest moments",
];

export function getStarterPrompts(projectType: string | undefined | null): string[] {
  if (!projectType) return FALLBACK;
  return PROMPTS_BY_TYPE[projectType] ?? FALLBACK;
}
```

- [ ] **Step 3: Run the test to verify it passes**

```bash
cd apps/desktop && npm run test:starter-prompts
```

Expected: `starter-prompts: OK`.

- [ ] **Step 4: Build `EmptyConversation.tsx`**

Create `apps/desktop/src/agent/EmptyConversation.tsx`:

```tsx
import { getStarterPrompts } from "./starterPrompts";
import { useProject, useIndexing, useAgent } from "../state";   // adapt

export function EmptyConversation() {
  const project = useProject((s) => s.current);
  const indexing = useIndexing();
  const sendPrompt = useAgent((s) => s.send);

  const prompts = getStarterPrompts(project?.type);
  const allReady = indexing.ready === indexing.total && indexing.total > 0;
  const opener = allReady
    ? `I've indexed your ${project?.durationLabel ?? "clip"} — speech, scenes, color, audio all ready. Tell me how you want this cut, or pick a starting move below.`
    : `Reading your media now — ${indexing.ready} of ${indexing.total} signals done. You can still send a message; I'll work on it once indexing finishes.`;

  return (
    <div className="flex flex-col h-full">
      <div className="m-3 p-3 rounded-lg border border-[var(--color-border-subtle)]
                      bg-[linear-gradient(180deg,rgba(255,122,24,0.05),transparent),var(--color-surface-panel)]">
        <div className="text-[12px] font-semibold text-[var(--color-text-primary)] mb-1">
          ▲ Awidat
          <span className="ml-2 text-[10px] text-[var(--color-text-muted)] font-normal">
            · read AGENTS.md · {project?.type ?? "neutral"} mode
          </span>
        </div>
        <p className="text-[11px] text-[var(--color-text-secondary)] leading-snug">{opener}</p>
      </div>

      <div className="px-3 flex flex-col gap-1.5">
        <div className="text-[9px] uppercase tracking-[0.08em] text-[var(--color-text-muted)] font-bold">
          Try
        </div>
        {prompts.map((p) => (
          <button
            key={p}
            onClick={() => sendPrompt(p)}
            className="flex items-center gap-2 px-2.5 py-1.5 rounded-md border border-[var(--color-border-subtle)]
                       bg-[var(--color-surface-panel)] text-[11px] text-[var(--color-text-secondary)]
                       hover:border-[var(--color-border)] hover:bg-[var(--color-surface-card-hover)]
                       hover:text-[var(--color-text-primary)] text-left"
          >
            <span className="text-[var(--color-brand)]">▸</span> {p}
          </button>
        ))}
      </div>
    </div>
  );
}
```

- [ ] **Step 5: Mount in `ChatStream.tsx`**

Open `apps/desktop/src/agent/ChatStream.tsx`. Find where the message list renders. When the list is empty, render `<EmptyConversation />` instead. Pseudo:

```tsx
import { EmptyConversation } from "./EmptyConversation";
…
if (messages.length === 0) return <EmptyConversation />;
return <ExistingMessageList … />;
```

- [ ] **Step 6: Boot and verify**

Open a project, open the agent rail. With an empty conversation, you should see the opener card + 4 starter prompts + the existing composer. Click a starter — it should populate the composer / send (depending on how `sendPrompt` is wired).

- [ ] **Step 7: Run tests**

```bash
cd apps/desktop && pnpm test
```

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/src/agent/EmptyConversation.tsx apps/desktop/src/agent/starterPrompts.ts apps/desktop/src/agent/ChatStream.tsx apps/desktop/tests/starter-prompts.test.ts apps/desktop/package.json
git commit -m "redesign(agent): empty conversation with project-aware starter prompts"
```

---

## Task 14: Index rail — split `IndexingDashboard.tsx`; build `IndexRailPro`

**Files:**
- Create: `apps/desktop/src/shell/IndexRailPro.tsx`
- Create: `apps/desktop/src/shell/IndexRail.tsx` (Pro-only for now; Creator added in Task 15)
- Modify: `apps/desktop/src/shell/IndexingDashboard.tsx` (becomes a re-export of `IndexRail` for compatibility)
- Modify: callers of `IndexingDashboard` (one-shot import rename)

The current `IndexingDashboard.tsx` is 776 lines — too large per CLAUDE.md guidance. This task carves it up.

- [ ] **Step 1: Read the current file end-to-end**

```bash
cd apps/desktop && wc -l src/shell/IndexingDashboard.tsx
```

Read the whole file. Identify the natural sections (header, evidence coverage, stats grid, per-signal rows, indexers list, etc.). Each section becomes a small subcomponent.

- [ ] **Step 2: Extract Pro-mode rendering into `IndexRailPro.tsx`**

Create `apps/desktop/src/shell/IndexRailPro.tsx`. Move (don't duplicate) the existing rendering for Pro into it, splitting into small named functions per section:

```tsx
import { StatusPill } from "../ui/primitives/StatusPill";
import { useIndexing } from "../state";   // adapt

export function IndexRailPro() {
  const idx = useIndexing();
  return (
    <div className="flex flex-col gap-3 p-3.5 text-[12px]">
      <Header idx={idx} />
      <StatGrid idx={idx} />
      <SignalGroup title="Speech"  signals={idx.bySection.speech}  />
      <SignalGroup title="Visuals" signals={idx.bySection.visuals} />
      <SignalGroup title="Audio"   signals={idx.bySection.audio}   />
      <IndexersStrip idx={idx} />
    </div>
  );
}

function Header({ idx }: { idx: ReturnType<typeof useIndexing> }) {
  return (
    <div className="flex flex-col gap-1">
      <div className="flex justify-between items-center">
        <h4 className="text-[13px] font-semibold text-[var(--color-text-primary)]">Index readiness</h4>
        {idx.percent < 100
          ? <StatusPill family="job" state="running" percent={idx.percent} label="Indexing" />
          : <StatusPill family="job" state="ready" />}
      </div>
      <div className="text-[11px] text-[var(--color-text-muted)] font-mono">
        {idx.ready} of {idx.total} ready · {idx.queued} queued{idx.etaText ? ` · ETA ${idx.etaText}` : ""}
      </div>
      <div className="h-1 rounded-full bg-[var(--color-surface-input)] overflow-hidden">
        <div className="h-full bg-gradient-to-r from-[var(--color-brand)] to-[#FCA67A]"
             style={{ width: `${idx.percent}%` }} />
      </div>
    </div>
  );
}

function StatGrid({ idx }: { idx: ReturnType<typeof useIndexing> }) {
  const cells = [
    { val: idx.durationLabel ?? "—", lab: "Duration" },
    { val: idx.scenes ?? "—",         lab: "Scenes" },
    { val: idx.segments ?? "—",       lab: "Segments" },
    { val: idx.transcriptLabel ?? "—",lab: "Transcript" },
  ];
  return (
    <div className="grid grid-cols-2 gap-1.5">
      {cells.map((c) => (
        <div key={c.lab} className="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2.5 py-2">
          <div className="font-mono text-[13px] font-semibold text-[var(--color-text-primary)]">{c.val}</div>
          <div className="text-[10px] uppercase tracking-[0.06em] text-[var(--color-text-muted)] mt-0.5">{c.lab}</div>
        </div>
      ))}
    </div>
  );
}

interface Signal { name: string; state: "idle" | "running" | "ready" | "failed"; percent?: number; }
function SignalGroup({ title, signals }: { title: string; signals: Signal[] }) {
  const ready = signals.filter(s => s.state === "ready").length;
  // pad to a multiple of 3 with low-opacity placeholders so the grid never reflows
  const padded: (Signal | null)[] = [...signals];
  while (padded.length % 3 !== 0) padded.push(null);

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex justify-between items-baseline">
        <span className="text-[10px] uppercase tracking-[0.08em] text-[var(--color-text-muted)] font-bold">{title}</span>
        <span className="text-[11px] text-[var(--color-text-secondary)] font-mono">{ready} / {signals.length} ready</span>
      </div>
      <div className="grid grid-cols-3 gap-px bg-[var(--color-border-subtle)] rounded-md overflow-hidden border border-[var(--color-border-subtle)]">
        {padded.map((s, i) =>
          s
            ? (
              <div key={s.name} className="bg-[var(--color-surface-card)] px-2 py-1.5 min-h-[42px] flex flex-col justify-between">
                <div className="text-[11px] font-semibold text-[var(--color-text-primary)]">{s.name}</div>
                <div className="text-[10px] text-[var(--color-text-muted)] font-mono flex items-center gap-1">
                  <StatusPill family="job" state={s.state} percent={s.state === "running" ? s.percent : undefined} dotOnly />
                  {s.state}{s.state === "running" && s.percent !== undefined ? ` ${s.percent}%` : ""}
                </div>
              </div>
            )
            : <div key={`pad-${i}`} className="bg-[var(--color-surface-card)] opacity-40 px-2 py-1.5 min-h-[42px] text-[var(--color-text-disabled)]">—</div>
        )}
      </div>
    </div>
  );
}

function IndexersStrip({ idx }: { idx: ReturnType<typeof useIndexing> }) {
  // Collapsed by default — click to expand into the full indexer list
  const names = idx.indexers.map((i) => i.name);
  const first = names.slice(0, 3);
  const overflow = Math.max(0, names.length - 3);
  return (
    <div className="flex justify-between items-center pt-2 border-t border-[var(--color-border-subtle)] text-[11px] text-[var(--color-text-muted)]">
      <span>{names.length} indexers active</span>
      <div className="flex gap-1 flex-wrap">
        {first.map((n) => (
          <span key={n} className="px-1.5 py-px font-mono text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border-subtle)] rounded bg-[var(--color-surface-card)]">{n}</span>
        ))}
        {overflow > 0 && <span className="px-1.5 py-px font-mono text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border-subtle)] rounded bg-[var(--color-surface-card)]">+{overflow}</span>}
      </div>
    </div>
  );
}
```

The `useIndexing` hook is whatever currently feeds the old `IndexingDashboard` — adapt the field names to match the existing shape (rename `bySection` etc. if the existing store uses different names). **Do not invent new state**; only re-render existing state in the new layout.

- [ ] **Step 3: Create `IndexRail.tsx` as a thin selector**

Create `apps/desktop/src/shell/IndexRail.tsx`:

```tsx
import { useMode } from "../state/mode";
import { IndexRailPro } from "./IndexRailPro";

export function IndexRail() {
  const mode = useMode((s) => s.mode);
  if (mode === "creator") {
    // Creator surface arrives in Task 15. For now, Pro is the only render path.
    return <IndexRailPro />;
  }
  return <IndexRailPro />;
}
```

- [ ] **Step 4: Reduce `IndexingDashboard.tsx` to a compatibility shim**

Replace the entire contents of `apps/desktop/src/shell/IndexingDashboard.tsx` with:

```tsx
// Compatibility shim — the dashboard split into IndexRail{Pro,Creator}. New callers should import from "./IndexRail".
export { IndexRail as IndexingDashboard } from "./IndexRail";
```

The 776-line body is now distributed across `IndexRailPro.tsx` plus the future `IndexRailCreator.tsx`.

- [ ] **Step 5: Boot, verify, screenshot**

```bash
make desktop-stop && make desktop &
```

Open a project with media. Confirm the Index rail now shows:
- Header with title, running/ready pill, mono `n of m ready` line, 4px orange progress bar
- 4-stat grid (Duration / Scenes / Segments / Transcript)
- Three groups (Speech / Visuals / Audio) each as a 3-col tile grid, padded with `—` placeholders
- Bottom indexers strip with first 3 names + overflow chip

Take a screenshot for the PR.

- [ ] **Step 6: Run all tests**

```bash
cd apps/desktop && pnpm test
```

If the smoke test asserts on Old-IndexingDashboard text (e.g., "Waiting for local indexer", "Missing"), update the assertions to look for the new strings.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/shell/IndexRailPro.tsx apps/desktop/src/shell/IndexRail.tsx apps/desktop/src/shell/IndexingDashboard.tsx apps/desktop/tests/desktop-ui-smoke.mjs
git commit -m "redesign(index): Pro grid replaces 776-line IndexingDashboard"
```

---

## Task 15: Index rail — `IndexRailCreator` + mode-driven selection

**Files:**
- Create: `apps/desktop/src/shell/IndexRailCreator.tsx`
- Modify: `apps/desktop/src/shell/IndexRail.tsx`

- [ ] **Step 1: Build `IndexRailCreator.tsx`**

Create `apps/desktop/src/shell/IndexRailCreator.tsx`:

```tsx
import { useState } from "react";
import { StatusPill } from "../ui/primitives/StatusPill";
import { useIndexing } from "../state";
import { IndexRailPro } from "./IndexRailPro";

export function IndexRailCreator() {
  const idx = useIndexing();
  const [showDetails, setShowDetails] = useState(false);

  if (showDetails) {
    return (
      <div className="flex flex-col">
        <button
          onClick={() => setShowDetails(false)}
          className="text-[var(--color-brand)] text-[11px] font-semibold px-3 py-2 text-left hover:bg-[var(--color-surface-hover)]"
        >
          ▴ Hide signal details
        </button>
        <IndexRailPro />
      </div>
    );
  }

  const allReady = idx.percent >= 100;

  return (
    <div className="p-3.5 text-[12px] flex flex-col gap-3">
      <div className="rounded-lg border border-[var(--color-border-subtle)]
                      bg-[linear-gradient(180deg,rgba(255,122,24,0.04),transparent),var(--color-surface-panel)] p-3">
        <div className="flex justify-between items-center">
          <div className="text-[14px] font-semibold text-[var(--color-text-primary)]">
            {allReady ? "Ready to edit" : "Indexing your media…"}
          </div>
          {allReady
            ? <StatusPill family="job" state="ready" />
            : <StatusPill family="job" state="running" percent={idx.percent} label="" />}
        </div>
        <div className="h-1 rounded-full bg-[var(--color-surface-input)] overflow-hidden mt-2">
          <div className="h-full bg-gradient-to-r from-[var(--color-brand)] to-[#FCA67A]"
               style={{ width: `${idx.percent}%` }} />
        </div>
        <div className="text-[11px] text-[var(--color-text-muted)] mt-1">
          {idx.ready} of {idx.total} signals ready{idx.etaText ? ` · ETA ${idx.etaText}` : ""} · works offline
        </div>
      </div>

      <div className="grid grid-cols-2 gap-1.5">
        <Stat val={idx.durationLabel ?? "—"} lab="Duration" />
        <Stat val={idx.scenes !== undefined ? `${idx.scenes} scenes` : "—"} lab="Detected" />
      </div>

      <p className="text-[11px] text-[var(--color-text-muted)] leading-snug">
        Awidat is reading your media. As soon as it's done, the agent can propose cleanup edits.
      </p>

      <button
        onClick={() => setShowDetails(true)}
        className="text-[var(--color-brand)] text-[11px] font-semibold rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface-panel)] py-2 hover:bg-[var(--color-surface-card-hover)]"
      >
        ▾ Show signal details · advanced
      </button>
    </div>
  );
}

function Stat({ val, lab }: { val: string | number; lab: string }) {
  return (
    <div className="rounded-md border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2.5 py-2">
      <div className="font-mono text-[13px] font-semibold text-[var(--color-text-primary)]">{val}</div>
      <div className="text-[10px] uppercase tracking-[0.06em] text-[var(--color-text-muted)] mt-0.5">{lab}</div>
    </div>
  );
}
```

- [ ] **Step 2: Wire `IndexRail` to pick by mode**

Replace `apps/desktop/src/shell/IndexRail.tsx`:

```tsx
import { useMode } from "../state/mode";
import { IndexRailPro } from "./IndexRailPro";
import { IndexRailCreator } from "./IndexRailCreator";

export function IndexRail() {
  const mode = useMode((s) => s.mode);
  return mode === "creator" ? <IndexRailCreator /> : <IndexRailPro />;
}
```

- [ ] **Step 3: Boot and verify both modes**

```bash
make desktop-stop && make desktop &
```

Toggle the mode pill in the chrome:
- **Pro**: dense grid (Task 14 surface).
- **Creator**: summary card + 2-stat block + "Show signal details" disclosure.
- Clicking "Show signal details" expands inline into the Pro grid with a "Hide signal details" header.

Verify in the React devtools profiler that toggling the mode pill does not unmount the entire tree of either rail — only the rail container swaps. The actual nested components inside `IndexRailPro` should keep their identity.

- [ ] **Step 4: Tests**

```bash
cd apps/desktop && pnpm test
```

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/shell/IndexRailCreator.tsx apps/desktop/src/shell/IndexRail.tsx
git commit -m "redesign(index): Creator summary surface with disclose-to-pro affordance"
```

---

## Task 16: Inspector rail — mode-driven defaults

**Files:**
- Create: `apps/desktop/src/ui/primitives/CollapsiblePanel.tsx`
- Modify: `apps/desktop/src/inspector/ClipInspector.tsx`

- [ ] **Step 1: Build `CollapsiblePanel`**

Create `apps/desktop/src/ui/primitives/CollapsiblePanel.tsx`:

```tsx
import { useState, useEffect, useRef, type ReactNode } from "react";
import { useMode } from "../../state/mode";

type RevealLevel = "always" | "pro" | "advanced";

interface Props {
  title: string;
  /**
   * "always" — both modes default expanded.
   * "pro"     — expanded in Pro mode, collapsed in Creator mode.
   * "advanced"— always collapsed by default in both modes; user opens manually.
   */
  revealLevel?: RevealLevel;
  /** Per-instance override of the initial state; otherwise derived from mode + revealLevel. */
  initialOpen?: boolean;
  children: ReactNode;
}

export function CollapsiblePanel({ title, revealLevel = "always", initialOpen, children }: Props) {
  const mode = useMode((s) => s.mode);
  const initial = initialOpen ?? deriveInitialOpen(mode, revealLevel);
  const [open, setOpen] = useState(initial);

  // When mode flips, re-derive only if the user hasn't manually overridden.
  const userTouched = useRef(false);
  useEffect(() => {
    if (!userTouched.current) setOpen(deriveInitialOpen(mode, revealLevel));
  }, [mode, revealLevel]);

  return (
    <section className="flex flex-col">
      <button
        onClick={() => { setOpen((v) => !v); userTouched.current = true; }}
        className="flex items-center justify-between px-3 py-2 text-[10px] uppercase tracking-[0.08em] text-[var(--color-text-muted)] font-bold hover:text-[var(--color-text-secondary)]"
      >
        {title}
        <span className="text-[var(--color-text-disabled)]">{open ? "▾" : "▸"}</span>
      </button>
      {open && <div className="px-3 pb-3 flex flex-col gap-2">{children}</div>}
    </section>
  );
}

function deriveInitialOpen(mode: "pro" | "creator", level: RevealLevel): boolean {
  if (level === "always")   return true;
  if (level === "advanced") return false;
  // "pro": open in pro, closed in creator
  return mode === "pro";
}
```

- [ ] **Step 2: Wrap sections of `ClipInspector.tsx`**

Open `apps/desktop/src/inspector/ClipInspector.tsx`. Find each section (Identity, Visual, Audio, Timing, Track Mix, Timing Metadata, Danger Zone). Wrap each in `<CollapsiblePanel>` with the right `revealLevel`:

| Section | revealLevel |
|---|---|
| Identity | always |
| Visual (full slider stack incl. LUT) | pro |
| Audio (with Volume/Fades surfaced) | always — but with Volume only-by-default in Creator (split if needed) |
| Timing | always — but Speed only-by-default in Creator (split if needed) |
| Track Mix | pro |
| Timing Metadata | advanced |
| Danger Zone | advanced |

For "Audio" and "Timing": if the spec's "Volume only" / "Speed only" Creator defaults conflict with wrapping the whole section, split each into two CollapsiblePanels — a small `always` panel with just the Creator-surface controls, and an `advanced` (or `pro`) panel with the rest. Match the table above as a target.

- [ ] **Step 3: Boot and verify**

```bash
make desktop-stop && make desktop &
```

Open a project with a clip. Click the clip. In Pro mode: every section expanded by default. Switch to Creator mode: only Identity + the four Creator-default controls visible; everything else collapsed. Click a collapsed header — it opens, and stays open across a mode flip (user override sticks).

- [ ] **Step 4: Tests**

```bash
cd apps/desktop && pnpm test
```

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/ui/primitives/CollapsiblePanel.tsx apps/desktop/src/inspector/ClipInspector.tsx
git commit -m "redesign(inspector): mode-driven section reveal via CollapsiblePanel"
```

---

## Task 17: App icon (mark + icon set + Tauri manifest)

**Files:**
- Create: `apps/desktop/src/brand/awidat-mark.svg` (already a placeholder from Task 8 — finalize it here)
- Create: `apps/desktop/src-tauri/icons/iconset/icon_<size>.png` for 16/32/64/128/256/512/1024
- Modify: `apps/desktop/src-tauri/tauri.conf.json` (icon manifest)
- Modify: `apps/desktop/src-tauri/icons/icon.icns`, `icon.ico`, top-level `*.png` files

- [ ] **Step 1: Finalize the SVG mark**

Replace the placeholder `apps/desktop/src/brand/awidat-mark.svg` with the production master:

```xml
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
  <!-- plinth -->
  <rect x="0" y="0" width="1024" height="1024" rx="256" fill="#0F1110"/>
  <rect x="0.5" y="0.5" width="1023" height="1023" rx="255.5" fill="none" stroke="#2A2C2B" stroke-width="1"/>
  <!-- equilateral triangle, ~640px tall, centered, glow handled via filter at hero sizes only -->
  <polygon points="512,210 814,778 210,778" fill="#FF7A18"/>
</svg>
```

- [ ] **Step 2: Generate raster sizes**

Use `rsvg-convert` (available on macOS via `brew install librsvg`) or any SVG → PNG converter you prefer. From `apps/desktop/src-tauri/icons/`:

```bash
cd apps/desktop/src-tauri/icons
mkdir -p iconset
for s in 16 32 64 128 256 512 1024; do
  rsvg-convert -w $s -h $s ../../src/brand/awidat-mark.svg -o iconset/icon_${s}.png
done
# also regenerate the legacy top-level files Tauri references:
cp iconset/icon_32.png 32x32.png
cp iconset/icon_128.png 128x128.png
cp iconset/icon_256.png 128x128@2x.png
cp iconset/icon_512.png icon.png
```

For `.icns` (macOS) and `.ico` (Windows):
```bash
# Build .icns from the iconset folder
mkdir -p icon.iconset
cp iconset/icon_16.png  icon.iconset/icon_16x16.png
cp iconset/icon_32.png  icon.iconset/icon_16x16@2x.png
cp iconset/icon_32.png  icon.iconset/icon_32x32.png
cp iconset/icon_64.png  icon.iconset/icon_32x32@2x.png
cp iconset/icon_128.png icon.iconset/icon_128x128.png
cp iconset/icon_256.png icon.iconset/icon_128x128@2x.png
cp iconset/icon_256.png icon.iconset/icon_256x256.png
cp iconset/icon_512.png icon.iconset/icon_256x256@2x.png
cp iconset/icon_512.png icon.iconset/icon_512x512.png
cp iconset/icon_1024.png icon.iconset/icon_512x512@2x.png
iconutil -c icns icon.iconset -o icon.icns
rm -rf icon.iconset

# .ico from 256
sips -s format ico iconset/icon_256.png --out icon.ico   # or use ImageMagick `convert iconset/icon_*.png icon.ico`
```

Verify file sizes are non-zero.

- [ ] **Step 3: Tauri config update (no change required if pointing at the same paths)**

Open `apps/desktop/src-tauri/tauri.conf.json`. The `bundle.icon` array already points at `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.icns`, `icon.ico` — all of which were overwritten in Step 2. No change needed unless you want to add the larger PNGs for any specific bundler target.

- [ ] **Step 4: Rebuild and verify**

```bash
make desktop-stop && make desktop &
```

In the macOS dock, confirm the app icon is the new orange triangle on the dark plinth. Take a screenshot.

- [ ] **Step 5: Verify the in-app mark looks right**

The same SVG drives the in-app mark via the imports in `IdentityRow.tsx`, `Landing.tsx`. Walk the surfaces: chrome (small), landing (large with glow). Both should look correct.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/brand/awidat-mark.svg apps/desktop/src-tauri/icons/
git commit -m "redesign(brand): new ▲ mark, regenerated icon set across all sizes"
```

---

## Task 18: Acceptance pass

**Files:**
- (verification only; no edits unless something fails)

- [ ] **Step 1: Run the acceptance criteria from the spec one by one**

For each of the seven criteria in spec §13, perform the verification:

- [ ] `--color-brand` mint is no longer the primary CTA color anywhere:
  ```bash
  cd apps/desktop && grep -rEn '#20C997' src/ --include='*.tsx' --include='*.ts' --include='*.css'
  ```
  Expected: matches only in `tokens.css` `--color-accent-mint`, nowhere else.

- [ ] Index rail with no indexers run yet shows no "Missing":
  ```bash
  cd apps/desktop && grep -rEn '\bMissing\b' src/ --include='*.tsx' --include='*.ts'
  ```
  Expected: no matches in src (only optionally in tests as a "must not appear" assertion).

- [ ] Source review with an asset and no decoded proxy frame shows the slate, not a black rectangle. Manual: import a fresh file, observe the preview.

- [ ] No `kind="missing"` / `kind="reviewing"` pill survives:
  ```bash
  cd apps/desktop && grep -rEn 'kind="(missing|reviewing)"' src/
  ```
  Expected: no matches.

- [ ] macOS window has no native "Awidat" title. Manual: look at the window's title bar — should be invisible.

- [ ] Landing surface has exactly one filled accent-color button:
  ```bash
  cd apps/desktop && grep -E 'bg-\[var\(--color-brand\)\]' src/shell/empty/Landing.tsx | wc -l
  ```
  Expected: exactly 1 match.

- [ ] Mode toggle does not unmount panels. Manual: React Profiler trace — toggle the mode pill, confirm `IndexRailPro` subtree does not unmount its non-rail siblings, and `CollapsiblePanel` sections preserve user-set open state.

- [ ] **Step 2: Run the full test suite one last time**

```bash
cd apps/desktop && pnpm test
```

Expected: all pass.

- [ ] **Step 3: Run the existing Rust checks (no Rust code changed, but the spec says so)**

```bash
cd /Users/explicit/Projects/awidat && make check
```

Expected: pass (cargo fmt + clippy + test). If any of those fails, the failure is pre-existing — record and report.

- [ ] **Step 4: Take a screenshot tour for the PR**

Boot the app and capture (`⌘⇧4` or any screenshot tool) one image per surface to attach to the PR:

- Landing
- Project just opened, indexing
- Project indexed, Pro mode (full Edit view)
- Project indexed, Creator mode (notice the rails collapsed)
- Deliver view
- Agent rail with conversation empty

Save under `/tmp/redesign-final/`.

- [ ] **Step 5: Final commit (if anything was tweaked during the acceptance pass)**

```bash
cd apps/desktop && git status
# if dirty:
git add -u
git commit -m "redesign: acceptance pass tweaks"
```

---

## Self-Review

Performed inline during plan authoring:

**Spec coverage map:**

| Spec section | Plan task(s) |
|---|---|
| §1 Why | covered by every task implicitly |
| §2 Direction | tokens (Task 2) + chrome (8–10) |
| §3 Brand | mark (Task 8, 17), wordmark (8), voice (encoded in copy of empty states + footer) |
| §4 Mode system | Task 6 (store) + Task 16 (CollapsiblePanel) + Tasks 15 + 16 wire-up |
| §5 Token revisions | Task 2 |
| §6 Chrome | Tasks 7, 8, 9, 10 |
| §7 Index rail | Tasks 14, 15 |
| §8 Inspector rail | Task 16 |
| §9 StatusPill primitive | Tasks 3, 4 |
| §10 Empty + loading | Tasks 11 (landing), 12 (slate), 13 (agent), 14 (no-clips note absorbed into IndexRailPro/Creator) |
| §11 Out of scope | not built; foundation sweeps (Tasks 2, 5) inoculate the deferred surfaces against visual lag |
| §12 Non-goals | not built (correct) |
| §13 Acceptance criteria | Task 18 |
| §14 Risks | mitigations baked into task ordering (foundation before surface) |
| §15 References | linked in spec, not in plan |

No spec section unmapped. Task 11's drop-anywhere note depends on existing app drag-drop infrastructure; if that infrastructure is missing entirely, the task notes it and the engineer can flag for follow-up.

**Placeholder scan:** none found in the plan body. Several tasks intentionally say "adapt to actual hooks" — those are not placeholders, they are instructions to grep for the current names because the plan author cannot enumerate every Zustand selector from outside the codebase.

**Type consistency:** `StatusPill` props use `family + state + percent` throughout. `useMode` exposes `{ mode, setMode, toggle }` consistently across Tasks 6, 8, 15, 16. `CollapsiblePanel`'s `revealLevel: 'always' | 'pro' | 'advanced'` matches the spec's reveal taxonomy.

**Scope check:** single plan, sequenced. PR boundaries are a user decision; the natural split (foundation Tasks 1–7, surface Tasks 8–18) is suggested but not enforced by the plan structure.
