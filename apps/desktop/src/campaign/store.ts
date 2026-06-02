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
