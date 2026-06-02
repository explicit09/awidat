import { CalendarClock, CheckCircle2, Sparkles } from "lucide-react";
import { useMemo } from "react";
import type { DeliveryTargetKey } from "../../app/deliveryTargets";
import type { RenderQueueEntry } from "../../app/renderQueue";
import { approvalSummary, type CampaignType } from "../../campaign/manifest";
import { planCampaignFromDelivery } from "../../campaign/planner";
import { useCampaignStore } from "../../campaign/store";
import { Button, Card, Inline, Stack, StatusPill, cn } from "../../ui";
import { TARGET_META } from "./targetMeta";

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
  const active = useMemo(
    () =>
      campaigns
        .filter((campaign) => campaign.sourceAssetId === sourceAssetId)
        .sort((a, b) => b.updatedAt - a.updatedAt)[0],
    [campaigns, sourceAssetId],
  );
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
  const campaignType: CampaignType = selectedTargets.includes("youtube")
    ? "podcast"
    : "shorts";
  const canCreate = readyEntries.length > 0 && selectedTargets.length > 0;
  const summary = useMemo(
    () => (active ? approvalSummary(active) : undefined),
    [active],
  );

  function createLocalCampaign() {
    if (!canCreate) return;
    const campaign = planCampaignFromDelivery({
      campaignType,
      sourceAssetId,
      title:
        campaignType === "podcast"
          ? "Podcast rollout campaign"
          : "Shorts campaign",
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
              state={active.approvalState === "approved" ? "accepted" : "proposed"}
              size="sm"
              label={active.approvalState === "approved" ? "Approved" : "Draft"}
            />
          ) : null}
        </Inline>

        {active && summary ? (
          <Stack gap="2">
            <Inline
              gap="2"
              align="center"
              className="text-[var(--text-caption)] text-[var(--color-text-muted)]"
            >
              <CalendarClock size={14} aria-hidden="true" />
              <span>
                {summary.totalItems} item{summary.totalItems === 1 ? "" : "s"} ·{" "}
                {summary.totalVariants} platform variant
                {summary.totalVariants === 1 ? "" : "s"}
              </span>
            </Inline>
            <Stack gap="2">
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
                      onClick={() =>
                        requestChanges(active.campaignId, item.itemId)
                      }
                      className="rounded-[var(--radius-sm)] border border-[rgba(245,158,11,0.45)] bg-[rgba(245,158,11,0.1)] px-2 py-1 text-[var(--color-warning)] hover:bg-[rgba(245,158,11,0.16)]"
                    >
                      Needs changes
                    </button>
                  </Inline>
                </div>
              ))}
            </Stack>
            <Inline gap="2" wrap="wrap">
              {active.platformVariants.map((variant) => (
                <button
                  key={variant.variantId}
                  type="button"
                  onClick={() =>
                    approveVariant(active.campaignId, variant.variantId)
                  }
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
              Render at least one video target, then create a local campaign for
              approval.
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
