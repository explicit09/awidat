# Native Caption Rendering + Lower-Third-Safe Placement — Implementation Plan (Captions Iteration 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make awidat render all captions through the libass/ASS subtitle engine (fixing the whole-cue render failure) and place captions clear of a busy bottom region / lower-third.

**Architecture:** (1) `crates/render/src/ass.rs` — route every `role=caption` overlay to ASS regardless of word timings, emitting a whole-cue Dialogue when there are no word timings; add a `lower_third` margin profile. (2) `crates/core/src/awidat_mcp/tools/plan_captions.rs` — read the `composition` sidecar opportunistically and emit `safe_area="lower_third"` when the bottom region is busy, else `"standard"` (reusing the short-form composition parser). (3) Prove via awidat-native renders.

**Tech Stack:** Rust (`awidat-render`, `awidat-core`), ffmpeg `subtitles=` (libass), serde_json. Specs: `docs/superpowers/specs/2026-06-04-caption-native-render-and-placement-design.md`.

**Conventions (per project + disk memory):**
- Disk-scoped commands only: `CARGO_INCREMENTAL=0 cargo test -p <crate> <name>`. Never `--workspace` (vendored codex-* crates overflow disk).
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Render tests: `cargo test -p awidat-render <name>`. Core tests: `cargo test -p awidat-core <name>`.

---

## File structure

- Modify: `crates/render/src/ass.rs` — `is_libass_eligible` (caption→ASS always), `build_dialogue_lines` (whole-cue branch), `CaptionLayoutProfile::for_title` (`lower_third` arm).
- Modify: `crates/core/src/scene_aware_short_form.rs` — make `composition_zones` + `caption_placement_from_str` `pub(crate)` (reuse, no behavior change).
- Modify: `crates/core/src/awidat_mcp/tools/plan_captions.rs` — composition-aware `safe_area`.

---

## Task 1: Route all captions to ASS + whole-cue Dialogue

**Files:** Modify `crates/render/src/ass.rs`

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)] mod tests` in `ass.rs` (it exists; reuse its `TitlePlan` construction style — see the existing `hex_to_ass_color` test and `build_ass_document_for_test`). If the tests module lacks a TitlePlan builder, add this helper inside the module:

```rust
fn caption_title(text: &str, word_timings: Vec<crate::timeline::CaptionWordTiming>) -> crate::timeline::TitlePlan {
    crate::timeline::TitlePlan {
        text: text.into(),
        start_s: 1.0,
        end_s: 3.0,
        position: crate::timeline::TitlePosition::Bottom,
        font_size: 44,
        color: "#FFFFFF".into(),
        font_weight: crate::timeline::TitleWeight::Normal,
        animation: crate::timeline::TitleAnimation::None,
        phases: None,
        reveal: crate::timeline::TextReveal::None,
        role: "caption".into(),
        safe_area: Some("standard".into()),
        ..Default::default()
    }
}
```

NOTE: `TitlePlan` may not derive `Default`. If it does not, construct it fully by copying the field list from `crates/render/src/timeline.rs` (`pub struct TitlePlan`, ~line 4274) — every field, no `..Default::default()`. Verify the field set against the struct before running.

Tests:

```rust
#[test]
fn caption_without_word_timings_is_libass_eligible() {
    let t = caption_title("hello world", vec![]);
    assert!(is_libass_eligible(&t), "whole-cue captions must use libass, not drawtext");
}

#[test]
fn whole_cue_caption_emits_one_dialogue_with_full_text() {
    let t = caption_title("the rise of solo entrepreneurs", vec![]);
    let doc = build_ass_document(&t);
    let dialogues: Vec<&str> = doc.lines().filter(|l| l.starts_with("Dialogue:")).collect();
    assert_eq!(dialogues.len(), 1, "exactly one whole-cue dialogue, got: {dialogues:?}");
    // text present (libass may insert \\N wrap markers, so check word fragments)
    assert!(doc.contains("rise") && doc.contains("entrepreneurs"));
    // no karaoke timing tags for a whole-cue line
    assert!(!dialogues[0].contains("\\k"), "whole-cue must not emit karaoke \\k tags");
}
```

- [ ] **Step 2: Run — verify FAIL**

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-render caption_without_word_timings_is_libass_eligible whole_cue_caption -- --nocapture`
Expected: FAIL — `is_libass_eligible` returns false for empty word_timings, and `build_dialogue_lines` returns no dialogue (0 lines).

(If `cargo test` rejects two filters, run each separately: it accepts one substring filter.)

- [ ] **Step 3: Fix `is_libass_eligible`**

Replace the body of `is_libass_eligible` (currently requires non-empty word_timings) with:

```rust
pub(crate) fn is_libass_eligible(title: &TitlePlan) -> bool {
    // Captions/subtitles always render via libass — the industry-standard
    // subtitle engine — whether word-timed (karaoke) or whole-cue. Plain
    // `title` overlays keep the drawtext path.
    matches!(
        title.role.as_str(),
        "caption" | "captions" | "subtitle" | "subtitles"
    )
}
```

- [ ] **Step 4: Add the whole-cue branch in `build_dialogue_lines`**

In `build_dialogue_lines`, replace the early `if words.is_empty() { return Vec::new(); }` (around line 287) with a whole-cue fallback that emits one Dialogue using the title text, wrapped to the layout width (mirroring `wrap_plain_subtitle_text`):

```rust
    if words.is_empty() {
        let text = title.text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        let layout = CaptionLayoutProfile::for_title(title);
        let wrapped = wrap_caption_text(&escape_ass_text(text), layout.max_chars_per_line);
        let start = format_ass_time(title.start_s.max(0.0));
        let end = format_ass_time(title.end_s.max(title.start_s));
        return vec![format!("Dialogue: 0,{start},{end},Caption,,0,0,0,,{wrapped}")];
    }
```

Add a `wrap_caption_text` helper near `wrap_plain_subtitle_text` (or reuse `wrap_plain_subtitle_text` if its 32-char hardcode is acceptable — but the layout width differs, so add the parameterized version):

```rust
/// Word-wrap already-escaped caption text to <= max_chars_per_line per visible
/// line, inserting ASS `\N` hard breaks. Mirrors wrap_plain_subtitle_text but
/// honors the layout width.
fn wrap_caption_text(escaped: &str, max_chars_per_line: usize) -> String {
    let mut out = String::new();
    let mut visible = 0usize;
    for word in escaped.split_whitespace() {
        let n = word.chars().count();
        if visible > 0 && visible + 1 + n > max_chars_per_line {
            out.push_str("\\N");
            visible = 0;
        } else if visible > 0 {
            out.push(' ');
            visible += 1;
        }
        out.push_str(word);
        visible += n;
    }
    out
}
```

(`escape_ass_text` already exists and is used at line 307.)

- [ ] **Step 5: Run — verify PASS**

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-render ass -- --nocapture`
Expected: PASS — the 2 new tests plus existing ass tests (word-timed karaoke path untouched).

- [ ] **Step 6: Clippy + commit**

Run: `CARGO_INCREMENTAL=0 cargo clippy -p awidat-render --all-targets -- -D warnings 2>&1 | tail -5` → clean.
```bash
git add crates/render/src/ass.rs
git commit -m "feat(render): render all captions via libass, including whole-cue (no word timings)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Lower-third-safe margin profile

**Files:** Modify `crates/render/src/ass.rs`

- [ ] **Step 1: Write failing test**

Add to the `ass.rs` tests module:

```rust
#[test]
fn lower_third_safe_area_raises_bottom_margin() {
    let mut t = caption_title("x", vec![]);
    t.safe_area = Some("lower_third".into());
    let raised = CaptionLayoutProfile::for_title(&t);
    t.safe_area = Some("standard".into());
    let standard = CaptionLayoutProfile::for_title(&t);
    assert_eq!(raised.margin_v_bottom, 300, "lower_third clears the banner band");
    assert_eq!(standard.margin_v_bottom, 162, "standard unchanged");
    assert!(raised.margin_v_bottom > standard.margin_v_bottom);
}
```

NOTE: `CaptionLayoutProfile` and `for_title` are `pub(crate)`/private; the test is in-module so it can call them. If `CaptionLayoutProfile` fields are private (they are), access via the returned struct in-module is fine.

- [ ] **Step 2: Run — verify FAIL**

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-render lower_third_safe_area -- --nocapture`
Expected: FAIL — `"lower_third"` currently falls into the default arm (162), so `raised.margin_v_bottom == 162 != 300`.

- [ ] **Step 3: Add the `lower_third` arm**

In `CaptionLayoutProfile::for_title`, add a match arm before the default `_`:

```rust
            Some("lower_third") => Self {
                margin_l: 80,
                margin_r: 80,
                margin_v_bottom: 300, // ~28% of 1080: clears a typical lower-third banner
                margin_v_top: 54,
                max_chars_per_line: 32,
            },
```

(Keep the existing `"mobile"|"9:16"|"vertical"` arm and the default `_` → 162 arm.)

- [ ] **Step 4: Run — verify PASS**

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-render lower_third_safe_area -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Clippy + commit**

Run: `CARGO_INCREMENTAL=0 cargo clippy -p awidat-render --all-targets -- -D warnings 2>&1 | tail -5` → clean.
```bash
git add crates/render/src/ass.rs
git commit -m "feat(render): add lower_third caption safe-area margin profile

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: Expose the composition zone parser for reuse

**Files:** Modify `crates/core/src/scene_aware_short_form.rs`

Pure visibility change (no behavior change) so `plan_captions` can reuse the existing composition busy-region parser.

- [ ] **Step 1: Make the two helpers `pub(crate)`**

In `crates/core/src/scene_aware_short_form.rs`:
- Change `fn composition_zones(` (≈ line 901) to `pub(crate) fn composition_zones(`.
- Change `fn caption_placement_from_str(` (≈ line 955) to `pub(crate) fn caption_placement_from_str(`.

- [ ] **Step 2: Build + verify no behavior change**

Run: `CARGO_INCREMENTAL=0 cargo build -p awidat-core`
Expected: compiles. Then:
Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-core scene_aware_short_form -- --nocapture`
Expected: PASS unchanged.

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/scene_aware_short_form.rs
git commit -m "refactor(caption): expose composition_zones for reuse by plan_captions

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Composition-aware placement in `plan_captions`

**Files:** Modify `crates/core/src/awidat_mcp/tools/plan_captions.rs`

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)] mod tests` in `plan_captions.rs` (it already has `write_whisper`; add a `write_composition` helper). The composition sidecar shape mirrors what `composition_zones` reads — a `regions` array with `busy_regions` labels:

```rust
fn write_composition(root: &std::path::Path, asset: &str, data: serde_json::Value) {
    let path = root.join("index").join("composition").join(format!("{asset}.json"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec_pretty(&serde_json::json!({
        "indexer": "composition", "asset_id": asset, "data": data,
    })).unwrap()).unwrap();
}

#[test]
fn busy_bottom_region_raises_safe_area_to_lower_third() {
    let dir = tempfile::tempdir().unwrap();
    let asset = "raw/ep.mp4";
    write_whisper(dir.path(), asset, serde_json::json!({
        "words": [{"text": "hello", "start_s": 0.0, "end_s": 1.0}],
        "segments": [{"text": "hello", "start_s": 0.0, "end_s": 1.0}]
    }));
    write_composition(dir.path(), asset, serde_json::json!({
        "regions": [{"start_s": 0.0, "end_s": 60.0, "busy_regions": ["bottom"]}]
    }));
    let ctx = McpToolCtx { project_root: dir.path().to_path_buf() };
    let out = run(PlanCaptionsArgs { asset_id: asset.into(), clip_id: "c".into(),
        format: "long_form".into(), mood: "minimal_cinematic".into() }, ctx).unwrap();
    let body: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(body["edl_fragment"].as_str().unwrap().contains("+ safe_area: lower_third"),
        "busy bottom must raise captions: {}", body["edl_fragment"]);
}

#[test]
fn no_composition_defaults_to_standard_safe_area() {
    let dir = tempfile::tempdir().unwrap();
    let asset = "raw/ep.mp4";
    write_whisper(dir.path(), asset, serde_json::json!({
        "words": [{"text": "hello", "start_s": 0.0, "end_s": 1.0}],
        "segments": [{"text": "hello", "start_s": 0.0, "end_s": 1.0}]
    }));
    let ctx = McpToolCtx { project_root: dir.path().to_path_buf() };
    let out = run(PlanCaptionsArgs { asset_id: asset.into(), clip_id: "c".into(),
        format: "long_form".into(), mood: "minimal_cinematic".into() }, ctx).unwrap();
    let body: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(body["edl_fragment"].as_str().unwrap().contains("+ safe_area: standard"));
}
```

NOTE: confirm the composition fixture shape against `composition_zones` (`crates/core/src/scene_aware_short_form.rs` ~901): it reads a regions array, filters by `[start_s,end_s]` overlap, and collects labels from the given keys (`busy_regions`/`unsafe_text_zones`/`protected_regions`) via `collect_placements`. If the real reader expects a different container key than `regions`, match it — read `composition_zones` + `collect_placements` first and mirror exactly.

- [ ] **Step 2: Run — verify FAIL**

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-core busy_bottom_region_raises no_composition_defaults -- --nocapture`
Expected: FAIL — `run` currently emits `safe_area = if ShortForm {"mobile"} else {"standard"}` and never reads composition, so the busy-bottom test sees `standard`, not `lower_third`.

- [ ] **Step 3: Implement composition-aware safe_area**

In `plan_captions.rs` `run`, replace the current `safe_area` line:

```rust
    let safe_area = if format == CaptionFormat::ShortForm { "mobile" } else { "standard" };
```

with a composition-aware computation (place it after the transcript read, before building the EDL):

```rust
    // Opportunistic placement: if the source has a busy bottom region (e.g. a
    // burned-in lower-third), raise captions clear of it. Absent composition
    // data, fall back to the standard band. (short_form is rejected earlier.)
    let safe_area = if composition_bottom_busy(&ctx.project_root, &asset) {
        "lower_third"
    } else {
        "standard"
    };
```

Add the helper (private fn in `plan_captions.rs`):

```rust
fn composition_bottom_busy(project_root: &std::path::Path, asset: &AssetId) -> bool {
    use crate::scene_aware_short_form::{caption_placement_from_str, composition_zones};
    let composition = match read_sidecar(project_root, "composition", asset) {
        Ok(s) => s.get("data").cloned().unwrap_or(serde_json::Value::Null),
        Err(_) => return false,
    };
    // Scan the whole clip span for any busy bottom region.
    let busy = composition_zones(
        &composition,
        0.0,
        f64::MAX,
        &["busy_regions", "unsafe_text_zones", "protected_regions"],
    );
    busy.contains(&caption_placement_from_str("bottom").expect("bottom is a valid placement"))
}
```

NOTE: `composition_zones` returns `Vec<CaptionPlacement>` and `caption_placement_from_str("bottom")` returns `Some(CaptionPlacement::Bottom)`. Confirm `composition_zones`'s exact signature `(&Value, f64, f64, &[&str]) -> Vec<CaptionPlacement>` after Task 3 and adjust the call if it differs. `AssetId` and `read_sidecar` are already imported in this file.

- [ ] **Step 4: Run — verify PASS**

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-core plan_captions -- --nocapture`
Expected: PASS — both new tests plus the existing plan_captions tests (the existing long-form test has no composition sidecar → `standard`, still contains `*** Insert Caption`; verify it doesn't assert a conflicting safe_area — if it asserted `standard` implicitly it still holds).

- [ ] **Step 5: Clippy + commit**

Run: `CARGO_INCREMENTAL=0 cargo clippy -p awidat-core --all-targets -- -D warnings 2>&1 | tail -5` → clean.
```bash
git add crates/core/src/awidat_mcp/tools/plan_captions.rs
git commit -m "feat(caption): plan_captions raises safe_area to lower_third on a busy bottom region

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Scoped workspace gate

**Files:** none (verification).

- [ ] **Step 1: fmt**

Run: `cargo fmt --all` then `cargo fmt --all -- --check` → no diff (warnings about the unstable `imports_granularity` config are pre-existing and OK).

- [ ] **Step 2: clippy (the two touched crates)**

Run: `CARGO_INCREMENTAL=0 cargo clippy -p awidat-core -p awidat-render --all-targets -- -D warnings 2>&1 | tail -6` → clean.

- [ ] **Step 3: tests (the two touched crates)**

Run: `CARGO_INCREMENTAL=0 cargo test -p awidat-core -p awidat-render 2>&1 | grep -E "test result:|FAILED" | tail` → all `ok`, 0 failed.

- [ ] **Step 4: commit any fmt fixups**

```bash
git add -A && git commit -m "chore(caption): fmt/clippy fixups for iteration 2" -m "Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>" || echo "nothing to commit"
```

---

## Task 6: Awidat-native proof renders (manual, user sign-off)

**Files:** none. Reuses the `capproof` project + transcript on `/Volumes/Explicit's Hard Drive/capproof` from iteration 1. No `ANTHROPIC_API_KEY` needed (CLI path).

- [ ] **Step 1: Build the CLI**

Run: `CARGO_INCREMENTAL=0 cargo build -p awidat-cli --bin awidat`. Confirm `target/debug/awidat` is fresh.

- [ ] **Step 2: Index composition on the Episode slice**

The placement detection needs the `composition` sidecar. Run:
`./target/debug/awidat index "/Volumes/Explicit's Hard Drive/capproof" --indexer composition`
(First run pre-warms the composition-mcp env; if it hits the 20s MCP init timeout like whisper did in iteration 1, pre-warm with `cd python && uv run --package composition-mcp python -c "pass"` then retry, OR run the composition indexer's transcription-equivalent directly — but composition has no model download, so a retry after env sync should succeed.) If composition indexing cannot be made to run, proceed with no composition sidecar — `plan_captions` will default to `standard`, and you can still validate the ASS render; note this in the report.

- [ ] **Step 3: Produce the long-form caption EDL via the MCP tool**

`plan_captions` is an MCP tool (no CLI subcommand). Invoke it via the in-process MCP server, or — simplest, matching iteration 1 — write a throwaway example that calls `awidat_core::awidat_mcp::tools::plan_captions::run(...)`, prints `edl_fragment` to stdout, run it, and **delete the example before finishing** (it must not be committed; it trips clippy as a quick script). Generate the EDL for `format=long_form mood=minimal_cinematic`, save to `/tmp/ep1_min_v2.edl`. Confirm it contains `+ safe_area: lower_third` (if composition ran and flagged bottom) or `standard`.

- [ ] **Step 4: Apply + render through awidat**

```
cp "<proj>/project.otio.json" "<proj>/project.otio.json.bak"  # if not already clean
./target/debug/awidat apply-edl "<proj>" /tmp/ep1_min_v2.edl
./target/debug/awidat render "<proj>"
```
Expected: render SUCCEEDS (captions now go through libass, not the broken drawtext chain). Output mp4 under `<proj>/renders/`. If render still errors, capture the ffmpeg stderr log and report — do not fall back to the libass-direct workaround silently.

- [ ] **Step 5: Short6 short-form regression render**

Create/import Short6 into a project (`awidat new short6proof --import "/Volumes/Explicit's Hard Drive/Short6_VCTake.mp4" --no-index --at ...`, then copy the real file into `raw/` per iteration 1's symlink caveat). Index `whisper` and `composition` (the short-form caption placement needs them). Run `plan_scene_aware_short_form` (via MCP/example) → apply its EDL → `awidat render`. Short-form captions are word-timed → libass karaoke. Confirm render succeeds.

- [ ] **Step 6: Inspect + present**

Extract representative frames (`ffmpeg -ss <t> -i <out> -frames:v 1 frame.png`) — especially a long cue (wrap) and a moment over the lower-third (placement). Present the user: the awidat-native Episode minimal render (captions raised clear of the banner) and the Short6 render, with frames. Summarize cue count, observed placement (raised vs standard), and whether composition detection fired.

- [ ] **Step 7: Capture the verdict**

Ask the user to sign off or request another craft iteration. Done only on sign-off.

---

## Self-review

**Spec coverage:**
- §3.1 ASS routing for all captions + whole-cue dialogue → Task 1. ✅
- §3.2 lower_third margin profile (300) → Task 2. ✅
- §3.3 composition-aware inset with fallback (one safe_area per call) → Tasks 3–4. ✅
- §3.4 EDL unchanged, single safe_area param → Task 4 (passes computed safe_area to existing `build_caption_edl_lines`). ✅
- §6 testing (ass unit, planner unit, regression, e2e) → Tasks 1,2,4,5,6. ✅
- §8 disk-scoped builds → all tasks use `-p` + `CARGO_INCREMENTAL=0`. ✅
- Short6 short-form sidecar note → Task 6 Step 5. ✅

**Placeholder scan:** No "TBD/handle errors/etc." Verify-against-codebase notes (TitlePlan Default, composition fixture shape, composition_zones signature) are explicit guardrails with concrete fallback actions — acceptable. Task 6 is inherently manual (render + human judgment); its steps are concrete commands.

**Type consistency:** `safe_area` string values (`"mobile"`/`"standard"`/`"lower_third"`) consistent across `CaptionLayoutProfile::for_title` (Task 2) and `plan_captions` (Task 4). `composition_zones(&Value, f64, f64, &[&str]) -> Vec<CaptionPlacement>` + `caption_placement_from_str(&str) -> Option<CaptionPlacement>` reused in Task 4 exactly as exposed in Task 3. `is_libass_eligible(&TitlePlan) -> bool` and `build_dialogue_lines` signatures unchanged (Task 1). `wrap_caption_text(&str, usize) -> String` defined and used within Task 1.
