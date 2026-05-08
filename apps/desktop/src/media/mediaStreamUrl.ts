import { invoke } from "@tauri-apps/api/core";

const urlCache = new Map<string, string>();
const inflight = new Map<string, Promise<string>>();

export function cachedMediaStreamUrl(path: string): string | undefined {
  return urlCache.get(path);
}

export async function mediaStreamUrl(path: string): Promise<string> {
  const cached = urlCache.get(path);
  if (cached) return cached;
  const pending = inflight.get(path);
  if (pending) return pending;
  const promise = invoke<string>("media_url_for_path", { path }).then((url) => {
    urlCache.set(path, url);
    inflight.delete(path);
    return url;
  });
  inflight.set(path, promise);
  return promise;
}
