/**
 * FacesDrillDown — grid of detected faces grouped by identity cluster.
 * The face-mcp sidecar carries bounding boxes but no thumbnail crops;
 * we render placeholder tiles labelled by detection time and identity
 * id. Once a thumbnail pipeline ships, swap the placeholder for an
 * `<img src={face.thumbnail_path} />`.
 */

import { useMemo } from "react";
import { useFaces } from "../../../state/evidenceAccessors";
import { DrillDownEmpty, formatTimecode, jumpTimelineTo } from "./DrillDownModal";
import type { EvidenceFace } from "../../../state/evidenceAccessors";

interface FacesDrillDownProps {
  onRunIndexers?: () => void;
}

export function FacesDrillDown({ onRunIndexers }: FacesDrillDownProps) {
  const faces = useFaces();
  const grouped = useMemo(() => groupByIdentity(faces), [faces]);

  if (faces.length === 0) {
    return (
      <DrillDownEmpty
        message="No faces detected yet. Run the face indexer first."
        onRunIndexers={onRunIndexers}
      />
    );
  }

  return (
    <div className="flex flex-col gap-3" aria-label={`${faces.length} face detections`}>
      {grouped.map(({ identity, faces: faceList }) => (
        <section key={identity} className="flex flex-col gap-1.5">
          <header className="flex items-baseline justify-between gap-2">
            <span className="text-[var(--text-label)] uppercase tracking-[var(--text-label--letter-spacing)] text-[var(--color-text-muted)]">
              {identity}
            </span>
            <span className="font-mono tabular-nums text-[var(--text-caption)] text-[var(--color-text-muted)]">
              {faceList.length}
            </span>
          </header>
          <div
            className="grid gap-1.5"
            style={{
              gridTemplateColumns: "repeat(auto-fill, minmax(72px, 1fr))",
            }}
            role="list"
          >
            {faceList.slice(0, 24).map((face) => (
              <button
                key={face.id}
                type="button"
                role="listitem"
                onClick={() => jumpTimelineTo(face.at_s)}
                className="flex flex-col gap-0.5 p-1 rounded-[var(--radius-xs)] border border-[var(--color-border-subtle)] bg-[var(--color-surface-card)] hover:bg-[var(--color-surface-card-hover)]"
                title={`Jump to ${formatTimecode(face.at_s)}`}
              >
                <div
                  className="aspect-square w-full rounded-[var(--radius-xs)] bg-[var(--color-surface-input)] grid place-items-center text-[var(--text-caption)] font-mono text-[var(--color-text-muted)]"
                  aria-hidden="true"
                >
                  {formatBox(face.bbox)}
                </div>
                <span className="font-mono text-[var(--text-caption)] tabular-nums text-[var(--color-text-muted)] text-center">
                  {formatTimecode(face.at_s)}
                </span>
              </button>
            ))}
          </div>
          {faceList.length > 24 ? (
            <span className="text-[var(--text-caption)] text-[var(--color-text-muted)]">
              + {faceList.length - 24} more
            </span>
          ) : null}
        </section>
      ))}
    </div>
  );
}

function groupByIdentity(
  faces: ReadonlyArray<EvidenceFace>,
): Array<{ identity: string; faces: EvidenceFace[] }> {
  const map = new Map<string, EvidenceFace[]>();
  for (const face of faces) {
    const key = face.identity_id ?? "unknown";
    const list = map.get(key);
    if (list) {
      list.push(face);
    } else {
      map.set(key, [face]);
    }
  }
  return Array.from(map.entries()).map(([identity, faceList]) => ({
    identity,
    faces: faceList,
  }));
}

function formatBox(bbox: EvidenceFace["bbox"]): string {
  return `${Math.round(bbox.w)}×${Math.round(bbox.h)}`;
}
