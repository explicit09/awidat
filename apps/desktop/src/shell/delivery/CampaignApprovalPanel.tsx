import { CalendarClock, CheckCircle2, Sparkles } from "lucide-react";
import { useMemo, useState } from "react";
import type { RenderQueueEntry } from "../../app/renderQueue";
import { Button, Card, Inline, Stack, StatusPill, cn } from "../../ui";
import { TARGET_META } from "./targetMeta";
import type { DeliveryTargetKey } from "./types";

type CampaignApprovalPanelProps = {
  sourceAssetId: string;
  selectedTargets: DeliveryTargetKey[];
  renderEntries: RenderQueueEntry[];
};

// Session-local fallback until the campaign manifest/store/planner modules land.
type CampaignApprovalState = "draft" | "approved" | "changes_requested";
type CampaignPlatform = Extract<DeliveryTargetKey, "youtube" | "tiktok" | "instagram">;

type LocalCampaignItem = {
  itemId: string;
  title: string;
  kind: "long_form" | "short";
  approvalState: CampaignApprovalState;
};

type LocalCampaignVariant = {
  variantId: string;
  itemId: string;
  platform: CampaignPlatform;
  status: "draft" | "approved";
};

type LocalCampaign = {
  campaignId: string;
  sourceAssetId: string;
  campaignType: "podcast" | "shorts";
  title: string;
  items: LocalCampaignItem[];
  platformVariants: LocalCampaignVariant[];
  approvalState: CampaignApprovalState;
};

const PUBLISHING_TARGETS = new Set<DeliveryTargetKey>([
  "youtube",
  "tiktok",
  "instagram",
]);

export function CampaignApprovalPanel({
  sourceAssetId,
  selectedTargets,
  renderEntries,
}: CampaignApprovalPanelProps) {
  const [campaign, setCampaign] = useState<LocalCampaign | null>(null);
  const readyEntries = useMemo(
    () =>
      renderEntries.filter(
        (entry) =>
          entry.status === "done" &&
          entry.outputPath !== undefined &&
          (entry.kind === "video_master" || entry.kind === "video_reframe"),
      ),
    [renderEntries],
  );
  const publishingTargets = useMemo(
    () =>
      selectedTargets.filter((target): target is CampaignPlatform =>
        PUBLISHING_TARGETS.has(target),
      ),
    [selectedTargets],
  );
  const canCreate = readyEntries.length > 0 && publishingTargets.length > 0;
  const summary = useMemo(() => summarizeCampaign(campaign), [campaign]);

  function createLocalCampaign() {
    if (!canCreate) return;
    setCampaign(createCampaignDraft(sourceAssetId, publishingTargets, readyEntries));
  }

  function approveItem(itemId: string) {
    setCampaign((current) =>
      current
        ? refreshCampaignApproval({
            ...current,
            items: current.items.map((item) =>
              item.itemId === itemId ? { ...item, approvalState: "approved" } : item,
            ),
          })
        : current,
    );
  }

  function requestChanges(itemId: string) {
    setCampaign((current) =>
      current
        ? refreshCampaignApproval({
            ...current,
            items: current.items.map((item) =>
              item.itemId === itemId
                ? { ...item, approvalState: "changes_requested" }
                : item,
            ),
          })
        : current,
    );
  }

  function approveVariant(variantId: string) {
    setCampaign((current) =>
      current
        ? refreshCampaignApproval({
            ...current,
            platformVariants: current.platformVariants.map((variant) =>
              variant.variantId === variantId ? { ...variant, status: "approved" } : variant,
            ),
          })
        : current,
    );
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
          {campaign ? (
            <StatusPill
              family="proposal"
              state={campaign.approvalState === "approved" ? "accepted" : "proposed"}
              size="sm"
              label={campaign.approvalState === "approved" ? "Approved" : "Draft"}
            />
          ) : null}
        </Inline>

        {campaign && summary ? (
          <Stack gap="2">
            <Inline
              gap="2"
              align="center"
              className="text-[var(--text-caption)] text-[var(--color-text-muted)]"
            >
              <CalendarClock size={14} aria-hidden="true" />
              <span>
                {summary.totalItems} item{summary.totalItems === 1 ? "" : "s"} ·{" "}
                {summary.totalVariants} platform variant{summary.totalVariants === 1 ? "" : "s"}
              </span>
            </Inline>
            <Stack gap="2">
              {campaign.items.map((item) => (
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
                      onClick={() => approveItem(item.itemId)}
                      className="rounded-[var(--radius-sm)] border border-[rgba(32,201,151,0.45)] bg-[rgba(32,201,151,0.12)] px-2 py-1 text-[var(--color-success)] hover:bg-[rgba(32,201,151,0.18)]"
                    >
                      Approve item
                    </button>
                    <button
                      type="button"
                      onClick={() => requestChanges(item.itemId)}
                      className="rounded-[var(--radius-sm)] border border-[rgba(245,158,11,0.45)] bg-[rgba(245,158,11,0.1)] px-2 py-1 text-[var(--color-warning)] hover:bg-[rgba(245,158,11,0.16)]"
                    >
                      Needs changes
                    </button>
                  </Inline>
                </div>
              ))}
            </Stack>
            <Inline gap="2" wrap="wrap">
              {campaign.platformVariants.map((variant) => (
                <button
                  key={variant.variantId}
                  type="button"
                  onClick={() => approveVariant(variant.variantId)}
                  className={cn(
                    "inline-flex items-center gap-1 rounded-[var(--radius-sm)] border px-2 py-1 text-[var(--text-caption)] hover:border-[var(--color-border-strong)]",
                    variant.status === "approved"
                      ? "border-[rgba(32,201,151,0.45)] bg-[rgba(32,201,151,0.12)] text-[var(--color-success)]"
                      : "border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] text-[var(--color-text-secondary)]",
                  )}
                >
                  {variant.status === "approved" ? (
                    <CheckCircle2 size={12} aria-hidden="true" />
                  ) : null}
                  {TARGET_META[variant.platform].label}
                </button>
              ))}
            </Inline>
          </Stack>
        ) : (
          <Stack gap="2">
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              Render at least one video target, then create a local campaign for approval.
            </span>
            <Button
              size="sm"
              variant="secondary"
              onClick={createLocalCampaign}
              disabled={!canCreate}
            >
              Create campaign
            </Button>
          </Stack>
        )}
      </Stack>
    </Card>
  );
}

function createCampaignDraft(
  sourceAssetId: string,
  selectedTargets: CampaignPlatform[],
  renderEntries: RenderQueueEntry[],
): LocalCampaign {
  const campaignType = selectedTargets.includes("youtube") ? "podcast" : "shorts";
  const items = renderEntries.map((entry): LocalCampaignItem => ({
    itemId: `${entry.id}:item`,
    title: entry.label,
    kind: entry.kind === "video_master" ? "long_form" : "short",
    approvalState: "draft",
  }));
  return {
    campaignId: `${sourceAssetId}:local-campaign`,
    sourceAssetId,
    campaignType,
    title: campaignType === "podcast" ? "Podcast rollout campaign" : "Shorts campaign",
    items,
    platformVariants: items.flatMap((item) =>
      selectedTargets.map((platform): LocalCampaignVariant => ({
        variantId: `${item.itemId}:${platform}`,
        itemId: item.itemId,
        platform,
        status: "draft",
      })),
    ),
    approvalState: "draft",
  };
}

function refreshCampaignApproval(campaign: LocalCampaign): LocalCampaign {
  const allItemsApproved = campaign.items.every((item) => item.approvalState === "approved");
  const allVariantsApproved = campaign.platformVariants.every(
    (variant) => variant.status === "approved",
  );
  const anyChangesRequested = campaign.items.some(
    (item) => item.approvalState === "changes_requested",
  );
  return {
    ...campaign,
    approvalState: anyChangesRequested
      ? "changes_requested"
      : allItemsApproved && allVariantsApproved
        ? "approved"
        : "draft",
  };
}

function summarizeCampaign(campaign: LocalCampaign | null) {
  if (!campaign) return null;
  return {
    totalItems: campaign.items.length,
    totalVariants: campaign.platformVariants.length,
  };
}
