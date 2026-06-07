import type { RenderQueueEntry } from "../app/renderQueue.ts";
import { reasonCopy } from "../app/social/socialModel.ts";
import type {
  UploadMetadata,
  UploadVisibility,
} from "../state/uploadMetadata.ts";
import type {
  CampaignManifest,
  CampaignPlatform,
  PlatformVariant,
  PublishableItem,
} from "./manifest.ts";

export type CampaignUploadRequest = {
  campaignId: string;
  itemId: string;
  variantId: string;
  provider: CampaignPlatform;
  accountId?: string;
  jobId: string;
  filePath: string;
  title: string;
  metadata: UploadMetadata;
};

type InvokeFn = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

type ValidatedTarget = {
  validation_state?: string;
  validationState?: string;
  validation_reasons?: string[];
  validationReasons?: string[];
};

const VISIBILITIES = new Set<UploadVisibility>([
  "private",
  "unlisted",
  "public",
]);

function visibilityFrom(value: unknown): UploadVisibility {
  return typeof value === "string" && VISIBILITIES.has(value as UploadVisibility)
    ? (value as UploadVisibility)
    : "private";
}

function tagsFrom(value: unknown, fallback: string[]): string[] {
  if (Array.isArray(value)) {
    return value
      .filter((tag): tag is string => typeof tag === "string")
      .map((tag) => tag.trim())
      .filter(Boolean);
  }
  if (typeof value === "string") {
    return value
      .split(",")
      .map((tag) => tag.trim())
      .filter(Boolean);
  }
  return fallback;
}

function validationState(target: ValidatedTarget): string {
  return target.validation_state ?? target.validationState ?? "unknown";
}

function validationFailureReason(target: ValidatedTarget): string {
  const reasons = target.validation_reasons ?? target.validationReasons ?? [];
  const first = reasons[0];
  return first ? reasonCopy(first) : validationState(target);
}

function itemForVariant(
  campaign: CampaignManifest,
  variant: PlatformVariant,
): PublishableItem | undefined {
  return campaign.items.find((item) => item.itemId === variant.itemId);
}

function entryForItem(
  item: PublishableItem,
  entries: RenderQueueEntry[],
): RenderQueueEntry | undefined {
  return entries.find((entry) => `item-${entry.id}` === item.itemId);
}

function metadataFor(
  item: PublishableItem,
  variant: PlatformVariant,
): UploadMetadata {
  return {
    title: String(variant.platformFields.title ?? item.title),
    description: String(
      variant.platformFields.description ?? (item.description || item.caption),
    ),
    tags: tagsFrom(variant.platformFields.tags, item.hashtags),
    visibility: visibilityFrom(variant.platformFields.visibility),
    scheduledAt: variant.scheduledFor,
    thumbnailPath:
      typeof variant.platformFields.thumbnailPath === "string"
        ? variant.platformFields.thumbnailPath
        : item.thumbnailPath,
  };
}

/** True when the render already has a terminal `published` upload state for
 *  this provider — re-uploading it would create a duplicate provider post. */
function alreadyPublished(
  entry: RenderQueueEntry,
  provider: CampaignPlatform,
): boolean {
  return entry.uploadStates?.[provider]?.state === "published";
}

export function campaignUploadRequests(
  campaign: CampaignManifest,
  entries: RenderQueueEntry[],
): CampaignUploadRequest[] {
  const requests: CampaignUploadRequest[] = [];
  for (const variant of campaign.platformVariants) {
    if (variant.status !== "approved" && variant.status !== "scheduled") {
      continue;
    }
    const item = itemForVariant(campaign, variant);
    if (!item || item.approvalState !== "approved") continue;
    const entry = entryForItem(item, entries);
    if (
      !entry ||
      entry.status !== "done" ||
      !entry.jobId ||
      !entry.outputPath
    ) {
      continue;
    }
    // Skip targets whose render already published to this provider — the
    // auto-upload path may have handled it, and re-registering would risk a
    // duplicate provider post.
    if (alreadyPublished(entry, variant.platform)) {
      continue;
    }
    requests.push({
      campaignId: campaign.campaignId,
      itemId: item.itemId,
      variantId: variant.variantId,
      provider: variant.platform,
      accountId: variant.accountId ?? entry.uploadAccountIds?.[variant.platform],
      jobId: entry.jobId,
      filePath: entry.outputPath,
      title: item.title,
      metadata: metadataFor(item, variant),
    });
  }
  return requests;
}

// ── Server-backed publishing (Phase 5 path) ────────────────────────────────
//
// Publishes campaign variants through the montage-social SERVER instead of the
// legacy desktop-local render-queue path. Per request: resolve the connected
// account for the provider, then
//   social_bind_target → social_validate_target → social_schedule_target
//     → social_upload_artifact
// The server worker fires the upload. Returns the per-variant job id (or an
// error) so the caller can record variant→job mappings + surface failures.

type AccountSummaryLite = {
  id: string;
  provider: string;
  capabilities: { uploadVideo: boolean };
};

export type ServerPublishResult = {
  variantId: string;
  jobId?: string;
  error?: string;
};

function randomId(prefix: string): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${prefix}-${hex}`;
}

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

function localArtifactRef(path: string): string {
  return `file://${path}`;
}

function platformFieldsFromMetadata(
  provider: CampaignPlatform,
  metadata: UploadMetadata,
  fallbackTitle: string,
): Record<string, unknown> {
  const fields: Record<string, unknown> = {
    privacy: metadata.visibility ?? "private",
    title: metadata.title || fallbackTitle,
    description: metadata.description ?? "",
    tags: metadata.tags,
  };
  if (metadata.thumbnailPath) {
    fields.thumbnailRef = localArtifactRef(metadata.thumbnailPath);
  }
  if (provider === "instagram") {
    delete fields.title;
  }
  return fields;
}

/**
 * Publish the campaign's approved variants via the server. `scheduledFor`
 * defaults to "now" (fire on the next worker tick).
 */
export async function publishCampaignViaServer(
  campaign: CampaignManifest,
  entries: RenderQueueEntry[],
  invoke: InvokeFn,
  options: { scheduledFor?: number } = {},
): Promise<ServerPublishResult[]> {
  const requests = campaignUploadRequests(campaign, entries);
  if (requests.length === 0) return [];

  const accounts = await invoke<AccountSummaryLite[]>("social_accounts");
  const accountFor = (
    provider: string,
    accountId: string | undefined,
  ): AccountSummaryLite | undefined => {
    if (accountId) {
      return accounts.find(
        (a) =>
          a.id === accountId &&
          a.provider === provider &&
          a.capabilities.uploadVideo,
      );
    }
    return accounts.find((a) => a.provider === provider && a.capabilities.uploadVideo);
  };

  const scheduledFor = options.scheduledFor ?? nowSeconds();
  const results: ServerPublishResult[] = [];

  for (const req of requests) {
    const account = accountFor(req.provider, req.accountId);
    if (!account) {
      const error = req.accountId
        ? `Selected ${req.provider} account is not connected or cannot upload video`
        : `No upload-capable ${req.provider} account connected`;
      results.push({
        variantId: req.variantId,
        error,
      });
      continue;
    }
    try {
      const targetId = randomId("target");
      await invoke("social_bind_target", {
        args: {
          targetId,
          campaignId: req.campaignId,
          variantId: req.variantId,
          connectedAccountId: account.id,
          platformFields: platformFieldsFromMetadata(
            req.provider,
            req.metadata,
            req.title,
          ),
          scheduledFor,
          now: nowSeconds(),
        },
      });
      const validated = await invoke<ValidatedTarget>(
        "social_validate_target",
        { targetId, now: nowSeconds() },
      );
      const validatedState = validationState(validated);
      if (validatedState !== "valid") {
        results.push({
          variantId: req.variantId,
          error: `Not valid: ${validationFailureReason(validated)}`,
        });
        continue;
      }
      const jobId = randomId("job");
      const job = await invoke<{ id: string }>("social_schedule_target", {
        args: { targetId, jobId, artifactRef: "", now: nowSeconds() },
      });
      await invoke("social_upload_artifact", {
        jobId: job.id,
        filePath: req.filePath,
      });
      results.push({ variantId: req.variantId, jobId: job.id });
    } catch (e) {
      results.push({ variantId: req.variantId, error: String(e) });
    }
  }
  return results;
}

/** Per-target upload entry returned by `poll_upload_states`. Mirrors the
 *  backend `UploadJobEntry` (same shape the render-queue worker polls). */
type UploadJobEntryWire = {
  job_id: string;
  upload_targets: string[];
  upload_states: Record<string, { state: string } & Record<string, unknown>>;
  published_urls: Record<string, string>;
};

export type StartCampaignUploadsOptions = {
  /** When true (default), stamp the computed AI disclosure onto the upload so
   *  generated-media cuts carry the platform flag. Mirrors the render-queue
   *  worker's default-on behavior; pass false for the power-user opt-out. */
  autoDisclose?: boolean;
  /** Poll cadence for upload outcomes, in ms. Default 800ms. */
  pollIntervalMs?: number;
  /** Max poll ticks before returning (the backend keeps running). */
  maxPollTicks?: number;
  /** Sleep hook — injectable so tests don't wait on real timers. */
  sleep?: (ms: number) => Promise<void>;
  /** Invoked with the started requests immediately after uploads kick off,
   *  before polling begins. Lets the caller record variant→job mappings right
   *  away rather than waiting for the poll loop to drain. */
  onStarted?: (requests: CampaignUploadRequest[]) => void;
};

const defaultSleep = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms));

export async function startCampaignUploads(
  campaign: CampaignManifest,
  entries: RenderQueueEntry[],
  invoke: InvokeFn,
  options: StartCampaignUploadsOptions = {},
): Promise<CampaignUploadRequest[]> {
  const autoDisclose = options.autoDisclose ?? true;
  const pollIntervalMs = options.pollIntervalMs ?? 800;
  const maxPollTicks = options.maxPollTicks ?? 750;
  const sleep = options.sleep ?? defaultSleep;

  const requests = campaignUploadRequests(campaign, entries);
  const byJob = new Map<string, CampaignUploadRequest[]>();
  for (const request of requests) {
    const bucket = byJob.get(request.jobId) ?? [];
    bucket.push(request);
    byJob.set(request.jobId, bucket);
  }
  for (const [jobId, jobRequests] of byJob) {
    await invoke<void>("set_render_upload_targets", {
      jobId,
      providers: jobRequests.map((request) => request.provider),
    });
    await Promise.all(
      jobRequests.map((request) =>
        invoke<void>("set_upload_metadata", {
          jobId,
          provider: request.provider,
          metadata: request.metadata,
        }),
      ),
    );
    // Compute + stamp the AI disclosure BEFORE starting uploads, matching the
    // render-queue worker. Without this, campaign-approved uploads of
    // generated-media cuts would reach providers with `ai_disclosure: None`,
    // bypassing the default-on disclosure. Failures are non-fatal — the
    // backend command logs and the upload chain stays alive.
    try {
      await invoke<unknown>("compute_ai_disclosure", { jobId, autoDisclose });
    } catch {
      // Disclosure compute is best-effort; proceed with the upload.
    }
    await invoke<void>("start_uploads_for_job", {
      jobId,
      filePath: jobRequests[0].filePath,
      title: jobRequests[0].title,
    });
  }

  // Surface the started requests before the (potentially slow) poll loop so
  // the caller can record variant→job mappings immediately.
  options.onStarted?.(requests);

  // Poll each started job to terminal so campaign variants transition to
  // published/failed and capture provider URLs — the campaign path has no
  // other poller (the render-queue worker only polls its own auto-uploads).
  await Promise.all(
    [...byJob.keys()].map((jobId) =>
      pollCampaignUploadStates(jobId, invoke, {
        pollIntervalMs,
        maxPollTicks,
        sleep,
      }),
    ),
  );

  return requests;
}

/** Polls `poll_upload_states` for one job until every target is terminal
 *  (`published`/`failed`) or the tick budget is exhausted. */
async function pollCampaignUploadStates(
  jobId: string,
  invoke: InvokeFn,
  opts: {
    pollIntervalMs: number;
    maxPollTicks: number;
    sleep: (ms: number) => Promise<void>;
  },
): Promise<void> {
  for (let tick = 0; tick < opts.maxPollTicks; tick++) {
    await opts.sleep(opts.pollIntervalMs);
    let snapshot: UploadJobEntryWire | null;
    try {
      snapshot = await invoke<UploadJobEntryWire | null>("poll_upload_states", {
        jobId,
      });
    } catch {
      // Transient IPC error — retry next tick.
      continue;
    }
    if (!snapshot) continue;
    const states = Object.values(snapshot.upload_states);
    const allTerminal =
      states.length > 0 &&
      states.every((s) => s.state === "published" || s.state === "failed");
    if (allTerminal) return;
  }
}
