import { useEffect, useMemo } from "react";
import { useMediaStore } from "../media/store";
import { useTimelineStore } from "../timeline/store";
import {
  findActiveSegment,
  usePlaySegments,
} from "../timeline/usePlaySegments";
import { useTranscriptStore } from "./store";
import { TranscriptView } from "./TranscriptView";

export function TranscriptSidebar() {
  const selectedStem = useMediaStore((s) => s.selectedStem);
  const timelineTime = useMediaStore((s) => s.timelineTime);
  const timelineDurationS = useTimelineStore((s) => s.snapshot.duration_s);
  const segments = usePlaySegments();
  const setActiveStem = useTranscriptStore((s) => s.setActiveStem);

  const activeStem = useMemo(() => {
    if (timelineDurationS > 0) {
      const idx = findActiveSegment(segments, timelineTime);
      if (idx >= 0) return segments[idx].proxyStem;
      return segments[segments.length - 1]?.proxyStem ?? null;
    }
    return selectedStem;
  }, [segments, selectedStem, timelineDurationS, timelineTime]);

  useEffect(() => {
    setActiveStem(activeStem);
  }, [activeStem, setActiveStem]);

  return (
    <div className="sidebar-pane transcript-sidebar">
      {activeStem ? (
        <TranscriptView stem={activeStem} />
      ) : (
        <p className="chat-empty chat-empty-loaded">
          Select media or load a timeline to inspect its transcript.
        </p>
      )}
    </div>
  );
}
