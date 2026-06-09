import { invoke, isTauri } from "@tauri-apps/api/core";
import { Check, Download, KeyRound, Loader2, Trash2, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import {
  providerKeyActionLabel,
  providerKeyStatusLabel,
  providerKeySubtitle,
  type ProviderKeyImportSummary,
  type ProviderKeyRow,
  type ProviderKeyTestResult,
} from "./providerKeysSettingsModel";

export function ProviderKeysSettings() {
  const [rows, setRows] = useState<ProviderKeyRow[]>([]);
  const [editing, setEditing] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const rowsByKey = useMemo(
    () => new Map(rows.map((row) => [row.key, row] as const)),
    [rows],
  );

  async function refresh() {
    if (!isTauri()) {
      setRows([]);
      return;
    }
    const nextRows = await invoke<ProviderKeyRow[]>("list_provider_keys");
    setRows(nextRows);
  }

  useEffect(() => {
    void refresh().catch((err) => setError(String(err)));
  }, []);

  function beginEdit(row: ProviderKeyRow) {
    setError(null);
    setNotice(null);
    setEditing(row.key);
    setDrafts((current) => ({ ...current, [row.key]: "" }));
  }

  async function save(provider: string) {
    const row = rowsByKey.get(provider);
    const value = drafts[provider]?.trim() ?? "";
    if (!row || !value) return;
    setBusy(provider);
    setError(null);
    setNotice(null);
    try {
      const nextRows = await invoke<ProviderKeyRow[]>("save_provider_key", {
        provider,
        value,
      });
      setRows(nextRows);
      setEditing(null);
      setDrafts((current) => ({ ...current, [provider]: "" }));
      setNotice(`${row.label} key saved.`);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }

  async function remove(provider: string) {
    const row = rowsByKey.get(provider);
    if (!row) return;
    setBusy(provider);
    setError(null);
    setNotice(null);
    try {
      const nextRows = await invoke<ProviderKeyRow[]>("remove_provider_key", {
        provider,
      });
      setRows(nextRows);
      setEditing((current) => (current === provider ? null : current));
      setNotice(`${row.label} key removed.`);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }

  async function testDraft(provider: string) {
    const value = drafts[provider]?.trim() ?? "";
    if (!value) return;
    setBusy(`${provider}:test`);
    setError(null);
    setNotice(null);
    try {
      const result = await invoke<ProviderKeyTestResult>("test_provider_key", {
        provider,
        value,
      });
      setNotice(result.message);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }

  async function importLegacy() {
    setBusy("import");
    setError(null);
    setNotice(null);
    try {
      const summary = await invoke<ProviderKeyImportSummary>(
        "import_legacy_provider_keys",
      );
      setRows(summary.rows);
      setNotice(
        summary.imported.length === 0
          ? "No legacy provider keys found."
          : `Imported ${summary.imported.length} provider key${summary.imported.length === 1 ? "" : "s"}.`,
      );
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  }

  if (!isTauri()) {
    return (
      <ProviderKeysMessage tone="muted">
        Provider keys are available in the desktop app.
      </ProviderKeysMessage>
    );
  }

  return (
    <div className="grid gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="min-w-0">
          <p className="m-0 text-[12px] text-[var(--color-text-secondary)]">
            Bring-your-own keys for local indexing, generated media, and media
            search.
          </p>
        </div>
        <button
          type="button"
          className="glass-ghost inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[12px] font-semibold disabled:pointer-events-none disabled:opacity-45"
          onClick={() => void importLegacy()}
          disabled={busy !== null}
          title="Import keys saved by older Montage builds"
        >
          {busy === "import" ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <Download className="h-3.5 w-3.5" />
          )}
          Import old keys
        </button>
      </div>

      {error ? <ProviderKeysMessage tone="error">{error}</ProviderKeysMessage> : null}
      {notice ? <ProviderKeysMessage tone="success">{notice}</ProviderKeysMessage> : null}

      <div className="grid gap-2">
        {rows.map((row) => {
          const isEditing = editing === row.key;
          const isConfigured = row.status === "configured";
          const draft = drafts[row.key] ?? "";
          const rowBusy = busy === row.key;
          const testBusy = busy === `${row.key}:test`;

          return (
            <section
              key={row.key}
              className="rounded-lg border border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)] px-3 py-3"
            >
              <div className="flex items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-[13px] font-bold text-[var(--color-text-primary)]">
                      {row.label}
                    </span>
                    <StatusBadge configured={isConfigured}>
                      {providerKeyStatusLabel(row)}
                    </StatusBadge>
                  </div>
                  <p className="m-0 mt-1 text-[11px] leading-snug text-[var(--color-text-muted)]">
                    {providerKeySubtitle(row)}
                  </p>
                  <p className="m-0 mt-1 font-mono text-[11px] text-[var(--color-text-secondary)]">
                    {row.redacted ?? row.account}
                  </p>
                </div>
                <div className="flex shrink-0 items-center gap-1.5">
                  <button
                    type="button"
                    className="glass-cta inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[12px] font-semibold disabled:pointer-events-none disabled:opacity-45"
                    onClick={() => beginEdit(row)}
                    disabled={busy !== null}
                    title={`${providerKeyActionLabel(row)} ${row.label} key`}
                  >
                    <KeyRound className="h-3.5 w-3.5" />
                    {providerKeyActionLabel(row)}
                  </button>
                  <button
                    type="button"
                    className="glass-ghost grid h-8 w-8 place-items-center rounded-lg disabled:pointer-events-none disabled:opacity-35"
                    onClick={() => void remove(row.key)}
                    disabled={!isConfigured || busy !== null}
                    title={`Remove ${row.label} key`}
                    aria-label={`Remove ${row.label} key`}
                  >
                    {rowBusy ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <Trash2 className="h-3.5 w-3.5" />
                    )}
                  </button>
                </div>
              </div>

              {isEditing ? (
                <div className="mt-3 grid gap-2 rounded-lg border border-[var(--glass-border)] bg-[rgba(0,0,0,0.18)] p-2">
                  <input
                    type="password"
                    autoComplete="off"
                    spellCheck={false}
                    value={draft}
                    onChange={(event) =>
                      setDrafts((current) => ({
                        ...current,
                        [row.key]: event.target.value,
                      }))
                    }
                    placeholder={row.envVar}
                    className="min-h-9 rounded-lg border border-[var(--glass-border)] bg-[rgba(0,0,0,0.22)] px-3 font-mono text-[12px] text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-muted)] focus:border-[var(--color-brand)]"
                  />
                  <div className="flex flex-wrap justify-end gap-2">
                    <button
                      type="button"
                      className="glass-ghost inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[12px] font-semibold"
                      onClick={() => setEditing(null)}
                      disabled={busy !== null}
                    >
                      <X className="h-3.5 w-3.5" />
                      Cancel
                    </button>
                    <button
                      type="button"
                      className="glass-ghost inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[12px] font-semibold disabled:pointer-events-none disabled:opacity-45"
                      onClick={() => void testDraft(row.key)}
                      disabled={!draft.trim() || busy !== null}
                    >
                      {testBusy ? (
                        <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      ) : (
                        <Check className="h-3.5 w-3.5" />
                      )}
                      Check
                    </button>
                    <button
                      type="button"
                      className="glass-cta inline-flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-[12px] font-semibold disabled:pointer-events-none disabled:opacity-45"
                      onClick={() => void save(row.key)}
                      disabled={!draft.trim() || busy !== null}
                    >
                      {rowBusy ? (
                        <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      ) : (
                        <Check className="h-3.5 w-3.5" />
                      )}
                      Save
                    </button>
                  </div>
                </div>
              ) : null}
            </section>
          );
        })}
      </div>
    </div>
  );
}

function StatusBadge({
  configured,
  children,
}: {
  configured: boolean;
  children: string;
}) {
  return (
    <span
      className={
        configured
          ? "rounded-md border border-[rgba(34,197,94,0.35)] bg-[rgba(34,197,94,0.10)] px-1.5 py-0.5 text-[10px] font-semibold text-[rgb(134,239,172)]"
          : "rounded-md border border-[var(--glass-border)] bg-[rgba(255,255,255,0.035)] px-1.5 py-0.5 text-[10px] font-semibold text-[var(--color-text-muted)]"
      }
    >
      {children}
    </span>
  );
}

function ProviderKeysMessage({
  tone,
  children,
}: {
  tone: "error" | "success" | "muted";
  children: string;
}) {
  const className =
    tone === "error"
      ? "border-[rgba(239,68,68,0.34)] bg-[rgba(239,68,68,0.10)] text-[var(--color-text-danger,#f87171)]"
      : tone === "success"
        ? "border-[rgba(34,197,94,0.32)] bg-[rgba(34,197,94,0.08)] text-[rgb(134,239,172)]"
        : "border-[var(--glass-border)] bg-[rgba(255,255,255,0.025)] text-[var(--color-text-muted)]";
  return (
    <p
      role={tone === "error" ? "alert" : "status"}
      className={`m-0 rounded-lg border px-3 py-2 text-[12px] ${className}`}
    >
      {children}
    </p>
  );
}
