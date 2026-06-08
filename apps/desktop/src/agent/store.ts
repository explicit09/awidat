// Zustand store for the agent's emitted Items + turn lifecycle.
//
// Items arrive as a stream (Started → Delta → Completed). The store
// upserts by `id` so progressive updates re-render the same slot
// instead of appending duplicates. Every protocol Item variant carries
// an `id`, so the same upsert path covers Text / ToolCall / Plan /
// AwaitingUserInput / ApprovalRequest / Error.

import { create } from "zustand";
import type { Item } from "../protocol";

type AgentState = {
  /** Items in arrival order. Append-only at the back; in-place upsert by id. */
  items: Item[];
  /** True from invoke('start_turn') until montage://turn-end fires. */
  running: boolean;
  /**
   * Turn id of the currently in-flight turn. Cleared when the matching
   * turn-end arrives. Used to drop stale turn-end events from a turn
   * the user already cancelled and superseded.
   */
  activeTurnId: string | null;
  /** Last error from a turn-end event (if any). Cleared on next turn. */
  turnError: string | null;
  /** Upsert an Item: replace if `id` already present, append otherwise. */
  upsert: (item: Item) => void;
  /** Replace the visible stream with persisted history for a session. */
  replace: (items: Item[]) => void;
  /** Clear all items + error (used between turns or on project change). */
  clear: () => void;
  /** Set the running flag explicitly. */
  setRunning: (running: boolean) => void;
  /** Set/clear the active turn id. */
  setActiveTurnId: (id: string | null) => void;
  /** Record a turn-end error. Pass null to clear. */
  setTurnError: (err: string | null) => void;
};

function itemId(item: Item): string {
  // `id` field is `Id` in TS (= plain string due to the Rust
  // `#[serde(transparent)]` wrapper). Every Item variant carries one.
  return item.id;
}

function isOptimisticUserInput(
  item: Item,
): item is Extract<Item, { kind: "user_input" }> {
  return item.kind === "user_input" && item.id.startsWith("optimistic-user-");
}

export const useAgentStore = create<AgentState>((set) => ({
  items: [],
  running: false,
  activeTurnId: null,
  turnError: null,
  upsert: (item) =>
    set((state) => {
      const id = itemId(item);
      const idx = state.items.findIndex((i) => itemId(i) === id);
      if (idx === -1) {
        if (item.kind === "user_input") {
          const optimisticIdx = state.items.findIndex(
            (existing) => isOptimisticUserInput(existing) && existing.text === item.text,
          );
          if (optimisticIdx !== -1) {
            const next = state.items.slice();
            next[optimisticIdx] = item;
            return { items: next };
          }
        }
        return { items: [...state.items, item] };
      }
      const next = state.items.slice();
      next[idx] = item;
      return { items: next };
    }),
  replace: (items) =>
    set({ items, turnError: null, running: false, activeTurnId: null }),
  clear: () => set({ items: [], turnError: null }),
  setRunning: (running) => set({ running }),
  setActiveTurnId: (id) => set({ activeTurnId: id }),
  setTurnError: (err) => set({ turnError: err }),
}));
