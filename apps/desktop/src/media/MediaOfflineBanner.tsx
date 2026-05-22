// Banner listing every timeline clip whose source asset is missing
// on disk. Each row exposes a "Locate…" button that opens a file
// picker; the picked path is sent to `relink_missing_asset` which
// rewrites the OTIO so the affected clips point at the new location.
//
// Distinct from a per-clip overlay because the segmented player
// already drops `playable_kind === "missing"` clips — the banner is
// the only surface where the user can actually recover them.

import { useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { AlertTriangle } from "lucide-react";
import { useTimelineStore } from "../timeline/store";
import type { TimelineSnapshot } from "../timeline/store";

type MissingEntry = {
  assetId: string;
  clipCount: number;
};

export function MediaOfflineBanner() {
  const snapshot = useTimelineStore((s) => s.snapshot);
  const refresh = useTimelineStore((s) => s.refresh);
  const [busyAssetId, setBusyAssetId] = useState<string | null>(null);
  const [errorPerAsset, setErrorPerAsset] = useState<Record<string, string>>(
    {},
  );

  const entries = useMemo(() => collectMissing(snapshot), [snapshot]);
  if (entries.length === 0) return null;

  async function locate(assetId: string) {
    setBusyAssetId(assetId);
    setErrorPerAsset((prev) => {
      const next = { ...prev };
      delete next[assetId];
      return next;
    });
    try {
      const picked = await openDialog({
        directory: false,
        multiple: false,
        title: `Locate ${assetId}`,
      });
      if (typeof picked !== "string") {
        setBusyAssetId(null);
        return;
      }
      await invoke<number>("relink_missing_asset", {
        oldAssetId: assetId,
        newAssetId: picked,
      });
      await refresh().catch(() => {});
    } catch (e) {
      setErrorPerAsset((prev) => ({ ...prev, [assetId]: String(e) }));
    } finally {
      setBusyAssetId(null);
    }
  }

  return (
    <div className="media-offline-banner" role="status" aria-live="polite">
      <header>
        <AlertTriangle size={14} strokeWidth={2} aria-hidden />
        <strong>
          {entries.length} {entries.length === 1 ? "clip" : "clips"} offline
        </strong>
        <span className="media-offline-banner-hint">
          Source media moved or was deleted. Locate to restore.
        </span>
      </header>
      <ul>
        {entries.map((entry) => (
          <li key={entry.assetId}>
            <code className="media-offline-banner-path" title={entry.assetId}>
              {entry.assetId}
            </code>
            <span className="media-offline-banner-count">
              {entry.clipCount === 1
                ? "1 clip"
                : `${entry.clipCount} clips`}
            </span>
            <button
              type="button"
              onClick={() => locate(entry.assetId)}
              disabled={busyAssetId !== null}
            >
              {busyAssetId === entry.assetId ? "Locating…" : "Locate…"}
            </button>
            {errorPerAsset[entry.assetId] && (
              <span className="media-offline-banner-error">
                {errorPerAsset[entry.assetId]}
              </span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

function collectMissing(snapshot: TimelineSnapshot): MissingEntry[] {
  const counts = new Map<string, number>();
  for (const track of snapshot.tracks ?? []) {
    for (const item of track.items ?? []) {
      if (item.kind !== "clip") continue;
      if (item.playable_kind !== "missing") continue;
      if (!item.asset_id) continue;
      counts.set(item.asset_id, (counts.get(item.asset_id) ?? 0) + 1);
    }
  }
  return Array.from(counts, ([assetId, clipCount]) => ({ assetId, clipCount }))
    .sort((a, b) => a.assetId.localeCompare(b.assetId));
}
