import { DELIVERY_TARGETS, type DeliveryTargetKey } from "../app/deliveryTargets";
import type { RenderQueueEntry } from "../app/renderQueue";
import {
  createCampaignManifest,
  createPlatformVariant,
  createPublishableItem,
  type CampaignManifest,
  type CampaignPlatform,
  type CampaignType,
  type PublishableItemKind,
} from "./manifest";

const PUBLISHING_TARGETS = new Set<DeliveryTargetKey>(["youtube", "tiktok", "instagram"]);

export type PlanCampaignInput = {
  campaignType: CampaignType;
  sourceAssetId: string;
  title: string;
  selectedTargets: DeliveryTargetKey[];
  renderEntries: RenderQueueEntry[];
  createdAt?: number;
};

function deliveryTargetForEntry(entry: RenderQueueEntry): DeliveryTargetKey | undefined {
  return entry.targetId in DELIVERY_TARGETS ? (entry.targetId as DeliveryTargetKey) : undefined;
}

function itemKindForEntry(entry: RenderQueueEntry, target: DeliveryTargetKey | undefined): PublishableItemKind {
  if (target === "youtube" || entry.kind === "video_master") return "long_form";
  return "short";
}

function aspectRatioForTarget(target: DeliveryTargetKey | undefined): string | undefined {
  if (target === undefined) return undefined;

  const spec = DELIVERY_TARGETS[target];
  if (spec.width === undefined || spec.height === undefined) return undefined;

  const divisor = greatestCommonDivisor(spec.width, spec.height);
  return `${spec.width / divisor}:${spec.height / divisor}`;
}

function greatestCommonDivisor(a: number, b: number): number {
  let x = Math.abs(a);
  let y = Math.abs(b);
  while (y !== 0) {
    const next = x % y;
    x = y;
    y = next;
  }
  return x || 1;
}

function platformForTarget(target: DeliveryTargetKey): CampaignPlatform | undefined {
  return PUBLISHING_TARGETS.has(target) ? (target as CampaignPlatform) : undefined;
}

function bestItemForPlatform(
  platform: CampaignPlatform,
  entries: RenderQueueEntry[],
): RenderQueueEntry | undefined {
  if (platform === "youtube") {
    return entries.find((entry) => entry.targetId === "youtube") ?? entries[0];
  }

  return (
    entries.find((entry) => entry.targetId === platform) ??
    entries.find((entry) => entry.kind === "video_reframe") ??
    entries[0]
  );
}

export function planCampaignFromDelivery(input: PlanCampaignInput): CampaignManifest {
  const doneVideoEntries = input.renderEntries.filter(
    (entry) =>
      entry.status === "done" &&
      entry.outputPath !== undefined &&
      (entry.kind === "video_master" || entry.kind === "video_reframe"),
  );
  const items = doneVideoEntries.map((entry) => {
    const target = deliveryTargetForEntry(entry);
    return createPublishableItem({
      itemId: `item-${entry.id}`,
      kind: itemKindForEntry(entry, target),
      title: entry.label,
      caption: "",
      description: "",
      artifactPath: entry.outputPath,
      aspectRatio: aspectRatioForTarget(target),
      preflightChecks: [
        {
          id: "render.done",
          severity: "pass",
          message: "Render finished and is ready for campaign approval.",
        },
      ],
      approvalState: entry.reviewStatus === "approved" ? "approved" : "draft",
    });
  });
  const platformVariants = input.selectedTargets
    .map(platformForTarget)
    .filter((platform): platform is CampaignPlatform => platform !== undefined)
    .map((platform) => {
      const entry = bestItemForPlatform(platform, doneVideoEntries);
      return entry
        ? createPlatformVariant({
            variantId: `variant-${entry.id}-${platform}`,
            itemId: `item-${entry.id}`,
            platform,
            platformFields: {},
          })
        : undefined;
    })
    .filter((variant) => variant !== undefined);

  return createCampaignManifest({
    campaignId: `campaign-${input.sourceAssetId}-${input.createdAt ?? Date.now()}`,
    sourceAssetId: input.sourceAssetId,
    campaignType: input.campaignType,
    title: input.title,
    items,
    platformVariants,
    evidence: {
      selectedTargets: input.selectedTargets,
      renderEntryIds: doneVideoEntries.map((entry) => entry.id),
    },
    createdAt: input.createdAt,
  });
}
