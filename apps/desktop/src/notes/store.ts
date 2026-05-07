// Zustand store for the editorial notes lifecycle (Phase 1.7).
//
// Source of truth is .awidat/notes.json on disk. The store loads it
// on project change, ingests Item::EditorialNote emissions from the
// agent, and writes back through the Tauri commands.
//
// Two write paths:
//   1) Agent surfaces a note → Item::EditorialNote arrives →
//      ingest() upserts into the store + persists.
//   2) User clicks Dismiss / Resolve → setStatus() writes
//      through; on Dismiss it also stamps a coarse pattern in
//      dismissed_patterns.json so the agent stops re-surfacing
//      that kind.

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type {
  ContinuityVerdictTag,
  EditorialNoteKind,
  EditorialNoteStatus,
  Item,
} from "../protocol";

/** One note as the panel renders it. Mirrors PersistedNote on the
 *  Rust side plus a local "dirty" flag for the writes-in-flight
 *  state (kept lightweight; the panel re-disables a button on
 *  click and re-enables on resolve). */
export type Note = {
  id: string;
  kind: EditorialNoteKind;
  status: EditorialNoteStatus;
  anchorAtS: number;
  summary: string;
  suggestedProposal?: string;
  /** For `continuity_warning` notes only — drives card color in
   *  the panel. `undefined` for other kinds. */
  continuityVerdict?: ContinuityVerdictTag;
  /** Per-rule reasons for `continuity_warning` notes (rendered as
   *  bullets under the summary). `undefined` for other kinds. */
  continuityReasons?: string[];
};

/** Disk shape — must match crates/core/src/notes.rs::NotesFile. */
type NotesFile = {
  version: number;
  notes: Array<{
    id: string;
    kind: string;
    status: string;
    anchor_at_s: number;
    summary: string;
    suggested_proposal?: string;
    continuity_verdict?: string;
    continuity_reasons?: string[];
  }>;
};

/** Coarse pattern key the dismissal-memory matcher uses. Mirrors
 *  awidat_core::dismissal::DismissalBucket. */
export type DismissalBucket =
  | { kind: "silence_under_2s" }
  | { kind: "silence_2s_to_5s" }
  | { kind: "silence_over_5s" }
  | { kind: "filler_basic" }
  | { kind: "filler_aggressive" }
  | { kind: "false_start" };

type State = {
  notes: Note[];
  /** True while an async load/write is in flight. UI dims the
   *  panel during a write so rapid clicks can't double-fire. */
  busy: boolean;
  /** Refresh from disk. Called on project change + after writes. */
  refresh: () => Promise<void>;
  /** Ingest a fresh agent emission. Upserts by id. */
  ingest: (item: Extract<Item, { kind: "editorial_note" }>) => Promise<void>;
  /** Mark a note resolved or dismissed. When dismissing, also
   *  stamp the coarse pattern via the Tauri command. */
  setStatus: (
    id: string,
    status: EditorialNoteStatus,
    dismissBucket?: DismissalBucket,
  ) => Promise<void>;
  /** Clear local state (project switch). */
  clear: () => void;
};

function deserialize(file: NotesFile): Note[] {
  return file.notes.map((n) => ({
    id: n.id,
    kind: n.kind as EditorialNoteKind,
    status: n.status as EditorialNoteStatus,
    anchorAtS: n.anchor_at_s,
    summary: n.summary,
    suggestedProposal: n.suggested_proposal,
    continuityVerdict: n.continuity_verdict as ContinuityVerdictTag | undefined,
    continuityReasons: n.continuity_reasons,
  }));
}

export const useNotesStore = create<State>((set, get) => ({
  notes: [],
  busy: false,
  refresh: async () => {
    set({ busy: true });
    try {
      const file = await invoke<NotesFile>("list_notes");
      set({ notes: deserialize(file), busy: false });
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("list_notes failed", e);
      set({ busy: false });
    }
  },
  ingest: async (item) => {
    // Map wire shape → persistence shape. Server-side upsert keeps
    // the on-disk file authoritative; we re-read after the write
    // so the store's view matches disk exactly.
    try {
      await invoke<NotesFile>("upsert_note", {
        note: {
          id: item.id,
          kind: item.note_kind,
          status: item.status,
          anchor_at_s: item.anchor_at_s,
          summary: item.summary,
          suggested_proposal: item.suggested_proposal ?? null,
          continuity_verdict: item.continuity_verdict ?? null,
          continuity_reasons: item.continuity_reasons ?? null,
        },
      });
      await get().refresh();
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("upsert_note failed", e);
    }
  },
  setStatus: async (id, status, dismissBucket) => {
    set({ busy: true });
    try {
      await invoke("set_note_status", {
        args: {
          id,
          status,
          dismiss_bucket: dismissBucket ?? null,
        },
      });
      await get().refresh();
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("set_note_status failed", e);
      set({ busy: false });
    }
  },
  clear: () => set({ notes: [], busy: false }),
}));
