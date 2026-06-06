// Server-backed connected-accounts surface: connect a provider (opens the
// server's OAuth URL in the browser), list connected accounts with status +
// eligibility, and disconnect. Talks to the `social_*` Tauri commands; all
// derivation lives in `socialModel.ts`.
//
// House style: muted surfaces, hairline borders, dot + label for status (no
// colored pills), using the shared `ui` primitives + Tailwind tokens.

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
import { PROVIDERS, providerDisplayName } from "../publishingSettingsModel";
import { Button, Inline, Stack } from "../../ui";

const SOCIAL_PROVIDERS = PROVIDERS.map((provider) => provider.key);

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
    <Stack gap="2">
      <Inline justify="between" align="center" gap="2">
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
          Connect an account to publish on your behalf. You sign in once; the
          server keeps it connected.
        </span>
        <Button variant="ghost" size="sm" onClick={() => void refresh()}>
          Refresh
        </Button>
      </Inline>

      {/* Connect buttons */}
      <Inline gap="2" align="center" className="flex-wrap">
        {SOCIAL_PROVIDERS.map((p) => (
          <Button
            key={p}
            variant="secondary"
            size="sm"
            disabled={busy !== null}
            onClick={() => void connect(p)}
          >
            {busy === p ? "Opening…" : `Connect ${providerDisplayName(p)}`}
          </Button>
        ))}
      </Inline>

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
      <Stack gap="1">
        {accounts.map((a) => {
          const auditOpen = auditAccountId === a.id;
          return (
            <Stack
              key={a.id}
              gap="2"
              className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] px-3 py-2"
            >
              <Inline justify="between" align="center" gap="2">
                <Inline gap="2" align="center" className="min-w-0">
                  <span
                    aria-hidden="true"
                    className="inline-block h-2 w-2 rounded-full shrink-0"
                    style={{ backgroundColor: statusDotColor(a.status) }}
                  />
                  <Stack gap="0" className="min-w-0">
                    <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)] truncate">
                      {a.displayName || a.providerAccountId}
                    </span>
                    <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
                      {providerDisplayName(a.provider)} · {accountStatusLabel(a.status)}
                      {!a.eligibility.eligible
                        ? ` · ${eligibilitySummary(a.eligibility)}`
                        : ""}
                    </span>
                  </Stack>
                </Inline>
                <Inline gap="1" align="center">
                  {canViewAccountAudit(a.status) && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() =>
                        setAuditAccountId((current) =>
                          current === a.id ? null : a.id,
                        )
                      }
                    >
                      {auditOpen ? "Hide audit" : "View audit"}
                    </Button>
                  )}
                  {canReconnect(a.status) && (
                    <Button
                      variant="secondary"
                      size="sm"
                      disabled={busy !== null}
                      onClick={() => void connect(a.provider)}
                    >
                      {busy === a.provider ? "Opening…" : "Reconnect"}
                    </Button>
                  )}
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => void disconnect(a.id)}
                  >
                    Disconnect
                  </Button>
                </Inline>
              </Inline>
              {auditOpen && <SocialAudit accountId={a.id} />}
            </Stack>
          );
        })}
        {accounts.length === 0 && !error && !polling && (
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
            No accounts connected yet.
          </span>
        )}
      </Stack>
    </Stack>
  );
}
