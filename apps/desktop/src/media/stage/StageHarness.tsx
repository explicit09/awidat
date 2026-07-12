/**
 * StageHarness — dev-only, Tauri-free route that mounts `Stage` over a
 * paused, seeked fixture video at a frozen timestamp. Playwright (Task 8)
 * screenshots this route to gate the Stage compositor's visual output.
 *
 * Determinism contract (binding — see docs/superpowers/plans):
 *  - fixed 1280x720 program frame
 *  - `frozenClock(t)` with `t` from `?t=` (default 1.0) — no free-running
 *    animations; every layer derives its look from `clock.now()` alone.
 *  - video: `preload="auto"`, muted, paused, `currentTime = t`, no autoplay
 *  - `document.title = "stage-harness-ready"` only after ALL of: the
 *    video's `seeked` event has fired, the seeked frame has actually been
 *    PRESENTED (via `requestVideoFrameCallback`, since `seeked` alone can
 *    still screenshot the pre-seek frame), `document.fonts.ready` has
 *    resolved, the scene JSON has been fetched + parsed, and the scene's
 *    image layer (if any) has been decoded — Playwright waits on the
 *    title, not on a timeout. A scene fetch/decode failure also flips
 *    the title (with the error visible in the DOM) so the test fails
 *    fast on its DOM asserts instead of timing out.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { Stage } from "./Stage";
import { frozenClock } from "./stageClock";
import type { PreviewTitleOverlay } from "./titles";
import type {
  PreviewMotionImageOverlay,
  PreviewMotionSceneOverlay,
  PreviewMotionShapeOverlay,
} from "./motionScene";
import type { TimelineParameterAnimation } from "../../protocol";

const PROGRAM_WIDTH = 1280;
const PROGRAM_HEIGHT = 720;
const DEFAULT_T = 1.0;
const DEFAULT_SCENE_URL = "/fixtures/stage/scene-basic.json";
const DEFAULT_CLIP_URL = "/fixtures/stage/clip.mp4";

/**
 * Raw JSON scene keyframe shape. Mirrors `TimelineKeyframe` but uses
 * `timeS` (camelCase) since the fixture is hand-authored JSON, not a
 * ts-rs export from the Rust protocol. Mapped into `TimelineKeyframe`
 * ("time_s") below.
 */
type SceneKeyframe = {
  timeS: number;
  value: number;
  interpolation: string;
  easing: string;
  /** Spring parameters for `interpolation: "spring"` segments. */
  spring?: { mass: number; stiffness: number; damping: number } | null;
};

type SceneAnimation = {
  id: string;
  parameter: string;
  keyframes: SceneKeyframe[];
};

type SceneTitle = {
  key: string;
  startS: number;
  endS: number;
  text: string;
  position: "top" | "center" | "bottom";
  fontSize: number;
  color: string;
  fontWeight: "normal" | "bold";
  animation: PreviewTitleOverlay["animation"];
  reveal: PreviewTitleOverlay["reveal"];
  box: PreviewTitleOverlay["box"];
  animations?: SceneAnimation[];
};

type SceneShape = {
  key: string;
  startS: number;
  endS: number;
  shape: "rect";
  x: number;
  y: number;
  width: number;
  height: number;
  color: string;
  opacity: number;
  scale: number;
  anchorX: number;
  anchorY: number;
  rotationDeg: number;
  animation?: SceneAnimation;
  animations?: SceneAnimation[];
};

type SceneImage = {
  key: string;
  startS: number;
  endS: number;
  src: string;
  x: number;
  y: number;
  width: number;
  height: number;
  opacity: number;
  fit: "cover" | "contain" | "fill";
  scale: number;
  anchorX: number;
  anchorY: number;
  rotationDeg: number;
  animation?: SceneAnimation;
  animations?: SceneAnimation[];
};

/**
 * MotionScene text layer as lowered by the Tauri snapshot (role
 * "motion_scene"): rendered inside `.timeline-motion-scene-layer`
 * via `TimelineMotionSceneOverlays`, unlike the legacy `title`
 * band overlay in `.timeline-title-layer`. Mirrors
 * `PreviewTitleOverlay` minus `isMotionScene` (always true here).
 */
type SceneMotionTitle = {
  key: string;
  startS: number;
  endS: number;
  text: string;
  position: "top" | "center" | "bottom";
  fontSize: number;
  color: string;
  fontWeight: "normal" | "bold";
  animation: PreviewTitleOverlay["animation"];
  reveal: PreviewTitleOverlay["reveal"];
  box: PreviewTitleOverlay["box"];
  animations?: SceneAnimation[];
};

/**
 * One entry of the scene's ordered MotionScene layer list — the
 * snapshot-level mirror of what `activeMotionSceneOverlays` builds
 * from a Tauri `TimelineSnapshot`. Draw order follows array order
 * (the Rust sync test emits layers sorted by z_index).
 */
type SceneMotionLayer =
  | ({ kind: "title" } & SceneMotionTitle)
  | ({ kind: "shape" } & SceneShape)
  | ({ kind: "image" } & SceneImage);

type SceneDocument = {
  title?: SceneTitle;
  shape?: SceneShape;
  image?: SceneImage;
  /** Ordered MotionScene layers (template harness scenes). */
  layers?: SceneMotionLayer[];
};

/** Maps the hand-authored `SceneAnimation` shape onto the runtime
 * `TimelineParameterAnimation` type. Defaults: no motion path, hold
 * extrapolation on both ends (frozen-clock harness never reads past
 * the authored keyframes but this keeps the shape faithful), no
 * rationale. */
function toTimelineParameterAnimation(
  animation: SceneAnimation,
  clipId: string,
): TimelineParameterAnimation {
  return {
    id: animation.id,
    target: { clip_id: clipId, parameter: animation.parameter },
    keyframes: animation.keyframes.map((kf) => ({
      time_s: kf.timeS,
      value: kf.value,
      interpolation: kf.interpolation,
      easing: kf.easing,
      bezier: null,
      tangent_mode: "auto",
      spring: kf.spring ?? null,
    })),
    pre_extrapolation: "hold",
    post_extrapolation: "hold",
    motion_path: null,
    rationale: null,
  };
}

function timelineAnimationsFor(
  key: string,
  animations: SceneAnimation[] | undefined,
): TimelineParameterAnimation[] {
  return (animations ?? []).map((animation) => toTimelineParameterAnimation(animation, key));
}

function keyframeAnimationsFor(scene: {
  key: string;
  animation?: SceneAnimation;
  animations?: SceneAnimation[];
}): TimelineParameterAnimation[] {
  const list = scene.animations ?? (scene.animation ? [scene.animation] : []);
  return timelineAnimationsFor(scene.key, list);
}

function titleOverlayFromScene(scene: SceneTitle): PreviewTitleOverlay {
  return {
    key: scene.key,
    startS: scene.startS,
    endS: scene.endS,
    text: scene.text,
    position: scene.position,
    fontSize: scene.fontSize,
    color: scene.color,
    fontWeight: scene.fontWeight,
    animation: scene.animation,
    reveal: scene.reveal,
    animations: timelineAnimationsFor(scene.key, scene.animations),
    isMotionScene: false,
    box: scene.box,
  };
}

/** MotionScene text layer → PreviewTitleOverlay (role motion_scene). */
function motionTitleOverlayFromScene(scene: SceneMotionTitle): PreviewTitleOverlay {
  return {
    key: scene.key,
    startS: scene.startS,
    endS: scene.endS,
    text: scene.text,
    position: scene.position,
    fontSize: scene.fontSize,
    color: scene.color,
    fontWeight: scene.fontWeight,
    animation: scene.animation,
    reveal: scene.reveal,
    animations: timelineAnimationsFor(scene.key, scene.animations),
    isMotionScene: true,
    box: scene.box,
  };
}

function shapeOverlayFromScene(scene: SceneShape): PreviewMotionShapeOverlay {
  return {
    key: scene.key,
    startS: scene.startS,
    endS: scene.endS,
    shape: scene.shape,
    x: scene.x,
    y: scene.y,
    width: scene.width,
    height: scene.height,
    color: scene.color,
    opacity: scene.opacity,
    scale: scene.scale,
    anchorX: scene.anchorX,
    anchorY: scene.anchorY,
    rotationDeg: scene.rotationDeg,
    animations: keyframeAnimationsFor(scene),
  };
}

function imageOverlayFromScene(scene: SceneImage): PreviewMotionImageOverlay {
  return {
    key: scene.key,
    startS: scene.startS,
    endS: scene.endS,
    src: scene.src,
    x: scene.x,
    y: scene.y,
    width: scene.width,
    height: scene.height,
    opacity: scene.opacity,
    fit: scene.fit,
    scale: scene.scale,
    anchorX: scene.anchorX,
    anchorY: scene.anchorY,
    rotationDeg: scene.rotationDeg,
    animations: keyframeAnimationsFor(scene),
  };
}

function parseHarnessParams(search: string): { t: number; sceneUrl: string; clipUrl: string } {
  const params = new URLSearchParams(search);
  const tParam = params.get("t");
  const t = tParam !== null && Number.isFinite(Number(tParam)) ? Number(tParam) : DEFAULT_T;
  const sceneUrl = params.get("scene") ?? DEFAULT_SCENE_URL;
  const clipUrl = params.get("clip") ?? DEFAULT_CLIP_URL;
  return { t, sceneUrl, clipUrl };
}

export function StageHarness() {
  const { t, sceneUrl, clipUrl } = useMemo(
    () => parseHarnessParams(window.location.search),
    [],
  );
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [scene, setScene] = useState<SceneDocument | null>(null);
  const [sceneError, setSceneError] = useState<string | null>(null);
  const [videoSeeked, setVideoSeeked] = useState(false);
  const [videoFramePresented, setVideoFramePresented] = useState(false);
  const [fontsReady, setFontsReady] = useState(false);
  const [overlayAssetsReady, setOverlayAssetsReady] = useState(false);

  // Fetch + parse the scene JSON once. A fetch failure surfaces as
  // `sceneError` (visible in the DOM) rather than throwing, so the
  // harness page always renders — Playwright can assert on the error
  // state instead of hanging.
  useEffect(() => {
    let cancelled = false;
    fetch(sceneUrl)
      .then((res) => {
        if (!res.ok) throw new Error(`scene fetch ${res.status}`);
        return res.json();
      })
      .then((json: SceneDocument) => {
        if (!cancelled) setScene(json);
      })
      .catch((e) => {
        if (!cancelled) setSceneError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, [sceneUrl]);

  // document.fonts.ready resolves once webfonts (if any) have loaded;
  // in a plain-Chromium harness with system fonts only, this resolves
  // essentially immediately, but we still wait on it per the binding
  // determinism contract.
  useEffect(() => {
    if (typeof document.fonts === "undefined") {
      setFontsReady(true);
      return;
    }
    let cancelled = false;
    document.fonts.ready.then(() => {
      if (!cancelled) setFontsReady(true);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    setVideoSeeked(false);
    const onSeeked = () => setVideoSeeked(true);

    const seekTo = (target: number) => {
      // If the target time is already current and we're not seeking, fire onSeeked immediately
      if (Math.abs(video.currentTime - target) < 0.001 && !video.seeking) {
        onSeeked();
        return;
      }
      video.addEventListener("seeked", onSeeked, { once: true });
      video.currentTime = target;
    };

    const onLoadedMetadata = () => {
      seekTo(t);
    };

    if (video.readyState >= 1) {
      seekTo(t);
    } else {
      video.addEventListener("loadedmetadata", onLoadedMetadata, { once: true });
    }
    return () => {
      video.removeEventListener("seeked", onSeeked);
      video.removeEventListener("loadedmetadata", onLoadedMetadata);
    };
  }, [t, clipUrl]);

  // The `seeked` event fires when the seek completes internally, but the
  // seeked frame is only PRESENTED for composition on a later rendering
  // step — a screenshot taken between the two captures the pre-seek
  // frame (observed as a real flake: kinetic-text golden showed the
  // t=0 frame). Gate readiness on `requestVideoFrameCallback` reporting
  // a presented frame whose mediaTime matches the target `t`. All
  // harness `t` values are frame-aligned for the 30fps fixture clip, so
  // a half-frame-at-60fps tolerance can never accept a neighbor frame.
  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    setVideoFramePresented(false);
    type VideoFrameMetadata = { mediaTime: number };
    const rvfcVideo = video as HTMLVideoElement & {
      requestVideoFrameCallback?: (
        callback: (now: number, metadata: VideoFrameMetadata) => void,
      ) => number;
      cancelVideoFrameCallback?: (handle: number) => void;
    };
    if (typeof rvfcVideo.requestVideoFrameCallback !== "function") {
      // No presentation signal available — fall back to the seeked
      // event alone (pre-rVFC engines; the harness runs Chromium).
      setVideoFramePresented(true);
      return;
    }
    let cancelled = false;
    let handle = 0;
    const toleranceS = 1 / 60;
    const onFrame = (_now: number, metadata: VideoFrameMetadata) => {
      if (cancelled) return;
      if (Math.abs(metadata.mediaTime - t) <= toleranceS) {
        setVideoFramePresented(true);
        return;
      }
      handle = rvfcVideo.requestVideoFrameCallback!(onFrame);
    };
    handle = rvfcVideo.requestVideoFrameCallback(onFrame);
    return () => {
      cancelled = true;
      rvfcVideo.cancelVideoFrameCallback?.(handle);
    };
  }, [t, clipUrl]);

  // Decode the scene's image layer (if any) before declaring the scene's
  // assets ready. The decoded image lands in the browser cache, so the
  // Stage's own <img> paints immediately rather than racing the
  // screenshot.
  useEffect(() => {
    if (!scene) return;
    const src = scene.image?.src;
    if (!src) {
      setOverlayAssetsReady(true);
      return;
    }
    let cancelled = false;
    const img = new Image();
    img.src = src;
    img.decode().then(
      () => {
        if (!cancelled) setOverlayAssetsReady(true);
      },
      (e) => {
        if (!cancelled) setSceneError(`image decode failed for ${src}: ${e}`);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [scene]);

  useEffect(() => {
    const sceneReady = sceneError !== null || (scene !== null && overlayAssetsReady);
    if (videoSeeked && videoFramePresented && fontsReady && sceneReady) {
      document.title = "stage-harness-ready";
    }
  }, [
    videoSeeked,
    videoFramePresented,
    fontsReady,
    scene,
    sceneError,
    overlayAssetsReady,
  ]);

  const clock = useMemo(() => frozenClock(t), [t]);

  const titles: PreviewTitleOverlay[] = useMemo(() => {
    if (!scene?.title) return [];
    return [titleOverlayFromScene(scene.title)];
  }, [scene]);

  const motionSceneLayers: PreviewMotionSceneOverlay[] = useMemo(() => {
    const layers: PreviewMotionSceneOverlay[] = [];
    for (const layer of scene?.layers ?? []) {
      if (layer.kind === "title") {
        layers.push({ kind: "title", overlay: motionTitleOverlayFromScene(layer) });
      } else if (layer.kind === "shape") {
        layers.push({ kind: "shape", overlay: shapeOverlayFromScene(layer) });
      } else {
        layers.push({ kind: "image", overlay: imageOverlayFromScene(layer) });
      }
    }
    if (scene?.shape) {
      layers.push({ kind: "shape", overlay: shapeOverlayFromScene(scene.shape) });
    }
    if (scene?.image) {
      layers.push({ kind: "image", overlay: imageOverlayFromScene(scene.image) });
    }
    return layers;
  }, [scene]);

  const programFrameCss = useMemo(
    () => ({
      position: "absolute" as const,
      left: 0,
      top: 0,
      width: `${PROGRAM_WIDTH}px`,
      height: `${PROGRAM_HEIGHT}px`,
    }),
    [],
  );
  const programFrameSize = useMemo(
    () => ({ width: PROGRAM_WIDTH, height: PROGRAM_HEIGHT }),
    [],
  );

  return (
    <div
      className="stage-harness-root"
      data-testid="stage-harness-root"
      style={{
        position: "relative",
        width: `${PROGRAM_WIDTH}px`,
        height: `${PROGRAM_HEIGHT}px`,
        background: "#000",
        overflow: "hidden",
      }}
    >
      <video
        ref={videoRef}
        className="stage-harness-video"
        data-testid="stage-harness-video"
        src={clipUrl}
        preload="auto"
        muted
        playsInline
        autoPlay={false}
        style={{
          position: "absolute",
          left: 0,
          top: 0,
          width: `${PROGRAM_WIDTH}px`,
          height: `${PROGRAM_HEIGHT}px`,
          objectFit: "cover",
        }}
      />
      <Stage
        clock={clock}
        programFrameCss={programFrameCss}
        programFrameSize={programFrameSize}
        projectRoot={null}
        videoOverlays={[]}
        transition={null}
        renderTransitionOnGpu={false}
        titles={titles}
        motionSceneLayers={motionSceneLayers}
        broadcastOverlay={null}
        showGap={false}
      />
      {sceneError && (
        <div data-testid="stage-harness-scene-error" style={{ position: "absolute", inset: 0, color: "red" }}>
          {sceneError}
        </div>
      )}
    </div>
  );
}
