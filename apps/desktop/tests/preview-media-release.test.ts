import { strict as assert } from "node:assert";

const mediaRelease = await import("../src/media/previewMediaRelease.ts").catch(() => null);

assert.ok(mediaRelease, "preview media release module must exist");

const calls: string[] = [];
const element = {
  pause() {
    calls.push("pause");
  },
  removeAttribute(name: string) {
    calls.push(`remove:${name}`);
  },
  load() {
    calls.push("load");
  },
};

mediaRelease.releasePreviewMediaElement(element);

assert.deepEqual(
  calls,
  ["pause", "remove:src", "load"],
  "unmount must stop playback and force WebKit to release its decoder",
);

console.log("preview-media-release: all assertions passed");

const disconnected: string[] = [];
let closedContexts = 0;
class FakeAudioNode {
  gain = {
    value: 1,
    setTargetAtTime() {},
  };

  connect() {}

  disconnect() {
    disconnected.push("disconnect");
  }
}

class FakeAudioContext {
  state = "suspended";
  currentTime = 0;
  destination = {};

  createMediaElementSource() {
    return new FakeAudioNode();
  }

  createGain() {
    return new FakeAudioNode();
  }

  async close() {
    closedContexts += 1;
  }
}

(globalThis as typeof globalThis & { AudioContext: typeof FakeAudioContext }).AudioContext =
  FakeAudioContext;

const audioGraph = await import("../src/media/previewAudioGraph.ts");
assert.equal(
  audioGraph.setPreviewElementGain(element, 1),
  false,
  "unity gain must stay on the native media element without allocating WebAudio",
);
assert.equal(closedContexts, 0);
assert.equal(audioGraph.setPreviewElementGain(element, 2), true);
assert.equal(
  typeof audioGraph.releasePreviewElementGain,
  "function",
  "preview audio graph must expose an explicit element release",
);
audioGraph.releasePreviewElementGain(element);
assert.equal(disconnected.length, 2, "source and gain nodes must disconnect on unmount");
assert.equal(closedContexts, 1, "the idle audio context must close after its last video unmounts");
