import { resolvePreviewMedia } from "../src/media/previewSource";
import type { ProxyEntry, SourceMediaEntry } from "../src/media/store";

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

const source: SourceMediaEntry = {
  id: "raw/interview.mov",
  name: "interview.mov",
  path: "/project/raw/interview.mov",
  size_bytes: 1_000,
};

const proxy: ProxyEntry = {
  stem: "interview-1080p-abcdef12",
  proxy_path: "/project/.awidat/proxies/interview-1080p-abcdef12.mp4",
  size_bytes: 500,
};

const auto = resolvePreviewMedia({ mode: "auto", source, proxy });
assert(auto?.path === source.path, "auto preview should prefer original source media");
assert(auto?.mode === "auto", "auto preview should keep its mode label");

const full = resolvePreviewMedia({ mode: "full", source, proxy });
assert(full?.path === source.path, "full preview should use original source media");

const explicitProxy = resolvePreviewMedia({ mode: "proxy", source, proxy });
assert(explicitProxy?.path === proxy.proxy_path, "proxy preview should use proxy media only when explicitly requested");

const fallback = resolvePreviewMedia({ mode: "auto", source: null, proxy });
assert(fallback?.path === proxy.proxy_path, "auto preview should fall back to proxy when source is unavailable");
