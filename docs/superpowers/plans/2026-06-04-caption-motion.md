# Animated Caption Motion Implementation Plan (Captions Phase 2.1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Composable caption motion (entrance / active-word / exit / continuous) lowered to ASS animation tags, with corpus-tuned restrained presets.

**Architecture:** A `CaptionMotion` struct (4 slots) on the core `CaptionStyleSpec`, mirrored field-for-field by a render `CaptionRenderMotion` on `CaptionRenderStyle`, so it rides the existing `style_json` blob (no EDL/parser/apply changes). `ass.rs` gains a composer that prepends a `{\...}` override block (entrance/exit/continuous) per Dialogue line and adds active-word scale tags inside the active-word-pop emitter, honoring the N-dialogue composition rule (entrance on the first word's line, exit on the last).

**Tech Stack:** Rust (`awidat-core`, `awidat-render`), serde, ASS/libass. Spec: `docs/superpowers/specs/2026-06-04-caption-motion-design.md`.

**Conventions (memory):** every cargo command prefixed `CARGO_INCREMENTAL=0`, scoped `-p awidat-core` / `-p awidat-render`; never `--workspace`. Workspace **denies `clippy::unwrap_used`/`expect_used` in non-test code** — use `match`/`unwrap_or`/`?`. Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Render proofs from a clean (no-apostrophe) path.

---

## File structure
- Modify: `crates/core/src/caption/styles.rs` — `CaptionMotion` + 4 enums; `motion` field on `CaptionStyleSpec`; preset motions + `emphasis` preset.
- Modify: `crates/render/src/timeline.rs` — `CaptionRenderMotion` + 4 enums; `motion` field on `CaptionRenderStyle`.
- Modify: `crates/render/src/ass.rs` — motion→ASS composer; integrate into `build_dialogue_lines` (all three branches) + active-word-pop.
- Modify: `video_editing_transcripts/knowledge/captions/SKILL.md` — motion vocabulary + restraint rule.

**Why `style_json` auto-carries motion:** `build_caption_edl_lines` serializes the whole `CaptionStyleSpec` to `style_json`; `apply` stores it; `parse_title_plan` does `serde_json::from_value::<CaptionRenderStyle>`. Adding `motion` (with `#[serde(default)]`) to BOTH structs makes it flow end-to-end with no grammar change — and old (Phase-1) `style_json` without `motion` deserializes to the all-`None` default.

---

## Task 1: Core CaptionMotion model + preset motions

**Files:** Modify `crates/core/src/caption/styles.rs`

- [ ] **Step 1: Write failing tests**

Add to the `styles.rs` tests module:
```rust
    #[test]
    fn caption_motion_defaults_to_none_and_round_trips() {
        let m = CaptionMotion::default();
        assert!(matches!(m.entrance, EntranceMotion::None));
        assert!(matches!(m.active_word, ActiveWordMotion::None));
        assert!(matches!(m.exit, ExitMotion::None));
        assert!(matches!(m.continuous, ContinuousMotion::None));
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<CaptionMotion>(&json).unwrap(), m);
    }

    #[test]
    fn preset_motions_match_corpus_defaults() {
        let clean = resolve_preset("clean_white").unwrap();
        assert_eq!(clean.motion, CaptionMotion::default(), "cinematic = minimal/none");
        let boxed = resolve_preset("boxed").unwrap();
        assert_eq!(boxed.motion, CaptionMotion::default());
        let pop = resolve_preset("word_pop").unwrap();
        assert!(matches!(pop.motion.entrance, EntranceMotion::PopIn));
        assert!(matches!(pop.motion.active_word, ActiveWordMotion::Bounce));
    }

    #[test]
    fn style_json_without_motion_deserializes_to_default() {
        // Phase-1 style_json (no `motion` key) must still parse.
        let legacy = r#"{"font_size":44,"weight":"normal","casing":"as_is","primary_color":"#FFFFFF","highlight_color":null,"reveal":"whole_cue","background":{"kind":"none"}}"#;
        let spec: CaptionStyleSpec = serde_json::from_str(legacy).unwrap();
        assert_eq!(spec.motion, CaptionMotion::default());
    }
```

- [ ] **Step 2: Run — FAIL** (`cargo test -p awidat-core caption::styles -- --nocapture`): `CaptionMotion`/enums/`motion` field absent.

- [ ] **Step 3: Implement the model**

Add to `styles.rs`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntranceMotion { None, PopIn, SlideUp, FadeIn }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveWordMotion { None, Bounce, ScalePop, Shake }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitMotion { None, PopOut, FadeOut, SlideDown }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuousMotion { None, Float }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptionMotion {
    pub entrance: EntranceMotion,
    pub active_word: ActiveWordMotion,
    pub exit: ExitMotion,
    pub continuous: ContinuousMotion,
}

impl Default for CaptionMotion {
    fn default() -> Self {
        Self {
            entrance: EntranceMotion::None,
            active_word: ActiveWordMotion::None,
            exit: ExitMotion::None,
            continuous: ContinuousMotion::None,
        }
    }
}
```

Add to `CaptionStyleSpec` (and to its with_floor — motion needs no floor):
```rust
    #[serde(default)]
    pub motion: CaptionMotion,
```

Update `resolve_preset`: every existing preset literal gains a `motion:` field.
- `clean_white` → `motion: CaptionMotion::default()`
- `boxed` → `motion: CaptionMotion::default()`
- `word_pop` → `motion: CaptionMotion { entrance: EntranceMotion::PopIn, active_word: ActiveWordMotion::Bounce, exit: ExitMotion::None, continuous: ContinuousMotion::None }`

- [ ] **Step 4: Fix call sites** — any `CaptionStyleSpec { .. }` literal in tests (e.g. in `caption::edl` tests, `styles` tests) needs `motion: CaptionMotion::default()`. Build `CARGO_INCREMENTAL=0 cargo build -p awidat-core` and add the field where it errors.

- [ ] **Step 5: Run — PASS** `CARGO_INCREMENTAL=0 cargo test -p awidat-core caption:: -- --nocapture` (all caption tests green).

- [ ] **Step 6: Clippy + commit**

`CARGO_INCREMENTAL=0 cargo clippy -p awidat-core --all-targets -- -D warnings 2>&1 | tail -5` → clean.
```bash
git add crates/core/src/caption/styles.rs crates/core/src/caption/edl.rs
git commit -m "feat(caption): add composable CaptionMotion model + preset motions

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Render CaptionRenderMotion mirror

**Files:** Modify `crates/render/src/timeline.rs`

- [ ] **Step 1: Write failing test**

Add to `timeline.rs` tests (near the `caption_render_style_deserializes_from_json` test from Phase 1):
```rust
    #[test]
    fn caption_render_style_carries_motion_from_json() {
        let json = r#"{"font_size":64,"weight":"bold","casing":"upper","primary_color":"#FFFFFF","highlight_color":"#FFE000","reveal":"active_word_pop","background":{"kind":"none"},"motion":{"entrance":"pop_in","active_word":"bounce","exit":"none","continuous":"none"}}"#;
        let s: CaptionRenderStyle = serde_json::from_str(json).unwrap();
        assert!(matches!(s.motion.entrance, CaptionRenderEntrance::PopIn));
        assert!(matches!(s.motion.active_word, CaptionRenderActiveWord::Bounce));
    }

    #[test]
    fn caption_render_style_motion_defaults_when_absent() {
        // Phase-1 JSON (no motion) → default motion.
        let json = r#"{"font_size":44,"weight":"normal","casing":"as_is","primary_color":"#FFFFFF","highlight_color":null,"reveal":"whole_cue","background":{"kind":"none"}}"#;
        let s: CaptionRenderStyle = serde_json::from_str(json).unwrap();
        assert!(matches!(s.motion.entrance, CaptionRenderEntrance::None));
    }
```

- [ ] **Step 2: Run — FAIL** `CARGO_INCREMENTAL=0 cargo test -p awidat-render caption_render_style_carries_motion -- --nocapture`.

- [ ] **Step 3: Implement the render mirror** (field/tag parity with core)

Add to `timeline.rs`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionRenderEntrance { None, PopIn, SlideUp, FadeIn }
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionRenderActiveWord { None, Bounce, ScalePop, Shake }
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionRenderExit { None, PopOut, FadeOut, SlideDown }
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionRenderContinuous { None, Float }
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CaptionRenderMotion {
    pub entrance: CaptionRenderEntrance,
    pub active_word: CaptionRenderActiveWord,
    pub exit: CaptionRenderExit,
    pub continuous: CaptionRenderContinuous,
}
impl Default for CaptionRenderMotion {
    fn default() -> Self {
        Self {
            entrance: CaptionRenderEntrance::None,
            active_word: CaptionRenderActiveWord::None,
            exit: CaptionRenderExit::None,
            continuous: CaptionRenderContinuous::None,
        }
    }
}
```
Add to `CaptionRenderStyle`:
```rust
    #[serde(default)]
    pub motion: CaptionRenderMotion,
```

- [ ] **Step 4: Run — PASS** + `CARGO_INCREMENTAL=0 cargo test -p awidat-render -- --nocapture` (existing render tests green; any `CaptionRenderStyle { .. }` test literal needs `motion: CaptionRenderMotion::default()` — add where it errors).

- [ ] **Step 5: Clippy + commit**
```bash
git add crates/render/src/timeline.rs
git commit -m "feat(render): CaptionRenderStyle carries the motion mirror

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: ASS composer — entrance/exit (scale + fade), whole-cue & karaoke lines

**Files:** Modify `crates/render/src/ass.rs`

Implements the alignment-compatible motions (PopIn, FadeIn entrance; PopOut, FadeOut exit) on the single-line branches. Active-word-pop integration is Task 4; slide/float is Task 5.

- [ ] **Step 1: Write failing tests**

```rust
    fn motion_style(entrance: crate::timeline::CaptionRenderEntrance, exit: crate::timeline::CaptionRenderExit) -> crate::timeline::CaptionRenderStyle {
        crate::timeline::CaptionRenderStyle {
            font_size: 44,
            weight: crate::timeline::CaptionRenderWeight::Normal,
            casing: crate::timeline::CaptionRenderCasing::AsIs,
            primary_color: "#FFFFFF".into(),
            highlight_color: None,
            reveal: crate::timeline::CaptionRenderReveal::WholeCue,
            background: crate::timeline::CaptionRenderBackground::None,
            motion: crate::timeline::CaptionRenderMotion {
                entrance, exit,
                active_word: crate::timeline::CaptionRenderActiveWord::None,
                continuous: crate::timeline::CaptionRenderContinuous::None,
            },
        }
    }

    #[test]
    fn pop_in_entrance_emits_scale_animation_on_whole_cue() {
        use crate::timeline::*;
        let mut t = caption_title("hello", vec![]);
        t.caption_style = Some(motion_style(CaptionRenderEntrance::PopIn, CaptionRenderExit::None));
        let doc = build_ass_document(&t);
        let d = doc.lines().find(|l| l.starts_with("Dialogue:")).unwrap();
        assert!(d.contains("\\fscx") && d.contains("\\t("), "PopIn must emit a scale \\t: {d}");
    }

    #[test]
    fn fade_in_out_emits_fad_on_whole_cue() {
        use crate::timeline::*;
        let mut t = caption_title("hello", vec![]);
        t.caption_style = Some(motion_style(CaptionRenderEntrance::FadeIn, CaptionRenderExit::FadeOut));
        let d = build_ass_document(&t).lines().find(|l| l.starts_with("Dialogue:")).unwrap().to_string();
        assert!(d.contains("\\fad("), "fade must emit \\fad: {d}");
    }

    #[test]
    fn no_motion_whole_cue_is_unchanged() {
        let t = caption_title("hello world", vec![]); // caption_style None
        let doc = build_ass_document(&t);
        let d = doc.lines().find(|l| l.starts_with("Dialogue:")).unwrap();
        assert!(!d.contains("\\t(") && !d.contains("\\fad("), "no motion -> no animation tags: {d}");
    }
```

- [ ] **Step 2: Run — FAIL.**

- [ ] **Step 3: Implement the composer + integrate**

Add a helper that builds the leading override block for a cue line, given the motion, the line's role, and the line duration in centiseconds:
```rust
/// Which line of a cue this Dialogue is, for placing entrance/exit tags.
#[derive(Clone, Copy, PartialEq)]
enum CueLineRole { Sole, First, Middle, Last }

/// Build the leading ASS override block for entrance/exit/continuous motion on
/// one Dialogue line. `dur_cs` is the line's [start,end] span in centiseconds.
/// Entrance applies on Sole|First; exit on Sole|Last; continuous on all. Scale
/// and fade only here (alignment-compatible); slide/float handled separately.
fn motion_override(motion: &crate::timeline::CaptionRenderMotion, role: CueLineRole, dur_cs: i64) -> String {
    use crate::timeline::{CaptionRenderEntrance as E, CaptionRenderExit as X};
    let dur_ms = (dur_cs * 10).max(1);
    let mut init = String::new();   // initial-state tags (e.g. \fscx80)
    let mut anim = String::new();   // \t / \fad tags
    let mut fad_in = 0i64;
    let mut fad_out = 0i64;
    let entrance_here = matches!(role, CueLineRole::Sole | CueLineRole::First);
    let exit_here = matches!(role, CueLineRole::Sole | CueLineRole::Last);
    if entrance_here {
        match motion.entrance {
            E::PopIn => { init.push_str("\\fscx80\\fscy80"); anim.push_str("\\t(0,120,\\fscx103\\fscy103)\\t(120,170,\\fscx100\\fscy100)"); }
            E::FadeIn => { fad_in = 150; }
            E::SlideUp | E::None => {} // SlideUp handled in Task 5
        }
    }
    if exit_here {
        match motion.exit {
            X::PopOut => { let s = (dur_ms - 150).max(0); anim.push_str(&format!("\\t({s},{dur_ms},\\fscx80\\fscy80)")); }
            X::FadeOut => { fad_out = 150; }
            X::SlideDown | X::None => {} // SlideDown handled in Task 5
        }
    }
    if fad_in > 0 || fad_out > 0 {
        anim.push_str(&format!("\\fad({fad_in},{fad_out})"));
    }
    if init.is_empty() && anim.is_empty() {
        return String::new();
    }
    format!("{{{init}{anim}}}")
}
```

Integrate: in `build_dialogue_lines`, compute `let motion = title.caption_style.as_ref().map(|s| &s.motion);` and prepend `motion_override(...)` to each single-line branch's text:
- Whole-cue branch: `role = Sole`, `dur_cs = seconds_to_centiseconds(title.end_s - title.start_s)`; prepend the override to `wrapped`.
- Karaoke (word-by-word) branch: `role = Sole`, same `dur_cs`; prepend the override to `text` BEFORE the `\k` run.
When `motion` is `None`, `motion_override` returns `""` (no change → byte-identical).

(Active-word-pop branch handled in Task 4; do not touch it here beyond leaving it working.)

- [ ] **Step 4: Run — PASS + ass suite green.**

- [ ] **Step 5: Clippy + commit**
```bash
git add crates/render/src/ass.rs
git commit -m "feat(render): caption entrance/exit motion (pop/fade) on whole-cue + karaoke

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Active-word motion + active-word-pop composition

**Files:** Modify `crates/render/src/ass.rs`

- [ ] **Step 1: Write failing tests**

```rust
    fn pop_style_with(active: crate::timeline::CaptionRenderActiveWord, entrance: crate::timeline::CaptionRenderEntrance, exit: crate::timeline::CaptionRenderExit) -> crate::timeline::CaptionRenderStyle {
        let mut s = motion_style(entrance, exit); // from Task 3 helper
        s.reveal = crate::timeline::CaptionRenderReveal::ActiveWordPop;
        s.highlight_color = Some("#FFE000".into());
        s.motion.active_word = active;
        s
    }

    #[test]
    fn active_word_bounce_springs_only_the_active_word() {
        use crate::timeline::*;
        let wt = vec![
            CaptionWordTiming { text: "a".into(), start_s: 1.0, end_s: 1.3 },
            CaptionWordTiming { text: "b".into(), start_s: 1.3, end_s: 1.7 },
        ];
        let mut t = caption_title("a b", wt);
        t.caption_style = Some(pop_style_with(CaptionRenderActiveWord::Bounce, CaptionRenderEntrance::None, CaptionRenderExit::None));
        let dialogues: Vec<String> = build_ass_document(&t).lines().filter(|l| l.starts_with("Dialogue:")).map(String::from).collect();
        assert_eq!(dialogues.len(), 2);
        // each dialogue: exactly one bounce \t scale (on the active word)
        assert_eq!(dialogues[0].matches("\\fscx115").count(), 1, "one bounce per line: {}", dialogues[0]);
    }

    #[test]
    fn pop_entrance_only_on_first_word_exit_only_on_last() {
        use crate::timeline::*;
        let wt = vec![
            CaptionWordTiming { text: "a".into(), start_s: 1.0, end_s: 1.3 },
            CaptionWordTiming { text: "b".into(), start_s: 1.3, end_s: 1.7 },
        ];
        let mut t = caption_title("a b", wt);
        t.caption_style = Some(pop_style_with(CaptionRenderActiveWord::None, CaptionRenderEntrance::PopIn, CaptionRenderExit::FadeOut));
        let d: Vec<String> = build_ass_document(&t).lines().filter(|l| l.starts_with("Dialogue:")).map(String::from).collect();
        assert!(d[0].contains("\\fscx80"), "entrance on first word's line");
        assert!(!d[1].contains("\\fscx80"), "no entrance on second word's line");
        assert!(d[1].contains("\\fad(0,150)") || d[1].contains("\\fad(0"), "exit fade on last word's line");
    }
```

- [ ] **Step 2: Run — FAIL.**

- [ ] **Step 3: Implement**

In the active-word-pop branch of `build_dialogue_lines`:
1. Prepend the cue-level motion override per line using `motion_override(motion, role, word_dur_cs)` where `role` = `First` for `active_idx==0`, `Last` for `active_idx==words.len()-1`, else `Middle` (Sole if a single word). `word_dur_cs = seconds_to_centiseconds(word.end_s - word.start_s)`.
2. For the active word's inline wrapper, add the active-word motion scale tags. Replace the active-word format with:
```rust
let aw_anim = active_word_anim(&s.motion.active_word); // returns e.g. "\\t(0,90,\\fscx115\\fscy115)\\t(90,180,\\fscx100\\fscy100)" or ""
line.push_str(&format!("{{\\c{hi_col}{aw_anim}}}{escaped}{{\\c{primary_col}}}"));
```
Add the helper:
```rust
fn active_word_anim(m: &crate::timeline::CaptionRenderActiveWord) -> String {
    use crate::timeline::CaptionRenderActiveWord as A;
    match m {
        A::None => String::new(),
        A::Bounce => "\\fscx100\\fscy100\\t(0,90,\\fscx115\\fscy115)\\t(90,180,\\fscx100\\fscy100)".into(),
        A::ScalePop => "\\fscx100\\fscy100\\t(0,120,\\fscx112\\fscy112)".into(),
        A::Shake => "\\frz0\\t(0,60,\\frz3)\\t(60,120,\\frz-3)\\t(120,180,\\frz0)".into(),
    }
}
```
The per-line cue override goes at the very start of `line` (before the first word).

- [ ] **Step 4: Run — PASS + ass suite green.**

- [ ] **Step 5: Clippy + commit**
```bash
git add crates/render/src/ass.rs
git commit -m "feat(render): active-word motion + active-word-pop entrance/exit composition

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Position-based motion (SlideUp / SlideDown / Float) + resting position

**Files:** Modify `crates/render/src/ass.rs`

Slide/float need the caption's resting (x,y). Compute it from the canvas + alignment + `CaptionLayoutProfile` margins; emit `\move`; fall back to fade/none when unavailable.

- [ ] **Step 1: Write failing tests**

```rust
    #[test]
    fn slide_up_emits_move_into_resting_position() {
        use crate::timeline::*;
        let mut t = caption_title("hello", vec![]);
        let mut s = motion_style(CaptionRenderEntrance::SlideUp, CaptionRenderExit::None);
        t.caption_style = Some(s);
        // SlideUp needs a canvas; build via the canvas-aware path.
        let doc = build_ass_document_with_canvas(&t, RenderCanvas { width: 1080, height: 1920 });
        let d = doc.lines().find(|l| l.starts_with("Dialogue:")).unwrap();
        assert!(d.contains("\\move("), "SlideUp must emit \\move: {d}");
    }

    #[test]
    fn slide_falls_back_when_no_canvas_resting_position() {
        // build_ass_document (default canvas helper) still produces SOMETHING
        // (a move or a fade), never a broken tag.
        use crate::timeline::*;
        let mut t = caption_title("hello", vec![]);
        t.caption_style = Some(motion_style(CaptionRenderEntrance::SlideUp, CaptionRenderExit::None));
        let d = build_ass_document(&t).lines().find(|l| l.starts_with("Dialogue:")).unwrap().to_string();
        assert!(d.contains("\\move(") || d.contains("\\fad("), "slide must move or fall back to fade: {d}");
    }
```

- [ ] **Step 2: Run — FAIL** (SlideUp currently a no-op from Task 3).

- [ ] **Step 3: Implement**

`build_ass_document`/`build_dialogue_lines` already receive the canvas (from the PlayRes fix — `build_ass_document_with_canvas`/the canvas plumbing). Thread the canvas into `build_dialogue_lines` (add a `canvas: RenderCanvas` param; update its callers in this file — they already have the canvas from `build_ass_document(title, canvas)`).

Compute the resting baseline for the bottom-aligned caption:
```rust
/// Resting anchor (x,y) in PlayRes coords for a bottom-aligned caption.
/// x = horizontal center; y = canvas height minus the bottom margin.
fn caption_resting_xy(title: &TitlePlan, canvas: RenderCanvas) -> (i64, i64) {
    let layout = CaptionLayoutProfile::for_title(title);
    let x = (canvas.width / 2) as i64;
    let y = (canvas.height as i64 - layout.margin_v_bottom as i64).max(0);
    (x, y)
}
```
Extend `motion_override` (or add a position-aware sibling that takes `(title, canvas)`) so:
- `SlideUp` entrance → `\move(x, y+SLIDE_PX, x, y, 0, 150)` with `SLIDE_PX = 60`, plus a `\an2\pos`-free approach: since `\move` sets position, also emit `\an2` so the anchor matches bottom-center. Concretely prepend `\an2\move({x},{y2},{x},{y},0,150)` where `y2 = y + 60`. Pair with a `\fad(150,0)`.
- `SlideDown` exit → `\move({x},{y},{x},{y+60},{dur-150},{dur})` + `\fad(0,150)`.
- `Float` continuous → `\move({x},{y},{x},{y-12},0,{dur})` (a single slow upward drift).
- If the canvas is degenerate (width 0 or height <= margin) → fall back: `SlideUp`→`FadeIn`, `SlideDown`→`FadeOut`, `Float`→none.

Wire the position-aware path so the slide/float tags compose with the scale/fade ones from Task 3 in the single override block. (Keep entrance/exit mutually consistent: a slide entrance replaces the scale-init for that line.)

- [ ] **Step 4: Run — PASS + ass suite green.**

- [ ] **Step 5: Clippy + commit**
```bash
git add crates/render/src/ass.rs
git commit -m "feat(render): slide/float caption motion via resting-position \\move (+ fade fallback)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: `emphasis` preset

**Files:** Modify `crates/core/src/caption/styles.rs`

- [ ] **Step 1: Failing test**
```rust
    #[test]
    fn emphasis_preset_is_poppy_boxed_and_animated() {
        let e = resolve_preset("emphasis").expect("emphasis");
        assert!(matches!(e.background, CaptionBackground::Box { .. }));
        assert!(matches!(e.motion.entrance, EntranceMotion::PopIn));
        assert!(matches!(e.motion.active_word, ActiveWordMotion::Bounce));
        assert!(e.font_size >= 64);
        assert!(preset_names().contains(&"emphasis"));
    }
```

- [ ] **Step 2: Run — FAIL.**

- [ ] **Step 3: Implement** — add to `preset_names()` (`&["clean_white", "word_pop", "boxed", "emphasis"]`) and a `resolve_preset` arm:
```rust
        "emphasis" => CaptionStyleSpec {
            font_size: 72,
            weight: CaptionWeight::Bold,
            casing: CaptionCasing::Upper,
            primary_color: "#FFFFFF".into(),
            highlight_color: Some("#FFE000".into()),
            reveal: RevealMode::ActiveWordPop,
            background: CaptionBackground::Box { color: "#000000".into(), opacity: 178 },
            motion: CaptionMotion {
                entrance: EntranceMotion::PopIn,
                active_word: ActiveWordMotion::Bounce,
                exit: ExitMotion::None,
                continuous: ContinuousMotion::None,
            },
        },
```

- [ ] **Step 4: Run — PASS** (`cargo test -p awidat-core caption::styles`).

- [ ] **Step 5: Commit**
```bash
git add crates/core/src/caption/styles.rs
git commit -m "feat(caption): add emphasis preset (poppy, boxed, animated) for hook/keyword lines

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: Skill — motion vocabulary + restraint rule

**Files:** Modify `video_editing_transcripts/knowledge/captions/SKILL.md`

- [ ] **Step 1:** In the "Style presets" section, add a short **Motion** subsection: the vocabulary (entrance pop/slide/fade; active-word bounce/scale/shake; exit; continuous float), the presets' motion (`clean_white` none, `word_pop` subtle pop+bounce, `emphasis` poppy), and the corpus rule — *motion = premium but purposeful; minimal by default; motion is an emphasis lever (reserve the poppy/`emphasis` look for hooks/keywords, keep filler simple); register by content (cinematic minimal, short-form active); never animate so much it distracts from the story.* Cite "a little bounce" / "no distractions … popping up every word is distracting."
- [ ] **Step 2:** No commit needed (gitignored tree) — working-tree change, consistent with how that deliverable is managed.

---

## Task 8: Scoped gate

**Files:** none.
- [ ] `cargo fmt --all` then `cargo fmt --all -- --check` (nightly-config warnings OK).
- [ ] `CARGO_INCREMENTAL=0 cargo clippy -p awidat-core -p awidat-render --all-targets -- -D warnings 2>&1 | tail -6` → clean.
- [ ] `CARGO_INCREMENTAL=0 cargo test -p awidat-core -p awidat-render 2>&1 | grep -E "test result:|FAILED" | tail` → 0 failed.
- [ ] Commit any fmt fixups (`git add crates/ && git commit -m "chore(caption): fmt fixups for motion" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"`).

---

## Task 9: Proof renders (manual, user sign-off)

**Files:** none. Render from a clean path. Reuse `/Users/explicit/vshort_src.mp4` (synthetic raw vertical) + the warm whisper env.

- [ ] **Step 1: Build CLI** — `CARGO_INCREMENTAL=0 cargo build -p awidat-cli --bin awidat`.
- [ ] **Step 2:** Reuse/recreate a clean-path vertical project from `vshort_src.mp4`; transcribe out-of-band (`/tmp/transcribe_sidecar.py` via `uv run --package whisper-mcp`); write the whisper sidecar.
- [ ] **Step 3:** Generate the short-form EDL via a throwaway `crates/core/examples/*.rs` harness calling `plan_scene_aware_short_form` (short-form already uses `word_pop` → now with PopIn + Bounce). **Also** generate an `emphasis`-preset EDL via a `plan_captions preset=emphasis` harness on the same clip. DELETE the harness(es) before finishing (they trip clippy).
- [ ] **Step 4:** `apply-edl` + `render` each through `awidat render` (vertical 1080×1920). Confirm: the cue **pops in**, the active word **springs** as spoken (Bounce), `emphasis` reads poppier (box + bigger), and all-None presets are still calm.
- [ ] **Step 5:** Extract frames at word boundaries (mid-bounce) + a pop-in moment; present to the user. Delete the harness.
- [ ] **Step 6:** Capture the verdict — *premium but not distracting?* Done only on sign-off.

---

## Self-review

**Spec coverage:** §3.1 model → Task 1; §3.2 render lowering (entrance/exit) → Task 3, (active-word + composition) → Task 4, (slide/float + position) → Task 5; render mirror → Task 2; §3.3 presets + emphasis → Tasks 1 & 6; §3.4 skill → Task 7; §6 testing → per-task + Task 8; proof → Task 9. The "no new EDL grammar" claim → relies on the existing style_json path (called out in File structure; Task 2's default-when-absent test guards back-compat). ✅

**Placeholder scan:** No "TBD/handle errors". Concrete ASS tag strings + timings throughout (tuning noted in spec §9). The slide/float resting-position math is fully specified (Task 5) with an explicit fallback. ✅

**Type consistency:** core `CaptionMotion{entrance,active_word,exit,continuous}` with `EntranceMotion/ActiveWordMotion/ExitMotion/ContinuousMotion` (Task 1) ↔ render `CaptionRenderMotion` with `CaptionRenderEntrance/ActiveWord/Exit/Continuous` (Task 2), field/tag parity for the shared `style_json`. `motion_override(motion,role,dur_cs)`, `active_word_anim(&active)`, `caption_resting_xy(title,canvas)`, `CueLineRole{Sole,First,Middle,Last}` defined in Tasks 3–5 and used consistently. `#[serde(default)]` on both `motion` fields guarantees Phase-1 `style_json` still parses. ✅
