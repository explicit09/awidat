# Clip Campaign Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Phase 1 of the Clip Campaign Engine: local campaign manifests, campaign planning from delivery outputs, approval state, and a delivery-surface campaign panel without live posting.

**Architecture:** Add a small `apps/desktop/src/campaign/` module above the existing delivery/render queue code. The campaign layer owns the manifest shape, local campaign store, and pure planning helpers; the delivery UI reads that state and reuses existing upload metadata and render queue concepts.

**Tech Stack:** TypeScript, React, Zustand, existing desktop localStorage persistence, Node strip-types tests, existing Awidat delivery target and render queue stores.

---

## Scope

This plan implements only Phase 1 from [the design spec](../specs/2026-06-02-clip-campaign-engine-design.md): campaign manifest and local approval. It does not implement server-side OAuth account storage, durable scheduled workers, live TikTok/Instagram posting, or analytics ingestion.

## File Structure

- Create `apps/desktop/src/campaign/manifest.ts`: serializable campaign manifest types, version constant, factory helpers, and approval summary selectors.
- Create `apps/desktop/src/campaign/planner.ts`: pure helpers that turn selected delivery targets and rendered queue entries into a local campaign plan.
- Create `apps/desktop/src/campaign/store.ts`: Zustand store for local campaign manifests and approval mutations.
- Create `apps/desktop/src/shell/delivery/CampaignApprovalPanel.tsx`: compact campaign-native approval UI for the delivery surface.
- Modify `apps/desktop/src/shell/DeliverySurface.tsx`: render the campaign panel near the existing render queue and pass existing target/render state into it.
- Modify `apps/desktop/package.json`: add narrow test scripts for the campaign module.
- Create `apps/desktop/tests/campaign-manifest.test.ts`: pure manifest tests.
- Create `apps/desktop/tests/campaign-planner.test.ts`: planner tests.
- Create `apps/desktop/tests/campaign-store.test.ts`: store approval and persistence tests.

---

### Task 1: Campaign Manifest Types

**Files:**
- Create: `apps/desktop/src/campaign/manifest.ts`
- Create: `apps/desktop/tests/campaign-manifest.test.ts`
- Modify: `apps/desktop/package.json`

- [ ] **Step 1: Write the failing manifest test**

Create `apps/desktop/tests/campaign-manifest.test.ts`:

```ts
import { strict as assert } from "node:assert";

import {
  CAMPAIGN_MANIFEST_VERSION,
  approvalSummary,
  createCampaignManifest,
  createPublishableItem,
  createPlatformVariant,
  type CampaignManifest,
} from "../src/campaign/manifest.ts";

const now = 1_782_600_000_000;

const item = createPublishableItem({
  itemId: "item-short-1",
  kind: "short",
  title: "Founder Explains The AI Workflow",
  caption: "Founder explains the AI workflow #ai #startup",
  hashtags: ["ai", "startup"],
  artifactPath: "/tmp/short.mp4",
  durationS: 42,
  aspectRatio: "9:16",
  transcriptAnchors: [{ startS: 12.1, endS: 54.2, text: "This is the workflow." }],
  preflightChecks: [{ id: "duration.short", severity: "pass", message: "Short duration is valid." }],
});

const variant = createPlatformVariant({
  variantId: "variant-tiktok-1",
  itemId: "item-short-1",
  platform: "tiktok",
  accountId: "local-tiktok",
  scheduledFor: now + 86_400_000,
  platformFields: {
    privacy: "private",
    coverTimestampMs: 1000,
  },
});

const manifest: CampaignManifest = createCampaignManifest({
  campaignId: "campaign-1",
  sourceAssetId: "asset-1",
  campaignType: "shorts",
  title: "AI Workflow Shorts Campaign",
  items: [item],
  platformVariants: [variant],
  createdAt: now,
});

assert.equal(CAMPAIGN_MANIFEST_VERSION, 1);
assert.equal(manifest.version, 1);
assert.equal(manifest.approvalState, "draft");
assert.equal(manifest.items[0].approvalState, "draft");
assert.equal(manifest.platformVariants[0].status, "draft");
assert.equal(manifest.updatedAt, now);

const summary = approvalSummary(manifest);
assert.deepEqual(summary, {
  totalItems: 1,
  approvedItems: 0,
  totalVariants: 1,
  approvedVariants: 0,
  scheduledVariants: 1,
});

console.log("campaign-manifest: OK");
```

- [ ] **Step 2: Add the test script**

Modify `apps/desktop/package.json` scripts:

```json
"test:campaign-manifest": "node --experimental-strip-types tests/campaign-manifest.test.ts",
```

Place it near the existing `test:publishing-settings` script.

- [ ] **Step 3: Run test to verify it fails**

Run:

```bash
cd apps/desktop
pnpm test:campaign-manifest
```

Expected: FAIL with module-not-found for `../src/campaign/manifest.ts`.

- [ ] **Step 4: Implement the manifest module**

Create `apps/desktop/src/campaign/manifest.ts`:

```ts
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
export type CampaignPlatform = Extract<DeliveryTargetKey, "youtube" | "tiktok" | "instagram">;
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
```

- [ ] **Step 5: Run test to verify it passes**

Run:

```bash
cd apps/desktop
pnpm test:campaign-manifest
```

Expected: PASS and prints `campaign-manifest: OK`.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/package.json apps/desktop/src/campaign/manifest.ts apps/desktop/tests/campaign-manifest.test.ts
git commit -m "feat(campaign): add local manifest model"
```

---

### Task 2: Campaign Store and Approval Mutations

**Files:**
- Create: `apps/desktop/src/campaign/store.ts`
- Create: `apps/desktop/tests/campaign-store.test.ts`
- Modify: `apps/desktop/package.json`

- [ ] **Step 1: Write the failing store test**

Create `apps/desktop/tests/campaign-store.test.ts`:

```ts
import { strict as assert } from "node:assert";

import {
  createCampaignManifest,
  createPlatformVariant,
  createPublishableItem,
} from "../src/campaign/manifest.ts";
import { campaignStoreSelectors, useCampaignStore } from "../src/campaign/store.ts";

function resetStore(): void {
  useCampaignStore.setState({ campaigns: [] });
}

const campaign = createCampaignManifest({
  campaignId: "campaign-store-1",
  sourceAssetId: "asset-1",
  campaignType: "podcast",
  title: "Podcast Campaign",
  createdAt: 1_782_600_000_000,
  items: [
    createPublishableItem({
      itemId: "item-long",
      kind: "long_form",
      title: "Full Episode",
      artifactPath: "/tmp/episode.mp4",
    }),
  ],
  platformVariants: [
    createPlatformVariant({
      variantId: "variant-youtube",
      itemId: "item-long",
      platform: "youtube",
    }),
  ],
});

resetStore();
useCampaignStore.getState().upsertCampaign(campaign);
assert.equal(useCampaignStore.getState().campaigns.length, 1);
assert.equal(campaignStoreSelectors.active(useCampaignStore.getState())?.campaignId, "campaign-store-1");

useCampaignStore.getState().approveItem("campaign-store-1", "item-long");
let saved = useCampaignStore.getState().campaigns[0];
assert.equal(saved.items[0].approvalState, "approved");

useCampaignStore.getState().approveVariant("campaign-store-1", "variant-youtube");
saved = useCampaignStore.getState().campaigns[0];
assert.equal(saved.platformVariants[0].status, "approved");
assert.equal(saved.approvalState, "approved");

useCampaignStore.getState().requestChanges("campaign-store-1", "item-long");
saved = useCampaignStore.getState().campaigns[0];
assert.equal(saved.items[0].approvalState, "changes_requested");
assert.equal(saved.approvalState, "changes_requested");

useCampaignStore.getState().removeCampaign("campaign-store-1");
assert.equal(useCampaignStore.getState().campaigns.length, 0);

console.log("campaign-store: OK");
```

- [ ] **Step 2: Add the test script**

Modify `apps/desktop/package.json` scripts:

```json
"test:campaign-store": "node --experimental-strip-types tests/campaign-store.test.ts",
```

- [ ] **Step 3: Run test to verify it fails**

Run:

```bash
cd apps/desktop
pnpm test:campaign-store
```

Expected: FAIL with module-not-found for `../src/campaign/store.ts`.

- [ ] **Step 4: Implement the store**

Create `apps/desktop/src/campaign/store.ts`:

```ts
import { create } from "zustand";
import type {
  CampaignApprovalState,
  CampaignManifest,
  PlatformVariantStatus,
} from "./manifest";

const STORAGE_KEY = "awidat.campaigns.v1";

type CampaignState = {
  campaigns: CampaignManifest[];
  upsertCampaign: (campaign: CampaignManifest) => void;
  removeCampaign: (campaignId: string) => void;
  approveItem: (campaignId: string, itemId: string) => void;
  requestChanges: (campaignId: string, itemId: string) => void;
  approveVariant: (campaignId: string, variantId: string) => void;
  setVariantStatus: (
    campaignId: string,
    variantId: string,
    status: PlatformVariantStatus,
  ) => void;
};

function loadCampaigns(): CampaignManifest[] {
  if (typeof localStorage === "undefined") return [];
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    return Array.isArray(parsed) ? (parsed as CampaignManifest[]) : [];
  } catch {
    return [];
  }
}

function persist(campaigns: CampaignManifest[]): void {
  if (typeof localStorage === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(campaigns));
  } catch {
    // localStorage may be unavailable; local state still works.
  }
}

function campaignApprovalState(campaign: CampaignManifest): CampaignApprovalState {
  if (campaign.items.some((item) => item.approvalState === "changes_requested")) {
    return "changes_requested";
  }
  if (
    campaign.items.length > 0 &&
    campaign.platformVariants.length > 0 &&
    campaign.items.every((item) => item.approvalState === "approved") &&
    campaign.platformVariants.every((variant) => variant.status === "approved")
  ) {
    return "approved";
  }
  return "draft";
}

function updateCampaign(
  campaigns: CampaignManifest[],
  campaignId: string,
  update: (campaign: CampaignManifest) => CampaignManifest,
): CampaignManifest[] {
  return campaigns.map((campaign) =>
    campaign.campaignId === campaignId
      ? update({ ...campaign, updatedAt: Date.now() })
      : campaign,
  );
}

export const useCampaignStore = create<CampaignState>((set) => ({
  campaigns: loadCampaigns(),
  upsertCampaign: (campaign) => {
    set((state) => {
      const exists = state.campaigns.some((existing) => existing.campaignId === campaign.campaignId);
      const next = exists
        ? state.campaigns.map((existing) =>
            existing.campaignId === campaign.campaignId ? campaign : existing,
          )
        : [...state.campaigns, campaign];
      persist(next);
      return { campaigns: next };
    });
  },
  removeCampaign: (campaignId) => {
    set((state) => {
      const next = state.campaigns.filter((campaign) => campaign.campaignId !== campaignId);
      persist(next);
      return { campaigns: next };
    });
  },
  approveItem: (campaignId, itemId) => {
    set((state) => {
      const next = updateCampaign(state.campaigns, campaignId, (campaign) => {
        const updated = {
          ...campaign,
          items: campaign.items.map((item) =>
            item.itemId === itemId ? { ...item, approvalState: "approved" as const } : item,
          ),
        };
        return { ...updated, approvalState: campaignApprovalState(updated) };
      });
      persist(next);
      return { campaigns: next };
    });
  },
  requestChanges: (campaignId, itemId) => {
    set((state) => {
      const next = updateCampaign(state.campaigns, campaignId, (campaign) => {
        const updated = {
          ...campaign,
          items: campaign.items.map((item) =>
            item.itemId === itemId
              ? { ...item, approvalState: "changes_requested" as const }
              : item,
          ),
        };
        return { ...updated, approvalState: campaignApprovalState(updated) };
      });
      persist(next);
      return { campaigns: next };
    });
  },
  approveVariant: (campaignId, variantId) => {
    set((state) => {
      const next = updateCampaign(state.campaigns, campaignId, (campaign) => {
        const updated = {
          ...campaign,
          platformVariants: campaign.platformVariants.map((variant) =>
            variant.variantId === variantId ? { ...variant, status: "approved" as const } : variant,
          ),
        };
        return { ...updated, approvalState: campaignApprovalState(updated) };
      });
      persist(next);
      return { campaigns: next };
    });
  },
  setVariantStatus: (campaignId, variantId, status) => {
    set((state) => {
      const next = updateCampaign(state.campaigns, campaignId, (campaign) => ({
        ...campaign,
        platformVariants: campaign.platformVariants.map((variant) =>
          variant.variantId === variantId ? { ...variant, status } : variant,
        ),
      }));
      persist(next);
      return { campaigns: next };
    });
  },
}));

export const campaignStoreSelectors = {
  active: (state: Pick<CampaignState, "campaigns">) =>
    [...state.campaigns].sort((a, b) => b.updatedAt - a.updatedAt)[0],
};
```

- [ ] **Step 5: Run test to verify it passes**

Run:

```bash
cd apps/desktop
pnpm test:campaign-store
```

Expected: PASS and prints `campaign-store: OK`.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/package.json apps/desktop/src/campaign/store.ts apps/desktop/tests/campaign-store.test.ts
git commit -m "feat(campaign): add local approval store"
```

---

### Task 3: Campaign Planner From Delivery State

**Files:**
- Create: `apps/desktop/src/campaign/planner.ts`
- Create: `apps/desktop/tests/campaign-planner.test.ts`
- Modify: `apps/desktop/package.json`

- [ ] **Step 1: Write the failing planner test**

Create `apps/desktop/tests/campaign-planner.test.ts`:

```ts
import { strict as assert } from "node:assert";

import type { RenderQueueEntry } from "../src/app/renderQueue.ts";
import { planCampaignFromDelivery } from "../src/campaign/planner.ts";

const entries: RenderQueueEntry[] = [
  {
    id: "render-youtube",
    targetId: "youtube",
    label: "YouTube",
    kind: "video_master",
    status: "done",
    outputPath: "/tmp/youtube.mp4",
    reviewStatus: "approved",
    enqueuedAt: 1,
  },
  {
    id: "render-tiktok",
    targetId: "tiktok",
    label: "TikTok",
    kind: "video_reframe",
    status: "done",
    outputPath: "/tmp/tiktok.mp4",
    reviewStatus: "pending",
    enqueuedAt: 2,
  },
];

const campaign = planCampaignFromDelivery({
  campaignType: "podcast",
  sourceAssetId: "asset-123",
  title: "Episode rollout",
  selectedTargets: ["youtube", "tiktok", "instagram"],
  renderEntries: entries,
  createdAt: 1_782_600_000_000,
});

assert.equal(campaign.campaignType, "podcast");
assert.equal(campaign.items.length, 2);
assert.equal(campaign.platformVariants.length, 3);

const youtubeItem = campaign.items.find((item) => item.itemId === "item-render-youtube");
assert.ok(youtubeItem);
assert.equal(youtubeItem.kind, "long_form");
assert.equal(youtubeItem.artifactPath, "/tmp/youtube.mp4");
assert.equal(youtubeItem.approvalState, "approved");

const tiktokItem = campaign.items.find((item) => item.itemId === "item-render-tiktok");
assert.ok(tiktokItem);
assert.equal(tiktokItem.kind, "short");
assert.equal(tiktokItem.aspectRatio, "9:16");

const instagramVariant = campaign.platformVariants.find((variant) => variant.platform === "instagram");
assert.ok(instagramVariant);
assert.equal(instagramVariant.status, "draft");
assert.equal(instagramVariant.itemId, "item-render-tiktok");

console.log("campaign-planner: OK");
```

- [ ] **Step 2: Add the test script**

Modify `apps/desktop/package.json` scripts:

```json
"test:campaign-planner": "node --experimental-strip-types tests/campaign-planner.test.ts",
```

- [ ] **Step 3: Run test to verify it fails**

Run:

```bash
cd apps/desktop
pnpm test:campaign-planner
```

Expected: FAIL with module-not-found for `../src/campaign/planner.ts`.

- [ ] **Step 4: Implement the planner**

Create `apps/desktop/src/campaign/planner.ts`:

```ts
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

function itemKindForTarget(target: DeliveryTargetKey): PublishableItemKind {
  return target === "youtube" ? "long_form" : "short";
}

function aspectRatioForTarget(target: DeliveryTargetKey): string | undefined {
  const spec = DELIVERY_TARGETS[target];
  if (spec.width === undefined || spec.height === undefined) return undefined;
  return `${spec.width}:${spec.height}`;
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
    const target = entry.targetId as DeliveryTargetKey;
    return createPublishableItem({
      itemId: `item-${entry.id}`,
      kind: itemKindForTarget(target),
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
```

- [ ] **Step 5: Run test to verify it passes**

Run:

```bash
cd apps/desktop
pnpm test:campaign-planner
```

Expected: PASS and prints `campaign-planner: OK`.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/package.json apps/desktop/src/campaign/planner.ts apps/desktop/tests/campaign-planner.test.ts
git commit -m "feat(campaign): plan campaigns from delivery renders"
```

---

### Task 4: Campaign Approval Panel

**Files:**
- Create: `apps/desktop/src/shell/delivery/CampaignApprovalPanel.tsx`
- Modify: `apps/desktop/src/shell/DeliverySurface.tsx`

- [ ] **Step 1: Add the campaign panel component**

Create `apps/desktop/src/shell/delivery/CampaignApprovalPanel.tsx`:

```tsx
import { CalendarClock, CheckCircle2, Sparkles } from "lucide-react";
import { useMemo } from "react";
import type { DeliveryTargetKey } from "../../app/deliveryTargets";
import type { RenderQueueEntry } from "../../app/renderQueue";
import { approvalSummary, type CampaignType } from "../../campaign/manifest";
import { planCampaignFromDelivery } from "../../campaign/planner";
import { campaignStoreSelectors, useCampaignStore } from "../../campaign/store";
import { Button, Card, Inline, Stack, StatusPill } from "../../ui";

type CampaignApprovalPanelProps = {
  sourceAssetId: string;
  selectedTargets: DeliveryTargetKey[];
  renderEntries: RenderQueueEntry[];
};

export function CampaignApprovalPanel({
  sourceAssetId,
  selectedTargets,
  renderEntries,
}: CampaignApprovalPanelProps) {
  const campaigns = useCampaignStore((state) => state.campaigns);
  const upsertCampaign = useCampaignStore((state) => state.upsertCampaign);
  const approveItem = useCampaignStore((state) => state.approveItem);
  const requestChanges = useCampaignStore((state) => state.requestChanges);
  const approveVariant = useCampaignStore((state) => state.approveVariant);
  const active = campaignStoreSelectors.active({ campaigns });
  const readyEntries = renderEntries.filter(
    (entry) =>
      entry.status === "done" &&
      entry.outputPath !== undefined &&
      (entry.kind === "video_master" || entry.kind === "video_reframe"),
  );
  const campaignType: CampaignType = selectedTargets.includes("youtube") ? "podcast" : "shorts";
  const canCreate = readyEntries.length > 0 && selectedTargets.length > 0;
  const summary = useMemo(() => (active ? approvalSummary(active) : undefined), [active]);

  function createLocalCampaign() {
    const campaign = planCampaignFromDelivery({
      campaignType,
      sourceAssetId,
      title: campaignType === "podcast" ? "Podcast rollout campaign" : "Shorts campaign",
      selectedTargets,
      renderEntries,
    });
    upsertCampaign(campaign);
  }

  return (
    <Card padding="md">
      <Stack gap="3">
        <Inline justify="between" align="center">
          <Inline gap="2" align="center">
            <Sparkles size={16} aria-hidden="true" />
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)]">
              Campaign
            </span>
          </Inline>
          {active ? (
            <StatusPill
              family="proposal"
              state={active.approvalState === "approved" ? "accepted" : "pending"}
              size="sm"
              label={active.approvalState === "approved" ? "Approved" : "Draft"}
            />
          ) : null}
        </Inline>

        {active && summary ? (
          <Stack gap="2">
            <Inline gap="2" align="center" className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              <CalendarClock size={14} aria-hidden="true" />
              <span>
                {summary.totalItems} item{summary.totalItems === 1 ? "" : "s"} · {summary.totalVariants} platform variant{summary.totalVariants === 1 ? "" : "s"}
              </span>
            </Inline>
            <Stack gap="1.5">
              {active.items.map((item) => (
                <div
                  key={item.itemId}
                  className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] p-2 text-[var(--text-caption)]"
                >
                  <Inline justify="between" align="center" className="gap-2">
                    <span className="min-w-0 truncate font-medium text-[var(--color-text-primary)]">
                      {item.title}
                    </span>
                    <span className="shrink-0 text-[var(--color-text-muted)]">
                      {item.kind === "long_form" ? "Long form" : "Short"}
                    </span>
                  </Inline>
                  <Inline gap="2" className="mt-2">
                    <button
                      type="button"
                      onClick={() => approveItem(active.campaignId, item.itemId)}
                      className="rounded-[var(--radius-sm)] border border-[rgba(32,201,151,0.45)] bg-[rgba(32,201,151,0.12)] px-2 py-1 text-[var(--color-success)] hover:bg-[rgba(32,201,151,0.18)]"
                    >
                      Approve item
                    </button>
                    <button
                      type="button"
                      onClick={() => requestChanges(active.campaignId, item.itemId)}
                      className="rounded-[var(--radius-sm)] border border-[rgba(245,158,11,0.45)] bg-[rgba(245,158,11,0.1)] px-2 py-1 text-[var(--color-warning)] hover:bg-[rgba(245,158,11,0.16)]"
                    >
                      Needs changes
                    </button>
                  </Inline>
                </div>
              ))}
            </Stack>
            <Inline gap="2" wrap>
              {active.platformVariants.map((variant) => (
                <button
                  key={variant.variantId}
                  type="button"
                  onClick={() => approveVariant(active.campaignId, variant.variantId)}
                  className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] px-2 py-1 text-[var(--text-caption)] text-[var(--color-text-secondary)] hover:border-[var(--color-border-strong)]"
                >
                  {variant.status === "approved" ? <CheckCircle2 size={12} aria-hidden="true" /> : null}
                  {variant.platform}
                </button>
              ))}
            </Inline>
          </Stack>
        ) : (
          <Stack gap="2">
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              Render at least one video target, then create a local campaign for approval.
            </span>
            <Button size="sm" variant="secondary" onClick={createLocalCampaign} disabled={!canCreate}>
              Create campaign
            </Button>
          </Stack>
        )}
      </Stack>
    </Card>
  );
}
```

- [ ] **Step 2: Wire the panel into the delivery surface**

Modify `apps/desktop/src/shell/DeliverySurface.tsx`:

```tsx
import { CampaignApprovalPanel } from "./delivery/CampaignApprovalPanel";
```

Inside the right-column `<Stack gap="3">`, render the panel immediately before `<RenderQueuePanel />`:

```tsx
<CampaignApprovalPanel
  sourceAssetId="active-project"
  selectedTargets={resolvedTargets.filter((target) => target.active).map((target) => target.key)}
  renderEntries={queueEntries}
/>
```

Use the same insertion in the sheet variant if `DeliverySheet` renders its own right-side stack later in the file.

- [ ] **Step 3: Run TypeScript build to catch UI/type errors**

Run:

```bash
cd apps/desktop
pnpm build
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src/shell/DeliverySurface.tsx apps/desktop/src/shell/delivery/CampaignApprovalPanel.tsx
git commit -m "feat(campaign): surface local campaign approval"
```

---

### Task 5: Campaign Test Suite and Final Verification

**Files:**
- Modify: `apps/desktop/package.json`

- [ ] **Step 1: Add aggregate campaign test script**

Modify `apps/desktop/package.json` scripts:

```json
"test:campaign": "pnpm test:campaign-manifest && pnpm test:campaign-store && pnpm test:campaign-planner",
```

- [ ] **Step 2: Run narrow campaign tests**

Run:

```bash
cd apps/desktop
pnpm test:campaign
```

Expected: all three tests pass and print:

```text
campaign-manifest: OK
campaign-store: OK
campaign-planner: OK
```

- [ ] **Step 3: Run existing delivery-adjacent tests**

Run:

```bash
cd apps/desktop
pnpm test:render-queue-upload
pnpm test:upload-metadata
pnpm test:publishing-settings
```

Expected: all pass. These protect the existing render/upload path that the campaign layer now reads.

- [ ] **Step 4: Run frontend build**

Run:

```bash
cd apps/desktop
pnpm build
```

Expected: PASS.

- [ ] **Step 5: Run Rust formatting check if backend files were touched**

Run this only if the implementation expands beyond this plan and touches Rust:

```bash
cargo fmt --all -- --check
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/package.json
git commit -m "test(campaign): add campaign verification script"
```

---

## Self-Review

- Spec coverage: Phase 1 requirements are covered by manifest format, local campaign planning, local approval state, and delivery UI visibility. Server queue, OAuth registry, live adapters, and metrics are intentionally out of scope for this first implementation plan.
- Placeholder scan: this plan avoids undefined future work inside implementation steps. Every new module has concrete test and implementation content.
- Type consistency: `CampaignManifest`, `PublishableItem`, `PlatformVariant`, `CampaignPlatform`, `CampaignType`, and `CampaignApprovalState` are introduced in Task 1 and reused consistently by later tasks.
