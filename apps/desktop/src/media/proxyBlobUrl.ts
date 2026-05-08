// Tauri's `asset://` protocol on macOS WKWebView decodes video bytes
// but doesn't surface audio reliably to <video> elements (audioTracks
// reports 0; sound is silent). Fetching the same URL through `fetch`
// and feeding the bytes back via `URL.createObjectURL` gives WKWebView
// a regular blob source where audio decoding works as expected.
//
// Cache the blob URLs per proxy path. Each blob URL holds a reference
// to its bytes in WebKit's blob registry until we revoke it; we never
// revoke today (a session lives long enough that re-fetching mid-edit
// is the worse cost). Phase B follow-up: revoke when the proxy file
// gets superseded by a newer transcode.

import { convertFileSrc } from "@tauri-apps/api/core";

const proxyBlobCache = new Map<string, string>(); // proxyPath -> blob URL
const proxyBlobInflight = new Map<string, Promise<string>>(); // dedupe parallel fetches

/** Synchronous cache hit (or undefined). Use this for the fast path
 *  in render functions; call `blobUrlForProxy` to populate. */
export function cachedProxyBlobUrl(proxyPath: string): string | undefined {
  return proxyBlobCache.get(proxyPath);
}

/** Fetch the proxy via the asset protocol and stash a blob URL.
 *  Idempotent: parallel calls for the same path coalesce. */
export async function blobUrlForProxy(proxyPath: string): Promise<string> {
  const cached = proxyBlobCache.get(proxyPath);
  if (cached) return cached;
  const inflight = proxyBlobInflight.get(proxyPath);
  if (inflight) return inflight;
  const promise = (async () => {
    const assetUrl = convertFileSrc(proxyPath);
    const resp = await fetch(assetUrl);
    if (!resp.ok) {
      throw new Error(`proxy fetch ${resp.status}: ${proxyPath}`);
    }
    const blob = await resp.blob();
    const url = URL.createObjectURL(blob);
    proxyBlobCache.set(proxyPath, url);
    proxyBlobInflight.delete(proxyPath);
    return url;
  })();
  proxyBlobInflight.set(proxyPath, promise);
  return promise;
}
