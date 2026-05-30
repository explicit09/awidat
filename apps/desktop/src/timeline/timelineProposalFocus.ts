// Focus state for inline ghost-clip review on the timeline (Wave 3 C2).
//
// The Brief surface owns the *list* review motion — the timeline owns the
// *inline* motion. When a reviewer wants to see a cut proposal in
// context, they tab through the ghost clips here; this store tracks
// which one currently holds the focus ring + which one ⌘↵/Esc apply to.
//
// Why a separate store from `useTimelineSelectionStore`?
//   - Selection drives the Inspector (a different concept — committed
//     clips, not pending proposals).
//   - Focus has to coexist with selection (the user might inspect a
//     committed clip *and* have a ghost focused), so collapsing them
//     into one slot would force one to clobber the other.
//
// Lifecycle:
//   - `focusFirst(ids)` seeds the focus when ⌘↵ is the next likely
//     action (e.g. cursor enters the timeline canvas with pending cuts).
//   - `cycle(ids, direction)` walks Tab / Shift+Tab through the list.
//     Ids are passed in so the store doesn't need its own subscription
//     to `usePendingProposals`.
//   - `clear()` drops focus (proposal accepted/rejected, ghost gone).

import { create } from "zustand";

interface FocusState {
  /** Currently focused proposal id, or null when nothing is focused. */
  focusedId: string | null;
  /** Whether the ? cheatsheet overlay is visible. */
  cheatsheetOpen: boolean;
  /** Drop focus. Called when the focused proposal is accepted/rejected
   *  or when no proposals remain. */
  clear: () => void;
  /** Set focus to a specific id. */
  focus: (id: string | null) => void;
  /** Pick the first id from the list if nothing is currently focused;
   *  no-op when focus is already on something inside `ids`. */
  focusFirst: (ids: ReadonlyArray<string>) => void;
  /** Move focus forward (Tab) or backward (Shift+Tab) through `ids`.
   *  Wraps at both ends. If the current focus isn't in the list, starts
   *  at the first (forward) or last (backward) entry. */
  cycle: (ids: ReadonlyArray<string>, direction: "forward" | "backward") => void;
  /** Toggle the ? cheatsheet overlay. */
  toggleCheatsheet: () => void;
  /** Close the cheatsheet (Esc inside it). */
  closeCheatsheet: () => void;
}

export const useTimelineProposalFocus = create<FocusState>((set, get) => ({
  focusedId: null,
  cheatsheetOpen: false,
  clear() {
    if (get().focusedId === null) return;
    set({ focusedId: null });
  },
  focus(id) {
    if (get().focusedId === id) return;
    set({ focusedId: id });
  },
  focusFirst(ids) {
    const current = get().focusedId;
    if (current && ids.includes(current)) return;
    set({ focusedId: ids[0] ?? null });
  },
  cycle(ids, direction) {
    if (ids.length === 0) {
      if (get().focusedId !== null) set({ focusedId: null });
      return;
    }
    const current = get().focusedId;
    const idx = current ? ids.indexOf(current) : -1;
    let nextIdx: number;
    if (idx === -1) {
      nextIdx = direction === "forward" ? 0 : ids.length - 1;
    } else {
      nextIdx =
        direction === "forward"
          ? (idx + 1) % ids.length
          : (idx - 1 + ids.length) % ids.length;
    }
    set({ focusedId: ids[nextIdx] ?? null });
  },
  toggleCheatsheet() {
    set({ cheatsheetOpen: !get().cheatsheetOpen });
  },
  closeCheatsheet() {
    if (!get().cheatsheetOpen) return;
    set({ cheatsheetOpen: false });
  },
}));
