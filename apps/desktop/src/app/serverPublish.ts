import type { RenderUploadEvent, RenderUploadState } from "./renderQueue";
import type { UploadMetadata } from "../state/uploadMetadata";
import {
  buildPlatformFieldsForPublish,
  reasonCopy,
  type Provider,
} from "./social/socialModel.ts";

type InvokeFn = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

type AccountSummaryLite = {
  id: string;
  provider: string;
  capabilities: { uploadVideo: boolean };
};

type PublishJob = {
  id: string;
  status: string;
  scheduledFor?: number;
  providerPostId?: string | null;
  providerPostUrl?: string | null;
  normalizedError?: string | null;
  requiresActionReason?: string | null;
  events?: RenderUploadEvent[];
};

const JOB_POLL_INTERVAL_MS = 2_000;
const JOB_POLL_TIMEOUT_MS = 180_000;
const IMMEDIATE_FIRE_WINDOW_SECS = 60;

type ValidatedTarget = {
  validation_state?: string;
  validationState?: string;
  validation_reasons?: string[];
  validationReasons?: string[];
};

export type ServerRenderPublishInput = {
  renderQueueId: string;
  renderJobId: string;
  outputPath: string;
  title: string;
  targets: string[];
  accountIdsByProvider?: Record<string, string>;
  metadataByProvider: Record<string, UploadMetadata>;
  invoke: InvokeFn;
  idFactory?: (prefix: string) => string;
  nowSeconds?: () => number;
  sleepMs?: (ms: number) => Promise<void>;
  onState?: (provider: string, state: RenderUploadState) => void;
};

export type ServerRenderPublishResult = {
  states: Record<string, RenderUploadState>;
  publishedUrls: Record<string, string>;
};

function randomId(prefix: string): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${prefix}-${hex}`;
}

function defaultNowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

function uploadCapableAccount(
  accounts: AccountSummaryLite[],
  provider: string,
  accountId?: string,
): AccountSummaryLite | undefined {
  if (accountId) {
    return accounts.find(
      (account) =>
        account.id === accountId &&
        account.provider === provider &&
        account.capabilities.uploadVideo,
    );
  }
  return accounts.find(
    (account) =>
      account.provider === provider && account.capabilities.uploadVideo,
  );
}

function failed(reason: string, jobId?: string): RenderUploadState {
  return jobId
    ? { state: "failed", reason, job_id: jobId }
    : { state: "failed", reason };
}

function stateFromServerJob(job: PublishJob): RenderUploadState {
  const events = job.events && job.events.length > 0 ? job.events : undefined;
  if (job.status === "published") {
    return {
      state: "published",
      remote_id: job.providerPostId ?? job.id,
      ...(job.providerPostUrl ? { remote_url: job.providerPostUrl } : {}),
      ...(events ? { events } : {}),
    };
  }
  if (job.status === "requires_action") {
    const reason =
      job.requiresActionReason ??
      job.normalizedError ??
      "server publish requires_action";
    return {
      state: "requires_action",
      reason: reasonCopy(reason),
      job_id: job.id,
      ...(events ? { events } : {}),
    };
  }
  if (job.status === "failed" || job.status === "cancelled") {
    const reason =
      job.normalizedError ??
      job.requiresActionReason ??
      `server publish ${job.status}`;
    return {
      ...failed(reasonCopy(reason), job.id),
      ...(events ? { events } : {}),
    };
  }
  if (job.status === "processing" || job.status === "uploading") {
    return {
      state: "processing",
      job_id: job.id,
      ...(events ? { events } : {}),
    };
  }
  return {
    state: "scheduled",
    job_id: job.id,
    ...(job.scheduledFor !== undefined ? { scheduled_for: job.scheduledFor } : {}),
    ...(events ? { events } : {}),
  };
}

function validationState(target: ValidatedTarget): string | undefined {
  return target.validation_state ?? target.validationState;
}

function validationFailureReason(target: ValidatedTarget): string {
  const reasons = target.validation_reasons ?? target.validationReasons ?? [];
  const first = reasons[0];
  return first ? reasonCopy(first) : (validationState(target) ?? "unknown");
}

function platformFieldsFromMetadata(
  provider: string,
  metadata: UploadMetadata | undefined,
  fallbackTitle: string,
): Record<string, unknown> {
  return buildPlatformFieldsForPublish({
    provider: provider as Provider,
    privacy: metadata?.visibility ?? "private",
    title: metadata?.title || fallbackTitle,
    description: metadata?.description ?? "",
    tagsInput: (metadata?.tags ?? []).join(","),
    thumbnailPath: metadata?.thumbnailPath ?? "",
    tiktokInteractions: metadata?.tiktokInteractions,
  });
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function isTerminalJob(job: PublishJob): boolean {
  return (
    job.status === "published" ||
    job.status === "failed" ||
    job.status === "requires_action" ||
    job.status === "cancelled"
  );
}

function isDue(scheduledFor: number | undefined, now: number): boolean {
  return scheduledFor === undefined || scheduledFor <= now;
}

function isNearImmediate(scheduledFor: number | undefined, now: number): boolean {
  return (
    scheduledFor === undefined ||
    scheduledFor <= now + IMMEDIATE_FIRE_WINDOW_SECS
  );
}

async function waitUntilDue(
  scheduledFor: number | undefined,
  nowSeconds: () => number,
  sleepMs: (ms: number) => Promise<void>,
): Promise<void> {
  if (scheduledFor === undefined || isDue(scheduledFor, nowSeconds())) {
    return;
  }
  await sleepMs(Math.max(0, (scheduledFor - nowSeconds()) * 1000));
}

async function pollServerPublishJob(
  invoke: InvokeFn,
  provider: string,
  job: PublishJob,
  nowSeconds: () => number,
  sleepMs: (ms: number) => Promise<void>,
  onState?: (provider: string, state: RenderUploadState) => void,
): Promise<PublishJob> {
  let current = job;
  const started = Date.now();
  while (!isTerminalJob(current) && Date.now() - started < JOB_POLL_TIMEOUT_MS) {
    await sleepMs(JOB_POLL_INTERVAL_MS);
    const command =
      current.status === "processing" || current.status === "uploading"
        ? "social_poll_publish_job"
        : isNearImmediate(current.scheduledFor, nowSeconds())
          ? "social_fire_due_job"
          : "social_publish_job";
    current = await invoke<PublishJob>(command, { jobId: current.id });
    onState?.(provider, stateFromServerJob(current));
  }
  return current;
}

async function fireScheduledJobWhenDue(
  invoke: InvokeFn,
  job: PublishJob,
  scheduledFor: number | undefined,
  nowSeconds: () => number,
  sleepMs: (ms: number) => Promise<void>,
): Promise<PublishJob> {
  await waitUntilDue(scheduledFor, nowSeconds, sleepMs);
  return invoke<PublishJob>("social_fire_due_job", { jobId: job.id });
}

export async function publishRenderTargetsViaServer({
  renderQueueId,
  renderJobId,
  outputPath,
  title,
  targets,
  accountIdsByProvider,
  metadataByProvider,
  invoke,
  idFactory = randomId,
  nowSeconds = defaultNowSeconds,
  sleepMs = sleep,
  onState,
}: ServerRenderPublishInput): Promise<ServerRenderPublishResult> {
  const states: Record<string, RenderUploadState> = {};
  const publishedUrls: Record<string, string> = {};
  const accounts = await invoke<AccountSummaryLite[]>("social_accounts");

  for (const provider of targets) {
    const selectedAccountId = accountIdsByProvider?.[provider];
    const account = uploadCapableAccount(accounts, provider, selectedAccountId);
    if (!account) {
      const reason = selectedAccountId
        ? `Selected ${provider} account is not connected or cannot upload video`
        : `No upload-capable ${provider} account connected`;
      states[provider] = failed(reason);
      onState?.(provider, states[provider]);
      continue;
    }

    try {
      states[provider] = { state: "uploading", progress: 0 };
      onState?.(provider, states[provider]);

      const metadata = metadataByProvider[provider];
      const targetId = idFactory("target");
      const jobId = idFactory("job");
      const now = nowSeconds();
      const scheduledFor = metadata?.scheduledAt ?? now + 1;
      await invoke("social_bind_target", {
        args: {
          targetId,
          campaignId: `render-${renderJobId}`,
          variantId: `${provider}-${renderQueueId}-${jobId}`,
          connectedAccountId: account.id,
          platformFields: platformFieldsFromMetadata(provider, metadata, title),
          scheduledFor,
          now,
        },
      });

      const validated = await invoke<ValidatedTarget>(
        "social_validate_target",
        { targetId, now },
      );
      const validatedState = validationState(validated);
      if (validatedState !== "valid") {
        states[provider] = failed(`Not valid: ${validationFailureReason(validated)}`);
        onState?.(provider, states[provider]);
        continue;
      }

      const scheduled = await invoke<PublishJob>("social_schedule_target", {
        args: {
          targetId,
          jobId,
          artifactRef: "",
          createdBy: "desktop-render-queue",
          now,
        },
      });
      const uploaded = await invoke<PublishJob>("social_upload_artifact", {
        jobId: scheduled.id,
        filePath: outputPath,
      });

      const fired =
        uploaded.status === "scheduled" &&
        isNearImmediate(uploaded.scheduledFor ?? scheduledFor, now)
          ? await fireScheduledJobWhenDue(
              invoke,
              uploaded,
              uploaded.scheduledFor ?? scheduledFor,
              nowSeconds,
              sleepMs,
            )
          : uploaded;

      states[provider] = stateFromServerJob(fired);
      onState?.(provider, states[provider]);
      if (
        fired.status === "scheduled" &&
        !isNearImmediate(fired.scheduledFor ?? scheduledFor, nowSeconds())
      ) {
        continue;
      }
      const finalJob = await pollServerPublishJob(
        invoke,
        provider,
        fired,
        nowSeconds,
        sleepMs,
        onState,
      );
      states[provider] = stateFromServerJob(finalJob);
      if (
        states[provider].state === "published" &&
        states[provider].remote_url
      ) {
        publishedUrls[provider] = states[provider].remote_url;
      }
      onState?.(provider, states[provider]);
    } catch (error) {
      states[provider] = failed(error instanceof Error ? error.message : String(error));
      onState?.(provider, states[provider]);
    }
  }

  return { states, publishedUrls };
}
