# Caption Style Presets + Vertical Output — Implementation Plan (Captions Iteration 3, Phase 1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An agent-native caption **style preset** system (weight, casing, color, active-word highlight, background box) carried via an extensible `style_json` blob and rendered by libass, plus the **9:16 vertical-output fix**, proven by awidat-native renders.

**Architecture:** Core `caption::styles` grows a rich `CaptionStyleSpec` + a named preset registry; `plan_captions`/scene-aware pick a preset and serialize it into the caption EDL as `style_json`; the EDL parser carries it on `InsertCaption`; `apply` lowers it to a render-side `CaptionRenderStyle` on `TitlePlan`; `ass.rs` honors it (Style row + a new active-word-pop emission). Separately, fix the render so `Set Output Format 9:16` yields a vertical canvas.

**Tech Stack:** Rust (`awidat-core`, `awidat-render`), serde_json, ffmpeg/libass. Spec: `docs/superpowers/specs/2026-06-04-caption-style-presets-design.md`.

**Conventions (disk memory):** every cargo command prefixed `CARGO_INCREMENTAL=0`, scoped `-p awidat-core` / `-p awidat-render`; never `--workspace`. Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Render proofs from a clean (no-apostrophe) path.

---

## File structure
- Modify: `crates/render/src/timeline.rs` — vertical-output fix (canvas → output dims).
- Modify: `crates/core/src/caption/styles.rs` — rich `CaptionStyleSpec` + preset registry + `RevealMode::ActiveWordPop`.
- Modify: `crates/core/src/caption/readability.rs` — add `ActiveWordPop` to `RevealMode` (the enum lives here).
- Modify: `crates/core/src/caption/edl.rs` — emit `+ style_json:`.
- Modify: `crates/core/src/edl/op.rs`, `crates/core/src/edl/parser.rs`, `crates/core/src/edl/apply.rs` — carry `style_json` on `InsertCaption` → `TitlePlan.caption_style`.
- Modify: `crates/render/src/timeline.rs` (`TitlePlan`) — add `caption_style: Option<CaptionRenderStyle>`; define `CaptionRenderStyle`.
- Modify: `crates/render/src/ass.rs` — honor style + active-word-pop emission.
- Modify: `crates/core/src/awidat_mcp/tools/plan_captions.rs` — `preset` arg + default selection.
- Modify: `video_editing_transcripts/knowledge/captions/SKILL.md` — preset docs.

---

## Task 1: Vertical 9:16 output fix (test-first debugging)

**Files:** Modify `crates/render/src/timeline.rs`

Evidence (already gathered): a timeline whose `metadata.awidat` carries `output_format = {aspect_ratio:"9:16"}` (flattened into `extra`) still renders 1920×1080. `timeline_render_canvas` (line ~107) reads `extra["output_format"]["aspect_ratio"]` → `from_aspect_ratio` (1080×1920 for "9:16"), and `collect_timeline_full_plan` (~line 2027) returns that `canvas` into `build_timeline_render_spec_inner`. So the break is between loaded metadata and applied output dimensions.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `timeline.rs`:

```rust
#[test]
fn nine_sixteen_output_format_yields_vertical_canvas() {
    // A timeline carrying output_format 9:16 must conform to a vertical canvas.
    use awidat_proto::awidat_meta::AwidatTimelineMetadata;
    let mut md = AwidatTimelineMetadata::default();
    md.extra.insert(
        "output_format".into(),
        serde_json::json!({"aspect_ratio": "9:16", "platform": "vertical_short", "safe_area": "mobile"}),
    );
    let canvas = timeline_render_canvas(Some(&md));
    assert_eq!((canvas.width, canvas.height), (1080, 1920), "9:16 must be vertical");

    // And the round-trip through serde (flatten) must preserve it, since the CLI
    // path loads the timeline from disk before computing the canvas.
    let json = serde_json::to_string(&md).unwrap();
    let back: AwidatTimelineMetadata = serde_json::from_str(&json).unwrap();
    let canvas2 = timeline_render_canvas(Some(&back));
    assert_eq!((canvas2.width, canvas2.height), (1080, 1920), "9:16 must survive serde round-trip");
}
```

- [ ] **Step 2: Run — see which assertion fails**

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-render nine_sixteen_output_format -- --nocapture`
The in-memory assertion likely passes; the **serde round-trip** assertion is the suspect (flatten may not repopulate `extra["output_format"]` on deserialize, or `from_aspect_ratio` isn't reached). Whichever fails localizes the bug.

- [ ] **Step 3: Diagnose + fix (driven by the failing assertion)**

Use `superpowers:systematic-debugging`. Likely causes and the targeted fix:
- **If the round-trip assertion fails** (most likely): the flattened `output_format` key isn't landing back in `extra` on deserialize (e.g. another field consumes it, or `extra` isn't `#[serde(flatten)] HashMap`). Fix in `crates/proto/src/awidat_meta.rs` so unknown top-level awidat keys round-trip into `extra` (or add a typed `output_format` field read by `timeline_render_canvas`). If you add a typed field, update `timeline_render_canvas` to read it.
- **If both canvas assertions pass** but the real render is still 16:9: the canvas is correct but not applied to output dimensions — trace `build_timeline_render_spec_inner` → the conform/scale/pad + encoder output dims, and ensure they use `canvas.width`/`canvas.height` (not `TIMELINE_RENDER_WIDTH/HEIGHT`). Add an assertion in the test on the produced `RenderJobSpec`'s output dimensions if reachable.

Make the minimal change that turns both assertions green. Do NOT change the 16:9 default behavior.

- [ ] **Step 4: Run — verify PASS**

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-render nine_sixteen_output_format -- --nocapture` → PASS.
Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-render -- --nocapture` → existing render tests stay green (16:9 default unchanged).

- [ ] **Step 5: Clippy + commit**

`CARGO_INCREMENTAL=0 cargo clippy -p awidat-render --all-targets -- -D warnings 2>&1 | tail -5` → clean (if the proto crate changed, also `cargo clippy -p awidat-proto`).
```bash
git add -A
git commit -m "fix(render): honor Set Output Format 9:16 -> vertical canvas

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Step 6: Report the actual root cause** in the task report (for the controller).

---

## Task 2: Rich CaptionStyleSpec + preset registry

**Files:** Modify `crates/core/src/caption/styles.rs`, `crates/core/src/caption/readability.rs`

- [ ] **Step 1: Add `ActiveWordPop` to `RevealMode`**

In `crates/core/src/caption/readability.rs`, the `RevealMode` enum currently has `WholeCue, WordByWord`. Add a variant:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevealMode {
    WholeCue,
    WordByWord,
    ActiveWordPop,
}
```
Build: `CARGO_INCREMENTAL=0 cargo build -p awidat-core` — fix any non-exhaustive `match RevealMode` arms (e.g. in `caption::edl`/`plan_captions`) by treating `ActiveWordPop` like `WordByWord` for now (it also needs word timings); the render handles the visual difference. Re-run `cargo test -p awidat-core caption::` to confirm green.

- [ ] **Step 2: Write the failing tests for the rich spec + presets**

Replace the `styles.rs` tests module additions (keep existing tests) with these new tests:
```rust
    #[test]
    fn presets_resolve_with_expected_shapes() {
        let clean = resolve_preset("clean_white").expect("clean_white");
        assert!(matches!(clean.reveal, crate::caption::readability::RevealMode::WholeCue));
        assert!(clean.highlight_color.is_none());
        assert!(matches!(clean.background, CaptionBackground::None));

        let pop = resolve_preset("word_pop").expect("word_pop");
        assert!(matches!(pop.reveal, crate::caption::readability::RevealMode::ActiveWordPop));
        assert_eq!(pop.highlight_color.as_deref(), Some("#FFE000"));
        assert!(matches!(pop.weight, CaptionWeight::Bold));
        assert!(matches!(pop.casing, CaptionCasing::Upper));

        let boxed = resolve_preset("boxed").expect("boxed");
        assert!(matches!(boxed.background, CaptionBackground::Box { .. }));

        assert!(resolve_preset("nope").is_none());
    }

    #[test]
    fn every_preset_meets_legibility_floor_and_round_trips() {
        for name in preset_names() {
            let spec = resolve_preset(name).unwrap();
            assert!(spec.font_size >= MIN_LEGIBLE_FONT_SIZE);
            assert!(spec.primary_color.starts_with('#') && spec.primary_color.len() == 7);
            let json = serde_json::to_string(&spec).unwrap();
            let back: CaptionStyleSpec = serde_json::from_str(&json).unwrap();
            assert_eq!(spec, back);
        }
    }
```

- [ ] **Step 3: Run — verify FAIL** (`CaptionWeight`/`CaptionCasing`/`CaptionBackground`/`resolve_preset`/`preset_names` not found, spec fields missing).

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-core caption::styles -- --nocapture`

- [ ] **Step 4: Implement the rich spec + registry**

Replace the `CaptionStyleSpec` and add the new types + registry in `styles.rs`:
```rust
use serde::{Deserialize, Serialize};
use crate::caption::readability::RevealMode;

pub const MIN_LEGIBLE_FONT_SIZE: u32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionWeight { Normal, Bold }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionCasing { AsIs, Upper }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptionBackground {
    None,
    Box { color: String, opacity: u8 }, // opacity 0..=255 (255 = opaque)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionStyleSpec {
    pub font_size: u32,
    pub weight: CaptionWeight,
    pub casing: CaptionCasing,
    pub primary_color: String,        // "#RRGGBB"
    pub highlight_color: Option<String>, // active-word color; None = no pop
    pub reveal: RevealMode,
    pub background: CaptionBackground,
}

impl CaptionStyleSpec {
    fn with_floor(mut self) -> Self {
        if self.font_size < MIN_LEGIBLE_FONT_SIZE { self.font_size = MIN_LEGIBLE_FONT_SIZE; }
        if !(self.primary_color.starts_with('#') && self.primary_color.len() == 7) {
            self.primary_color = "#FFFFFF".into();
        }
        self
    }
}

/// Phase-1 preset library. Names are the agent's vocabulary.
pub fn preset_names() -> &'static [&'static str] {
    &["clean_white", "word_pop", "boxed"]
}

pub fn resolve_preset(name: &str) -> Option<CaptionStyleSpec> {
    let spec = match name {
        "clean_white" => CaptionStyleSpec {
            font_size: 44, weight: CaptionWeight::Normal, casing: CaptionCasing::AsIs,
            primary_color: "#FFFFFF".into(), highlight_color: None,
            reveal: RevealMode::WholeCue, background: CaptionBackground::None,
        },
        "word_pop" => CaptionStyleSpec {
            font_size: 64, weight: CaptionWeight::Bold, casing: CaptionCasing::Upper,
            primary_color: "#FFFFFF".into(), highlight_color: Some("#FFE000".into()),
            reveal: RevealMode::ActiveWordPop, background: CaptionBackground::None,
        },
        "boxed" => CaptionStyleSpec {
            font_size: 48, weight: CaptionWeight::Normal, casing: CaptionCasing::AsIs,
            primary_color: "#FFFFFF".into(), highlight_color: None,
            reveal: RevealMode::WholeCue,
            background: CaptionBackground::Box { color: "#000000".into(), opacity: 153 }, // ~60%
        },
        _ => return None,
    };
    Some(spec.with_floor())
}
```

Update the existing `resolve_style(format, mood)` to delegate to presets: `minimal_cinematic`→`clean_white`, `active_pop`→`word_pop`; keep its signature/return type (`CaptionStyleSpec`) so callers compile. Remove the old thin `CaptionStyleSpec` fields usage; update any caller that read `.color`/`.reveal` only (e.g. `caption::edl`, `plan_captions`) — `.color` becomes `.primary_color`. Build + fix call sites.

- [ ] **Step 5: Run — verify PASS** + caption suite green.

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-core caption:: -- --nocapture`

- [ ] **Step 6: Clippy + commit**

```bash
git add crates/core/src/caption/styles.rs crates/core/src/caption/readability.rs
git commit -m "feat(caption): rich CaptionStyleSpec + preset registry (clean_white/word_pop/boxed)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

(NOTE: Step 4 touches `caption::edl` and `plan_captions` call sites that used `.color`/`.reveal`. Commit those small call-site fixes here too so the crate builds.)

---

## Task 3: Emit `style_json` in the caption EDL

**Files:** Modify `crates/core/src/caption/edl.rs`

- [ ] **Step 1: Failing test**

Add to `edl.rs` tests:
```rust
    #[test]
    fn emits_style_json_with_spec_fields() {
        use crate::caption::styles::resolve_preset;
        let spec = resolve_preset("word_pop").unwrap();
        let blob = build_caption_edl_lines(&[rec()], &spec, "mobile").join("\n");
        assert!(blob.contains("+ style_json:"), "must emit style_json: {blob}");
        assert!(blob.contains("\"reveal\":\"active_word_pop\""));
        assert!(blob.contains("\"highlight_color\":\"#FFE000\""));
    }
```

- [ ] **Step 2: Run — FAIL.** `CARGO_INCREMENTAL=0 cargo test -p awidat-core caption::edl -- --nocapture`

- [ ] **Step 3: Implement**

`build_caption_edl_lines` already takes `spec: &CaptionStyleSpec`. After the existing `+ font_size:`/`+ color:` lines (keep them, sourced from `spec.font_size`/`spec.primary_color` for back-compat), add a serialized style line per caption:
```rust
        let style_json = serde_json::to_string(spec).unwrap_or_else(|_| "{}".into());
        lines.push(format!("+ style_json: {style_json}"));
```
(Place it before the `word_timings_json` line. Note `+ color:` now reads `spec.primary_color`.) Reveal gating for emitting `word_timings_json` should include `ActiveWordPop` (it needs word timings): `matches!(spec.reveal, RevealMode::WordByWord | RevealMode::ActiveWordPop)`.

- [ ] **Step 4: Run — PASS** + caption suite green.

- [ ] **Step 5: Commit**
```bash
git add crates/core/src/caption/edl.rs
git commit -m "feat(caption): emit style_json in the Insert Caption EDL

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Parse `style_json` onto `EdlOp::InsertCaption`

**Files:** Modify `crates/core/src/edl/op.rs`, `crates/core/src/edl/parser.rs`

- [ ] **Step 1: Add the field to the op**

In `crates/core/src/edl/op.rs`, `EdlOp::InsertCaption { ... }` add:
```rust
        /// Optional resolved caption style (from `style_json`); None for legacy captions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style_json: Option<serde_json::Value>,
```

- [ ] **Step 2: Failing parser test**

In `parser.rs` tests (near the existing InsertCaption parse test), add:
```rust
    #[test]
    fn parses_caption_style_json() {
        let edl = "*** Begin EDL\n*** Insert Caption\n+ start_s: 1.0\n+ end_s: 2.0\n+ text: \"hi\"\n+ position: bottom\n+ font_size: 64\n+ color: #FFFFFF\n+ safe_area: mobile\n+ style_json: {\"reveal\":\"active_word_pop\",\"highlight_color\":\"#FFE000\"}\n*** End EDL\n";
        let env = parse_edl(edl).expect("parse");
        match env.ops.iter().find(|o| matches!(o, EdlOp::InsertCaption{..})).unwrap() {
            EdlOp::InsertCaption { style_json, .. } => {
                let v = style_json.as_ref().expect("style_json present");
                assert_eq!(v["reveal"], "active_word_pop");
            }
            _ => panic!("want InsertCaption"),
        }
    }
```
(Adjust `parse_edl`/`EdlEnvelope` names to the actual parser API used by existing parser tests in this file.)

- [ ] **Step 3: Run — FAIL.** `CARGO_INCREMENTAL=0 cargo test -p awidat-core parses_caption_style_json -- --nocapture`

- [ ] **Step 4: Implement parsing**

In `parser.rs`, the `OpKind::InsertCaption => { ... Ok(EdlOp::InsertCaption { ... }) }` block (~line 1279–1320): read the optional field with the existing JSON helper used for `word_timings_json` (`take_field_json::<serde_json::Value>` / the optional variant) keyed `style_json`, and pass it into the constructed `EdlOp::InsertCaption { ..., style_json }`. Mirror exactly how `word_timings_json` is read (optional, default None).

- [ ] **Step 5: Run — PASS** + edl parser suite green (`cargo test -p awidat-core edl::`).

- [ ] **Step 6: Commit**
```bash
git add crates/core/src/edl/op.rs crates/core/src/edl/parser.rs
git commit -m "feat(caption): carry style_json on EdlOp::InsertCaption

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Lower `style_json` to `TitlePlan.caption_style`

**Files:** Modify `crates/render/src/timeline.rs` (TitlePlan + `CaptionRenderStyle`), `crates/core/src/edl/apply.rs`

- [ ] **Step 1: Define `CaptionRenderStyle` + add to `TitlePlan`**

In `crates/render/src/timeline.rs`, add a render-side style mirror (same field names as core `CaptionStyleSpec` so serde round-trips from the same JSON):
```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionRenderWeight { Normal, Bold }
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionRenderCasing { AsIs, Upper }
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptionRenderBackground { None, Box { color: String, opacity: u8 } }
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionRenderReveal { WholeCue, WordByWord, ActiveWordPop }
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CaptionRenderStyle {
    pub font_size: u32,
    pub weight: CaptionRenderWeight,
    pub casing: CaptionRenderCasing,
    pub primary_color: String,
    pub highlight_color: Option<String>,
    pub reveal: CaptionRenderReveal,
    pub background: CaptionRenderBackground,
}
```
Add to `TitlePlan`: `pub caption_style: Option<CaptionRenderStyle>,` (and to every `TitlePlan { .. }` literal in render tests/builders — search and add `caption_style: None`).

- [ ] **Step 2: Failing apply test**

In `apply.rs` tests, extend the Insert Caption apply test (or add one): apply an EDL with `+ style_json: {...word_pop...}` and assert the resulting Titles-track caption clip carries the style (e.g. effect metadata `caption_style` present, or the TitlePlan built downstream has it). Match the assertion style of the existing caption-apply test. (If apply stores captions as clip effects rather than TitlePlan directly, assert the effect metadata includes the style_json.)

- [ ] **Step 3: Run — FAIL.**

- [ ] **Step 4: Implement**

In `apply.rs`, `apply_insert_caption` (and its `EdlOp::InsertCaption` match arm at ~line 780) now receives `style_json`. Store it on the caption node's effect metadata (key `caption_style`) alongside the existing caption fields, so the render's caption→TitlePlan construction can read it into `TitlePlan.caption_style` (deserialize the JSON into `CaptionRenderStyle`; on error → None). Wire the render path that builds `TitlePlan` from caption clips to populate `caption_style` from that metadata.

- [ ] **Step 5: Run — PASS** + `cargo test -p awidat-core edl::` and `cargo test -p awidat-render` green.

- [ ] **Step 6: Commit**
```bash
git add crates/render/src/timeline.rs crates/core/src/edl/apply.rs
git commit -m "feat(caption): lower style_json to TitlePlan.caption_style

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: ASS honors weight / casing / background box

**Files:** Modify `crates/render/src/ass.rs`

- [ ] **Step 1: Failing tests**

Add to `ass.rs` tests (reuse the `caption_title` helper; set `.caption_style`):
```rust
    fn styled(spec: CaptionRenderStyle, text: &str, wt: Vec<crate::timeline::CaptionWordTiming>) -> crate::timeline::TitlePlan {
        let mut t = caption_title(text, wt);
        t.caption_style = Some(spec);
        t
    }

    #[test]
    fn upper_casing_uppercases_dialogue_text() {
        let spec = /* word_pop-like but WholeCue for this test */ CaptionRenderStyle {
            font_size: 64, weight: CaptionRenderWeight::Bold, casing: CaptionRenderCasing::Upper,
            primary_color: "#FFFFFF".into(), highlight_color: None,
            reveal: CaptionRenderReveal::WholeCue, background: CaptionRenderBackground::None,
        };
        let doc = build_ass_document(&styled(spec, "rise of solo", vec![]));
        assert!(doc.contains("RISE OF SOLO"));
    }

    #[test]
    fn boxed_background_uses_opaque_border_style() {
        let spec = CaptionRenderStyle {
            font_size: 48, weight: CaptionRenderWeight::Normal, casing: CaptionRenderCasing::AsIs,
            primary_color: "#FFFFFF".into(), highlight_color: None,
            reveal: CaptionRenderReveal::WholeCue,
            background: CaptionRenderBackground::Box { color: "#000000".into(), opacity: 153 },
        };
        let doc = build_ass_document(&styled(spec, "hi", vec![]));
        // BorderStyle field (17th in the Style row) must be 3 (opaque box) when boxed.
        let style_line = doc.lines().find(|l| l.starts_with("Style: Caption")).unwrap();
        let fields: Vec<&str> = style_line.trim_start_matches("Style: ").split(',').collect();
        assert_eq!(fields[15], "3", "BorderStyle must be 3 (box) for boxed background; got {style_line}");
    }
```
(Field index: the Format is `Name,Fontname,Fontsize,Primary,Secondary,Outline,Back,Bold,Italic,Underline,StrikeOut,ScaleX,ScaleY,Spacing,Angle,BorderStyle,...` → after `split(',')`, index 0 = "Caption", so BorderStyle is index 15. Verify against the emitted row and adjust the index if needed.)

- [ ] **Step 2: Run — FAIL.**

- [ ] **Step 3: Implement**

In `push_styles`, when `title.caption_style` is `Some(style)`: use `style.weight` for Bold, `style.primary_color` for PrimaryColour, and for `CaptionRenderBackground::Box{color,opacity}` set BorderStyle=3 and BackColour from color+opacity (ASS `&HAABBGGRR`, AA = 255-opacity since ASS alpha is inverted). When `None`, keep current behavior. Apply `casing == Upper` by uppercasing the text in the whole-cue and reveal emitters (transform `title.text` / each word's glyphs). Keep the outline/shadow invariant.

- [ ] **Step 4: Run — PASS** + ass suite green.

- [ ] **Step 5: Commit**
```bash
git add crates/render/src/ass.rs
git commit -m "feat(render): caption ASS honors weight, casing, and background box

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: Active-word-pop dialogue emission

**Files:** Modify `crates/render/src/ass.rs`

- [ ] **Step 1: Failing test**

```rust
    #[test]
    fn active_word_pop_emits_one_dialogue_per_word_with_one_highlight() {
        let spec = CaptionRenderStyle {
            font_size: 64, weight: CaptionRenderWeight::Bold, casing: CaptionRenderCasing::Upper,
            primary_color: "#FFFFFF".into(), highlight_color: Some("#FFE000".into()),
            reveal: CaptionRenderReveal::ActiveWordPop, background: CaptionRenderBackground::None,
        };
        let wt = vec![
            crate::timeline::CaptionWordTiming { text: "five".into(), start_s: 1.0, end_s: 1.4 },
            crate::timeline::CaptionWordTiming { text: "ten".into(), start_s: 1.4, end_s: 2.0 },
        ];
        let doc = build_ass_document(&styled(spec, "five ten", wt));
        let dialogues: Vec<&str> = doc.lines().filter(|l| l.starts_with("Dialogue:")).collect();
        assert_eq!(dialogues.len(), 2, "one dialogue per word");
        // first dialogue: 'five' highlighted (a \c color override appears exactly once)
        let highlights = dialogues[0].matches("\\c&").count();
        assert_eq!(highlights, 1, "exactly one highlighted word per dialogue");
        assert!(dialogues[0].contains("FIVE") && dialogues[0].contains("TEN"), "full line shown");
    }
```
(Adjust the `\c` count/marker to the exact inline override you emit; the contract is: full line shown, exactly one word recolored to highlight_color, N dialogues for N words, each spanning that word's window.)

- [ ] **Step 2: Run — FAIL.**

- [ ] **Step 3: Implement**

Add an active-word-pop branch in `build_dialogue_lines` (selected when `caption_style.reveal == ActiveWordPop` and word timings exist; otherwise fall back to the existing whole-cue/word-by-word logic). For each word *i*, emit one `Dialogue` spanning `[word[i].start, word[i].end]` showing the full (casing-applied) line where word *i* is wrapped in `{\c<highlight>}WORD{\c<primary>}` and all others use the primary color. Use `hex_to_ass_color` for the colors. If `highlight_color` is None, degrade to whole-cue. If no word timings, degrade to whole-cue.

- [ ] **Step 4: Run — PASS** + ass suite green.

- [ ] **Step 5: Commit**
```bash
git add crates/render/src/ass.rs
git commit -m "feat(render): active-word-pop caption reveal (current word highlighted)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 8: Planner/skill pick the preset

**Files:** Modify `crates/core/src/awidat_mcp/tools/plan_captions.rs`, `crates/core/src/scene_aware_short_form.rs`

- [ ] **Step 1: Failing test (plan_captions preset arg)**

In `plan_captions.rs` tests:
```rust
    #[test]
    fn explicit_preset_drives_style_json() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/ep.mp4";
        write_whisper(dir.path(), asset, serde_json::json!({
            "words": [{"text":"hi","start_s":0.0,"end_s":1.0}], "segments":[{"text":"hi","start_s":0.0,"end_s":1.0}]
        }));
        let ctx = McpToolCtx { project_root: dir.path().to_path_buf() };
        let out = run(PlanCaptionsArgs { asset_id: asset.into(), clip_id: "c".into(),
            format: "long_form".into(), mood: "minimal_cinematic".into(), preset: Some("word_pop".into()) }, ctx).unwrap();
        let body: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(body["edl_fragment"].as_str().unwrap().contains("\"reveal\":\"active_word_pop\""));
    }
```

- [ ] **Step 2: Run — FAIL** (no `preset` field on args).

- [ ] **Step 3: Implement**

Add `pub preset: Option<String>` to `PlanCaptionsArgs` (`#[serde(default)]`). In `run`, resolve the spec: if `preset` is Some and `resolve_preset` returns Some, use it; else fall back to the existing `resolve_style(format, mood)` default (which now maps to presets). Pass the resolved spec to `build_caption_edl_lines`. Update the `DESCRIPTION` + the MCP `#[tool]` arg doc in `awidat_mcp/mod.rs` to mention `preset` (optional; `preset_names()` values).

For `scene_aware_short_form` `build_edl_fragment`: choose the preset for short-form captions — `word_pop` (energetic short-form) by default; pass its spec to `build_caption_edl_lines` (replacing the hardcoded `CaptionStyleSpec { font_size: 56, ... }`). Keep a brief comment.

- [ ] **Step 4: Run — PASS** + `cargo test -p awidat-core plan_captions scene_aware_short_form` green (update any test asserting the old hardcoded font/white spec).

- [ ] **Step 5: Commit**
```bash
git add crates/core/src/awidat_mcp/tools/plan_captions.rs crates/core/src/awidat_mcp/mod.rs crates/core/src/scene_aware_short_form.rs
git commit -m "feat(caption): preset selection in plan_captions + short-form word_pop default

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 9: Skill docs

**Files:** Modify `video_editing_transcripts/knowledge/captions/SKILL.md`

- [ ] **Step 1: Document the preset library** — add a short "Caption style presets" section listing `clean_white` (long-form clean), `word_pop` (short-form active-word pop, UPPER bold, yellow highlight), `boxed` (busy backgrounds), and that the agent proposes a preset which the user may override; note `plan_captions` accepts `preset`.
- [ ] **Step 2: Commit** (`video_editing_transcripts` is gitignored; this is a working-tree change — no commit needed, consistent with how that tree is managed).

---

## Task 10: Scoped gate

**Files:** none.

- [ ] `cargo fmt --all` then `cargo fmt --all -- --check` (nightly-config warnings OK).
- [ ] `CARGO_INCREMENTAL=0 cargo clippy -p awidat-core -p awidat-render --all-targets -- -D warnings 2>&1 | tail -6` → clean (add `-p awidat-proto` if it changed in Task 1).
- [ ] `CARGO_INCREMENTAL=0 cargo test -p awidat-core -p awidat-render 2>&1 | grep -E "test result:|FAILED" | tail` → 0 failed.
- [ ] Commit any fmt fixups.

---

## Task 11: Proof renders (manual, user sign-off)

**Files:** none. Render from a clean (no-apostrophe) path. No `ANTHROPIC_API_KEY` needed.

- [ ] **Step 1: Build CLI** — `CARGO_INCREMENTAL=0 cargo build -p awidat-cli --bin awidat`.
- [ ] **Step 2: Make a synthetic raw vertical clip** from an uncaptioned Episode slice:
  `ffmpeg -y -i "/Volumes/Explicit's Hard Drive/capproof_src/ep1_60s.mp4" -t 20 -vf "crop=ih*9/16:ih,scale=1080:1920" -c:v libx264 -preset veryfast -crf 20 -c:a aac /Users/explicit/vshort_src.mp4` (center-crop to 9:16, 20s).
- [ ] **Step 3: Long-form preset renders** (clean path project, reuse the `clean_white` and `boxed` presets via `plan_captions preset=...`): index whisper (out-of-band per the warm env), generate EDL for each preset via a throwaway `crates/core/examples/*.rs` harness (DELETE before finishing), `apply-edl` + `render`. Confirm clean_white and boxed look right.
- [ ] **Step 4: Vertical word_pop render** — new clean-path project from `vshort_src.mp4`; transcribe out-of-band; `plan_scene_aware_short_form` (now emits `word_pop`) → `apply-edl` → `render`. Verify: **vertical 1080×1920** output (Task 1 fix), UPPER bold captions, the active word popping `#FFE000`, no double-up (source is uncaptioned). `ffprobe` the output dims.
- [ ] **Step 5: Inspect + present** representative frames (a word-pop moment showing exactly one yellow word; a boxed-caption frame; a clean_white frame). Delete any throwaway harness.
- [ ] **Step 6: Capture verdict.** Done only on user sign-off.

---

## Self-review

**Spec coverage:** §3.1 rich spec+presets → Task 2; §3.2 style_json plumbing → Tasks 3–5; §3.3 ASS honoring + active-word-pop → Tasks 6–7; §3.4 planner/skill preset → Tasks 8–9; §3.5 vertical fix → Task 1; §6 testing → per-task + Task 10; proof → Task 11. ✅

**Placeholder scan:** No "TBD/handle errors". Task 1 is deliberately test-first/diagnostic (the root cause is genuinely unknown after static tracing) with concrete candidate fixes + a failing test to localize it — that's the honest structure, not a placeholder. Verify-against-codebase notes (parser API names, BorderStyle field index, how apply stores captions) carry explicit "match the existing pattern / adjust index" instructions.

**Type consistency:** `RevealMode::{WholeCue,WordByWord,ActiveWordPop}` (core) mirrored by `CaptionRenderReveal` (render); `CaptionStyleSpec`{font_size,weight,casing,primary_color,highlight_color,reveal,background} (core) mirrored field-for-field by `CaptionRenderStyle` (render) so the same `style_json` round-trips. `CaptionWeight/CaptionCasing/CaptionBackground` (core) ↔ `CaptionRenderWeight/Casing/Background` (render). `resolve_preset`/`preset_names` defined Task 2, used Tasks 3/8/11. `style_json` field on `InsertCaption` defined Task 4, consumed Task 5. `build_caption_edl_lines(&[rec], &spec, safe_area)` signature unchanged (spec now richer). Consistent.
