export type PreviewLutData = {
  size: number;
  domainMin: [number, number, number];
  domainMax: [number, number, number];
  rgba: Uint8Array;
};

const cache = new Map<string, Promise<PreviewLutData | null>>();

export function previewLutCacheKey(projectRoot: string | null, lutPath: string): string {
  return `${projectRoot ?? ""}\u0000${lutPath}`;
}

export function fetchPreviewLut(
  projectRoot: string | null,
  lutPath: string,
  load: () => Promise<PreviewLutData | null>,
): Promise<PreviewLutData | null> {
  const key = previewLutCacheKey(projectRoot, lutPath);
  let pending = cache.get(key);
  if (!pending) {
    pending = load();
    cache.set(key, pending);
  }
  return pending;
}

export function clearPreviewLutCache(): void {
  cache.clear();
}
