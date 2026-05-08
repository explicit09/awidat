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

/** Debounce window before pushing a slider/input change through
 *  propose_user_edit. Long enough to coalesce a rapid drag; short
 *  enough that a single deliberate change feels responsive. */
const COMMIT_DEBOUNCE_MS = 300;

export function PropertiesPane() {
  const snapshot = useTimelineStore((s) => s.snapshot);
  const key = useTimelineSelectionStore((s) => s.selectedClipKey);
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
            <VolumeControl clipUuid={item.clip_uuid} value={item.volume} />
            <SpeedControl clipUuid={item.clip_uuid} factor={item.speed} />
          </>
        )}
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
