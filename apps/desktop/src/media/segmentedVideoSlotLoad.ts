export type SegmentSlotLoadState = {
  loadToken: number;
  wantedPath: string | null;
};

export type SegmentSlotLoadRequest = {
  token: number;
  wantedPath: string;
};

export const HAVE_FUTURE_DATA = 3;

export function beginSlotLoad(
  slot: SegmentSlotLoadState,
  wantedPath: string,
): SegmentSlotLoadRequest {
  const token = slot.loadToken + 1;
  slot.loadToken = token;
  slot.wantedPath = wantedPath;
  return { token, wantedPath };
}

export function clearSlotLoad(slot: SegmentSlotLoadState): void {
  slot.loadToken += 1;
  slot.wantedPath = null;
}

export function isCurrentSlotLoad(
  slot: SegmentSlotLoadState,
  request: SegmentSlotLoadRequest,
): boolean {
  return slot.loadToken === request.token && slot.wantedPath === request.wantedPath;
}

export function mediaHasFutureData(readyState: number): boolean {
  return readyState >= HAVE_FUTURE_DATA;
}

export function shouldStartMediaPlayback({
  isPlaying,
  paused,
  readyState,
}: {
  isPlaying: boolean;
  paused: boolean;
  readyState: number;
}): boolean {
  return isPlaying && paused && mediaHasFutureData(readyState);
}
