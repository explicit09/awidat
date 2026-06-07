# Native Caption Rendering + Lower-Third-Safe Placement — Design (Captions Iteration 2)

**Date:** 2026-06-04
**Status:** Approved (brainstorming) → ready for implementation plan
**Feature:** captions (iteration 2)
**Branch base:** `feat/montage-editorial-upgrades`
**Builds on:** iteration 1 spec `2026-06-04-caption-readability-and-cross-format-design.md`

---

## 1. Goal

Make **montage itself** render captions correctly (no external/libass workaround) using the
industry-standard subtitle engine, and place captions so they **clear an existing
lower-third / busy bottom region**. Close the loop with montage-native proof renders the
user signs off on.

Two concrete problems from iteration 1's proof render motivate this:
- `montage render` produced an **invalid ffmpeg `filter_complex`** for whole-cue captions
  because they fell through to the fragile `drawtext` path (the libass path required word
  timings). Render aborted.
- Captions placed bottom-center **collided with the Episode's burned-in lower-third banner**.

## 2. Background (verified against the codebase)

- `crates/render/src/ass.rs` is a working **libass/ASS** path (`subtitles=` filter) with
  outline/shadow, wrapping, and `Alignment`+`MarginV` positioning via `CaptionLayoutProfile`.
  It is the industry-standard subtitle substrate (same engine VLC/mpv/Aegisub use).
- `is_libass_eligible(title)` currently returns true only when `!word_timings.is_empty()`
  **and** `role ∈ {caption, captions, subtitle, subtitles}`. So **whole-cue captions
  (no word timings) are NOT ASS-eligible** and fall back to `drawtext`.
- `format_title_filter` routes ASS-eligible titles to `ffmpeg_subtitles_filter_arg`; all
  others to `format_drawtext_filter`. The `drawtext` multi-filter chain has a separate
  pre-existing bug (filed as a chip) — **out of scope here**; we route captions away from it.
- `CaptionLayoutProfile::for_title` maps `title.safe_area`:
  `"mobile"|"9:16"|"vertical"` → `margin_v_bottom=216`; everything else → `162`
  (against `ASS_REFERENCE_HEIGHT=1080`).
- The `Insert Caption` EDL op already carries a `safe_area: String` field (parser +
  `EdlOp::InsertCaption` + `apply` + `TitlePlan.safe_area`). **No new EDL field is needed.**
- The short-form planner (`scene_aware_short_form.rs`) already derives `safe_text_zones` /
  `busy_regions` from the `composition` sidecar and chooses caption placement from them.

## 3. Architecture

Industry-standard substrate + reuse the existing `safe_area`→margin mechanism for placement.
Cross-layer but cohesive ("native, well-placed caption rendering").

### 3.1 Render: all captions use ASS (`crates/render/src/ass.rs`)

Change `is_libass_eligible` so any `role ∈ {caption,captions,subtitle,subtitles}` is ASS-eligible
**regardless of `word_timings`**:
- with word timings → karaoke/word-by-word ASS (existing behavior),
- without → a plain whole-cue ASS dialogue event.

`build_ass_document` must already handle (or be extended minimally to handle) the no-word-timings
case by emitting one `Dialogue` line with the full text. Plain `title` roles (non-caption) keep
the drawtext path unchanged.

### 3.2 Render: lower-third-safe margin profile (`ass.rs`)

Extend `CaptionLayoutProfile::for_title` with a raised profile selected by `safe_area="lower_third"`:
- `"lower_third"` → `margin_v_bottom = 300` (≈28% of `ASS_REFERENCE_HEIGHT=1080`), clearing a
  typical lower-third band; same `margin_l/r` and `max_chars_per_line` as the standard profile.
- Existing `"mobile"` (216) and default `"standard"` (162) profiles unchanged.

### 3.3 Planner: composition-aware vertical inset (`plan_captions`)

`plan_captions` gains opportunistic composition awareness:
- Read the `composition` (and/or `shot`) sidecar if present (same `read_sidecar` pattern as the
  whisper read; absent → skip, no error).
- Decide **one `safe_area` for the whole call**: if a **busy bottom region** is detected anywhere
  (reuse the existing short-form busy-region/safe-zone logic, factored into a shared helper so both
  planners use one implementation — DRY), use `safe_area = "lower_third"` for all caption cues in
  the call; otherwise the per-format default (`"standard"` for long-form/accessibility; `"mobile"`
  is short-form only and not produced here). This keeps `build_caption_edl_lines`'s single
  `safe_area` parameter unchanged.
- **Guaranteed fallback:** with no composition data, long-form defaults to `"standard"`.
  (A future iteration may make the default itself lower-third-safe; v1 keeps `"standard"` as the
  floor and only raises on detection.)

The detection reliability is explicitly **validated at render review** (§6): if composition
analysis does not reliably flag the lower-third on the test Episode, we record that and rely on
the default / a forced `"lower_third"`.

### 3.4 EDL / apply

Unchanged ops and unchanged `build_caption_edl_lines` signature. `plan_captions` decides one
`safe_area` for the call (§3.3) and passes it to `build_caption_edl_lines` as today; every emitted
`+ safe_area:` line carries that value.

## 4. Data flow

`plan_captions` (whisper + optional composition → per-cue `safe_area`) →
`*** Insert Caption … + safe_area: lower_third|standard` →
`apply_edl` → `TitlePlan{ role:"caption", safe_area }` →
`is_libass_eligible` = true → `render_ass_file` (`MarginV` from `CaptionLayoutProfile`) →
ffmpeg `subtitles=` (libass) → rendered mp4.

## 5. Error handling / non-destructive contract

- Missing composition sidecar → fall back to default placement; never error.
- Captions remain non-destructive graph nodes; ASS files are render-time artifacts.
- Unknown/empty `safe_area` → standard profile (existing default-arm behavior).
- Non-caption titles untouched (drawtext path unchanged; its bug stays out of scope).

## 6. Testing

- **`ass.rs` unit:** `is_libass_eligible` true for a caption with empty `word_timings`;
  `CaptionLayoutProfile` returns the raised `margin_v_bottom` for `safe_area="lower_third"` and
  the standard value otherwise; `build_ass_document` for a no-word-timings caption emits exactly
  one `Dialogue` line with the full text.
- **Planner unit:** with a composition fixture showing a busy bottom region, `plan_captions`
  emits `safe_area="lower_third"`; with no composition data it emits `"standard"`. The shared
  busy-region helper has a direct unit test; short-form behavior stays green (parity).
- **EDL unit:** `build_caption_edl_lines` emits the per-cue `safe_area`.
- **Regression:** full `montage-core` + `montage-render` suites green (scoped gate per the disk
  constraint memory).
- **End-to-end (manual proof, the sign-off):** render the long-form Episode (minimal_cinematic)
  **through `montage render`** — captions appear via libass, wrapped, outlined, and raised clear of
  the lower-third — and the Short6 short-form regression. User reviews real frames and signs off.

## 7. Scope boundaries (YAGNI)

**In:** ASS routing for all caption roles; one raised margin profile; composition-aware inset with
fallback; per-cue `safe_area` emission; montage-native proof renders (Episode minimal + Short6).

**Out:** the general `drawtext` filter-chain bug (separate chip); the EDL `text` quote round-trip
fix (separate chip); continuous/pixel-precise margins (discrete profiles suffice for v1); true
graphic-overlay detection beyond the existing composition busy-region signal; new caption moods;
short-form re-architecture.

## 8. Risks / dependencies

- **Composition detection may not flag burned-in lower-thirds reliably** (the indexer is tuned for
  framing/negative-space). Mitigation: the `"standard"` default is the floor; `"lower_third"` can
  be forced; reliability is judged at render review.
- **Short6 render needs short-form sidecars.** Index the minimum the short-form caption path needs
  (whisper + composition); other recommendations may lack data — acceptable for a caption-focused
  regression render.
- **Disk constraint** (see `disk_space_full_build_constraint` memory): build/test scoped to
  `montage-core` + `montage-render`, `CARGO_INCREMENTAL=0`.
- Whisper local pipeline already warmed in iteration 1; reuse the `capproof` project + transcript.

## 9. Open items for review time

- Final pixel value for the `"lower_third"` `margin_v_bottom`.
- Whether composition detection earns its keep, or the forced/default inset is enough.
