import type { RenderQueueEntry } from "../app/renderQueue.ts";
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
  jobId: string;
  filePath: string;
  title: string;
  metadata: UploadMetadata;
};

type InvokeFn = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

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
    tags: item.hashtags,
    visibility: visibilityFrom(variant.platformFields.visibility),
    scheduledAt: variant.scheduledFor,
    thumbnailPath:
      typeof variant.platformFields.thumbnailPath === "string"
        ? variant.platformFields.thumbnailPath
        : item.thumbnailPath,
  };
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
    requests.push({
      campaignId: campaign.campaignId,
      itemId: item.itemId,
      variantId: variant.variantId,
      provider: variant.platform,
      jobId: entry.jobId,
      filePath: entry.outputPath,
      title: item.title,
      metadata: metadataFor(item, variant),
    });
  }
  return requests;
}

export async function startCampaignUploads(
  campaign: CampaignManifest,
  entries: RenderQueueEntry[],
  invoke: InvokeFn,
): Promise<CampaignUploadRequest[]> {
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
    await invoke<void>("start_uploads_for_job", {
      jobId,
      filePath: jobRequests[0].filePath,
      title: jobRequests[0].title,
    });
  }
  return requests;
}
