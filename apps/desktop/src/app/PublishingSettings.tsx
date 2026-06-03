// SettingsModal → Publishing section.
//
// Owns three groups:
//
//   1. Connected accounts — the server-backed social publishing surface
//      (`<SocialAccounts />`). Connect / list / disconnect run through the
//      `social_*` Tauri commands over the `awidat-social` SocialApi facade;
//      tokens stay server-side. This replaces the legacy desktop-local
//      OAuth-connection rows ("replace as we go" — see
//      docs/superpowers/specs/2026-06-03-social-desktop-ui-design.md).
//
//   2. Global publishing preferences — Auto-disclose AI toggle and the
//      default-targets checkbox set. These persist across projects.
//
//   3. BYO OAuth-app credentials — `client_id` / `client_secret` inputs per
//      provider. Still serve the legacy upload path that has not yet been
//      replaced by the server-backed worker; retained until that cutover.
//
// Why a separate file: SettingsModal.tsx is already a stack of orthogonal
// sections; the Publishing section is the heaviest, so isolating it keeps the
// modal readable.

import { invoke, isTauri } from "@tauri-apps/api/core";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";

import { SocialAccounts } from "./social/SocialAccounts";
import {
  DEFAULT_PROVIDER_STATE,
  PROVIDERS,
  type ClientCredentialsState,
  type ProviderUiState,
} from "./publishingSettingsModel";
import { useAiDisclosure } from "../state/aiDisclosure";
import { useUploadPrefs } from "../state/uploadPrefs";
import { Button, Inline, Stack } from "../ui";

export function PublishingSettings() {
  const autoDisclose = useAiDisclosure((s) => s.autoDiscloseEnabled);
  const setAutoDisclose = useAiDisclosure((s) => s.setAutoDiscloseEnabled);
  const uploadDefaults = useUploadPrefs((s) => s.enabled);
  const toggleUploadDefault = useUploadPrefs((s) => s.toggle);

  // Per-provider BYO-credential presence. The connection state that used to
  // live here moved to the server-backed `<SocialAccounts />` surface; this
  // map now only tracks `credState` for the BYO-credentials rows.
  const [byProvider, setByProvider] = useState<
    Record<string, ProviderUiState>
  >(() => {
    const seed: Record<string, ProviderUiState> = {};
    for (const p of PROVIDERS) seed[p.key] = { ...DEFAULT_PROVIDER_STATE };
    return seed;
  });
  const [credentialsPath, setCredentialsPath] = useState<string | null>(null);

  // Refresh every provider's BYO-credential presence. Called on mount and
  // after a credentials save.
  const refreshCredentials = useCallback(async () => {
    if (!isTauri()) return;
    await Promise.all(
      PROVIDERS.map(async (p) => {
        try {
          const credState = await invoke<ClientCredentialsState>(
            "get_provider_client_credentials",
            { key: p.key },
          );
          setByProvider((prev) => ({
            ...prev,
            [p.key]: { ...prev[p.key], credState },
          }));
        } catch (e) {
          console.warn(`refresh creds ${p.key} failed`, e);
        }
      }),
    );
  }, []);

  useEffect(() => {
    void refreshCredentials();
    if (!isTauri()) return;
    invoke<string>("get_publishing_credentials_path")
      .then(setCredentialsPath)
      .catch((e) => console.warn("get_publishing_credentials_path failed", e));
  }, [refreshCredentials]);

  const updateProvider = useCallback(
    (key: string, patch: Partial<ProviderUiState>) => {
      setByProvider((prev) => ({ ...prev, [key]: { ...prev[key], ...patch } }));
    },
    [],
  );

  async function handleSubmitClientCredentials(
    key: string,
    clientId: string,
    clientSecret: string,
  ) {
    try {
      await invoke<void>("set_provider_client_credentials", {
        key,
        clientId,
        clientSecret,
      });
      updateProvider(key, {
        banner: { kind: "success", text: "Client credentials saved" },
      });
      await refreshCredentials();
    } catch (e) {
      updateProvider(key, {
        banner: { kind: "error", text: `Save failed: ${stringify(e)}` },
      });
    }
  }

  function revealCredentialsFolder() {
    if (!credentialsPath || !isTauri()) return;
    revealItemInDir(credentialsPath).catch((e) =>
      console.warn("revealItemInDir failed", e),
    );
  }

  return (
    <Stack gap="3">
      {/* ---- Connected accounts (server-backed) ---- */}
      <SocialAccounts />

      {/* ---- Global preferences ---- */}
      <Stack gap="2" className="pt-3 border-t border-[var(--color-border-subtle)]">
        <h4 className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)] m-0">
          Preferences
        </h4>
        <PreferenceToggle
          label="Auto-disclose AI content"
          note="AI labels are required by YouTube, TikTok, and Meta for synthetic content."
          checked={autoDisclose}
          onChange={setAutoDisclose}
        />
        <Stack gap="1">
          <span className="text-[var(--text-body-sm)] text-[var(--color-text-secondary)]">
            Default upload targets
          </span>
          <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
            Providers below get the Upload pip toggled on by default when
            you queue a new render.
          </span>
          <Inline gap="2" align="center" className="flex-wrap">
            {PROVIDERS.map((p) => (
              <DefaultTargetCheckbox
                key={p.key}
                label={p.displayName}
                checked={uploadDefaults.has(p.key)}
                onChange={() => void toggleUploadDefault(p.key)}
              />
            ))}
          </Inline>
        </Stack>
      </Stack>

      {/* ---- BYO credentials ---- */}
      <Stack gap="2" className="pt-3 border-t border-[var(--color-border-subtle)]">
        <h4 className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] font-semibold text-[var(--color-text-muted)] m-0">
          Bring your own credentials
        </h4>
        <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
          Register your own OAuth app at each platform&apos;s developer
          console, then paste the client ID + secret here. Required for
          real uploads (the bundled placeholder is rejected by the
          platform).
        </span>
        {PROVIDERS.map((p) => (
          <ByoCredentialsRow
            key={p.key}
            displayName={p.displayName}
            devConsoleUrl={p.devConsoleUrl}
            state={(byProvider[p.key] ?? DEFAULT_PROVIDER_STATE).credState}
            onSubmit={(id, secret) =>
              handleSubmitClientCredentials(p.key, id, secret)
            }
          />
        ))}
      </Stack>

      {/* ---- Credentials file footer ---- */}
      <Stack gap="1" className="pt-3 border-t border-[var(--color-border-subtle)]">
        <Inline justify="between" align="center" gap="2">
          <span
            className="font-mono text-[var(--text-caption)] text-[var(--color-text-muted)] truncate"
            title={credentialsPath ?? "Unavailable"}
          >
            Credentials stored at: {credentialsPath ?? "Unavailable"}
          </span>
          <Button
            variant="ghost"
            size="sm"
            onClick={revealCredentialsFolder}
            disabled={!credentialsPath}
          >
            Open folder
          </Button>
        </Inline>
        <span className="text-[var(--text-caption)] text-[var(--color-text-success,#4ade80)]">
          ✓ Secrets stored in the OS keychain. Metadata in {credentialsPath ?? "publishing.json"}.
        </span>
      </Stack>
    </Stack>
  );
}

// ----------------------------------------------------------------- helpers

function stringify(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}

// ----------------------------------------------------------------- rows

function PreferenceToggle({
  label,
  note,
  checked,
  onChange,
}: {
  label: string;
  note: string;
  checked: boolean;
  onChange: (next: boolean) => void;
}) {
  return (
    <Stack gap="1">
      <Inline justify="between" align="center" gap="2">
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)]">
          {label}
        </span>
        <label className="inline-flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            checked={checked}
            onChange={(e) => onChange(e.target.checked)}
            className="accent-[var(--color-accent-primary,#3b82f6)]"
          />
          <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">
            {checked ? "On" : "Off"}
          </span>
        </label>
      </Inline>
      <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
        {note}
      </span>
    </Stack>
  );
}

function DefaultTargetCheckbox({
  label,
  checked,
  onChange,
}: {
  label: string;
  checked: boolean;
  onChange: () => void;
}) {
  return (
    <label className="inline-flex items-center gap-1.5 cursor-pointer px-2 py-1 rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)]">
      <input
        type="checkbox"
        checked={checked}
        onChange={onChange}
        className="accent-[var(--color-accent-primary,#3b82f6)]"
      />
      <span className="text-[var(--text-caption)] text-[var(--color-text-primary)]">
        {label}
      </span>
    </label>
  );
}

function ByoCredentialsRow({
  displayName,
  devConsoleUrl,
  state,
  onSubmit,
}: {
  displayName: string;
  devConsoleUrl: string;
  state: ClientCredentialsState;
  onSubmit: (clientId: string, clientSecret: string) => void;
}) {
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const configured = state.client_id_set && state.client_secret_set;

  function handleSubmit() {
    if (!clientId.trim() || !clientSecret.trim()) return;
    onSubmit(clientId.trim(), clientSecret.trim());
    setClientId("");
    setClientSecret("");
  }

  return (
    <Stack
      gap="1"
      className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] p-2"
    >
      <Inline justify="between" align="center" gap="2">
        <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)] font-semibold">
          {displayName}
        </span>
        <Inline gap="2" align="center">
          {configured ? (
            <span className="text-[var(--text-caption)] text-[var(--color-text-success,#4ade80)]">
              ✓ Configured
            </span>
          ) : (
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              Not set
            </span>
          )}
          <Button
            variant="ghost"
            size="sm"
            onClick={() =>
              isTauri()
                ? openUrl(devConsoleUrl).catch((e) =>
                    console.warn("openUrl failed", e),
                  )
                : window.open(devConsoleUrl, "_blank", "noopener")
            }
          >
            Dev console
          </Button>
        </Inline>
      </Inline>
      <Inline gap="1" align="center">
        <input
          type="password"
          value={clientId}
          onChange={(e) => setClientId(e.target.value)}
          placeholder={state.client_id_set ? "client_id (saved)" : "client_id"}
          className="flex-1 min-w-0 px-2 py-1 rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] text-[var(--text-body-sm)] text-[var(--color-text-primary)] font-mono"
          autoComplete="off"
        />
        <input
          type="password"
          value={clientSecret}
          onChange={(e) => setClientSecret(e.target.value)}
          placeholder={
            state.client_secret_set ? "client_secret (saved)" : "client_secret"
          }
          className="flex-1 min-w-0 px-2 py-1 rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] text-[var(--text-body-sm)] text-[var(--color-text-primary)] font-mono"
          autoComplete="off"
        />
        <Button
          variant="secondary"
          size="sm"
          onClick={handleSubmit}
          disabled={!clientId.trim() || !clientSecret.trim()}
        >
          Save
        </Button>
      </Inline>
    </Stack>
  );
}
