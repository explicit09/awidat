import { snapMoveDeltaS, type UserMoveDrag, type UserTrimDrag } from "./editMath";
import { useMediaStore } from "../media/store";
import type { TimelineSnapshot } from "./store";

export function UserTrimTooltip({
  drag,
  pps,
}: {
  drag: UserTrimDrag;
  pps: number;
}) {
  const dxPx = drag.currentX - drag.startX;
  const dxS = dxPx / Math.max(0.001, pps);
  const proposed =
    drag.hit.side === "start"
      ? Math.max(0, drag.hit.sourceStart + dxS)
      : Math.max(drag.hit.sourceStart + 0.1, drag.hit.sourceEnd + dxS);
  const label = `${drag.hit.side}: ${proposed.toFixed(2)}s`;
  return (
    <div className="user-trim-tooltip" style={{ left: drag.currentX }}>
      {label}
    </div>
  );
}

export function UserMoveTooltip({
  drag,
  snapshot,
  pps,
}: {
  drag: UserMoveDrag;
  snapshot: TimelineSnapshot;
  pps: number;
}) {
  const currentTime = useMediaStore((state) => state.timelineTime);
  const dxS = snapMoveDeltaS(snapshot, currentTime, drag, pps);
  return (
    <div className="user-trim-tooltip" style={{ left: drag.currentX }}>
      move {dxS >= 0 ? "+" : ""}
      {dxS.toFixed(2)}s
    </div>
  );
}
