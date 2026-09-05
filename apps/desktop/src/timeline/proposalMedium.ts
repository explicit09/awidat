import type { ActiveProposal } from "./proposal";

export type ProposalMedium =
  | "cut" // delete/trim/split/ripple/move — review on timeline
  | "color" // set_color_correction / apply_lut — review on preview
  | "audio" // volume / fade / lead / trail / fx / ducking — review on waveform
  | "transition" // insert/delete_transition — review on timeline
  | "broll" // insert_b_roll diff hint — review on preview + insertion
  | "title" // insert_title / set_title — review on preview
  | "caption" // insert_caption / set_caption / caption_* — review in transcript
  | "mixed" // proposal touches >1 medium — Brief decides
  | "other"; // anything we didn't classify (animations, track tail, etc.)

/**
 * Mediums each op heading maps to. These are the exact `***` headings
 * emitted by `edlBuilder.ts::appendOp` plus the agent-side variants
 * the backend produces. Anything not listed falls through to "other".
 */
const HEADING_MEDIUM: Record<string, ProposalMedium> = {
  // Cut family — anything that moves, removes, or splits clips.
  "Trim Clip": "cut",
  "Delete Clip": "cut",
  "Split Clip": "cut",
  "Move Clip": "cut",
  "Ripple Move": "cut",
  "Ripple Delete": "cut",
  "Ripple Trim": "cut",
  "Delete Gap": "cut",
  "Trim Track Tail": "cut",
  "Delete Track": "cut",
  "Professional Timeline Edit": "cut",
  "Set Speed": "cut",
  // Color family.
  "Set Color Correction": "color",
  "Apply LUT": "color",
  "Remove LUT": "color",
  // Audio family — everything touching levels, fades, fx, ducking.
  "Set Volume": "audio",
  "Set Audio Fade": "audio",
  "Set Audio Lead": "audio",
  "Set Audio Trail": "audio",
  "Set Track Audio": "audio",
  "Set Ducking": "audio",
  "Set Sync Group": "audio",
  "Set Clip Audio FX": "audio",
  "Set Track Audio FX": "audio",
  // Transition family.
  "Insert Transition": "transition",
  "Delete Transition": "transition",
  "Set Cut Intent": "transition",
  // Title family.
  "Insert Title": "title",
  "Set Title": "title",
  // Caption family — its own medium since captions live on the
  // transcript track, not the title overlay surface.
  "Insert Caption": "caption",
  "Set Caption": "caption",
};

/**
 * Walk the proposal's EDL text + diff hints and decide which medium
 * the proposal belongs to. Returns "mixed" when the proposal spans
 * more than one medium, "other" when nothing classifiable is found.
 */
export function deriveMedium(proposal: ActiveProposal): ProposalMedium {
  const mediums = new Set<ProposalMedium>();

  // 1. Scan `*** <Heading>` lines in the EDL text. This catches the
  //    full set in `HEADING_MEDIUM` above — including color/audio/
  //    transition/title which the diff_hints stream doesn't expose.
  const headingRe = /^\*\*\* (?!Begin EDL|End EDL)(.+?)$/gm;
  for (const match of proposal.edlText.matchAll(headingRe)) {
    const medium = HEADING_MEDIUM[match[1].trim()];
    if (medium) mediums.add(medium);
  }

  // 2. Diff hints catch the b-roll case the headings can't — b-roll
  //    arrives as a normal `Insert Clip` heading but the backend tags
  //    it via the `insert_b_roll` hint kind. Picture-in-picture is
  //    classed as b-roll too (same review surface — preview overlay).
  for (const hint of proposal.diffHints) {
    if (hint.kind === "insert_b_roll" || hint.kind === "insert_pi_p") {
      mediums.add("broll");
    }
  }

  if (mediums.size === 0) return "other";
  if (mediums.size === 1) return mediums.values().next().value as ProposalMedium;
  return "mixed";
}
