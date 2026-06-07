// Publishing page: the end-to-end "post a rendered video to a connected
// account" flow, wired to the NEW server path (not the legacy desktop upload):
//
//   social_bind_target → social_validate_target → social_schedule_target
//     → social_upload_artifact → (server worker fires it)
//
// Reuses <SocialAccounts/> for connect/list and <SocialJobs/> for live status.
// This is the page the campaign flow also routes into (CampaignApprovalPanel
// hands it a campaignId/variantId/artifact), but it stands alone so a single
// rendered clip can be published without a full campaign.

import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { SocialAccounts } from "./SocialAccounts";
import { SocialJobs } from "./SocialJobs";
import {
  type AccountSummary,
} from "./socialModel";
import { useRenderQueueStore } from "../renderQueue";
import {
  mergeSchedulerPublishResults,
  publishSchedulerPostToAccounts,
  schedulerMetadataControlProvider,
  schedulerMetadataFieldConfig,
  type SchedulerPublishAccount,
} from "../scheduler/schedulerPublish";
import type { UploadVisibility } from "../../state/uploadMetadata";
import { Button, Card, Inline, Stack } from "../../ui";

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

/** Format a unix-seconds value for a datetime-local input. */
function toLocalInput(epoch: number): string {
  const d = new Date(epoch * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(
    d.getHours(),
  )}:${pad(d.getMinutes())}`;
}

export function SocialPublish() {
  const [accounts, setAccounts] = useState<AccountSummary[]>([]);
  const renderEntries = useRenderQueueStore((s) => s.entries);

  // Only finished renders with an output file can be published.
  const publishable = useMemo(
    () => renderEntries.filter((e) => e.status === "done" && e.outputPath),
    [renderEntries],
  );

  const [accountIds, setAccountIds] = useState<string[]>([]);
  const [entryId, setEntryId] = useState<string>("");
  const [privacy, setPrivacy] = useState<UploadVisibility>("private");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [tagsInput, setTagsInput] = useState("");
  const [thumbnailPath, setThumbnailPath] = useState("");
  const [scheduledFor, setScheduledFor] = useState<number>(nowSeconds() + 600);

  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [jobIds, setJobIds] = useState<string[]>([]);

  const refreshAccounts = useCallback(async () => {
    try {
      setAccounts(await invoke<AccountSummary[]>("social_accounts"));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refreshAccounts();
  }, [refreshAccounts]);

  // Default the selects once data arrives.
  useEffect(() => {
    setAccountIds((current) => {
      const uploadCapable = accounts.filter((account) => account.capabilities.uploadVideo);
      const available = new Set(uploadCapable.map((account) => account.id));
      const kept = current.filter((id) => available.has(id));
      if (kept.length > 0) return kept;
      return uploadCapable[0] ? [uploadCapable[0].id] : [];
    });
  }, [accounts]);
  useEffect(() => {
    if (!entryId && publishable[0]) setEntryId(publishable[0].id);
  }, [publishable, entryId]);

  const selectedAccounts = accounts.filter((account) =>
    accountIds.includes(account.id),
  );
  const selectedEntry = publishable.find((e) => e.id === entryId);
  const eligible = selectedAccounts.length > 0;
  const selectedProvider = schedulerMetadataControlProvider(selectedAccounts);
  const fieldConfig = schedulerMetadataFieldConfig(selectedProvider);

  useEffect(() => {
    if (!selectedProvider || !selectedEntry) return;
    const metadata = selectedEntry.uploadMetadata?.[selectedProvider];
    setPrivacy(metadata?.visibility ?? "private");
    setTitle(metadata?.title ?? "");
    setDescription(metadata?.description ?? "");
    setTagsInput(metadata?.tags.join(", ") ?? "");
    setThumbnailPath(metadata?.thumbnailPath ?? "");
    if (metadata?.scheduledAt) setScheduledFor(metadata.scheduledAt);
  }, [selectedProvider, selectedEntry?.id]);

  /** The full publish chain for one clip. */
  const publish = useCallback(async () => {
    if (selectedAccounts.length === 0 || !selectedEntry?.outputPath) return;
    setError(null);
    setBusy("Scheduling…");
    try {
      const results = await publishSchedulerPostToAccounts({
        entry: selectedEntry,
        accounts: selectedAccounts as SchedulerPublishAccount[],
        title: title || selectedEntry.id,
        description,
        tagsInput,
        thumbnailPath,
        privacy,
        scheduledFor,
        invoke,
        createdBy: "desktop-manual-publish",
        campaignIdPrefix: "adhoc",
      });
      const store = useRenderQueueStore.getState();
      const latest = store.entries.find((entry) => entry.id === selectedEntry.id) ?? selectedEntry;
      const patch = mergeSchedulerPublishResults(latest, results);
      store.setUploadTargets(selectedEntry.id, patch.uploadTargets);
      store.setUploadMetadata(selectedEntry.id, patch.uploadMetadata);
      store.setUploadStates(selectedEntry.id, patch.uploadStates, patch.publishedUrls);

      setJobIds((prev) => [
        ...prev,
        ...results.map((result) => result.jobId).filter((jobId) => !prev.includes(jobId)),
      ]);
      setBusy(null);
    } catch (e) {
      setError(String(e));
      setBusy(null);
    }
  }, [
    selectedAccounts,
    selectedEntry,
    privacy,
    title,
    description,
    tagsInput,
    thumbnailPath,
    scheduledFor,
  ]);

  return (
    <Stack gap="4" className="p-4 max-w-[720px]">
      <header>
        <h2 className="text-[var(--text-h3)] text-[var(--color-text-primary)] m-0">
          Publish
        </h2>
        <p className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)] mt-1">
          Post a finished render to a connected account. Scheduled posts fire
          server-side — your machine doesn&apos;t need to stay open.
        </p>
      </header>

      <SocialAccounts />

      <Card>
        <Stack gap="3" className="p-3">
          <h3 className="text-[var(--text-label)] uppercase tracking-wide text-[var(--color-text-muted)] m-0">
            Schedule a post
          </h3>

          {accounts.length === 0 && (
            <p className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              Connect an account above first.
            </p>
          )}
          {publishable.length === 0 && (
            <p className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              No finished renders yet. Render a clip in Delivery, then return
              here.
            </p>
          )}

          {accounts.length > 0 && publishable.length > 0 && (
            <>
              <Field label="Destinations">
                <div className="grid gap-1">
                  {accounts.map((account) => {
                    const checked = accountIds.includes(account.id);
                    return (
                      <label
                        key={account.id}
                        className="flex min-h-[32px] items-center gap-2 rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-2 py-1.5 text-[var(--text-body-sm)] text-[var(--color-text-secondary)]"
                      >
                        <input
                          type="checkbox"
                          checked={checked}
                          disabled={!account.capabilities.uploadVideo}
                          onChange={(event) => {
                            const nextChecked = event.currentTarget.checked;
                            setAccountIds((current) =>
                              nextChecked
                                ? [...new Set([...current, account.id])]
                                : current.filter((id) => id !== account.id),
                            );
                          }}
                        />
                        <span className="min-w-0 truncate">
                          {account.displayName} ({account.provider})
                        </span>
                      </label>
                    );
                  })}
                </div>
              </Field>
              {!eligible && (
                <p className="text-[var(--text-caption)] text-[var(--color-text-danger,#f87171)]">
                  Select at least one upload-capable account.
                </p>
              )}

              <Field label="Render">
                <select
                  className="social-select"
                  value={entryId}
                  onChange={(e) => setEntryId(e.target.value)}
                >
                  {publishable.map((e) => (
                    <option key={e.id} value={e.id}>
                      {e.outputPath?.split("/").pop() ?? e.id}
                    </option>
                  ))}
                </select>
              </Field>

              {fieldConfig.showTitle ? (
                <Field label={fieldConfig.titleLabel}>
                  <input
                    className="social-input"
                    value={title}
                    placeholder={selectedEntry?.id ?? "Untitled"}
                    onChange={(e) => setTitle(e.target.value)}
                  />
                </Field>
              ) : null}

              {fieldConfig.showDescription ? (
                <Field label={fieldConfig.descriptionLabel}>
                  <textarea
                    className="social-input min-h-[76px]"
                    value={description}
                    placeholder={fieldConfig.descriptionPlaceholder}
                    onChange={(e) => setDescription(e.target.value)}
                  />
                </Field>
              ) : null}

              <Inline gap="3" className="flex-wrap">
                {fieldConfig.showTags ? (
                  <Field label="Tags">
                    <input
                      className="social-input"
                      value={tagsInput}
                      placeholder="tag, tag"
                      onChange={(e) => setTagsInput(e.target.value)}
                    />
                  </Field>
                ) : null}
                {fieldConfig.showThumbnail ? (
                  <Field label="Thumbnail path">
                    <input
                      className="social-input"
                      value={thumbnailPath}
                      placeholder="/path/to/thumbnail.jpg"
                      onChange={(e) => setThumbnailPath(e.target.value)}
                    />
                  </Field>
                ) : null}
              </Inline>

              <Inline gap="3" className="flex-wrap">
                {fieldConfig.visibilityOptions ? (
                  <Field label="Privacy">
                    <select
                      className="social-select"
                      value={privacy}
                      onChange={(e) =>
                        setPrivacy(e.target.value as UploadVisibility)
                      }
                    >
                      {fieldConfig.visibilityOptions.map((option) => (
                        <option key={option.value} value={option.value}>
                          {option.label}
                        </option>
                      ))}
                    </select>
                  </Field>
                ) : (
                  <Field label="Visibility">
                    <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
                      Always public on {selectedProvider ?? "this provider"}.
                    </span>
                  </Field>
                )}
                <Field label="When">
                  <input
                    className="social-input"
                    type="datetime-local"
                    value={toLocalInput(scheduledFor)}
                    onChange={(e) =>
                      setScheduledFor(
                        Math.floor(new Date(e.target.value).getTime() / 1000),
                      )
                    }
                  />
                </Field>
              </Inline>

              <Inline gap="2" align="center">
                <Button
                  variant="primary"
                  disabled={!eligible || busy !== null}
                  onClick={() => void publish()}
                >
                  {busy ?? `Schedule + upload ${selectedAccounts.length}`}
                </Button>
                {error && (
                  <span
                    role="alert"
                    className="text-[var(--text-caption)] text-[var(--color-text-danger,#f87171)]"
                  >
                    {error}
                  </span>
                )}
              </Inline>
            </>
          )}
        </Stack>
      </Card>

      {jobIds.length > 0 && <SocialJobs jobIds={jobIds} />}
    </Stack>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">
        {label}
      </span>
      {children}
    </label>
  );
}
