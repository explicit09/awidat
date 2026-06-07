# Caption Style Presets (agent-native) + Vertical Output — Design (Captions Iteration 3, Phase 1)

**Date:** 2026-06-04
**Status:** Approved (brainstorming) → ready for implementation plan
**Feature:** captions (iteration 3, phase 1)
**Branch base:** `feat/montage-editorial-upgrades`
**Builds on:** iterations 1 & 2 specs (readability/cross-format; native render/placement)

---

## 1. Goal

Give montage an **agent-native caption style system**: a compact style schema + a curated
library of **preset templates** that the agent applies (user can pick/override) — delivering
the looks short-form editors actually use (esp. the **active-word "pop"** highlight), without
becoming a 25-knob manual control panel. Plus the **9:16 vertical-output fix** so short-form
renders on a correct canvas. Prove via montage-native renders the user signs off on.

Non-goal (this phase): exposing every CapCut knob for manual tweaking; rotation, character
spacing, glow, animated templates — deferred until a real reel needs them (YAGNI).

## 2. Background (verified)

- `crates/render/src/ass.rs` emits the caption ASS `Style:` row and `Dialogue:` lines. Every
  Phase-1 attribute maps to a native ASS field/tag: weight→`Bold`, casing→text transform,
  primary→`PrimaryColour`, active-word highlight→inline `\c`, background box→`BorderStyle=3`
  + `BackColour`, outline/shadow already on (`BorderStyle=1`, `Outline=3`, `Shadow=2` today).
- Caption styling today is thin: `CaptionStyleSpec { font_size, color, reveal }` in
  `caption::styles`; the EDL caption op carries discrete `font_size`/`color`; reveal is
  whole-cue or cumulative `\k` karaoke (both colors white today → no visible pop).
- The `Insert Caption` EDL op + parser + `apply` + `TitlePlan` carry discrete fields; the
  parser reads `*_json` fields via `take_field_json` (e.g. `word_timings_json`).
- `Set Output Format` sets `aspect_ratio` in the EDL/timeline, but the render canvas comes out
  1920x1080 regardless (the filed vertical bug) — short-form renders aren't vertical.

## 3. Architecture

Five components. The render crate already supports the visuals; the work is a style schema, a
preset library, EDL plumbing via an extensible `style_json` blob, the active-word-pop emission,
and the vertical-canvas fix.

### 3.1 Rich style model + preset registry (`crates/core/src/caption/styles.rs`)

Grow `CaptionStyleSpec` to the Phase-1 renderable attributes:

```
struct CaptionStyleSpec {
    font_size: u32,
    weight: CaptionWeight,        // Normal | Bold
    casing: CaptionCasing,        // AsIs | Upper
    primary_color: String,        // "#RRGGBB"
    highlight_color: Option<String>, // active-word color; None = no pop
    reveal: RevealMode,           // WholeCue | WordByWord | ActiveWordPop  (new variant)
    background: CaptionBackground, // None | Box { color: String, opacity: u8 }
}
```

A **preset registry** keyed by name → `CaptionStyleSpec`, with a legibility floor
(min font size, valid hex colors; outline/shadow remain a render invariant). Phase-1 presets:

- `clean_white` — font ~44, normal, as-is, white, no highlight, whole-cue, no box. Long-form default.
- `word_pop` — font ~64, bold, UPPER, white primary, **highlight `#FFE000` (yellow)**, active-word-pop, no box. The CapCut/Hormozi short-form look.
- `boxed` — font ~48, normal, as-is, white, no highlight, whole-cue, box `#000000` @ ~60% opacity. For busy backgrounds.

API: `resolve_preset(name: &str) -> Option<CaptionStyleSpec>`; `preset_names() -> &[&str]`. The
existing `resolve_style(format, mood)` maps to presets (mood `minimal_cinematic`→`clean_white`,
`active_pop`→`word_pop`) for back-compat. `CaptionStyleSpec` is `Serialize`/`Deserialize`
(→ the `style_json` blob).

### 3.2 EDL `style_json` plumbing

- `build_caption_edl_lines` emits `+ style_json: {<serialized CaptionStyleSpec>}` per caption,
  alongside the existing `font_size`/`color`/`position`/`safe_area` (kept for back-compat;
  when `style_json` is present it is authoritative).
- `EdlOp::InsertCaption` gains `style: Option<CaptionRenderStyle>` parsed from `style_json` via
  `take_field_json` (mirrors `word_timings_json`). `apply_insert_caption` forwards it to a new
  optional `TitlePlan.caption_style` field.
- `CaptionRenderStyle` (render-side mirror in `crates/render`) holds the same attributes;
  `TitlePlan.caption_style: Option<CaptionRenderStyle>`.

### 3.3 Render honors the style (`crates/render/src/ass.rs`)

- `push_styles`: when `caption_style` is present, set `Bold` from weight, `PrimaryColour` from
  primary_color, and the box via `BorderStyle=3` + `BackColour` (color+opacity) when
  `background = Box`; else keep the current outline style. Casing `Upper` uppercases the text
  before emission. (Italic/underline not in Phase 1.)
- Reveal:
  - `WholeCue` — one plain Dialogue (current behavior).
  - `WordByWord` — cumulative `\k` karaoke (current behavior).
  - **`ActiveWordPop`** — new: emit one Dialogue per word window; each shows the full cue text
    with the active word in `highlight_color` (inline `\c`) and the rest in `primary_color`,
    spanning that word's `[start,end]`. Only the current word pops; neighbors stay neutral.
- Legibility invariant: outline/shadow always drawn (unchanged).

### 3.4 Planner / skill pick the preset (`plan_captions`, scene_aware, SKILL.md)

- `plan_captions` gains an optional `preset` arg; default by format/mood (long-form→`clean_white`,
  `active_pop` mood→`word_pop`). Short-form `scene_aware` picks `word_pop` for energetic cuts,
  `clean_white` otherwise. The resolved spec is serialized into `style_json`.
- `caption-director` SKILL.md documents the preset library + when to use each, and that the agent
  proposes a preset which the user may override (the "agent sees + confirms" idea).

### 3.5 Vertical 9:16 output fix (`crates/render`)

Make the render canvas honor the timeline/EDL output format: a `9:16` (or `aspect_ratio` with
height>width) output yields a vertical canvas (e.g. 1080x1920) instead of a hardcoded/derived
1920x1080. Locate where the conform/scale stage chooses canvas WxH and read the output format.
Add a render test asserting a 9:16 output format produces vertical dimensions.

## 4. Data flow

`plan_captions`/`scene_aware` (pick preset → `CaptionStyleSpec`) → `+ style_json: {...}` in the
caption EDL → parser → `EdlOp::InsertCaption.style` → `apply` → `TitlePlan.caption_style` →
`ass.rs` (Style row + reveal emission incl. active-word-pop) → libass → render. Output canvas
shape from `Set Output Format`.

## 5. Error handling / contract

- No `style_json` (legacy caption) → fall back to `font_size`/`color` + whole/word reveal as today.
- Invalid/partial `style_json` → fall back to a safe default spec; never panic.
- Legibility floor enforced in `caption::styles` (min font, contrast color); outline a render invariant.
- `ActiveWordPop` with no word timings → degrade to `WholeCue`.

## 6. Testing

- **styles unit:** each preset resolves; legibility floor holds; serialize/deserialize round-trips;
  `word_pop` has a non-None highlight_color + ActiveWordPop reveal; `boxed` has a Box background.
- **EDL unit:** `build_caption_edl_lines` emits parseable `style_json`; parser → `InsertCaption.style`
  round-trips the attributes; `apply` sets `TitlePlan.caption_style`.
- **ass unit:** Bold/casing/box reflected in the Style row (BorderStyle=3 for box; uppercased text
  for Upper); `ActiveWordPop` emits one Dialogue per word with exactly one `\c`-highlighted word and
  N dialogues for N words; whole-cue/word-by-word paths unchanged.
- **render unit:** 9:16 output format → vertical canvas dimensions.
- **regression:** scoped `montage-core` + `montage-render` suites green (disk memory).
- **end-to-end (manual proof, sign-off):** (a) long-form Episode rendered with `clean_white` and
  `boxed`; (b) a **synthetic vertical clip** (crop/scale an uncaptioned Episode slice to 1080x1920)
  rendered short-form with `word_pop` — vertical canvas, UPPER bold captions, the active word
  popping yellow, no double-up. User reviews frames and signs off.

## 7. Scope boundaries (YAGNI)

**In:** style schema (weight, casing, primary/highlight color, reveal incl. active-word-pop,
background box); 3 presets (`clean_white`, `word_pop`, `boxed`); `style_json` EDL plumbing;
ASS honoring + active-word-pop emission; planner/skill preset selection; 9:16 vertical output fix;
synthetic vertical test clip + proof renders.

**Out (later phases):** italic/underline, character spacing, rotation, scale-per-caption, glow,
opacity animation, animated/preset *motion* templates, a manual style UI, data-file presets,
per-word manual styling. The two other filed render chips (drawtext chain, EDL text quote
round-trip) remain separate.

## 8. Risks / dependencies

- **Active-word-pop = N dialogues per cue** (one per word). Verify libass handles many short
  overlapping-in-time-but-sequential dialogues cleanly; keep the per-cue dialogue count bounded by
  the readability segmentation (already small cues).
- **`style_json` size** in the EDL — keep the serialized spec compact; it is one line per caption.
- **Vertical fix scope** — if the canvas-selection code is more tangled than expected, it may warrant
  its own task sequence; the plan isolates it as the first component so it can be validated alone.
- **Synthetic vertical clip** — center-crop may cut subjects; acceptable for a style proof (not a
  framing proof). Render to a clean (no-apostrophe) path per the path-escaping chip.
- **Disk**: scoped builds/tests only (`-p montage-core -p montage-render`, `CARGO_INCREMENTAL=0`).

## 9. Open items for review time

- Exact preset values (font sizes, the `word_pop` highlight hue, box opacity) — tune at render review.
- Whether `word_pop` should also bold the active word or only recolor it.
