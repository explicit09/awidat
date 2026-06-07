import type {
  RenderQueueEntry,
  RenderUploadEvent,
  RenderUploadState,
} from "../renderQueue.ts";
import {
  buildPlatformFieldsForPublish,
  reasonCopy,
  type AccountSummary,
  type Provider,
  type TikTokInteractionSettings,
} from "../social/socialModel.ts";
import type {
  UploadMetadata,
  UploadVisibility,
} from "../../state/uploadMetadata.ts";
import {
  validateMetadata,
  visibilityOptionsFor,
  type MetadataValidationError,
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
  scheduledFor?: number | null;
  events?: RenderUploadEvent[];
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
  tiktokInteractions?: Partial<TikTokInteractionSettings>;
  invoke: InvokeFn;
  idFactory?: (prefix: string) => string;
  nowSeconds?: () => number;
  createdBy?: string;
  campaignIdPrefix?: string;
};

export type SchedulerTargetMetadataUpdateInput = SchedulerMetadataInput & {
  provider: Provider;
  targetId: string;
  invoke: InvokeFn;
  nowSeconds?: () => number;
};

export type SchedulerMultiPublishInput = Omit<
  SchedulerPublishInput,
  "account"
> & {
  accounts: SchedulerPublishAccount[];
  cadenceMinutes?: number;
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

export type SchedulerMetadataInput = {
  title: string;
  description: string;
  tagsInput: string;
  thumbnailPath: string;
  privacy: UploadVisibility;
  scheduledFor: number;
  tiktokInteractions?: Partial<TikTokInteractionSettings>;
};

export type SchedulerCadenceAccount = {
  id: string;
  provider: string;
};

export type SchedulerCadenceSlot<T extends SchedulerCadenceAccount> = {
  account: T;
  provider: string;
  scheduledFor: number;
};

export type SchedulerMetadataValidationError = MetadataValidationError & {
  provider: string;
};

export type SchedulerMetadataFieldConfig = {
  showTitle: boolean;
  titleLabel: string;
  showDescription: boolean;
  descriptionLabel: string;
  descriptionPlaceholder: string;
  showTags: boolean;
  showThumbnail: boolean;
  visibilityOptions: ReturnType<typeof visibilityOptionsFor>;
};

export type SchedulerMetadataProfileInput = {
  provider: string | undefined;
  renderLabel: string;
  scheduledFor: number;
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

export function buildSchedulerMetadata({
  title,
  description,
  tagsInput,
  thumbnailPath,
  privacy,
  scheduledFor,
  tiktokInteractions,
}: SchedulerMetadataInput): UploadMetadata {
  const normalizedTikTokInteractions =
    normalizeTikTokInteractions(tiktokInteractions);
  return {
    title,
    description,
    tags: tagsFromInput(tagsInput),
    visibility: privacy,
    scheduledAt: scheduledFor,
    thumbnailPath: thumbnailPath.trim() || undefined,
    ...(normalizedTikTokInteractions
      ? { tiktokInteractions: normalizedTikTokInteractions }
      : {}),
  };
}

function normalizeTikTokInteractions(
  value?: Partial<TikTokInteractionSettings>,
): TikTokInteractionSettings | undefined {
  if (!value) return undefined;
  return {
    disableDuet: value.disableDuet ?? false,
    disableComment: value.disableComment ?? false,
    disableStitch: value.disableStitch ?? false,
  };
}

export function mergeSchedulerMetadataEdit(
  entry: RenderQueueEntry,
  provider: string,
  metadata: UploadMetadata,
): Record<string, UploadMetadata> {
  return {
    ...(entry.uploadMetadata ?? {}),
    [provider]: metadata,
  };
}

export function buildSchedulerCadenceSlots<T extends SchedulerCadenceAccount>(
  accounts: T[],
  startSeconds: number,
  cadenceMinutes: number,
): SchedulerCadenceSlot<T>[] {
  const intervalSeconds = Math.max(0, Math.floor(cadenceMinutes)) * 60;
  return accounts.map((account, index) => ({
    account,
    provider: account.provider,
    scheduledFor: startSeconds + index * intervalSeconds,
  }));
}

export function validateSchedulerMetadataForAccounts<
  T extends SchedulerCadenceAccount,
>(
  accounts: T[],
  input: SchedulerMetadataInput,
): SchedulerMetadataValidationError[] {
  const metadata = buildSchedulerMetadata(input);
  return accounts.flatMap((account) =>
    validateMetadata(account.provider, metadata)
      .filter((error) => error.field !== "schedule")
      .map((error) => ({
        provider: account.provider,
        ...error,
      })),
  );
}

export function schedulerMetadataFieldConfig(
  provider: string | undefined,
): SchedulerMetadataFieldConfig {
  const isInstagram = provider === "instagram";
  const isTwitterX = provider === "twitter_x";
  return {
    showTitle: !isInstagram,
    titleLabel: isTwitterX ? "Post text" : "Title",
    showDescription: !isTwitterX,
    descriptionLabel: isInstagram ? "Caption" : "Description",
    descriptionPlaceholder: isInstagram
      ? "Caption shown under the post"
      : "Long-form description",
    showTags: !isTwitterX,
    showThumbnail: !isTwitterX,
    visibilityOptions: provider ? visibilityOptionsFor(provider) : visibilityOptionsFor("youtube"),
  };
}

export function schedulerMetadataControlProvider<
  T extends SchedulerCadenceAccount,
>(accounts: T[]): string | undefined {
  return accounts.find((account) => account.provider !== "instagram")?.provider
    ?? accounts[0]?.provider;
}

export function buildSchedulerMetadataProfile({
  provider,
  renderLabel,
  scheduledFor,
}: SchedulerMetadataProfileInput): SchedulerMetadataInput {
  const title = provider === "instagram" ? "" : renderLabel;
  const description = provider === "instagram" ? renderLabel : "";
  return {
    title,
    description,
    tagsInput: "",
    thumbnailPath: "",
    privacy: "private",
    scheduledFor,
  };
}

export async function updateSchedulerTargetMetadata({
  provider,
  targetId,
  title,
  description,
  tagsInput,
  thumbnailPath,
  privacy,
  scheduledFor,
  tiktokInteractions,
  invoke,
  nowSeconds: now = nowSeconds,
}: SchedulerTargetMetadataUpdateInput): Promise<void> {
  const currentNow = now();
  await invoke("social_update_target", {
    args: {
      targetId,
      platformFields: buildPlatformFieldsForPublish({
        provider,
        privacy,
        title,
        description,
        tagsInput,
        thumbnailPath,
        tiktokInteractions,
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
}

function providerDisplayName(provider: string): string {
  const labels: Record<string, string> = {
    youtube: "YouTube",
    tiktok: "TikTok",
    instagram: "Instagram",
    twitter_x: "Twitter/X",
  };
  if (labels[provider]) return labels[provider];
  return provider
    .split("_")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function validationErrorCopy(error: SchedulerMetadataValidationError): string {
  const message =
    error.code === "title.required"
      ? "title required"
      : error.message.replace(/\.$/, "").toLowerCase();
  return `${providerDisplayName(error.provider)} ${message}`;
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

function scheduledUploadStateFromJob(
  job: PublishJob,
  targetId: string,
): RenderUploadState {
  const events = job.events && job.events.length > 0 ? job.events : undefined;
  return {
    state: "scheduled",
    job_id: job.id,
    target_id: targetId,
    ...(job.scheduledFor !== undefined && job.scheduledFor !== null
      ? { scheduled_for: job.scheduledFor }
      : {}),
    ...(events ? { events } : {}),
  };
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
  tiktokInteractions,
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
        tiktokInteractions,
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
  const uploaded = await invoke<PublishJob | undefined>("social_upload_artifact", {
    jobId: job.id,
    filePath: entry.outputPath,
  });
  const uploadJob = uploaded ?? job;

  return {
    provider,
    jobId: uploadJob.id,
    uploadState: scheduledUploadStateFromJob(uploadJob, targetId),
    metadata: buildSchedulerMetadata({
      title: title || entry.label,
      description,
      tagsInput,
      thumbnailPath,
      privacy,
      scheduledFor,
      tiktokInteractions,
    }),
  };
}

export async function publishSchedulerPostToAccounts({
  accounts,
  cadenceMinutes = 0,
  ...input
}: SchedulerMultiPublishInput): Promise<SchedulerPublishResult[]> {
  const validationErrors = validateSchedulerMetadataForAccounts(accounts, {
    title: input.title,
    description: input.description,
    tagsInput: input.tagsInput,
    thumbnailPath: input.thumbnailPath,
    privacy: input.privacy,
    scheduledFor: input.scheduledFor,
  });
  if (validationErrors.length > 0) {
    throw new Error(`Not valid: ${validationErrorCopy(validationErrors[0])}`);
  }

  const results: SchedulerPublishResult[] = [];
  for (const slot of buildSchedulerCadenceSlots(
    accounts,
    input.scheduledFor,
    cadenceMinutes,
  )) {
    results.push(
      await publishSchedulerPostViaServer({
        ...input,
        account: slot.account,
        scheduledFor: slot.scheduledFor,
      }),
    );
  }
  return results;
}
