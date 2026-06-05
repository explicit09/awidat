import {
  CalendarDays,
  Clock3,
  ExternalLink,
  FileVideo,
  ListTodo,
  Lock,
  Upload,
} from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";

import { useRenderQueueStore } from "../renderQueue";
import {
  deriveSchedulerPosts,
  formatSchedulerTime,
  schedulerStatusLabel,
  type SchedulerPost,
  type SchedulerStatus,
} from "./schedulerModel";

type ViewMode = "calendar" | "queue";

type CalendarDay = {
  key: string;
  day: number;
  inMonth: boolean;
};

const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

const STATUS_STYLES: Record<SchedulerStatus, string> = {
  draft:
    "border-[var(--color-job-idle-border)] bg-[var(--color-job-idle-fill)] text-[var(--color-job-idle-text)]",
  scheduled:
    "border-[var(--color-job-running-border)] bg-[var(--color-job-running-fill)] text-[var(--color-job-running-text)]",
  uploading:
    "border-[rgba(59,130,246,0.4)] bg-[rgba(59,130,246,0.1)] text-[#93c5fd]",
  processing:
    "border-[rgba(59,130,246,0.4)] bg-[rgba(59,130,246,0.1)] text-[#93c5fd]",
  published:
    "border-[var(--color-job-ready-border)] bg-[var(--color-job-ready-fill)] text-[var(--color-job-ready-text)]",
  failed:
    "border-[var(--color-job-failed-border)] bg-[var(--color-job-failed-fill)] text-[var(--color-job-failed-text)]",
  requires_action:
    "border-[rgba(245,158,11,0.45)] bg-[rgba(245,158,11,0.1)] text-[#fcd34d]",
  cancelled:
    "border-[var(--color-job-idle-border)] bg-[var(--color-job-idle-fill)] text-[var(--color-text-disabled)]",
};

export function SchedulerWorkspace() {
  const entries = useRenderQueueStore((s) => s.entries);
  const [viewMode, setViewMode] = useState<ViewMode>("calendar");
  const [selectedPostId, setSelectedPostId] = useState<string | null>(null);

  const timeZone = useMemo(
    () => Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
    [],
  );
  const posts = useMemo(() => deriveSchedulerPosts(entries), [entries]);
  const selectedPost =
    posts.find((post) => post.id === selectedPostId) ?? posts[0] ?? null;
  const anchorDate = selectedPost
    ? new Date(selectedPost.scheduledAt * 1000)
    : new Date();
  const calendarDays = useMemo(() => buildCalendarDays(anchorDate), [anchorDate]);
  const postsByDay = useMemo(() => groupPostsByDay(posts), [posts]);

  return (
    <section className="flex h-full min-h-0 flex-col bg-[rgba(8,9,12,0.36)] text-[var(--color-text-primary)]">
      <header className="shrink-0 border-b border-[var(--glass-border)] px-5 py-4">
        <div className="flex flex-wrap items-start gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2 text-[12px] font-semibold uppercase text-[var(--color-text-muted)]">
              <CalendarDays className="h-4 w-4 text-[#FF9A45]" />
              Calendar workspace
            </div>
            <h2 className="mt-1 m-0 text-[22px] font-bold tracking-[0] text-[var(--color-text-primary)]">
              Schedule
            </h2>
          </div>
          <div className="flex shrink-0 flex-wrap items-center gap-2">
            <button
              type="button"
              disabled
              className="glass-ghost inline-flex cursor-not-allowed items-center gap-2 rounded-lg px-3 py-2 text-[12px] opacity-70"
            >
              <CalendarDays className="h-3.5 w-3.5" />
              Schedule post
            </button>
            <button
              type="button"
              disabled
              className="glass-ghost inline-flex cursor-not-allowed items-center gap-2 rounded-lg px-3 py-2 text-[12px] opacity-70"
            >
              <Upload className="h-3.5 w-3.5" />
              Upload local video
            </button>
            <span className="glass-ghost inline-flex items-center gap-2 rounded-lg px-3 py-2 font-mono text-[12px] text-[var(--color-text-secondary)]">
              <Clock3 className="h-3.5 w-3.5 text-[#FF9A45]" />
              {formatGmtOffset()}
            </span>
          </div>
        </div>

        <div className="mt-4 flex flex-wrap items-center justify-between gap-3">
          <div>
            <div className="text-[15px] font-semibold text-[var(--color-text-primary)]">
              {monthLabel(anchorDate)}
            </div>
            <div className="text-[12px] text-[var(--color-text-muted)]">
              Read-only view of rendered posts and provider jobs.
            </div>
          </div>
          <div className="glass-ghost inline-flex rounded-lg p-1">
            <ModeButton
              icon={<CalendarDays className="h-3.5 w-3.5" />}
              active={viewMode === "calendar"}
              label="Calendar"
              onClick={() => setViewMode("calendar")}
            />
            <ModeButton
              icon={<ListTodo className="h-3.5 w-3.5" />}
              active={viewMode === "queue"}
              label="Queue"
              onClick={() => setViewMode("queue")}
            />
          </div>
        </div>
      </header>

      <div className="grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_308px]">
        <main className="min-h-0 overflow-auto p-4">
          {viewMode === "calendar" ? (
            <CalendarView
              days={calendarDays}
              postsByDay={postsByDay}
              selectedPostId={selectedPost?.id}
              onSelectPost={setSelectedPostId}
            />
          ) : (
            <QueueView
              posts={posts}
              selectedPostId={selectedPost?.id}
              timeZone={timeZone}
              onSelectPost={setSelectedPostId}
            />
          )}
        </main>
        <ReviewDrawer post={selectedPost} timeZone={timeZone} />
      </div>
    </section>
  );
}

function ModeButton({
  active,
  icon,
  label,
  onClick,
}: {
  active: boolean;
  icon: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`inline-flex items-center gap-2 rounded-md px-3 py-1.5 text-[12px] font-semibold transition ${
        active
          ? "bg-[rgba(255,122,24,0.18)] text-[#FFB073]"
          : "text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]"
      }`}
    >
      {icon}
      {label}
    </button>
  );
}

function CalendarView({
  days,
  postsByDay,
  selectedPostId,
  onSelectPost,
}: {
  days: CalendarDay[];
  postsByDay: Map<string, SchedulerPost[]>;
  selectedPostId?: string;
  onSelectPost: (postId: string) => void;
}) {
  const hasPosts = postsByDay.size > 0;

  return (
    <div className="min-w-[620px]">
      <div className="grid grid-cols-7 overflow-hidden rounded-lg border border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)]">
        {WEEKDAYS.map((day) => (
          <div
            key={day}
            className="border-b border-r border-[var(--glass-border)] px-3 py-2 text-[11px] font-semibold uppercase text-[var(--color-text-muted)] last:border-r-0"
          >
            {day}
          </div>
        ))}
        {days.map((day, index) => {
          const dayPosts = postsByDay.get(day.key) ?? [];
          const isLastColumn = index % 7 === 6;
          return (
            <div
              key={day.key}
              className={`min-h-[94px] border-b border-r border-[var(--glass-border)] p-2 ${
                isLastColumn ? "border-r-0" : ""
              } ${day.inMonth ? "bg-[rgba(12,14,20,0.54)]" : "bg-[rgba(255,255,255,0.018)]"}`}
            >
              <div
                className={`font-mono text-[11px] ${
                  day.inMonth
                    ? "text-[var(--color-text-secondary)]"
                    : "text-[var(--color-text-disabled)]"
                }`}
              >
                {day.day}
              </div>
              <div className="mt-2 flex flex-col gap-1.5">
                {dayPosts.slice(0, 3).map((post) => (
                  <button
                    key={post.id}
                    type="button"
                    onClick={() => onSelectPost(post.id)}
                    className={`min-h-[34px] rounded-md border px-2 py-1 text-left transition ${
                      post.id === selectedPostId
                        ? "border-[#FF9A45] bg-[rgba(255,122,24,0.18)]"
                        : "border-[var(--glass-border)] bg-[rgba(255,255,255,0.035)] hover:border-[var(--glass-border-strong)]"
                    }`}
                  >
                    <div className="truncate text-[11px] font-semibold text-[var(--color-text-primary)]">
                      {post.title}
                    </div>
                    <div className="mt-0.5 flex items-center gap-1.5 text-[10px] text-[var(--color-text-muted)]">
                      <span>{timeLabel(post.scheduledAt)}</span>
                      <span className="h-1 w-1 rounded-full bg-[var(--color-text-disabled)]" />
                      <span className="truncate">{providerLabel(post.provider)}</span>
                    </div>
                  </button>
                ))}
                {dayPosts.length > 3 ? (
                  <div className="px-1 text-[10px] text-[var(--color-text-muted)]">
                    +{dayPosts.length - 3} more
                  </div>
                ) : null}
              </div>
            </div>
          );
        })}
      </div>
      {!hasPosts ? <EmptyWorkspace /> : null}
    </div>
  );
}

function QueueView({
  posts,
  selectedPostId,
  timeZone,
  onSelectPost,
}: {
  posts: SchedulerPost[];
  selectedPostId?: string;
  timeZone: string;
  onSelectPost: (postId: string) => void;
}) {
  if (posts.length === 0) return <EmptyWorkspace />;

  return (
    <div className="flex min-w-[620px] flex-col gap-2">
      {posts.map((post) => (
        <button
          key={post.id}
          type="button"
          onClick={() => onSelectPost(post.id)}
          className={`grid min-h-[78px] grid-cols-[minmax(0,1fr)_128px_104px] items-center gap-3 rounded-lg border px-3 py-3 text-left transition ${
            post.id === selectedPostId
              ? "border-[#FF9A45] bg-[rgba(255,122,24,0.14)]"
              : "border-[var(--glass-border)] bg-[rgba(255,255,255,0.035)] hover:border-[var(--glass-border-strong)]"
          }`}
        >
          <div className="min-w-0">
            <div className="truncate text-[13px] font-semibold text-[var(--color-text-primary)]">
              {post.title}
            </div>
            <div className="mt-1 truncate text-[12px] text-[var(--color-text-muted)]">
              {post.description || "No generated caption yet."}
            </div>
          </div>
          <div className="min-w-0 text-[11px] text-[var(--color-text-muted)]">
            <div className="truncate font-mono text-[var(--color-text-secondary)]">
              {formatSchedulerTime(post.scheduledAt, timeZone)}
            </div>
            <div className="mt-1 truncate">{providerLabel(post.provider)}</div>
          </div>
          <StatusPill status={post.status} />
        </button>
      ))}
    </div>
  );
}

function ReviewDrawer({
  post,
  timeZone,
}: {
  post: SchedulerPost | null;
  timeZone: string;
}) {
  return (
    <aside className="min-h-0 border-l border-[var(--glass-border)] bg-[rgba(8,10,14,0.54)]">
      <div className="flex h-full min-h-0 flex-col">
        <div className="shrink-0 border-b border-[var(--glass-border)] px-4 py-3">
          <div className="flex items-center gap-2 text-[12px] font-semibold text-[var(--color-text-secondary)]">
            <FileVideo className="h-4 w-4 text-[#FF9A45]" />
            Review drawer
          </div>
        </div>
        {post ? (
          <div className="min-h-0 flex-1 overflow-auto px-4 py-4">
            <div className="rounded-lg border border-[var(--glass-border)] bg-[rgba(255,255,255,0.035)] p-3">
              <div className="flex items-start gap-3">
                <div className="grid h-10 w-10 shrink-0 place-items-center rounded-lg bg-[rgba(255,122,24,0.16)] text-[#FFB073]">
                  <CalendarDays className="h-5 w-5" />
                </div>
                <div className="min-w-0 flex-1">
                  <h3 className="m-0 text-[15px] font-bold leading-snug text-[var(--color-text-primary)]">
                    {post.title}
                  </h3>
                  <div className="mt-2 flex flex-wrap items-center gap-2">
                    <StatusPill status={post.status} />
                    <span className="inline-flex items-center gap-1 rounded-md border border-[var(--glass-border)] px-2 py-1 text-[11px] text-[var(--color-text-secondary)]">
                      <Lock className="h-3 w-3" />
                      {post.visibility}
                    </span>
                  </div>
                </div>
              </div>
            </div>

            <dl className="mt-4 grid gap-3">
              <Metadata label="Time">
                {formatSchedulerTime(post.scheduledAt, timeZone)}
              </Metadata>
              <Metadata label="Platform">{providerLabel(post.provider)}</Metadata>
              <Metadata label="Render path">{post.outputPath ?? "Not rendered yet"}</Metadata>
              <Metadata label="Render queue id">{post.renderQueueId}</Metadata>
              <Metadata label="Render job id">{post.renderJobId ?? "Not assigned"}</Metadata>
              <Metadata label="Provider job id">{post.jobId ?? "Not assigned"}</Metadata>
              <Metadata label="Provider URL">
                {post.providerUrl ? (
                  <a
                    href={post.providerUrl}
                    target="_blank"
                    rel="noreferrer"
                    className="inline-flex max-w-full items-center gap-1 text-[#7DD3FC] hover:text-[#BAE6FD]"
                  >
                    <span className="truncate">{post.providerUrl}</span>
                    <ExternalLink className="h-3 w-3 shrink-0" />
                  </a>
                ) : (
                  "Not published yet"
                )}
              </Metadata>
              <Metadata label="Failure reason">
                {post.failureReason ?? "No failure recorded"}
              </Metadata>
            </dl>

            <section className="mt-4 rounded-lg border border-[var(--glass-border)] bg-[rgba(255,255,255,0.03)] p-3">
              <div className="text-[12px] font-semibold text-[var(--color-text-secondary)]">
                Generated copy
              </div>
              <p className="mt-2 whitespace-pre-wrap break-words text-[12px] leading-relaxed text-[var(--color-text-primary)]">
                {post.description || "No caption or description was saved with this render."}
              </p>
            </section>
          </div>
        ) : (
          <div className="grid min-h-0 flex-1 place-items-center px-5 text-center">
            <div>
              <div className="mx-auto grid h-11 w-11 place-items-center rounded-lg border border-[var(--glass-border)] bg-[rgba(255,255,255,0.035)] text-[var(--color-text-muted)]">
                <CalendarDays className="h-5 w-5" />
              </div>
              <div className="mt-3 text-[13px] font-semibold text-[var(--color-text-primary)]">
                Nothing to review yet
              </div>
              <p className="mt-1 text-[12px] leading-relaxed text-[var(--color-text-muted)]">
                Finished renders with upload targets will appear here as
                read-only scheduler cards.
              </p>
            </div>
          </div>
        )}
      </div>
    </aside>
  );
}

function Metadata({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <div className="min-w-0 rounded-lg border border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)] p-3">
      <dt className="text-[11px] font-semibold uppercase text-[var(--color-text-muted)]">
        {label}
      </dt>
      <dd className="mt-1 break-words text-[12px] text-[var(--color-text-primary)]">
        {children}
      </dd>
    </div>
  );
}

function StatusPill({ status }: { status: SchedulerStatus }) {
  return (
    <span
      className={`inline-flex w-fit items-center rounded-full border px-2 py-1 text-[11px] font-semibold ${STATUS_STYLES[status]}`}
    >
      {schedulerStatusLabel(status)}
    </span>
  );
}

function EmptyWorkspace() {
  return (
    <div className="mt-4 rounded-lg border border-dashed border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)] p-8 text-center">
      <div className="mx-auto grid h-12 w-12 place-items-center rounded-lg bg-[rgba(255,122,24,0.14)] text-[#FFB073]">
        <CalendarDays className="h-6 w-6" />
      </div>
      <h3 className="mt-4 m-0 text-[15px] font-bold text-[var(--color-text-primary)]">
        Calendar is ready
      </h3>
      <p className="mx-auto mt-2 max-w-[420px] text-[12px] leading-relaxed text-[var(--color-text-muted)]">
        No scheduled or published render targets are in the queue yet. This
        slice only reads existing render metadata; scheduling actions are
        intentionally inactive.
      </p>
    </div>
  );
}

function buildCalendarDays(anchorDate: Date): CalendarDay[] {
  const year = anchorDate.getFullYear();
  const month = anchorDate.getMonth();
  const firstOfMonth = new Date(year, month, 1);
  const mondayOffset = (firstOfMonth.getDay() + 6) % 7;
  const gridStart = new Date(year, month, 1 - mondayOffset);

  return Array.from({ length: 42 }, (_, index) => {
    const date = new Date(
      gridStart.getFullYear(),
      gridStart.getMonth(),
      gridStart.getDate() + index,
    );
    return {
      key: dateKey(date),
      day: date.getDate(),
      inMonth: date.getMonth() === month,
    };
  });
}

function groupPostsByDay(posts: SchedulerPost[]): Map<string, SchedulerPost[]> {
  const grouped = new Map<string, SchedulerPost[]>();
  for (const post of posts) {
    const key = dateKey(new Date(post.scheduledAt * 1000));
    const existing = grouped.get(key);
    if (existing) {
      existing.push(post);
    } else {
      grouped.set(key, [post]);
    }
  }
  return grouped;
}

function dateKey(date: Date): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function monthLabel(date: Date): string {
  return new Intl.DateTimeFormat("en-US", {
    month: "long",
    year: "numeric",
  }).format(date);
}

function timeLabel(epochSeconds: number): string {
  return new Intl.DateTimeFormat("en-US", {
    hour: "numeric",
    minute: "2-digit",
  }).format(new Date(epochSeconds * 1000));
}

function providerLabel(provider: string): string {
  return provider
    .split(/[-_\s]+/)
    .filter(Boolean)
    .map((part) => part[0]?.toUpperCase() + part.slice(1))
    .join(" ");
}

function formatGmtOffset(): string {
  const offsetMinutes = -new Date().getTimezoneOffset();
  const sign = offsetMinutes >= 0 ? "+" : "-";
  const absolute = Math.abs(offsetMinutes);
  const hours = String(Math.floor(absolute / 60)).padStart(2, "0");
  const minutes = absolute % 60;
  return minutes === 0
    ? `GMT${sign}${hours}`
    : `GMT${sign}${hours}:${String(minutes).padStart(2, "0")}`;
}
