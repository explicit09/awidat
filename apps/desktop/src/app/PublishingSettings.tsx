// SettingsModal → Publishing section (W5.A5).
//
// Owns three groups of rows:
//
//   1. Provider connection — one row per publishing provider
//      (YouTube / TikTok / Instagram). Status pill + Connect / Reconnect
//      / Disconnect action; Connect kicks off the OAuth flow (system
//      browser + paste-the-code inline form).
//
//   2. Global publishing preferences — Auto-disclose AI toggle (W5.A4)
//      and the default-targets checkbox set (W5.A2). These persist
//      across projects so the user only sets them once.
//
//   3. BYO OAuth-app credentials — `client_id` / `client_secret`
//      inputs per provider. These are the path to making OAuth work
//      end-to-end; the user registers their own app at the platform's
//      dev console and pastes the credentials here.
//
// Why a separate file: SettingsModal.tsx was already ~400 LOC of
// orthogonal sections (Project / Agent / Workspace / Indexers / About).
// The Publishing section is the heaviest section — wraps async OAuth,
// inline forms, and BYO credential management — so isolating it keeps
// the modal readable.
//
// Stub-phase reality: until the user pastes real BYO credentials, the
// OAuth URL has `client_id=YOUR_CLIENT_ID_HERE` and the platform will
// reject it. The UI still walks the full flow so the user can see what
// would happen — and the moment they paste real creds, the placeholder
// substitution flips automatically (`client_id_for` in the backend).

import { invoke, isTauri } from "@tauri-apps/api/core";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useState } from "react";

import {
  DEFAULT_PROVIDER_STATE,
  PROVIDERS,
  primaryAction,
  statusText,
  type ClientCredentialsState,
  type ConnectionStatus,
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

  const [byProvider, setByProvider] = useState<
    Record<string, ProviderUiState>
  >(() => {
    const seed: Record<string, ProviderUiState> = {};
    for (const p of PROVIDERS) seed[p.key] = { ...DEFAULT_PROVIDER_STATE };
    return seed;
  });
  const [credentialsPath, setCredentialsPath] = useState<string | null>(null);

  // Refresh every provider's status + BYO-presence in parallel. Called
  // on mount and after every state-changing action (connect, disconnect,
  // BYO paste).
  const refreshAll = useCallback(async () => {
    if (!isTauri()) return;
    await Promise.all(
      PROVIDERS.map(async (p) => {
        try {
          const [status, credState] = await Promise.all([
            invoke<ConnectionStatus>("get_provider_status", { key: p.key }),
            invoke<ClientCredentialsState>(
              "get_provider_client_credentials",
              { key: p.key },
            ),
          ]);
          setByProvider((prev) => ({
            ...prev,
            [p.key]: { ...prev[p.key], status, credState },
          }));
        } catch (e) {
          console.warn(`refresh ${p.key} failed`, e);
        }
      }),
    );
  }, []);

  useEffect(() => {
    void refreshAll();
    if (!isTauri()) return;
    invoke<string>("get_publishing_credentials_path")
      .then(setCredentialsPath)
      .catch((e) => console.warn("get_publishing_credentials_path failed", e));
  }, [refreshAll]);

  const updateProvider = useCallback(
    (key: string, patch: Partial<ProviderUiState>) => {
      setByProvider((prev) => ({ ...prev, [key]: { ...prev[key], ...patch } }));
    },
    [],
  );

  async function handleConnect(key: string) {
    updateProvider(key, { banner: undefined });
    try {
      const challenge = await invoke<{ url: string; state: string }>(
        "begin_provider_oauth",
        { key },
      );
      // Open the URL in the user's default browser via Tauri's opener
      // plugin — same path the Project section uses for "Open folder".
      await openUrl(challenge.url).catch((e) =>
        console.warn("openUrl failed", e),
      );
      updateProvider(key, { oauthInProgress: true, pendingCode: "" });
    } catch (e) {
      updateProvider(key, {
        banner: { kind: "error", text: `Connect failed: ${stringify(e)}` },
      });
    }
  }

  async function handleSubmitCode(key: string) {
    const ui = byProvider[key];
    if (!ui || !ui.pendingCode.trim()) return;
    try {
      await invoke<void>("complete_provider_oauth", {
        key,
        code: ui.pendingCode.trim(),
      });
      updateProvider(key, {
        oauthInProgress: false,
        pendingCode: "",
        banner: { kind: "success", text: "Connected" },
      });
      await refreshAll();
    } catch (e) {
      updateProvider(key, {
        banner: { kind: "error", text: `OAuth failed: ${stringify(e)}` },
      });
    }
  }

  async function handleDisconnect(key: string) {
    try {
      await invoke<void>("disconnect_provider", { key });
      updateProvider(key, {
        oauthInProgress: false,
        pendingCode: "",
        banner: undefined,
      });
      await refreshAll();
    } catch (e) {
      updateProvider(key, {
        banner: { kind: "error", text: `Disconnect failed: ${stringify(e)}` },
      });
    }
  }

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
      await refreshAll();
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
      {/* ---- Connection rows ---- */}
      <Stack gap="2">
        {PROVIDERS.map((p) => {
          const ui = byProvider[p.key] ?? DEFAULT_PROVIDER_STATE;
          return (
            <ProviderConnectionRow
              key={p.key}
              providerKey={p.key}
              displayName={p.displayName}
              ui={ui}
              onConnect={() => handleConnect(p.key)}
              onSubmitCode={() => handleSubmitCode(p.key)}
              onCancelOAuth={() =>
                updateProvider(p.key, {
                  oauthInProgress: false,
                  pendingCode: "",
                  banner: undefined,
                })
              }
              onChangeCode={(code) => updateProvider(p.key, { pendingCode: code })}
              onDisconnect={() => handleDisconnect(p.key)}
            />
          );
        })}
      </Stack>

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
        <span className="text-[var(--text-caption)] text-[var(--color-text-warning,#facc15)]">
          ⚠ Credentials are stored in plain JSON. Keychain integration: Wave 6.
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

function ProviderConnectionRow({
  providerKey,
  displayName,
  ui,
  onConnect,
  onSubmitCode,
  onCancelOAuth,
  onChangeCode,
  onDisconnect,
}: {
  providerKey: string;
  displayName: string;
  ui: ProviderUiState;
  onConnect: () => void;
  onSubmitCode: () => void;
  onCancelOAuth: () => void;
  onChangeCode: (code: string) => void;
  onDisconnect: () => void;
}) {
  const action = primaryAction(ui);
  return (
    <Stack
      gap="1"
      className="rounded-[var(--radius-sm)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-input)] p-2"
    >
      <Inline justify="between" align="center" gap="2">
        <Stack gap="0" className="min-w-0 flex-1">
          <span className="text-[var(--text-body-sm)] text-[var(--color-text-primary)] font-semibold">
            {displayName}
          </span>
          <span
            className="text-[var(--text-caption)] text-[var(--color-text-muted)] truncate"
            title={statusText(ui)}
          >
            {statusText(ui)}
          </span>
        </Stack>
        {action.kind === "disconnect" ? (
          <Button variant="ghost" size="sm" onClick={onDisconnect}>
            {action.label}
          </Button>
        ) : (
          <Button
            variant="secondary"
            size="sm"
            onClick={onConnect}
            data-provider-key={providerKey}
          >
            {action.label}
          </Button>
        )}
      </Inline>
      {ui.oauthInProgress ? (
        <Stack gap="1" className="pt-1">
          <span className="text-[var(--text-caption)] text-[var(--color-text-secondary)]">
            Paste the authorization code from the browser:
          </span>
          <Inline gap="1" align="center">
            <input
              type="text"
              value={ui.pendingCode}
              onChange={(e) => onChangeCode(e.target.value)}
              placeholder="auth code"
              className="flex-1 min-w-0 px-2 py-1 rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] text-[var(--text-body-sm)] text-[var(--color-text-primary)] font-mono"
              autoFocus
            />
            <Button
              variant="primary"
              size="sm"
              onClick={onSubmitCode}
              disabled={!ui.pendingCode.trim()}
            >
              Submit
            </Button>
            <Button variant="ghost" size="sm" onClick={onCancelOAuth}>
              Cancel
            </Button>
          </Inline>
        </Stack>
      ) : null}
      {ui.banner ? (
        <span
          className={
            ui.banner.kind === "error"
              ? "text-[var(--text-caption)] text-[var(--color-text-danger,#f87171)]"
              : "text-[var(--text-caption)] text-[var(--color-text-success,#4ade80)]"
          }
        >
          {ui.banner.kind === "success" ? "✓ " : ""}
          {ui.banner.text}
        </span>
      ) : null}
    </Stack>
  );
}

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
