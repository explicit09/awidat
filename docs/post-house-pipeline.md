# Post-House Pipeline: Professional-Quality Autonomous Editing

Status: Design draft (2026-07-06), grounded in the DOAC/TBPN/own-catalog
editorial study. Companion to `docs/editorial-grammar-upgrade-plan.md`
(largely implemented) and the autonomous edit-eval loop spec
(`docs/superpowers/specs/2026-06-21-autonomous-edit-eval-loop-design.md`,
`crates/eval` still empty).

Study data and prototype: `/Volumes/My Passport for Mac/doac-study/`
(external drive) — `analysis/GRAMMAR_PROFILE.md` has the 13 findings this
design cites; `analysis/prototype/` has the drag-detector code and results.

## Problem

The agent "runs but doesn't do the job well." Diagnosis from the study:

1. **One generalist pass does all departments.** Real post-production is a
   pipeline of specialists with hard handoffs (picture → lock → sound →
   color → graphics → finishing), each verified against a deliverable spec
   before the next starts. We have the specialists' *tools* (~111 MCP tools)
   but not the *departments* — nothing enforces picture lock, and nothing
   verifies a pass before the next one runs.
2. **No measurable taste targets.** "Good pacing" had no number. Now it
   does (see Reference targets below).
3. **The craft is meso-scale and house-specific.** Professional editors make
   dozens of local judgment calls per episode (Finding 12: our own editor
   makes ~20 surgical speed-ups per episode, up to 2.45x). The drag-detector
   prototype proves these calls are learnable within a house (AUC 0.86
   single-feature on own edits) but INVERT across houses (TBPN-trained
   detector scores 0.35 on ours — worse than chance). Taste must be
   calibrated per house, trained on that house's own raw→published pairs.

## Reference targets (measured)

| Metric | DOAC | TBPN | Ours (today) |
|---|---|---|---|
| Cold open, first 90s | 26–55 cuts/min, peak minute 0–1 | n/a (live chrome) | 6.7 cuts/min flat, peak minute 42 |
| Body pacing, informational | 6.1–8.3 cuts/min | 9.3 (digest) | 3.2 |
| Body pacing, emotional/debate | 2.4–2.9 cuts/min (holds 25s shots) | 3.2 (interview) | — |
| In-show keep rate | full-episode product | 21% (digest product) | 43–60% |
| Speed-ups per episode | — | 8 @ 1.2–1.83x | 13–28 @ 1.2–2.45x |
| Shorts duration | 37–125s (cluster 90–120s) | — | — |
| Failure floor | — | — | 0 cuts in 30min (shipped raw composite) |

Key structural findings:
- **The cold open IS the short** (Findings 4, 7): DOAC's first ~90s is a
  blitz montage at short-form pacing, republished vertical. Author it once
  as a dual-purpose asset with 9:16-safe framing.
- **Hook = relocated peak line** (Finding 5): select passage → find the most
  provocative line inside it → prepend at 0:00 → let it recur in context →
  tighten body ~20%.
- **Two pacing archetypes, content-driven** (Findings 2, 8): raw-moment
  (2–13 cuts/min, minimal captions, punch-ins only) vs b-roll blitz
  (39–46 cuts/min, semantic caption emphasis, illustrated inserts).
  The choice is per-moment, not per-project.

## Architecture

```
                    ┌──────────────────────────────┐
                    │  house style profile (data)   │
                    └──────────────┬───────────────┘
                                   ▼
identify → picture pass → GATE → PICTURE LOCK → sound pass → GATE
                                                → color pass → GATE
                                                → graphics pass → GATE
                                                → finishing → GATE → publish
```

### 1. House style profile (new concept, data not code)

A versioned document (JSON/TOML sidecar per project or org) that binds what
today is scattered across prompts and skills:

- pacing targets per archetype (cuts/min bands, max shot hold, cold-open spec)
- b-roll sourcing policy (illustrated / macro / screenshots / memes / none)
- caption policy (chunk size, semantic-emphasis colors, font, placement)
- chrome policy (lower-thirds, ticker, bug — persistent vs none)
- keep-rate band per product type (full episode vs digest vs short)
- drag-judge calibration (feature polarities + thresholds, trained per house)

DOAC-cinematic and TBPN-chrome are the first two profiles (both measured);
"Technologia house" is the third, initialized from our own fingerprint
(Finding 12) and nudged toward targets we choose.

Consumers: every department pass reads it; every gate scores against it.
This is also where `ProjectFormat`'s prompt addenda should eventually live.

### 2. Department passes (orchestration over existing tools)

Each pass = one agent run with one skill, a tool allowlist, and the house
profile. No new engine code — the ops exist (`EdlOp`), the tools exist.

| Pass | Existing tools | Gate (deterministic) |
|---|---|---|
| identify | (new: sample-transcribe-and-match, see §4) | every asset resolved to episode/role |
| picture | find_dead_air/filler/false_starts, assess_edit_quality, cut-director, split-edit-director, SetSpeed/SetTimeRemap, drag judge (§3) | continuity clean; pacing within archetype band; cold open present & peak density at minute 0–1; keep-rate in band; **no 5-min window under 0.5 cuts/min** (SaaS-floor rule) |
| PICTURE LOCK | vedit tag | later passes may not move/trim clips |
| sound | plan_sound_design, SetDucking, dialogue_leveling, master_loudnorm, J/L-cuts | LUFS target; ducking under all b-roll; J/L coverage at speaker turns |
| color | plan_color_grade, ApplyLut, crates/lut | grade-vs-profile delta in tolerance |
| graphics | plan_captions, InsertRichTitle/InsertCaption, InsertBRoll/InsertPiP | caption policy conformance; emphasis words are semantic (negation/claim), not rhythmic |
| finishing | plan_delivery_export, verify_render, podcast_qc_report | render verified; full QC report attached |

Gates are Rust checks over the timeline + sidecars, not LLM opinions —
"the harness is the judge, not the LLM" (eval-loop spec). A department gate
and an eval-tier check are the same artifact: build once, use in both the
production pipeline and `crates/eval`.

### 3. The drag judge (meso-scale picture-pass component)

The hard problem ("1 minute needs 1.4x in a 50-minute video") — prototype
validated on real data (`analysis/prototype/`):

- **Features per 10s window**: speech rate, vocabulary novelty, semantic
  redundancy vs previous 2min, **fwd_repeat** (similarity to NEXT 2min —
  "said better later → cut it here", a recovered pro principle), audio RMS
  mean/variance, silence fraction. v3 adds sentence embeddings for true
  paraphrase detection.
- **Labels**: recovered from raw→published transcript diffs (difflib word
  blocks; whisper both sides — cross-ASR exact matching fails). Every
  episode we ship becomes training data. TBPN publishes live+digest daily
  as a supplementary public corpus.
- **Calibration is per-house** (Finding 13). Never ship a universal
  polarity. Within-house accuracy is high (rms alone 0.86); the combo
  target is ≥0.8 before the judge may auto-apply; below that it proposes,
  human disposes.
- **Local action policy**: flag → choose speed-up (1.2–1.8x band) vs
  interior trim vs full cut vs montage-compress (keep audio, blitz b-roll
  over), based on WHY it drags (redundancy → cut; low-energy but novel →
  speed-up; tangent → trim). Ops already exist: SetSpeed, SetTimeRemap,
  RippleDelete, InsertBRoll.

### 4. `montage identify` (pipeline hygiene, new tool)

Drive naming lies: two of our four assumed raw→published pairs were wrong
("Founder - Asray.mov" and "SAAS DEAD.mov.mp4" match different episodes
than their names suggest). Before any edit: sample-transcribe 4min from the
middle of each raw (whisper.cpp), match against known transcripts/uploads
by phrase containment, emit an identification manifest. Cheap (~40s/asset),
prevents editing the wrong footage, and feeds pair recovery for judge
training.

## Build order

1. **Gates first** (they are also the eval-loop checks; unblocks
   `crates/eval` progression): pacing/density/keep-rate/SaaS-floor checks
   over the timeline + `.cuts`-style sidecars. Smallest useful unit: the
   picture gate run against our own two published videos reproduces
   Finding 11 automatically.
2. **House style profile schema + the three initial profiles** (DOAC,
   TBPN, Technologia) with measured numbers from the study.
3. **Cold-open producer** (highest-leverage single feature, Finding 7):
   extend podcast-episode-producer to author the cold open as a dual
   asset (blitz montage + 9:16-safe framing), gate on "peak density at
   minute 0–1".
4. **Drag judge v3** (embeddings + per-house calibration) inside the
   picture pass, propose-only until ≥0.8 within-house AUC.
5. **`montage identify`** + automatic pair recovery → judge retraining
   loop on our own catalog.
6. **Department orchestration** (producer skill sequencing passes with
   picture lock) — last, because it composes the pieces above.

## Non-goals (this doc)

- New EdlOp variants — the op surface is sufficient.
- Universal cross-channel taste — proven not to exist at audio-feature
  level; per-house calibration is the design.
- Replacing the eval-loop spec — this supplies its gates and its taste
  tier's ground truth; progression/retention mechanics stay as specced.
