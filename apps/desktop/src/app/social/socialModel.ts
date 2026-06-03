// Pure model + derivation helpers for the server-backed social publishing
// desktop surfaces. JSX-free so it can be exercised by
// `apps/desktop/tests/social-model.test.ts` without React or Tauri.
//
// Field names are the camelCase serde mirror of the `awidat-social`
// `SocialApi` response DTOs (`AccountSummary`, `ProviderSummary`,
// `PublishJobResponse`, `AccountUsageAudit`). SRP: derivations here,
// presentation in the `.tsx` files.

export type Provider = "youtube" | "tiktok" | "instagram";

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

/** Maps facade reason codes to human copy. Extend as new codes appear. */
const REASON_COPY: Record<string, string> = {
  account_not_eligible: "account not eligible",
  account_not_connected: "account not connected",
  missing_publish_capability: "missing publish capability",
  scheduled_time_invalid: "scheduled time is in the past",
  missing_youtube_upload_scope: "missing YouTube upload permission",
};

export function reasonCopy(code: string): string {
  return REASON_COPY[code] ?? code.replace(/_/g, " ");
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

/** Which worker action advances a job from its current state, if any. */
export function nextWorkerAction(
  status: PublishJobStatus,
): "execute" | "poll" | null {
  if (status === "scheduled" || status === "uploading") return "execute";
  if (status === "processing") return "poll";
  return null;
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
