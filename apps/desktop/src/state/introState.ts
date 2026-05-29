/**
 * useIntroState — tracks which projects have already received their
 * one-shot synthetic editorial intro turn (Wave 3 task F3).
 *
 * Why a separate store: the project lifecycle effect in `appGlue.ts`
 * fires every time `current` changes (project open, close, switch).
 * We want to inject a synthetic `start_turn` exactly once per project
 * per persisted-state-clear, even across reloads of the same session.
 *
 * Persistence: project paths are kept in localStorage under
 * `STORAGE_KEY`. The Settings modal's "Re-introduce on project open"
 * action calls `reset()` to wipe the set; the next open of any
 * project will then fire the intro again.
 *
 * The synthetic-prompt body itself lives in `INTRO_PROMPT` so callers
 * (the dispatcher in appGlue) and any UI that needs to hide it
 * (ChatStream, App.tsx) share the same sentinel prefix.
 */

import { create } from "zustand";

/** Sentinel prefix the synthetic intro turn carries. Anything starting
 *  with this string is hidden from the conversation transcript — it is
 *  an editorial instruction, not a user message. Keep this constant
 *  load-bearing: it is the contract between the dispatcher (appGlue),
 *  the chat renderer (ChatStream.tsx), and the turn grouping in App.tsx. */
export const INTRO_PROMPT_PREFIX = "[awidat:intro]";

/** Sentinel prefix the explicit "Prepare a starting cut" action carries
 *  (Wave 3 B4). Same filtering contract as the intro prefix — the
 *  message is an editorial instruction to the model, not a user
 *  question, so the chat transcript hides it. Keep this load-bearing. */
export const PREPARE_PROMPT_PREFIX = "[awidat:prepare]";

/** The body fired by the Brief surface's "Prepare a starting cut"
 *  button (B4). Asks the agent for an editorial first pass grounded in
 *  AGENTS.md + indexer outputs and emitted as per-edit proposals. */
export const PREPARE_PROMPT = `${PREPARE_PROMPT_PREFIX}
Prepare a starting cut for this project. Use AGENTS.md as the editorial brief.
Use the indexer outputs to ground your suggestions. Aim for an editorial first
pass that the user can review and refine — not a finished cut.

Emit each proposed edit as a separate approval/proposal so the user can
accept/reject individually. Include a one-sentence rationale for each.`;

/** All sentinel prefixes that mean "this is editorial guidance, not a user message".
 *  Add new prefixes here and every renderer / turn-grouper picks them up. */
export const AWIDAT_SENTINEL_PREFIXES: readonly string[] = [
  INTRO_PROMPT_PREFIX,
  PREPARE_PROMPT_PREFIX,
];

/** True if `text` carries any registered awidat sentinel. The canonical
 *  cross-component "should this be hidden from transcript?" check. */
export function isAwidatSentinel(text: string): boolean {
  for (const prefix of AWIDAT_SENTINEL_PREFIXES) {
    if (text.startsWith(prefix)) return true;
  }
  return false;
}

/** The editorial-system body the synthetic turn carries. Written as a
 *  meta-instruction so the model treats it as guidance rather than a
 *  user request. Voice: terse, evidence-grounded, no hand-holding. */
export const INTRO_PROMPT = `${INTRO_PROMPT_PREFIX}
You've just been opened on a project. The user has not asked anything yet.
Introduce yourself in 2-3 short sentences:
1. State what you've read (AGENTS.md if present, indexer signals available).
2. State what you can do for this project_type given the skills loaded.
3. Offer ONE next step (e.g., "Say the word and I'll prepare a starting cut.").
Do not call any tools. Do not propose any edits. Wait for the user to direct you.`;

export const STORAGE_KEY = "awidat:intro:introduced";

/** True if the message text is a synthetic intro turn. */
export function isIntroSyntheticInput(text: string): boolean {
  return text.startsWith(INTRO_PROMPT_PREFIX);
}

interface StorageAdapter {
  getItem(k: string): string | null;
  setItem(k: string, v: string): void;
  removeItem(k: string): void;
}

interface IntroStateStore {
  /** Project paths that have already seen their intro turn. */
  introduced: Set<string>;
  /** True after `markIntroduced` has been called for `path`. */
  hasIntroduced: (path: string) => boolean;
  /** Record that `path` has now seen its intro turn (persisted). */
  markIntroduced: (path: string) => void;
  /** Wipe the set — the Settings "Re-introduce on project open" action. */
  reset: () => void;
}

interface CreateOpts {
  persist?: boolean;
  storage?: StorageAdapter;
}

function loadInitial(storage: StorageAdapter | null): Set<string> {
  if (!storage) return new Set();
  const raw = storage.getItem(STORAGE_KEY);
  if (!raw) return new Set();
  try {
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed)) {
      return new Set(parsed.filter((entry): entry is string => typeof entry === "string"));
    }
  } catch {
    // Corrupt entry — fall through to the empty set rather than crash.
  }
  return new Set();
}

function persistSet(storage: StorageAdapter | null, set: Set<string>): void {
  if (!storage) return;
  try {
    storage.setItem(STORAGE_KEY, JSON.stringify(Array.from(set)));
  } catch {
    // Quota/serialization failures are non-fatal; the worst case is
    // the intro fires again next session for the affected project.
  }
}

/** Framework-agnostic factory so the store can be exercised under
 *  plain node (see tests/intro-state.test.ts). */
export function createIntroStateStore(opts: CreateOpts = {}) {
  const persist = opts.persist ?? true;
  const storage: StorageAdapter | null = persist
    ? opts.storage ?? (typeof localStorage !== "undefined" ? localStorage : null)
    : null;

  return create<IntroStateStore>((set, get) => ({
    introduced: loadInitial(storage),
    hasIntroduced: (path) => get().introduced.has(path),
    markIntroduced: (path) =>
      set((state) => {
        if (state.introduced.has(path)) return state;
        const next = new Set(state.introduced);
        next.add(path);
        persistSet(storage, next);
        return { introduced: next };
      }),
    reset: () =>
      set(() => {
        const empty = new Set<string>();
        if (storage) storage.removeItem(STORAGE_KEY);
        return { introduced: empty };
      }),
  }));
}

export const useIntroState = createIntroStateStore();
