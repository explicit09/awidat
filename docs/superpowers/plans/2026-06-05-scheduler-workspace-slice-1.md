# Scheduler Workspace Slice 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a first product-facing Scheduler workspace with read-only calendar, queue, and review-drawer views backed by existing render queue and server-backed publish job state.

**Architecture:** This first slice adds a new StageShell destination named `schedule`, then mounts a Scheduler workspace component from `App.tsx`. The workspace derives `SchedulerPost` view models from existing `RenderQueueEntry.uploadStates`, `uploadMetadata`, and `publishedUrls`; no new scheduling writes, OAuth flows, or server commands are introduced in this slice.

**Tech Stack:** React 19, TypeScript, Zustand render queue store, existing Montage StageShell, Node `--experimental-strip-types` test harness.

Spec: `docs/superpowers/specs/2026-06-05-scheduler-workspace-design.md`.

---

## File Structure

- Modify `apps/desktop/src/state/stages.ts` — add the `schedule` destination and label.
- Modify `apps/desktop/src/shell/StageShell.tsx` — add the dock item, command routing, destination rendering, and prop for the scheduler node.
- Modify `apps/desktop/src/App.tsx` — compose and pass the new scheduler workspace.
- Create `apps/desktop/src/app/scheduler/schedulerModel.ts` — pure types and derivation helpers for calendar/queue cards.
- Create `apps/desktop/src/app/scheduler/SchedulerWorkspace.tsx` — read-only UI for calendar, queue, and review drawer.
- Create `apps/desktop/tests/scheduler-model.test.ts` — pure model tests.
- Modify `apps/desktop/package.json` — add `test:scheduler-model` script.

This plan intentionally keeps the first slice read-only. Slice 2 will add `ScheduleDraft`, bulk cadence generation, source picker writes, and approval into server-backed jobs.

---

## Task 1: Scheduler Model

**Files:**
- Create: `apps/desktop/src/app/scheduler/schedulerModel.ts`
- Create: `apps/desktop/tests/scheduler-model.test.ts`
- Modify: `apps/desktop/package.json`

- [ ] **Step 1: Write the failing test**

Create `apps/desktop/tests/scheduler-model.test.ts`:

```ts
import { strict as assert } from "node:assert";

import {
  deriveSchedulerPosts,
  formatSchedulerTime,
  schedulerStatusLabel,
} from "../src/app/scheduler/schedulerModel.ts";
import type { RenderQueueEntry } from "../src/app/renderQueue.ts";

const entry: RenderQueueEntry = {
  id: "queue_1",
  project: "/project",
  label: "Launch clip",
  target: "youtube",
  status: "done",
  progress: 1,
  createdAt: 1_000,
  updatedAt: 1_010,
  jobId: "render_1",
  outputPath: "/project/renders/launch.mp4",
  uploadTargets: ["youtube", "tiktok"],
  uploadMetadata: {
    youtube: {
      title: "YouTube launch title",
      description: "Long-form description",
      tags: ["montage"],
      visibility: "public",
      scheduledAt: 1_800,
      thumbnailPath: undefined,
    },
    tiktok: {
      title: "TikTok hook",
      description: "Short caption",
      tags: [],
      visibility: "private",
      scheduledAt: undefined,
      thumbnailPath: undefined,
    },
  },
  uploadStates: {
    youtube: { state: "scheduled", job_id: "job_yt" },
    tiktok: {
      state: "published",
      remote_url: "https://tiktok.example/post/1",
      remote_id: "tt_1",
    },
  },
  publishedUrls: {
    tiktok: "https://tiktok.example/post/1",
  },
};

const posts = deriveSchedulerPosts([entry], 1_700);

assert.equal(posts.length, 2);
assert.deepEqual(
  posts.map((post) => `${post.provider}:${post.status}:${post.title}`),
  [
    "youtube:scheduled:YouTube launch title",
    "tiktok:published:TikTok hook",
  ],
);
assert.equal(posts[0].scheduledAt, 1_800);
assert.equal(posts[0].jobId, "job_yt");
assert.equal(posts[0].visibility, "public");
assert.equal(posts[1].providerUrl, "https://tiktok.example/post/1");
assert.equal(schedulerStatusLabel("requires_action"), "Action needed");
assert.equal(formatSchedulerTime(1_800, "UTC"), "1970-01-01 00:30 UTC");

console.log("scheduler-model: OK");
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cd apps/desktop
node --experimental-strip-types tests/scheduler-model.test.ts
```

Expected: FAIL with module-not-found for `src/app/scheduler/schedulerModel.ts`.

- [ ] **Step 3: Implement the model**

Create `apps/desktop/src/app/scheduler/schedulerModel.ts`:

```ts
import type {
  RenderQueueEntry,
  RenderUploadState,
} from "../renderQueue";
import type { UploadVisibility } from "../../state/uploadMetadata";

export type SchedulerStatus =
  | "draft"
  | "scheduled"
  | "uploading"
  | "processing"
  | "published"
  | "failed"
  | "requires_action"
  | "cancelled";

export type SchedulerPost = {
  id: string;
  renderQueueId: string;
  renderJobId?: string;
  provider: string;
  title: string;
  description: string;
  visibility: UploadVisibility;
  scheduledAt: number;
  status: SchedulerStatus;
  jobId?: string;
  outputPath?: string;
  providerUrl?: string;
  failureReason?: string;
  updatedAt: number;
};

const STATUS_LABELS: Record<SchedulerStatus, string> = {
  draft: "Draft",
  scheduled: "Scheduled",
  uploading: "Uploading",
  processing: "Processing",
  published: "Published",
  failed: "Failed",
  requires_action: "Action needed",
  cancelled: "Cancelled",
};

export function schedulerStatusLabel(status: SchedulerStatus): string {
  return STATUS_LABELS[status];
}

export function deriveSchedulerPosts(
  entries: RenderQueueEntry[],
  nowSeconds = Math.floor(Date.now() / 1000),
): SchedulerPost[] {
  return entries.flatMap((entry) => {
    const targets = entry.uploadTargets ?? Object.keys(entry.uploadStates ?? {});
    return targets.map((provider) =>
      postFromRenderQueueEntry(entry, provider, nowSeconds),
    );
  }).sort((a, b) => a.scheduledAt - b.scheduledAt || a.title.localeCompare(b.title));
}

export function formatSchedulerTime(epochSeconds: number, timezone = "UTC"): string {
  const formatter = new Intl.DateTimeFormat("en-CA", {
    timeZone: timezone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
  return `${formatter.format(new Date(epochSeconds * 1000)).replace(",", "")} ${timezone}`;
}

function postFromRenderQueueEntry(
  entry: RenderQueueEntry,
  provider: string,
  nowSeconds: number,
): SchedulerPost {
  const metadata = entry.uploadMetadata?.[provider];
  const state = entry.uploadStates?.[provider] ?? { state: "pending" as const };
  const title = metadata?.title?.trim() || entry.label;
  return {
    id: `${entry.id}:${provider}`,
    renderQueueId: entry.id,
    renderJobId: entry.jobId,
    provider,
    title,
    description: metadata?.description ?? "",
    visibility: metadata?.visibility ?? "private",
    scheduledAt: metadata?.scheduledAt ?? entry.updatedAt ?? nowSeconds,
    status: schedulerStatusFromUploadState(state),
    jobId: jobIdFromUploadState(state),
    outputPath: entry.outputPath,
    providerUrl: providerUrlFromUploadState(state) ?? entry.publishedUrls?.[provider],
    failureReason: state.state === "failed" ? state.reason : undefined,
    updatedAt: entry.updatedAt,
  };
}

function schedulerStatusFromUploadState(state: RenderUploadState): SchedulerStatus {
  if (state.state === "pending") return "draft";
  if (state.state === "uploading") return "uploading";
  if (state.state === "scheduled") return "scheduled";
  if (state.state === "processing") return "processing";
  if (state.state === "published") return "published";
  return state.reason === "server publish requires_action" ? "requires_action" : "failed";
}

function jobIdFromUploadState(state: RenderUploadState): string | undefined {
  if (state.state === "scheduled" || state.state === "processing") return state.job_id;
  if (state.state === "failed") return state.job_id;
  return undefined;
}

function providerUrlFromUploadState(state: RenderUploadState): string | undefined {
  return state.state === "published" ? state.remote_url : undefined;
}
```

- [ ] **Step 4: Add the package script**

In `apps/desktop/package.json`, add:

```json
"test:scheduler-model": "node --experimental-strip-types tests/scheduler-model.test.ts",
```

Place it beside the other `test:*` scripts.

- [ ] **Step 5: Verify and commit**

Run:

```bash
cd apps/desktop
pnpm test:scheduler-model
pnpm exec tsc --noEmit
```

Expected: `scheduler-model: OK`; TypeScript exits 0.

Commit:

```bash
git add apps/desktop/src/app/scheduler/schedulerModel.ts apps/desktop/tests/scheduler-model.test.ts apps/desktop/package.json
git commit -m "feat(desktop): derive scheduler posts from render queue"
```

---

## Task 2: Add Schedule as a Stage Destination

**Files:**
- Modify: `apps/desktop/src/state/stages.ts`
- Modify: `apps/desktop/src/shell/StageShell.tsx`

- [ ] **Step 1: Write the failing test**

Create a short assertion in the existing `apps/desktop/tests/center-mode.test.ts` is the wrong place, so do not mix concerns. Instead add this focused check to the new scheduler model test from Task 1 after the existing assertions:

```ts
const { STAGE_LABEL } = await import("../src/state/stages.ts");
assert.equal(STAGE_LABEL.schedule, "Schedule");
```

Run:

```bash
cd apps/desktop
pnpm test:scheduler-model
```

Expected: FAIL because `schedule` is not in `STAGE_LABEL`.

- [ ] **Step 2: Extend the stage type**

In `apps/desktop/src/state/stages.ts`, change the Stage union and label map:

```ts
export type Stage = (typeof STAGES)[number] | "schedule" | "skills" | "history";

export const STAGE_LABEL: Record<Stage, string> = {
  edit: "Edit",
  deliver: "Deliver",
  schedule: "Schedule",
  skills: "Skills",
  history: "History",
};
```

Update `DEV_INITIAL_STAGE`:

```ts
  return v === "deliver" || v === "schedule" || v === "skills" || v === "history" ? v : "edit";
```

- [ ] **Step 3: Add StageShell prop and dock item**

In `apps/desktop/src/shell/StageShell.tsx`, add a dock item:

```ts
const DOCK: DockItem[] = [
  { id: "edit", glyph: "▶", label: "Stage" },
  { id: "deliver", glyph: "↑", label: "Deliver" },
  { id: "schedule", glyph: "◷", label: "Schedule" },
  { id: "skills", glyph: "✦", label: "Skills" },
  { id: "history", glyph: "◷", label: "History" },
];
```

Add the prop to `StageShellProps`:

```ts
  schedule: ReactNode;
```

Destructure it:

```ts
    deliver, schedule, skills, history,
```

Update command routing:

```ts
    } else if (lower === "deliver" || lower === "schedule" || lower === "skills" || lower === "history" || lower === "stage" || lower === "edit") {
      onStage(lower === "stage" ? "edit" : (lower as Stage));
```

Update destination rendering:

```tsx
{stage === "deliver"
  ? deliver
  : stage === "schedule"
    ? schedule
    : stage === "skills"
      ? skills
      : stage === "history"
        ? history
        : null}
```

Update command shortcut list:

```ts
{(["deliver", "schedule", "skills", "history"] as Stage[]).map((s) => (
```

- [ ] **Step 4: Verify and commit**

Run:

```bash
cd apps/desktop
pnpm test:scheduler-model
pnpm exec tsc --noEmit
```

Expected: test passes; TypeScript exits 0.

Commit:

```bash
git add apps/desktop/src/state/stages.ts apps/desktop/src/shell/StageShell.tsx apps/desktop/tests/scheduler-model.test.ts
git commit -m "feat(desktop): add schedule workspace destination"
```

---

## Task 3: Read-Only Scheduler Workspace UI

**Files:**
- Create: `apps/desktop/src/app/scheduler/SchedulerWorkspace.tsx`
- Modify: `apps/desktop/src/App.tsx`

- [ ] **Step 1: Implement the component**

Create `apps/desktop/src/app/scheduler/SchedulerWorkspace.tsx`:

```tsx
import { useMemo, useState } from "react";
import { CalendarDays, ExternalLink, ListChecks } from "lucide-react";
import { useRenderQueueStore } from "../renderQueue";
import {
  deriveSchedulerPosts,
  formatSchedulerTime,
  schedulerStatusLabel,
  type SchedulerPost,
} from "./schedulerModel";

export function SchedulerWorkspace() {
  const entries = useRenderQueueStore((state) => state.entries);
  const posts = useMemo(() => deriveSchedulerPosts(entries), [entries]);
  const [view, setView] = useState<"calendar" | "queue">("calendar");
  const [selectedId, setSelectedId] = useState<string | null>(posts[0]?.id ?? null);
  const selected = posts.find((post) => post.id === selectedId) ?? posts[0] ?? null;

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <header className="flex shrink-0 items-center gap-3 border-b border-[var(--glass-border)] px-5 py-3">
        <div>
          <h2 className="text-[16px] font-bold text-[var(--color-text-primary)]">Schedule</h2>
          <p className="text-[12px] text-[var(--color-text-secondary)]">
            Review scheduled, staged, published, and failed posts from the server-backed publish queue.
          </p>
        </div>
        <div className="ml-auto flex items-center gap-1 rounded-xl border border-[var(--color-border-subtle)] p-1">
          <button
            type="button"
            onClick={() => setView("calendar")}
            className={viewButton(view === "calendar")}
          >
            <CalendarDays className="h-3.5 w-3.5" />
            Calendar
          </button>
          <button
            type="button"
            onClick={() => setView("queue")}
            className={viewButton(view === "queue")}
          >
            <ListChecks className="h-3.5 w-3.5" />
            Queue
          </button>
        </div>
      </header>
      <main className="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_320px] gap-3 overflow-hidden p-4">
        <section className="min-h-0 overflow-auto">
          {posts.length === 0 ? (
            <EmptyScheduler />
          ) : view === "calendar" ? (
            <CalendarGrid posts={posts} selectedId={selected?.id ?? null} onSelect={setSelectedId} />
          ) : (
            <QueueList posts={posts} selectedId={selected?.id ?? null} onSelect={setSelectedId} />
          )}
        </section>
        <PostDrawer post={selected} />
      </main>
    </div>
  );
}

function viewButton(active: boolean): string {
  return [
    "inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[12px] transition",
    active
      ? "bg-[var(--color-surface-elevated)] text-[var(--color-text-primary)]"
      : "text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]",
  ].join(" ");
}

function EmptyScheduler() {
  return (
    <div className="grid h-full min-h-[360px] place-items-center rounded-xl border border-dashed border-[var(--color-border-subtle)] text-center">
      <div className="max-w-[320px]">
        <p className="text-[14px] font-semibold text-[var(--color-text-primary)]">No scheduled posts yet</p>
        <p className="mt-1 text-[12px] leading-relaxed text-[var(--color-text-secondary)]">
          Render a clip with upload targets selected, then this workspace will show its publish status here.
        </p>
      </div>
    </div>
  );
}

function CalendarGrid({
  posts,
  selectedId,
  onSelect,
}: {
  posts: SchedulerPost[];
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  const first = posts[0]?.scheduledAt ?? Math.floor(Date.now() / 1000);
  const start = startOfWeek(first);
  const days = Array.from({ length: 14 }, (_, index) => start + index * 86_400);
  return (
    <div className="grid min-w-[760px] grid-cols-7 overflow-hidden rounded-xl border border-[var(--color-border-subtle)]">
      {days.map((day) => {
        const dayPosts = posts.filter((post) => sameDay(post.scheduledAt, day));
        return (
          <div key={day} className="min-h-[136px] border-r border-b border-[var(--color-border-subtle)] p-2 last:border-r-0">
            <div className="mb-2 text-[11px] font-semibold text-[var(--color-text-muted)]">
              {formatSchedulerTime(day, "UTC").slice(0, 10)}
            </div>
            <div className="space-y-1.5">
              {dayPosts.map((post) => (
                <PostCard
                  key={post.id}
                  post={post}
                  active={post.id === selectedId}
                  onSelect={onSelect}
                />
              ))}
            </div>
          </div>
        );
      })}
    </div>
  );
}

function QueueList({
  posts,
  selectedId,
  onSelect,
}: {
  posts: SchedulerPost[];
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  return (
    <div className="space-y-2">
      {posts.map((post) => (
        <PostCard
          key={post.id}
          post={post}
          active={post.id === selectedId}
          onSelect={onSelect}
          dense={false}
        />
      ))}
    </div>
  );
}

function PostCard({
  post,
  active,
  onSelect,
  dense = true,
}: {
  post: SchedulerPost;
  active: boolean;
  onSelect: (id: string) => void;
  dense?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={() => onSelect(post.id)}
      className={[
        "w-full rounded-lg border px-2 py-2 text-left transition",
        active
          ? "border-[var(--color-brand-secondary)] bg-[var(--color-surface-elevated)]"
          : "border-[var(--color-border-subtle)] bg-[var(--color-surface)] hover:border-[var(--color-text-muted)]",
      ].join(" ")}
    >
      <div className="flex items-center gap-2">
        <span className="h-1.5 w-1.5 rounded-full bg-[var(--color-brand-secondary)]" aria-hidden />
        <span className="min-w-0 flex-1 truncate text-[12px] font-semibold text-[var(--color-text-primary)]">
          {post.title}
        </span>
      </div>
      <div className="mt-1 flex items-center gap-2 text-[11px] text-[var(--color-text-secondary)]">
        <span className="capitalize">{post.provider}</span>
        <span>·</span>
        <span>{schedulerStatusLabel(post.status)}</span>
      </div>
      {!dense && post.description ? (
        <p className="mt-1 line-clamp-2 text-[12px] text-[var(--color-text-secondary)]">
          {post.description}
        </p>
      ) : null}
    </button>
  );
}

function PostDrawer({ post }: { post: SchedulerPost | null }) {
  if (!post) {
    return (
      <aside className="rounded-xl border border-[var(--color-border-subtle)] p-4 text-[12px] text-[var(--color-text-secondary)]">
        Select a post to review its schedule and delivery state.
      </aside>
    );
  }
  return (
    <aside className="min-h-0 overflow-auto rounded-xl border border-[var(--color-border-subtle)] p-4">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <h3 className="text-[14px] font-bold text-[var(--color-text-primary)]">{post.title}</h3>
          <p className="mt-1 text-[12px] text-[var(--color-text-secondary)]">
            {post.provider} · {schedulerStatusLabel(post.status)}
          </p>
        </div>
        {post.providerUrl ? (
          <a
            href={post.providerUrl}
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-1 rounded-lg border border-[var(--color-border-subtle)] px-2 py-1 text-[11px] text-[var(--color-text-secondary)]"
          >
            <ExternalLink className="h-3 w-3" />
            Open
          </a>
        ) : null}
      </div>
      <dl className="mt-4 space-y-3 text-[12px]">
        <Detail label="Time" value={formatSchedulerTime(post.scheduledAt, "UTC")} />
        <Detail label="Visibility" value={post.visibility} />
        <Detail label="Render" value={post.outputPath ?? "No render file"} />
        <Detail label="Job" value={post.jobId ?? "No publish job yet"} />
        {post.description ? <Detail label="Caption" value={post.description} /> : null}
        {post.failureReason ? <Detail label="Failure" value={post.failureReason} /> : null}
      </dl>
    </aside>
  );
}

function Detail({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt className="text-[11px] font-semibold uppercase text-[var(--color-text-muted)]">{label}</dt>
      <dd className="mt-1 break-words text-[12px] text-[var(--color-text-primary)]">{value}</dd>
    </div>
  );
}

function startOfWeek(epochSeconds: number): number {
  const date = new Date(epochSeconds * 1000);
  const day = date.getUTCDay();
  date.setUTCHours(0, 0, 0, 0);
  date.setUTCDate(date.getUTCDate() - day);
  return Math.floor(date.getTime() / 1000);
}

function sameDay(a: number, b: number): boolean {
  const da = new Date(a * 1000);
  const db = new Date(b * 1000);
  return (
    da.getUTCFullYear() === db.getUTCFullYear() &&
    da.getUTCMonth() === db.getUTCMonth() &&
    da.getUTCDate() === db.getUTCDate()
  );
}
```

- [ ] **Step 2: Mount from App**

In `apps/desktop/src/App.tsx`, import:

```ts
import { SchedulerWorkspace } from "./app/scheduler/SchedulerWorkspace";
```

Create the node near `realDeliveryWorkspace`:

```tsx
  const realSchedulerWorkspace = <SchedulerWorkspace />;
```

Pass it into `StageShell`:

```tsx
        schedule={realSchedulerWorkspace}
```

Add the prop wherever `StageShell` is rendered. There is only one active StageShell path with `STAGE_SHELL = true`; update any compile-visible fallback render too if TypeScript requires it.

- [ ] **Step 3: Verify and commit**

Run:

```bash
cd apps/desktop
pnpm exec tsc --noEmit
```

Expected: TypeScript exits 0.

Commit:

```bash
git add apps/desktop/src/app/scheduler/SchedulerWorkspace.tsx apps/desktop/src/App.tsx
git commit -m "feat(desktop): add read-only scheduler workspace"
```

---

## Task 4: Browser Verification and Polish Pass

**Files:**
- Modify only files from Tasks 1-3 if verification finds concrete issues.

- [ ] **Step 1: Run desktop build check**

Run:

```bash
cd apps/desktop
pnpm exec tsc --noEmit
```

Expected: TypeScript exits 0.

- [ ] **Step 2: Run focused tests**

Run:

```bash
cd apps/desktop
pnpm test:scheduler-model
pnpm test:render-queue-upload
```

Expected: both tests exit 0.

- [ ] **Step 3: Start desktop dev app**

Run:

```bash
cd apps/desktop
MONTAGE_SOCIAL_SERVER_URL=http://127.0.0.1:3000 \
MONTAGE_SOCIAL_AUTH_TOKEN=local-dev-token \
pnpm tauri dev
```

Expected: app starts without TypeScript or Tauri command registration errors.

- [ ] **Step 4: Verify the Scheduler surface**

In the app:

1. Open a project with existing render queue rows.
2. Click the left dock `Schedule` item.
3. Confirm the destination sheet opens and shows `Schedule`.
4. Confirm Calendar and Queue toggles switch views.
5. Select a post card and confirm the right drawer shows time, visibility, render path, job id, description, failure reason, or provider URL based on available state.
6. Confirm there is no publish/schedule write action yet.

Expected: the workspace is visible inside the product, derives posts from existing render queue state, and does not call new server write commands.

- [ ] **Step 5: Final commit if polish was needed**

If Step 4 required fixes:

```bash
git add apps/desktop/src/app/scheduler apps/desktop/src/App.tsx apps/desktop/src/shell/StageShell.tsx apps/desktop/src/state/stages.ts apps/desktop/package.json apps/desktop/tests/scheduler-model.test.ts
git commit -m "fix(desktop): polish scheduler workspace shell"
```

If no fixes were needed, do not create an empty commit.

---

## Coverage Checklist

- Scheduler is an actual product destination: Task 2 and Task 3.
- Calendar and queue views exist: Task 3.
- Post review drawer exists: Task 3.
- Existing render queue and server-backed status drive the surface: Task 1 and Task 3.
- No frontend-only scheduled firing is added: all tasks are read-only.
- Bulk scheduling, source picker, metadata generation, and approval writes remain deferred to Slice 2 and Slice 3 by design.
