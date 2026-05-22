// Bottom-row timeline shell. Owns project refresh and delegates the editing surface.

import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { useTimelineStore } from "./store";
import { useMediaStore } from "../media/store";
import { useAgentStore } from "../agent/store";
import { useProjectStore } from "../app/state";
import { ProposalActions } from "./ProposalActions";
import { TIMELINE_CHANGED_EVENT } from "../protocol";
import { TimelineSurface } from "./TimelineSurface.tsx";
import { countCompletedTimelineEdits } from "./refreshActivity.ts";

export function TimelinePane() {
  const projectReady = useProjectStore((s) => s.current !== null);
  const projectRoot = useProjectStore((s) => s.current);
  const snapshot = useTimelineStore((s) => s.snapshot);
  const zoom = useTimelineStore((s) => s.zoom);
  const refresh = useTimelineStore((s) => s.refresh);
  const items = useAgentStore((s) => s.items);
  // The canvas is a timeline-time surface; the playhead should track
  // the timeline-time clock the SegmentedVideoView drives, not the
  // source-time of whatever proxy happens to be loaded.
  const currentTime = useMediaStore((s) => s.timelineTime);

  // Refresh on mount + on project change.
  useEffect(() => {
    if (projectReady) {
      refresh();
    }
  }, [projectReady, projectRoot, refresh]);

  useEffect(() => {
    const unlisten = listen<string>(TIMELINE_CHANGED_EVENT, (event) => {
      if (useProjectStore.getState().current === event.payload) {
        refresh();
      }
    });
    return () => {
      unlisten.then((u) => u());
    };
  }, [refresh]);

  // Refresh after every completed apply_edl OR every completed
  // proposed_edit. Both paths can mutate the OTIO on disk:
  //   - apply_edl Completed lands when the agent's tool handler
  //     finishes (Allow path: agent wrote the file).
  //   - proposed_edit Completed lands when a proposal accept /
  //     reject finishes (Deny-with-adjustment path: desktop wrote
  //     the file, no agent tool ran; user-initiated edits via
  //     propose_user_edit also take this path).
  // We watch a stable scalar (count of completions) rather than
  // the full items array so React doesn't re-fire the effect on
  // every text delta.
  const completedEdits = countCompletedTimelineEdits(items);
  useEffect(() => {
    if (projectReady && completedEdits > 0) {
      refresh();
    }
  }, [completedEdits, projectReady, refresh]);

  if (!projectReady) {
    return null;
  }

  return (
    <section className="timeline-pane">
      <header className="timeline-header">
        <span className="timeline-label">Timeline</span>
        <span className="timeline-meta">
          {snapshot.tracks.length === 0
            ? "no tracks yet"
            : `${snapshot.duration_s.toFixed(1)}s · ${snapshot.tracks.length} track${snapshot.tracks.length === 1 ? "" : "s"}`}
        </span>
      </header>
      <div className="timeline-stage">
        <TimelineSurface snapshot={snapshot} currentTime={currentTime} zoom={zoom} />
        <ProposalActions />
      </div>
    </section>
  );
}
