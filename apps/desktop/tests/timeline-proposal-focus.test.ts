// Tests for useTimelineProposalFocus — the focus-cycling store used by
// the timeline ghost-clip review surface (Wave 3 C2).
//
// We exercise the store's cycling, focus, focusFirst, and cheatsheet
// behaviours directly; the DOM overlay's keyboard handlers feed these
// methods, so locking the contract here protects every keybinding.

import { strict as assert } from "node:assert";
import { useTimelineProposalFocus } from "../src/timeline/timelineProposalFocus.ts";

function reset(): void {
  useTimelineProposalFocus.setState({
    focusedId: null,
    cheatsheetOpen: false,
  });
}

// initial state is empty
{
  reset();
  const state = useTimelineProposalFocus.getState();
  assert.equal(state.focusedId, null);
  assert.equal(state.cheatsheetOpen, false);
}

// focus() sets the id
{
  reset();
  useTimelineProposalFocus.getState().focus("p1");
  assert.equal(useTimelineProposalFocus.getState().focusedId, "p1");
  useTimelineProposalFocus.getState().focus(null);
  assert.equal(useTimelineProposalFocus.getState().focusedId, null);
}

// focusFirst seeds when empty, is a no-op when the current id is in
// the list, and re-seeds when the current id has been dropped
{
  reset();
  useTimelineProposalFocus.getState().focusFirst(["a", "b", "c"]);
  assert.equal(useTimelineProposalFocus.getState().focusedId, "a");
  // Already focused on a member — no change.
  useTimelineProposalFocus.getState().focusFirst(["a", "b", "c"]);
  assert.equal(useTimelineProposalFocus.getState().focusedId, "a");
  // Stale focus — re-seed to first.
  useTimelineProposalFocus.getState().focusFirst(["b", "c"]);
  assert.equal(useTimelineProposalFocus.getState().focusedId, "b");
}

// cycle forward / backward walks the list and wraps at both ends
{
  reset();
  const ids = ["a", "b", "c"];
  useTimelineProposalFocus.getState().focus("a");
  useTimelineProposalFocus.getState().cycle(ids, "forward");
  assert.equal(useTimelineProposalFocus.getState().focusedId, "b");
  useTimelineProposalFocus.getState().cycle(ids, "forward");
  assert.equal(useTimelineProposalFocus.getState().focusedId, "c");
  // Wrap forward.
  useTimelineProposalFocus.getState().cycle(ids, "forward");
  assert.equal(useTimelineProposalFocus.getState().focusedId, "a");
  // Wrap backward.
  useTimelineProposalFocus.getState().cycle(ids, "backward");
  assert.equal(useTimelineProposalFocus.getState().focusedId, "c");
  useTimelineProposalFocus.getState().cycle(ids, "backward");
  assert.equal(useTimelineProposalFocus.getState().focusedId, "b");
}

// cycle starting from "not in list" lands on first (forward) / last (backward)
{
  reset();
  useTimelineProposalFocus.getState().focus("stale");
  useTimelineProposalFocus.getState().cycle(["a", "b", "c"], "forward");
  assert.equal(useTimelineProposalFocus.getState().focusedId, "a");
  useTimelineProposalFocus.getState().focus("stale");
  useTimelineProposalFocus.getState().cycle(["a", "b", "c"], "backward");
  assert.equal(useTimelineProposalFocus.getState().focusedId, "c");
}

// cycle on empty list clears focus
{
  reset();
  useTimelineProposalFocus.getState().focus("a");
  useTimelineProposalFocus.getState().cycle([], "forward");
  assert.equal(useTimelineProposalFocus.getState().focusedId, null);
}

// cycle from null focus on a non-empty list lands on first/last
{
  reset();
  useTimelineProposalFocus.getState().cycle(["a", "b", "c"], "forward");
  assert.equal(useTimelineProposalFocus.getState().focusedId, "a");
  reset();
  useTimelineProposalFocus.getState().cycle(["a", "b", "c"], "backward");
  assert.equal(useTimelineProposalFocus.getState().focusedId, "c");
}

// clear() drops focus
{
  reset();
  useTimelineProposalFocus.getState().focus("a");
  useTimelineProposalFocus.getState().clear();
  assert.equal(useTimelineProposalFocus.getState().focusedId, null);
}

// toggleCheatsheet flips, closeCheatsheet only closes
{
  reset();
  useTimelineProposalFocus.getState().toggleCheatsheet();
  assert.equal(useTimelineProposalFocus.getState().cheatsheetOpen, true);
  useTimelineProposalFocus.getState().closeCheatsheet();
  assert.equal(useTimelineProposalFocus.getState().cheatsheetOpen, false);
  // Closing again is a no-op (doesn't toggle).
  useTimelineProposalFocus.getState().closeCheatsheet();
  assert.equal(useTimelineProposalFocus.getState().cheatsheetOpen, false);
  useTimelineProposalFocus.getState().toggleCheatsheet();
  assert.equal(useTimelineProposalFocus.getState().cheatsheetOpen, true);
  useTimelineProposalFocus.getState().toggleCheatsheet();
  assert.equal(useTimelineProposalFocus.getState().cheatsheetOpen, false);
}

console.log("timeline-proposal-focus: OK");
