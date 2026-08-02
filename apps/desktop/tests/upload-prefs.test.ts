import { strict as assert } from "node:assert";
import {
  providerKeyForTarget,
  useUploadPrefs,
} from "../src/state/uploadPrefs.ts";

const state = useUploadPrefs.getState();
assert.equal("hydrate" in state, false, "local preferences need no backend hydrate");
assert.equal("revision" in state, false, "local preferences need no race counter");
assert.equal(providerKeyForTarget("youtube"), "youtube");
assert.equal(providerKeyForTarget("twitter_x"), "twitter_x");
assert.equal(providerKeyForTarget("video_master"), null);

state.setEnabled([]);
state.toggle("youtube");
assert.deepEqual([...useUploadPrefs.getState().enabled], ["youtube"]);
useUploadPrefs.getState().toggle("youtube");
assert.deepEqual([...useUploadPrefs.getState().enabled], []);

console.log("upload-prefs: OK");
