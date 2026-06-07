# Proposal-to-Visual-Support Gap Analysis

Status: first end-to-end workflow slice implemented and tested for quote
highlights, animated/retention lists, and source-backed B-roll packages.
The planner now uses a hybrid editorial-skills layer: Rust owns deterministic
registration, matching, ranking, and proposal interfaces while bundled
`SKILL.md` files remain the inspectable editorial playbooks.

## Goal

An editor can select transcript text, a topic, quote, claim, chapter, list item,
or timeline region and ask Montage for visual support. Montage returns reviewable
artifact proposals with evidence, missing-information prompts, preview
expectations, apply-ready timeline payloads when possible, and render
verification steps.

The first implemented slice is the read-only
`plan_visual_support_proposals` MCP tool. It does not mutate the project. It
hands accepted work back to the existing `apply_edl` proposal path.

## Reused Systems

- `apply_edl` remains the only timeline mutation path.
- Existing `Add Proposal Package` EDL metadata and desktop `ProposedEdit`
  inspector fields now carry visual-support review context into the native
  Proposal Inspector path.
- Motion graphics use existing MotionScene storage through `*** Set Motion Scene`.
- B-roll placement uses existing `*** Insert BRoll` EDL.
- Review metadata follows existing Proposal Inspector concepts: intent,
  rationale, confidence, risk, evidence, alternatives/iteration guidance.
- B-roll generation stays routed through existing generated-media tools:
  `find_generated_broll_opportunities`, `start_generated_media_job`,
  `poll_generated_media_job`, and `use_generated_media`.
- Render verification stays routed through existing timeline render and
  verification tools: `start_render`, render manifests, and `verify_render`.
- Podcast pipeline integration reuses the existing `podcast_visual_polish`
  pre-render gate instead of adding a DaVinci-style UI surface.
- The existing bundled skill loader remains the inspectability mechanism for
  skill playbooks; the new Rust registry links to those files instead of
  replacing them.

## Newly Added Systems

- `plan_visual_support_proposals` in the in-process Montage MCP tool surface.
- `editorial_skills` core module with:
  - `EditorialSkillDefinition` for reusable skill metadata and `SKILL.md`
    linkage
  - `EditorialSkillInstance` for per-selection matches, confidence, reason,
    transcript anchor, and artifact type
  - `EditorialSkillOpportunity` for story/timeline signals that should become
    proposal-ready skill candidates
  - deterministic bundled registration, matching, and confidence ranking
- A single workflow planner that maps selected editorial context to artifact
  proposals instead of only routing to a lane.
- Artifact proposal contracts for:
  - quote highlight
  - animated list
  - title card
  - search bar
  - counter/stat graphic
  - map visualization
  - B-roll package
- Proposal output includes:
  - artifact type and title
  - `editorial_skill` provenance for inspectable, composable reuse
  - reference asset policy, structured reference items, inferred reference kind
    and role, path-derived style tokens, influence guidance, and reference
    evidence rows
  - export intent such as aspect ratio, platform, alpha, duration, and
    preview/render requirements
  - intent, rationale, confidence, risk, and evidence
  - missing-information prompts when an artifact cannot be applied yet
  - `apply_edl` payloads when the artifact is ready to become a timeline object
  - a prepended `Add Proposal Package` op with summary, status, confidence,
    and evidence for native Proposal Inspector review
  - preview expectations
  - post-acceptance and artifact-specific verification steps
  - natural-language revision contract and examples
- Planner output includes `skill_candidates`, preserving the ranked
  `EditorialSkillInstance` evidence used to create proposals.
- The editorial-skills layer can convert story signals such as hooks, topic
  shifts, weak visuals, lists, stats, quotes, and map/location references into
  `EditorialSkillOpportunity` records with `next_tool:
  plan_visual_support_proposals`.
- `plan_visual_support_proposals` accepts optional story-map/topic/beat/shot
  or transcript-window context. That context participates in deterministic
  skill matching and is carried into proposal review evidence.
- `editorial_skill_registry` now reports the hybrid contract explicitly:
  Rust-owned registration/triggering/ranking/composition/verification and
  `SKILL.md`-owned reasoning/examples/style guidance.
- `revise_visual_support_proposal` is a read-only revision tool that accepts a
  previous proposal plus a natural-language instruction, returns a revised
  proposal, supports detected artifact-type conversion across the planned
  proposal set, includes a compact diff before the editor applies it, and
  returns a side-by-side preview comparison contract for before/after review.
- `verify_visual_support_artifact` is a read-only proposal verifier that runs
  deterministic artifact-specific checks before pairing the accepted artifact
  with `verify_render`. It now also emits a rendered-frame verification
  contract and can consume rendered-frame reports so failed frame checks fail
  the visual-support artifact.
- Search bars, counter/stat graphics, and route maps now use explicit
  MotionScene slot schemas (`search-query`, `stat-value`, `stat-label`,
  `map-origin`, `map-destination`, `route-line`) instead of only a generic
  headline layer.
- `podcast_visual_polish` now extracts project timeline markers plus topic,
  editorial-moment, whisper transcript, and weak shot sidecars into
  `EditorialSkillOpportunity` records, preserving labels and exact timeline
  ranges for downstream proposal planning.
- Apply-ready visual-support proposals now keep proposal review metadata and
  concrete artifact creation in the same `apply_edl` envelope, so the desktop
  proposal bridge can surface summary, intent, risk/confidence, and evidence
  before acceptance.
- `save_visual_support_defaults` persists clarification answers such as aspect
  ratio, platform, alpha/transparent-background intent, reference assets,
  typography, color palette, motion intensity, safe-area policy, and show brand
  package to `.montage/visual_support_defaults.json`. Later proposal planning
  reuses those defaults when the editor omits the same values.
- Initial editorial skills:
  - retention-list-opener
  - quote-highlight
  - search-bar-sequence
  - source-backed-broll
  - route-map
  - statistic-counter
  - podcast-hook
  - chapter-intro
  - short-form-reframing
- Bundled `skills/<name>/SKILL.md` playbooks for those initial editorial
  skills. Each one routes through proposal planning, revision, `apply_edl`,
  render, and verification instead of creating a separate editor surface.
- Bundled `skills/<name>/examples/visual-support-proposal.json` examples for
  each initial editorial skill, giving agents and contributors an inspectable
  reusable proposal shape beyond prose guidance.
- The canonical podcast episode producer skill now routes visual polish through
  `plan_visual_support_proposals` and `revise_visual_support_proposal` at the
  same pre-render gate.

## Demonstrated End-to-End Slice

The focused test suite demonstrates that:

- A quote selection produces a quote-highlight proposal with evidence and a
  parseable MotionScene `apply_edl` payload.
- A list-like selection produces an animated-list proposal with MotionScene
  step layers that preserve transcript evidence items in order.
- A B-roll request with a project-relative generated-media asset produces an
  apply-ready `Insert BRoll` package with no missing-information prompts.
- Accepted quote, list, and B-roll proposals commit through `apply_edl` into a
  temporary Montage project, storing both the review package and the timeline
  artifact.
- The accepted project renders through `start_render`.
- The rendered output passes `verify_render`.
- Proposal contracts include editorial skill provenance, reference assets,
  structured reference records, export intent, revision instructions, and
  artifact-specific verification.
- Artifact-specific proposal verification now passes for every currently
  planned proposal artifact type:
  - apply-ready proposals store a native proposal package with transcript/source
    evidence before the concrete timeline artifact op
  - quote highlight: transcript evidence anchor appears in MotionScene text
  - animated list: missing list-item clarifications fail verification, and
    MotionScene contains list step layers that preserve transcript evidence
    items in order
  - title card: topic/title text appears in the MotionScene text from
    transcript evidence
  - B-roll package: `InsertBRoll` anchor matches transcript evidence, source
    provenance and disclosure policy are present, and the project-relative
    asset can be required to exist
  - search bar: missing query clarifications fail verification, query text
    appears in the MotionScene text from transcript evidence, and the scene
    carries a typed `search-query` slot
  - counter/stat graphic: missing numeric-value clarifications fail
    verification, selected numeric value appears in the MotionScene text from
    transcript evidence, and the scene carries a count-up `stat-value` slot
  - map visualization: missing route/location clarifications fail verification,
    specified route labels must appear in the MotionScene text, and the scene
    carries origin/destination labels plus a `route-line` slot
- Natural-language revision updates pending proposals for duration/pacing,
  alpha/transparent-background intent, and lower visual intensity, then returns
  a reviewable diff before `apply_edl`.
- Natural-language revision can convert a pending proposal into another planned
  artifact type, including source-backed B-roll, title cards, quote highlights,
  animated lists, search bars, counters, and maps. The returned revision
  includes a `visual_diff` summary comparing artifact type, timeline object,
  duration, alpha intent, visual intensity, and missing information count
  before and after the change, plus a side-by-side preview comparison contract
  for before/after preview-cache outputs and verification.
- `verify_visual_support_artifact` now declares required rendered-frame sample
  points and frame-level checks for accepted artifacts. When a rendered-frame
  report is supplied, failed frame checks such as overlay visibility or alpha
  preservation fail the artifact verification result.
- Project visual-support defaults can be saved once and reused by later
  proposal planning, reducing repeated clarification prompts for style, brand,
  motion, safe-area, and export intent.
- Clarification prompts are now artifact-specific where implemented:
  source-backed B-roll asks for an asset or generation approval when missing,
  animated lists ask for list items or structure when the transcript selection
  is too vague, and map visualizations ask for a location/route only when
  transcript and anchor text do not identify one clearly enough.
- `podcast_visual_polish` now points agents at the editorial-skill proposal
  gate after story map/cleanup and before final render.
- `podcast_visual_polish` now emits structured
  `editorial_skill_opportunities`, so an agent/pipeline can request proposal
  planning automatically for missing B-roll packages, missing chapter/title
  support, hard-cut B-roll, and caption/topic-support moments.
- Timeline markers stored in `metadata.montage.timeline_markers`, topics stored
  under `index/topic/**.json`, editorial moments stored under
  `index/editorial-moments/**.json`, transcript segments stored under
  `index/whisper/**.json`, and weak shots stored under `index/shot/**.json`
  now become editorial-skill opportunities automatically. Overlapping duplicate
  story signals are deduplicated before skill matching. Story-map, chapter,
  topic, hook, stat claim, and weak-visual evidence can trigger exact-range
  title-card, hook, counter, map, quote/list, or B-roll proposals without
  flooding the editor with exact duplicates.
- The planner can also take story context directly, so a generic visual-support
  request can still become a title card, quote highlight, list, B-roll package,
  counter, map, or other planned artifact when story evidence points there.
- Bundled editorial skills are loadable through the existing skill registry and
  documented as Proposal-to-Visual-Support playbooks.
- The desktop transcript pane can now turn a selected transcript word range
  into a visual-support proposal directly. The command plans through
  `plan_visual_support_proposals`, selects an apply-ready proposal, and opens it
  through the existing Proposal Inspector/ghost-overlay flow.
- Hybrid editorial-skill matching is covered by
  `cargo test -p montage-core --test editorial_skills`, including the
  Definition/Instance split and deterministic ranking of competing list/B-roll
  candidates.

Run:

```bash
cargo test -p montage-core plan_visual_support_proposals --lib
```

Manual end-to-end flow for the required artifacts:

1. Select transcript text in the editor and request visual support, or pass the
   selection to `plan_visual_support_proposals`.
2. Review the resulting Proposal Inspector card with intent, explanation,
   confidence/risk, and evidence.
3. If planning reports missing information, answer the clarification before
   opening the apply-ready proposal.
4. Inspect the accepted timeline object with `view_timeline`.
5. Preview the affected region in desktop or preview cache.
6. Render with `start_render`.
7. Verify with `verify_render`.
8. Run `verify_visual_support_artifact` on the accepted proposal for
   proposal-level quote/list/B-roll contract checks, rendered-frame contract
   checks when a frame report is supplied, and generated B-roll provenance and
   disclosure metadata.
9. For generated B-roll, confirm provider metadata still matches the accepted
   source-provenance record.

## Remaining Limitations

- The planner is read-only and produces proposal payloads. Apply-ready
  proposals now carry native Proposal Inspector metadata through
  `Add Proposal Package`, and transcript selections can open an apply-ready
  visual-support proposal without agent mediation. The desktop proposal store
  now retains multiple pending visual-support proposals and exposes a compact
  picker above the Proposal Inspector when several skills match.
- Natural-language iteration now has a dedicated revision tool for basic
  pacing/duration, alpha intent changes, lower visual intensity, and detected
  artifact-type conversion across the planned proposal set. It returns a
  structured visual diff plus a side-by-side preview comparison contract. It
  does not yet support free-form conversions outside known artifact types.
- MotionScene templates now have dedicated slot schemas for quote/list/title,
  search, counter, and map proposals. Remaining template limitations are richer
  typography controls, rendered preview thumbnails, and more advanced map/chart
  drawing beyond the current native shape/text slots.
- The editorial-skill registry now has deterministic Rust definitions, bundled
  `SKILL.md` playbooks, and one inspectable proposal example per skill. Those
  skills still do not have deep per-skill scripts or template libraries.
- Story-signal opportunity detection exists, the planner accepts
  story-map/topic/beat/shot/transcript-window context as first-class proposal
  input, and podcast visual polish extracts timeline markers plus topic,
  editorial-moment, whisper transcript, and weak shot sidecars automatically.
  Overlapping duplicate story signals are deduplicated. The remaining
  limitation is richer transcript scoring and cross-modal ranking, not the
  basic exact-range extraction path.
- Reference-driven creation now has first-class structured proposal records,
  evidence rows, and path-derived style tokens for supplied references, but it
  does not yet inspect reference images or extract pixel-level style features.
- Style/project defaults now cover export intent, references, typography,
  color palette, motion intensity, safe-area policy, and per-show brand
  package. They are still project-level defaults rather than per-series,
  per-platform, or per-skill default profiles.
- B-roll packages are apply-ready only when a project-relative asset already
  exists. Otherwise the planner correctly asks for generation/asset selection.
  Animated lists now ask for explicit list items or structure when the selected
  text is underspecified, map visualizations ask for route/location
  clarification, counter/stat graphics ask for a numeric value or source when
  none is present, and search-bar sequences ask for an exact query when the
  selection does not contain one. Remaining clarification contracts are still
  intentionally conservative and should be expanded as richer templates land.
- Artifact-specific verification now covers quote/list/title/B-roll/search/
  counter/map proposal contracts and can consume rendered-frame reports for
  frame-level pass/fail evidence. Remaining verification depth is specialized
  semantic analysis such as geographic route correctness and quote-to-transcript
  OCR alignment.

## Recommended Future Work

- Add richer proposal comparison controls for transcript selections, including
  UI rendering of the side-by-side preview contract and explicit skill-ranking
  rationale when the planner returns multiple apply-ready visual-support
  proposals.
- Expand `revise_visual_support_proposal` beyond detected artifact conversion
  and export/style tweaks to typography edits and free-form conversions outside
  known artifact types.
- Expand persisted clarification defaults from one project-level profile into
  per-series, per-platform, and per-skill default profiles.
- Add deeper per-skill scripts, templates, and richer evidence policies where
  the bundled playbooks and example proposal artifacts are not enough.
- Improve sidecar-derived `EditorialSkillOpportunity` creation with richer
  transcript scoring and shot/transcript fusion so agents can rank multiple
  quote highlights, list openers, B-roll packages, and chapter intros across
  exact podcast timeline ranges without flooding the editor.
- Expand MotionScene templates beyond current slot schemas with typography
  variants, richer map geometry, chart primitives, and rendered comparison
  thumbnails.
- Expand artifact-specific verification beyond the current rendered-frame
  report contract:
  - rendered list text is readable in-frame at the expected timing
  - generated B-roll provider/job metadata matches the proposal provenance
    record
  - quote-to-transcript alignment is verified from rendered pixels or preview
    frames
