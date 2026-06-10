import { strict as assert } from "node:assert";
import { composerAuthGateState } from "../src/agent/composerAuthGate.ts";
import type { AuthStatus } from "../src/state/auth.ts";

const signedOut: AuthStatus = {
  mode: "none",
  walletTitle: "No auth",
  walletDetail: "Not signed in",
  accountHint: null,
  viaEnv: false,
  envVar: null,
};

const apiKey: AuthStatus = {
  mode: "api_key",
  walletTitle: "API key",
  walletDetail: "User key",
  accountHint: "sk-...",
  viaEnv: false,
  envVar: null,
};

assert.deepEqual(
  composerAuthGateState({ projectReady: true, running: false, authStatus: signedOut }),
  {
    authReady: false,
    disabledReason: "Sign in to get started",
    textareaDisabled: true,
    sendDisabled: false,
    sendLabel: "Sign in to get started",
    placeholder: "Sign in to get started",
  },
);

assert.deepEqual(
  composerAuthGateState({ projectReady: true, running: false, authStatus: apiKey, text: "cut intro" }),
  {
    authReady: true,
    disabledReason: null,
    textareaDisabled: false,
    sendDisabled: false,
    sendLabel: "Send",
    placeholder: "Message Montage",
  },
);

assert.equal(
  composerAuthGateState({ projectReady: false, running: false, authStatus: apiKey }).placeholder,
  "Open a project to start chatting",
);

console.log("composer-auth-gate: OK");
