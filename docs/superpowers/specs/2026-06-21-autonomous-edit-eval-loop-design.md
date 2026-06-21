# Autonomous Editing-Improvement Loop (montage-eval)

**Status:** Design approved (brainstorm), pending spec review
**Date:** 2026-06-21
**Author:** Tadiwa + Claude
**Scope target:** Build out the already-expected `montage-eval` crate as a long-running,
mostly-deterministic loop that edits a batch of videos with the agent, verifies the output
without a human in the loop, auto-fixes while improving, and gates progression
tool-by-tool then flow-by-flow then category-by-category.

---

## 1. Problem & intent

We have a large batch of source videos on an external drive and a 111-tool agent-native
editor (Montage, Codex-driven). We want an automated pipeline where the agent **edits →
output is verified → if wrong, a fix pass runs → re-verify → repeat while improving →
pass → move on**. It must run unattended for **hours to days**, perfecting one tool at a
time, then a whole flow end-to-end (e.g. podcast start→finish), before unlocking the next
category (e.g. shot extractor).

The hard part the user named: **LLMs are poor whole-video judges.** The design's central
move is to **not** ask "is this video good?" Instead, convert the edit into structured
evidence and check it against a contract the agent committed to.

### Failure classes to catch (user: "all, especially taste")
1. **Mechanical** — wrong-frame cuts, timeline gaps, op applied to wrong clip, render
   failure, manifest inconsistency.
2. **Measurable** — loudness, black/freeze frames, clicks at cuts, speaker cropped out,
   caption desync.
3. **Faithfulness** — edit preserves meaning of its own source (no mid-word/mid-sentence
   cuts; edit transcript is a coherent subset of source transcript).
4. **Taste/style** — result vs. the playbook approach the agent committed to + vs. genre
   exemplars (good videos downloaded beforehand).

---

## 2. Core principle

> **The harness is the judge, not the LLM.** The LLM proposes edits, explains failures,
> and gives narrow rubric verdicts on extracted evidence. The deterministic driver decides
> pass/fail, retry, stop, and progression. Agents are workers; the driver is the boss.

This is the only way a multi-day unattended run survives the "agents don't follow commands"
problem the user flagged: the loop logic (queue, gating, improvement check, best-version
retention, resume) is plain code that cannot drift.

---

## 3. What already exists (reuse) vs. must-build

Verified against repo at `664f427e`.

### Reuse (do NOT reinvent)
- **`montage-eval` crate is already expected.** `.github/workflows/evals.yml` invokes
  `cargo run -p montage-eval -- --ci --product --golden --json`, `--stress`, `--live`,
  with lanes: `eval-suite` (product/golden/stress), `python-audio-energy`, `live-agent`,
  `real-corpus` (self-hosted). The crate dir `crates/eval/` exists but is **empty**.
  → **We build out this crate; CI lanes already point at it.**
- **Sidecar evidence already emitted:** `audio-energy-mcp` → integrated LUFS +
  `true_peak_dbfs`; `frame-quality-mcp` → `thumbnail_score` + ranked `thumbnail_candidates`;
  `composition-mcp` → `verification` object with `passed` / `checked_regions` / `issues`.
  → **Verifier reads sidecars; does not re-derive these signals.** (`python/SMOKE.md`.)
- **Skills already carry their own contracts.** `skills/auto-cutter/SKILL.md` ("## Done
  when") and `skills/short-form/SKILL.md` ("## You are done when…") declare explicit
  acceptance checklists (e.g. "Every silence ≥1.0s gone", "every word has a caption
  overlay", "hook at position 0", "`vedit_diff` reviewed", "render verified").
  → **The scenario contract is the machine-encoding of the skill's own checklist.**
- **`codex-exec` binary** (`vendor/codex-rs/exec/`) supports `--json` (JSONL events),
  `--output-schema FILE` (schema-constrained final output), and `Resume`/`--last`.
  → **Driver shells out to `codex-exec` directly** for worker threads. (Note: there is no
  `codex exec` subcommand — the binary is `codex-exec`; advice that named a subcommand was
  wrong.)
- **`montage validate`** already validates timeline + edit plan + index structurally →
  tier-1 plumbing reuse.
- **OTIO timeline + render manifests** (`crates/render/src/manifest.rs`) with input
  fingerprints, FFmpeg replay argv, outputs, limitations → tier-1 evidence source.
- **vedit** git-style versioning → diff/blame/revert for fix attempts and best-version
  retention.

### Must-build
- The `montage-eval` crate body: scenario loader, run-folder/artifact contract, the driver
  state machine, deterministic validators, evidence-packet generator, judge invocation,
  scorecard writer, run DB, campaign/gating runner.
- Scenario files per tool/flow/category.
- Exemplar corpus ingestion (download + index good videos) for the style judge.
- A run-memory/regression store (the `lessons` subsystem is **stubbed** pending re-port
  onto `~/.codex/sessions/`, so it is NOT ready — we ship our own SQLite run DB now and may
  wire `lessons` later).

---

## 4. Architecture: deterministic spine, agentic workers

```
                      ┌──────────────────────────────────────────────┐
                      │   montage-eval DRIVER (deterministic Rust)     │
                      │   queue · gating · improvement check ·         │
                      │   best-version retention · checkpoint/resume   │
                      └───────┬───────────────┬───────────────┬───────┘
            spawns codex-exec │               │ runs in-proc  │ spawns codex-exec
            (--output-schema) │               ▼               │ (--output-schema)
                      ┌───────▼──────┐  ┌──────────────┐  ┌───▼──────────┐
                      │ EDIT worker  │  │ Tier 1+2     │  │ VERIFY judge │
                      │ (codex agent)│  │ checks (code)│  │ (codex agent)│
                      └───────┬──────┘  └──────┬───────┘  └───┬──────────┘
                              │                │              │
                              ▼                ▼              ▼
                       OTIO + render +   scorecard tiers  faithfulness +
                       manifest + sidecars                style verdicts
                              │
                              └────────────► FIX worker (codex agent) ◄── defect report
```

- **Driver** = plain Rust in `montage-eval`. Owns everything that must be reliable. Writes
  a checkpoint after every step.
- **Workers** = `codex-exec` invocations (edit / verify / fix), each schema-constrained via
  `--output-schema` so the driver gets structured JSON, not prose. This is where Codex's
  thread/session capability is used — for the work, never for the loop control.
- **Tier 1+2 checks** = in-process Rust + ffprobe + reading existing sidecars. No LLM.

**Runtime placement:** runs locally on the user's machine where the external drive lives.
Invoked from the Codex app's terminal (`montage-eval campaign …`) or via a Codex
skill/command. The driver is a CLI process, not an always-on server, but it persists state
so it survives restarts across days.

---

## 5. The four-tier verification stack

| Tier | Checks | Mechanism | LLM? |
|---|---|---|---|
| **1 Mechanical** | render exists/playable/right duration/fps/aspect/codec; audio+video streams present; manifest internally consistent; OTIO valid, no negative durations, no illegal overlaps, no gaps > tol; op applied to intended clip; required ops present | `ffprobe -print_format json`, `montage validate`, OTIO parse, manifest parse | none |
| **2 Measurable** | dead air (silencedetect / audio-energy sidecar), black frames (blackdetect), freeze (freezedetect), loudness LUFS + true-peak (audio-energy sidecar), clicks at cut boundaries, speaker stays in crop (face-box vs reframe-box), caption safe-area + sync | ffmpeg filters + existing sidecars (audio-energy, frame-quality, composition, face/gaze) | none |
| **3 Faithfulness** | edit transcript is a coherent subset of source transcript; no mid-word cut; no mid-sentence cut unless allowed; required spans preserved; forbidden spans removed | transcript diff (existing transcript tooling) + WER-style metric; narrow LLM check only on flagged spans | grounded on text |
| **4 Taste/style** | result vs. the **playbook approach the agent declared** (skill's own "Done when" criteria) + pairwise vs. a genre **exemplar**: "which is the more professional edit, and on which axes does the weaker lose?" | multimodal judge over an **evidence packet** (frames at cut points, not whole video), few-shot calibrated on exemplars | grounded on named standard + reference |

Tiers 1–2 are the **hard gate** (must pass) and catch the large majority of "did it wrong"
cheaply and 100% reliably. Tier 3 is a hard gate on faithfulness (meaning-preservation is
not negotiable). Tier 4 is a **scored gate against the declared playbook** — see §8.

---

## 6. Evidence packets (how tiers 3–4 avoid whole-video judging)

For each rendered output the driver generates an evidence packet the judges consume instead
of the raw video:

- Contact sheet: frame at each cut, plus ±0.5s around each cut.
- Periodic frames (every N seconds) for global sanity.
- Frames at low-confidence windows (face detector low, composition `issues`).
- `transcript_source.json` + `transcript_rendered.json` + computed `transcript_diff.json`.
- Timeline diff (`vedit_diff`) source→edit.
- Audio/silence/loudness report (from sidecars).
- Short failure-window clips around suspected defects.
- The exemplar drawn for pairwise comparison (tier 4).

The judge is asked **narrow** questions ("Between A and B for 00:42–01:15, which keeps the
active speaker visible and preserves reaction timing?"), never "is this good?".

---

## 7. The loop (per work item)

```
edit ──► tier1+2 (code) ──► tier3+4 (judge) ──► scorecard + defect report
                                                        │
        ┌───────────────── improving? ─────────────────┤
        │ yes: fewer/less-severe defects AND no         │ no: same-failure-repeated
        │      previously-fixed defect re-broke         │     OR score-not-improving
        ▼                                               ▼     OR safety-ceiling (~10)
   fix worker (gets structured defect report)        STOP → flag for human review
        │   re-render, re-verify                      (best-scoring version retained)
        └───────────────────────────────────────────────┘
```

**Stop conditions (explicit):** `PASS` · `SAME_FAILURE_REPEATED` · `SCORE_NOT_IMPROVING` ·
`MAX_ATTEMPTS` (safety ceiling, fire-alarm only) · `HUMAN_REVIEW_REQUIRED`.

**"Keep fixing while improving"** = continue iff each round strictly reduces the
defect set (count or severity) and does not reintroduce a previously-fixed defect.
Converging clips run many rounds; stuck/oscillating clips bail fast. **Best-scoring version
is always retained**, never just the latest, so a fix can't lose a good intermediate.

This is the user's "leave it running for days, as long as it's iteratively improving."

---

## 8. Taste as a technique library + a grounded gate

Taste lives in two distinct places (this was the key clarification):

1. **Taste as input — technique library.** Curated playbooks (built on the existing
   `editorial_skills` / `load_skill` + the skills' own SKILL.md flows) encode "how to do
   transitions," "how to run the full podcast pipeline," etc., distilled from advice and
   from the downloaded exemplars. Before editing, the agent **loads the relevant playbook,
   chooses an approach case-by-case, pulls/saves transcripts as working notes, and declares
   which approach it committed to.**

2. **Taste as output — the gate.** Tier-4 grades the result **against the declared
   approach's own criteria** (the skill's "Done when" checklist) **and** pairwise against a
   genre exemplar. Because the standard is named and written, the gate is checkable and
   converges — you cannot endlessly game "be good," but "did you achieve approach X's stated
   criteria" terminates.

**Anti-gaming:** the fuzzy tier-4 cannot drive endless churn because (a) tiers 1–3 are the
hard mechanical/faithfulness floor, (b) tier-4 grades against a fixed declared checklist not
an open vibe, and (c) `SCORE_NOT_IMPROVING` / `SAME_FAILURE_REPEATED` halt churn.

---

## 9. Progression gating (strict)

```
TOOL level:     run tool across batch ─► must hit pass-rate threshold ─► next tool
FLOW level:     all tools in category green ─► run whole flow end-to-end
                (e.g. podcast story-map→…→render) ─► must pass ─► unlock next category
CATEGORY level: podcast ─► shot extractor ─► shorts ─► b-roll ─► full creative
```

Strict gates give clean failure attribution (perfect one thing before compounding). A
failing tool hard-blocks its category's flow run and the next category.

**Recommended starting order** (cheapest-to-score first, per advice + repo readiness):
1. **Stage 0 — indexer reliability** (transcript/scene/audio/face sidecars correct on a
   labeled mini-set; garbage-in guard before any editing is judged).
2. **Podcast / auto-cutter** (most measurable; skill checklist already exists).
3. **Short-form / shorts extractor** (skill checklist exists; objective crop/caption/hook
   checks + transcript-based hook judgment).
4. **Shot extractor.**
5. **B-roll / visual support.**
6. **Full creative edits.**

Thresholds (pass-rate per tier, safety-ceiling number, "what counts as improvement" deltas)
are **knobs tuned on first real runs**, not blockers now.

---

## 10. Scenario & artifact contracts

### Scenario file (per tool/flow), e.g. `crates/eval/scenarios/podcast/dead_air_basic_001.yaml`
```yaml
id: podcast_dead_air_basic_001
category: podcast
tool: auto-cutter            # or a single tool for tool-level gating
source: corpus/podcast/two_speaker_dead_air_12min.mp4
task: >
  Remove dead air longer than 1.0s, preserve all meaningful speech,
  export 16:9 with captions. Use the auto-cutter playbook.
hard_gates:                  # tiers 1-2-3 — must pass
  playable: true
  aspect_ratio: "16:9"
  max_remaining_silence_seconds: 1.0
  min_speech_retention: 0.97
  max_caption_wer: 0.08
  no_black_frames: true
  no_invalid_timeline_overlaps: true
  no_mid_word_cuts: true
soft_gates:                  # tier 4 — scored against declared playbook
  declared_playbook: auto-cutter
  min_style_score: 0.80
repair:
  policy: while_improving
  safety_ceiling: 10
guards:                      # anti-gaming — driver-enforced, agents cannot edit
  allow_scenario_edits: false
  allow_threshold_edits: false
  max_files_changed: 6
  max_lines_changed: 400
```

### Run folder = source of truth (per attempt)
```
runs/<run_id>/<scenario_id>/
  task.md  input_manifest.json
  attempt_1/
    edit_plan.json  output.otio  output.mp4  render.manifest.json
    ffprobe.json  silence.json  black.json  loudness.json
    transcript_source.json  transcript_rendered.json  transcript_diff.json
    evidence/ (contact sheets, failure-window clips, drawn exemplar)
    scorecard.json  verifier_report.json
  attempt_2/ …
  final_status.json
```

### Scorecard (machine-readable, drives the improvement check)
```json
{ "scenario_id": "...", "attempt": 2, "status": "fail", "score": 0.74,
  "tiers": { "mechanical": "pass", "measurable": "fail", "faithfulness": "pass", "style": 0.71 },
  "blocking_failures": [ { "code": "SILENCE_TOO_LONG", "severity": "blocker",
      "evidence": { "segments": [{"start":118.2,"end":121.6}] },
      "repair_instruction": "Tighten these silences without cutting speech." } ],
  "stop_reason": null, "next_action": "repair" }
```

---

## 11. Durability (multi-day runs)

- **Checkpoint after every step**; resume the queue on restart (mirrors `codex-exec`'s own
  `Resume`/`--last`).
- **SQLite run DB** (`runs.db`): videos, scenarios, runs, attempts, metrics, failures
  (tagged taxonomy), repairs, stop_reason, cost estimate, best_version_path. Enables
  "which tool breaks most / is the category improving / which clips are hardest."
- **Best version retained on disk** per scenario regardless of latest attempt.
- **Failure taxonomy** (`SILENCE_NOT_REMOVED`, `MID_SENTENCE_CUT`, `FACE_CROPPED`,
  `CAPTION_MISMATCH`, `BAD_VERTICAL_CROP`, `AGENT_IGNORED_INSTRUCTIONS`, …) so the fix
  worker gets focused targets and the DB is queryable.
- **End-of-run report:** pass-rate per tool, stuck/quarantined items, score trends per
  category, cost.

---

## 12. Anti-gaming invariants (driver-enforced)

Agents (edit/verify/fix workers) **cannot**: edit scenario files, edit thresholds/gates,
edit the verifier code, delete or rewrite failing artifacts, mark themselves pass, or exceed
the diff budget (`max_files_changed` / `max_lines_changed`). Verify worker is read-only.
These are enforced by the driver (workspace isolation + post-run diff inspection), not by
asking the agent nicely.

---

## 13. Exemplar corpus (the "download good videos" idea)

A pre-stage downloads and indexes genre exemplars (great podcast edits, viral shorts,
clean shot-extractions). They serve two roles:
1. **Calibration / few-shot** for the tier-4 style judge.
2. **Pairwise reference** ("more professional than this exemplar?").

Note: exemplars calibrate *style*, not per-source faithfulness — that's why tier 3
(faithfulness) checks the edit against *its own* source, separately.

---

## 14. Human-in-the-loop (governance, not per-run)

No human per run. Humans are used for: initial threshold calibration, validating the tier-4
judge against a small human-labeled golden set before trusting it at scale, periodic spot
checks, and the `HUMAN_REVIEW_REQUIRED` queue (stuck/quarantined items). Review is fast
because it's evidence packets (changed regions, transcript diff, 3 flagged clips), never
"watch the whole render."

---

## 15. Build order (high level — detailed plan follows in writing-plans)

1. `montage-eval` crate skeleton + scenario loader + run-folder contract + scorecard writer,
   wired to the existing `--ci --product --golden --json` CI entrypoint.
2. Tier-1 deterministic validators (ffprobe + `montage validate` + OTIO/manifest parse).
3. Tier-2 measurable validators (ffmpeg filters + read existing sidecars).
4. Evidence-packet generator.
5. Tier-3 faithfulness (transcript diff + narrow LLM span check).
6. Edit/verify/fix workers via `codex-exec --output-schema` + driver state machine +
   while-improving loop + checkpoint/resume + SQLite run DB.
7. Tier-4 style judge + exemplar corpus ingestion.
8. Campaign/gating runner (tool → flow → category) + end-of-run report.
9. Seed scenarios for Stage 0 (indexer reliability) + podcast/auto-cutter, then expand.

---

## 16. Open knobs (tune on first runs, not blockers)
- Pass-rate threshold per tier and per gate level.
- Safety-ceiling iteration count.
- Exact "improvement" deltas (defect-count vs. severity weighting).
- Style-judge score threshold and exemplar pairing strategy.
- Cost-aware scheduling: cheap checks on every artifact, deep multimodal only on uncertain
  or high-value windows.
```
