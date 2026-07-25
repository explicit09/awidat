# Taste gate: measurable editorial quality from professional ground truth

**Date:** 2026-07-25 · **Status:** Plan of record for the "editorial study follow-ups" arc

## Goal

Give Montage its first *editorial-quality* number: **agreement with a
professional editor's keep/cut/tighten/speed decisions on held-out
raw→published pairs.** Today the system has mechanical gates (tier-1/2),
pacing gates, and — as of PR #104 — a preview=export parity gate. It has
no measure of whether its *edits are good*. This arc builds that measure.
"Match DOAC/TBPN quality" is the north star; the deliverable is the
measuring stick that makes progress toward it visible.

## What already exists (do not rebuild)

The 2026-07-06 editorial study (`/Volumes/My Passport for Mac/doac-study/`,
findings in `analysis/GRAMMAR_PROFILE.md`) delivered:

- **13 quantified findings** across DOAC shorts/long-form, TBPN, and the
  own back-catalog (two-archetype pacing, cold-open-is-the-short, hook
  transplant construction, house-style table).
- **Ground-truth decision recovery works** (Finding 10): word-level
  transcript alignment of TBPN's 2:30h live show vs its 31min digest
  recovered the keep map (21% survival), 8 surgical speed-ups
  (1.2–1.83x), and hard structural jumps. TBPN publishes live+digest
  daily — a free, growing corpus.
- **Own-catalog pairs align too** (Finding 12): BRINK kept 43% with 13
  speed-ups; DRONE kept 60% with 28 speed-ups to 2.45x. Editorial
  fingerprint: we compress time but keep structure; pros keep the best
  21% and rebuild.
- **House-specificity is proven** (Finding 13): a TBPN-trained drag
  detector scores AUC 0.35 (inverted) on own edits. Audio-level "drag"
  polarity does not transfer. Consequences baked into this plan: the
  gate is *house-conditioned*, own back-catalog is the training/eval set
  for the autonomous editor's judge, and cross-house features must be
  semantic, not audio-mix proxies.
- **Prototype pipeline** in `analysis/prototype/`: `diff_own.py`,
  `diff_own_fuzzy.py`, `build_labels.py`, `extract_features.py`,
  `match_matrix.py`, `v2_features.py`. Plus 15+ `.cuts` files (3,616
  professional cut timestamps) already in `montage-eval`'s
  `load_cut_times` format.
- **Pipeline hygiene lesson**: drive naming lies — raw-asset
  identification via sample-transcribe-and-match ("montage identify")
  is a required preprocessing step, already prototyped
  (4-min whisper samples × published transcripts, containment).

## Phases

### Phase A — Corpus pipeline (productionize the prototype)

1. **Decision-list format** (the contract everything else consumes):
   a canonical JSON sidecar per raw→published pair —
   `{pair_id, house, raw_ref, published_ref, segments: [{raw_span:
   [s,e], action: keep|cut|speed, factor?, published_span?}], anchors,
   alignment_confidence}`. Lives next to the eval fixtures; schema
   validated in Rust (montage-eval) and Python (pipeline) against the
   same JSON Schema file.
2. **Aligner CLI** (`tools/taste-corpus/`): port `diff_own_fuzzy.py` into
   a tested CLI: `align --raw-vtt --published-vtt --out pair.json`.
   Known noise: auto-sub cue timestamps are ±0.1; sustained 1.2x+ speed
   readings trustworthy, isolated 0.7–1.15x are jitter (study caveat) —
   the CLI encodes those thresholds and emits confidence per segment.
3. **Identify CLI**: port sample-transcribe-and-match so raw drive
   assets are matched to published episodes by content, not filename.
4. **Corpus build**: run over (a) the existing TBPN pair, (b) BRINK +
   DRONE own pairs, (c) newly fetched TBPN live+digest pairs (the corpus
   grows daily), (d) more own back-catalog as identified. Target: ≥10
   pairs across ≥2 houses, held-out split defined up front.

### Phase B — The gate (deterministic scoring, no LLM)

5. **Agreement scorer** in `montage-eval`: given a proposed decision
   list and ground truth — per-window keep/cut agreement (10s windows),
   cut-boundary F1 with ±tolerance, speed-decision agreement (action +
   factor bucket), structural-curation score (kept-mass overlap).
   Scored per house per pair; aggregate = the taste number.
6. **Scenario wiring**: extend the eval Scenario/lane machinery
   (the `--product`/`--live` lanes are stubbed "not implemented") with a
   `taste` lane: run the agent (or a non-agent baseline first) on a raw
   input with a house profile, lower its EDL to a decision list, score.
   Baselines to run before any agent: (a) keep-everything, (b) the
   existing find_dead_air pipeline — establishing the floor the agent
   must beat.

### Phase C — House-calibrated judge (feeds the autonomous loop)

7. Drag-detector v2 with semantic features (embedding novelty, claim
   density) trained per-house on own pairs; becomes the Tier-4 judge in
   the autonomous edit-eval loop design. Out of scope until A+B ship.

## Metrics of record

- **keep/cut agreement %** (10s windows, per house)
- **cut-boundary F1** (±0.5s and ±2s tolerances)
- **speed-decision agreement** (action match; factor within bucket)
- Reported per pair + aggregated; held-out pairs never used for tuning.

## Risks

- Alignment noise on auto-sub timestamps (mitigated: confidence fields,
  sustained-reading thresholds, acoustic spot-verification per study).
- TBPN reorders content (hook transplants, structural jumps) — the
  decision-list format must represent non-monotonic mappings.
- Corpus licensing: third-party media stays on the drive as reference
  data; only derived decision lists/metrics enter the repo. Own-catalog
  media is unrestricted.
- House-conditioning discipline: never mix houses in a single score.
