// Right-rail properties inspector — third pane in the workspace top
// row. Reads the selected clip from useTimelineSelectionStore and
// renders read-only metadata + interactive Volume / Speed controls
// (Step 15.5). Slider/input edits debounce 300ms then fire a
// propose_user_edit envelope so the change rides the same Step-5
// proposal pipeline (ghost overlay, Accept/Reject) as drag-trim.
//
// Future Step 16 grows a Title editor into this same pane.

import { useEffect, useRef, useState } from "react";
import { useTimelineStore } from "../timeline/store";
import type { TimelineItem, TimelineTrack } from "../timeline/store";
import { useTimelineSelectionStore } from "./store";
import { useMediaStore } from "../media/store";
import type { EdlOp } from "../timeline/edlBuilder";
import { MotionAnimationControl } from "./MotionAnimationControl";
import { editorDispatch } from "../editor/tauriDispatch";

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
            <PanelSection title="Visual">
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
          <PanelSection title="Track Mix">
            <TrackAudioControl trackName={track.name} audio={track.audio} />
          </PanelSection>
        )}
        <PanelSection title="Timing Metadata">
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
        </PanelSection>
        <PanelSection title="Danger Zone">
          <div className="properties-action-row">
            <button className="properties-danger" onClick={() => void deleteClip()}>
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
  { value: "awidat.cross_dissolve", label: "Cross Dissolve" },
  { value: "awidat.match_dissolve", label: "Match Dissolve" },
  { value: "SMPTE_Dissolve", label: "SMPTE Dissolve" },
  { value: "awidat.fade_black", label: "Fade Black" },
  { value: "awidat.flash_white", label: "Flash White" },
  { value: "awidat.wipe_left", label: "Wipe Left" },
  { value: "awidat.wipe_right", label: "Wipe Right" },
  { value: "awidat.slide_left", label: "Slide Left" },
  { value: "awidat.slide_right", label: "Slide Right" },
  { value: "awidat.smooth_push_left", label: "Smooth Push Left" },
  { value: "awidat.motion_blur", label: "Motion Blur" },
  { value: "awidat.whip_pan_left", label: "Whip Pan Left" },
  { value: "awidat.whip_pan_right", label: "Whip Pan Right" },
  { value: "awidat.pass_by_left", label: "Pass-By Left" },
  { value: "awidat.pass_by_right", label: "Pass-By Right" },
  { value: "awidat.iris_open", label: "Iris Open" },
  { value: "awidat.iris_close", label: "Iris Close" },
  { value: "awidat.invisible_cut", label: "Invisible Cut" },
  { value: "awidat.zoom_in", label: "Zoom In" },
  { value: "awidat.pixelize", label: "Pixelize" },
  { value: "awidat.radial", label: "Radial" },
];
const HIGH_ATTENTION_TRANSITIONS = new Set([
  "awidat.flash_white",
  "awidat.motion_blur",
  "awidat.whip_pan_left",
  "awidat.whip_pan_right",
  "awidat.pass_by_left",
  "awidat.pass_by_right",
  "awidat.iris_open",
  "awidat.iris_close",
  "awidat.zoom_in",
  "awidat.pixelize",
  "awidat.radial",
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
            {formatIntentLabel(transition.effect_name.replace(/^awidat\./, ""))} appears{" "}
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
      <PanelSection title="Timing Metadata">
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
        <label className="properties-mini-field">
          <span>Lead</span>
          <input
            type="number"
            className="properties-number-input"
            min={0}
            step={0.01}
            value={lead}
            onChange={(e) => setLead(parseFloat(e.target.value))}
          />
          <button className="properties-apply" type="button" onClick={() => apply("lead")}>
            Apply
          </button>
        </label>
        <label className="properties-mini-field">
          <span>Trail</span>
          <input
            type="number"
            className="properties-number-input"
            min={0}
            step={0.01}
            value={trail}
            onChange={(e) => setTrail(parseFloat(e.target.value))}
          />
          <button className="properties-apply" type="button" onClick={() => apply("trail")}>
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
    editorDispatch.proposeUserEdit([op]).catch((err) => {
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
    editorDispatch.proposeUserEdit([op]).catch((err) => {
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
    editorDispatch.proposeUserEdit([op]).catch((err) => {
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
    editorDispatch.proposeUserEdit([op]).catch((err) => {
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
        {canRemove && (
          <button className="properties-apply" type="button" onClick={remove}>
            Clear
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

function PanelSection({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="properties-section">
      <h3>{title}</h3>
      {children}
    </section>
  );
}
