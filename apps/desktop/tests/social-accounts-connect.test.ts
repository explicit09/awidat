/**
 * Pure-logic test for the OAuth connect side-effect (Phase 5).
 *
 * The desktop must call `social_oauth_start` with ONLY `{ provider, returnTo }`
 * (no client id / redirect uri / state / timestamps — all server-owned now),
 * then open the returned authorization URL. No React, no Tauri — fakes injected.
 */
import { strict as assert } from "node:assert";

import { startConnect } from "../src/app/social/socialActions.ts";
import type { Provider } from "../src/app/social/socialModel.ts";

type Call = { command: string; args?: Record<string, unknown> };

function fakes(authorizationUrl: string) {
  const calls: Call[] = [];
  const opened: string[] = [];
  const invoke = async <T,>(
    command: string,
    args?: Record<string, unknown>,
  ): Promise<T> => {
    calls.push({ command, args });
    return {
      oauthConnectionId: "conn-1",
      provider: "youtube" as Provider,
      authorizationUrl,
    } as T;
  };
  const openUrl = async (url: string): Promise<void> => {
    opened.push(url);
  };
  return { invoke, openUrl, calls, opened };
}

// startConnect sends only { provider, returnTo } and opens the returned URL.
{
  const { invoke, openUrl, calls, opened } = fakes("https://accounts.google.com/o/oauth2/auth?x=1");
  const result = await startConnect(invoke, openUrl, "youtube" as Provider);

  assert.equal(calls.length, 1, "exactly one invoke");
  assert.equal(calls[0].command, "social_oauth_start");
  assert.deepEqual(
    calls[0].args,
    { args: { provider: "youtube", returnTo: "/" } },
    "no client_id / redirect_uri / state / timestamps are sent",
  );
  assert.deepEqual(opened, ["https://accounts.google.com/o/oauth2/auth?x=1"]);
  assert.equal(result.authorizationUrl, "https://accounts.google.com/o/oauth2/auth?x=1");
}

// A custom returnTo is forwarded.
{
  const { invoke, openUrl, calls } = fakes("https://x");
  await startConnect(invoke, openUrl, "tiktok" as Provider, "/campaigns");
  assert.deepEqual(calls[0].args, {
    args: { provider: "tiktok", returnTo: "/campaigns" },
  });
}

console.log("social-accounts-connect.test.ts: ok");
