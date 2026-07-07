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
 *  - `document.title = "stage-harness-ready"` only after BOTH the video's
 *    `seeked` event has fired AND `document.fonts.ready` has resolved —
 *    Playwright waits on the title, not on a timeout.
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

type SceneDocument = {
  title?: SceneTitle;
  shape?: SceneShape;
  image?: SceneImage;
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
      spring: null,
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
  const [fontsReady, setFontsReady] = useState(false);

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
    video.addEventListener("seeked", onSeeked);
    const onLoadedMetadata = () => {
      video.currentTime = t;
    };
    if (video.readyState >= 1) {
      video.currentTime = t;
    } else {
      video.addEventListener("loadedmetadata", onLoadedMetadata, { once: true });
    }
    return () => {
      video.removeEventListener("seeked", onSeeked);
      video.removeEventListener("loadedmetadata", onLoadedMetadata);
    };
  }, [t, clipUrl]);

  useEffect(() => {
    if (videoSeeked && fontsReady) {
      document.title = "stage-harness-ready";
    }
  }, [videoSeeked, fontsReady]);

  const clock = useMemo(() => frozenClock(t), [t]);

  const titles: PreviewTitleOverlay[] = useMemo(() => {
    if (!scene?.title) return [];
    return [titleOverlayFromScene(scene.title)];
  }, [scene]);

  const motionSceneLayers: PreviewMotionSceneOverlay[] = useMemo(() => {
    const layers: PreviewMotionSceneOverlay[] = [];
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
