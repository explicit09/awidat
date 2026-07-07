import type { CSSProperties } from "react";
import type { TimelineSnapshot } from "../../timeline/store";
import type { PreviewTransition, VideoOverlaySegment } from "../../timeline/usePlaySegments";
import type { StageClock } from "./stageClock";
import {
  GpuTransitionPreview,
  TimelineTransitionColorOverlay,
  TimelineTransitionOverlay,
} from "./transitions";
import { TimelineTitleOverlays, type PreviewTitleOverlay } from "./titles";
import { TimelineMotionSceneOverlays, type PreviewMotionSceneOverlay } from "./motionScene";
import { TimelineVideoOverlays } from "./videoOverlays";
import { TimelineBroadcastOverlay } from "./broadcast";

export type StageProps = {
  clock: StageClock;
  programFrameCss: CSSProperties;
  programFrameSize: { width: number; height: number };
  projectRoot: string | null;
  videoOverlays: VideoOverlaySegment[];
  transition: PreviewTransition | null;
  renderTransitionOnGpu: boolean;
  titles: PreviewTitleOverlay[];
  motionSceneLayers: PreviewMotionSceneOverlay[];
  broadcastOverlay: TimelineSnapshot["broadcast_overlay"];
  showGap: boolean;
};

export function Stage({
  clock,
  programFrameCss,
  programFrameSize,
  projectRoot,
  videoOverlays,
  transition,
  renderTransitionOnGpu,
  titles,
  motionSceneLayers,
  broadcastOverlay,
  showGap,
}: StageProps) {
  const timelineTime = clock.now();
  const isPlaying = clock.isPlaying();
  const cssTransition = renderTransitionOnGpu ? null : transition;

  return (
    <div className="timeline-program-frame" style={programFrameCss}>
      <TimelineVideoOverlays
        overlays={videoOverlays}
        timelineTime={timelineTime}
        isPlaying={isPlaying}
      />
      {showGap && <TimelineGapOverlay />}
      <TimelineTransitionOverlay
        transition={cssTransition}
        timelineTime={timelineTime}
        isPlaying={isPlaying}
      />
      <TimelineTransitionColorOverlay transition={cssTransition} timelineTime={timelineTime} />
      <GpuTransitionPreview
        transition={transition}
        timelineTime={timelineTime}
        width={programFrameSize.width}
        height={programFrameSize.height}
      />
      <TimelineTitleOverlays overlays={titles} timelineTime={timelineTime} />
      <TimelineMotionSceneOverlays overlays={motionSceneLayers} timelineTime={timelineTime} />
      <TimelineBroadcastOverlay
        overlay={broadcastOverlay}
        timelineTime={timelineTime}
        projectRoot={projectRoot}
        previewFrameSize={programFrameSize}
      />
    </div>
  );
}

function TimelineGapOverlay() {
  return (
    <div
      style={{
        position: "absolute",
        inset: 0,
        background: "#000",
        pointerEvents: "none",
        zIndex: 2,
      }}
      aria-hidden="true"
    />
  );
}
