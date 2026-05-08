---
name: meeting-highlights
description: Create concise meeting highlight cuts and reports focused on decisions, action items, blockers, and executive-ready context.
version: 0.1.0
tier: editorial
tools_allowlist:
  - view_episode
  - read_index
  - find_dead_air
  - inspect_moment
  - view_timeline
  - apply_edl
  - vedit_diff
  - start_render
  - poll_render
  - update_plan
  - bash
---

# Meeting highlights

Use this for meeting recaps, executive summaries, decision reels,
action-item summaries, and async updates.

## Workflow

### 1. Choose the audience

Default to `team` if the user does not specify. Executive summaries
prioritize decisions and risks. Team summaries keep more context and
action-owner detail. Async summaries preserve reasoning.

### 2. Classify transcript

```bash
python3 <skill-root>/scripts/classify_meeting.py \
  --transcript index/whisper/raw/<asset>.json \
  --audience team
```

### 3. Build the highlight cut

Keep segments tagged `decision`, `action_item`, and `risk`. Remove
setup friction, "can you hear me", screen-share fumbling, and small
talk unless it gives essential context. Use `apply_edl` to turn the
selected ranges into timeline edits, then call `view_timeline` to
verify clip order, source ranges, and total duration before rendering.
Call `vedit_diff` before final report so the decision/action-item cut
can be audited against the selected ranges.

### 4. Verify

The output should answer: what was decided, why it matters, who owns
next steps, and what remains blocked.

## Rules

- Do not create a meeting highlight from energy alone.
- Do not remove reasoning for contentious decisions.
- Do not skip the `vedit_diff` checkpoint before final report.
- Final report must include decisions, action items, risks, and open
  questions even when no render is requested.
