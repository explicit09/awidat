import type {
  RenderQueueEntry,
  RenderUploadState,
} from "../renderQueue.ts";
import type { UploadVisibility } from "../../state/uploadMetadata.ts";

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
  nowSeconds: number = Math.floor(Date.now() / 1000),
): SchedulerPost[] {
  return entries
    .flatMap((entry) => {
      const providers = uploadProviders(entry);
      return providers.map((provider) =>
        deriveSchedulerPost(entry, provider, nowSeconds),
      );
    })
    .sort((a, b) => {
      if (a.scheduledAt !== b.scheduledAt) {
        return a.scheduledAt - b.scheduledAt;
      }
      return a.title.localeCompare(b.title);
    });
}

export function formatSchedulerTime(
  epochSeconds: number,
  timeZone: string,
): string {
  const date = new Date(epochSeconds * 1000);
  const formatter = new Intl.DateTimeFormat("en-CA", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  });
  const parts = Object.fromEntries(
    formatter.formatToParts(date).map((part) => [part.type, part.value]),
  );
  return `${parts.year}-${parts.month}-${parts.day} ${parts.hour}:${parts.minute} ${timeZone}`;
}

function deriveSchedulerPost(
  entry: RenderQueueEntry,
  provider: string,
  nowSeconds: number,
): SchedulerPost {
  const metadata = entry.uploadMetadata?.[provider];
  const uploadState = entry.uploadStates?.[provider];
  const status = schedulerStatusFromEntry(entry, uploadState);
  const updatedAt = queueTimestampSeconds(entry.completedAt ?? entry.enqueuedAt);
  const scheduledAt =
    metadata?.scheduledAt ?? updatedAt ?? Math.floor(nowSeconds);
  const providerUrl =
    uploadState?.state === "published"
      ? uploadState.remote_url
      : entry.publishedUrls?.[provider];
  const failureReason =
    uploadState?.state === "failed" ? uploadState.reason : entry.error;

  return {
    id: `${entry.id}:${provider}`,
    renderQueueId: entry.id,
    renderJobId: entry.jobId,
    provider,
    title: metadata?.title || entry.label,
    description: metadata?.description ?? "",
    visibility: metadata?.visibility ?? "private",
    scheduledAt,
    status,
    jobId: uploadJobId(uploadState),
    outputPath: entry.outputPath,
    providerUrl,
    failureReason:
      status === "failed" || status === "requires_action"
        ? failureReason
        : undefined,
    updatedAt: updatedAt ?? Math.floor(nowSeconds),
  };
}

function uploadProviders(entry: RenderQueueEntry): string[] {
  if (entry.uploadTargets && entry.uploadTargets.length > 0) {
    return uniqueProviders(entry.uploadTargets);
  }
  return uniqueProviders(Object.keys(entry.uploadStates ?? {}));
}

function uniqueProviders(source: string[]): string[] {
  const providers: string[] = [];
  for (const provider of source) {
    if (!providers.includes(provider)) providers.push(provider);
  }
  return providers;
}

function schedulerStatusFromEntry(
  entry: RenderQueueEntry,
  uploadState: RenderUploadState | undefined,
): SchedulerStatus {
  if (uploadState) return schedulerStatusFromUploadState(uploadState);
  if (entry.status === "cancelled") return "cancelled";
  if (entry.status === "failed") return "failed";
  return "draft";
}

function schedulerStatusFromUploadState(
  uploadState: RenderUploadState,
): SchedulerStatus {
  switch (uploadState.state) {
    case "pending":
      return "draft";
    case "uploading":
      return "uploading";
    case "scheduled":
      return "scheduled";
    case "processing":
      return "processing";
    case "published":
      return "published";
    case "failed":
      return uploadState.reason === "server publish requires_action"
        ? "requires_action"
        : "failed";
  }
}

function uploadJobId(
  uploadState: RenderUploadState | undefined,
): string | undefined {
  if (
    uploadState?.state === "scheduled" ||
    uploadState?.state === "processing" ||
    uploadState?.state === "failed"
  ) {
    return uploadState.job_id;
  }
  return undefined;
}

function queueTimestampSeconds(timestamp: number | undefined): number | undefined {
  if (timestamp === undefined || !Number.isFinite(timestamp)) return undefined;
  return Math.floor(timestamp / 1000);
}
