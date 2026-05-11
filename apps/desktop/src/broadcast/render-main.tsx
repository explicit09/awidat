import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import "../App.css";
import { TimelineBroadcastOverlay } from "../media/SegmentedVideoView";
import type { BroadcastOverlayConfig } from "../protocol";

type RenderPayload = {
  overlay: BroadcastOverlayConfig;
  projectRoot: string;
  width: number;
  height: number;
};

declare global {
  interface Window {
    __AWIDAT_OVERLAY_PAYLOAD__?: RenderPayload;
    __AWIDAT_SET_OVERLAY_TIME__?: (time: number) => void;
    __AWIDAT_OVERLAY_READY__?: boolean;
  }
}

function fileAssetUrl(projectRoot: string | null, relPath: string | null): string | null {
  if (!projectRoot || !relPath) return null;
  if (relPath.startsWith("/") || relPath.includes("..")) return null;
  const encoded = relPath.split("/").map(encodeURIComponent).join("/");
  return `/__awidat_asset__/${encoded}`;
}

function OverlayRenderApp({ payload }: { payload: RenderPayload }) {
  const [time, setTime] = useState(0);

  useEffect(() => {
    window.__AWIDAT_SET_OVERLAY_TIME__ = setTime;
    window.__AWIDAT_OVERLAY_READY__ = true;
    return () => {
      delete window.__AWIDAT_SET_OVERLAY_TIME__;
      window.__AWIDAT_OVERLAY_READY__ = false;
    };
  }, []);

  return (
    <div
      className="broadcast-render-root"
      style={{
        width: `${payload.width}px`,
        height: `${payload.height}px`,
      }}
    >
      <TimelineBroadcastOverlay
        overlay={payload.overlay}
        timelineTime={time}
        projectRoot={payload.projectRoot}
        resolveAssetUrl={fileAssetUrl}
      />
    </div>
  );
}

const root = document.getElementById("root");
const payload = window.__AWIDAT_OVERLAY_PAYLOAD__;

if (!root || !payload) {
  throw new Error("missing Awidat overlay render payload");
}

createRoot(root).render(<OverlayRenderApp payload={payload} />);
