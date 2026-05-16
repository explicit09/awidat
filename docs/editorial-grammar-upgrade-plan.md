# Awidat Editorial Grammar Upgrade Plan

This plan turns the lessons from the four editing videos into an
implementation roadmap for Awidat. The core shift is from "can the
agent perform this timeline operation?" to "does the agent understand
why this cut, split edit, cutaway, or transition belongs here?"

## Source Lessons

The four videos converge on the same editing grammar:

- Hard cuts are the default. Visible transitions need a job.
- The important decision is usually not which effect to use, but what
  to show, when to cut, and what to cut to.
- Core cut types need to be intentional: cut on action, cutaway,
  insert, eyeline match, shot/reverse shot, match cut, cross-cutting,
  smash cut, jump cut, J-cut, and L-cut.
- Organic transitions work best when they follow camera, subject, or
  screen motion: whip pans, pass-by/mask moves, invisible cuts, and
  match dissolves.
- J-cuts and L-cuts are not polish; they are basic dialogue and scene
  grammar.
- Cutaways can be literal continuity covers or intellectual/thematic
  montage. Those modes should not be conflated.

## Current Awidat Strengths

Awidat already has several pieces of the right system:

- `SemanticTransitionSpec` stores transition id, family, intent, energy,
  direction, params, and composition metadata in
  `crates/proto/src/transitions.rs`.
- `InsertTransition` in `crates/core/src/edl/op.rs` can carry semantic
  transition metadata and explicit OTIO offsets.
- The renderer in `crates/render/src/timeline.rs` supports graph-native
  transitions, handles, video `xfade`, audio `acrossfade`, explicit
  audio tracks, clip fades, ducking, and audio effects.
- `assess_continuity` in `crates/core/src/continuity.rs` already checks
  risks such as mid-word, breath timing, speaker turns, rhythm, and
  motion.
- B-roll and cutaway workflows exist through `find_broll_opportunities`,
  `broll_candidates`, stock b-roll tools, and bundled skills.
- The eval harness in `crates/eval` already supports deterministic
  product and golden fixtures.
- `docs/transition-decision-layer.md` already defines a focused
  decision layer for transition taste.

## Gaps

### 1. Cuts Are Not First-Class Editorial Objects

Transitions have semantic metadata; hard cuts do not. A cut can be made
or checked, but Awidat cannot persist "this boundary is a cut on
action", "this is an eyeline match", or "this is a smash cut for
contrast."

Impact:

- The agent cannot explain or preserve cut intent.
- The UI cannot inspect cut type.
- Evals cannot assert that the right editorial grammar was chosen.
- User preference learning only sees tool usage, not accepted or
  rejected cut decisions.

### 2. Dirty-Cut Repair Is Too Transition-Centric

The always-on prompt currently pushes dirty cuts toward a centered
cross dissolve or b-roll cover. That is useful as a safety rule, but it
is not the best editorial hierarchy.

Better hierarchy:

1. Move the cut point to a cleaner sentence/action boundary.
2. Use cut on action, eyeline match, insert, or cutaway when the footage
   supports it.
3. Use J-cut or L-cut when audio continuity is the real problem.
4. Use b-roll when visual continuity needs a cover.
5. Use a visible transition only when it has a named purpose.

### 3. J-Cuts and L-Cuts Are Missing as Operations

The renderer has enough audio-track machinery to support split edits,
but the EDL layer does not expose an operation such as `Set Audio Lead`,
`Set Audio Trail`, `Insert Split Edit`, `JCut`, or `LCut`.

Impact:

- Agents cannot ask for the next speaker's audio before picture.
- Agents cannot hold previous audio under the next image as a named
  operation.
- The desktop timeline cannot display audio-picture offsets as an
  intentional edit.

### 4. Continuity Checking Is Defensive, Not Expressive

`assess_continuity` answers whether a proposed edit is clean, risky, or
dirty. It does not yet answer which better edit Awidat should make.

Needed upgrade:

- Keep continuity verdicts.
- Add a richer `assess_edit_quality` or `plan_cut` layer that recommends
  a cut type, split edit, cutaway, hard cut, or transition.

### 5. Visual Analysis Is Not Yet Edit-Aware

The shot sidecars describe broad shot type and motion. The system needs
more signals to support the grammar from the videos:

- action peaks for cut-on-action
- screen direction
- subject center and eye trace
- face/gaze position
- visual similarity for match cuts
- pass-by/mask opportunities
- camera whip or fast pan candidates

### 6. Transition Library Is Missing Organic Motion Grammar

The current registry covers useful basics, but the video lessons point
to additional transition families:

- `awidat.whip_pan_left`
- `awidat.whip_pan_right`
- `awidat.pass_by_left`
- `awidat.pass_by_right`
- `awidat.iris_open`
- `awidat.iris_close`
- `awidat.match_dissolve`
- `awidat.invisible_cut`

These should not be added as decorative choices only. Each needs
`best_for`, `avoid_for`, duration defaults, direction, audio policy, and
fallback lowering.

### 7. B-Roll Is Literal, But Montage Needs a Separate Mode

The bundled b-roll skill correctly avoids abstract metaphorical cutaways
for normal cover work. Awidat also needs a separate thematic montage
mode for intellectual or associative editing.

The safe split:

- `b-roll-suggester`: literal continuity and explanation cover.
- `thematic-montage-director`: deliberate symbolic or associative
  montage, only when user/project style calls for it.

### 8. Desktop UI Exposes Mechanics More Than Intent

The transition properties pane exposes kind, duration, and offsets. It
does not expose:

- transition intent
- best-use/avoid-use guidance
- repeated-transition warnings
- "hard cut recommended"
- cut type at a boundary
- split-edit lead/trail visualization

The frontend protocol currently flattens transition items with timing
and effect name, but not the richer semantic fields needed by the UI.

### 9. Preview/Render Parity Needs Split-Edit and Audio Policy Coverage

Desktop preview has a pragmatic video-segment model and transition CSS
approximations. It does not yet fully model:

- explicit audio track mix preview
- J-cut/L-cut lead and trail behavior
- transition audio policy display
- semantic transition metadata

## Target Architecture

### Semantic Cut Model

Add a first-class cut schema in proto, likely as a new module:

```rust
pub struct SemanticCutSpec {
    pub cut_type: CutType,
    pub intent: String,
    pub energy: Option<f32>,
    pub visual_anchor: Option<VisualAnchor>,
    pub audio_relation: AudioRelation,
    pub confidence: Option<f32>,
    pub reason: Option<String>,
}
```

```rust
pub enum CutType {
    HardCut,
    CutOnAction,
    Cutaway,
    Insert,
    EyelineMatch,
    ShotReverseShot,
    MatchCut,
    SmashCut,
    JumpCut,
    CrossCut,
    JCut,
    LCut,
}
```

```rust
pub enum AudioRelation {
    Sync,
    AudioLeads,
    AudioTrails,
    Overlap,
    AudioCut,
}
```

Store cut metadata at the timeline boundary level, not as fake
transition nodes. A hard cut should remain a hard cut. The most robust
storage shape is timeline-level Awidat metadata keyed by adjacent clip
UUIDs:

```json
{
  "cut_boundaries": {
    "clip-a::clip-b": {
      "cut_type": "cut_on_action",
      "intent": "hide_action_continuity",
      "audio_relation": "sync",
      "reason": "The outgoing hand motion resolves into the incoming shot."
    }
  }
}
```

### EDL Operations

Add EDL operations for intent and split edits:

```text
*** Set Cut Intent
@@ between: clip_uuid=clip-a and clip_uuid=clip-b
cut_type: cut_on_action
intent: hide_action_continuity
reason: outgoing hand motion resolves into incoming frame
```

```text
*** Set Audio Lead
@@ anchor: clip_uuid=clip-b
lead_s: 0.45
reason: let the next speaker enter before picture
```

```text
*** Set Audio Trail
@@ anchor: clip_uuid=clip-a
trail_s: 0.60
reason: hold previous answer under reaction shot
```

Implementation can start by materializing leads/trails into explicit
audio tracks using existing render support. The public operation should
remain semantic even if the apply layer lowers it into track edits,
gaps, fades, and link-group metadata.

### Edit Quality Tooling

Introduce a read-only tool:

```text
assess_edit_quality
```

Input:

- candidate cut boundary or source timestamp
- optional objective, such as `tighten`, `speaker_handoff`,
  `scene_transition`, `beat_hit`, or `hide_jump`
- optional allowed modes, such as `hard_cut`, `split_edit`, `broll`,
  `transition`

Output:

- verdict: `clean`, `risky`, `dirty`, or `abstain`
- recommended edit: hard cut, recut, cutaway, J-cut, L-cut, transition
- cut type and intent
- continuity risks
- missing signals
- short EDL fragment when safe
- reason suitable for UI display and proposal review

This can initially wrap `assess_continuity`, transcript windows,
topic/beat information, b-roll opportunities, and transition handle
checks. Later it can incorporate richer composition sidecars.

### Visual Signal Upgrade

Extend the shot/composition index layer with edit-aware outputs:

```json
{
  "time_s": 42.30,
  "subject_center": [0.54, 0.42],
  "face_center": [0.51, 0.36],
  "motion_vector": [0.82, 0.05],
  "dominant_direction": "right",
  "action_score": 0.77,
  "whip_pan_score": 0.63,
  "occlusion_score": 0.71,
  "match_candidates": [
    {"time_s": 88.10, "similarity": 0.81, "basis": "shape_and_motion"}
  ]
}
```

Use cases:

- cut on action
- eye trace preservation
- screen-direction-aware wipes/slides/pushes
- match cuts
- pass-by or invisible-cut opportunities
- warnings when a cut jumps across the frame in an ugly way

### Transition Registry Upgrade

Add organic and editorial transitions only with usage metadata:

| Id | Purpose | Avoid When |
| --- | --- | --- |
| `awidat.whip_pan_left/right` | camera-motion bridge, energy, location jump | static dialogue, serious tone |
| `awidat.pass_by_left/right` | object/person wipes frame, invisible move | no occlusion/mask signal |
| `awidat.iris_open/close` | stylized reveal, vintage/comic grammar | documentary realism |
| `awidat.match_dissolve` | visual echo, time/memory bridge | unrelated images |
| `awidat.invisible_cut` | hide a cut on occlusion/dark frame | visible mismatch |

Phase-one lowering can be approximate through existing primitives:
blur, push, wipe, opacity, flash, and zoom. The important contract is
semantic id and intent first, backend fidelity later.

### Desktop UI Upgrade

Protocol and UI changes:

- Add cut boundary metadata to timeline snapshots.
- Expose transition semantic fields in `TimelineItem::Transition`.
- Add boundary badges for cut type.
- Add a cut/transition inspector with intent, reason, confidence, and
  warnings.
- Show split edit lead/trail on linked audio/video clips.
- Warn on transition density and repeated flashy transitions.
- Offer safer alternatives: hard cut, J-cut, L-cut, cutaway, or shorter
  transition.

The source of truth should be `crates/desktop-protocol/src/lib.rs`, with
generated TypeScript refreshed through the existing generation path.

### Skills Upgrade

Add or revise bundled skills:

- `cut-director`: pick cut points and cut types.
- `split-edit-director`: plan J-cuts and L-cuts.
- `thematic-montage-director`: associative/thematic montage.
- `transition-director`: keep stricter transition intent rules and use
  the broader edit quality tool.
- `b-roll-suggester`: remain literal and continuity-focused.
- `podcast-episode-producer`, `auto-cutter`, `interview-tightener`, and
  `beat-sync-editor`: route risky cuts through edit-quality planning
  rather than defaulting to dissolve repair.

### Learning Upgrade

The current lesson system learns from tool accept/deny patterns. Add
editorial dimensions:

- accepted/rejected cut type
- accepted/rejected transition family
- preferred split-edit lead/trail ranges
- transition density tolerance
- literal b-roll versus thematic montage preference
- project-format defaults

Learning should tune deterministic scores and defaults. It should not
remove the requirement that every visible transition has an intent.

## Phased Delivery Plan

### Phase 0: Design Lock

Deliverables:

- Finalize `SemanticCutSpec` schema.
- Decide exact metadata storage under `metadata.awidat`.
- Decide whether split-edit ops lower to explicit audio tracks or to
  clip-level effects first.
- Add this plan to the engineering docs and link it from transition docs
  once implementation starts.

Exit criteria:

- Schema and storage decision are documented.
- No code behavior changes yet.

### Phase 1: Semantic Cut Metadata

Deliverables:

- Add proto types for semantic cuts.
- Add timeline metadata storage for cut boundaries.
- Add `Set Cut Intent` EDL op.
- Parse/apply/round-trip cut intent.
- Add snapshot protocol fields for read-only UI display.
- Add tests for parse, apply, metadata round trip, and generated TS.

Exit criteria:

- Awidat can preserve and display "this boundary is a cutaway/cut on
  action/match cut" without adding a transition.

### Phase 2: First-Class Split Edits

Deliverables:

- Add `Set Audio Lead` and `Set Audio Trail` or one `Set Split Edit`
  operation.
- Apply split edits through explicit audio tracks and link groups.
- Add renderer tests proving lead/trail audio appears in exported
  filter plans.
- Add desktop timeline display for lead/trail state.
- Add preview support or a visible "render-faithful, preview-limited"
  warning until parity lands.

Exit criteria:

- The agent can create J-cuts and L-cuts as named operations.
- Exports honor the split edit.

### Phase 3: Edit Quality Assessor

Deliverables:

- Add `assess_edit_quality` read-only tool.
- Use existing continuity, transcript, topic, beat, b-roll, and handle
  signals.
- Return ranked recommendations and safe EDL fragments.
- Update system prompt and relevant skills to use it.
- Remove cross-dissolve as the default dirty-cut repair.

Exit criteria:

- Dirty cuts route through recut, split edit, b-roll, or transition
  recommendations with reasons.

### Phase 4: Visual Composition Signals

Deliverables:

- Extend shot/composition sidecars or add a focused composition indexer.
- Emit subject center, motion vector, screen direction, action score,
  occlusion score, and match-cut candidates.
- Feed those signals into `assess_edit_quality` and transition planning.

Exit criteria:

- Awidat can recommend cut-on-action, match cut, screen-direction
  transition, and pass-by/invisible-cut candidates from indexed data.

### Phase 5: Transition Registry Expansion

Deliverables:

- Add organic transition ids.
- Add metadata for best use, avoid use, duration ranges, audio policy,
  and fallback lowering.
- Extend render tests.
- Extend preview approximations where feasible.

Exit criteria:

- The registry covers the transition grammar taught in the videos
  without encouraging decorative overuse.

### Phase 6: Desktop Editorial Inspector

Deliverables:

- Cut boundary inspector.
- Transition intent inspector.
- Split-edit visualization.
- Density/repetition warnings.
- Suggested alternatives surfaced in properties or notes.

Exit criteria:

- A user can see not only what edit exists, but why Awidat thinks it is
  there.

### Phase 7: Editorial Evals

Deliverables:

- Golden fixtures for each lesson:
  - hard cut default
  - cut on action
  - cutaway/insert
  - J-cut
  - L-cut
  - match cut
  - smash cut
  - transition overuse warning
  - literal b-roll versus thematic montage
- Product scenarios that assert recommendation reasons, not only EDL
  shape.

Exit criteria:

- Regressions in editorial judgment fail before they reach users.

## Recommended First Sprint

Build the foundation before adding flashy effects:

1. Add `SemanticCutSpec` and boundary metadata.
2. Add `Set Cut Intent` EDL parse/apply/round-trip.
3. Add split-edit operation design and a minimal `Set Audio Lead` /
   `Set Audio Trail` implementation.
4. Update dirty-cut guidance so dissolve is no longer the first repair.
5. Expose cut intent and transition intent in the desktop protocol.
6. Add initial eval fixtures for hard cut default, cut on action, J-cut,
   L-cut, and decorative-transition rejection.

## Implementation Status

As of May 16, 2026, the first-sprint foundation is partially
implemented in the worktree:

- Semantic cut metadata exists through `SemanticCutSpec`, cut-boundary
  metadata, `Set Cut Intent`, desktop protocol fields, and inspector
  display. The EDL apply path now also prunes stale cut-boundary
  metadata after timeline mutations, so delete/move edits do not leave
  semantic intent attached to boundaries that no longer exist. Split
  edits preserve boundary intent more precisely: incoming intent stays
  with the left piece, and outgoing intent follows the new right piece.
  Split-edit offsets are partitioned the same way, so an incoming
  audio lead stays on the left split piece while an outgoing audio
  trail moves to the right split piece instead of duplicating across
  both. New split-edit offsets also record the neighboring clip id they
  were authored against and are pruned after deletes or moves if that
  boundary changes.
- Split edits exist through `Set Audio Lead` and `Set Audio Trail`,
  explicit audio-track lowering, export support, and preview limitation
  warnings where desktop preview cannot yet match render behavior.
- `assess_edit_quality` exists as the recommendation layer between
  raw continuity checks and edit operations. It routes dirty/risky edits
  toward recut, J-cut, L-cut, b-roll, or a named transition with intent.
- The transition registry includes the first organic motion grammar:
  match dissolve, motion blur, whip pans, pass-by moves, iris, and
  invisible-cut entries with family, duration, audio policy, and
  composition metadata.
- `transition_context` now gives agents a deterministic read-only
  transition-decision packet for one adjacent boundary: neighboring clip
  metadata, timeline/source ranges, handle availability, transcript
  context, continuity verdict, suggested frame timestamps, and
  missing-signal names. It is registered in CLI, TUI, and desktop
  sessions and deliberately stops short of choosing a transition.
- `plan_transition` now consumes that packet and returns a read-only
  hard-cut or visible-transition recommendation with reason, alternates,
  safe duration, and an EDL fragment. Clean/no-job contexts stay hard
  cuts; dirty/risky or named-job contexts only produce visible
  transitions when the boundary has enough safe handles.
- Shot sidecars now expose initial edit-aware signals such as dominant
  direction, face-derived subject/face centers, gaze score, at-camera
  ratio, eye-trace bucket, action score, whip-pan score, and occlusion
  score.
  `assess_edit_quality` also consumes `match_candidates` and
  `match_cut_score` when present, recommending semantic `match_cut`
  repairs before visible motion-cover transitions.
- `assess_edit_quality` now preserves indexed `subject_center` and
  `face_center` fields in `visual_context` when shot sidecars provide
  them, giving agents explicit framing positions for eye-trace and
  screen-continuity reasoning.
- `shot-mcp` now aggregates gaze sidecars into shot-level
  `gaze_score`, `at_camera_ratio`, `eye_trace`, and `gaze_samples`
  fields, and `assess_edit_quality` surfaces those fields for
  eye-contact and eye-trace continuity decisions.
- `shot-mcp` now also consumes optional CLIP sidecars to generate
  shot-level `match_candidates` and `match_cut_score`, excluding the
  current shot window so match-cut recommendations can come from
  indexed visual similarity instead of hand-authored fixtures.
- CLIP-derived match candidates now rank across every available asset
  sidecar, carry `asset_id` in the candidate payload, and
  `assess_edit_quality` includes that asset in match-cut reasons when
  recommending cross-asset visual matches.
- Shot sidecars now derive coarse composition semantics from centers and
  gaze: subject zone, face zone, headroom, and look-space quality.
  CLIP match candidates also carry confidence derived from similarity
  and nearest-neighbor margin, and `assess_edit_quality` preserves those
  fields in `visual_context`.
- CLIP-derived match candidates now apply temporal de-duplication before
  limiting results, so adjacent frames from one visual moment do not
  crowd out distinct match-cut opportunities elsewhere in the footage.
- `assess_edit_quality` now requires adequate match-candidate
  confidence before promoting a visual match to a semantic match cut,
  falling back to cut-on-action or motion-cover repairs for ambiguous
  high-similarity candidates.
- `assess_edit_quality` now preserves model-enriched composition
  sidecar labels in `visual_context`, including composition source,
  confidence, subject role, depth layer, and framing. The live visual
  coverage gate counts those model-only labels as visual metadata.
- `shot-mcp` now reads optional `index/composition/<asset>.json`
  sidecars and merges overlapping model composition regions into each
  emitted shot, giving the real model-backed composition indexer a
  stable upstream handoff into the existing shot sidecar.
- `composition-mcp` now exists as a lightweight workspace indexer that
  emits `index/composition/<asset>.json` regions from scenedetect plus
  optional face/gaze sidecars. It gives the pipeline a stable
  composition schema and producer path while the heavier visual model is
  still being tuned.
- `composition-mcp` now also reads optional
  `index/composition-model/<asset>.json` sidecars. Overlapping
  `composition_source: model:*` regions override heuristic labels while
  preserving the heuristic values under `heuristic_*` audit fields, so a
  real classifier can feed the same `index/composition` handoff without
  changing `shot-mcp` or `assess_edit_quality`.
- Python safe smoke now validates the `composition-model` sidecar
  contract: non-empty regions, valid time ranges, `model:*` source,
  bounded confidence, and controlled subject/depth/framing labels.
- A checked-in `python/fixtures/composition-model/sample.json` sidecar
  gives model-indexer authors an executable fixture for the contract
  that safe smoke validates on every run.
- Python safe smoke can now also validate a mounted real project
  `index/composition-model/**/*.json` tree via
  `AWIDAT_COMPOSITION_MODEL_PROJECT` and
  `AWIDAT_COMPOSITION_MODEL_MIN_REGIONS`, giving the model-backed
  composition rollout a schema and minimum-coverage gate before live
  eval thresholds are tuned.
- Bundled skills have started to separate literal b-roll from thematic
  montage and to route risky cuts through edit-quality assessment.
- Dedicated `cut-director` and `split-edit-director` skills expose
  first-class hard-cut, semantic-cut, J-cut, and L-cut workflows.
- Lesson extraction now learns from stable editorial tags, not only
  approval-summary prose. Captured decisions can carry cut type,
  transition family and intent, split-edit range, and b-roll mode tags.
- `assess_edit_quality` now reads learned-style output and uses denied
  transition-family or transition-id tags to suppress visible transition
  recommendations in favor of cut-on-action or b-roll cover.
- Learned split-edit range tags now tune `Set Audio Lead` and
  `Set Audio Trail` guidance so accepted J-cut/L-cut timing buckets
  become the next suggested defaults.
- `apply_edl` records transition-density learning tags for transition
  envelopes, and `assess_edit_quality` uses learned high-density
  acceptance to raise the visible-transition suppression threshold.
- Learned b-roll mode tags now tune `assess_edit_quality` b-roll
  guidance. Literal continuity cover remains the default, while a
  strong thematic-montage preference surfaces as an explicit opt-in
  `broll_mode` recommendation.
- `apply_edl` records project-format learning tags from
  `Set Output Format`, including aspect ratio, platform, and safe-area
  profile.
- Learned project-format tags now render as actionable
  `Set Output Format` guidance in the learned-style prompt, so future
  sessions see preferred aspect ratio, platform, and safe-area defaults.
- Learned project-format defaults are now applied directly to fresh
  CLI and desktop projects when no explicit output format exists, and
  timeline/package export paths stamp those defaults before planning
  delivery artifacts.
- The golden eval tier now asserts semantic cut-boundary metadata and
  covers hard cut, cut on action, cutaway, insert, eyeline match,
  shot/reverse shot, match cut, smash cut, cross-cut, J-cut, L-cut,
  thematic montage, podcast dialogue, short-form hook, tutorial
  insert-repair, documentary no-transition, multi-step dialogue cleanup,
  rough short-form assembly with delivery constraints, invalid
  split-edit/cut-confidence rejection, and transition-overuse rejection
  fixtures.
- Golden fixtures can now assert `Set Output Format` delivery metadata
  and `Set Package Metadata` platform/title/description/tag metadata, so
  short-form and platform packaging constraints are tested as timeline
  state instead of only applied-op log text.
- Golden fixtures now include a second longer rough-assembly case with a
  rejected J-cut recommendation and accepted L-cut follow-up history.
  The harness checks that rejected proposal snippets are absent from the
  final EDL and accepted follow-up snippets are present.
- Desktop smoke coverage now exercises semantic cut-boundary inspector
  display, split-edit lead/trail offsets, preview limitation caveats,
  transition-density warnings, timeline-level cut/split badges, and
  generated assessor recommendations flowing into the proposal pipeline.
- The desktop editorial inspector now lets users update semantic cut
  type/intent through `Set Cut Intent` and emit safer hard-cut, J-cut,
  or L-cut alternatives through the same proposal pipeline.
- Completed `assess_edit_quality` tool-call cards now parse structured
  recommendations and can open the recommended cut intent or split edit
  as a standard desktop proposal against the nearest timeline boundary.
- Desktop smoke coverage now verifies the assessor proposal lifecycle
  through a rejected J-cut recommendation followed by a fresh L-cut
  follow-up proposal, so rejected recommendations no longer block
  user-adjusted follow-up flows in the proposal UI.
- The live eval tier now has mounted real-project assessor proposal
  fixture hooks. `AWIDAT_REAL_ASSESSOR_PROPOSAL_FIXTURE` can point to a
  JSON file or directory, and real projects can provide
  `.awidat/eval/assessor-proposal-flow.json` plus additional
  `.awidat/eval/assessor-proposals/*.json` fixtures. The eval applies
  every discovered final EDL to the mounted real timeline while
  asserting rejected snippets are absent and accepted follow-up snippets
  are present. Each assessor proposal fixture must include at least one
  rejected recommendation and one accepted follow-up.
  `proposal_history.edl_contains` is now status-aware: accepted entries
  must appear in the final EDL, while rejected entries must not, and
  each history entry must include at least one proof snippet. Optional
  `final_edl_must_contain` and `final_edl_must_not_contain` assertions
  must also be non-empty so they cannot pass vacuously.
  Real-corpus runs can set
  `AWIDAT_REAL_MIN_ASSESSOR_PROPOSAL_FIXTURES` to fail when the mounted
  corpus does not include enough tuned proposal-flow fixtures.
- A checked-in
  `crates/eval/fixtures/real/assessor-proposal-flow.sample.json` manifest
  gives real-corpus runners an executable example for that lifecycle
  fixture format, and eval unit coverage now mounts it at the default
  `.awidat/eval/assessor-proposal-flow.json` path to exercise live
  scenario discovery. A second checked-in directory-layout sample under
  `crates/eval/fixtures/real/assessor-proposals/` keeps the multi-fixture
  discovery path executable too.
- The live eval tier now also has a mounted transition-planner fixture
  hook. `AWIDAT_REAL_TRANSITION_PLANNER_FIXTURE` can point to a JSON
  file or directory, and real projects can provide
  `.awidat/eval/transition-planner-flow.json` plus additional
  `.awidat/eval/transition-planners/*.json` fixtures. Each fixture runs
  `transition_context` on a real adjacent clip boundary, feeds the
  packet to `plan_transition`, checks the expected hard-cut or
  visible-transition recommendation, parses the returned EDL fragment,
  and applies it against the mounted timeline. Fixtures can also assert
  `edl_must_not_contain` snippets, so hard-cut-default cases prove the
  planner did not smuggle in a visible transition. The fixture validator
  now enforces those proofs: hard-cut cases must assert `Set Cut Intent`,
  `cut_type: hard_cut`, and forbidden `Insert Transition`, while visible
  transition cases must provide a `transition_id` and prove it appears in
  the EDL fragment. Checked-in sample manifests cover single-file
  discovery plus directory-layout hard-cut and motion-cover cases, and
  `AWIDAT_REAL_MIN_TRANSITION_PLANNER_FIXTURES` can fail undercovered
  corpus runs.
- The live eval tier now also has a mounted rough-assembly fixture hook.
  `AWIDAT_REAL_ROUGH_ASSEMBLY_FIXTURE` can point to a JSON file or
  directory, and real projects can provide
  `.awidat/eval/rough-assembly-flow.json` plus additional
  `.awidat/eval/rough-assemblies/*.json` fixtures. Each fixture can
  provide a final rough-cut EDL plus structural expectations for clip
  ranges, semantic cut boundaries, delivery metadata, forbidden ops, and
  optional proposal history. Proposal-history final-EDL assertion
  snippets must be non-empty here too. A checked-in
  `crates/eval/fixtures/real/rough-assembly-flow.sample.json` manifest
  validates that real-project fixture schema against a generated project
  with matching clip UUIDs, and
  `crates/eval/fixtures/real/rough-assemblies/` now includes the
  corresponding directory-layout sample. Real-corpus runs can set
  `AWIDAT_REAL_MIN_ROUGH_ASSEMBLY_FIXTURES` to fail when the mounted
  corpus does not include enough tuned rough-assembly fixtures.
- The live eval tier now includes a real-corpus `assess_edit_quality`
  visual-context check that fails configured corpora lacking shot
  composition or match-candidate sidecar metadata.
- That live visual-context check now measures coverage, requiring
  configurable minimum metadata-shot count, metadata coverage ratio, and
  generated match-candidate shot count before probing the assessor.
- The same live visual-context gate now counts model-backed composition
  sources separately from lightweight heuristic labels. Real corpus runs
  can require a minimum number of `composition_source: model:*` shots via
  `AWIDAT_REAL_VISUAL_MIN_MODEL_COMPOSITION_SHOTS`, making the final
  model-indexer rollout verifiable instead of silently passing on
  heuristic sidecars.
- The live visual-context gate now also counts actual
  `index/composition-model` regions via
  `AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS`, so corpus runs can
  prove valid model sidecars exist instead of only trusting copied model
  labels in `index/shot`. The live count now uses the same region
  contract as Python safe smoke: `model:*` source, valid time range,
  bounded confidence, and controlled subject/depth/framing labels.
  Invalid model-region tolerance defaults to zero and can be relaxed
  only through
  `AWIDAT_REAL_VISUAL_MAX_INVALID_COMPOSITION_MODEL_REGIONS`. Coverage
  summaries report valid and invalid composition-model region counts
  separately, so corpus tuning can distinguish missing model output from
  model output that failed the contract. The Python safe smoke preflight
  now reports the same valid/invalid region distinction with sample
  path/reason diagnostics before the Rust live eval runs, and the
  workflow maps the same minimum-region and max-invalid thresholds into
  `AWIDAT_COMPOSITION_MODEL_MIN_REGIONS` and
  `AWIDAT_COMPOSITION_MODEL_MAX_INVALID_REGIONS`. Python safe smoke now
  also accepts the real-corpus variable names as fallbacks, and a
  real-corpus minimum-region value of `0` keeps the Python preflight
  disabled to match the workflow condition. If any Python project-tree
  threshold is configured, safe smoke requires
  `AWIDAT_COMPOSITION_MODEL_PROJECT` or `AWIDAT_REAL_CORPUS` instead of
  silently skipping the configured gate. Rust live eval threshold parsing
  now fails malformed integers and metadata ratios outside `[0, 1]`
  instead of silently weakening gates through default fallback.
- The real-corpus GitHub eval workflow now forwards the visual,
  composition-model, assessor-proposal, transition-planner, and
  rough-assembly gate variables into the self-hosted corpus run. It
  now refuses an empty `AWIDAT_REAL_CORPUS` path and requires that path
  to be an existing Awidat project directory with `project.otio.json`
  before optional sidecar preflight or live eval execution. When
  `AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS` is non-zero, the
  workflow runs Python safe smoke against `AWIDAT_REAL_CORPUS` before
  the Rust live eval so model sidecar contract failures stop the corpus
  job early. The same workflow contract is now checked by Python safe
  smoke, so CI catches accidental removal of the real-corpus gate
  variables or sidecar preflight step. Python safe smoke also counts
  mounted assessor-proposal, transition-planner, and rough-assembly
  fixture files whenever their minimum gate variables are non-zero,
  catching undercovered corpora before Rust parses and applies fixture
  contents.

Completion audit evidence:

- A unified local real-corpus fixture at
  `/private/tmp/awidat-ben-unified-corpus` reuses real Ben media/index
  sidecars, adds an adjacent-clip timeline, and mounts transition
  planner, assessor proposal, and rough-assembly fixture manifests.
- `python/scripts/composition_model_from_sidecars.py` gives the
  model-sidecar rollout a repeatable producer path from existing
  model-derived face/gaze/CLIP sidecars into the stable
  `index/composition-model/<asset>.json` contract, then refreshes the
  composition and shot handoff so `composition_source: model:*` reaches
  `assess_edit_quality`.
- Strict safe smoke validates the unified corpus model tree with
  `AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS=1`, reporting one
  composition-model file, 373 valid model regions, and zero invalid
  regions.
- Strict live eval validates the same corpus with
  `AWIDAT_REAL_VISUAL_MIN_COMPOSITION_MODEL_REGIONS=1`,
  `AWIDAT_REAL_VISUAL_MIN_MODEL_COMPOSITION_SHOTS=1`, and minimum
  fixture gates for assessor proposal, transition planner, and rough
  assembly, reporting 8 passed, 0 failed, and 0 skipped scenarios.
- The live visual-context gate now proves 373 metadata shots out of 373
  total, 373 shots with match candidates, 373 with model composition,
  373 valid composition-model regions, and zero invalid
  composition-model regions.

Operational follow-up:

- The current proof corpus is local and intentionally not committed
  because it references large private media. A self-hosted runner should
  mount an equivalent persistent corpus and set the same gate variables
  for scheduled regression runs.

## Risks and Mitigations

| Risk | Mitigation |
| --- | --- |
| Semantic metadata becomes stale after clip moves or splits | Key by stable clip UUIDs and update boundaries in EDL apply paths |
| Split edits become too complex for v1 | Lower semantic ops into explicit audio tracks first |
| Agent overuses new transition ids | Require `intent`, density checks, and hard-cut alternatives |
| UI becomes cluttered | Show compact badges by default; details only in inspector |
| Visual indexers are expensive | Start with deterministic existing sidecars; add composition indexer later |
| Evals become subjective | Assert structured recommendation fields and reasons, not vague prose |

## Definition of Done

This upgrade is complete when:

- Hard cuts, semantic cuts, transitions, J-cuts, L-cuts, cutaways, and
  thematic montage are represented as distinct editorial choices.
- The agent can explain why it chose the edit.
- The desktop UI can inspect that reason.
- Export honors the edit.
- Preview either honors it or clearly marks any limitation.
- Evals cover the editing grammar from the four videos.
- Existing projects without semantic cut metadata continue to load and
  render unchanged.

## Traceability to Goal

| Required area | Covered in this plan |
| --- | --- |
| Four video-editing lessons | `Source Lessons`, `Gaps`, and `Editorial Evals` |
| Current codebase gap audit | `Current Awidat Strengths` and `Gaps` |
| Semantic cut metadata | `Semantic Cut Model`, Phase 1 |
| Split edits | `EDL Operations`, Phase 2 |
| Edit-quality tooling | `Edit Quality Tooling`, Phase 3 |
| Visual analysis | `Visual Signal Upgrade`, Phase 4 |
| Transition registry updates | `Transition Registry Upgrade`, Phase 5 |
| Desktop UI upgrades | `Desktop UI Upgrade`, Phase 6 |
| Skills | `Skills Upgrade` |
| Evals | Phase 7 |
| Recommended first sprint | `Recommended First Sprint` |
