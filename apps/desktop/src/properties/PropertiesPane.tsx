// Right-rail properties inspector — third pane in the workspace top
// row. Reads the selected clip from useTimelineSelectionStore and
// renders read-only metadata + interactive Volume / Speed controls
// (Step 15.5). Slider/input edits debounce 300ms then fire a
// propose_user_edit envelope so the change rides the same Step-5
// proposal pipeline (ghost overlay, Accept/Reject) as drag-trim.
//
// Future Step 16 grows a Title editor into this same pane.

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTimelineStore } from "../timeline/store";
import type { TimelineItem, TimelineTrack } from "../timeline/store";
import { useTimelineSelectionStore } from "./store";
import { useMediaStore } from "../media/store";
import { serializeEdl, type EdlOp } from "../timeline/edlBuilder";

/** Default values when a clip carries no awidat.volume / awidat.speed effect.
 *  Surface as "1.0" so the slider/input shows unity rather than empty. */
const DEFAULT_VOLUME = 1.0;
const DEFAULT_SPEED = 1.0;
const DEFAULT_COLOR = {
  exposureEv: 0,
  contrast: 1,
  saturation: 1,
  temperature: 0,
  tint: 0,
  shadows: 0,
  highlights: 0,
};

/** Debounce window before pushing a slider/input change through
 *  propose_user_edit. Long enough to coalesce a rapid drag; short
 *  enough that a single deliberate change feels responsive. */
const COMMIT_DEBOUNCE_MS = 300;

export function PropertiesPane() {
  const snapshot = useTimelineStore((s) => s.snapshot);
  const key = useTimelineSelectionStore((s) => s.selectedClipKey);
  const clearSelection = useTimelineSelectionStore((s) => s.clear);
  const timelineTime = useMediaStore((s) => s.timelineTime);

  const followsSelection = key !== null;
  const activeKey = key ?? findClipKeyAtTime(snapshot.tracks, timelineTime);
  const track =
    activeKey === null ? null : snapshot.tracks[activeKey.trackIndex] ?? null;
  const item =
    activeKey === null || track === null
      ? null
      : track.items.find((candidate) => candidate.index === activeKey.clipIndex) ??
        track.items[activeKey.clipIndex] ??
        null;

  if (!item || item.kind !== "clip") {
    return (
      <section className="properties-pane">
        <header className="properties-header">
          <span className="properties-label">Inspector</span>
          <span className="properties-header-meta">No active clip</span>
        </header>
        <div className="properties-empty">
          Scrub over a clip or select one on the timeline to inspect it.
        </div>
      </section>
    );
  }

  const sourceStart = item.source_start_s ?? 0;
  const sourceEnd = sourceStart + item.duration_s;
  const trackStart = item.track_start_s;
  const trackEnd = trackStart + item.duration_s;
  const trackName = track?.name ?? "?";
  const trackKind = track?.kind ?? "?";
  const deleteClip = async () => {
    const clips =
      item.link_group_id !== null
        ? snapshot.tracks.flatMap((candidateTrack) =>
            candidateTrack.items.filter(
              (candidate): candidate is Extract<TimelineItem, { kind: "clip" }> =>
                candidate.kind === "clip" &&
                candidate.link_group_id === item.link_group_id,
            ),
          )
        : [item];
    const seen = new Set<string>();
    const ops: EdlOp[] = clips
      .filter((clip) => {
        if (seen.has(clip.clip_uuid)) return false;
        seen.add(clip.clip_uuid);
        return true;
      })
      .map((clip) => ({
        kind: "delete_clip",
        anchor: { kind: "clip_uuid", uuid: clip.clip_uuid },
      }));
    if (ops.length === 0) return;
    await invoke<string>("propose_user_edit", { edlText: serializeEdl(ops) });
    clearSelection();
  };

  return (
    <section className="properties-pane">
      <header className="properties-header">
        <span className="properties-label">Inspector</span>
        <span className="properties-header-meta">
          {followsSelection ? "Selected clip" : "At playhead"}
        </span>
      </header>
      <div className="properties-body">
        <Field label="Name">
          <span className="properties-value">{item.name}</span>
        </Field>
        <div className="properties-action-row">
          <button className="properties-danger" onClick={() => void deleteClip()}>
            Delete clip
          </button>
          {item.link_group_id && (
            <span className="properties-action-hint">Deletes linked audio/video</span>
          )}
        </div>
        {item.title ? (
          <TitleEditor
            clipUuid={item.clip_uuid}
            title={item.title}
            startS={trackStart}
            endS={trackEnd}
          />
        ) : (
          <>
            {(item.volume ?? DEFAULT_VOLUME) <= 0.001 && (
              <div className="properties-alert">
                This clip is muted. Preview audio will be silent here.
              </div>
            )}
            <ColorCorrectionControl
              clipUuid={item.clip_uuid}
              value={item.color_correction}
            />
            <LutControl clipUuid={item.clip_uuid} lutPath={item.lut_path} />
            <VolumeControl clipUuid={item.clip_uuid} value={item.volume} />
            <AudioFadeControl
              clipUuid={item.clip_uuid}
              fadeInS={item.fade_in_s}
              fadeOutS={item.fade_out_s}
            />
            <SpeedControl clipUuid={item.clip_uuid} factor={item.speed} />
          </>
        )}
        {track?.audio && <TrackAudioControl trackName={track.name} audio={track.audio} />}
        <Field label="Track">
          <span className="properties-value">
            {trackName} <span className="properties-dim">· {trackKind}</span>
          </span>
        </Field>
        <Field label="Asset">
          <code className="properties-code" title={item.asset_id ?? ""}>
            {item.asset_id ?? "(none)"}
          </code>
        </Field>
        <Field label="Source">
          <span className="properties-value">
            {sourceStart.toFixed(2)}s → {sourceEnd.toFixed(2)}s
          </span>
        </Field>
        <Field label="Timeline">
          <span className="properties-value">
            {trackStart.toFixed(2)}s → {trackEnd.toFixed(2)}s
          </span>
        </Field>
        <Field label="Duration">
          <span className="properties-value">{item.duration_s.toFixed(2)}s</span>
        </Field>
        <Field label="Clip uuid">
          <code className="properties-code" title={item.clip_uuid}>
            {item.clip_uuid}
          </code>
        </Field>
      </div>
    </section>
  );
}

function findClipKeyAtTime(
  tracks: TimelineTrack[],
  timelineTime: number,
): { trackIndex: number; clipIndex: number } | null {
  if (!Number.isFinite(timelineTime)) return null;
  for (let trackIndex = 0; trackIndex < tracks.length; trackIndex += 1) {
    const track = tracks[trackIndex];
    for (const item of track.items) {
      if (!isInspectableClip(item)) continue;
      const start = item.track_start_s;
      const end = start + item.duration_s;
      if (timelineTime >= start && timelineTime < end) {
        return { trackIndex, clipIndex: item.index };
      }
    }
  }
  return null;
}

function isInspectableClip(item: TimelineItem): item is Extract<TimelineItem, { kind: "clip" }> {
  return item.kind === "clip";
}

type TitlePosition = "top" | "center" | "bottom";
type TitleWeight = "normal" | "bold";
type TitleAnimation =
  | "none"
  | "fade_in"
  | "fade_out"
  | "fade_in_out"
  | "slide_in"
  | "slide_out";

function TitleEditor({
  clipUuid,
  title,
  startS,
  endS,
}: {
  clipUuid: string;
  title: import("../protocol").TitleStyling;
  startS: number;
  endS: number;
}) {
  const [text, setText] = useState(title.text);
  const [position, setPosition] = useState<TitlePosition>(
    (title.position as TitlePosition) ?? "center",
  );
  const [fontSize, setFontSize] = useState<number>(title.font_size);
  const [color, setColor] = useState<string>(title.color);
  const [fontWeight, setFontWeight] = useState<TitleWeight>(
    (title.font_weight as TitleWeight) ?? "normal",
  );
  const [animation, setAnimation] = useState<TitleAnimation>(
    (title.animation as TitleAnimation) ?? "none",
  );

  // Reset local state when the user selects a different title clip
  // or the persisted styling changes from outside.
  useEffect(() => {
    setText(title.text);
    setPosition((title.position as TitlePosition) ?? "center");
    setFontSize(title.font_size);
    setColor(title.color);
    setFontWeight((title.font_weight as TitleWeight) ?? "normal");
    setAnimation((title.animation as TitleAnimation) ?? "none");
    // Track last-committed snapshot so debounce can no-op when the
    // local state matches what we last sent.
    lastCommittedRef.current = signature(
      title.text,
      (title.position as TitlePosition) ?? "center",
      title.font_size,
      title.color,
      (title.font_weight as TitleWeight) ?? "normal",
      (title.animation as TitleAnimation) ?? "none",
    );
  }, [
    clipUuid,
    title.text,
    title.position,
    title.font_size,
    title.color,
    title.font_weight,
    title.animation,
  ]);

  const lastCommittedRef = useRef<string>(
    signature(
      title.text,
      (title.position as TitlePosition) ?? "center",
      title.font_size,
      title.color,
      (title.font_weight as TitleWeight) ?? "normal",
      (title.animation as TitleAnimation) ?? "none",
    ),
  );

  // Debounced commit: only when the local snapshot diverges from
  // the last-sent one. Each commit goes through propose_user_edit
  // with a *** Set Title envelope listing every styling field
  // (the apply layer treats unchanged fields as no-ops, so it's
  // safe to over-send).
  useEffect(() => {
    const currentSig = signature(
      text,
      position,
      fontSize,
      color,
      fontWeight,
      animation,
    );
    if (currentSig === lastCommittedRef.current) return;
    if (text.length === 0) return; // never commit empty text
    const handle = setTimeout(() => {
      lastCommittedRef.current = currentSig;
      const op: EdlOp = {
        kind: "set_title",
        anchor: { kind: "clip_uuid", uuid: clipUuid },
        text,
        position,
        fontSize,
        color,
        fontWeight,
        animation,
      };
      invoke<string>("propose_user_edit", {
        edlText: serializeEdl([op]),
      }).catch((err) => {
        // eslint-disable-next-line no-console
        console.warn("propose_user_edit (set_title) failed", err);
      });
    }, COMMIT_DEBOUNCE_MS);
    return () => clearTimeout(handle);
  }, [clipUuid, text, position, fontSize, color, fontWeight, animation]);

  return (
    <>
      <Field label="Window">
        <span className="properties-value">
          {startS.toFixed(2)}s → {endS.toFixed(2)}s
        </span>
      </Field>
      <Field label="Text">
        <input
          type="text"
          className="properties-text-input"
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
      </Field>
      <Field label="Position">
        <select
          className="properties-select"
          value={position}
          onChange={(e) => setPosition(e.target.value as TitlePosition)}
        >
          <option value="top">Top</option>
          <option value="center">Center</option>
          <option value="bottom">Bottom</option>
        </select>
      </Field>
      <Field label="Font size">
        <div className="properties-control-row">
          <input
            type="range"
            min={16}
            max={128}
            step={1}
            value={fontSize}
            onChange={(e) => setFontSize(parseInt(e.target.value, 10))}
            className="properties-slider"
          />
          <span className="properties-control-value">{fontSize}px</span>
        </div>
      </Field>
      <Field label="Color">
        <input
          type="color"
          className="properties-color-input"
          value={color}
          onChange={(e) => setColor(e.target.value.toUpperCase())}
        />
      </Field>
      <Field label="Weight">
        <select
          className="properties-select"
          value={fontWeight}
          onChange={(e) => setFontWeight(e.target.value as TitleWeight)}
        >
          <option value="normal">Normal</option>
          <option value="bold">Bold</option>
        </select>
      </Field>
      <Field label="Animation">
        <select
          className="properties-select"
          value={animation}
          onChange={(e) => setAnimation(e.target.value as TitleAnimation)}
        >
          <option value="none">None</option>
          <option value="fade_in">Fade in</option>
          <option value="fade_out">Fade out</option>
          <option value="fade_in_out">Fade in & out</option>
          <option value="slide_in">Slide in</option>
          <option value="slide_out">Slide out</option>
        </select>
      </Field>
    </>
  );
}

/** Build a stable signature string for the title styling so the
 *  debounce effect can compare current vs last-committed without
 *  shallow-comparing six independent state values. */
function signature(
  text: string,
  position: TitlePosition,
  fontSize: number,
  color: string,
  fontWeight: TitleWeight,
  animation: TitleAnimation,
): string {
  return `${text}|${position}|${fontSize}|${color}|${fontWeight}|${animation}`;
}

function VolumeControl({
  clipUuid,
  value,
}: {
  clipUuid: string;
  value: number | null;
}) {
  const initial = value ?? DEFAULT_VOLUME;
  const [local, setLocal] = useState(initial);
  const lastCommittedRef = useRef<number>(initial);

  // Reset local state when the clip's persisted value changes (or
  // the user selects a different clip).
  useEffect(() => {
    setLocal(initial);
    lastCommittedRef.current = initial;
  }, [clipUuid, initial]);

  const dirty = Math.abs(local - lastCommittedRef.current) >= 1e-6;

  function apply() {
    lastCommittedRef.current = local;
    const op: EdlOp = {
      kind: "set_volume",
      anchor: { kind: "clip_uuid", uuid: clipUuid },
      value: local,
    };
    invoke<string>("propose_user_edit", {
      edlText: serializeEdl([op]),
    }).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (set_volume) failed", err);
    });
  }

  return (
    <Field label="Volume">
      <div className="properties-control-row">
        <input
          type="range"
          min={0}
          max={2}
          step={0.01}
          value={local}
          onChange={(e) => setLocal(parseFloat(e.target.value))}
          className="properties-slider"
        />
        <span className="properties-control-value">{local.toFixed(2)}×</span>
        {dirty && (
          <button className="properties-apply" type="button" onClick={apply}>
            Apply
          </button>
        )}
      </div>
    </Field>
  );
}

function AudioFadeControl({
  clipUuid,
  fadeInS,
  fadeOutS,
}: {
  clipUuid: string;
  fadeInS: number | null;
  fadeOutS: number | null;
}) {
  const initialIn = fadeInS ?? 0;
  const initialOut = fadeOutS ?? 0;
  const [localIn, setLocalIn] = useState(initialIn);
  const [localOut, setLocalOut] = useState(initialOut);
  const lastCommittedRef = useRef(`${initialIn}|${initialOut}`);

  useEffect(() => {
    setLocalIn(initialIn);
    setLocalOut(initialOut);
    lastCommittedRef.current = `${initialIn}|${initialOut}`;
  }, [clipUuid, initialIn, initialOut]);

  const sig = `${localIn}|${localOut}`;
  const dirty = sig !== lastCommittedRef.current;

  function apply() {
    lastCommittedRef.current = sig;
    const op: EdlOp = {
      kind: "set_audio_fade",
      anchor: { kind: "clip_uuid", uuid: clipUuid },
      fadeInS: Math.max(0, localIn),
      fadeOutS: Math.max(0, localOut),
    };
    invoke<string>("propose_user_edit", {
      edlText: serializeEdl([op]),
    }).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (set_audio_fade) failed", err);
    });
  }

  return (
    <Field label="Fades">
      <div className="properties-control-row">
        <input
          type="number"
          min={0}
          step={0.05}
          className="properties-number-input"
          value={localIn}
          onChange={(e) => setLocalIn(parseFloat(e.target.value) || 0)}
          aria-label="Fade in seconds"
        />
        <input
          type="number"
          min={0}
          step={0.05}
          className="properties-number-input"
          value={localOut}
          onChange={(e) => setLocalOut(parseFloat(e.target.value) || 0)}
          aria-label="Fade out seconds"
        />
        {dirty && (
          <button className="properties-apply" type="button" onClick={apply}>
            Apply
          </button>
        )}
      </div>
    </Field>
  );
}

function TrackAudioControl({
  trackName,
  audio,
}: {
  trackName: string;
  audio: import("../protocol").TrackAudioControls;
}) {
  const [role, setRole] = useState(audio.role);
  const [volume, setVolume] = useState(audio.volume);
  const [muted, setMuted] = useState(audio.muted);
  const [solo, setSolo] = useState(audio.solo);
  const [ducking, setDucking] = useState(audio.ducking?.enabled ?? false);
  const [amountDb, setAmountDb] = useState(audio.ducking?.amount_db ?? -12);
  const lastCommittedRef = useRef("");

  useEffect(() => {
    setRole(audio.role);
    setVolume(audio.volume);
    setMuted(audio.muted);
    setSolo(audio.solo);
    setDucking(audio.ducking?.enabled ?? false);
    setAmountDb(audio.ducking?.amount_db ?? -12);
    lastCommittedRef.current = trackAudioSig(
      audio.role,
      audio.volume,
      audio.muted,
      audio.solo,
      audio.ducking?.enabled ?? false,
      audio.ducking?.amount_db ?? -12,
    );
  }, [trackName, audio]);

  const sig = trackAudioSig(role, volume, muted, solo, ducking, amountDb);
  const dirty = sig !== lastCommittedRef.current;

  function apply() {
    lastCommittedRef.current = sig;
    const ops: EdlOp[] = [
      {
        kind: "set_track_audio",
        track: trackName,
        role,
        volume,
        muted,
        solo,
      },
    ];
    if (role !== "dialogue") {
      ops.push({
        kind: "set_ducking",
        track: trackName,
        enabled: ducking,
        amountDb,
        attackMs: 80,
        releaseMs: 300,
      });
    }
    invoke<string>("propose_user_edit", {
      edlText: serializeEdl(ops),
    }).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (set_track_audio) failed", err);
    });
  }

  return (
    <>
      <Field label="Track role">
        <select
          className="properties-select"
          value={role}
          onChange={(e) => setRole(e.target.value)}
        >
          <option value="dialogue">Dialogue</option>
          <option value="music">Music</option>
          <option value="sfx">SFX</option>
        </select>
      </Field>
      <Field label="Track mix">
        <div className="properties-control-row">
          <input
            type="range"
            min={0}
            max={2}
            step={0.01}
            value={volume}
            onChange={(e) => setVolume(parseFloat(e.target.value))}
            className="properties-slider"
          />
          <span className="properties-control-value">{volume.toFixed(2)}×</span>
          <label className="properties-inline-check">
            <input
              type="checkbox"
              checked={muted}
              onChange={(e) => setMuted(e.target.checked)}
            />
            Mute
          </label>
          <label className="properties-inline-check">
            <input
              type="checkbox"
              checked={solo}
              onChange={(e) => setSolo(e.target.checked)}
            />
            Solo
          </label>
        </div>
      </Field>
      {role !== "dialogue" && (
        <Field label="Ducking">
          <div className="properties-control-row">
            <label className="properties-inline-check">
              <input
                type="checkbox"
                checked={ducking}
                onChange={(e) => setDucking(e.target.checked)}
              />
              On
            </label>
            <input
              type="number"
              step={1}
              className="properties-number-input"
              value={amountDb}
              onChange={(e) => setAmountDb(parseFloat(e.target.value) || -12)}
            />
            <span className="properties-control-value">dB</span>
          </div>
        </Field>
      )}
      {dirty && (
        <Field label="Track audio">
          <button className="properties-apply" type="button" onClick={apply}>
            Apply
          </button>
        </Field>
      )}
    </>
  );
}

function trackAudioSig(
  role: string,
  volume: number,
  muted: boolean,
  solo: boolean,
  ducking: boolean,
  amountDb: number,
): string {
  return `${role}|${volume.toFixed(3)}|${muted}|${solo}|${ducking}|${amountDb.toFixed(1)}`;
}

function SpeedControl({
  clipUuid,
  factor,
}: {
  clipUuid: string;
  factor: number | null;
}) {
  const initial = factor ?? DEFAULT_SPEED;
  const [local, setLocal] = useState(initial);
  const lastCommittedRef = useRef<number>(initial);

  useEffect(() => {
    setLocal(initial);
    lastCommittedRef.current = initial;
  }, [clipUuid, initial]);

  const dirty = Math.abs(local - lastCommittedRef.current) >= 1e-6;

  function apply() {
    if (!isFinite(local) || local <= 0) return;
    lastCommittedRef.current = local;
    const op: EdlOp = {
      kind: "set_speed",
      anchor: { kind: "clip_uuid", uuid: clipUuid },
      factor: local,
    };
    invoke<string>("propose_user_edit", {
      edlText: serializeEdl([op]),
    }).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (set_speed) failed", err);
    });
  }

  return (
    <Field label="Speed">
      <div className="properties-control-row">
        <input
          type="range"
          min={0.25}
          max={4}
          step={0.05}
          value={local}
          onChange={(e) => setLocal(parseFloat(e.target.value))}
          className="properties-slider"
        />
        <span className="properties-control-value">{local.toFixed(2)}×</span>
        {dirty && (
          <button className="properties-apply" type="button" onClick={apply}>
            Apply
          </button>
        )}
      </div>
    </Field>
  );
}

type ColorCorrectionValue = {
  exposureEv: number;
  contrast: number;
  saturation: number;
  temperature: number;
  tint: number;
  shadows: number;
  highlights: number;
};

function normalizeColorCorrection(
  value: import("../protocol").ColorCorrectionStyling | null,
): ColorCorrectionValue {
  return {
    exposureEv: value?.exposure_ev ?? DEFAULT_COLOR.exposureEv,
    contrast: value?.contrast ?? DEFAULT_COLOR.contrast,
    saturation: value?.saturation ?? DEFAULT_COLOR.saturation,
    temperature: value?.temperature ?? DEFAULT_COLOR.temperature,
    tint: value?.tint ?? DEFAULT_COLOR.tint,
    shadows: value?.shadows ?? DEFAULT_COLOR.shadows,
    highlights: value?.highlights ?? DEFAULT_COLOR.highlights,
  };
}

function colorSignature(value: ColorCorrectionValue): string {
  return [
    value.exposureEv,
    value.contrast,
    value.saturation,
    value.temperature,
    value.tint,
    value.shadows,
    value.highlights,
  ]
    .map((n) => n.toFixed(3))
    .join("|");
}

function ColorCorrectionControl({
  clipUuid,
  value,
}: {
  clipUuid: string;
  value: import("../protocol").ColorCorrectionStyling | null;
}) {
  const initial = normalizeColorCorrection(value);
  const [local, setLocal] = useState<ColorCorrectionValue>(initial);
  const lastCommittedRef = useRef<string>(colorSignature(initial));

  useEffect(() => {
    const next = normalizeColorCorrection(value);
    setLocal(next);
    lastCommittedRef.current = colorSignature(next);
  }, [clipUuid, value]);

  const currentSig = colorSignature(local);
  const dirty = currentSig !== lastCommittedRef.current;

  function setField<K extends keyof ColorCorrectionValue>(
    key: K,
    nextValue: number,
  ) {
    setLocal((prev) => ({ ...prev, [key]: nextValue }));
  }

  function apply() {
    lastCommittedRef.current = currentSig;
    const op: EdlOp = {
      kind: "set_color_correction",
      anchor: { kind: "clip_uuid", uuid: clipUuid },
      exposureEv: local.exposureEv,
      contrast: local.contrast,
      saturation: local.saturation,
      temperature: local.temperature,
      tint: local.tint,
      shadows: local.shadows,
      highlights: local.highlights,
    };
    invoke<string>("propose_user_edit", {
      edlText: serializeEdl([op]),
    }).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (set_color_correction) failed", err);
    });
  }

  return (
    <>
      <ColorSlider
        label="Exposure"
        value={local.exposureEv}
        min={-4}
        max={4}
        step={0.05}
        unit=" EV"
        onChange={(next) => setField("exposureEv", next)}
      />
      <ColorSlider
        label="Contrast"
        value={local.contrast}
        min={0}
        max={3}
        step={0.05}
        unit="×"
        onChange={(next) => setField("contrast", next)}
      />
      <ColorSlider
        label="Saturation"
        value={local.saturation}
        min={0}
        max={3}
        step={0.05}
        unit="×"
        onChange={(next) => setField("saturation", next)}
      />
      <ColorSlider
        label="Temp"
        value={local.temperature}
        min={-1}
        max={1}
        step={0.02}
        onChange={(next) => setField("temperature", next)}
      />
      <ColorSlider
        label="Tint"
        value={local.tint}
        min={-1}
        max={1}
        step={0.02}
        onChange={(next) => setField("tint", next)}
      />
      <ColorSlider
        label="Shadows"
        value={local.shadows}
        min={-1}
        max={1}
        step={0.02}
        onChange={(next) => setField("shadows", next)}
      />
      <ColorSlider
        label="Highlights"
        value={local.highlights}
        min={-1}
        max={1}
        step={0.02}
        onChange={(next) => setField("highlights", next)}
      />
      {dirty && (
        <Field label="Color">
          <button className="properties-apply" type="button" onClick={apply}>
            Apply
          </button>
        </Field>
      )}
    </>
  );
}

function ColorSlider({
  label,
  value,
  min,
  max,
  step,
  unit = "",
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  unit?: string;
  onChange: (value: number) => void;
}) {
  return (
    <Field label={label}>
      <div className="properties-control-row">
        <input
          type="range"
          min={min}
          max={max}
          step={step}
          value={value}
          onChange={(e) => onChange(parseFloat(e.target.value))}
          className="properties-slider"
        />
        <span className="properties-control-value">
          {value.toFixed(2)}
          {unit}
        </span>
      </div>
    </Field>
  );
}

function LutControl({
  clipUuid,
  lutPath,
}: {
  clipUuid: string;
  lutPath: string | null;
}) {
  const initial = lutPath ?? "";
  const [local, setLocal] = useState(initial);
  const lastCommittedRef = useRef(initial);

  useEffect(() => {
    setLocal(initial);
    lastCommittedRef.current = initial;
  }, [clipUuid, initial]);

  const dirty = local !== lastCommittedRef.current;
  const canApply =
    dirty &&
    local.trim().length > 0 &&
    !local.startsWith("/") &&
    !local.split(/[\\/]/).includes("..");

  function apply() {
    if (!canApply) return;
    lastCommittedRef.current = local;
    const op: EdlOp = {
      kind: "apply_lut",
      anchor: { kind: "clip_uuid", uuid: clipUuid },
      lutPath: local.trim(),
    };
    invoke<string>("propose_user_edit", {
      edlText: serializeEdl([op]),
    }).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (apply_lut) failed", err);
    });
  }

  return (
    <Field label="LUT">
      <div className="properties-control-row">
        <input
          type="text"
          className="properties-text-input"
          value={local}
          placeholder="luts/show-look.cube"
          onChange={(e) => setLocal(e.target.value)}
        />
        {dirty && (
          <button
            className="properties-apply"
            type="button"
            onClick={apply}
            disabled={!canApply}
          >
            Apply
          </button>
        )}
      </div>
    </Field>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="properties-field">
      <div className="properties-field-label">{label}</div>
      <div className="properties-field-value">{children}</div>
    </div>
  );
}
