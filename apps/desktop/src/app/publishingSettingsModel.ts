// Pure helpers + types for the W5.A5 Settings → Publishing section.
//
// Split off from `PublishingSettings.tsx` so the JSX-free pieces (the
// provider catalog, the status-string derivation, the primary-action
// picker) can be exercised by `tests/publishing-settings.test.ts`
// without dragging React + Tauri into the node harness.
//
// SRP: model + derivation logic here, presentation in the .tsx file.

import type { DeliveryTargetKey } from "../shell/delivery/types";
import type { Provider } from "./social/socialModel";

/** The providers the publishing pipeline exposes. Kept as
 *  a constant so the Settings section renders even when
 *  `list_providers` is unreachable (running outside Tauri, backend not
 *  yet initialised). */
export const PROVIDERS: ReadonlyArray<{
  key: Provider;
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
  {
    key: "twitter_x",
    displayName: "Twitter/X",
    devConsoleUrl: "https://developer.x.com/",
  },
];

export const VISIBLE_PROVIDERS = PROVIDERS.filter(
  (provider) => provider.key !== "tiktok" && provider.key !== "instagram",
);

export function providerDisplayName(key: DeliveryTargetKey): string {
  return PROVIDERS.find((provider) => provider.key === key)?.displayName ?? key;
}

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

/** Per-provider local UI state.
 *
 *  W6.A1 — the OAuth flow is now best-effort auto-capture: Connect
 *  opens the browser AND the 127.0.0.1:8419 backend listener;
 *  redirect handoff is invisible. The paste form stays as a fallback
 *  for when the listener can't bind / the event doesn't arrive in
 *  time. `awaitingCallback` drives the "Waiting for browser…" copy,
 *  `fallbackToPaste` flips to true after the LISTENER_FALLBACK_MS
 *  deadline OR a backend error event so the user has an escape hatch.
 *  `pendingCode` remains the in-progress paste input for that path.
 *
 *  `expectedState` is the CSRF nonce from the `OAuthChallenge` —
 *  parked here so the event subscriber can ignore stale / cross-flow
 *  callbacks without round-tripping through Tauri state. */
export type ProviderUiState = {
  status: ConnectionStatus;
  credState: ClientCredentialsState;
  /** True between Connect click and either auto-capture success or
   *  manual paste submission. Drives the "OAuth UI is open" branch. */
  oauthInProgress: boolean;
  /** True while we're waiting on the backend listener event. False
   *  means show the paste form (either listener failed or the
   *  fallback timer fired). */
  awaitingCallback: boolean;
  /** True once the fallback path is engaged — paste form visible,
   *  awaiting copy reads "Listener didn't catch the redirect — paste
   *  the code manually." */
  fallbackToPaste: boolean;
  pendingCode: string;
  /** CSRF nonce echoed back by the provider. Used by the event
   *  subscriber to discard cross-flow callbacks. Empty when no flow
   *  is in progress. */
  expectedState: string;
  /** Inline error or success banner copy. */
  banner?: { kind: "error" | "success"; text: string };
};

export const DEFAULT_PROVIDER_STATE: ProviderUiState = {
  status: { connected: false },
  credState: { client_id_set: false, client_secret_set: false },
  oauthInProgress: false,
  awaitingCallback: false,
  fallbackToPaste: false,
  pendingCode: "",
  expectedState: "",
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
