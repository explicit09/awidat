# Transition Render Proof Pack

## Goal

Make transition validation catch the practical failure mode where an agent
claims a transition worked because the render manifest contains a transition,
but the visible proof does not actually show one.

## Plan

1. Add a focused CLI integration test that builds vertical proof projects from
   two high-contrast constant-rate clips.
2. Apply output-format and transition EDL in the same sequence an agent would
   use.
3. Render the timeline and validate manifest evidence.
4. Probe the encoded video dimensions with `ffprobe`.
5. Extract grayscale frames before, during, and after the transition with
   `ffmpeg`.
6. Compare luma frames so the mid-transition frame must differ from both clips.
7. Parameterize the proof over slide, dissolve, flash, wipe, and zoom families.
8. Keep each proof short and one-transition-only so test renders do not become
   a preview-performance bottleneck.
9. Update `transition-director` guidance so future agents use distinct proof
   clips, correct aspect ratio, and frame inspection before calling a transition
   proof successful.

## Verification

- `cargo fmt --all -- --check`
- `CARGO_INCREMENTAL=0 cargo test -p awidat-cli --test transition_render_proof`
- Existing transition planner tests remain in scope after implementation.
