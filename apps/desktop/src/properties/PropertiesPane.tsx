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
import { useColorPreviewOverride, useTimelineSelectionStore } from "./store";
import { useMediaStore } from "../media/store";
import { type EdlOp } from "../timeline/edlBuilder";
import { editorDispatch } from "../editor/tauriDispatch";
import { MotionAnimationControl } from "./MotionAnimationControl";
import { Slider } from "./Slider";
import { CollapsiblePanel, type RevealLevel } from "../ui/primitives/CollapsiblePanel";

/** Default values when a clip carries no montage.volume / montage.speed effect.
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
const SUPPORTED_LUT_EXTENSIONS = new Set(["3dl", "cube", "dat", "m3d", "csp"]);
const CUT_TYPE_OPTIONS = [
  { value: "hard_cut", label: "Hard Cut" },
  { value: "cut_on_action", label: "Cut On Action" },
  { value: "cutaway", label: "Cutaway" },
  { value: "insert", label: "Insert" },
  { value: "eyeline_match_cut", label: "Eyeline Match" },
  { value: "shot_reverse_shot", label: "Shot Reverse Shot" },
  { value: "match_cut", label: "Match Cut" },
  { value: "smash_cut", label: "Smash Cut" },
  { value: "cross_cut", label: "Cross Cut" },
  { value: "j_cut", label: "J-Cut" },
  { value: "l_cut", label: "L-Cut" },
];

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

  if (!item) {
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

  if (item.kind === "transition") {
    return (
      <section className="properties-pane">
        <header className="properties-header">
          <span className="properties-label">Inspector</span>
          <span className="properties-header-meta">
            {followsSelection ? "Selected transition" : "At playhead"}
          </span>
        </header>
        <div className="properties-body">
          <TransitionEditor
            track={track}
            transition={item}
            clearSelection={clearSelection}
          />
        </div>
      </section>
    );
  }

  if (item.kind !== "clip") {
    return (
      <section className="properties-pane">
        <header className="properties-header">
          <span className="properties-label">Inspector</span>
          <span className="properties-header-meta">No active clip</span>
        </header>
        <div className="properties-empty">
          Select a clip or transition on the timeline to inspect it.
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
  const incomingCut = snapshot.cut_boundaries.find(
    (boundary) => boundary.to_clip_id === item.clip_uuid,
  );
  const outgoingCut = snapshot.cut_boundaries.find(
    (boundary) => boundary.from_clip_id === item.clip_uuid,
  );
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
    await editorDispatch.proposeUserEdit(ops);
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
        <PanelSection title="Identity">
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
        </PanelSection>
        {item.title ? (
          <>
            <PanelSection title="Title">
              <TitleEditor
                clipUuid={item.clip_uuid}
                title={item.title}
                startS={trackStart}
                endS={trackEnd}
              />
            </PanelSection>
            <PanelSection title="Motion">
              <MotionAnimationControl clip={item} />
            </PanelSection>
          </>
        ) : (
          <>
            <PanelSection title="Visual" revealLevel="pro">
              <ColorCorrectionControl
                clipUuid={item.clip_uuid}
                value={item.color_correction}
              />
              <LutControl clipUuid={item.clip_uuid} lutPath={item.lut_path} />
            </PanelSection>
            <PanelSection title="Audio">
              {(item.volume ?? DEFAULT_VOLUME) <= 0.001 && (
                <div className="properties-alert">
                  This clip is muted. Preview audio will be silent here.
                </div>
              )}
              <VolumeControl clipUuid={item.clip_uuid} value={item.volume} />
              <AudioFadeControl
                clipUuid={item.clip_uuid}
                fadeInS={item.fade_in_s}
                fadeOutS={item.fade_out_s}
              />
              <SplitEditControl
                clipUuid={item.clip_uuid}
                audioLeadS={item.audio_lead_s}
                audioTrailS={item.audio_trail_s}
                reason={item.split_edit_reason}
                confidence={item.split_edit_confidence}
              />
            </PanelSection>
            {(item.animations?.length ?? 0) > 0 && (
              <PanelSection title="Motion">
                <MotionAnimationControl clip={item} />
              </PanelSection>
            )}
            <PanelSection title="Timing">
              <SpeedControl clipUuid={item.clip_uuid} factor={item.speed} />
            </PanelSection>
            {(incomingCut || outgoingCut || item.split_edit_reason) && (
              <PanelSection title="Editorial">
                <EditorialIntent
                  incomingCut={incomingCut}
                  outgoingCut={outgoingCut}
                  splitEditReason={item.split_edit_reason}
                  splitEditConfidence={item.split_edit_confidence}
                />
              </PanelSection>
            )}
          </>
        )}
        {track?.audio && (
          <PanelSection title="Track Mix" revealLevel="pro">
            <TrackAudioControl trackName={track.name} audio={track.audio} />
          </PanelSection>
        )}
        <PanelSection title="Timing Metadata" revealLevel="advanced">
          <Field label="Source">
            <span className="properties-meta-row">
              <span className="properties-value">
                {sourceStart.toFixed(2)}s → {sourceEnd.toFixed(2)}s
              </span>
            </span>
          </Field>
          <Field label="Timeline">
            <span className="properties-meta-row">
              <span className="properties-value">
                {trackStart.toFixed(2)}s → {trackEnd.toFixed(2)}s
              </span>
            </span>
          </Field>
          <Field label="Duration">
            <span className="properties-meta-row">
              <span className="properties-value">{item.duration_s.toFixed(2)}s</span>
            </span>
          </Field>
          <Field label="Clip uuid">
            <code className="properties-code" title={item.clip_uuid}>
              {item.clip_uuid}
            </code>
          </Field>
        </PanelSection>
        <PanelSection title="Danger Zone" revealLevel="advanced">
          <div className="properties-action-row">
            <button className="properties-danger-pro" onClick={() => void deleteClip()}>
              Delete clip
            </button>
            {item.link_group_id && (
              <span className="properties-action-hint">Deletes linked audio/video</span>
            )}
          </div>
        </PanelSection>
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
type TimelineCutBoundary = import("../protocol").TimelineCutBoundary;

const TRANSITION_KIND_OPTIONS = [
  { value: "montage.cross_dissolve", label: "Cross Dissolve" },
  { value: "montage.match_dissolve", label: "Match Dissolve" },
  { value: "SMPTE_Dissolve", label: "SMPTE Dissolve" },
  { value: "montage.fade_black", label: "Fade Black" },
  { value: "montage.flash_white", label: "Flash White" },
  { value: "montage.wipe_left", label: "Wipe Left" },
  { value: "montage.wipe_right", label: "Wipe Right" },
  { value: "montage.slide_left", label: "Slide Left" },
  { value: "montage.slide_right", label: "Slide Right" },
  { value: "montage.smooth_push_left", label: "Smooth Push Left" },
  { value: "montage.motion_blur", label: "Motion Blur" },
  { value: "montage.whip_pan_left", label: "Whip Pan Left" },
  { value: "montage.whip_pan_right", label: "Whip Pan Right" },
  { value: "montage.pass_by_left", label: "Pass-By Left" },
  { value: "montage.pass_by_right", label: "Pass-By Right" },
  { value: "montage.iris_open", label: "Iris Open" },
  { value: "montage.iris_close", label: "Iris Close" },
  { value: "montage.invisible_cut", label: "Invisible Cut" },
  { value: "montage.zoom_in", label: "Zoom In" },
  { value: "montage.pixelize", label: "Pixelize" },
  { value: "montage.radial", label: "Radial" },
];
const HIGH_ATTENTION_TRANSITIONS = new Set([
  "montage.flash_white",
  "montage.motion_blur",
  "montage.whip_pan_left",
  "montage.whip_pan_right",
  "montage.pass_by_left",
  "montage.pass_by_right",
  "montage.iris_open",
  "montage.iris_close",
  "montage.zoom_in",
  "montage.pixelize",
  "montage.radial",
]);

function TransitionEditor({
  track,
  transition,
  clearSelection,
}: {
  track: TimelineTrack | null;
  transition: Extract<TimelineItem, { kind: "transition" }>;
  clearSelection: () => void;
}) {
  const adjacent = track ? adjacentTransitionClips(track, transition.index) : null;
  const transitionDensity = track ? recentTransitionDensity(track, transition) : null;
  const repeatedHighAttention =
    track && HIGH_ATTENTION_TRANSITIONS.has(transition.effect_name)
      ? recentSameTransitionCount(track, transition)
      : 0;
  const [kind, setKind] = useState(transition.effect_name);
  const [duration, setDuration] = useState(transition.duration_s);
  const [inOffset, setInOffset] = useState(transition.in_offset_s);
  const [outOffset, setOutOffset] = useState(transition.out_offset_s);

  useEffect(() => {
    setKind(transition.effect_name);
    setDuration(transition.duration_s);
    setInOffset(transition.in_offset_s);
    setOutOffset(transition.out_offset_s);
  }, [transition]);

  const canApply =
    adjacent !== null &&
    Number.isFinite(duration) &&
    duration > 0 &&
    Number.isFinite(inOffset) &&
    Number.isFinite(outOffset) &&
    inOffset >= 0 &&
    outOffset >= 0 &&
    Math.abs(inOffset + outOffset - duration) < 0.001;

  function setDurationScaled(nextDuration: number) {
    const clamped = Math.max(0.01, nextDuration);
    const currentTotal = Math.max(0.001, inOffset + outOffset);
    setDuration(clamped);
    setInOffset((inOffset / currentTotal) * clamped);
    setOutOffset((outOffset / currentTotal) * clamped);
  }

  function apply() {
    if (!canApply || adjacent === null) return;
    const op: EdlOp = {
      kind: "insert_transition",
      from: { kind: "clip_uuid", uuid: adjacent.from.clip_uuid },
      to: { kind: "clip_uuid", uuid: adjacent.to.clip_uuid },
      transitionKind: kind,
      durationS: duration,
      inOffsetS: inOffset,
      outOffsetS: outOffset,
    };
    editorDispatch.proposeUserEdit([op]).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (transition edit) failed", err);
    });
  }

  function remove() {
    if (adjacent === null) return;
    const op: EdlOp = {
      kind: "delete_transition",
      from: { kind: "clip_uuid", uuid: adjacent.from.clip_uuid },
      to: { kind: "clip_uuid", uuid: adjacent.to.clip_uuid },
    };
    editorDispatch.proposeUserEdit([op])
      .then(() => clearSelection())
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn("propose_user_edit (delete transition) failed", err);
      });
  }

  return (
    <>
      <PanelSection title="Transition">
        {adjacent === null && (
          <div className="properties-alert">
            Adjacent clips could not be resolved for this transition.
          </div>
        )}
        {transitionDensity !== null && transitionDensity >= 3 && (
          <div className="properties-warning">
            {transitionDensity} visible transitions land within this 30s window. Review
            whether this one still has a job.
          </div>
        )}
        {repeatedHighAttention >= 2 && (
          <div className="properties-warning">
            {formatIntentLabel(transition.effect_name.replace(/^montage\./, ""))} appears{" "}
            {repeatedHighAttention} times in this 30s window. Repeated high-attention
            transitions can read as style drift.
          </div>
        )}
        <Field label="Kind">
          <select
            className="properties-select"
            value={kind}
            onChange={(e) => setKind(e.target.value)}
          >
            {!TRANSITION_KIND_OPTIONS.some((option) => option.value === kind) && (
              <option value={kind}>{kind}</option>
            )}
            {TRANSITION_KIND_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </Field>
        {transition.transition_id && (
          <Field label="Intent">
            <div className="properties-intent-stack">
              <span className="properties-intent-chip">
                {formatIntentLabel(transition.transition_intent ?? "semantic_transition")}
              </span>
              <span className="properties-intent-meta">
                {[
                  transition.transition_id,
                  transition.transition_family,
                  transition.transition_direction
                    ? `direction ${transition.transition_direction}`
                    : null,
                  transition.transition_energy !== null
                    ? `energy ${transition.transition_energy.toFixed(2)}`
                    : null,
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
            </div>
          </Field>
        )}
        {transition.audio_policy && (
          <Field label="Audio">
            <div className="properties-intent-stack">
              <span className="properties-intent-chip">
                {transition.audio_policy === "crossfade" ? "Crossfade" : "Cut"}
              </span>
              <span className="properties-intent-meta">
                {transition.audio_policy === "crossfade"
                  ? "Adjacent source audio overlaps through the transition."
                  : "Picture overlaps, but dialogue/audio stays cut-style."}
              </span>
            </div>
          </Field>
        )}
        <Field label="Duration">
          <input
            type="number"
            className="properties-number-input"
            min={0.01}
            step={0.01}
            value={duration}
            onChange={(e) => setDurationScaled(parseFloat(e.target.value))}
          />
        </Field>
        <Field label="Incoming">
          <input
            type="number"
            className="properties-number-input"
            min={0}
            step={0.01}
            value={inOffset}
            onChange={(e) => setInOffset(parseFloat(e.target.value))}
          />
        </Field>
        <Field label="Outgoing">
          <input
            type="number"
            className="properties-number-input"
            min={0}
            step={0.01}
            value={outOffset}
            onChange={(e) => setOutOffset(parseFloat(e.target.value))}
          />
        </Field>
        <div className="properties-action-row">
          <button
            className="properties-apply"
            type="button"
            onClick={apply}
            disabled={!canApply}
          >
            Apply
          </button>
          <button className="properties-danger" type="button" onClick={remove}>
            Delete
          </button>
        </div>
      </PanelSection>
      <PanelSection title="Timing Metadata" revealLevel="advanced">
        <Field label="Timeline">
          <span className="properties-value">
            {transition.track_start_s.toFixed(2)}s →{" "}
            {(transition.track_start_s + transition.duration_s).toFixed(2)}s
          </span>
        </Field>
        <Field label="Cut">
          <span className="properties-value">
            {(transition.track_start_s + transition.in_offset_s).toFixed(2)}s
          </span>
        </Field>
      </PanelSection>
    </>
  );
}

function adjacentTransitionClips(track: TimelineTrack, transitionIndex: number) {
  const position = track.items.findIndex((item) => item.index === transitionIndex);
  if (position < 0) return null;
  const from = track.items[position - 1];
  const to = track.items[position + 1];
  if (from?.kind !== "clip" || to?.kind !== "clip") return null;
  return { from, to };
}

function recentTransitionDensity(
  track: TimelineTrack,
  transition: Extract<TimelineItem, { kind: "transition" }>,
) {
  const cutS = transitionCutS(transition);
  return track.items.filter((item) => {
    if (item.kind !== "transition") return false;
    const candidateCutS = transitionCutS(item);
    return candidateCutS >= cutS - 30 && candidateCutS <= cutS;
  }).length;
}

function recentSameTransitionCount(
  track: TimelineTrack,
  transition: Extract<TimelineItem, { kind: "transition" }>,
) {
  const cutS = transitionCutS(transition);
  return track.items.filter((item) => {
    if (item.kind !== "transition") return false;
    if (item.effect_name !== transition.effect_name) return false;
    const candidateCutS = transitionCutS(item);
    return candidateCutS >= cutS - 30 && candidateCutS <= cutS;
  }).length;
}

function transitionCutS(transition: Extract<TimelineItem, { kind: "transition" }>) {
  return transition.track_start_s + transition.in_offset_s;
}

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
      editorDispatch.proposeUserEdit([op]).catch((err) => {
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

function EditorialIntent({
  incomingCut,
  outgoingCut,
  splitEditReason,
  splitEditConfidence,
}: {
  incomingCut: TimelineCutBoundary | undefined;
  outgoingCut: TimelineCutBoundary | undefined;
  splitEditReason: string | null;
  splitEditConfidence: number | null;
}) {
  return (
    <>
      {incomingCut && <CutBoundaryField label="Incoming" boundary={incomingCut} />}
      {outgoingCut && <CutBoundaryField label="Outgoing" boundary={outgoingCut} />}
      {splitEditReason && (
        <Field label="Split edit">
          <div className="properties-intent-stack">
            <span className="properties-value">{splitEditReason}</span>
            {splitEditConfidence !== null && (
              <span className="properties-intent-meta">
                Confidence {Math.round(splitEditConfidence * 100)}%
              </span>
            )}
          </div>
        </Field>
      )}
    </>
  );
}

function CutBoundaryField({
  label,
  boundary,
}: {
  label: string;
  boundary: TimelineCutBoundary;
}) {
  const [cutType, setCutType] = useState(boundary.cut_type);
  const [intent, setIntent] = useState(boundary.intent);

  useEffect(() => {
    setCutType(boundary.cut_type);
    setIntent(boundary.intent);
  }, [boundary.key, boundary.cut_type, boundary.intent]);

  const from = { kind: "clip_uuid" as const, uuid: boundary.from_clip_id };
  const to = { kind: "clip_uuid" as const, uuid: boundary.to_clip_id };
  const reason =
    boundary.reason ??
    `Inspector update for ${formatIntentLabel(cutType).toLowerCase()} boundary.`;

  function applyCutIntent(nextCutType = cutType, nextIntent = intent) {
    const op: EdlOp = {
      kind: "set_cut_intent",
      from,
      to,
      cutType: nextCutType,
      intent: nextIntent || "manual_editorial_intent",
      audioRelation: boundary.audio_relation,
      energy: boundary.energy ?? undefined,
      confidence: boundary.confidence ?? 1,
      reason,
    };
    editorDispatch.proposeUserEdit([op]).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (cut intent) failed", err);
    });
  }

  function applySplitAlternative(kind: "j" | "l") {
    const op: EdlOp =
      kind === "j"
        ? {
            kind: "set_audio_lead",
            anchor: to,
            leadS: 0.35,
            reason: "inspector alternative: use a J-cut instead of a visible transition",
            confidence: boundary.confidence ?? 1,
          }
        : {
            kind: "set_audio_trail",
            anchor: from,
            trailS: 0.35,
            reason: "inspector alternative: use an L-cut instead of a visible transition",
            confidence: boundary.confidence ?? 1,
          };
    editorDispatch.proposeUserEdit([op]).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (split alternative) failed", err);
    });
  }

  const detail = [
    boundary.intent,
    boundary.audio_relation,
    boundary.energy !== null ? `energy ${boundary.energy.toFixed(2)}` : null,
    boundary.confidence !== null
      ? `confidence ${Math.round(boundary.confidence * 100)}%`
      : null,
  ].filter(Boolean);
  return (
    <Field label={label}>
      <div className="properties-intent-stack">
        <span className="properties-intent-chip">{formatIntentLabel(boundary.cut_type)}</span>
        <span className="properties-intent-meta">{detail.join(" · ")}</span>
        {boundary.reason && <span className="properties-value">{boundary.reason}</span>}
        <label className="properties-mini-field properties-cut-field">
          <span>Type</span>
          <select
            className="properties-select properties-cut-type-select"
            value={cutType}
            onChange={(e) => setCutType(e.target.value)}
          >
            {!CUT_TYPE_OPTIONS.some((option) => option.value === cutType) && (
              <option value={cutType}>{formatIntentLabel(cutType)}</option>
            )}
            {CUT_TYPE_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
        <label className="properties-mini-field properties-cut-field">
          <span>Intent</span>
          <input
            className="properties-text-input"
            value={intent}
            onChange={(e) => setIntent(e.target.value)}
          />
        </label>
        <div className="properties-action-row properties-alternative-row">
          <button
            className="properties-apply"
            type="button"
            onClick={() => applyCutIntent()}
          >
            Apply cut intent
          </button>
          <button
            className="properties-secondary"
            type="button"
            onClick={() => applyCutIntent("hard_cut", "low_attention_edit")}
          >
            Use hard cut
          </button>
          <button
            className="properties-secondary"
            type="button"
            onClick={() => applySplitAlternative("j")}
          >
            Use J-cut
          </button>
          <button
            className="properties-secondary"
            type="button"
            onClick={() => applySplitAlternative("l")}
          >
            Use L-cut
          </button>
        </div>
      </div>
    </Field>
  );
}

function SplitEditControl({
  clipUuid,
  audioLeadS,
  audioTrailS,
  reason,
  confidence,
}: {
  clipUuid: string;
  audioLeadS: number | null;
  audioTrailS: number | null;
  reason: string | null;
  confidence: number | null;
}) {
  const [lead, setLead] = useState(audioLeadS ?? 0);
  const [trail, setTrail] = useState(audioTrailS ?? 0);

  useEffect(() => {
    setLead(audioLeadS ?? 0);
    setTrail(audioTrailS ?? 0);
  }, [clipUuid, audioLeadS, audioTrailS]);

  function apply(kind: "lead" | "trail") {
    const value = kind === "lead" ? lead : trail;
    if (!Number.isFinite(value) || value < 0) return;
    const shared = {
      anchor: { kind: "clip_uuid", uuid: clipUuid } as const,
      reason: reason ?? "manual split edit",
      confidence: confidence ?? 1,
    };
    const op: EdlOp =
      kind === "lead"
        ? { kind: "set_audio_lead", leadS: value, ...shared }
        : { kind: "set_audio_trail", trailS: value, ...shared };
    editorDispatch.proposeUserEdit([op]).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (split edit) failed", err);
    });
  }

  return (
    <Field label="Split edit">
      <div className="properties-split-edit">
        <label className="properties-split-row">
          <span>Lead</span>
          <input
            type="number"
            className="properties-number-input"
            min={0}
            step={0.01}
            value={lead}
            onChange={(e) => setLead(parseFloat(e.target.value))}
          />
          <button className="properties-inline-apply" type="button" onClick={() => apply("lead")}>
            Apply
          </button>
        </label>
        <label className="properties-split-row">
          <span>Trail</span>
          <input
            type="number"
            className="properties-number-input"
            min={0}
            step={0.01}
            value={trail}
            onChange={(e) => setTrail(parseFloat(e.target.value))}
          />
          <button className="properties-inline-apply" type="button" onClick={() => apply("trail")}>
            Apply
          </button>
        </label>
      </div>
    </Field>
  );
}

function formatIntentLabel(value: string): string {
  return value
    .split("_")
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
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

function ValueBadge({
  value,
  unit,
  atRest,
  precision = 2,
}: {
  value: number;
  unit?: string;
  atRest?: boolean;
  precision?: number;
}) {
  return (
    <span className="properties-value-badge" data-rest={atRest ? "true" : "false"}>
      <span>{value.toFixed(precision)}</span>
      {unit ? <span className="unit">{unit}</span> : null}
    </span>
  );
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
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Reset local state when the clip's persisted value changes (or
  // the user selects a different clip).
  useEffect(() => {
    setLocal(initial);
    lastCommittedRef.current = initial;
  }, [clipUuid, initial]);

  function handleChange(next: number) {
    setLocal(next);
    if (debounceRef.current !== null) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      lastCommittedRef.current = next;
      invoke("set_clip_volume", { clipUuid, volume: next }).catch((err) => {
        // eslint-disable-next-line no-console
        console.warn("set_clip_volume failed", err);
      });
    }, COMMIT_DEBOUNCE_MS);
  }

  return (
    <Field label="Volume">
      <div className="properties-control-row">
        <Slider
          value={local}
          min={0}
          max={4}
          step={0.01}
          defaultValue={DEFAULT_VOLUME}
          onChange={handleChange}
          ariaLabel="Clip volume"
        />
        <ValueBadge value={local} unit="×" atRest={Math.abs(local - DEFAULT_VOLUME) < 0.005} />
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
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    setLocalIn(initialIn);
    setLocalOut(initialOut);
  }, [clipUuid, initialIn, initialOut]);

  function scheduleCommit(nextIn: number, nextOut: number) {
    if (debounceRef.current !== null) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      invoke("set_clip_fade", {
        clipUuid,
        fadeInS: Math.max(0, nextIn),
        fadeOutS: Math.max(0, nextOut),
      }).catch((err) => {
        // eslint-disable-next-line no-console
        console.warn("set_clip_fade failed", err);
      });
    }, COMMIT_DEBOUNCE_MS);
  }

  function handleInChange(next: number) {
    setLocalIn(next);
    scheduleCommit(next, localOut);
  }

  function handleOutChange(next: number) {
    setLocalOut(next);
    scheduleCommit(localIn, next);
  }

  return (
    <Field label="Fades">
      <div className="properties-fade-row">
        <FadeStepper
          label="In"
          value={localIn}
          step={0.05}
          onChange={handleInChange}
          ariaLabel="Fade in seconds"
        />
        <FadeStepper
          label="Out"
          value={localOut}
          step={0.05}
          onChange={handleOutChange}
          ariaLabel="Fade out seconds"
        />
      </div>
    </Field>
  );
}

function FadeStepper({
  label,
  value,
  step,
  onChange,
  ariaLabel,
}: {
  label: string;
  value: number;
  step: number;
  onChange: (next: number) => void;
  ariaLabel?: string;
}) {
  const bump = (delta: number) => {
    const next = Math.max(0, Math.round((value + delta) * 100) / 100);
    onChange(next);
  };
  return (
    <div className="properties-fade-cell">
      <span className="properties-fade-cell-label">{label}</span>
      <div className="properties-stepper">
        <input
          type="number"
          min={0}
          step={step}
          value={value}
          aria-label={ariaLabel}
          onChange={(e) => onChange(Math.max(0, parseFloat(e.target.value) || 0))}
        />
        <div className="properties-stepper-stack">
          <button type="button" aria-label={`Increase ${label}`} onClick={() => bump(step)}>
            ▴
          </button>
          <button type="button" aria-label={`Decrease ${label}`} onClick={() => bump(-step)}>
            ▾
          </button>
        </div>
      </div>
    </div>
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
    editorDispatch.proposeUserEdit(ops).catch((err) => {
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
        <div className="properties-mix-row">
          <Slider
            value={volume}
            min={0}
            max={2}
            step={0.01}
            defaultValue={1}
            onChange={setVolume}
            ariaLabel="Track volume"
          />
          <ValueBadge value={volume} unit="×" atRest={Math.abs(volume - 1) < 0.005} />
          <button
            type="button"
            className="properties-chip-toggle"
            data-on={muted ? "true" : "false"}
            data-tone="mute"
            onClick={() => setMuted(!muted)}
            aria-pressed={muted}
          >
            Mute
          </button>
          <button
            type="button"
            className="properties-chip-toggle"
            data-on={solo ? "true" : "false"}
            data-tone="solo"
            onClick={() => setSolo(!solo)}
            aria-pressed={solo}
          >
            Solo
          </button>
        </div>
      </Field>
      {role !== "dialogue" && (
        <Field label="Ducking">
          <div className="properties-ducking-row">
            <button
              type="button"
              className="properties-chip-toggle"
              data-on={ducking ? "true" : "false"}
              data-tone="duck"
              onClick={() => setDucking(!ducking)}
              aria-pressed={ducking}
            >
              {ducking ? "On" : "Off"}
            </button>
            <div className="properties-stepper">
              <input
                type="number"
                step={1}
                value={amountDb}
                aria-label="Ducking amount in dB"
                onChange={(e) => setAmountDb(parseFloat(e.target.value) || -12)}
              />
              <div className="properties-stepper-stack">
                <button
                  type="button"
                  aria-label="Increase ducking"
                  onClick={() => setAmountDb(amountDb + 1)}
                >
                  ▴
                </button>
                <button
                  type="button"
                  aria-label="Decrease ducking"
                  onClick={() => setAmountDb(amountDb - 1)}
                >
                  ▾
                </button>
              </div>
            </div>
            <span className="properties-value-badge">
              <span className="unit">dB</span>
            </span>
          </div>
        </Field>
      )}
      {dirty && (
        <Field label="Track audio">
          <button className="properties-inline-apply" type="button" onClick={apply}>
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
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    setLocal(initial);
  }, [clipUuid, initial]);

  function handleChange(next: number) {
    if (!isFinite(next) || next <= 0) return;
    setLocal(next);
    if (debounceRef.current !== null) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      invoke("set_clip_speed", { clipUuid, speed: next }).catch((err) => {
        // eslint-disable-next-line no-console
        console.warn("set_clip_speed failed", err);
      });
    }, COMMIT_DEBOUNCE_MS);
  }

  return (
    <Field label="Speed">
      <div className="properties-control-row">
        <Slider
          value={local}
          min={0.25}
          max={4}
          step={0.05}
          defaultValue={DEFAULT_SPEED}
          onChange={handleChange}
          ariaLabel="Clip speed"
        />
        <ValueBadge value={local} unit="×" atRest={Math.abs(local - DEFAULT_SPEED) < 0.025} />
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

function toColorStyling(
  v: ColorCorrectionValue,
): import("../protocol").ColorCorrectionStyling {
  return {
    exposure_ev: v.exposureEv,
    contrast: v.contrast,
    saturation: v.saturation,
    temperature: v.temperature,
    tint: v.tint,
    shadows: v.shadows,
    highlights: v.highlights,
  };
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
  const setPreviewOverride = useColorPreviewOverride((s) => s.setOverride);
  const clearPreviewOverride = useColorPreviewOverride((s) => s.clearOverride);

  useEffect(() => {
    const next = normalizeColorCorrection(value);
    setLocal(next);
    lastCommittedRef.current = colorSignature(next);
    // Persisted values caught up (accept landed / clip changed) — the
    // monitor should render from the timeline again, not the drag.
    clearPreviewOverride();
  }, [clipUuid, value, clearPreviewOverride]);

  // Leaving the control (deselect, panel switch) ends the live
  // preview for this clip; a newer clip's override is left alone.
  useEffect(() => {
    return () => clearPreviewOverride(clipUuid);
  }, [clipUuid, clearPreviewOverride]);

  const currentSig = colorSignature(local);
  const dirty = currentSig !== lastCommittedRef.current;

  function setField<K extends keyof ColorCorrectionValue>(
    key: K,
    nextValue: number,
  ) {
    setLocal((prev) => {
      const next = { ...prev, [key]: nextValue };
      // Live monitor feedback while dragging — before Apply/Accept.
      setPreviewOverride({ clipUuid, values: toColorStyling(next) });
      return next;
    });
  }

  function apply() {
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
    editorDispatch.proposeUserEdit([op]).catch((err) => {
      clearPreviewOverride(clipUuid);
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
        unit="EV"
        defaultValue={DEFAULT_COLOR.exposureEv}
        onChange={(next) => setField("exposureEv", next)}
      />
      <ColorSlider
        label="Contrast"
        value={local.contrast}
        min={0}
        max={3}
        step={0.05}
        unit="×"
        defaultValue={DEFAULT_COLOR.contrast}
        onChange={(next) => setField("contrast", next)}
      />
      <ColorSlider
        label="Saturation"
        value={local.saturation}
        min={0}
        max={3}
        step={0.05}
        unit="×"
        defaultValue={DEFAULT_COLOR.saturation}
        onChange={(next) => setField("saturation", next)}
      />
      <ColorSlider
        label="Temp"
        value={local.temperature}
        min={-1}
        max={1}
        step={0.02}
        defaultValue={DEFAULT_COLOR.temperature}
        onChange={(next) => setField("temperature", next)}
      />
      <ColorSlider
        label="Tint"
        value={local.tint}
        min={-1}
        max={1}
        step={0.02}
        defaultValue={DEFAULT_COLOR.tint}
        onChange={(next) => setField("tint", next)}
      />
      <ColorSlider
        label="Shadows"
        value={local.shadows}
        min={-1}
        max={1}
        step={0.02}
        defaultValue={DEFAULT_COLOR.shadows}
        onChange={(next) => setField("shadows", next)}
      />
      <ColorSlider
        label="Highlights"
        value={local.highlights}
        min={-1}
        max={1}
        step={0.02}
        defaultValue={DEFAULT_COLOR.highlights}
        onChange={(next) => setField("highlights", next)}
      />
      {dirty && (
        <Field label="Color">
          <button className="properties-inline-apply" type="button" onClick={apply}>
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
  unit,
  defaultValue,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  unit?: string;
  defaultValue: number;
  onChange: (value: number) => void;
}) {
  const atRest = Math.abs(value - defaultValue) < step / 2;
  return (
    <Field label={label}>
      <div className="properties-control-row">
        <Slider
          value={value}
          min={min}
          max={max}
          step={step}
          defaultValue={defaultValue}
          onChange={onChange}
          ariaLabel={label}
        />
        <ValueBadge value={value} unit={unit} atRest={atRest} />
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
  // Project .cube files for the dropdown — scanned once per mount;
  // the free-text input remains for paths the scan missed.
  const [available, setAvailable] = useState<string[]>([]);

  useEffect(() => {
    let stale = false;
    invoke<string[]>("list_preview_luts")
      .then((luts) => {
        if (!stale) setAvailable(luts);
      })
      .catch(() => {
        // No project / scan failure — dropdown simply doesn't render.
      });
    return () => {
      stale = true;
    };
  }, []);

  useEffect(() => {
    setLocal(initial);
    lastCommittedRef.current = initial;
  }, [clipUuid, initial]);

  const dirty = local !== lastCommittedRef.current;
  const trimmed = local.trim();
  const lutExtension = trimmed.split(".").pop()?.toLowerCase() ?? "";
  const canApply =
    dirty &&
    trimmed.length > 0 &&
    !trimmed.startsWith("/") &&
    !trimmed.includes("\\") &&
    !trimmed.split("/").some((part) => part === "." || part === ".." || part === "") &&
    SUPPORTED_LUT_EXTENSIONS.has(lutExtension);
  const canRemove = lastCommittedRef.current.length > 0;

  function apply() {
    if (!canApply) return;
    lastCommittedRef.current = trimmed;
    const op: EdlOp = {
      kind: "apply_lut",
      anchor: { kind: "clip_uuid", uuid: clipUuid },
      lutPath: trimmed,
    };
    editorDispatch.proposeUserEdit([op]).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (apply_lut) failed", err);
    });
  }

  function remove() {
    lastCommittedRef.current = "";
    setLocal("");
    const op: EdlOp = {
      kind: "remove_lut",
      anchor: { kind: "clip_uuid", uuid: clipUuid },
    };
    editorDispatch.proposeUserEdit([op]).catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("propose_user_edit (remove_lut) failed", err);
    });
  }

  // No `firstFrameAverage` field exists on TimelineItem yet, so a loaded
  // LUT always paints the neutral checkerboard fallback. When the backend
  // starts shipping an average colour, swap the swatch's inline style
  // here. See LutSwatch below.
  const lutState: "empty" | "loaded" =
    lastCommittedRef.current.length > 0 ? "loaded" : "empty";

  return (
    <Field label="LUT">
      <div className="properties-lut-row">
        <LutSwatch state={lutState} />
        {available.length > 0 && (
          <select
            className="properties-select"
            value={available.includes(local) ? local : ""}
            onChange={(e) => {
              if (e.target.value) setLocal(e.target.value);
            }}
            aria-label="Choose a project LUT"
          >
            <option value="">
              {local && !available.includes(local) ? "(custom path)" : "Choose LUT…"}
            </option>
            {available.map((path) => (
              <option key={path} value={path}>
                {path}
              </option>
            ))}
          </select>
        )}
        <input
          type="text"
          className="properties-text-input"
          value={local}
          placeholder="luts/show-look.cube"
          onChange={(e) => setLocal(e.target.value)}
        />
        {dirty && (
          <button
            className="properties-inline-apply"
            type="button"
            onClick={apply}
            disabled={!canApply}
          >
            Apply
          </button>
        )}
        {canRemove && (
          <button className="properties-inline-apply" type="button" onClick={remove}>
            Clear
          </button>
        )}
      </div>
    </Field>
  );
}

function LutSwatch({ state }: { state: "empty" | "loaded" }) {
  return (
    <span
      className="properties-lut-swatch"
      data-state={state}
      aria-hidden
      title={state === "empty" ? "No LUT applied" : "LUT applied"}
    />
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

function PanelSection({
  title,
  children,
  revealLevel,
}: {
  title: string;
  children: React.ReactNode;
  revealLevel?: RevealLevel;
}) {
  if (revealLevel !== undefined) {
    return (
      <CollapsiblePanel title={title} revealLevel={revealLevel}>
        {children}
      </CollapsiblePanel>
    );
  }
  return (
    <section className="properties-section">
      <h3>{title}</h3>
      {children}
    </section>
  );
}
