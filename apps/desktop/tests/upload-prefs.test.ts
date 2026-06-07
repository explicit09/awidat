import { strict as assert } from "node:assert";
import { shouldApplyBackendUploadPrefs } from "../src/state/uploadPrefs.ts";

assert.equal(shouldApplyBackendUploadPrefs(0, 0), true);
assert.equal(
  shouldApplyBackendUploadPrefs(0, 1),
  false,
  "backend hydrate must not overwrite a local edit made after scheduling",
);

console.log("upload-prefs: OK");
