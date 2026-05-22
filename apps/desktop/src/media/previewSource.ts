import type { ProxyEntry, SourceMediaEntry } from "./store";

export type PreviewQualityMode = "auto" | "full" | "proxy";

export type PreviewMediaChoice = {
  path: string;
  label: string;
  name: string;
  mode: PreviewQualityMode;
};

export function resolvePreviewMedia({
  mode,
  source,
  proxy,
}: {
  mode: PreviewQualityMode;
  source: SourceMediaEntry | null;
  proxy: ProxyEntry | null;
}): PreviewMediaChoice | null {
  if (mode === "proxy") {
    return proxy
      ? {
          path: proxy.proxy_path,
          label: "Source proxy",
          name: proxy.stem,
          mode,
        }
      : source
        ? {
            path: source.path,
            label: "Source media",
            name: source.name,
            mode: "full",
          }
        : null;
  }

  if (source) {
    return {
      path: source.path,
      label: mode === "full" ? "Source media" : "Source media · Auto",
      name: source.name,
      mode,
    };
  }

  if (proxy) {
    return {
      path: proxy.proxy_path,
      label: "Source proxy",
      name: proxy.stem,
      mode: "proxy",
    };
  }

  return null;
}
