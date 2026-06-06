import { strict as assert } from "node:assert";

import {
  accountLabelForProvider,
  selectedAccountIdForProvider,
  selectedAccountIdsForProviders,
  shouldUploadRenderTarget,
  uploadCapableAccountsForProvider,
  useUploadAccountSelections,
  type UploadAccountOption,
} from "../src/state/uploadAccountSelections.ts";

const accounts: UploadAccountOption[] = [
  {
    id: "acct_yt_a",
    provider: "youtube",
    displayName: "Main YouTube",
    capabilities: { uploadVideo: true },
  },
  {
    id: "acct_yt_b",
    provider: "youtube",
    displayName: "Clips YouTube",
    capabilities: { uploadVideo: true },
  },
  {
    id: "acct_tt_viewer",
    provider: "tiktok",
    displayName: "TikTok viewer",
    capabilities: { uploadVideo: false },
  },
  {
    id: "acct_tt",
    provider: "tiktok",
    displayName: "TikTok",
    capabilities: { uploadVideo: true },
  },
];

assert.deepEqual(
  uploadCapableAccountsForProvider(accounts, "youtube").map((account) => account.id),
  ["acct_yt_a", "acct_yt_b"],
);
assert.equal(
  selectedAccountIdForProvider(accounts, "youtube", "acct_yt_b"),
  "acct_yt_b",
);
assert.equal(
  selectedAccountIdForProvider(accounts, "youtube", "missing"),
  "acct_yt_a",
);
assert.equal(
  selectedAccountIdForProvider(accounts, "instagram", undefined),
  undefined,
);
assert.equal(
  accountLabelForProvider(accounts, "youtube", "acct_yt_b"),
  "Clips YouTube",
);
assert.equal(
  accountLabelForProvider(accounts, "youtube", "missing"),
  "Main YouTube",
);
assert.equal(
  accountLabelForProvider(accounts, "instagram", undefined),
  "No upload-capable account",
);
assert.deepEqual(
  selectedAccountIdsForProviders(accounts, ["youtube", "tiktok"], {
    youtube: "acct_yt_b",
  }),
  {
    youtube: "acct_yt_b",
    tiktok: "acct_tt",
  },
);

useUploadAccountSelections.setState({ byProvider: {} });
useUploadAccountSelections.getState().setSelected("youtube", "acct_yt_b");
assert.deepEqual(useUploadAccountSelections.getState().byProvider, {
  youtube: "acct_yt_b",
});
useUploadAccountSelections.getState().clearSelected("youtube");
assert.deepEqual(useUploadAccountSelections.getState().byProvider, {});

assert.equal(
  shouldUploadRenderTarget("youtube", new Set(["tiktok"]), new Set(["youtube"])),
  false,
  "implicit YouTube master render must not upload when YouTube was not selected",
);
assert.equal(
  shouldUploadRenderTarget("youtube", new Set(["youtube", "tiktok"]), new Set(["youtube"])),
  true,
  "selected YouTube target can upload when upload is enabled",
);
assert.equal(
  shouldUploadRenderTarget("tiktok", new Set(["tiktok"]), new Set(["tiktok"])),
  true,
  "selected TikTok target can upload when upload is enabled",
);

console.log("upload-account-selection: OK");
