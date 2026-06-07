# Caption Readability Model + Cross-Format Caption Planning — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give montage a real caption reading-speed/segmentation model and lift caption planning/styling out of the short-form-only path into a format-agnostic service, proven by a short-form regression render and a long-form Episode render.

**Architecture:** Approach A (extract-and-share). New `caption` module under `crates/core/src/caption/` holding the shared readability core, a style registry, a placement/style strategy trait + planner, and a format-agnostic caption EDL builder. The existing short-form planner becomes a thin caller of these. A new `plan_captions` MCP tool exposes the general path. The `CaptionRecommendation`/`CaptionStyle`/`CaptionPlacement` types move into the shared module and are re-exported from `scene_aware_short_form` so existing call sites keep compiling.

**Tech Stack:** Rust (workspace crate `montage-core`), `serde`/`serde_json`, `schemars` (MCP arg schemas), `rmcp` `#[tool]` macro. Render via existing `montage-render` ASS path (unchanged). Spec: `docs/superpowers/specs/2026-06-04-caption-readability-and-cross-format-design.md`.

**Conventions for every task:**
- Run a single test by name: `cargo test -p montage-core <test_name> -- --nocapture`
- Module tests live in `#[cfg(test)] mod tests` at the bottom of each module file (matches the codebase).
- Commit messages end with the project trailer:
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  ```
- Final gate per task group: `make check` (= `cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace`).

---

## File structure (created / modified)

- Create: `crates/core/src/caption/mod.rs` — module root + re-exports.
- Create: `crates/core/src/caption/types.rs` — `CaptionRecommendation`, `CaptionStyle`, `CaptionPlacement`, `CaptionWordTiming` (moved from `scene_aware_short_form.rs`).
- Create: `crates/core/src/caption/readability.rs` — `CaptionFormatProfile`, `RevealMode`, `Cue`, `ReadabilityProposal`, `segment()`, `lint()`.
- Create: `crates/core/src/caption/styles.rs` — `CaptionFormat`, `CaptionMood`, `CaptionStyleSpec`, `resolve_style()`, `MIN_LEGIBLE_FONT_SIZE`.
- Create: `crates/core/src/caption/planner.rs` — `CuePlan`, `CaptionPlanStrategy` trait, `LowerSafeZoneStrategy`, `plan()`.
- Create: `crates/core/src/caption/edl.rs` — `build_caption_edl_lines(recs, spec)` format-agnostic caption EDL.
- Create: `crates/core/src/montage_mcp/tools/plan_captions.rs` — new MCP tool `run()`.
- Modify: `crates/core/src/lib.rs` — add `pub mod caption;` (next to `pub mod captions;`).
- Modify: `crates/core/src/scene_aware_short_form.rs` — delete moved type defs, add `pub use`, add `ShotAwareStrategy`, rewire `plan_captions`, route caption EDL through `caption::edl`.
- Modify: `crates/core/src/montage_mcp/tools/mod.rs` — add `pub mod plan_captions;`.
- Modify: `crates/core/src/montage_mcp/mod.rs` — add `use` + `#[tool]` method.
- Modify: `video_editing_transcripts/knowledge/captions/SKILL.md` — add `plan_captions` to `tools_allowlist`.

---

## Task 1: Readability profiles, Cue, and `segment()`

**Files:**
- Create: `crates/core/src/caption/readability.rs`
- Create: `crates/core/src/caption/mod.rs`
- Modify: `crates/core/src/lib.rs`

- [ ] **Step 1: Add the module to the crate and create `mod.rs`**

In `crates/core/src/lib.rs`, add directly after the line `pub mod captions;`:

```rust
pub mod caption;
```

Create `crates/core/src/caption/mod.rs`:

```rust
//! Format-agnostic caption craft: readability segmentation, style registry,
//! planning strategies, and caption EDL emission. Shared by the short-form
//! planner and the general `plan_captions` tool.

pub mod readability;
```

- [ ] **Step 2: Write the failing test for `segment()`**

Create `crates/core/src/caption/readability.rs` with ONLY the test module first (so it fails to compile → fails):

```rust
//! Caption reading-speed and segmentation model. Pure: no I/O, no scene data.

#[cfg(test)]
mod tests {
    use super::*;

    fn words(pairs: &[(&str, f64, f64)]) -> Vec<InputWord> {
        pairs
            .iter()
            .map(|(t, s, e)| InputWord { text: (*t).into(), start_s: *s, end_s: *e })
            .collect()
    }

    #[test]
    fn segment_splits_by_char_budget_without_overlap() {
        let profile = CaptionFormatProfile::short_form(); // 1 line, 15 cpl -> budget 15
        let w = words(&[
            ("one", 0.0, 0.5),
            ("two", 0.5, 1.0),
            ("three", 1.0, 1.5),
            ("four", 1.5, 2.0),
            ("five", 2.0, 2.5),
            ("sixsix", 2.5, 3.0),
        ]);
        let cues = segment(&w, &profile);
        assert!(cues.len() >= 2, "should split across the char budget, got {}", cues.len());
        for cue in &cues {
            assert!(cue.lines.len() <= profile.max_lines);
            for line in &cue.lines {
                assert!(line.chars().count() <= profile.max_chars_per_line, "line too long: {line:?}");
            }
        }
        for pair in cues.windows(2) {
            assert!(pair[0].end_s <= pair[1].start_s + 1e-6, "cues must not overlap: {pair:?}");
        }
    }

    #[test]
    fn segment_is_zero_gap_on_continuous_speech() {
        let profile = CaptionFormatProfile::short_form();
        let w = words(&[
            ("the", 0.0, 0.3),
            ("quick", 0.3, 0.7),
            ("brown", 0.7, 1.1),
            ("fox", 1.1, 1.5),
            ("jumps", 1.5, 1.9),
        ]);
        let cues = segment(&w, &profile);
        assert!(cues.len() >= 2, "continuous speech over the budget should split");
        for pair in cues.windows(2) {
            assert!((pair[1].start_s - pair[0].end_s).abs() < 1e-6, "must be zero-gap (no gap, no overlap): {pair:?}");
        }
    }

    #[test]
    fn segment_extends_a_short_final_cue_toward_readable_minimum() {
        let profile = CaptionFormatProfile::long_form();
        let cues = segment(&words(&[("hi", 0.0, 0.1)]), &profile);
        assert_eq!(cues.len(), 1);
        assert!(
            cues[0].end_s - cues[0].start_s >= profile.min_cue_s - 1e-6,
            "the final short cue should extend toward the readable minimum: {:?}", cues[0]
        );
    }

    #[test]
    fn segment_does_not_overlap_or_desync_on_dense_fast_speech() {
        // 34 chars in 1.0s is physically faster than 17 CPS. segment() must NOT
        // overlap cues or shift starts to "fix" this — lint() surfaces the
        // residual instead. (Here the words fit one budget-cue, so the single
        // trailing cue is simply held longer; no start is moved.)
        let profile = CaptionFormatProfile::long_form();
        let w = words(&[
            ("absolutely", 0.0, 0.4),
            ("incredible", 0.4, 0.7),
            ("breakthrough", 0.7, 1.0),
        ]);
        let cues = segment(&w, &profile);
        assert_eq!(cues[0].start_s, 0.0, "starts must stay synced to audio");
        for pair in cues.windows(2) {
            assert!(pair[0].end_s <= pair[1].start_s + 1e-6, "cues must not overlap: {pair:?}");
        }
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p montage-core caption::readability -- --nocapture`
Expected: FAIL — compile errors (`InputWord`, `segment`, `CaptionFormatProfile`, `Cue` not found).

- [ ] **Step 4: Implement the minimal types + `segment()`**

Add above the test module in `crates/core/src/caption/readability.rs`:

```rust
use serde::{Deserialize, Serialize};

/// Hard reading-speed ceiling in characters per second (≈160–180 wpm).
pub const MAX_CPS: f64 = 17.0;

/// One transcript word with timing, the input to segmentation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputWord {
    pub text: String,
    pub start_s: f64,
    pub end_s: f64,
}

/// How a cue is revealed; controls whether per-word timings are emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevealMode {
    WholeCue,
    WordByWord,
}

/// Per-format readability constraints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionFormatProfile {
    pub max_chars_per_line: usize,
    pub max_lines: usize,
    pub max_cps: f64,
    pub min_cue_s: f64,
    pub max_cue_s: f64,
    pub reveal: RevealMode,
}

impl CaptionFormatProfile {
    pub fn short_form() -> Self {
        Self { max_chars_per_line: 15, max_lines: 1, max_cps: MAX_CPS, min_cue_s: 0.5, max_cue_s: 7.0, reveal: RevealMode::WordByWord }
    }
    pub fn long_form() -> Self {
        Self { max_chars_per_line: 42, max_lines: 2, max_cps: MAX_CPS, min_cue_s: 0.5, max_cue_s: 7.0, reveal: RevealMode::WholeCue }
    }
    pub fn accessibility() -> Self {
        Self { max_chars_per_line: 42, max_lines: 2, max_cps: MAX_CPS, min_cue_s: 0.7, max_cue_s: 7.0, reveal: RevealMode::WholeCue }
    }
}

/// A finished caption cue: timing, wrapped lines, and word timings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub start_s: f64,
    pub end_s: f64,
    pub lines: Vec<String>,
    pub word_timings: Vec<InputWord>,
}

impl Cue {
    pub fn char_count(&self) -> usize {
        self.lines.iter().map(|l| l.chars().count()).sum()
    }
    pub fn cps(&self) -> f64 {
        let dur = (self.end_s - self.start_s).max(1e-6);
        self.char_count() as f64 / dur
    }
}

/// Group words into cues by char budget and sense units, then fix timing.
///
/// Grouping ignores CPS on purpose: over-dense speech cannot be made readable by
/// splitting without overlapping cues or shifting starts off the audio. Instead
/// `finalize_timing` keeps starts synced, makes cues zero-gap and non-overlapping,
/// and extends only the trailing cue toward the readable minimum; `lint()` then
/// surfaces any residual CPS overrun as a proposal.
pub fn segment(words: &[InputWord], profile: &CaptionFormatProfile) -> Vec<Cue> {
    let budget = profile.max_chars_per_line * profile.max_lines;
    let mut cues = Vec::new();
    let mut current: Vec<InputWord> = Vec::new();

    for word in words {
        let mut candidate = current.clone();
        candidate.push(word.clone());
        if !current.is_empty() && !fits_budget(&candidate, profile, budget) {
            cues.push(flush(&current, profile));
            current = vec![word.clone()];
        } else {
            current = candidate;
            if ends_sense_unit(&word.text) {
                cues.push(flush(&current, profile));
                current = Vec::new();
            }
        }
    }
    if !current.is_empty() {
        cues.push(flush(&current, profile));
    }
    finalize_timing(&mut cues, profile);
    cues
}

fn fits_budget(words: &[InputWord], profile: &CaptionFormatProfile, budget: usize) -> bool {
    cue_chars(words) <= budget && cue_dur(words) <= profile.max_cue_s
}

fn cue_chars(words: &[InputWord]) -> usize {
    let text = words.iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" ");
    text.chars().count()
}

fn cue_dur(words: &[InputWord]) -> f64 {
    match (words.first(), words.last()) {
        (Some(f), Some(l)) => (l.end_s - f.start_s).max(1e-6),
        _ => 1e-6,
    }
}

fn ends_sense_unit(text: &str) -> bool {
    text.trim_end().ends_with(['.', '?', '!', ',', ';', ':'])
}

fn flush(words: &[InputWord], profile: &CaptionFormatProfile) -> Cue {
    let lines = wrap_lines(words, profile.max_chars_per_line, profile.max_lines);
    Cue {
        start_s: words.first().map(|w| w.start_s).unwrap_or(0.0),
        end_s: words.last().map(|w| w.end_s).unwrap_or(0.0),
        lines,
        word_timings: words.to_vec(),
    }
}

/// Greedily pack words into up to `max_lines` lines of `max_chars_per_line`.
/// For 2-line cues, keep the bottom line no longer than the top.
fn wrap_lines(words: &[InputWord], max_chars_per_line: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for w in words {
        let token = w.text.trim();
        if line.is_empty() {
            line.push_str(token);
        } else if line.chars().count() + 1 + token.chars().count() <= max_chars_per_line {
            line.push(' ');
            line.push_str(token);
        } else if lines.len() + 1 < max_lines {
            lines.push(std::mem::take(&mut line));
            line.push_str(token);
        } else {
            // No room left under the line budget: append anyway (segment() guards the budget).
            line.push(' ');
            line.push_str(token);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Fix cue timing without breaking audio sync:
/// - Non-last cue: end at the next cue's start (zero-gap), but never hold longer
///   than `max_cue_s` (so a long silence leaves a gap rather than a stuck caption),
///   and never overlap the next cue.
/// - Last cue: extend toward the readable minimum (`min_cue_s` and the CPS ceiling),
///   since it has no successor to overlap.
/// Starts are never moved, so captions stay synced to the spoken word. Residual
/// CPS overruns on interior cues are intentional and left for `lint()`.
fn finalize_timing(cues: &mut [Cue], profile: &CaptionFormatProfile) {
    let n = cues.len();
    for i in 0..n {
        if i + 1 < n {
            let next_start = cues[i + 1].start_s;
            let max_end = cues[i].start_s + profile.max_cue_s;
            let target = next_start.min(max_end);
            if cues[i].end_s < target {
                cues[i].end_s = target;
            }
            if cues[i].end_s > next_start {
                cues[i].end_s = next_start; // defensive: never overlap
            }
        } else {
            let chars = cues[i].char_count();
            let min_dur = (chars as f64 / profile.max_cps).max(profile.min_cue_s);
            let min_end = cues[i].start_s + min_dur;
            if cues[i].end_s < min_end {
                cues[i].end_s = min_end;
            }
        }
    }
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p montage-core caption::readability -- --nocapture`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/caption/mod.rs crates/core/src/caption/readability.rs crates/core/src/lib.rs
git commit -m "feat(caption): add reading-speed segmentation core (segment + profiles)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Readability `lint()` with proposals

**Files:**
- Modify: `crates/core/src/caption/readability.rs`

- [ ] **Step 1: Write the failing test**

Add inside the existing `mod tests` in `readability.rs`:

```rust
fn cue(start_s: f64, end_s: f64, text: &str) -> Cue {
    Cue { start_s, end_s, lines: vec![text.into()], word_timings: vec![] }
}

#[test]
fn lint_flags_cps_overrun_with_split_proposal() {
    // 40 chars in 1.0s = 40 CPS.
    let cues = vec![cue(0.0, 1.0, "an extraordinarily dense caption line!!")];
    let proposals = lint(&cues, &CaptionFormatProfile::long_form());
    assert!(proposals.iter().any(|p| matches!(p, ReadabilityProposal::Split { .. })));
    let p = proposals.iter().find(|p| matches!(p, ReadabilityProposal::Split { .. })).unwrap();
    assert!(p.rationale().contains("CPS"), "rationale must explain the CPS overrun: {}", p.rationale());
}

#[test]
fn lint_flags_sub_minimum_duration_with_extend_proposal() {
    let cues = vec![cue(0.0, 0.2, "hi")]; // 0.2s < 0.5s minimum
    let proposals = lint(&cues, &CaptionFormatProfile::long_form());
    assert!(proposals.iter().any(|p| matches!(p, ReadabilityProposal::Extend { .. })));
}

#[test]
fn lint_is_silent_on_clean_cues() {
    let cues = vec![cue(0.0, 2.0, "a calm, readable line")];
    let proposals = lint(&cues, &CaptionFormatProfile::long_form());
    assert!(proposals.is_empty(), "clean cue should not be flagged: {proposals:?}");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p montage-core caption::readability::tests::lint -- --nocapture`
Expected: FAIL — `lint`, `ReadabilityProposal` not found.

- [ ] **Step 3: Implement `ReadabilityProposal` + `lint()`**

Add to `readability.rs` (above the test module):

```rust
/// A non-destructive readability recommendation for an existing cue. The model
/// never rewrites a timeline; it proposes, with a human rationale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReadabilityProposal {
    Split { at_s: f64, rationale: String },
    Extend { to_s: f64, rationale: String },
    Reflow { rationale: String },
}

impl ReadabilityProposal {
    pub fn rationale(&self) -> &str {
        match self {
            Self::Split { rationale, .. } | Self::Extend { rationale, .. } | Self::Reflow { rationale } => rationale,
        }
    }
}

/// Inspect existing cues and emit split/extend/reflow proposals where a cue
/// violates the profile. Never mutates the cues.
pub fn lint(cues: &[Cue], profile: &CaptionFormatProfile) -> Vec<ReadabilityProposal> {
    let mut out = Vec::new();
    for c in cues {
        let dur = c.end_s - c.start_s;
        if c.cps() > profile.max_cps {
            let mid = c.start_s + dur / 2.0;
            out.push(ReadabilityProposal::Split {
                at_s: mid,
                rationale: format!(
                    "Split at {mid:.1}s — {} chars/{:.1}s = {:.0} CPS exceeded {:.0} CPS reading ceiling.",
                    c.char_count(), dur, c.cps(), profile.max_cps
                ),
            });
        }
        if dur < profile.min_cue_s {
            let to = c.start_s + profile.min_cue_s;
            out.push(ReadabilityProposal::Extend {
                to_s: to,
                rationale: format!("Extend to {to:.1}s — cue is {dur:.2}s, under the {:.1}s minimum.", profile.min_cue_s),
            });
        }
        if c.lines.len() > profile.max_lines
            || c.lines.iter().any(|l| l.chars().count() > profile.max_chars_per_line)
        {
            out.push(ReadabilityProposal::Reflow {
                rationale: format!(
                    "Reflow — cue exceeds {} line(s) of {} chars.",
                    profile.max_lines, profile.max_chars_per_line
                ),
            });
        }
    }
    out
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p montage-core caption::readability -- --nocapture`
Expected: PASS (all readability tests).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/caption/readability.rs
git commit -m "feat(caption): add readability lint() emitting split/extend/reflow proposals

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: Style registry with legibility floor

**Files:**
- Create: `crates/core/src/caption/styles.rs`
- Modify: `crates/core/src/caption/mod.rs`

- [ ] **Step 1: Register the module**

In `crates/core/src/caption/mod.rs` add:

```rust
pub mod styles;
```

- [ ] **Step 2: Write the failing test**

Create `crates/core/src/caption/styles.rs` with the test module first:

```rust
//! Caption style registry keyed by (format, mood). Returns the EDL-carryable
//! style knobs (font size, color, reveal). The render layer always draws an
//! outline + shadow, so the in-code legibility floor here is min font size +
//! a valid high-contrast color.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caption::readability::RevealMode;

    #[test]
    fn every_mood_meets_the_legibility_floor() {
        for format in [CaptionFormat::ShortForm, CaptionFormat::LongForm, CaptionFormat::Accessibility] {
            for mood in [CaptionMood::MinimalCinematic, CaptionMood::ActivePop] {
                let spec = resolve_style(format, mood);
                assert!(spec.font_size >= MIN_LEGIBLE_FONT_SIZE, "{format:?}/{mood:?} font too small");
                assert!(spec.color.starts_with('#') && spec.color.len() == 7, "{format:?}/{mood:?} bad color");
            }
        }
    }

    #[test]
    fn moods_are_distinct() {
        let calm = resolve_style(CaptionFormat::LongForm, CaptionMood::MinimalCinematic);
        let pop = resolve_style(CaptionFormat::LongForm, CaptionMood::ActivePop);
        assert_eq!(calm.reveal, RevealMode::WholeCue);
        assert_eq!(pop.reveal, RevealMode::WordByWord);
        assert!(pop.font_size >= calm.font_size);
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p montage-core caption::styles -- --nocapture`
Expected: FAIL — types not found.

- [ ] **Step 4: Implement the registry**

Add above the test module in `styles.rs`:

```rust
use serde::{Deserialize, Serialize};

use crate::caption::readability::RevealMode;

/// Smallest font size we will ever emit; below this captions fail on mobile.
pub const MIN_LEGIBLE_FONT_SIZE: u32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionFormat {
    ShortForm,
    LongForm,
    Accessibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptionMood {
    MinimalCinematic,
    ActivePop,
}

/// EDL-carryable caption style knobs. Outline/shadow are render invariants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptionStyleSpec {
    pub font_size: u32,
    pub color: String,
    pub reveal: RevealMode,
}

/// Resolve the style for a (format, mood), enforcing the legibility floor.
pub fn resolve_style(format: CaptionFormat, mood: CaptionMood) -> CaptionStyleSpec {
    let (mut font_size, color, reveal) = match (format, mood) {
        (CaptionFormat::ShortForm, CaptionMood::MinimalCinematic) => (52, "#FFFFFF", RevealMode::WholeCue),
        (CaptionFormat::ShortForm, CaptionMood::ActivePop) => (64, "#FFFFFF", RevealMode::WordByWord),
        (CaptionFormat::LongForm, CaptionMood::MinimalCinematic) => (44, "#FFFFFF", RevealMode::WholeCue),
        (CaptionFormat::LongForm, CaptionMood::ActivePop) => (56, "#FFFFFF", RevealMode::WordByWord),
        (CaptionFormat::Accessibility, _) => (44, "#FFFFFF", RevealMode::WholeCue),
    };
    if font_size < MIN_LEGIBLE_FONT_SIZE {
        font_size = MIN_LEGIBLE_FONT_SIZE;
    }
    let color = if color.starts_with('#') && color.len() == 7 { color.to_string() } else { "#FFFFFF".to_string() };
    CaptionStyleSpec { font_size, color, reveal }
}
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p montage-core caption::styles -- --nocapture`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/caption/styles.rs crates/core/src/caption/mod.rs
git commit -m "feat(caption): add (format,mood) style registry with legibility floor

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Move shared caption types into `caption::types`

**Files:**
- Create: `crates/core/src/caption/types.rs`
- Modify: `crates/core/src/caption/mod.rs`
- Modify: `crates/core/src/scene_aware_short_form.rs`

This is a pure refactor — no behavior change. All existing tests must stay green.

- [ ] **Step 1: Create `types.rs` with the moved definitions**

Create `crates/core/src/caption/types.rs` by copying these four items VERBATIM from `crates/core/src/scene_aware_short_form.rs` (lines ~112–168): `CaptionPlacement` (incl. its `impl` with `edl_value`), `CaptionRecommendation`, `CaptionWordTiming`, `CaptionStyle`. Header:

```rust
//! Shared caption planning types. Moved out of `scene_aware_short_form` so the
//! general caption path and the short-form path use one definition.

use serde::{Deserialize, Serialize};

// <-- paste CaptionPlacement (+ impl), CaptionRecommendation, CaptionWordTiming, CaptionStyle here -->
```

Make `CaptionPlacement::edl_value` public (`pub fn edl_value`) so `caption::edl` can call it.

- [ ] **Step 2: Register the module and re-export**

In `crates/core/src/caption/mod.rs` add:

```rust
pub mod types;
```

- [ ] **Step 3: Delete the moved defs from `scene_aware_short_form.rs` and re-export**

In `crates/core/src/scene_aware_short_form.rs`, delete the four moved type definitions (and the `CaptionPlacement` impl) and add near the top (after the existing `use serde...` line):

```rust
pub use crate::caption::types::{CaptionPlacement, CaptionRecommendation, CaptionStyle, CaptionWordTiming};
```

If `edl_value` was called as `self.placement.edl_value()` within the module, it still resolves through the re-export — no call-site change needed.

- [ ] **Step 4: Build to verify the refactor compiles with zero behavior change**

Run: `cargo build -p montage-core`
Expected: builds clean. Then:
Run: `cargo test -p montage-core scene_aware_short_form -- --nocapture`
Expected: PASS unchanged (placement/style behavior untouched).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/caption/types.rs crates/core/src/caption/mod.rs crates/core/src/scene_aware_short_form.rs
git commit -m "refactor(caption): move shared caption types into caption::types (re-exported)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Planner — `CaptionPlanStrategy` trait + `plan()` + `LowerSafeZoneStrategy`

**Files:**
- Create: `crates/core/src/caption/planner.rs`
- Modify: `crates/core/src/caption/mod.rs`

- [ ] **Step 1: Register the module**

In `crates/core/src/caption/mod.rs` add:

```rust
pub mod planner;
```

- [ ] **Step 2: Write the failing test**

Create `crates/core/src/caption/planner.rs` with the test module first:

```rust
//! Turns readability cues into CaptionRecommendations via an injected strategy.
//! Placement/style are the only short-form-specific concerns, so they live
//! behind `CaptionPlanStrategy`. The short-form strategy lives in
//! `scene_aware_short_form`; `LowerSafeZoneStrategy` here is the general default.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caption::readability::{Cue, InputWord};

    fn cue(text: &str) -> Cue {
        Cue { start_s: 1.0, end_s: 2.5, lines: vec![text.into()], word_timings: vec![InputWord { text: text.into(), start_s: 1.0, end_s: 2.5 }] }
    }

    #[test]
    fn lower_safe_zone_strategy_places_at_bottom() {
        let cues = vec![cue("hello world")];
        let recs = plan(&cues, &LowerSafeZoneStrategy);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].placement, crate::caption::types::CaptionPlacement::Bottom);
        assert_eq!(recs[0].text, "hello world");
        assert_eq!(recs[0].start_s, 1.0);
        assert_eq!(recs[0].word_timings.len(), 1);
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p montage-core caption::planner -- --nocapture`
Expected: FAIL — `plan`, `LowerSafeZoneStrategy`, `CaptionPlanStrategy` not found.

- [ ] **Step 4: Implement the trait, `plan()`, and the default strategy**

Add above the test module in `planner.rs`:

```rust
use crate::caption::readability::Cue;
use crate::caption::types::{CaptionPlacement, CaptionRecommendation, CaptionStyle, CaptionWordTiming};

/// Per-cue placement + style decision plus the reasons that justify it.
pub struct CuePlan {
    pub placement: CaptionPlacement,
    pub style: CaptionStyle,
    pub visual_reason: String,
    pub safety_reason: String,
    pub confidence: f64,
}

/// Strategy that decides placement/style for each cue. Implementors:
/// `ShotAwareStrategy` (short-form, in `scene_aware_short_form`) and
/// `LowerSafeZoneStrategy` (general default, below).
pub trait CaptionPlanStrategy {
    fn plan_cue(&self, cue: &Cue) -> CuePlan;
}

/// Build CaptionRecommendations from cues using the given strategy.
pub fn plan(cues: &[Cue], strategy: &dyn CaptionPlanStrategy) -> Vec<CaptionRecommendation> {
    cues
        .iter()
        .filter(|c| !c.lines.join(" ").trim().is_empty())
        .map(|c| {
            let decision = strategy.plan_cue(c);
            CaptionRecommendation {
                start_s: c.start_s,
                end_s: c.end_s,
                text: c.lines.join("\n"),
                word_timings: c
                    .word_timings
                    .iter()
                    .map(|w| CaptionWordTiming { text: w.text.clone(), start_s: w.start_s, end_s: w.end_s })
                    .collect(),
                placement: decision.placement,
                style: decision.style,
                transcript_reason: "readability-segmented transcript cue with source timings".into(),
                visual_reason: decision.visual_reason,
                safety_reason: decision.safety_reason,
                confidence: decision.confidence,
            }
        })
        .collect()
}

/// General default: bottom safe zone, whole-cue, no scene analysis required.
pub struct LowerSafeZoneStrategy;

impl CaptionPlanStrategy for LowerSafeZoneStrategy {
    fn plan_cue(&self, _cue: &Cue) -> CuePlan {
        CuePlan {
            placement: CaptionPlacement::Bottom,
            style: CaptionStyle::Plain,
            visual_reason: "lower safe-zone default (no scene analysis for this format)".into(),
            safety_reason: "bottom band avoids faces and the action by convention".into(),
            confidence: 0.7,
        }
    }
}
```

NOTE on the `CaptionWordTiming` field names: verify against `caption::types` after Task 4. If the moved struct uses different field names, match them here (the original is `{ text, start_s, end_s }` per `scene_aware_short_form.rs:155-160`).

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p montage-core caption::planner -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/caption/planner.rs crates/core/src/caption/mod.rs
git commit -m "feat(caption): add CaptionPlanStrategy + plan() + LowerSafeZoneStrategy

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: Format-agnostic caption EDL builder

**Files:**
- Create: `crates/core/src/caption/edl.rs`
- Modify: `crates/core/src/caption/mod.rs`

- [ ] **Step 1: Register the module**

In `crates/core/src/caption/mod.rs` add:

```rust
pub mod edl;
```

- [ ] **Step 2: Write the failing test**

Create `crates/core/src/caption/edl.rs` with the test module first:

```rust
//! Format-agnostic caption EDL emission. Produces `*** Insert Caption` blocks
//! consumable by `apply_edl`, applying the style spec's font/color/reveal.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caption::readability::RevealMode;
    use crate::caption::styles::CaptionStyleSpec;
    use crate::caption::types::{CaptionPlacement, CaptionRecommendation, CaptionStyle, CaptionWordTiming};

    fn rec() -> CaptionRecommendation {
        CaptionRecommendation {
            start_s: 1.0, end_s: 2.0, text: "hello".into(),
            word_timings: vec![CaptionWordTiming { text: "hello".into(), start_s: 1.0, end_s: 2.0 }],
            placement: CaptionPlacement::Bottom, style: CaptionStyle::Plain,
            transcript_reason: "t".into(), visual_reason: "v".into(), safety_reason: "s".into(), confidence: 0.7,
        }
    }

    #[test]
    fn whole_cue_spec_omits_word_timings() {
        let spec = CaptionStyleSpec { font_size: 44, color: "#FFFFFF".into(), reveal: RevealMode::WholeCue };
        let lines = build_caption_edl_lines(&[rec()], &spec, "standard");
        let blob = lines.join("\n");
        assert!(blob.contains("*** Insert Caption"));
        assert!(blob.contains("+ font_size: 44"));
        assert!(blob.contains("+ position: bottom"));
        assert!(!blob.contains("word_timings_json"), "whole-cue must not emit per-word timings");
    }

    #[test]
    fn word_by_word_spec_emits_word_timings() {
        let spec = CaptionStyleSpec { font_size: 56, color: "#FFFFFF".into(), reveal: RevealMode::WordByWord };
        let lines = build_caption_edl_lines(&[rec()], &spec, "standard");
        let blob = lines.join("\n");
        assert!(blob.contains("word_timings_json"));
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p montage-core caption::edl -- --nocapture`
Expected: FAIL — `build_caption_edl_lines` not found.

- [ ] **Step 4: Implement the builder**

Add above the test module in `edl.rs`:

```rust
use crate::caption::readability::RevealMode;
use crate::caption::styles::CaptionStyleSpec;
use crate::caption::types::CaptionRecommendation;

/// Emit `*** Insert Caption` EDL lines for each recommendation, applying the
/// style spec. `safe_area` is the profile string (e.g. "mobile" / "standard").
/// Per-word timings are emitted only when the spec's reveal mode is word-by-word.
pub fn build_caption_edl_lines(
    recs: &[CaptionRecommendation],
    spec: &CaptionStyleSpec,
    safe_area: &str,
) -> Vec<String> {
    let mut lines = Vec::new();
    for caption in recs {
        lines.push("*** Insert Caption".to_string());
        lines.push(format!("+ start_s: {}", fmt_seconds(caption.start_s)));
        lines.push(format!("+ end_s: {}", fmt_seconds(caption.end_s)));
        lines.push(format!("+ text: {}", json_string(&caption.text)));
        lines.push(format!("+ position: {}", edl_position(caption.placement)));
        lines.push(format!("+ font_size: {}", spec.font_size));
        lines.push(format!("+ color: {}", spec.color));
        lines.push(format!("+ safe_area: {safe_area}"));
        if spec.reveal == RevealMode::WordByWord && !caption.word_timings.is_empty() {
            lines.push(format!(
                "+ word_timings_json: {}",
                serde_json::to_string(&caption.word_timings).unwrap_or_else(|_| "[]".into())
            ));
        }
    }
    lines
}

/// The EDL caption `position` field is a vertical band; the parser accepts only
/// `top|center|bottom`. Map any placement (incl. the horizontal Left/Right hints)
/// to a valid vertical value so the emitted EDL always re-parses.
fn edl_position(placement: crate::caption::types::CaptionPlacement) -> &'static str {
    use crate::caption::types::CaptionPlacement;
    match placement {
        CaptionPlacement::Upper => "top",
        CaptionPlacement::Bottom | CaptionPlacement::Left | CaptionPlacement::Right => "bottom",
    }
}

fn fmt_seconds(value: f64) -> String {
    // Trim to 3 decimals without trailing zeros, matching scene_aware fmt_number style.
    let s = format!("{value:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

fn json_string(text: &str) -> String {
    serde_json::Value::String(text.to_string()).to_string()
}
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p montage-core caption::edl -- --nocapture`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/caption/edl.rs crates/core/src/caption/mod.rs
git commit -m "feat(caption): add format-agnostic Insert Caption EDL builder

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: Rewire the short-form planner onto the shared core

**Files:**
- Modify: `crates/core/src/scene_aware_short_form.rs`

Goal: short-form now segments via `caption::readability::segment(short_form())` and plans via a `ShotAwareStrategy` implementing `CaptionPlanStrategy`, and its caption EDL goes through `caption::edl`. Behavior parity is "valid + legible," not byte-identical (cue boundaries may improve where the old chunking broke 17 CPS).

- [ ] **Step 1: Add a regression test pinning short-form invariants (not exact boundaries)**

Add to the `#[cfg(test)] mod tests` in `scene_aware_short_form.rs` (or create one if absent — match the file's existing test style):

```rust
#[test]
fn short_form_captions_obey_readability_and_stay_bottom_or_upper() {
    use crate::caption::readability::CaptionFormatProfile;
    let mut input = SceneAwareShortFormInput::default();
    input.source_width = 1080;
    input.source_height = 1920;
    input.transcript = serde_json::json!({
        "segments": [{
            "start_s": 0.0, "end_s": 1.0,
            "text": "absolutely incredible breakthrough today",
            "words": [
                {"text": "absolutely", "start_s": 0.0, "end_s": 0.25},
                {"text": "incredible", "start_s": 0.25, "end_s": 0.5},
                {"text": "breakthrough", "start_s": 0.5, "end_s": 0.75},
                {"text": "today", "start_s": 0.75, "end_s": 1.0}
            ]
        }]
    });
    let plan = build_scene_aware_short_form_plan(input);
    assert!(!plan.caption_plan.is_empty());
    let profile = CaptionFormatProfile::short_form();
    for cue in &plan.caption_plan {
        let chars = cue.text.replace('\n', " ").chars().count() as f64;
        let dur = (cue.end_s - cue.start_s).max(1e-6);
        assert!(chars / dur <= profile.max_cps + 1e-6, "short-form cue over CPS ceiling: {:?}", cue.text);
        assert!(matches!(cue.placement, CaptionPlacement::Bottom | CaptionPlacement::Upper | CaptionPlacement::Left | CaptionPlacement::Right));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p montage-core short_form_captions_obey_readability -- --nocapture`
Expected: FAIL — today's `plan_captions` uses raw `transcript_segments` (no CPS enforcement), so the over-fast single segment violates the ceiling.

- [ ] **Step 3: Add `ShotAwareStrategy` and rewire `plan_captions`**

In `scene_aware_short_form.rs`, replace the body of `fn plan_captions` (lines ~323–370) so it segments via the shared core, then plans via a shot-aware strategy. Add a `ShotAwareStrategy` struct holding the shots and the input's transcript-derived words.

First, build words from the transcript (reuse `transcript_segments` to gather `word_timings`, then flatten):

```rust
fn plan_captions(
    input: &SceneAwareShortFormInput,
    shots: &[SceneShotAnalysis],
) -> Vec<CaptionRecommendation> {
    use crate::caption::planner::{plan, CaptionPlanStrategy, CuePlan};
    use crate::caption::readability::{segment, CaptionFormatProfile, InputWord};

    // Flatten transcript word timings into the readability model's input.
    let words: Vec<InputWord> = transcript_segments(&input.transcript)
        .into_iter()
        .flat_map(|seg| {
            if seg.word_timings.is_empty() {
                // No per-word timing: treat the whole segment text as one "word".
                vec![InputWord { text: seg.text, start_s: seg.start_s, end_s: seg.end_s }]
            } else {
                seg.word_timings
                    .into_iter()
                    .map(|w| InputWord { text: w.text, start_s: w.start_s, end_s: w.end_s })
                    .collect()
            }
        })
        .collect();

    let cues = segment(&words, &CaptionFormatProfile::short_form());

    struct ShotAwareStrategy<'a> { shots: &'a [SceneShotAnalysis] }
    impl CaptionPlanStrategy for ShotAwareStrategy<'_> {
        fn plan_cue(&self, cue: &crate::caption::readability::Cue) -> CuePlan {
            let shot = shot_at(self.shots, cue.start_s);
            let placement = shot
                .and_then(|shot| shot.safe_text_zones.first().copied())
                .unwrap_or(CaptionPlacement::Bottom);
            let style = if shot.is_some_and(|s| s.busy_regions.contains(&placement)) {
                CaptionStyle::Boxed
            } else if shot.is_some_and(|s| s.motion_intensity > 0.7) {
                CaptionStyle::Minimal
            } else {
                CaptionStyle::Plain
            };
            let visual_reason = shot.map(caption_visual_reason).unwrap_or_else(|| "uses default mobile caption zone".into());
            let safety_reason = if shot.is_some_and(|s| s.face_box.is_some()) {
                format!(
                    "placement avoids detected face/eye/mouth regions; bottom_safe={}",
                    !shot.is_some_and(|s| placement_overlaps_face(CaptionPlacement::Bottom, s.face_box))
                )
            } else {
                "no face region detected in this shot".into()
            };
            CuePlan { placement, style, visual_reason, safety_reason, confidence: shot.map(confidence_for_shot).unwrap_or(0.58) }
        }
    }

    plan(&cues, &ShotAwareStrategy { shots })
}
```

(Keep `caption_recommendation`, `shot_at`, `caption_visual_reason`, `placement_overlaps_face`, `confidence_for_shot` as they are — they're reused.)

- [ ] **Step 4: Route short-form caption EDL through the shared builder**

In `build_edl_fragment` (lines ~646–661), replace the inline caption loop with a call to the shared builder using a short-form spec (preserving today's font_size 56 / white / per-word reveal):

```rust
// Captions (shared format-agnostic emission; short-form spec preserves prior look).
{
    use crate::caption::edl::build_caption_edl_lines;
    use crate::caption::readability::RevealMode;
    use crate::caption::styles::CaptionStyleSpec;
    let spec = CaptionStyleSpec { font_size: 56, color: "#FFFFFF".into(), reveal: RevealMode::WordByWord };
    lines.extend(build_caption_edl_lines(captions, &spec, "mobile"));
}
```

Delete the old `for caption in captions { lines.extend([... "*** Insert Caption" ...]) }` block.

- [ ] **Step 5: Run the new regression test + the full short-form suite**

Run: `cargo test -p montage-core short_form_captions_obey_readability -- --nocapture`
Expected: PASS.
Run: `cargo test -p montage-core scene_aware_short_form short_form_review -- --nocapture`
Expected: PASS. **If a pre-existing test asserts exact old cue text/boundaries** (e.g. `caption_plan[0]["word_timings"][0]["text"]` style assertions), update its expected values to the readability-model output — this is the intended improvement. Tests asserting placement/style/structure must remain unchanged; if one of those breaks, the rewire changed behavior it shouldn't have — fix the code, not the test.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/scene_aware_short_form.rs
git commit -m "refactor(caption): route short-form planner through shared readability + EDL core

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 8: `plan_captions` MCP tool — `run()`

**Files:**
- Create: `crates/core/src/montage_mcp/tools/plan_captions.rs`
- Modify: `crates/core/src/montage_mcp/tools/mod.rs`

- [ ] **Step 1: Register the module**

In `crates/core/src/montage_mcp/tools/mod.rs`, add (alphabetical, near the other `plan_*`):

```rust
pub mod plan_captions;
```

- [ ] **Step 2: Write the failing test**

Create `crates/core/src/montage_mcp/tools/plan_captions.rs` with the test module first (mirrors the sidecar-writing test pattern from `plan_scene_aware_short_form.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::montage_mcp::context::McpToolCtx;

    fn write_whisper(root: &std::path::Path, asset: &str, data: serde_json::Value) {
        let path = root.join("index").join("whisper").join(format!("{asset}.json"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
            "indexer": "whisper", "asset_id": asset, "data": data,
        })).unwrap()).unwrap();
    }

    #[test]
    fn long_form_plan_emits_bottom_captions_under_cps_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let asset = "raw/episode.mp4";
        write_whisper(dir.path(), asset, serde_json::json!({
            "segments": [{
                "start_s": 0.0, "end_s": 1.0,
                "text": "absolutely incredible breakthrough today",
                "words": [
                    {"text": "absolutely", "start_s": 0.0, "end_s": 0.25},
                    {"text": "incredible", "start_s": 0.25, "end_s": 0.5},
                    {"text": "breakthrough", "start_s": 0.5, "end_s": 0.75},
                    {"text": "today", "start_s": 0.75, "end_s": 1.0}
                ]
            }]
        }));
        let ctx = McpToolCtx { project_root: dir.path().to_path_buf() };
        let out = run(PlanCaptionsArgs {
            asset_id: asset.into(), clip_id: "clip-1".into(),
            format: "long_form".into(), mood: "minimal_cinematic".into(),
        }, ctx).unwrap();
        let body: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(body["caption_plan"].as_array().unwrap().len() >= 2, "over-fast speech should split");
        assert!(body["edl_fragment"].as_str().unwrap().contains("*** Insert Caption"));
        assert_eq!(body["format"], "long_form");
    }

    #[test]
    fn missing_transcript_is_a_clear_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = McpToolCtx { project_root: dir.path().to_path_buf() };
        let err = run(PlanCaptionsArgs {
            asset_id: "raw/none.mp4".into(), clip_id: "c".into(),
            format: "long_form".into(), mood: "minimal_cinematic".into(),
        }, ctx).unwrap_err();
        assert!(err.to_lowercase().contains("transcript") || err.to_lowercase().contains("index"));
    }
}
```

NOTE (resolved): `McpToolCtx` has a single public field `project_root: PathBuf`. Build it in tests with the direct struct literal `McpToolCtx { project_root: dir.path().to_path_buf() }` (this is the pattern used by `crates/core/src/montage_mcp/tools/apply_episode_spans.rs`). No `for_test` helper exists or is needed.

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p montage-core plan_captions -- --nocapture`
Expected: FAIL — `run`, `PlanCaptionsArgs` not found.

- [ ] **Step 4: Implement `run()`**

Add above the test module in `plan_captions.rs`:

```rust
//! `plan_captions` — format-agnostic caption planner. Reads the whisper
//! transcript sidecar, segments via the readability model for the requested
//! format, applies the (format, mood) style, and returns CaptionRecommendations
//! plus a reviewable `*** Insert Caption` EDL fragment. Read-only; apply with
//! apply_edl after inspection. Never burns captions into the picture.

use montage_index::{read_sidecar, SidecarError};
use montage_proto::index::AssetId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::montage_mcp::context::McpToolCtx;
use crate::caption::edl::build_caption_edl_lines;
use crate::caption::planner::{plan, LowerSafeZoneStrategy};
use crate::caption::readability::{lint, segment, CaptionFormatProfile, InputWord};
use crate::caption::styles::{resolve_style, CaptionFormat, CaptionMood};

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct PlanCaptionsArgs {
    /// Project-relative source asset id, e.g. raw/episode.mp4.
    pub asset_id: String,
    /// Timeline clip uuid/name used in EDL anchors.
    pub clip_id: String,
    /// Caption format: "short_form" | "long_form" | "accessibility".
    pub format: String,
    /// Mood register: "minimal_cinematic" | "active_pop".
    pub mood: String,
}

pub fn run(args: PlanCaptionsArgs, ctx: McpToolCtx) -> Result<String, String> {
    if args.asset_id.trim().is_empty() {
        return Err("plan_captions: asset_id must be non-empty.".into());
    }
    let format = parse_format(&args.format)?;
    let mood = parse_mood(&args.mood)?;
    let profile = match format {
        CaptionFormat::ShortForm => CaptionFormatProfile::short_form(),
        CaptionFormat::LongForm => CaptionFormatProfile::long_form(),
        CaptionFormat::Accessibility => CaptionFormatProfile::accessibility(),
    };

    let asset = AssetId::new(args.asset_id.clone());
    let transcript = match read_sidecar(&ctx.project_root, "whisper", &asset) {
        Ok(sidecar) => sidecar.get("data").cloned().unwrap_or(serde_json::Value::Null),
        Err(SidecarError::NotFound { .. }) => {
            return Err("plan_captions: no transcript index for this asset. Run indexing (whisper) first; captions need word timings.".into());
        }
        Err(err) => return Err(format!("plan_captions: failed to read whisper sidecar: {err}")),
    };

    let words = words_from_transcript(&transcript);
    if words.is_empty() {
        return Err("plan_captions: transcript index is empty for this asset; nothing to caption.".into());
    }

    let cues = segment(&words, &profile);
    let lint_proposals = lint(&cues, &profile);
    let recs = plan(&cues, &LowerSafeZoneStrategy);
    let spec = resolve_style(format, mood);
    let safe_area = if format == CaptionFormat::ShortForm { "mobile" } else { "standard" };

    let mut lines = vec!["*** Begin EDL".to_string()];
    lines.extend(build_caption_edl_lines(&recs, &spec, safe_area));
    lines.push("*** End EDL".to_string());
    let edl_fragment = lines.join("\n") + "\n";

    let body = serde_json::json!({
        "asset_id": args.asset_id,
        "clip_id": args.clip_id,
        "format": args.format,
        "mood": args.mood,
        "style": spec,
        "caption_plan": recs,
        "readability_lint": lint_proposals,
        "edl_fragment": edl_fragment,
        "verification_plan": [
            "Confirm no cue exceeds the 17 CPS reading ceiling.",
            "Confirm captions sit in the lower safe zone and clear of faces.",
            "Inspect the timeline diff, then render and check the artifact frame."
        ],
    });
    serde_json::to_string_pretty(&body).map_err(|e| format!("plan_captions: serialization failed: {e}"))
}

fn parse_format(s: &str) -> Result<CaptionFormat, String> {
    match s.trim() {
        "short_form" => Ok(CaptionFormat::ShortForm),
        "long_form" => Ok(CaptionFormat::LongForm),
        "accessibility" => Ok(CaptionFormat::Accessibility),
        other => Err(format!("plan_captions: unknown format {other:?}; use short_form|long_form|accessibility.")),
    }
}

fn parse_mood(s: &str) -> Result<CaptionMood, String> {
    match s.trim() {
        "minimal_cinematic" => Ok(CaptionMood::MinimalCinematic),
        "active_pop" => Ok(CaptionMood::ActivePop),
        other => Err(format!("plan_captions: unknown mood {other:?}; use minimal_cinematic|active_pop.")),
    }
}

fn words_from_transcript(transcript: &serde_json::Value) -> Vec<InputWord> {
    let mut out = Vec::new();
    let Some(segments) = transcript.pointer("/segments").and_then(|v| v.as_array()) else {
        return out;
    };
    for seg in segments {
        let seg_start = num(seg, "start_s", num(seg, "start", 0.0));
        let seg_end = num(seg, "end_s", num(seg, "end", seg_start));
        match seg.get("words").and_then(|v| v.as_array()) {
            Some(ws) if !ws.is_empty() => {
                for w in ws {
                    let text = w.get("text").or_else(|| w.get("word")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                    if text.is_empty() { continue; }
                    out.push(InputWord { text, start_s: num(w, "start_s", num(w, "start", seg_start)), end_s: num(w, "end_s", num(w, "end", seg_end)) });
                }
            }
            _ => {
                let text = seg.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if !text.is_empty() {
                    out.push(InputWord { text, start_s: seg_start, end_s: seg_end });
                }
            }
        }
    }
    out
}

fn num(v: &serde_json::Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(|x| x.as_f64()).unwrap_or(default)
}

pub const DESCRIPTION: &str = "\
Build a read-only, format-aware caption plan for one clip from its transcript \
index. Segments transcript words to a reading-speed ceiling (<=17 CPS) and the \
per-format characters-per-line / line-count targets (short_form, long_form, or \
accessibility), applies a (format, mood) style, and returns caption \
recommendations, a readability lint, and a reviewable Insert Caption EDL \
fragment. Apply separately with apply_edl after inspection. Never burns captions \
into the picture.";
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p montage-core plan_captions -- --nocapture`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/montage_mcp/tools/plan_captions.rs crates/core/src/montage_mcp/tools/mod.rs
git commit -m "feat(caption): add plan_captions MCP tool run() (format-aware caption planner)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 9: Expose `plan_captions` on the MCP server

**Files:**
- Modify: `crates/core/src/montage_mcp/mod.rs`

- [ ] **Step 1: Add the import**

Near the other tool imports (after the `plan_scene_aware_short_form` import at line ~108):

```rust
use crate::montage_mcp::tools::plan_captions::{self, PlanCaptionsArgs};
```

- [ ] **Step 2: Add the `#[tool]` method**

Inside the same `#[tool_router]`/`impl` block as `plan_scene_aware_short_form` (immediately after that method, ~line 1128), add:

```rust
/// `plan_captions` — format-aware, read-only caption planner.
#[tool(
    description = "\
Build a read-only, format-aware caption plan for one clip from its transcript \
index. Segments transcript words to a <=17 CPS reading ceiling with per-format \
characters-per-line targets (short_form|long_form|accessibility), applies a \
(format, mood) style, and returns caption recommendations, a readability lint, \
and a reviewable Insert Caption EDL fragment. Apply with apply_edl after \
inspection. Never burns captions into the picture.",
    annotations(read_only_hint = true)
)]
pub async fn plan_captions(
    &self,
    args: Parameters<PlanCaptionsArgs>,
) -> Result<String, ErrorData> {
    plan_captions::run(args.0, McpToolCtx::resolve())
        .map_err(|msg| ErrorData::invalid_params(msg, None))
}
```

- [ ] **Step 3: Build and verify the tool is exposed**

Run: `cargo build -p montage-core`
Expected: builds clean.

- [ ] **Step 4: Verify the capability/catalog tests still pass (and include the new tool if they enumerate tools)**

Run: `cargo test -p montage-core capability_manifest skill_catalog -- --nocapture`
Expected: PASS. If a test asserts an exact set/count of MCP tools, add `plan_captions` to its expected list (this is the intended addition). If it asserts the catalog is internally consistent (every allowlisted tool exists), it should pass once Task 10 adds the allowlist entry — run it again after Task 10.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/montage_mcp/mod.rs
git commit -m "feat(caption): expose plan_captions on the in-process MCP server

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 10: Update the caption-director skill

**Files:**
- Modify: `video_editing_transcripts/knowledge/captions/SKILL.md`

- [ ] **Step 1: Add `plan_captions` to the allowlist**

In the frontmatter `tools_allowlist`, add `- plan_captions` directly under `- plan_scene_aware_short_form`.

- [ ] **Step 2: Document the general tool in the "Tools you'll use" section**

After the `plan_scene_aware_short_form` bullet (line ~115), add:

```markdown
- `plan_captions` — format-aware caption planner for **any** format
  (`format: short_form|long_form|accessibility`, `mood: minimal_cinematic|active_pop`).
  Segments to the ≤17 CPS reading ceiling and per-format chars/line targets, applies a
  mood style, and returns recommendations + a readability lint + an Insert Caption EDL
  fragment. Use this for long-form/cinematic; `plan_scene_aware_short_form` remains the
  scene-aware choice for vertical short-form.
```

- [ ] **Step 3: Commit**

```bash
git add video_editing_transcripts/knowledge/captions/SKILL.md
git commit -m "docs(caption): add plan_captions to caption-director skill allowlist + tools

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

Note: `video_editing_transcripts/` is gitignored; if `git add` reports the path is ignored, use `git add -f video_editing_transcripts/knowledge/captions/SKILL.md` (the knowledge deliverables are intentionally tracked-by-force, or skip the commit and leave it as a working-tree change — confirm with the user which they prefer).

---

## Task 11: Full workspace gate

**Files:** none (verification only).

- [ ] **Step 1: Format**

Run: `cargo fmt --all`
Then: `cargo fmt --all -- --check`
Expected: no diff.

- [ ] **Step 2: Clippy (warnings are errors)**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean. Fix any lint in the new modules (common: needless clones in `segment`'s candidate building — acceptable for clarity, but silence with a local `#[allow(clippy::...)]` only if clippy objects and the rewrite hurts readability).

- [ ] **Step 3: Full test suite**

Run: `cargo test --workspace`
Expected: PASS. This is the real regression guard for the type move + short-form rewire.

- [ ] **Step 4: Commit any fmt/lint fixups**

```bash
git add -A
git commit -m "chore(caption): fmt + clippy fixups for caption module

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 12: Proof renders (manual, human-in-the-loop)

**Files:** none (operating montage against real footage). This is the close-the-loop step; it is interactive and depends on the local environment + the external drive. Do NOT mark the feature done until the user signs off.

Test footage: `/Volumes/Explicit's Hard Drive/` — `Short6_VCTake.mp4` (36s vertical) and an Episode clip (use the first ~60s of `Episode1...`).

- [ ] **Step 1: Confirm environment**

Run: `echo "${ANTHROPIC_API_KEY:+set}"` (expect `set`) and `ls "/Volumes/Explicit's Hard Drive/" | head` (expect the clips).
If the API key is unset, stop and ask the user to provide it (the agent loop needs it).

- [ ] **Step 2: Short-form regression render (existing path)**

Drive montage to produce the short-form caption render for `Short6_VCTake.mp4` using `plan_scene_aware_short_form` → `apply_edl` → `start_render` → `poll_render`, exactly as the existing short-form flow does. Capture the output path. This proves the rewire didn't break short-form.

- [ ] **Step 3: Resolve the long-form transcript dependency**

The Episode needs a whisper transcript sidecar. Try montage's local indexing on a ~60s Episode slice. Verify a sidecar appears at `index/whisper/<asset>.json`.
**Fallback if local Whisper indexing is not runnable:** import an `.srt`/`.vtt` for the slice via the existing import path, OR hand-write a minimal whisper sidecar JSON (`{"segments":[{"start_s","end_s","text","words":[...]}]}`) from a transcript you produce, so `plan_captions` has word timings. Report which path you used.

- [ ] **Step 4: Long-form render — both moods**

Call `plan_captions` with `format: long_form` twice — `mood: minimal_cinematic` and `mood: active_pop` — on the Episode slice. For each: `apply_edl` the returned `edl_fragment` → `start_render` → `poll_render`. Capture both output paths.

- [ ] **Step 5: Inspect and present to the user**

For each render, inspect a representative frame (caption present, inside safe area, not occluding faces, readable) — e.g. extract a frame with `ffmpeg` and Read it. Present the user the output file paths for the Short6 render and the two Episode renders (minimal-cinematic vs active-pop) so they can open them, plus the inspected frames. Summarize: cue count, any readability-lint proposals, CPS observed.

- [ ] **Step 6: Capture the verdict**

Ask the user which long-form register to keep and whether the captions meet their pro-editor bar. Record the decision. Iterate on the readability/style params if rejected. The feature is done only on user sign-off.

---

## Self-review

**Spec coverage:**
- Gap #1 readability/segmentation → Tasks 1–2. ✅
- Gap #2 cross-format planning (extract + strategy) → Tasks 4, 5, 7 + Task 8 tool. ✅
- Gap #3 styling registry, 2 moods, legibility floor → Task 3 (+ render-invariant outline/shadow noted). ✅
- Short-form rewiring / parity → Task 7. ✅
- New `plan_captions` tool + MCP exposure → Tasks 8, 9. ✅
- Skill update → Task 10. ✅
- Data flow (long-form proof) + both moods + Short6 regression → Task 12. ✅
- Non-destructive proposal contract → Task 2 (`lint` proposals), Task 8 (read-only tool). ✅
- Whisper-indexing risk + `.srt` fallback → Task 12 Step 3. ✅
- Tests per spec §6 → embedded in Tasks 1–9; catalog test in Task 9. ✅

**Placeholder scan:** No "TBD"/"handle edge cases"/"similar to". Two explicit verify-against-codebase notes (Task 5 field names; Task 8 test-context constructor) are intentional guardrails, each with the concrete fallback action. ✅

**Type consistency:** `InputWord` (readability input) vs `CaptionWordTiming` (planning/EDL type) are deliberately distinct; `plan()` converts between them (Task 5). `CaptionStyleSpec` produced by `resolve_style` (Task 3) is consumed by `build_caption_edl_lines` (Task 6) and `plan_captions::run` (Task 8) — field names `font_size`/`color`/`reveal` consistent across all three. `RevealMode` defined in readability (Task 1), used by styles + edl. `CaptionFormat`/`CaptionMood` defined in styles (Task 3), parsed in the tool (Task 8). Consistent. ✅
