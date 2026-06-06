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
  campaign_id?: string;
  campaignId?: string;
  variant_id?: string;
  variantId?: string;
  connected_account_id?: string;
  connectedAccountId?: string;
  provider: string;
  platform_fields?: unknown;
  platformFields?: unknown;
  scheduled_for?: number;
  scheduledFor?: number;
  validation_state?: ValidationState;
  validationState?: ValidationState;
  validation_reasons?: string[];
  validationReasons?: string[];
  created_at?: number;
  createdAt?: number;
  updated_at?: number;
  updatedAt?: number;
};

type TargetValidationReport = {
  state: ValidationState;
  reasons: string[];
};

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

function randomId(prefix: string): string {
  // Target/job ids aren't security tokens, but use a secure RNG for collision
  // resistance and consistency with the OAuth state generation.
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${prefix}-${hex}`;
}

function targetValidationState(
  target: CampaignVariantTarget,
): ValidationState {
  return target.validation_state ?? target.validationState ?? "invalid";
}

function targetValidationReasons(target: CampaignVariantTarget): string[] {
  return target.validation_reasons ?? target.validationReasons ?? [];
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
          platformFields: { privacy: "private", title: variantId },
          scheduledFor,
          now: nowSeconds(),
        },
      });
      setTargetId(id);
      const validated = await invoke<CampaignVariantTarget>(
        "social_validate_target",
        { targetId: id, now: nowSeconds() },
      );
      const state = targetValidationState(validated);
      // The command returns the updated target plus server validation reasons.
      // A non-valid state means scheduling is blocked.
      setReport({
        state,
        reasons: state === "valid" ? [] : targetValidationReasons(validated),
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
