// JSX-free side-effect helpers for the social surface, kept separate from the
// React components so they can be unit-tested with a fake `invoke`/`openUrl`
// (node:assert), mirroring how `socialModel.ts` stays JSX-free for testing.
//
// Phase 5: OAuth start is server-owned. The desktop sends only `{ provider }`
// (and an optional return path); the server mints the connection id + CSRF
// `state`, owns client_id/redirect_uri, and returns the authorization URL the
// desktop opens in the system browser. The provider redirects to the *server*
// callback, so the desktop never sees the `code` — after the browser flow it
// re-polls `social_accounts` to discover the new account.

import type { Provider } from "./socialModel";

export type OAuthStartResponse = {
  oauthConnectionId: string;
  provider: Provider;
  authorizationUrl: string;
};

/** Minimal seams so tests can inject fakes without Tauri. */
export type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
export type OpenUrlFn = (url: string) => Promise<void>;

/**
 * Begin a provider connection: ask the server for the authorization URL, then
 * open it in the system browser. Returns the server response so callers can
 * thread the connection id if needed.
 *
 * The desktop supplies ONLY `provider` (+ optional `returnTo`) — no client id,
 * redirect uri, state, or timestamps (all server-owned now).
 */
export async function startConnect(
  invoke: InvokeFn,
  openUrl: OpenUrlFn,
  provider: Provider,
  returnTo = "/",
): Promise<OAuthStartResponse> {
  const start = await invoke<OAuthStartResponse>("social_oauth_start", {
    args: { provider, returnTo },
  });
  await openUrl(start.authorizationUrl);
  return start;
}
