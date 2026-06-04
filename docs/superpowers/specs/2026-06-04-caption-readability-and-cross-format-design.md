# Caption Readability Model + Cross-Format Caption Planning — Design

**Date:** 2026-06-04
**Status:** Approved (brainstorming) → ready for implementation plan
**Feature:** captions (iteration 1 of the editorial-skills program)
**Branch base:** `feat/awidat-editorial-upgrades`

---

## 1. Goal

Give awidat a real **caption reading-speed / segmentation model** and lift caption
**planning + styling** out of the short-form-only path into a **format-agnostic**
service, so captions read well on any format — not just vertical short-form.

Close the loop by producing **real renders** the user signs off on:

- **Short6_VCTake.mp4** (vertical short-form, 36s) — regression: short-form must
  still produce **valid, well-formed, legible** captions through the refactored path.
  Note: short-form now routes through the shared readability model, so cue boundaries
  may *improve* (change) wherever the old naive chunking exceeded 17 CPS. "Regression"
  here means "still correct and legible," **not** byte-identical to today's output.
- **A long-form Episode segment** (~60s) — proof: captioned end-to-end through the
  **new** cross-format path, rendered **twice** (minimal-cinematic and active-pop)
  so the user compares real frames and picks the register.

Definition of done: both renders produced, short-form still valid/legible through the
refactored path, long-form cleanly captioned through the new path, user reviews real
output and signs off.

## 2. Background / current state (verified against the codebase)

- `apply_edl` **Insert Caption** (`crates/core/src/tools/apply_edl.rs`) inserts
  non-destructive caption nodes on the Titles track (`role="caption"`, optional
  per-word reveal). **Format-agnostic. Reused as-is.**
- `crates/core/src/captions.rs` — caption inventory / export authority / sidecar /
  safe-area QC. **Already format-agnostic**; wired into `render_preflight`,
  `start_render`, `verify_render`, and `podcast_qc_report` (long-form already uses it).
  **Reused as-is.**
- `crates/core/src/caption_rendered_output_scorer.rs` — post-render frame QC
  (safe-area containment + occlusion via luma variance). **Already format-agnostic
  (takes safe-area profile + dims).** Reused as-is.
- `crates/core/src/scene_aware_short_form.rs` — contains the **only** caption
  planner (`plan_captions`), `CaptionRecommendation`, `CaptionWordTiming`,
  `CaptionPlacement`, and `CaptionStyle` (`Plain`/`Boxed`/`Minimal`). Today
  `plan_captions` segments via `transcript_segments` (whatever the transcript was
  pre-chunked into — **no CPS/readability enforcement**) and derives placement/style
  from scene-aware shot analysis.
- **Confirmed absent:** any characters-per-line, characters-per-second, or
  reading-speed model anywhere. (The only `max_chars` logic is title text-wrapping in
  `crates/render/src/timeline.rs`.) This is the highest-value gap.

So the real work is narrower than the original handoff implied: export/QC/insertion
are already shared. Only **segmentation readability**, **planning**, and **styling**
need building/generalizing.

## 3. Architecture (Approach A — extract-and-share)

Three new modules under `crates/core/src/`, plus rewiring the short-form caller and
adding one new format-agnostic tool.

### 3.1 `caption_readability` (shared core — gap #1)

Pure, no I/O, no scene analysis. The single source of truth for "does this read?"

```
struct CaptionFormatProfile {
    max_chars_per_line: usize,   // short_form 15, long_form 42, accessibility 42
    max_lines: usize,            // short_form 1, long_form 2, accessibility 2
    max_cps: f64,                // 17.0 everywhere
    min_cue_s: f64,              // ~0.5
    max_cue_s: f64,              // ~7.0
    reveal: RevealMode,          // WordByWord | PhraseByPhrase | WholeCue
}
// constructors: CaptionFormatProfile::short_form() / long_form() / accessibility()

struct Cue { start_s, end_s, lines: Vec<String>, word_timings: Vec<CaptionWordTiming> }

enum ReadabilityProposal {       // for linting EXISTING cues — never auto-rewrites
    Split  { at_s, rationale },
    Extend { to_s,  rationale },
    Reflow { rationale },
}
```

- `segment(words: &[CaptionWordTiming], profile) -> Vec<Cue>`
  Groups words into cues honoring `max_cps`, `max_chars_per_line`, `max_lines`,
  `min_cue_s`/`max_cue_s`; breaks on **sense units** (punctuation/clause boundaries,
  not mid-phrase); **zero-gap** on continuous speech; bottom line ≤ top line for
  2-line cues.
- `lint(cues: &[Cue], profile) -> Vec<ReadabilityProposal>`
  Flags CPS overruns, over-long lines, too-many-lines, sub-min / over-max durations,
  inter-cue gaps. Each proposal carries a human rationale string.

### 3.2 `caption_planner` (format-agnostic planning — gap #2)

Turns cues into `CaptionRecommendation`s using injected strategies. Placement is the
only genuinely visual/short-form-specific concern, so it is a strategy:

```
trait PlacementStrategy {
    fn place(&self, cue: &Cue) -> (CaptionPlacement, visual_reason, safety_reason, confidence);
}
// ShotAwarePlacement  — wraps today's scene-aware logic (face/negative-space/motion)
// LowerSafeZonePlacement — long-form/cinematic default (bottom safe zone, whole cue)

fn plan(cues, &dyn PlacementStrategy, &StyleStrategy) -> Vec<CaptionRecommendation>
```

`CaptionRecommendation` / `CaptionWordTiming` / `CaptionPlacement` are **moved** from
`scene_aware_short_form.rs` into the shared layer (re-exported so existing callers and
tests keep compiling). `CaptionPlacement` gains no new variants for v1.

### 3.3 `caption_styles` (gap #3)

Registry keyed by `(format, mood)` returning a `CaptionStyle` + concrete style params,
with a **hard legibility floor**: outline or shadow is always on, high contrast, weight
that survives compression. Two mood registers implemented for v1:

- `minimal_cinematic` — clean static lower-third, whole-cue, subtle.
- `active_pop` — energetic word-by-word / karaoke highlight, bolder.

The legibility floor is enforced in code (a style can never disable both
outline and shadow), so "mood" can never override readability.

### 3.4 Rewiring + new entry point

- `build_scene_aware_short_form_plan` keeps its public signature. Its internal
  `plan_captions` becomes a thin caller:
  `readability::segment(short_form())` → `caption_planner::plan(ShotAwarePlacement, short_form style)`.
  Behavior parity is the Short6 regression target.
- **New tool `plan_captions`** (format-agnostic), registered in the tool catalog
  (`crates/tools/src/lib.rs` / `crates/core/src/tools/`). Input: asset/clip +
  transcript + `format` + `mood`. Output: caption EDL fragment (same Insert Caption
  format `apply_edl` consumes) + readiness + the readability lint summary. This is the
  tool the `caption-director` skill calls for long-form/cinematic.

## 4. Data flow

**Short-form (regression):**
`transcript → readability::segment(short_form) → caption_planner(ShotAwarePlacement, short_form style) → CaptionRecommendations → existing EDL fragment → apply_edl`

**Long-form (proof):**
`Whisper transcript (words+timings) → readability::segment(long_form) → caption_planner(LowerSafeZonePlacement) + caption_styles(long_form, mood) → CaptionRecommendations → EDL fragment → apply_edl Insert Caption → start_render → poll_render → inspect frame` (run for both moods)

## 5. Error handling / non-destructive contract

- No transcript / not indexed → the tool stops and says so (skill rule); never invents
  words.
- `lint` and the readability model **emit proposals with rationale**; they never
  silently rewrite an existing user timeline.
- Legibility floor is non-overridable by mood/style selection.
- New tool degrades to a clear error (not a panic) on missing word timings.

## 6. Testing

- **Unit (`caption_readability`):** CPS ceiling enforced; cpl/line caps; sense-unit
  break (no mid-phrase splits); zero-gap on continuous speech; min/max duration; 2-line
  bottom≤top; `lint` emits a `Split` for a known >17 CPS cue and an `Extend` for a
  sub-min cue, each with rationale.
- **Unit (`caption_styles`):** legibility floor holds for every (format, mood) entry;
  both mood registers return distinct, valid styles.
- **Unit (`caption_planner`):** `ShotAwarePlacement` reproduces today's short-form
  placement/style on a fixture; `LowerSafeZonePlacement` returns bottom safe zone.
- **Regression:** existing `scene_aware_short_form` / `short_form_review` tests pass
  after the move/extraction. Tests that assert exact pre-CPS cue boundaries are updated
  to the readability-model output (expected, since adding a real CPS model is the point);
  tests asserting structure/placement/style behavior must stay green unchanged.
- **Catalog:** `plan_captions` appears with correct schema (`skill_catalog.rs` /
  `capability_manifest.rs` style tests).
- **End-to-end (manual, the proof):** Short6 render unchanged; Episode segment rendered
  for both moods; frames inspected for safe-area, occlusion, and readability.

## 7. Scope boundaries (YAGNI)

**In:** gap #1 (readability/segmentation), gap #2 (cross-format planning), gap #3
(styling registry, 2 moods). Short6 + Episode renders.

**Out (later iterations):** gap #4 proofreading lint, gap #5 translation, gap #6
unified `assess_captions` across formats (existing QC is reused as-is for now),
gap #7 first-class keyword emphasis. No new `CaptionPlacement` variants. No changes to
`apply_edl`, `captions.rs`, or the rendered-output scorer.

## 8. Risks / dependencies

- **Whisper indexing for the Episode (blocking the long-form proof).** Need word
  timings for a ~60s Episode slice. Verify awidat's local indexing path runs on the
  test footage during planning. **Fallback:** import an `.srt`/`.vtt` sidecar as the
  caption source (import path already exists) if local Whisper indexing is not runnable.
- **Driving the agent loop** needs `ANTHROPIC_API_KEY` in the environment.
- **Extraction risk** to short-form behavior — mitigated by keeping the public API,
  re-exporting moved types, and the Short6 regression render + existing tests.
- Test footage is on the external drive `/Volumes/Explicit's Hard Drive/`.

## 9. Open items for review time

- Pick the long-form style register (minimal-cinematic vs active-pop) **after** seeing
  both rendered frames.
