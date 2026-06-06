import { strict as assert } from "node:assert";

import { summarizeEditorPublishing } from "../src/editor/publishingBridge.ts";

assert.deepEqual(
  summarizeEditorPublishing({
    selectedTargets: new Set(),
    uploadTargets: new Set(),
    accountSelections: {},
  }),
  {
    selectedCount: 0,
    uploadCount: 0,
    accountCount: 0,
    readyToExport: false,
    copy: "No delivery targets selected",
  },
);

assert.deepEqual(
  summarizeEditorPublishing({
    selectedTargets: new Set(["youtube", "captions"]),
    uploadTargets: new Set(["youtube", "tiktok"]),
    accountSelections: { youtube: "acct-youtube" },
  }),
  {
    selectedCount: 2,
    uploadCount: 1,
    accountCount: 1,
    readyToExport: true,
    copy: "1 social upload selected · 1 account set",
  },
);

assert.deepEqual(
  summarizeEditorPublishing({
    selectedTargets: new Set(["youtube", "tiktok"]),
    uploadTargets: new Set(["youtube", "tiktok"]),
    accountSelections: { youtube: "acct-youtube" },
  }),
  {
    selectedCount: 2,
    uploadCount: 2,
    accountCount: 1,
    readyToExport: true,
    copy: "2 social uploads selected · 1 account set",
  },
);

console.log("editor-publishing-bridge: OK");
