// Pure model + derivation helpers for the server-backed social publishing
// desktop surfaces. JSX-free so it can be exercised by
// `apps/desktop/tests/social-model.test.ts` without React or Tauri.
//
// Field names are the camelCase serde mirror of the `montage-social`
// `SocialApi` response DTOs (`AccountSummary`, `ProviderSummary`,
// `PublishJobResponse`, `AccountUsageAudit`). SRP: derivations here,
// presentation in the `.tsx` files.

import type { TikTokInteractionSettings } from "../../state/uploadMetadata.ts";

export type { TikTokInteractionSettings } from "../../state/uploadMetadata.ts";

export type Provider = "youtube" | "tiktok" | "instagram" | "twitter_x";

export type OwnerRef = { user: string } | { workspace: string };

export type Eligibility = { eligible: boolean; reasons: string[] };

export type Capabilities = {
  nativeScheduling: boolean;
  queueScheduling: boolean;
  uploadVideo: boolean;
  uploadThumbnail: boolean;
  publicPosting: boolean;
  requiresUserConsent: boolean;
};

export type AccountStatus =
  | "connected"
  | "needs_reauth"
  | "missing_scope"
  | "ineligible"
  | "disabled"
  | "revoked";

export type ProviderSummary = {
  provider: Provider;
  displayName: string;
  scopes: string[];
  capabilities: Capabilities;
  eligibility: Eligibility;
};

export type AccountSummary = {
  id: string;
  owner: OwnerRef;
  provider: Provider;
  providerAccountId: string;
  displayName: string;
  handle: string | null;
  avatarUrl: string | null;
  accountKind: string;
  status: AccountStatus;
  scopes: string[];
  capabilities: Capabilities;
  eligibility: Eligibility;
  lastVerifiedAt: number | null;
  createdAt: number;
  updatedAt: number;
};

const STATUS_LABELS: Record<AccountStatus, string> = {
  connected: "Connected",
  needs_reauth: "Needs reconnect",
  missing_scope: "Missing permission",
  ineligible: "Not eligible",
  disabled: "Disabled",
  revoked: "Revoked",
};

export function accountStatusLabel(status: AccountStatus): string {
  return STATUS_LABELS[status];
}

export function canReconnect(status: AccountStatus): boolean {
  return status === "needs_reauth" || status === "missing_scope" || status === "revoked";
}

export function canViewAccountAudit(_status: AccountStatus): boolean {
  return true;
}

export type ManualPublishFieldsInput = {
  provider: Provider;
  privacy: "private" | "unlisted" | "public";
  title: string;
  description: string;
  tagsInput: string;
  thumbnailPath: string;
  tiktokInteractions?: Partial<TikTokInteractionSettings>;
};

export function buildPlatformFieldsForPublish(
  input: ManualPublishFieldsInput,
): Record<string, unknown> {
  const fields: Record<string, unknown> = {
    privacy: input.privacy,
    title: input.title,
    description: input.description,
    tags: parseTagsInput(input.tagsInput),
  };
  if (input.thumbnailPath.trim()) {
    fields.thumbnailRef = `file://${input.thumbnailPath.trim()}`;
  }
  if (input.provider === "instagram") {
    if (!input.description.trim() && input.title.trim()) {
      fields.description = input.title.trim();
    }
    delete fields.title;
  }
  if (input.provider === "twitter_x") {
    delete fields.privacy;
    delete fields.description;
    delete fields.tags;
    delete fields.thumbnailRef;
  }
  if (input.provider === "tiktok") {
    delete fields.description;
    delete fields.tags;
    delete fields.thumbnailRef;
    fields.disableDuet = input.tiktokInteractions?.disableDuet ?? false;
    fields.disableComment = input.tiktokInteractions?.disableComment ?? false;
    fields.disableStitch = input.tiktokInteractions?.disableStitch ?? false;
  }
  return fields;
}

function parseTagsInput(value: string): string[] {
  const tags: string[] = [];
  for (const raw of value.split(",")) {
    const tag = raw.trim();
    if (tag && !tags.includes(tag)) tags.push(tag);
  }
  return tags;
}

/** Maps facade reason codes to human copy. Extend as new codes appear. */
const REASON_COPY: Record<string, string> = {
  account_not_eligible: "account not eligible",
  account_not_connected: "account not connected",
  unaudited_client_can_only_post_to_private_accounts:
    "TikTok app is in review mode; set the TikTok account to private, then retry",
  url_ownership_unverified: "TikTok media URL domain is not verified",
  missing_publish_capability: "missing publish capability",
  network_or_server_error: "temporary server/provider error",
  scheduled_time_invalid: "scheduled time is in the past",
  missing_youtube_upload_scope: "missing YouTube upload permission",
};

export function reasonCopy(code: string): string {
  return REASON_COPY[code] ?? code.replace(/[_.]/g, " ");
}

export function eligibilitySummary(eligibility: Eligibility): string {
  if (eligibility.eligible) return "Eligible";
  const first = eligibility.reasons[0];
  return first ? `Not eligible — ${reasonCopy(first)}` : "Not eligible";
}

export type PublishJobStatus =
  | "draft"
  | "validated"
  | "scheduled"
  | "uploading"
  | "processing"
  | "published"
  | "failed"
  | "requires_action"
  | "cancelled";

export type PublishJobEvent = {
  id: string;
  eventType: string;
  message: string;
  metadata: unknown;
  createdAt: number;
};

export type PublishJob = {
  id: string;
  campaignId: string;
  variantId: string;
  connectedAccountId: string;
  provider: Provider;
  status: PublishJobStatus;
  attemptCount: number;
  scheduledFor: number;
  providerPostId: string | null;
  providerPostUrl: string | null;
  normalizedError: string | null;
  rawErrorRef: string | null;
  requiresActionReason: string | null;
  createdAt: number;
  updatedAt: number;
  events: PublishJobEvent[];
};

const JOB_STATUS_LABELS: Record<PublishJobStatus, string> = {
  draft: "Draft",
  validated: "Validated",
  scheduled: "Scheduled",
  uploading: "Uploading",
  processing: "Processing",
  published: "Published",
  failed: "Failed",
  requires_action: "Action needed",
  cancelled: "Cancelled",
};

export function jobStatusLabel(status: PublishJobStatus): string {
  return JOB_STATUS_LABELS[status];
}

export function canCancel(status: PublishJobStatus): boolean {
  return status !== "published" && status !== "cancelled";
}

export function canRetry(status: PublishJobStatus): boolean {
  return status === "failed" || status === "requires_action";
}

export function canReschedule(status: PublishJobStatus): boolean {
  return status === "scheduled";
}

/** A job is still "in flight" (server will advance it) until it reaches a
 * terminal state. The UI polls while any job is non-terminal. */
export function isTerminal(status: PublishJobStatus): boolean {
  return status === "published" || status === "failed" || status === "cancelled";
}

export type PublishJobStatusCounts = {
  scheduled: number;
  processing: number;
  published: number;
  failed: number;
  requiresAction: number;
};

export type AccountUsageAudit = {
  connectedAccountId: string;
  owner: OwnerRef;
  jobs: PublishJob[];
  events: PublishJobEvent[];
  statusCounts: PublishJobStatusCounts;
};
