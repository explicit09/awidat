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
  }),
  true,
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
  }),
  false,
  "new user-visible items must block deferred intro",
);

console.log("deferred-hydration-guards: OK");
