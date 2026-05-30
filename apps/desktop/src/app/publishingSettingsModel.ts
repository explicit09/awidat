// Pure helpers + types for the W5.A5 Settings → Publishing section.
//
// Split off from `PublishingSettings.tsx` so the JSX-free pieces (the
// provider catalog, the status-string derivation, the primary-action
// picker) can be exercised by `tests/publishing-settings.test.ts`
// without dragging React + Tauri into the node harness.
//
// SRP: model + derivation logic here, presentation in the .tsx file.

import type { DeliveryTargetKey } from "../shell/delivery/types";

/** The three providers the W5 publishing pipeline supports. Kept as
 *  a constant so the Settings section renders even when
 *  `list_providers` is unreachable (running outside Tauri, backend not
 *  yet initialised). */
export const PROVIDERS: ReadonlyArray<{
  key: DeliveryTargetKey;
  displayName: string;
  devConsoleUrl: string;
}> = [
  {
    key: "youtube",
    displayName: "YouTube",
    devConsoleUrl: "https://console.cloud.google.com/apis/credentials",
  },
  {
    key: "tiktok",
    displayName: "TikTok",
    devConsoleUrl: "https://developers.tiktok.com/",
  },
  {
    key: "instagram",
    displayName: "Instagram",
    devConsoleUrl: "https://developers.facebook.com/apps",
  },
];

/** Mirror of the backend `ConnectionStatus` shape. */
export type ConnectionStatus = {
  connected: boolean;
  account_name?: string | null;
  expires_at?: number | null;
};

/** Mirror of the backend `ClientCredentialsState` — booleans only,
 *  the actual `client_secret` never leaves the backend. */
export type ClientCredentialsState = {
  client_id_set: boolean;
  client_secret_set: boolean;
};

/** Per-provider local UI state. The OAuth flow is two-step: Connect
 *  opens the browser, then the user pastes the code into this inline
 *  form. `pendingCode` is the in-progress input. */
export type ProviderUiState = {
  status: ConnectionStatus;
  credState: ClientCredentialsState;
  /** "code" input visible after Connect was clicked. */
  oauthInProgress: boolean;
  pendingCode: string;
  /** Inline error or success banner copy. */
  banner?: { kind: "error" | "success"; text: string };
};

export const DEFAULT_PROVIDER_STATE: ProviderUiState = {
  status: { connected: false },
  credState: { client_id_set: false, client_secret_set: false },
  oauthInProgress: false,
  pendingCode: "",
};

/** Render the human-facing status string for one provider's row.
 *
 *  Resolution order:
 *  - `connected` false + no BYO creds → `"Not connected"`
 *  - `connected` false + BYO creds set → `"Not connected · client ID configured"`
 *  - `connected` true → `"Connected[ as <account>]"` + optional expiry hint
 *
 *  Expiry hint only fires when the token is within 7 days of expiry
 *  (or already past) — long-lived tokens (Instagram, no `expires_at`)
 *  read as a clean "Connected" so the row isn't visually noisy.
 */
export function statusText(ui: ProviderUiState): string {
  if (!ui.status.connected) {
    return ui.credState.client_id_set
      ? "Not connected · client ID configured"
      : "Not connected";
  }
  const parts: string[] = [];
  parts.push(
    ui.status.account_name
      ? `Connected as ${ui.status.account_name}`
      : "Connected",
  );
  if (ui.status.expires_at != null) {
    const daysLeft = Math.floor(
      (ui.status.expires_at * 1000 - Date.now()) / 86_400_000,
    );
    if (daysLeft < 0) parts.push("Token expired");
    else if (daysLeft <= 7) parts.push(`expires in ${daysLeft}d`);
  }
  return parts.join(" · ");
}

/** Pick the right primary-action label + kind for the current state.
 *  Three states:
 *  - `connect` (not connected, OAuth not in progress)
 *  - `reconnect` (was connected but token expired)
 *  - `disconnect` (connected + valid token)
 */
export function primaryAction(ui: ProviderUiState): {
  label: string;
  kind: "connect" | "reconnect" | "disconnect";
} {
  if (!ui.status.connected) return { label: "Connect…", kind: "connect" };
  const expired =
    ui.status.expires_at != null && ui.status.expires_at * 1000 < Date.now();
  if (expired) return { label: "Reconnect", kind: "reconnect" };
  return { label: "Disconnect", kind: "disconnect" };
}
