/**
 * EvidenceDrillDown — kind→panel switch invoked by `EvidenceChipRow`.
 *
 * Owns the modal shell; the per-kind components only render the body.
 * Keeping the switch here (rather than inside `EvidenceChipRow`) keeps
 * the chip row dumb and lets the drill-down internals evolve without
 * touching the chip layout.
 */

import { invoke, isTauri } from "@tauri-apps/api/core";
import {
  useAudioSamples,
  useCaptions,
  useColorStats,
  useEpisodes,
  useFaces,
  useMotionRegions,
  useScenes,
  useSilences,
  useSpeakers,
  useTranscriptSegments,
  type EvidenceAvailability,
} from "../../../state/evidenceAccessors";
import { AudioDrillDown } from "./AudioDrillDown";
import { CaptionsDrillDown } from "./CaptionsDrillDown";
import { ColorDrillDown } from "./ColorDrillDown";
import { DrillDownModal } from "./DrillDownModal";
import { EpisodesDrillDown } from "./EpisodesDrillDown";
import { FacesDrillDown } from "./FacesDrillDown";
import { MotionDrillDown } from "./MotionDrillDown";
import { ScenesDrillDown } from "./ScenesDrillDown";
import { SilencesDrillDown } from "./SilencesDrillDown";
import { SpeakersDrillDown } from "./SpeakersDrillDown";
import { TranscriptDrillDown } from "./TranscriptDrillDown";

export type DrillDownKind = keyof EvidenceAvailability;

interface EvidenceDrillDownProps {
  kind: DrillDownKind;
  label: string;
  onClose: () => void;
}

export function EvidenceDrillDown({ kind, label, onClose }: EvidenceDrillDownProps) {
  // Counts live in the title row; subscribe at the router so the count
  // header reflects the latest accessor value even when the inner
  // panel re-renders.
  const count = useCountFor(kind);

  function runIndexers() {
    if (!isTauri()) return;
    invoke("index_project").catch((err) => {
      // eslint-disable-next-line no-console
      console.warn("index_project failed", err);
    });
    onClose();
  }

  return (
    <DrillDownModal title={label} count={count} onClose={onClose}>
      <PanelFor kind={kind} onRunIndexers={runIndexers} />
    </DrillDownModal>
  );
}

function PanelFor({
  kind,
  onRunIndexers,
}: {
  kind: DrillDownKind;
  onRunIndexers: () => void;
}) {
  switch (kind) {
    case "scenes":
      return <ScenesDrillDown onRunIndexers={onRunIndexers} />;
    case "speakers":
      return <SpeakersDrillDown onRunIndexers={onRunIndexers} />;
    case "silences":
      return <SilencesDrillDown onRunIndexers={onRunIndexers} />;
    case "transcript":
      return <TranscriptDrillDown onRunIndexers={onRunIndexers} />;
    case "captions":
      return <CaptionsDrillDown onRunIndexers={onRunIndexers} />;
    case "audio":
      return <AudioDrillDown onRunIndexers={onRunIndexers} />;
    case "faces":
      return <FacesDrillDown onRunIndexers={onRunIndexers} />;
    case "motion":
      return <MotionDrillDown onRunIndexers={onRunIndexers} />;
    case "color":
      return <ColorDrillDown onRunIndexers={onRunIndexers} />;
    case "episodes":
      return <EpisodesDrillDown onRunIndexers={onRunIndexers} />;
  }
}

function useCountFor(kind: DrillDownKind): number {
  // We have to call hooks unconditionally — call every accessor and
  // pick the active one. Each accessor returns [] when its data isn't
  // loaded, so the unused subscriptions are cheap.
  const scenes = useScenes().length;
  const speakers = useSpeakers().length;
  const silences = useSilences().length;
  const transcript = useTranscriptSegments().length;
  const captions = useCaptions().length;
  const audio = useAudioSamples().length;
  const faces = useFaces().length;
  const motion = useMotionRegions().length;
  const color = useColorStats().length;
  const episodes = useEpisodes().length;
  switch (kind) {
    case "scenes":
      return scenes;
    case "speakers":
      return speakers;
    case "silences":
      return silences;
    case "transcript":
      return transcript;
    case "captions":
      return captions;
    case "audio":
      return audio;
    case "faces":
      return faces;
    case "motion":
      return motion;
    case "color":
      return color;
    case "episodes":
      return episodes;
  }
}
