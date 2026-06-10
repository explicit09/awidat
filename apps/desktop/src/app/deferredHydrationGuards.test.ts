import assert from "node:assert/strict";

import { shouldStartDeferredIntro } from "./deferredHydrationGuards.ts";

const readyBase = {
  scheduledProject: "/project",
  currentProject: "/project",
  introduced: false,
  running: false,
  itemCount: 0,
  mediaSourceCount: 1,
  mediaProxyCount: 0,
  authReady: true,
  welcomeConsentReady: true,
};

assert.equal(
  shouldStartDeferredIntro(readyBase),
  true,
  "eligible project should start the deferred intro",
);

assert.equal(
  shouldStartDeferredIntro({ ...readyBase, authReady: false }),
  false,
  "intro must wait until auth is ready",
);

assert.equal(
  shouldStartDeferredIntro({ ...readyBase, welcomeConsentReady: false }),
  false,
  "intro must wait until first-run consent is accepted",
);

console.log("deferred-hydration-guards: OK");
