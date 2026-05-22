/**
 * Workflow lenses — the human-facing working surfaces:
 *   Import · Index · Selects · Review · Captions · Audio · Color · Delivery.
 *
 * A lens represents *which view the user is working in*. It is navigation, not state.
 * Stages (see ./stages.ts) and lenses are orthogonal — a user can be in the Proposal
 * stage viewing the Review lens, or in the Deliver stage viewing the Color lens, etc.
 *
 * Assembly is intentionally not a top-level lens: in an agent-first editor, building
 * the rough cut is agent work surfaced inside Proposal/Review.
 */

import { create } from "zustand";

export const LENSES = [
  "import",
  "index",
  "selects",
  "review",
  "captions",
  "audio",
  "color",
  "delivery",
] as const;
export type Lens = (typeof LENSES)[number];

export const LENS_LABEL: Record<Lens, string> = {
  import: "Import",
  index: "Index",
  selects: "Selects",
  review: "Review",
  captions: "Captions",
  audio: "Audio",
  color: "Color",
  delivery: "Delivery",
};

export type LensStore = {
  current: Lens;
  set: (lens: Lens) => void;
  reset: () => void;
};

export const useLensStore = create<LensStore>((set) => ({
  current: "review",
  set: (lens) => set({ current: lens }),
  reset: () => set({ current: "review" }),
}));
