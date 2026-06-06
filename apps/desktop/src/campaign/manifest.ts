import type { DeliveryTargetKey } from "../app/deliveryTargets";

export const CAMPAIGN_MANIFEST_VERSION = 1;

export type CampaignType = "podcast" | "shorts";
export type CampaignApprovalState = "draft" | "approved" | "changes_requested";
export type PublishableItemKind = "long_form" | "short" | "thumbnail" | "caption" | "metadata";
export type PlatformVariantStatus =
  | "draft"
  | "approved"
  | "scheduled"
  | "uploading"
  | "processing"
  | "published"
  | "failed"
  | "requires_action"
  | "cancelled";
export type CampaignPlatform = Extract<
  DeliveryTargetKey,
  "youtube" | "tiktok" | "instagram" | "twitter_x"
>;
export type PreflightSeverity = "pass" | "info" | "warning" | "error" | "failure";

export type TranscriptAnchor = {
  startS: number;
  endS: number;
  text: string;
};

export type CampaignPreflightCheck = {
  id: string;
  severity: PreflightSeverity;
  message: string;
};

export type PublishableItem = {
  itemId: string;
  kind: PublishableItemKind;
  title: string;
  caption: string;
  description: string;
  hashtags: string[];
  artifactPath?: string;
  thumbnailPath?: string;
  coverTimestampMs?: number;
  durationS?: number;
  aspectRatio?: string;
  transcriptAnchors: TranscriptAnchor[];
  preflightChecks: CampaignPreflightCheck[];
  approvalState: CampaignApprovalState;
};

export type PlatformVariant = {
  variantId: string;
  itemId: string;
  platform: CampaignPlatform;
  accountId?: string;
  platformFields: Record<string, string | number | boolean | undefined>;
  scheduledFor?: number;
  status: PlatformVariantStatus;
  publishJobId?: string;
};

export type CampaignManifest = {
  version: typeof CAMPAIGN_MANIFEST_VERSION;
  campaignId: string;
  sourceAssetId: string;
  campaignType: CampaignType;
  title: string;
  items: PublishableItem[];
  platformVariants: PlatformVariant[];
  evidence: Record<string, unknown>;
  approvalState: CampaignApprovalState;
  createdAt: number;
  updatedAt: number;
};

export type ApprovalSummary = {
  totalItems: number;
  approvedItems: number;
  totalVariants: number;
  approvedVariants: number;
  scheduledVariants: number;
};

export function createPublishableItem(
  input: Omit<Partial<PublishableItem>, "itemId" | "kind" | "title"> &
    Pick<PublishableItem, "itemId" | "kind" | "title">,
): PublishableItem {
  return {
    caption: input.caption ?? "",
    description: input.description ?? "",
    hashtags: input.hashtags ?? [],
    transcriptAnchors: input.transcriptAnchors ?? [],
    preflightChecks: input.preflightChecks ?? [],
    approvalState: input.approvalState ?? "draft",
    ...input,
  };
}

export function createPlatformVariant(
  input: Omit<Partial<PlatformVariant>, "variantId" | "itemId" | "platform"> &
    Pick<PlatformVariant, "variantId" | "itemId" | "platform">,
): PlatformVariant {
  return {
    platformFields: input.platformFields ?? {},
    status: input.status ?? "draft",
    ...input,
  };
}

export function createCampaignManifest(
  input: Omit<
    Partial<CampaignManifest>,
    "campaignId" | "sourceAssetId" | "campaignType" | "title" | "items" | "platformVariants"
  > &
    Pick<
      CampaignManifest,
      "campaignId" | "sourceAssetId" | "campaignType" | "title" | "items" | "platformVariants"
    >,
): CampaignManifest {
  const createdAt = input.createdAt ?? Date.now();
  return {
    version: CAMPAIGN_MANIFEST_VERSION,
    evidence: input.evidence ?? {},
    approvalState: input.approvalState ?? "draft",
    updatedAt: input.updatedAt ?? createdAt,
    ...input,
    createdAt,
  };
}

export function approvalSummary(manifest: CampaignManifest): ApprovalSummary {
  return {
    totalItems: manifest.items.length,
    approvedItems: manifest.items.filter((item) => item.approvalState === "approved").length,
    totalVariants: manifest.platformVariants.length,
    approvedVariants: manifest.platformVariants.filter((variant) => variant.status === "approved").length,
    scheduledVariants: manifest.platformVariants.filter((variant) => variant.scheduledFor !== undefined).length,
  };
}
