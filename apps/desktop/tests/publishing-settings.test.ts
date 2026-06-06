// Tests for the W5.A5 Settings → Publishing pure helpers.
//
// We don't mount React — the surface we care about (the status-string
// and primary-action derivations + the provider catalog) is pure, and
// these are the load-bearing pieces the integration tests can't fake
// once Tauri's invoke is stubbed out.
//
// Backend coverage (BYO client-id substitution, disconnect preserves
// client_credentials) lives in the publishing crate's Rust tests; see
// `apps/desktop/src-tauri/src/publishing/oauth.rs` and
// `apps/desktop/src-tauri/src/publishing/storage.rs`.

import { strict as assert } from "node:assert";

import {
  DEFAULT_PROVIDER_STATE,
  PROVIDERS,
  providerDisplayName,
  primaryAction,
  statusText,
  type ProviderUiState,
} from "../src/app/publishingSettingsModel.ts";

// ---- PROVIDERS catalog has the major publishing platforms ----
{
  assert.equal(PROVIDERS.length, 4, "four providers");
  const keys = PROVIDERS.map((p) => p.key);
  assert.deepEqual(
    keys.sort(),
    ["instagram", "tiktok", "twitter_x", "youtube"],
    "the publishing pipeline targets YouTube/TikTok/Instagram/Twitter-X",
  );
  for (const p of PROVIDERS) {
    assert.ok(
      p.devConsoleUrl.startsWith("https://"),
      `${p.key} dev console must be https`,
    );
  }
  assert.deepEqual(
    PROVIDERS.map((p) => providerDisplayName(p.key)),
    ["YouTube", "TikTok", "Instagram", "Twitter/X"],
    "connected-account and publishing defaults should share provider labels",
  );
}

// ---- DEFAULT_PROVIDER_STATE is "fresh modal, nothing configured" ----
{
  // Brief-named scenario: "Settings modal renders the section
  // correctly when no providers configured."
  assert.equal(DEFAULT_PROVIDER_STATE.status.connected, false);
  assert.equal(DEFAULT_PROVIDER_STATE.credState.client_id_set, false);
  assert.equal(DEFAULT_PROVIDER_STATE.credState.client_secret_set, false);
  assert.equal(DEFAULT_PROVIDER_STATE.oauthInProgress, false);
  // W6.A1 — the auto-capture path starts disabled on a fresh state;
  // both the waiting and fallback flags must be false so the row
  // shows the bare Connect button and nothing else.
  assert.equal(DEFAULT_PROVIDER_STATE.awaitingCallback, false);
  assert.equal(DEFAULT_PROVIDER_STATE.fallbackToPaste, false);
  assert.equal(DEFAULT_PROVIDER_STATE.expectedState, "");
  assert.equal(DEFAULT_PROVIDER_STATE.pendingCode, "");
  // statusText on a fresh state is the "Not connected" copy — this
  // is what every provider row renders on first open.
  assert.equal(statusText(DEFAULT_PROVIDER_STATE), "Not connected");
  // primaryAction defaults to Connect.
  const action = primaryAction(DEFAULT_PROVIDER_STATE);
  assert.equal(action.kind, "connect");
  assert.equal(action.label, "Connect…");
}

// ---- statusText surfaces BYO-only progress ----
{
  // The user has pasted client_id + client_secret but not completed
  // OAuth — surface that distinction so they know progress is partial.
  const ui: ProviderUiState = {
    ...DEFAULT_PROVIDER_STATE,
    credState: { client_id_set: true, client_secret_set: true },
  };
  assert.equal(statusText(ui), "Not connected · client ID configured");
}

// ---- statusText: connected with account name ----
{
  const ui: ProviderUiState = {
    ...DEFAULT_PROVIDER_STATE,
    status: { connected: true, account_name: "you@gmail.com" },
  };
  assert.equal(statusText(ui), "Connected as you@gmail.com");
}

// ---- statusText: connected without account name falls back to "Connected" ----
{
  const ui: ProviderUiState = {
    ...DEFAULT_PROVIDER_STATE,
    status: { connected: true },
  };
  assert.equal(statusText(ui), "Connected");
}

// ---- statusText: token expiring soon surfaces "expires in Nd" ----
{
  // 3 days from now in epoch seconds.
  const threeDaysOut = Math.floor(Date.now() / 1000) + 3 * 86_400;
  const ui: ProviderUiState = {
    ...DEFAULT_PROVIDER_STATE,
    status: {
      connected: true,
      account_name: "@handle",
      expires_at: threeDaysOut,
    },
  };
  const text = statusText(ui);
  assert.ok(
    text.includes("@handle") && text.includes("expires in"),
    `expected expiry copy, got: ${text}`,
  );
}

// ---- statusText: long-lived token (no expiry) doesn't add noise ----
{
  // Instagram long-lived tokens have no expires_at. We must not invent
  // an "expires in" hint for those.
  const ui: ProviderUiState = {
    ...DEFAULT_PROVIDER_STATE,
    status: { connected: true, account_name: "@ig" },
  };
  assert.equal(statusText(ui), "Connected as @ig");
}

// ---- statusText: expired token shows "Token expired" ----
{
  const past = Math.floor(Date.now() / 1000) - 3600;
  const ui: ProviderUiState = {
    ...DEFAULT_PROVIDER_STATE,
    status: { connected: true, account_name: "@h", expires_at: past },
  };
  assert.ok(statusText(ui).includes("Token expired"));
}

// ---- primaryAction: expired connection → Reconnect ----
{
  const past = Math.floor(Date.now() / 1000) - 3600;
  const ui: ProviderUiState = {
    ...DEFAULT_PROVIDER_STATE,
    status: { connected: true, expires_at: past },
  };
  const action = primaryAction(ui);
  assert.equal(action.kind, "reconnect");
  assert.equal(action.label, "Reconnect");
}

// ---- primaryAction: connected + non-expired → Disconnect ----
{
  const future = Math.floor(Date.now() / 1000) + 30 * 86_400;
  const ui: ProviderUiState = {
    ...DEFAULT_PROVIDER_STATE,
    status: { connected: true, expires_at: future },
  };
  const action = primaryAction(ui);
  assert.equal(action.kind, "disconnect");
}

console.log("publishing-settings: OK");
