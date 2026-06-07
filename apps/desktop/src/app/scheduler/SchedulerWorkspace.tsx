import {
  Ban,
  CalendarDays,
  Clock3,
  ExternalLink,
  FileVideo,
  KeyRound,
  ListTodo,
  Lock,
  RefreshCw,
  RotateCcw,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";

import {
  useRenderQueueStore,
  type RenderQueueEntry,
} from "../renderQueue";
import { refreshServerUploadState } from "../useRenderQueueWorker";
import type {
  TikTokInteractionSettings,
  UploadVisibility,
} from "../../state/uploadMetadata";
import type { AccountSummary, Provider } from "../social/socialModel";
import { startConnect } from "../social/socialActions";
import {
  deriveSchedulerPostActions,
  deriveSchedulerPosts,
  formatSchedulerTime,
  schedulerStatusLabel,
  type SchedulerPost,
  type SchedulerStatus,
} from "./schedulerModel";
import {
  buildSchedulerMetadata,
  buildSchedulerCadenceSlots,
  buildSchedulerMetadataProfile,
  loadSchedulerAccounts,
  mergeSchedulerMetadataEdit,
  mergeSchedulerPublishResult,
  publishSchedulerPostToAccounts,
  schedulerMetadataControlProvider,
  schedulerMetadataFieldConfig,
  schedulerPublishableEntries,
  updateSchedulerTargetMetadata,
  validateSchedulerMetadataForAccounts,
  type SchedulerPublishAccount,
} from "./schedulerPublish";

type ViewMode = "calendar" | "queue";

type CalendarDay = {
  key: string;
  day: number;
  inMonth: boolean;
};

const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const SOCIAL_PROVIDERS: readonly Provider[] = [
  "youtube",
  "tiktok",
  "instagram",
  "twitter_x",
];

function defaultTikTokInteractions(): TikTokInteractionSettings {
  return {
    disableDuet: false,
    disableComment: false,
    disableStitch: false,
  };
}

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
  const [accounts, setAccounts] = useState<AccountSummary[]>([]);
  const [accountError, setAccountError] = useState<string | null>(null);
  const [accountsBusy, setAccountsBusy] = useState(false);

  const timeZone = useMemo(
    () => Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
    [],
  );
  const posts = useMemo(() => deriveSchedulerPosts(entries), [entries]);
  const selectedPost =
    posts.find((post) => post.id === selectedPostId) ?? posts[0] ?? null;
  const selectedEntry =
    selectedPost
      ? entries.find((entry) => entry.id === selectedPost.renderQueueId) ?? null
      : null;
  const anchorDate = selectedPost
    ? new Date(selectedPost.scheduledAt * 1000)
    : new Date();
  const calendarDays = useMemo(() => buildCalendarDays(anchorDate), [anchorDate]);
  const postsByDay = useMemo(() => groupPostsByDay(posts), [posts]);

  const refreshAccounts = useCallback(async () => {
    setAccountsBusy(true);
    const result = await loadSchedulerAccounts(invoke);
    setAccounts(result.accounts);
    setAccountError(result.error);
    setAccountsBusy(false);
  }, []);

  useEffect(() => {
    let cancelled = false;
    async function loadAccountsOnce() {
      const result = await loadSchedulerAccounts(invoke);
      if (!cancelled) {
        setAccounts(result.accounts);
        setAccountError(result.error);
      }
    }
    void loadAccountsOnce();
    return () => {
      cancelled = true;
    };
  }, []);

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
              Rendered posts and provider jobs.
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
        <ReviewDrawer
          post={selectedPost}
          entry={selectedEntry}
          entries={entries}
          accounts={accounts}
          accountError={accountError}
          accountsBusy={accountsBusy}
          timeZone={timeZone}
          onRefreshAccounts={refreshAccounts}
          onSelectPost={setSelectedPostId}
        />
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
  entry,
  entries,
  accounts,
  accountError,
  accountsBusy,
  timeZone,
  onRefreshAccounts,
  onSelectPost,
}: {
  post: SchedulerPost | null;
  entry: RenderQueueEntry | null;
  entries: RenderQueueEntry[];
  accounts: AccountSummary[];
  accountError: string | null;
  accountsBusy: boolean;
  timeZone: string;
  onRefreshAccounts: () => Promise<void>;
  onSelectPost: (postId: string) => void;
}) {
  const [actionError, setActionError] = useState<string | null>(null);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [rescheduleAt, setRescheduleAt] = useState("");
  const [editTitle, setEditTitle] = useState("");
  const [editDescription, setEditDescription] = useState("");
  const [editTagsInput, setEditTagsInput] = useState("");
  const [editThumbnailPath, setEditThumbnailPath] = useState("");
  const [editPrivacy, setEditPrivacy] = useState<UploadVisibility>("private");
  const [editTikTokInteractions, setEditTikTokInteractions] = useState(
    defaultTikTokInteractions,
  );
  const actions = post ? deriveSchedulerPostActions(post) : null;
  const postTagsKey = post?.tags.join("\u0000") ?? "";
  const editFieldConfig = schedulerMetadataFieldConfig(post?.provider);

  useEffect(() => {
    if (!post) {
      setRescheduleAt("");
      setActionError(null);
      return;
    }
    setRescheduleAt(toDateTimeLocal(post.scheduledAt));
    setEditTitle(post.title);
    setEditDescription(post.description);
    setEditTagsInput(post.tags.join(", "));
    setEditThumbnailPath(post.thumbnailPath ?? "");
    setEditPrivacy(post.visibility);
    setEditTikTokInteractions(post.tiktokInteractions ?? defaultTikTokInteractions());
    setActionError(null);
  }, [
    post?.description,
    post?.id,
    post?.scheduledAt,
    postTagsKey,
    post?.thumbnailPath,
    post?.tiktokInteractions,
    post?.title,
    post?.visibility,
  ]);

  const refresh = useCallback(async () => {
    if (!post || !entry) return;
    setBusyAction("refresh");
    try {
      await refreshServerUploadState(entry, post.provider);
      setActionError(null);
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusyAction(null);
    }
  }, [entry, post]);

  const runJobCommand = useCallback(
    async (command: string, args: Record<string, unknown>) => {
      if (!post?.jobId) return;
      setBusyAction(command);
      try {
        await invoke(command, args);
        if (entry) await refreshServerUploadState(entry, post.provider);
        setActionError(null);
      } catch (error) {
        setActionError(error instanceof Error ? error.message : String(error));
      } finally {
        setBusyAction(null);
      }
    },
    [entry, post],
  );

  const reschedule = useCallback(async () => {
    if (!post?.jobId) return;
    const scheduledFor = fromDateTimeLocal(rescheduleAt);
    if (!scheduledFor) {
      setActionError("Choose a valid reschedule time");
      return;
    }
    await runJobCommand("social_reschedule_job", {
      jobId: post.jobId,
      args: { scheduledFor },
    });
  }, [post?.jobId, rescheduleAt, runJobCommand]);

  const reconnect = useCallback(async () => {
    if (!post || !isSocialProvider(post.provider)) return;
    setBusyAction("social_reconnect_provider");
    try {
      await startConnect(invoke, openUrl, post.provider, "/schedule");
      await onRefreshAccounts();
      if (entry) await refreshServerUploadState(entry, post.provider);
      setActionError(null);
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusyAction(null);
    }
  }, [entry, onRefreshAccounts, post]);

  const regenerateMetadata = useCallback(() => {
    if (!post || !entry) return;
    const profile = buildSchedulerMetadataProfile({
      provider: post.provider,
      renderLabel: entry.label,
      scheduledFor: post.scheduledAt,
    });
    setEditTitle(profile.title);
    setEditDescription(profile.description);
    setEditTagsInput(profile.tagsInput);
    setEditThumbnailPath(profile.thumbnailPath);
    setEditPrivacy(profile.privacy);
    setEditTikTokInteractions(defaultTikTokInteractions());
    setActionError(null);
  }, [entry, post]);

  const saveMetadata = useCallback(async () => {
    if (!post || !entry) return;
    setBusyAction("save_metadata");
    try {
      const store = useRenderQueueStore.getState();
      const latest = store.entries.find((item) => item.id === entry.id) ?? entry;
      const metadata = buildSchedulerMetadata({
        title: editTitle || latest.label,
        description: editDescription,
        tagsInput: editTagsInput,
        thumbnailPath: editThumbnailPath,
        privacy: editPrivacy,
        scheduledFor: post.scheduledAt,
        tiktokInteractions:
          post.provider === "tiktok" ? editTikTokInteractions : undefined,
      });
      if (post.targetId && isSocialProvider(post.provider)) {
        await updateSchedulerTargetMetadata({
          provider: post.provider,
          targetId: post.targetId,
          title: metadata.title,
          description: metadata.description,
          tagsInput: editTagsInput,
          thumbnailPath: editThumbnailPath,
          privacy: metadata.visibility,
          scheduledFor: post.scheduledAt,
          tiktokInteractions: metadata.tiktokInteractions,
          invoke,
        });
      }
      store.setUploadMetadata(
        latest.id,
        mergeSchedulerMetadataEdit(latest, post.provider, metadata),
      );
      setActionError(null);
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusyAction(null);
    }
  }, [
    editDescription,
    editPrivacy,
    editTagsInput,
    editThumbnailPath,
    editTikTokInteractions,
    editTitle,
    entry,
    post,
  ]);

  return (
    <aside className="min-h-0 border-l border-[var(--glass-border)] bg-[rgba(8,10,14,0.54)]">
      <div className="flex h-full min-h-0 flex-col">
        <div className="shrink-0 border-b border-[var(--glass-border)] px-4 py-3">
          <div className="flex items-center gap-2 text-[12px] font-semibold text-[var(--color-text-secondary)]">
            <FileVideo className="h-4 w-4 text-[#FF9A45]" />
            Review drawer
          </div>
        </div>
        <SchedulerComposer
          entries={entries}
          accounts={accounts}
          accountError={accountError}
          accountsBusy={accountsBusy}
          timeZone={timeZone}
          onRefreshAccounts={onRefreshAccounts}
          onSelectPost={onSelectPost}
        />
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
                Metadata
              </div>
              <div className="mt-3 grid gap-2">
                {editFieldConfig.showTitle ? (
                  <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                    {editFieldConfig.titleLabel}
                    <input
                      value={editTitle}
                      onChange={(event) => setEditTitle(event.currentTarget.value)}
                      className="min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                    />
                  </label>
                ) : null}
                {editFieldConfig.showDescription ? (
                  <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                    {editFieldConfig.descriptionLabel}
                    <textarea
                      value={editDescription}
                      onChange={(event) =>
                        setEditDescription(event.currentTarget.value)
                      }
                      placeholder={editFieldConfig.descriptionPlaceholder}
                      className="min-h-[66px] min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                    />
                  </label>
                ) : null}
                <div className="grid grid-cols-2 gap-2">
                  {editFieldConfig.showTags ? (
                    <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                      Tags
                      <input
                        value={editTagsInput}
                        onChange={(event) =>
                          setEditTagsInput(event.currentTarget.value)
                        }
                        className="min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                      />
                    </label>
                  ) : null}
                  {editFieldConfig.visibilityOptions ? (
                    <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                      Privacy
                      <select
                        value={editPrivacy}
                        onChange={(event) =>
                          setEditPrivacy(event.currentTarget.value as UploadVisibility)
                        }
                        className="min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                      >
                        {editFieldConfig.visibilityOptions.map((option) => (
                          <option key={option.value} value={option.value}>
                            {option.label}
                          </option>
                        ))}
                      </select>
                    </label>
                  ) : (
                    <div className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                      Visibility
                      <span className="rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)] px-2 py-1.5 text-[12px] text-[var(--color-text-secondary)]">
                        Always public on {providerLabel(post.provider)}
                      </span>
                    </div>
                  )}
                </div>
                {editFieldConfig.showThumbnail ? (
                  <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                    Thumbnail path
                    <input
                      value={editThumbnailPath}
                      onChange={(event) =>
                        setEditThumbnailPath(event.currentTarget.value)
                      }
                      className="min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                    />
                  </label>
                ) : null}
                {post.provider === "tiktok" ? (
                  <TikTokInteractionControls
                    value={editTikTokInteractions}
                    onChange={setEditTikTokInteractions}
                  />
                ) : null}
                <div className="flex flex-wrap gap-2">
                  <SchedulerActionButton
                    icon={<RefreshCw className="h-3.5 w-3.5" />}
                    label="Regenerate"
                    busy={false}
                    onClick={regenerateMetadata}
                  />
                  <SchedulerActionButton
                    icon={<ListTodo className="h-3.5 w-3.5" />}
                    label="Save metadata"
                    busy={busyAction === "save_metadata"}
                    onClick={() => void saveMetadata()}
                  />
                </div>
              </div>
            </section>

            {actions ? (
              <section className="mt-4 rounded-lg border border-[var(--glass-border)] bg-[rgba(255,255,255,0.03)] p-3">
                <div className="flex items-center justify-between gap-2">
                  <div className="text-[12px] font-semibold text-[var(--color-text-secondary)]">
                    Job controls
                  </div>
                  {post.jobId ? (
                    <span className="truncate font-mono text-[11px] text-[var(--color-text-muted)]">
                      {post.jobId}
                    </span>
                  ) : null}
                </div>
                <div className="mt-3 flex flex-wrap gap-2">
                  {actions.canRefresh ? (
                    <SchedulerActionButton
                      icon={<RefreshCw className="h-3.5 w-3.5" />}
                      label="Refresh"
                      busy={busyAction === "refresh"}
                      onClick={() => void refresh()}
                    />
                  ) : null}
                  {actions.canCancel ? (
                    <SchedulerActionButton
                      icon={<Ban className="h-3.5 w-3.5" />}
                      label="Cancel"
                      busy={busyAction === "social_cancel_job"}
                      onClick={() =>
                        void runJobCommand("social_cancel_job", {
                          jobId: post.jobId,
                          now: nowSeconds(),
                        })
                      }
                    />
                  ) : null}
                  {actions.canRetry ? (
                    <SchedulerActionButton
                      icon={<RotateCcw className="h-3.5 w-3.5" />}
                      label="Retry"
                      busy={busyAction === "social_retry_job"}
                      onClick={() =>
                        void runJobCommand("social_retry_job", {
                          jobId: post.jobId,
                          now: nowSeconds(),
                        })
                      }
                    />
                  ) : null}
                  {actions.canReconnect && isSocialProvider(post.provider) ? (
                    <SchedulerActionButton
                      icon={<KeyRound className="h-3.5 w-3.5" />}
                      label="Reconnect"
                      busy={busyAction === "social_reconnect_provider"}
                      onClick={() => void reconnect()}
                    />
                  ) : null}
                  {actions.canOpenProviderUrl && post.providerUrl ? (
                    <SchedulerActionButton
                      icon={<ExternalLink className="h-3.5 w-3.5" />}
                      label="Open"
                      busy={false}
                      onClick={() => void openUrl(post.providerUrl as string)}
                    />
                  ) : null}
                </div>
                {actions.canReschedule ? (
                  <div className="mt-3 grid grid-cols-[minmax(0,1fr)_auto] gap-2">
                    <input
                      type="datetime-local"
                      value={rescheduleAt}
                      onChange={(event) => setRescheduleAt(event.currentTarget.value)}
                      className="min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                      aria-label={`Reschedule ${post.jobId}`}
                    />
                    <SchedulerActionButton
                      icon={<Clock3 className="h-3.5 w-3.5" />}
                      label="Reschedule"
                      busy={busyAction === "social_reschedule_job"}
                      onClick={() => void reschedule()}
                    />
                  </div>
                ) : null}
                {actionError ? (
                  <p className="mt-2 text-[12px] text-[var(--color-job-failed-text)]">
                    {actionError}
                  </p>
                ) : null}
              </section>
            ) : null}

            <section className="mt-4 rounded-lg border border-[var(--glass-border)] bg-[rgba(255,255,255,0.03)] p-3">
              <div className="flex items-center gap-2 text-[12px] font-semibold text-[var(--color-text-secondary)]">
                <ListTodo className="h-3.5 w-3.5" />
                Audit history
              </div>
              <AuditEventList post={post} timeZone={timeZone} />
            </section>

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
                scheduler cards.
              </p>
            </div>
          </div>
        )}
      </div>
    </aside>
  );
}

function SchedulerComposer({
  entries,
  accounts,
  accountError,
  accountsBusy,
  timeZone,
  onRefreshAccounts,
  onSelectPost,
}: {
  entries: RenderQueueEntry[];
  accounts: AccountSummary[];
  accountError: string | null;
  accountsBusy: boolean;
  timeZone: string;
  onRefreshAccounts: () => Promise<void>;
  onSelectPost: (postId: string) => void;
}) {
  const publishable = useMemo(() => schedulerPublishableEntries(entries), [entries]);
  const uploadAccounts = useMemo(
    () => accounts.filter((account) => account.capabilities.uploadVideo),
    [accounts],
  );
  const [entryId, setEntryId] = useState("");
  const [accountIds, setAccountIds] = useState<string[]>([]);
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [tagsInput, setTagsInput] = useState("");
  const [thumbnailPath, setThumbnailPath] = useState("");
  const [privacy, setPrivacy] = useState<UploadVisibility>("private");
  const [tiktokInteractions, setTikTokInteractions] = useState(
    defaultTikTokInteractions,
  );
  const [scheduledAt, setScheduledAt] = useState(toDateTimeLocal(nowSeconds() + 3600));
  const [cadenceMinutes, setCadenceMinutes] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!entryId && publishable[0]) setEntryId(publishable[0].id);
  }, [entryId, publishable]);

  useEffect(() => {
    setAccountIds((current) => {
      const available = new Set(uploadAccounts.map((account) => account.id));
      const kept = current.filter((id) => available.has(id));
      if (kept.length > 0) return kept;
      return uploadAccounts[0] ? [uploadAccounts[0].id] : [];
    });
  }, [uploadAccounts]);

  const selectedEntry = publishable.find((item) => item.id === entryId);
  const selectedAccounts = useMemo(
    () => uploadAccounts.filter((account) => accountIds.includes(account.id)),
    [accountIds, uploadAccounts],
  );
  const selectedProvider = schedulerMetadataControlProvider(selectedAccounts);
  const hasTikTokSelection = selectedAccounts.some(
    (account) => account.provider === "tiktok",
  );
  const fieldConfig = schedulerMetadataFieldConfig(selectedProvider);
  const scheduledForPreview = fromDateTimeLocal(scheduledAt);
  const cadenceSlots = scheduledForPreview
    ? buildSchedulerCadenceSlots(
        selectedAccounts,
        scheduledForPreview,
        cadenceMinutes,
      )
    : [];
  const metadataValidationErrors = useMemo(
    () =>
      scheduledForPreview
        ? validateSchedulerMetadataForAccounts(selectedAccounts, {
            title,
            description,
            tagsInput,
            thumbnailPath,
            privacy,
            scheduledFor: scheduledForPreview,
          })
        : [],
    [
      description,
      privacy,
      scheduledForPreview,
      selectedAccounts,
      tagsInput,
      thumbnailPath,
      title,
    ],
  );

  useEffect(() => {
    if (!selectedEntry || !selectedProvider) return;
    const metadata = selectedEntry.uploadMetadata?.[selectedProvider];
    setTitle(metadata?.title ?? selectedEntry.label);
    setDescription(metadata?.description ?? "");
    setTagsInput(metadata?.tags.join(", ") ?? "");
    setThumbnailPath(metadata?.thumbnailPath ?? "");
    setPrivacy(metadata?.visibility ?? "private");
    setTikTokInteractions(metadata?.tiktokInteractions ?? defaultTikTokInteractions());
    setScheduledAt(toDateTimeLocal(metadata?.scheduledAt ?? nowSeconds() + 3600));
  }, [selectedEntry?.id, selectedProvider]);

  const regenerateProfile = useCallback(() => {
    if (!selectedEntry) return;
    const scheduledFor = fromDateTimeLocal(scheduledAt) ?? nowSeconds() + 3600;
    const profile = buildSchedulerMetadataProfile({
      provider: selectedProvider,
      renderLabel: selectedEntry.label,
      scheduledFor,
    });
    setTitle(profile.title);
    setDescription(profile.description);
    setTagsInput(profile.tagsInput);
    setThumbnailPath(profile.thumbnailPath);
    setPrivacy(profile.privacy);
    setTikTokInteractions(defaultTikTokInteractions());
    setScheduledAt(toDateTimeLocal(profile.scheduledFor));
    setError(null);
  }, [scheduledAt, selectedEntry, selectedProvider]);

  const submit = useCallback(async () => {
    if (!selectedEntry || selectedAccounts.length === 0) return;
    const scheduledFor = fromDateTimeLocal(scheduledAt);
    if (!scheduledFor) {
      setError("Choose a valid schedule time");
      return;
    }
    if (metadataValidationErrors.length > 0) {
      setError(metadataValidationErrors[0].message);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const results = await publishSchedulerPostToAccounts({
        entry: selectedEntry,
        accounts: selectedAccounts as SchedulerPublishAccount[],
        title,
        description,
        tagsInput,
        thumbnailPath,
        privacy,
        scheduledFor,
        cadenceMinutes,
        tiktokInteractions,
        invoke,
      });
      const store = useRenderQueueStore.getState();
      const latest = store.entries.find((item) => item.id === selectedEntry.id) ?? selectedEntry;
      let merged: RenderQueueEntry = latest;
      for (const result of results) {
        const patch = mergeSchedulerPublishResult(merged, result);
        merged = {
          ...merged,
          uploadTargets: patch.uploadTargets,
          uploadMetadata: patch.uploadMetadata,
          uploadStates: patch.uploadStates,
          publishedUrls: patch.publishedUrls,
        };
      }
      const patch = {
        uploadTargets: merged.uploadTargets ?? [],
        uploadMetadata: merged.uploadMetadata ?? {},
        uploadStates: merged.uploadStates ?? {},
        publishedUrls: merged.publishedUrls ?? {},
      };
      store.setUploadTargets(selectedEntry.id, patch.uploadTargets);
      store.setUploadMetadata(selectedEntry.id, patch.uploadMetadata);
      store.setUploadStates(selectedEntry.id, patch.uploadStates, patch.publishedUrls);
      const firstResult = results[0];
      if (firstResult) onSelectPost(`${selectedEntry.id}:${firstResult.provider}`);
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }, [
    selectedEntry,
    selectedAccounts,
    scheduledAt,
    cadenceMinutes,
    title,
    description,
    tagsInput,
    thumbnailPath,
    privacy,
    tiktokInteractions,
    onSelectPost,
    metadataValidationErrors,
  ]);

  return (
    <section className="shrink-0 border-b border-[var(--glass-border)] px-4 py-4">
      <div className="flex items-center justify-between gap-2">
        <div className="text-[12px] font-semibold text-[var(--color-text-secondary)]">
          Schedule from render
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <button
            type="button"
            disabled={!selectedEntry}
            onClick={regenerateProfile}
            className="inline-flex min-h-[28px] items-center gap-1.5 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.035)] px-2 py-1 text-[11px] font-semibold text-[var(--color-text-secondary)] hover:border-[var(--glass-border-strong)] hover:text-[var(--color-text-primary)] disabled:cursor-not-allowed disabled:opacity-60"
          >
            <RefreshCw className="h-3.5 w-3.5" />
            Regenerate
          </button>
          <button
            type="button"
            disabled={accountsBusy}
            onClick={() => void onRefreshAccounts()}
            className="inline-flex min-h-[28px] items-center gap-1.5 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.035)] px-2 py-1 text-[11px] font-semibold text-[var(--color-text-secondary)] hover:border-[var(--glass-border-strong)] hover:text-[var(--color-text-primary)] disabled:cursor-wait disabled:opacity-60"
          >
            <RefreshCw className="h-3.5 w-3.5" />
            {accountsBusy ? "Refreshing" : "Accounts"}
          </button>
        </div>
      </div>
      <div className="mt-3 grid gap-2">
        {accountError ? (
          <p className="m-0 text-[12px] text-[var(--color-job-failed-text)]">
            {accountError}
          </p>
        ) : null}
        {publishable.length === 0 ? (
          <p className="m-0 text-[12px] leading-relaxed text-[var(--color-text-muted)]">
            Finished renders appear here after Delivery export.
          </p>
        ) : uploadAccounts.length === 0 ? (
          <p className="m-0 text-[12px] leading-relaxed text-[var(--color-text-muted)]">
            Connect an upload-capable social account first.
          </p>
        ) : (
          <>
            <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
              Render
              <select
                value={entryId}
                onChange={(event) => setEntryId(event.currentTarget.value)}
                className="min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
              >
                {publishable.map((item) => (
                  <option key={item.id} value={item.id}>
                    {item.outputPath?.split("/").pop() ?? item.label}
                  </option>
                ))}
              </select>
            </label>
            <div className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
              Destinations
              <div className="grid gap-1">
                {uploadAccounts.map((account) => {
                  const checked = accountIds.includes(account.id);
                  return (
                    <label
                      key={account.id}
                      className="flex min-h-[30px] items-center gap-2 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.035)] px-2 py-1.5 text-[12px] text-[var(--color-text-secondary)]"
                    >
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={(event) => {
                          const nextChecked = event.currentTarget.checked;
                          setAccountIds((current) =>
                            nextChecked
                              ? [...new Set([...current, account.id])]
                              : current.filter((id) => id !== account.id),
                          );
                        }}
                      />
                      <span className="min-w-0 truncate">
                        {account.displayName} ({providerLabel(account.provider)})
                      </span>
                    </label>
                  );
                })}
              </div>
            </div>
            {fieldConfig.showTitle ? (
              <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                {fieldConfig.titleLabel}
                <input
                  value={title}
                  onChange={(event) => setTitle(event.currentTarget.value)}
                  className="min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                />
              </label>
            ) : null}
            {fieldConfig.showDescription ? (
              <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                {fieldConfig.descriptionLabel}
                <textarea
                  value={description}
                  onChange={(event) => setDescription(event.currentTarget.value)}
                  placeholder={fieldConfig.descriptionPlaceholder}
                  className="min-h-[66px] min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                />
              </label>
            ) : null}
            <div className="grid grid-cols-2 gap-2">
              {fieldConfig.showTags ? (
                <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                  Tags
                  <input
                    value={tagsInput}
                    onChange={(event) => setTagsInput(event.currentTarget.value)}
                    className="min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                  />
                </label>
              ) : null}
              {fieldConfig.visibilityOptions ? (
                <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                  Privacy
                  <select
                    value={privacy}
                    onChange={(event) =>
                      setPrivacy(event.currentTarget.value as UploadVisibility)
                    }
                    className="min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                  >
                    {fieldConfig.visibilityOptions.map((option) => (
                      <option key={option.value} value={option.value}>
                        {option.label}
                      </option>
                    ))}
                  </select>
                </label>
              ) : (
                <div className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                  Visibility
                  <span className="rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)] px-2 py-1.5 text-[12px] text-[var(--color-text-secondary)]">
                    Always public on {providerLabel(selectedProvider ?? "")}
                  </span>
                </div>
              )}
            </div>
            {fieldConfig.showThumbnail ? (
              <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                Thumbnail path
                <input
                  value={thumbnailPath}
                  onChange={(event) => setThumbnailPath(event.currentTarget.value)}
                  className="min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                />
              </label>
            ) : null}
            {hasTikTokSelection ? (
              <TikTokInteractionControls
                value={tiktokInteractions}
                onChange={setTikTokInteractions}
              />
            ) : null}
            {metadataValidationErrors.length > 0 ? (
              <ul className="m-0 grid list-none gap-1 rounded-md border border-[var(--color-job-failed-border)] bg-[var(--color-job-failed-fill)] p-2 text-[12px] text-[var(--color-job-failed-text)]">
                {metadataValidationErrors.map((validationError) => (
                  <li key={`${validationError.provider}:${validationError.code}`}>
                    {providerLabel(validationError.provider)}:{" "}
                    {validationError.message}
                  </li>
                ))}
              </ul>
            ) : null}
            <div className="grid grid-cols-[minmax(0,1fr)_auto_auto] gap-2">
              <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                When
                <input
                  type="datetime-local"
                  value={scheduledAt}
                  onChange={(event) => setScheduledAt(event.currentTarget.value)}
                  className="min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                />
              </label>
              <label className="grid gap-1 text-[11px] text-[var(--color-text-muted)]">
                Cadence
                <input
                  type="number"
                  min={0}
                  step={5}
                  value={cadenceMinutes}
                  onChange={(event) =>
                    setCadenceMinutes(
                      Math.max(0, Number(event.currentTarget.value) || 0),
                    )
                  }
                  className="w-20 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.04)] px-2 py-1.5 text-[12px] text-[var(--color-text-primary)]"
                  aria-label="Cadence minutes between scheduled posts"
                />
              </label>
              <button
                type="button"
                disabled={
                  busy ||
                  selectedAccounts.length === 0 ||
                  metadataValidationErrors.length > 0
                }
                onClick={() => void submit()}
                className="self-end rounded-md border border-[rgba(255,122,24,0.42)] bg-[rgba(255,122,24,0.14)] px-3 py-1.5 text-[12px] font-semibold text-[#FFB073] hover:bg-[rgba(255,122,24,0.2)] disabled:cursor-wait disabled:opacity-60"
              >
                {busy ? "Scheduling" : `Schedule ${selectedAccounts.length}`}
              </button>
            </div>
            {cadenceSlots.length > 0 ? (
              <div className="rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)] p-2">
                <div className="text-[11px] font-semibold uppercase text-[var(--color-text-muted)]">
                  Slot preview
                </div>
                <ol className="mt-2 m-0 grid list-none gap-1 p-0">
                  {cadenceSlots.map((slot) => (
                    <li
                      key={slot.account.id}
                      className="flex min-w-0 items-center justify-between gap-2 text-[12px]"
                    >
                      <span className="min-w-0 truncate text-[var(--color-text-secondary)]">
                        {slot.account.displayName} ({providerLabel(slot.provider)})
                      </span>
                      <span className="shrink-0 text-[var(--color-text-muted)]">
                        {formatSchedulerTime(slot.scheduledFor, timeZone)}
                      </span>
                    </li>
                  ))}
                </ol>
              </div>
            ) : null}
          </>
        )}
        {error ? (
          <p className="m-0 text-[12px] text-[var(--color-job-failed-text)]">
            {error}
          </p>
        ) : null}
      </div>
    </section>
  );
}

function AuditEventList({
  post,
  timeZone,
}: {
  post: SchedulerPost;
  timeZone: string;
}) {
  if (post.auditEvents.length === 0) {
    return (
      <p className="mt-2 m-0 text-[12px] leading-relaxed text-[var(--color-text-muted)]">
        No audit events recorded yet.
      </p>
    );
  }

  return (
    <ol className="mt-3 m-0 grid list-none gap-2 p-0">
      {post.auditEvents.map((event) => (
        <li
          key={event.id}
          className="min-w-0 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)] px-2.5 py-2"
        >
          <div className="flex min-w-0 items-center justify-between gap-2">
            <span className="truncate text-[12px] font-semibold text-[var(--color-text-primary)]">
              {event.eventType}
            </span>
            <time className="shrink-0 text-[11px] text-[var(--color-text-muted)]">
              {formatSchedulerTime(event.createdAt, timeZone)}
            </time>
          </div>
          <p className="mt-1 m-0 break-words text-[12px] leading-relaxed text-[var(--color-text-secondary)]">
            {event.message || "No message"}
          </p>
        </li>
      ))}
    </ol>
  );
}

function SchedulerActionButton({
  icon,
  label,
  busy,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  busy: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      disabled={busy}
      onClick={onClick}
      className="inline-flex min-h-[30px] items-center gap-1.5 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.035)] px-2.5 py-1.5 text-[12px] font-semibold text-[var(--color-text-secondary)] hover:border-[var(--glass-border-strong)] hover:text-[var(--color-text-primary)] disabled:cursor-wait disabled:opacity-60"
    >
      {icon}
      {busy ? "Working" : label}
    </button>
  );
}

function TikTokInteractionControls({
  value,
  onChange,
}: {
  value: TikTokInteractionSettings;
  onChange: (value: TikTokInteractionSettings) => void;
}) {
  const options: Array<{
    key: keyof TikTokInteractionSettings;
    label: string;
  }> = [
    { key: "disableDuet", label: "Disable duet" },
    { key: "disableComment", label: "Disable comments" },
    { key: "disableStitch", label: "Disable stitch" },
  ];
  return (
    <fieldset className="grid gap-2 rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)] p-2">
      <legend className="px-1 text-[11px] text-[var(--color-text-muted)]">
        TikTok interactions
      </legend>
      <div className="grid gap-2 sm:grid-cols-3">
        {options.map((option) => (
          <label
            key={option.key}
            className="flex min-w-0 items-center gap-2 text-[12px] text-[var(--color-text-secondary)]"
          >
            <input
              type="checkbox"
              checked={value[option.key]}
              onChange={(event) =>
                onChange({ ...value, [option.key]: event.currentTarget.checked })
              }
              className="h-3.5 w-3.5 accent-[#FF7A18]"
            />
            <span className="min-w-0 truncate">{option.label}</span>
          </label>
        ))}
      </div>
    </fieldset>
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
        view follows render targets once they enter the publishing pipeline.
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

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

function toDateTimeLocal(secs: number): string {
  const date = new Date(secs * 1000);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function fromDateTimeLocal(value: string): number | null {
  const millis = new Date(value).getTime();
  if (!Number.isFinite(millis)) return null;
  return Math.floor(millis / 1000);
}

function providerLabel(provider: string): string {
  return provider
    .split(/[-_\s]+/)
    .filter(Boolean)
    .map((part) => part[0]?.toUpperCase() + part.slice(1))
    .join(" ");
}

function isSocialProvider(provider: string): provider is Provider {
  return SOCIAL_PROVIDERS.includes(provider as Provider);
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
