import type {
  RenderQueueEntry,
  RenderUploadState,
} from "../renderQueue.ts";
import {
  buildPlatformFieldsForPublish,
  reasonCopy,
  type AccountSummary,
  type Provider,
} from "../social/socialModel.ts";
import type {
  UploadMetadata,
  UploadVisibility,
} from "../../state/uploadMetadata.ts";

type InvokeFn = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export type SchedulerPublishAccount = {
  id: string;
  provider: Provider;
  capabilities: { uploadVideo: boolean };
};

type ValidatedTarget = {
  validation_state?: string;
  validationState?: string;
  validation_reasons?: string[];
  validationReasons?: string[];
};

type PublishJob = {
  id: string;
  status: string;
};

export type SchedulerPublishInput = {
  entry: RenderQueueEntry;
  account: SchedulerPublishAccount;
  title: string;
  description: string;
  tagsInput: string;
  thumbnailPath: string;
  privacy: UploadVisibility;
  scheduledFor: number;
  invoke: InvokeFn;
  idFactory?: (prefix: string) => string;
  nowSeconds?: () => number;
  createdBy?: string;
  campaignIdPrefix?: string;
};

export type SchedulerMultiPublishInput = Omit<
  SchedulerPublishInput,
  "account"
> & {
  accounts: SchedulerPublishAccount[];
};

export type SchedulerPublishResult = {
  provider: Provider;
  jobId: string;
  uploadState: RenderUploadState;
  metadata: UploadMetadata;
};

export type SchedulerPublishQueuePatch = {
  uploadTargets: string[];
  uploadStates: Record<string, RenderUploadState>;
  publishedUrls: Record<string, string>;
  uploadMetadata: Record<string, UploadMetadata>;
};

export type SchedulerAccountsResult = {
  accounts: AccountSummary[];
  error: string | null;
};

export async function loadSchedulerAccounts(
  invoke: InvokeFn,
): Promise<SchedulerAccountsResult> {
  try {
    return {
      accounts: await invoke<AccountSummary[]>("social_accounts"),
      error: null,
    };
  } catch (error) {
    return {
      accounts: [],
      error: error instanceof Error ? error.message : String(error),
    };
  }
}

export function schedulerPublishableEntries(
  entries: RenderQueueEntry[],
): RenderQueueEntry[] {
  return entries.filter((entry) => entry.status === "done" && Boolean(entry.outputPath));
}

export function mergeSchedulerPublishResult(
  entry: RenderQueueEntry,
  result: SchedulerPublishResult,
): SchedulerPublishQueuePatch {
  const providers = new Set(entry.uploadTargets ?? []);
  providers.add(result.provider);
  return {
    uploadTargets: [...providers],
    uploadStates: {
      ...(entry.uploadStates ?? {}),
      [result.provider]: result.uploadState,
    },
    publishedUrls: entry.publishedUrls ?? {},
    uploadMetadata: {
      ...(entry.uploadMetadata ?? {}),
      [result.provider]: result.metadata,
    },
  };
}

export function mergeSchedulerPublishResults(
  entry: RenderQueueEntry,
  results: SchedulerPublishResult[],
): SchedulerPublishQueuePatch {
  let merged: RenderQueueEntry = entry;
  for (const result of results) {
    const patch = mergeSchedulerPublishResult(merged, result);
    merged = {
      ...merged,
      uploadTargets: patch.uploadTargets,
      uploadStates: patch.uploadStates,
      publishedUrls: patch.publishedUrls,
      uploadMetadata: patch.uploadMetadata,
    };
  }
  return {
    uploadTargets: merged.uploadTargets ?? [],
    uploadStates: merged.uploadStates ?? {},
    publishedUrls: merged.publishedUrls ?? {},
    uploadMetadata: merged.uploadMetadata ?? {},
  };
}

function randomId(prefix: string): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${prefix}-${hex}`;
}

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

function validationState(target: ValidatedTarget): string {
  return target.validation_state ?? target.validationState ?? "unknown";
}

function validationFailureReason(target: ValidatedTarget): string {
  const reasons = target.validation_reasons ?? target.validationReasons ?? [];
  const first = reasons[0];
  return first ? reasonCopy(first) : reasonCopy(validationState(target));
}

function tagsFromInput(value: string): string[] {
  const tags: string[] = [];
  for (const raw of value.split(",")) {
    const tag = raw.trim();
    if (tag && !tags.includes(tag)) tags.push(tag);
  }
  return tags;
}

export async function publishSchedulerPostViaServer({
  entry,
  account,
  title,
  description,
  tagsInput,
  thumbnailPath,
  privacy,
  scheduledFor,
  invoke,
  idFactory = randomId,
  nowSeconds: now = nowSeconds,
  createdBy = "desktop-scheduler",
  campaignIdPrefix = "scheduler",
}: SchedulerPublishInput): Promise<SchedulerPublishResult> {
  if (!entry.outputPath) {
    throw new Error("Selected render has no output file");
  }
  if (!account.capabilities.uploadVideo) {
    throw new Error(`Selected ${account.provider} account cannot upload video`);
  }

  const provider = account.provider;
  const targetId = idFactory("target");
  const jobId = idFactory("job");
  const currentNow = now();
  await invoke("social_bind_target", {
    args: {
      targetId,
      campaignId: `${campaignIdPrefix}-${entry.id}`,
      variantId: `${provider}-${entry.id}-${jobId}`,
      connectedAccountId: account.id,
      platformFields: buildPlatformFieldsForPublish({
        provider,
        privacy,
        title: title || entry.label,
        description,
        tagsInput,
        thumbnailPath,
      }),
      scheduledFor,
      now: currentNow,
    },
  });

  const validated = await invoke<ValidatedTarget>("social_validate_target", {
    targetId,
    now: currentNow,
  });
  if (validationState(validated) !== "valid") {
    throw new Error(`Not valid: ${validationFailureReason(validated)}`);
  }

  const job = await invoke<PublishJob>("social_schedule_target", {
    args: {
      targetId,
      jobId,
      artifactRef: "",
      createdBy,
      now: currentNow,
    },
  });
  await invoke("social_upload_artifact", {
    jobId: job.id,
    filePath: entry.outputPath,
  });

  return {
    provider,
    jobId: job.id,
    uploadState: { state: "scheduled", job_id: job.id },
    metadata: {
      title: title || entry.label,
      description,
      tags: tagsFromInput(tagsInput),
      visibility: privacy,
      scheduledAt: scheduledFor,
      thumbnailPath: thumbnailPath.trim() || undefined,
    },
  };
}

export async function publishSchedulerPostToAccounts({
  accounts,
  ...input
}: SchedulerMultiPublishInput): Promise<SchedulerPublishResult[]> {
  const results: SchedulerPublishResult[] = [];
  for (const account of accounts) {
    results.push(await publishSchedulerPostViaServer({ ...input, account }));
  }
  return results;
}
