# Caption Emphasis Intelligence — Research Brief

**Date:** 2026-06-04
**Status:** Research brief (feeds a later brainstorm; not a spec, not a plan)
**Scope:** The agent auto-deciding which caption lines/words get the poppy "emphasis" look.

---

## 1. Premise

The agent already has full episode context — transcript, topics, editorial moments, audio
energy, gaze/shot/composition sidecars — all passed to the caption planner
(`plan_scene_aware_short_form.rs:38-54`). So the "intelligence" in *caption emphasis
intelligence* is the **agent's editorial judgment expressed in the skill**, not a Rust
keyword-detection algorithm. The genuine engineering work is therefore a **control surface**:
how the agent expresses per-line (and possibly per-word) emphasis intent, plus skill guidance
encoding the craft rules, plus whatever render plumbing word-level emphasis specifically needs.
Restraint ("emphasize 1-2 lines, not every line") is a *skill instruction*, not a Rust
rate-limiter — the model is the budget-keeper.

---

## 2. As-built map

### Per-cue style plumbing — VERIFIED, already exists end-to-end

Each `*** Insert Caption` op carries its **own** `style_json`, and the render path honors it
**per cue**:

- `build_caption_edl_lines` serializes the full spec onto every caption op:
  `crates/core/src/caption/edl.rs:31` → `let style_json = serde_json::to_string(spec)...`
  then `edl.rs:32` emits `+ style_json: {style_json}` *inside the per-caption loop*
  (`edl.rs:18` `for caption in recs`). **VERIFIED by direct read.**
- Parser keeps it: `crates/core/src/edl/parser.rs:1316-1328` parses `style_json` as
  `Option<serde_json::Value>`.
- Apply stores it: `crates/core/src/edl/apply.rs:8257-8258` writes it to effect metadata as
  `caption_style`.
- Render reads it per-effect: `crates/render/src/timeline.rs:3686-3688` reads `caption_style`
  from metadata per effect; `crates/render/src/ass.rs:160-198` derives style fields from
  `title.caption_style` independently for each `Dialogue`.

**Implication (VERIFIED):** an EDL that mixes an `emphasis`-styled cue with `word_pop` cues
already renders correctly today. The transport layer for *line-level* emphasis exists.

### The single-spec limitation — VERIFIED, this is the actual gap

Both call sites resolve **ONE** spec and apply it to **ALL** cues:

- MCP tool: `crates/core/src/awidat_mcp/tools/plan_captions.rs:75-78` resolves a single `spec`,
  then `plan_captions.rs:89` calls `build_caption_edl_lines(&recs, &spec, safe_area)` — one spec
  for every rec.
- Scene-aware: `crates/core/src/scene_aware_short_form.rs:613-615` hardcodes
  `resolve_preset("word_pop")` once, applied to all captions.
- The signature itself enforces it: `edl.rs:12-16`
  `build_caption_edl_lines(recs, spec: &CaptionStyleSpec, safe_area)` takes a single immutable
  `&CaptionStyleSpec`.

So the **plumbing supports per-cue specs but the call sites never produce more than one.** No
data structure currently carries per-cue emphasis intent:

- `CaptionRecommendation` (`crates/core/src/caption/types.rs:16-27`) has no
  emphasis/importance/salience field.
- `Cue` (`crates/core/src/caption/readability.rs:71-76`) has no emphasis field.
- `PlanCaptionsArgs` (`plan_captions.rs:18-31`) accepts only a single optional `preset` string
  for the whole call.

### What renders today: line-level vs word-level — VERIFIED

- **Line-level / whole-cue:** `crates/render/src/ass.rs:568-586` — one `Dialogue` per cue with
  motion + style from `title.caption_style`. Independent per cue. **Works today.**
- **Active-word-pop (per-word color):** `crates/render/src/ass.rs:547-559` wraps the active word
  in `{{\c{hi_col}{aw}}}word{{\c{primary_col}}}`, where `aw` = `active_word_anim()` emitting
  `\t(...)` transforms for Bounce/ScalePop/Shake (`ass.rs:466-477`). **VERIFIED by direct read:
  this branch emits ONLY `\c` (color) and `\t` (timed transform). No `\fs` (font size), no
  `\fscx/\fscy` (scale) is emitted per word.**
- **Per-word data model:** `CaptionWordTiming` (`crates/core/src/edl/op.rs:910-917`) has only
  `text`, `start_s`, `end_s` — no per-word emphasis/style flag.
- **Rich titles:** `InsertRichTitle` (`op.rs:620-636`) supports per-*segment* color/font_weight
  (`RichTextSegment`, `op.rs:895-906`), but that is whole-title segmentation, **not** per-word
  emphasis within a cue.

### The `emphasis` preset already exists — VERIFIED

`crates/core/src/caption/styles.rs:172-191` defines an `emphasis` preset (font ~92, box
background, bold, upper, yellow highlight, PopIn entrance + Bounce active-word). It is selectable
**globally** via `resolve_preset("emphasis")` (`styles.rs:129,132-195`) but there is **no
per-cue selection mechanism.** The preset ships in Phase 2.1; per-cue emphasis *intelligence* was
explicitly deferred to "a separate later sub-project" in
`docs/superpowers/specs/2026-06-04-caption-motion-design.md:94-106` — i.e. this brief.

---

## 3. Corpus principles (what the skill guidance should encode)

These are the craft rules for **WHICH** words/lines and the **restraint budget**, drawn from
`SKILL.md` and `_caption_excerpts.md`.

- **Target 1-2 words/lines that carry the line.** *"Lift the 1-2 words that carry the line
  (size, color, or a hold) — not every word. Reserve premium/animated treatment for hook lines
  and B-roll beats; plain clean captions for filler."* (`SKILL.md:89-91`); *"the 1-2 hook /
  keyword / payoff lines — not a whole video"* (`SKILL.md:123`).
- **Restraint is the whole game.** *"captions popping up every word is so distracting to the
  story... people appreciate the Simplicity of a good story."* (`_caption_excerpts.md:396`);
  *"Minimal by default. Reach for motion only when it earns its place."* (`SKILL.md:154`).
- **The knobs: size/scale, color, motion.** *"I have a more simple caption... and then I have
  some more like large, fun, poppy captions... my brand color background with some captions on
  top."* (`_caption_excerpts.md:596-597`). Motion slots: entrance (pop_in/slide_up/fade_in),
  active_word (bounce/scale_pop/shake), exit, continuous (`SKILL.md:140-144`).
- **Where it lands: B-roll beats and hook lines.** *"you really want to emphasize it on the
  B-roll moments"* (`_caption_excerpts.md:336`); *"Reserve premium/animated treatment for hook
  lines and B-roll beats."* (`SKILL.md:91`).
- **What gets lifted: payoff words, numbers, brand keywords, emotional/narrative turns.**
  *"highlight important keywords instead of just following auto captions on every word."*
  (`_caption_excerpts.md:800`).
- **Font style + color carry emotion.** *"bigger than the rest of your captions"*
  (`_caption_excerpts.md:635-637`); *"fonts set the tone... colors set emotions."*
  (`_caption_excerpts.md:698`).
- **Motion is premium but purposeful.** *"Animate to emphasize, never to decorate."*
  (`SKILL.md:131-134`).
- **Selection is judgment, not a formula.** *"changed the caption from the normal yellow and red
  to this white to emphasize"* — a deliberate, context-driven choice (`_caption_excerpts.md:507`).

**Corpus gaps the skill should acknowledge (not invent answers for):** no explicit rule on
digit/number auto-emphasis; no ranking of structural moments (intro vs climax) within a beat;
nothing on epistemic novelty (new term vs known); nothing on multi-language proper-noun handling;
nothing on emphasis-vs-reading-speed interaction (does emphasizing slow CPS?).

---

## 4. Effort split

### LINE-LEVEL emphasis — near-free (VERIFIED)

The render and EDL transport already honor a distinct `style_json` per cue (Section 2). Nothing
in the render path needs to change. The *only* work is upstream — letting the call sites produce
**more than one spec** across cues:

1. Add a per-cue emphasis signal to `CaptionRecommendation` (e.g. `is_emphasized: bool`),
   `types.rs:16-27`.
2. Change `build_caption_edl_lines` to pick the emphasis spec vs default spec per rec
   (`edl.rs:12-16`) — e.g. accept `(default_spec, emphasis_spec)` and branch on
   `caption.is_emphasized`.
3. Give the agent a way to set the flag (the control-surface decision — Section 5).

No new ASS code. No render changes. This is structurally a transport/wiring change.

### WORD-LEVEL emphasis — genuinely new render work (enumerated)

This is the real engineering. Concretely:

1. **Data model:** extend `CaptionWordTiming` (`op.rs:910-917`) with optional per-word style —
   e.g. `emphasis: bool`, or `emphasis_font_size: Option<u32>`, `emphasis_scale_pct:
   Option<f32>`, optional color override.
2. **Render emission:** in the active-word-pop branch (`ass.rs:547-559`), when a word is flagged,
   emit additional ASS override tags alongside the existing `\c`. Today that branch emits **only
   color + transform** (VERIFIED) — so font-size and scale are net-new:
   - font size: `{{\fs{emph}\c{hi_col}{aw}}}word{{\fs{base}\c{primary_col}}}`
   - scale: `{{\fscx{pct}\fscy{pct}...}}word{{\fscx100\fscy100}}`
3. **Plumbing:** thread per-word flags through parser/apply (`CaptionWordTiming` is serialized in
   `word_timings_json`, `edl.rs:39-42`) so flags survive the EDL round-trip.
4. **Reveal-mode coupling:** decide how per-word emphasis interacts with whole-cue vs
   word-by-word vs active-word-pop reveal modes (the whole-cue branch at `ass.rs:568-586` does
   not iterate words at all, so a different emission path is needed there).
5. **Skill guidance** for which words (Section 3) — same as line-level but finer-grained.

**Verdict:** line-level is near-free and verified. Word-level is the only place that buys new
render code, and it is non-trivial (data model + ASS emission + round-trip + reveal-mode
interaction). Recommend shipping line-level first, defer word-level.

---

## 5. Options matrix (control surface) + recommendation

How does the agent express per-cue emphasis intent? Four candidate surfaces:

| Option | Shape | Pros | Cons |
|---|---|---|---|
| **A. Flag on rec + arg on `plan_captions`** | `PlanCaptionsArgs.emphasis_line_indices: Vec<usize>`; tool sets `rec[i].is_emphasized`, emits emphasis spec for those | Smallest change; one tool; agent passes `[0,3,7]`; reuses existing emphasis preset; backwards-compatible (empty = today's behavior) | Indices are positional/brittle if cue set changes between calls; one-shot (revise = re-call) |
| **B. Separate `plan_caption_emphasis` tool** | New tool takes cue indices/time ranges, returns emphasis recs separately; agent merges before `apply_edl` | Clean separation of concerns; can carry its own rationale fields | Two tools to keep in sync; agent must merge two fragments; more surface area; index-aliasing risk across tools |
| **C. `emphasis_alternatives` on rec** | `CaptionRecommendation.emphasis_alternatives: Vec<(preset, rationale)>`; agent picks per cue when building `apply_edl` | No new tool; agent-native; mirrors `plan_emphasis` "recommended + alternates" prior art (`plan_emphasis.rs:110-131`) | Pushes EDL assembly into the agent; more agent work per cue; harder to QC uniformly |
| **D. Multi-tier proposal bucketing** | Model on `podcast_edit_proposal` (`podcast_edit_proposal.rs:50-64`): safe/review/risky with `requires_user_approval` per item | Strong propose-confirm story; prior art exists | Heavyweight for a styling decision; emphasis is low-risk, doesn't warrant per-item approval gating |

**RECOMMENDATION: Option A** for v1. It is the smallest change that exercises the
already-verified per-cue `style_json` path, keeps a single tool, and is naturally
backwards-compatible (no indices = current behavior). The agent expresses judgment by choosing
indices; Rust is pure transport. The positional-index brittleness is acceptable because the agent
builds indices against the same cue set it just received in the tool output (single round-trip).
Lift Option C's *rationale* idea by having the agent record its emphasis reasoning in the skill's
existing rationale-rules contract (`SKILL.md:181-185`), not as a new struct field.

---

## 6. Open questions for the brainstorm (human decisions)

1. **v1 = line-only, or line + word?** Render evidence says line-only is near-free and word-level
   is the only new render work. Recommend line-only v1 — confirm.
2. **Propose-confirm vs auto-apply?** Should the agent apply emphasis directly (it already owns
   the editorial call), propose indices for user confirmation, or surface in UI? Current
   `plan_* → apply_edl` flow is effectively one-shot, no per-item approval. Is that acceptable,
   or is emphasis worth a confirm step?
3. **Tool-arg shape (if Option A):** `emphasis_line_indices: Vec<usize>` (positional) vs a
   time-range map vs cue-id keying. Positional is simplest but brittle across re-segmentation.
4. **Motion override granularity:** can the agent override the emphasis preset's motion slots
   (e.g. emphasis without bounce), or are motion slots locked to the preset?
5. **Restraint enforcement:** purely a skill instruction (recommended), or a soft Rust guardrail
   (e.g. warn if >N% of cues flagged)? The reframe says skill-only — confirm no rate-limiter.
6. **Scene-aware hardcode:** `scene_aware_short_form.rs:613-615` hardcodes `word_pop`. Should
   emphasis indices flow from `editorial_moments` automatically there, or stay agent-driven via
   the MCP tool only?

---

## 7. Recommended v1 scope

**Ship (smallest shippable increment):**

- Add `is_emphasized: bool` to `CaptionRecommendation` (`types.rs:16-27`), serialized.
- Add `emphasis_line_indices: Vec<usize>` to `PlanCaptionsArgs` (`plan_captions.rs:18-31`);
  default empty = unchanged behavior.
- In `plan_captions` run, mark `rec[i].is_emphasized` for given indices; resolve **two** specs
  (default + `emphasis` preset) and pass both to `build_caption_edl_lines`.
- Change `build_caption_edl_lines` (`edl.rs:12-16`) to branch per rec on `is_emphasized`,
  emitting the emphasis spec's `style_json` for flagged cues. **No render changes** — the per-cue
  `style_json` path is verified.
- Skill guidance encoding Section 3 (which lines, restraint budget, B-roll/hook targeting,
  rationale rules) — the agent does the selecting.
- Tests: indices mark the right recs; EDL contains emphasis styling only for flagged cues.

**Explicitly deferred:**

- **Word-level emphasis** (per-word `\fs`/`\fscx`/color overrides) — the only net-new render
  work; defer to a later phase (Section 4 enumerates it).
- **Propose-confirm / approval bucketing** (Option D) unless the brainstorm decides emphasis
  warrants a confirm gate.
- **Auto-flow from `editorial_moments`** in `scene_aware_short_form` — keep v1 agent-driven via
  the MCP tool; auto-wiring is a follow-up.
- **Rust restraint rate-limiter** — restraint stays a skill instruction per the reframe.

---

### Verification notes

- VERIFIED by direct file read: per-cue `style_json` emission inside the caption loop
  (`edl.rs:18,31-32`); active-word branch emits only `\c`/`\t`, no `\fs`/`\fscx`
  (`ass.rs:547-563`); whole-cue branch is independent per cue (`ass.rs:568-586`).
- Other file:line claims are carried from the four upstream surveys and are internally consistent
  but were not all re-opened here; treat non-render claims (parser/apply line numbers, preset
  internals) as **survey-sourced, high-confidence but not independently re-verified** in this
  pass.
