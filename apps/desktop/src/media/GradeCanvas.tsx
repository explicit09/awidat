// WebGL grade pass for the preview monitor.
//
// A transparent canvas stacked over the double-buffered <video>
// slots. When the active clip carries a non-default color correction
// (or the Inspector's sliders are mid-drag), each displayed frame is
// uploaded as a texture and run through a fragment shader that
// mirrors the render engine's FFmpeg chain (see gradeMath.ts) — all
// seven fields, matching what exports will look like.
//
// Lifecycle rules:
// - Grade at rest / suspended (transition window) / WebGL unavailable
//   → canvas hidden, zero per-frame cost. `availabilityRef` tells the
//   player whether the pass is live so it can fall back to the CSS
//   approximation when it isn't.
// - Playing → rAF draw loop. Paused → draw on grade change and on the
//   element's seeked/loadeddata events (frame-stepping, scrubbing).
// - The video elements are crossOrigin="anonymous" (media server
//   sends ACAO:*); a tainted-canvas SecurityError or context loss
//   permanently disables the pass for the session → CSS fallback.

import { useEffect, useRef } from "react";
import type { ColorCorrectionStyling } from "../protocol";
import {
  CURVE_LUT_SIZE,
  buildCurveLut,
  buildGradePlan,
  isDefaultGrade,
} from "./gradeMath";

const VERT = `#version 300 es
in vec2 a_pos;
out vec2 v_uv;
void main() {
  v_uv = vec2(a_pos.x * 0.5 + 0.5, 0.5 - a_pos.y * 0.5);
  gl_Position = vec4(a_pos, 0.0, 1.0);
}`;

// Mirrors gradeMath.applyGradeToRgb — keep the two in sync.
const FRAG = `#version 300 es
precision mediump float;
uniform sampler2D u_video;
uniform sampler2D u_curve;
uniform int u_useEq;
uniform float u_brightness;
uniform float u_contrast;
uniform float u_saturation;
uniform int u_useCurves;
uniform int u_useCb;
uniform vec3 u_cbShadows;
uniform vec3 u_cbHighlights;
in vec2 v_uv;
out vec4 outColor;

const float LR = 0.2126;
const float LG = 0.7152;
const float LB = 0.0722;

void main() {
  vec3 c = texture(u_video, v_uv).rgb;

  if (u_useEq == 1) {
    float y = dot(c, vec3(LR, LG, LB));
    float cb = (c.b - y) / 1.8556;
    float cr = (c.r - y) / 1.5748;
    float y2 = (y - 0.5) * u_contrast + 0.5 + u_brightness;
    cb *= u_saturation;
    cr *= u_saturation;
    float r = y2 + 1.5748 * cr;
    float b = y2 + 1.8556 * cb;
    float g = (y2 - LR * r - LB * b) / LG;
    c = clamp(vec3(r, g, b), 0.0, 1.0);
  }

  if (u_useCurves == 1) {
    c = vec3(
      texture(u_curve, vec2(c.r, 0.5)).r,
      texture(u_curve, vec2(c.g, 0.5)).r,
      texture(u_curve, vec2(c.b, 0.5)).r
    );
  }

  if (u_useCb == 1) {
    float l = (max(c.r, max(c.g, c.b)) + min(c.r, min(c.g, c.b))) * 0.5;
    float third = 1.0 / 3.0;
    float ws = clamp((third - l) * 4.0 + 0.5, 0.0, 1.0) * 0.7;
    float wh = clamp((l + third - 1.0) * 4.0 + 0.5, 0.0, 1.0) * 0.7;
    c = clamp(c + u_cbShadows * ws + u_cbHighlights * wh, 0.0, 1.0);
  }

  outColor = vec4(c, 1.0);
}`;

type GlState = {
  gl: WebGL2RenderingContext;
  program: WebGLProgram;
  videoTex: WebGLTexture;
  curveTex: WebGLTexture;
  uniforms: Record<string, WebGLUniformLocation | null>;
};

function compile(gl: WebGL2RenderingContext, kind: number, src: string) {
  const shader = gl.createShader(kind);
  if (!shader) return null;
  gl.shaderSource(shader, src);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    // eslint-disable-next-line no-console
    console.warn("grade shader compile failed", gl.getShaderInfoLog(shader));
    gl.deleteShader(shader);
    return null;
  }
  return shader;
}

function initGl(canvas: HTMLCanvasElement): GlState | null {
  const gl = canvas.getContext("webgl2", {
    premultipliedAlpha: false,
    alpha: true,
  });
  if (!gl) return null;
  const vs = compile(gl, gl.VERTEX_SHADER, VERT);
  const fs = compile(gl, gl.FRAGMENT_SHADER, FRAG);
  if (!vs || !fs) return null;
  const program = gl.createProgram();
  if (!program) return null;
  gl.attachShader(program, vs);
  gl.attachShader(program, fs);
  gl.linkProgram(program);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    // eslint-disable-next-line no-console
    console.warn("grade shader link failed", gl.getProgramInfoLog(program));
    return null;
  }
  gl.useProgram(program);

  const quad = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, quad);
  gl.bufferData(
    gl.ARRAY_BUFFER,
    new Float32Array([-1, -1, 3, -1, -1, 3]),
    gl.STATIC_DRAW,
  );
  const aPos = gl.getAttribLocation(program, "a_pos");
  gl.enableVertexAttribArray(aPos);
  gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);

  const makeTex = () => {
    const tex = gl.createTexture();
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    return tex;
  };
  const videoTex = makeTex();
  const curveTex = makeTex();
  if (!videoTex || !curveTex) return null;

  const names = [
    "u_video",
    "u_curve",
    "u_useEq",
    "u_brightness",
    "u_contrast",
    "u_saturation",
    "u_useCurves",
    "u_useCb",
    "u_cbShadows",
    "u_cbHighlights",
  ];
  const uniforms: GlState["uniforms"] = {};
  for (const name of names) uniforms[name] = gl.getUniformLocation(program, name);
  gl.uniform1i(uniforms.u_video, 0);
  gl.uniform1i(uniforms.u_curve, 1);
  return { gl, program, videoTex, curveTex, uniforms };
}

export function GradeCanvas({
  grade,
  getVideo,
  isPlaying,
  suspended,
  availabilityRef,
}: {
  /** Resolved grade for the ACTIVE clip (override wins upstream). */
  grade: ColorCorrectionStyling | null;
  getVideo: () => HTMLVideoElement | null;
  isPlaying: boolean;
  /** True during a transition window — the CSS cross-fade under us
   *  must stay visible, so the pass steps aside. */
  suspended: boolean;
  /** Written, not read: whether the WebGL pass is currently painting
   *  (the player falls back to CSS filters when false). */
  availabilityRef: React.MutableRefObject<boolean>;
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const glRef = useRef<GlState | null>(null);
  const deadRef = useRef(false);
  const curveKeyRef = useRef<string>("");

  const active = !suspended && !deadRef.current && !isDefaultGrade(grade);

  useEffect(() => {
    availabilityRef.current = active && !deadRef.current;
    return () => {
      availabilityRef.current = false;
    };
  }, [active, availabilityRef]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !active) return;

    if (!glRef.current) {
      glRef.current = initGl(canvas);
      if (!glRef.current) {
        deadRef.current = true;
        availabilityRef.current = false;
        return;
      }
    }
    const state = glRef.current;
    const plan = buildGradePlan(grade);

    const { gl, uniforms } = state;
    gl.useProgram(state.program);
    gl.uniform1i(uniforms.u_useEq, plan.eq ? 1 : 0);
    if (plan.eq) {
      gl.uniform1f(uniforms.u_brightness, plan.eq.brightness);
      gl.uniform1f(uniforms.u_contrast, plan.eq.contrast);
      gl.uniform1f(uniforms.u_saturation, plan.eq.saturation);
    }
    gl.uniform1i(uniforms.u_useCurves, plan.curves ? 1 : 0);
    const curveKey = plan.curves
      ? `${plan.curves.shadowMid}|${plan.curves.highlightMid}`
      : "";
    if (curveKey !== curveKeyRef.current) {
      curveKeyRef.current = curveKey;
      const lut = buildCurveLut(plan.curves);
      gl.activeTexture(gl.TEXTURE1);
      gl.bindTexture(gl.TEXTURE_2D, state.curveTex);
      gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
      gl.texImage2D(
        gl.TEXTURE_2D,
        0,
        gl.R8,
        CURVE_LUT_SIZE,
        1,
        0,
        gl.RED,
        gl.UNSIGNED_BYTE,
        lut,
      );
    }
    gl.uniform1i(uniforms.u_useCb, plan.colorBalance ? 1 : 0);
    if (plan.colorBalance) {
      gl.uniform3fv(uniforms.u_cbShadows, plan.colorBalance.shadows);
      gl.uniform3fv(uniforms.u_cbHighlights, plan.colorBalance.highlights);
    }

    let raf = 0;
    let disposed = false;

    const draw = () => {
      if (disposed || deadRef.current) return;
      const v = getVideo();
      if (!v || v.readyState < 2 || v.videoWidth === 0) return;
      const dpr = window.devicePixelRatio || 1;
      const cw = Math.max(1, Math.round(canvas.clientWidth * dpr));
      const ch = Math.max(1, Math.round(canvas.clientHeight * dpr));
      if (canvas.width !== cw || canvas.height !== ch) {
        canvas.width = cw;
        canvas.height = ch;
      }
      // Replicate the video element's object-fit: contain letterbox.
      const scale = Math.min(cw / v.videoWidth, ch / v.videoHeight);
      const w = Math.round(v.videoWidth * scale);
      const h = Math.round(v.videoHeight * scale);
      gl.viewport(Math.round((cw - w) / 2), Math.round((ch - h) / 2), w, h);
      gl.clearColor(0, 0, 0, 0);
      gl.clear(gl.COLOR_BUFFER_BIT);
      try {
        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, state.videoTex);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGB, gl.RGB, gl.UNSIGNED_BYTE, v);
      } catch (e) {
        // Tainted canvas / upload failure — disable for the session,
        // the player falls back to CSS filters.
        // eslint-disable-next-line no-console
        console.warn("grade pass video upload failed; falling back", e);
        deadRef.current = true;
        availabilityRef.current = false;
        return;
      }
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };

    const loop = () => {
      if (disposed) return;
      draw();
      raf = window.requestAnimationFrame(loop);
    };

    draw();
    if (isPlaying) {
      raf = window.requestAnimationFrame(loop);
    }
    // Paused color work: repaint when the frame under the playhead
    // changes (scrub, frame-step, slot swap finishing a load).
    const v = getVideo();
    const onFrame = () => draw();
    v?.addEventListener("seeked", onFrame);
    v?.addEventListener("loadeddata", onFrame);

    return () => {
      disposed = true;
      if (raf) window.cancelAnimationFrame(raf);
      v?.removeEventListener("seeked", onFrame);
      v?.removeEventListener("loadeddata", onFrame);
    };
  }, [grade, active, isPlaying, getVideo, availabilityRef]);

  return (
    <canvas
      ref={canvasRef}
      aria-hidden
      style={{
        position: "absolute",
        inset: 0,
        width: "100%",
        height: "100%",
        pointerEvents: "none",
        zIndex: 3,
        display: active ? "block" : "none",
      }}
    />
  );
}
