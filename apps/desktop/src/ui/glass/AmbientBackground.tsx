/**
 * AmbientBackground — the z0 atmosphere plane.
 *
 * Four slow-drifting color fields (red brand light, rose reflection,
 * neutral counter-light, and a deep red accent) under a film-grain
 * overlay and a centering vignette. Pure CSS animation; GPU-promoted.
 * Sits behind everything with pointer-events disabled.
 *
 * Render exactly once near the app root. All glass surfaces blur whatever
 * shows through from this plane.
 */
export function AmbientBackground() {
  return (
    <div className="atmosphere" aria-hidden>
      <div className="atmosphere__orb atmosphere__orb--warm" />
      <div className="atmosphere__orb atmosphere__orb--amber" />
      <div className="atmosphere__orb atmosphere__orb--cool" />
      <div className="atmosphere__orb atmosphere__orb--violet" />
      <div className="atmosphere__vignette" />
      <div className="atmosphere__grain" />
    </div>
  );
}
