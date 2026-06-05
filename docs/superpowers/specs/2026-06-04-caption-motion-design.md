# Animated Caption Motion — Design (Captions Phase 2.1)

**Date:** 2026-06-04
**Status:** Approved (brainstorming) → ready for implementation plan
**Feature:** captions (Phase 2, sub-project 1 of several)
**Branch:** `feat/caption-styling-phase2`
**Builds on:** Phase 1 (`2026-06-04-caption-style-presets-design.md`) — the `CaptionStyleSpec`, `style_json` plumbing, libass render, and active-word-pop emission.

---

## 1. Goal

Add **agent-native caption motion** — composable animation (entrance, active-word emphasis, exit, continuous) applied via ASS animation tags — so awidat produces the "premium" animated short-form caption look. Grounded in the corpus and governed by **restraint**: minimal/purposeful by default, motion as an emphasis lever, register by content.

## 2. Corpus grounding (what real editors say)

From the transcript corpus (`video_editing_transcripts/knowledge/captions/_caption_excerpts.md`):
- **Motion = premium:** *"if you want your captions to actually look premium, you need to add motion."*
- **Staples:** *"a minimal pop-up animation to all the text"* and *"a normal slide-up word captioning,"* both synced per word.
- **"a little bounce, cute looking subtitles"** — bounce is wanted, but *little*.
- **Restraint is the loudest rule:** *"no distractions… captions popping up every word it is so distracting to the story… people appreciate the simplicity"*; *"clean subtitles, good styling, and **purposeful** animations."*
- **Motion is an emphasis lever:** editors keep *"a more simple caption… and some more large, fun, poppy captions"* and reach for poppy ones *only* *"if I want to emphasize a piece of text,"* not every line.
- **Register by content:** *"cinematic subtitles from short films and active minimal pop-ups from talking-head videos."*
- **A small curated favorites set**, not infinite knobs.

These drive the defaults and the skill guidance below — not just the mechanism.

## 3. Architecture

A composable motion model on the caption style, lowered to ASS tags at render. Consistent with Phase 1 (rides `style_json`; no new EDL grammar).

### 3.1 Style model (`crates/core/src/caption/styles.rs`)

Add `motion: CaptionMotion` to `CaptionStyleSpec`, with four independent slots that layer on one caption:

```
struct CaptionMotion { entrance, active_word, exit, continuous }

enum EntranceMotion   { None, PopIn, SlideUp, FadeIn }
enum ActiveWordMotion { None, Bounce, ScalePop, Shake }   // requires active-word-pop reveal
enum ExitMotion       { None, PopOut, FadeOut, SlideDown }
enum ContinuousMotion { None, Float }
```

`CaptionMotion::default()` = all `None` (so a caption with no motion renders exactly as Phase 1). `Serialize`/`Deserialize` → the `style_json` blob; render mirror `CaptionRenderMotion` (same field-parity pattern as `CaptionRenderStyle`).

### 3.2 Render lowering (`crates/render/src/ass.rs`)

A motion→ASS-tag composer that layers tags onto each Dialogue line, timed relative to that line. Concrete tag families and feel (subtle by corpus mandate; durations in ms):

- **Entrance** (line start, ~120–150ms): `PopIn` → `\fscx80\fscy80\t(0,120,\fscx103\fscy103)\t(120,170,\fscx100\fscy100)` (small overshoot — "a little"); `FadeIn` → `\fad(150,0)`; `SlideUp` → a vertical `\move` into the resting position + `\fad`.
- **Exit** (line end, ~150ms): `PopOut` → closing `\t(D-150,D,\fscx80\fscy80)` + fade; `FadeOut` → `\fad(0,150)`; `SlideDown` → closing `\move`.
- **Active-word** (per active word in the active-word-pop emitter, ~150–180ms): `Bounce` → `\t(0,90,\fscx115\fscy115)\t(90,180,\fscx100\fscy100)` spring; `ScalePop` → `\t(0,120,\fscx112\fscy112)` (grow, settle); `Shake` → small `\frz` oscillation via chained `\t`. Layered with the existing color `\c` override.
- **Continuous**: `Float` → a single slow `\move`/`\t` drift over the line (ASS has no native loop; documented as a subtle one-pass drift, not a perpetual bob).

**Composition with the active-word-pop reveal (the key rule):** an active-word-pop cue is N Dialogue lines (one per word). Entrance tags go only on the **first** word's line; exit only on the **last**; continuous on all; active-word on each word's own line. For whole-cue / word-by-word reveals, entrance/exit/continuous sit on the single line and active-word is N/A. The composer takes each line's role (first/middle/last/sole) to place tags correctly.

**Positioning note (honest scope):** scale (`\fscx/\fscy`), rotation (`\frz`), and alpha (`\fad`) motions compose cleanly with the existing alignment+`MarginV` positioning. **Position-based** motions (`SlideUp`, `SlideDown`, `Float`) need the caption's resting (x,y), which libass derives from alignment+margin+canvas — so the composer must compute that resting position to emit `\move`. This is the meatier render piece (see Risks).

### 3.3 Presets + the emphasis lever (`caption::styles`)

Corpus-tuned defaults (restraint + register + emphasis):
- `clean_white` (cinematic/long-form) → `motion = all None`. Minimal.
- `word_pop` (active short-form) → `entrance: PopIn`, `active_word: Bounce` (subtle), `exit: None`, `continuous: None`.
- `boxed` → `motion = all None` (the box already carries emphasis).
- **new `emphasis`** preset → poppier: bigger `PopIn` + `Bounce` + the box background + larger font — the look editors reserve for hook/keyword lines. The agent applies it to *emphasis* moments, not every line.

### 3.4 Skill (`caption-director` SKILL.md)

Encode the corpus rule: **motion = premium but purposeful.** Defaults minimal; register by content (cinematic→none, short-form→active pop); **motion is an emphasis tool** — reserve `emphasis`/poppier motion for the lines that matter (hooks, payoffs, keywords), keep filler simple; never animate so much it competes with the story. Document the motion vocabulary + that the agent proposes and the user can override.

## 4. Data flow

`plan_captions`/scene-aware (pick preset → `CaptionStyleSpec` incl. `motion`) → `style_json` on the Insert Caption EDL → parse → `TitlePlan.caption_style.motion` → `ass.rs` composer emits the layered ASS tags → libass → render.

## 5. Error handling / contract

- `motion` absent in `style_json` → `CaptionMotion::default()` (all None) → Phase-1 behavior unchanged.
- `ActiveWordMotion != None` but reveal isn't active-word-pop (or no word timings) → active-word motion is a no-op (degrade); entrance/exit still apply.
- Position-based motion when the resting position can't be computed → fall back to the non-position variant (e.g. `SlideUp`→`FadeIn`, `SlideDown`→`FadeOut`, `Float`→none) rather than mis-position.
- Outline/shadow/legibility invariants from Phase 1 unchanged.

## 6. Testing

- **styles unit:** each preset's `motion` matches the corpus-tuned defaults; `CaptionMotion` serde round-trips; `clean_white`/`boxed` are all-None; `word_pop` is PopIn+Bounce; `emphasis` is poppy+box.
- **ass unit (composer):** PopIn emits the overshoot `\t` scale on a cue line; FadeIn emits `\fad`; exit tags land at the end of the line; **active-word-pop composition** — entrance only on the first word's Dialogue, exit only on the last, Bounce on each word's line; whole-cue reveal puts entrance+exit on the single line; `ActiveWordMotion` with no word timings → no active-word tags (degrade); position-based motion falls back when resting position is unavailable.
- **regression:** all-None motion → byte-identical to Phase-1 ASS; scoped `awidat-core` + `awidat-render` suites green.
- **end-to-end (manual proof, sign-off):** render `word_pop` (subtle pop-in + active-word bounce) and `emphasis` (poppy) on the vertical clip; confirm the cue pops in, the active word springs as spoken, and it reads as *premium but not distracting*. User signs off.

## 7. Scope boundaries (YAGNI)

**In:** the 4-slot `CaptionMotion` + the named templates above; corpus-tuned preset motions + a new `emphasis` preset; `style_json` motion plumbing; ASS composer incl. the active-word-pop composition rule and resting-position computation for slide motions; skill guidance; proof renders.

**Out (later):** per-cue emphasis *intelligence* (the agent auto-deciding which lines get the poppy preset — a planner feature, separate sub-project); perpetual/looping continuous motion (ASS limitation); easing-curve customization; motion on non-caption titles; the other Phase-2 sub-projects (static knobs: italic/underline/spacing/scale/rotation; manual override; template-library expansion).

## 8. Risks / dependencies

- **Slide/Float need resting-position math** (alignment+margin+canvas → x,y). Most fragile part; isolated behind the position-based variants with a safe fallback to fade/none. The plan implements + tests it separately; if it proves heavy, slide/float can ship in a follow-up while pop/fade/bounce land first.
- **ASS has no native loop** → `Float` is a single subtle drift, not a perpetual bob. Documented; acceptable for v1.
- **Over-animation risk** is the product risk, not a code risk — mitigated by restrained defaults + the skill rule (the corpus's loudest lesson).
- **Disk**: scoped builds/tests (`-p awidat-core -p awidat-render`, `CARGO_INCREMENTAL=0`). Render proofs from a clean (no-apostrophe) path. Reuse `/Users/explicit/vshort_src.mp4` + the warm whisper env.

## 9. Open items for review time

- Exact pop/bounce timings and overshoot amounts (tune on the render — "a little").
- (Resolved) The `emphasis` **preset** ships in this sub-project (a preset value: poppy motion + box + larger font, selectable via `plan_captions preset=emphasis`). The **intelligence** that auto-applies it to hook/keyword lines is a separate later sub-project (§7 Out).
