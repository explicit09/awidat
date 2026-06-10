// Server-backed connected-accounts surface: connect a provider (opens the
// server's OAuth URL in the browser), list connected accounts with status +
// eligibility, and disconnect. Talks to the `social_*` Tauri commands; all
// derivation lives in `socialModel.ts`.
//
// House style: muted glass surfaces, hairline borders, dot + label for status.

import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

import {
  accountStatusLabel,
  canViewAccountAudit,
  canReconnect,
  eligibilitySummary,
  type AccountSummary,
  type Provider,
} from "./socialModel";
import { startConnect } from "./socialActions";
import { SocialAudit } from "./SocialAudit";
import { VISIBLE_PROVIDERS, providerDisplayName } from "../publishingSettingsModel";

const SOCIAL_PROVIDERS = VISIBLE_PROVIDERS.map((provider) => provider.key);

// status → dot color token. Connected = success; needs-action = warning; the
// rest = muted. Keeps the "dot + label" language, no full-color pills.
function statusDotColor(status: string): string {
  switch (status) {
    case "connected":
      return "var(--color-text-success, #4ade80)";
    case "needs_reauth":
    case "missing_scope":
      return "var(--color-text-warning, #fbbf24)";
    default:
      return "var(--color-text-muted, #9ca3af)";
  }
}

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

export function SocialAccounts() {
  const [accounts, setAccounts] = useState<AccountSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<Provider | null>(null);
  const [polling, setPolling] = useState(false);
  const [auditAccountId, setAuditAccountId] = useState<string | null>(null);
  const pollTimers = useRef<number[]>([]);

  const refresh = useCallback(async () => {
    try {
      setAccounts(await invoke<AccountSummary[]>("social_accounts"));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
    // Clear any pending poll timers on unmount.
    return () => {
      pollTimers.current.forEach((t) => window.clearTimeout(t));
      pollTimers.current = [];
    };
  }, [refresh]);

  const connect = useCallback(
    async (provider: Provider) => {
      setBusy(provider);
      try {
        // Server mints connection id + CSRF state and owns the OAuth client
        // config; the desktop only opens the returned URL. The OAuth callback
        // lands server-side and can take several seconds (consent + code
        // exchange + channel resolution), so we poll a handful of times after
        // opening the browser instead of relying on a single refresh.
        await startConnect(invoke, openUrl, provider);
        setError(null);
        setPolling(true);
        const before = accounts.length;
        pollTimers.current.forEach((t) => window.clearTimeout(t));
        pollTimers.current = [2000, 5000, 9000, 14000, 20000].map((ms) =>
          window.setTimeout(async () => {
            await refresh();
            // Stop showing the spinner once an account actually appears.
            setAccounts((cur) => {
              if (cur.length > before) setPolling(false);
              return cur;
            });
            if (ms >= 20000) setPolling(false);
          }, ms),
        );
      } catch (e) {
        setError(String(e));
        setPolling(false);
      } finally {
        setBusy(null);
      }
    },
    [refresh, accounts.length],
  );

  const disconnect = useCallback(
    async (id: string) => {
      try {
        await invoke("social_disconnect_account", {
          accountId: id,
          now: nowSeconds(),
        });
        await refresh();
      } catch (e) {
        setError(String(e));
      }
    },
    [refresh],
  );

  return (
    <div className="grid gap-3">
      <div className="flex items-start justify-between gap-3">
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
          Connect an account to publish on your behalf. You sign in once; the
          server keeps it connected.
        </span>
        <button
          type="button"
          className="glass-ghost rounded-lg px-3 py-1.5 text-[12px] font-semibold"
          onClick={() => void refresh()}
        >
          Refresh
        </button>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        {SOCIAL_PROVIDERS.map((p) => (
          <button
            key={p}
            type="button"
            className="glass-cta rounded-lg px-3 py-1.5 text-[12px] font-semibold disabled:pointer-events-none disabled:opacity-45"
            disabled={busy !== null}
            onClick={() => void connect(p)}
          >
            {busy === p ? "Opening…" : `Connect ${providerDisplayName(p)}`}
          </button>
        ))}
      </div>

      {error && (
        <span
          role="alert"
          className="text-[var(--text-caption)] text-[var(--color-text-danger,#f87171)]"
        >
          {error}
        </span>
      )}

      {polling && accounts.length === 0 && (
        <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
          Waiting for the browser sign-in to finish…
        </span>
      )}

      {/* Account list */}
      <div className="grid gap-2">
        {accounts.map((a) => {
          const auditOpen = auditAccountId === a.id;
          return (
            <div
              key={a.id}
              className="glass-content grid gap-2 rounded-xl px-3 py-2"
            >
              <div className="flex items-center justify-between gap-3">
                <div className="flex min-w-0 items-center gap-2">
                  <span
                    aria-hidden="true"
                    className="inline-block h-2 w-2 rounded-full shrink-0"
                    style={{ backgroundColor: statusDotColor(a.status) }}
                  />
                  <div className="min-w-0">
                    <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)] truncate">
                      {a.displayName || a.providerAccountId}
                    </span>
                    <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
                      {providerDisplayName(a.provider)} · {accountStatusLabel(a.status)}
                      {!a.eligibility.eligible
                        ? ` · ${eligibilitySummary(a.eligibility)}`
                        : ""}
                    </span>
                  </div>
                </div>
                <div className="flex shrink-0 items-center gap-1.5">
                  {canViewAccountAudit(a.status) && (
                    <button
                      type="button"
                      className="glass-ghost rounded-lg px-2.5 py-1 text-[11px] font-semibold"
                      onClick={() =>
                        setAuditAccountId((current) =>
                          current === a.id ? null : a.id,
                        )
                      }
                    >
                      {auditOpen ? "Hide audit" : "View audit"}
                    </button>
                  )}
                  {canReconnect(a.status) && (
                    <button
                      type="button"
                      className="glass-cta rounded-lg px-2.5 py-1 text-[11px] font-semibold disabled:pointer-events-none disabled:opacity-45"
                      disabled={busy !== null}
                      onClick={() => void connect(a.provider)}
                    >
                      {busy === a.provider ? "Opening…" : "Reconnect"}
                    </button>
                  )}
                  <button
                    type="button"
                    className="glass-ghost rounded-lg px-2.5 py-1 text-[11px] font-semibold"
                    onClick={() => void disconnect(a.id)}
                  >
                    Disconnect
                  </button>
                </div>
              </div>
              {auditOpen && <SocialAudit accountId={a.id} />}
            </div>
          );
        })}
        {accounts.length === 0 && !error && !polling && (
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
            No accounts connected yet.
          </span>
        )}
      </div>
    </div>
  );
}
