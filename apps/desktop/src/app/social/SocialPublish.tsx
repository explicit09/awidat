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
  reasonCopy,
  type AccountSummary,
  type PublishJob,
} from "./socialModel";
import { useRenderQueueStore } from "../renderQueue";
import { Button, Card, Inline, Stack } from "../../ui";

type UploadPrivacy = "private" | "unlisted" | "public";

type ValidatedTarget = {
  validation_state?: string;
  validationState?: string;
};

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

function randomId(prefix: string): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${prefix}-${hex}`;
}

function validationState(target: ValidatedTarget): string {
  return target.validation_state ?? target.validationState ?? "unknown";
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

  const [accountId, setAccountId] = useState<string>("");
  const [entryId, setEntryId] = useState<string>("");
  const [privacy, setPrivacy] = useState<UploadPrivacy>("private");
  const [title, setTitle] = useState("");
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
    if (!accountId && accounts[0]) setAccountId(accounts[0].id);
  }, [accounts, accountId]);
  useEffect(() => {
    if (!entryId && publishable[0]) setEntryId(publishable[0].id);
  }, [publishable, entryId]);

  const selectedAccount = accounts.find((a) => a.id === accountId);
  const selectedEntry = publishable.find((e) => e.id === entryId);
  const eligible = selectedAccount?.capabilities.uploadVideo === true;

  /** The full publish chain for one clip. */
  const publish = useCallback(async () => {
    if (!selectedAccount || !selectedEntry?.outputPath) return;
    setError(null);
    setBusy("Scheduling…");
    try {
      // 1. bind the (synthetic single-clip) target to the account.
      const targetId = randomId("target");
      const campaignId = `adhoc-${selectedEntry.id}`;
      const variantId = `clip-${selectedEntry.id}`;
      await invoke("social_bind_target", {
        args: {
          targetId,
          campaignId,
          variantId,
          connectedAccountId: selectedAccount.id,
          platformFields: { privacy, title: title || selectedEntry.id },
          scheduledFor,
          now: nowSeconds(),
        },
      });

      // 2. validate (eligibility + scheduled time).
      const validated = await invoke<ValidatedTarget>(
        "social_validate_target",
        { targetId, now: nowSeconds() },
      );
      const validatedState = validationState(validated);
      if (validatedState !== "valid") {
        setError(`Not valid to schedule: ${reasonCopy(validatedState)}`);
        setBusy(null);
        return;
      }

      // 3. schedule → creates the publish job.
      const jobId = randomId("job");
      const job = await invoke<PublishJob>("social_schedule_target", {
        args: {
          targetId,
          jobId,
          artifactRef: "",
          now: nowSeconds(),
        },
      });

      // 4. upload the rendered file to the job (server mints a signed URL,
      //    streams the bytes, records the artifact). The worker fires after.
      setBusy("Uploading render…");
      await invoke("social_upload_artifact", {
        jobId: job.id,
        filePath: selectedEntry.outputPath,
      });

      setJobIds((prev) => (prev.includes(job.id) ? prev : [...prev, job.id]));
      setBusy(null);
    } catch (e) {
      setError(String(e));
      setBusy(null);
    }
  }, [selectedAccount, selectedEntry, privacy, title, scheduledFor]);

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
              <Field label="Account">
                <select
                  className="social-select"
                  value={accountId}
                  onChange={(e) => setAccountId(e.target.value)}
                >
                  {accounts.map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.displayName} ({a.provider})
                    </option>
                  ))}
                </select>
              </Field>
              {!eligible && selectedAccount && (
                <p className="text-[var(--text-caption)] text-[var(--color-text-danger,#f87171)]">
                  This account can&apos;t upload video yet (reconnect or check
                  permissions).
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

              <Field label="Title">
                <input
                  className="social-input"
                  value={title}
                  placeholder={selectedEntry?.id ?? "Untitled"}
                  onChange={(e) => setTitle(e.target.value)}
                />
              </Field>

              <Inline gap="3" className="flex-wrap">
                <Field label="Privacy">
                  <select
                    className="social-select"
                    value={privacy}
                    onChange={(e) =>
                      setPrivacy(e.target.value as UploadPrivacy)
                    }
                  >
                    <option value="private">Private</option>
                    <option value="unlisted">Unlisted</option>
                    <option value="public">Public</option>
                  </select>
                </Field>
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
                  {busy ?? "Schedule + upload"}
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
