/**
 * Pure helpers shared by every drill-down. Pulled out of
 * `DrillDownModal.tsx` so a `.ts` test can import them without the
 * React renderer (node --experimental-strip-types refuses `.tsx`).
 */

/**
 * Format `t` seconds as `m:ss` (or `h:mm:ss` past one hour). Returns
 * `0:00` for negative / non-finite input so the modal never renders a
 * NaN cell.
 */
export function formatTimecode(t: number): string {
  if (!Number.isFinite(t) || t < 0) return "0:00";
  const total = Math.floor(t);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const mm = hours > 0 ? String(minutes).padStart(2, "0") : String(minutes);
  const ss = String(seconds).padStart(2, "0");
  return hours > 0 ? `${hours}:${mm}:${ss}` : `${mm}:${ss}`;
}

/**
 * Drive the timeline cursor to `t` seconds. Lazily imports the media
 * store so this module stays decoupled from the React tree (the
 * drill-down panels are React components, but this helper is called
 * from click handlers).
 */
export function jumpTimelineTo(t: number): void {
  void import("../../../media/store").then(({ useMediaStore }) => {
    try {
      useMediaStore.getState().setTimelineTime(Math.max(0, t));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn("jumpTimelineTo failed", err);
    }
  });
}
