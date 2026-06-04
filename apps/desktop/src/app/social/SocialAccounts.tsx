// Server-backed social accounts surface: list connected accounts with status
// + eligibility, connect a new one (opens the provider authorize URL), and
// disconnect. Talks to the `social_*` Tauri commands; all derivation lives in
// `socialModel.ts`. House style: dot + label for status, no colored pills.

import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

import {
  accountStatusLabel,
  eligibilitySummary,
  type AccountSummary,
  type Provider,
} from "./socialModel";
import { startConnect } from "./socialActions";

const PROVIDER_LABELS: Record<Provider, string> = {
  youtube: "YouTube",
  tiktok: "TikTok",
  instagram: "Instagram",
};

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

export function SocialAccounts() {
  const [accounts, setAccounts] = useState<AccountSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

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
  }, [refresh]);

  const connect = useCallback(
    async (provider: Provider) => {
      setBusy(true);
      try {
        // The server mints connection id + CSRF state and owns the OAuth client
        // config; the desktop sends only the provider, opens the returned URL,
        // and re-polls accounts after the (server-side) callback completes.
        await startConnect(invoke, openUrl, provider);
        setError(null);
        // The callback lands server-side, so poll for the new account. A short
        // delayed re-refresh catches the common "just authorized" case; the
        // manual Refresh button covers slower consent flows.
        await refresh();
        window.setTimeout(() => void refresh(), 4000);
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(false);
      }
    },
    [refresh],
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
    <section className="social-accounts">
      <header className="social-accounts__header">Connected accounts</header>

      {error && (
        <p className="social-accounts__error" role="alert">
          {error}
        </p>
      )}

      <div className="social-accounts__connect">
        {(Object.keys(PROVIDER_LABELS) as Provider[]).map((p) => (
          <button
            key={p}
            type="button"
            disabled={busy}
            onClick={() => void connect(p)}
          >
            Connect {PROVIDER_LABELS[p]}
          </button>
        ))}
        <button type="button" onClick={() => void refresh()}>
          Refresh accounts
        </button>
      </div>

      <ul className="social-accounts__list">
        {accounts.map((a) => (
          <li key={a.id} className="social-accounts__row">
            <span
              className="status-dot"
              data-status={a.status}
              aria-hidden="true"
            />
            <span className="social-accounts__name">{a.displayName}</span>
            <span className="social-accounts__provider">
              {PROVIDER_LABELS[a.provider]}
            </span>
            <span className="social-accounts__status">
              {accountStatusLabel(a.status)}
            </span>
            <span className="social-accounts__eligibility">
              {eligibilitySummary(a.eligibility)}
            </span>
            <button type="button" onClick={() => void disconnect(a.id)}>
              Disconnect
            </button>
          </li>
        ))}
        {accounts.length === 0 && !error && (
          <li className="social-accounts__empty">No accounts connected yet.</li>
        )}
      </ul>
    </section>
  );
}
