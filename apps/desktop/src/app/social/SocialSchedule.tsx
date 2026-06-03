// Schedule surface: bind a campaign variant to a connected account, validate
// it (surfacing reason codes), and schedule a publish job. Talks to the
// `social_*` Tauri commands; reason-code copy lives in `socialModel.ts`.

import { useCallback, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  reasonCopy,
  type AccountSummary,
  type PublishJob,
} from "./socialModel";

type ValidationState = "pending" | "valid" | "invalid" | "requires_action";

type CampaignVariantTarget = {
  id: string;
  campaignId: string;
  variantId: string;
  connectedAccountId: string;
  provider: string;
  platformFields: unknown;
  scheduledFor: number;
  validationState: ValidationState;
  createdAt: number;
  updatedAt: number;
};

type TargetValidationReport = {
  state: ValidationState;
  reasons: string[];
};

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

function randomId(prefix: string): string {
  return `${prefix}-${nowSeconds()}-${Math.floor(Math.random() * 1_000_000)}`;
}

export function SocialSchedule({
  accounts,
  campaignId,
  variantId,
  artifactRef,
}: {
  accounts: AccountSummary[];
  campaignId: string;
  variantId: string;
  artifactRef: string;
}) {
  const [accountId, setAccountId] = useState<string>(accounts[0]?.id ?? "");
  const [scheduledFor, setScheduledFor] = useState<number>(nowSeconds() + 3600);
  const [report, setReport] = useState<TargetValidationReport | null>(null);
  const [scheduled, setScheduled] = useState<PublishJob | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [targetId, setTargetId] = useState<string | null>(null);

  const bindAndValidate = useCallback(async () => {
    setError(null);
    setScheduled(null);
    try {
      const id = randomId("target");
      await invoke<CampaignVariantTarget>("social_bind_target", {
        args: {
          targetId: id,
          campaignId,
          variantId,
          connectedAccountId: accountId,
          platformFields: { privacy: "private" },
          scheduledFor,
          now: nowSeconds(),
        },
      });
      setTargetId(id);
      const validated = await invoke<CampaignVariantTarget>(
        "social_validate_target",
        { targetId: id, now: nowSeconds() },
      );
      // The command returns the updated target; reasons are derived from its
      // state. A non-valid state means scheduling is blocked.
      setReport({
        state: validated.validationState,
        reasons:
          validated.validationState === "valid"
            ? []
            : ["see account eligibility / scheduled time"],
      });
    } catch (e) {
      setError(String(e));
    }
  }, [accountId, campaignId, variantId, scheduledFor]);

  const schedule = useCallback(async () => {
    if (!targetId) return;
    try {
      const job = await invoke<PublishJob>("social_schedule_target", {
        args: {
          targetId,
          jobId: randomId("job"),
          artifactRef,
          now: nowSeconds(),
        },
      });
      setScheduled(job);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [targetId, artifactRef]);

  const canSchedule = report?.state === "valid" && !!targetId;

  return (
    <section className="social-schedule">
      <header className="social-schedule__header">Schedule publish</header>

      {error && (
        <p className="social-schedule__error" role="alert">
          {error}
        </p>
      )}

      <label className="social-schedule__field">
        Account
        <select
          value={accountId}
          onChange={(e) => setAccountId(e.target.value)}
        >
          {accounts.map((a) => (
            <option key={a.id} value={a.id}>
              {a.displayName}
            </option>
          ))}
        </select>
      </label>

      <label className="social-schedule__field">
        Scheduled for (unix seconds)
        <input
          type="number"
          value={scheduledFor}
          onChange={(e) => setScheduledFor(Number(e.target.value))}
        />
      </label>

      <button type="button" onClick={() => void bindAndValidate()}>
        Validate
      </button>

      {report && (
        <div className="social-schedule__report" data-state={report.state}>
          <span>Validation: {report.state}</span>
          {report.reasons.length > 0 && (
            <ul>
              {report.reasons.map((r) => (
                <li key={r}>{reasonCopy(r)}</li>
              ))}
            </ul>
          )}
        </div>
      )}

      <button type="button" disabled={!canSchedule} onClick={() => void schedule()}>
        Schedule
      </button>

      {scheduled && (
        <p className="social-schedule__ok">
          Scheduled job {scheduled.id} ({scheduled.status}).
        </p>
      )}
    </section>
  );
}
