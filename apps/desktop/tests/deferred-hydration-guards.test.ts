import { strict as assert } from "node:assert";
import {
  shouldReplaceDeferredChatHistory,
  shouldStartDeferredIntro,
} from "../src/app/deferredHydrationGuards.ts";

assert.equal(
  shouldReplaceDeferredChatHistory({
    scheduledProject: "/p",
    currentProject: "/p",
    scheduledItemCount: 0,
    currentItemCount: 0,
    running: false,
  }),
  true,
);

assert.equal(
  shouldReplaceDeferredChatHistory({
    scheduledProject: "/p",
    currentProject: "/p",
    scheduledItemCount: 0,
    currentItemCount: 1,
    running: false,
  }),
  false,
  "new live items must block stale history replacement",
);

assert.equal(
  shouldReplaceDeferredChatHistory({
    scheduledProject: "/p",
    currentProject: "/p",
    scheduledItemCount: 0,
    currentItemCount: 0,
    running: true,
  }),
  false,
  "running turn must block stale history replacement",
);

assert.equal(
  shouldStartDeferredIntro({
    scheduledProject: "/p",
    currentProject: "/p",
    introduced: false,
    running: false,
    itemCount: 0,
    mediaSourceCount: 1,
    mediaProxyCount: 0,
    authReady: true,
    welcomeConsentReady: false,
  }),
  false,
  "opening a media project must not start a hidden chat turn",
);

assert.equal(
  shouldStartDeferredIntro({
    scheduledProject: "/p",
    currentProject: "/p",
    introduced: false,
    running: false,
    itemCount: 0,
    mediaSourceCount: 1,
    mediaProxyCount: 0,
    authReady: false,
    welcomeConsentReady: true,
  }),
  false,
  "intro must wait until auth is ready",
);

assert.equal(
  shouldStartDeferredIntro({
    scheduledProject: "/p",
    currentProject: "/p",
    introduced: false,
    running: false,
    itemCount: 0,
    mediaSourceCount: 1,
    mediaProxyCount: 0,
    authReady: true,
    welcomeConsentReady: true,
  }),
  true,
  "eligible project should start the deferred intro",
);

assert.equal(
  shouldStartDeferredIntro({
    scheduledProject: "/p",
    currentProject: "/p",
    introduced: false,
    running: true,
    itemCount: 0,
    mediaSourceCount: 1,
    mediaProxyCount: 0,
    authReady: true,
    welcomeConsentReady: true,
  }),
  false,
  "user-started turn must block deferred intro",
);

assert.equal(
  shouldStartDeferredIntro({
    scheduledProject: "/p",
    currentProject: "/p",
    introduced: false,
    running: false,
    itemCount: 1,
    mediaSourceCount: 1,
    mediaProxyCount: 0,
    authReady: true,
    welcomeConsentReady: true,
  }),
  false,
  "new user-visible items must block deferred intro",
);

console.log("deferred-hydration-guards: OK");
