# Decision: Merge Awidat's editorial prompt layer into the vendored codex base prompt?

**Question:** Should Awidat collapse its 3-layer prompt stack (codex base prompt + Awidat editorial layer + dynamic per-project addenda) into 2 layers by folding the editorial layer into a base prompt we own?

**Date:** 2026-07-19 · **Author:** Engineering · **Status:** Recommendation

---

## 1. TL;DR recommendation

**DO IT BUT BUILD THE EVAL FIRST — and when you do merge, do it via config-level `base_instructions` override, never by fork-editing `prompt.md`.** The editorial case is real: Awidat's cut-governing rules ("never cut mid-sentence," silence thresholds, dirty-cut grammar, edit-graph-is-source-of-truth) currently ride the *developer* channel, which sits strictly below the *system*/`instructions` channel where codex's coding-agent framing lives — so on any conflict, codex's defaults win on authority. Promoting editorial doctrine to the base equalizes that authority and deletes a contradictory "You are a coding agent" self-concept. **But the headline win is unmeasured**, and we have **zero infrastructure today to compare two prompt configs end-to-end** (montage-eval runs no agent). Merging is a prompt-quality experiment, not a correctness fix — so gate it on a small A/B harness we can build in roughly one focused push, and ship the mechanism (override behind a flag, reversible with `None`) rather than a hand-forked vendored file.

---

## 2. The three-layer status quo, precisely

There are **three** prompt inputs the model sees on every Awidat turn, delivered on **two different request channels**:

| Layer | Content | Channel | Where it enters | Source |
|---|---|---|---|---|
| **1. codex base prompt** | 275-line coding-agent prompt (identity, harness contract, coding/validation ethos) | Top-level Responses API `instructions` field (**system tier**) | `include_str!("../prompt.md")` → `base_instructions` | `vendor/codex-rs/models-manager/src/model_info.rs:16`; `vendor/codex-rs/models-manager/prompt.md` |
| **2. Awidat editorial layer** | Montage identity, tool catalog, editorial doctrine, per-format addenda | A **developer-role** item in the conversation `input` array (**below system**) | `assemble_for_project(...)` → `developer_instructions` | `crates/core/src/system_prompt.rs:255` (BASE_PROMPT); assembled at `apps/desktop/src-tauri/src/codex_session.rs:114` |
| **3. dynamic runtime sections** | permissions, apps, skills catalog, AGENTS.md, personality, collaboration mode | ~10 developer/user-role items | `build_initial_context` | `vendor/codex-rs/core/src/session/mod.rs` (`build_initial_context`) |

**Critical nuance (from the skeptic angle, verified):** "3 layers" undersells layer 3. `build_initial_context` assembles ~10 developer-role sections at runtime (permissions, collaboration, personality, apps, skills, plugins, AGENTS.md, extension contributors). Merging 1+2 touches **one** of these; the other 8+ keep interleaving. **The merge does not actually reduce the effective layer count** — it changes the *authority tier* of the editorial content, which is the real (and only durable) benefit.

Resolution point where it all lands:
```
session/mod.rs:541  base_instructions = config.base_instructions.clone()
                      .or_else(|| history…)
                      .unwrap_or_else(|| model_info.get_model_instructions(config.personality))
```
`config.base_instructions`, when set, **replaces** the default `prompt.md` — there is no concatenation. Awidat sets it **nowhere today** (grep across `crates/core`, `crates/codex-bridge`, `apps/desktop/src-tauri` returns zero hits) — the editorial layer is purely additive on `developer_instructions`.

---

## 3. Mechanism: config-override vs fork-edit

**Recommended: config-level `base_instructions` override, authored in our tree. Do NOT fork-edit `vendor/.../prompt.md`.**

Both mechanisms deposit the merged text in the **identical** request slot (`ResponsesApiRequest.instructions` via `prompt.base_instructions.text`, `client.rs:724/753`), so neither is safer *at runtime*. The tiebreaker is **refresh durability** (see §5), which decisively favors the override:

- `prompt.md` is currently **zero-maintenance** — a single vendor-import commit, never fork-edited, not in the fork's 9-item patch inventory (`vendor/codex-rs/SOURCE`). Fork-editing it makes it patch #10, hand-re-applied every refresh, conflicting on any upstream prompt edit, and **invisible to the drift script** (`scripts/codex-upstream-drift.sh` `SECURITY_PATH_PATTERN` covers only sandbox/safety paths).
- The override path already exists end-to-end: `ConfigOverrides.base_instructions` (also settable via TOML `model_instructions_file`/`instructions`), plumbed through `thread/start`'s `baseInstructions` param → `build_thread_config_overrides`. The bridge simply doesn't send it yet. Set it at **thread start** — `thread/resume` ignores it with a warning.

> **Note on the "FORK-EDIT is applied uniformly across all entry paths (tui_cmd, chat_cmd)" argument from the mechanism angle: DISCOUNTED.** The verifier confirmed those CLI files don't exist in this repo, and `assemble_for_project` is invoked in exactly one place (`codex_session.rs:114`). That argument for fork-edit does not hold. The durability angle's reasoning (below) is what carries the override recommendation.

### Harness-contract sections that MUST be preserved verbatim-in-spirit

An override **replaces the whole string**, so the merged prompt must copy these load-bearing sections intact — they are wired to tool parsers, the plan renderer, and doc discovery. Dropping their *prose* leaves the tool *schemas* working (those are injected as code-level `ToolSpec`s regardless) but strips the guidance the model needs to use them correctly:

1. **`apply_patch` invocation envelope** — `prompt.md:132`. **The tool is named `apply_patch`** (confirmed: `apply_patch_spec.rs:19` registers `name: "apply_patch"`, and `prompt.md:132` says `apply_patch` — they match).
   > **The mechanism angle's "rename to `ln`" instruction is FABRICATED and must be ignored.** There is no `ln`/`ln_freeform_tool` anywhere in the fork. Acting on that instruction would name a nonexistent tool and introduce a bug. Keep `apply_patch` verbatim.
2. **`update_plan` status vocabulary** — `prompt.md:52–70, 267–275` (`pending`/`in_progress`/`completed`).
3. **AGENTS.md precedence + project-doc discovery rules** — `prompt.md:17–27` (Awidat layers its own discovery on top; AGENTS.md is a separate fs-driven channel and cannot even be absorbed by the merge — only its precedence prose lives in the base).
4. **`【F:…†` citation-marker prohibition** — `prompt.md:147`.
   > Minor correction to the skeptic angle: the "63 parse sites" for this marker is a false substring hit (`eliCITATION`/`soliCITATION`). Nothing *parses* the marker; it is a pure output-formatting instruction. Still keep it — dropping it risks the model emitting citation markers into chat.
5. **Approval-mode names** — `prompt.md:159–163` (the enum values, e.g. `never`/`on-failure`/`untrusted`/`on-request`; note `prompt.md:161` says `never`, not `manual`).
6. **File-reference format** — `prompt.md:219–227` (low value for an editor whose references are timestamps/clip_uuids — a trim candidate, but keep whatever the renderer keys on).

Safely **rewritable/deletable** (advisory prose, no runtime parser): personality/preamble (13–15, 29–50), planning examples (72–121), coding-guidelines & validation ethos (123–171), final-answer tone (181–256), shell `rg` preference (258–265).

---

## 4. Editorial case: gains, and codex framing to drop

**What merging gains (the honest split of measured vs speculative):**

- **Authority-tier promotion (mechanism is proven; behavioral payoff is speculative).** Editorial rules move from developer tier to system tier, so "never cut mid-sentence" (`system_prompt.rs:477`) carries the same weight as "use apply_patch" does today. That the channels differ in authority is **verified in code** (system `instructions` field outranks developer-role input items). That this *changes model behavior for the better* is **not measured anywhere** in the repo.
- **Delete a contradictory self-concept (concrete, magnitude unmeasured).** `prompt.md:1` "You are a coding agent running in the Codex CLI" directly contradicts `system_prompt.rs:256` "You are Montage… Do not answer as Codex." Today layer 2 *overrides by assertion* but the contradicting text stays in-context (residual pull + tokens). Deletion removes it.
- **Kill the coding-validation loop that has no referent.** `prompt.md:149–171` tells the model to run tests/build/lint to verify work; Awidat defines verification as `view_timeline`/`vedit_diff`/render+review and **bans shell for producing the artifact** (`system_prompt.rs:438`). This pull can waste turns in an editing session.
- **Token cost (measurable win, direction unknown).** Merging can trim ~50 lines of coding framing — but base prompt tokens are a **fixed cost on every request including every compaction** (`compact_remote_v2.rs`). A longer editorial base could *raise* per-compaction cost. This is exactly the kind of thing the eval should measure directly.

> **DISCOUNT the "~575 lines of editorial prose" figure** from the editorial angle — the verifier confirmed BASE_PROMPT + PODCAST_ADDENDUM is ~365 lines, and 575 only appears by summing shorts/tutorial stubs the podcast path never loads. The editorial layer is large, but not that large.

**codex framing to drop/override:** the coding-agent identity (`prompt.md:1, 125`), the test/build/lint validation loop (149–163), "don't fix unrelated bugs / match existing codebase style" (138, 140, 157), and the greenfield-vs-existing-codebase ambition guidance (165–171). Replace the identity, swap the validation loop for render/review, delete the rest.

**Bottom line:** the mechanics of the gain are verified; the *size* of the gain is speculative. That is precisely why this is an experiment to A/B, not a fix to ship blind.

---

## 5. Durability: refresh-conflict analysis & where the merged prompt lives

- **Status quo is the most refresh-durable arrangement that exists.** Editorial content is 100% in our tree, delivered additively; `prompt.md` is untouched and re-applies for free. Any merge makes refreshes *harder* than today — an honest cost to state plainly.
- **Override is refresh-immune at the file level; fork-edit is not.** An override snapshots upstream's harness-contract sections into *our* file, so `prompt.md` can churn upstream with no conflict. The cost shifts from "auto-free" to "manual drift review of the copied contract sections."
- **Where it lives:** author the merged prompt as an embedded file/const in `crates/core`, **co-located with `system_prompt.rs`** (e.g. `crates/core/src/system_prompt/base_prompt.md`), passed via `ConfigOverrides.base_instructions` at the `codex_session.rs` boundary. Reversible: pass `None` → stock `prompt.md`.
- **Required follow-ups if we override:**
  1. Extend `scripts/codex-upstream-drift.sh` to flag commits touching `models-manager/prompt.md` (currently uncovered) — we now depend on knowing when upstream's contract sections change.
  2. Note in `vendor/codex-rs/SOURCE` pointing to our in-tree merged prompt, so refreshers diff codex's new `prompt.md` against our snapshot.
  3. A `crates/core` test asserting the merged base still contains the contract tokens (`apply_patch`, `update_plan` vocab, approval names, AGENTS.md precedence, citation ban) so a stale copy fails CI. Mirror the existing pins in `system_prompt.rs` tests (`:666, :677, :681`).

> **Stale-SHA caveat (verified):** `SOURCE` records fork SHA `8a94430` (2026-05-25), **not** the task-cited PR #102 SHA. The drift script reads `SOURCE` as its only source of truth — a stale SHA makes the drift report wrong regardless of the merge decision. Fix this independently.

---

## 6. Evaluation plan

**Does eval infra exist today? NO.** `crates/eval` (montage-eval) is a **deterministic scorer over pre-committed `.cuts` fixtures — it runs no agent and cannot A/B two prompt configs.** All `--product`/`--stress`/`--live` lanes return `Skipped` ("lane runner is not implemented yet"). The autonomous edit→verify→fix loop is *designed* (`docs/superpowers/specs/2026-06-21-autonomous-edit-eval-loop-design.md`) but the driver, workers, and run DB are explicitly deferred. The one CI "live-agent" lane runs `#[ignore]`'d `live_*` tests that are **all social-publishing HTTP clients, not editing sessions** (verified — none produce OTIO/render). So today we are flying blind.

**The good news: the A/B harness is small because the pieces exist.** Build, scoped to comparison only (the deferred "step 6" driver, minus the fix loop):

1. **Prompt-swap seam** — add a `base_instructions` override path to the run harness (a `--base-instructions-file` flag on codex-exec, which today hardcodes `base_instructions: None` at `exec/src/lib.rs:433`, or construct `ConfigOverrides` directly). Config A = stock `prompt.md` + developer_instructions; Config B = merged base + reduced developer_instructions. **Single variable.**
2. **Minimal driver** — one edit-worker spawn (`codex-exec --output-schema`) per (scenario × config), writing the `attempt_N` folder the spec already specifies. No fix loop needed for A/B.
3. **Deterministic metrics that already exist** — run each output through tier-1 (`mechanical.rs`: playable/aspect/overlaps), tier-2 (`measurable.rs`: silence/black/freeze + loudness), and the pacing gates (`gates::cold_open`/`floor` vs house profile). Gives tool-call validity, gate pass-rate, and pacing deltas with zero LLM.
4. **Cheap telemetry from `--json` JSONL** — total tokens (the headline token-cost claim, measured directly), tool-call count + schema errors (**instruction-following / harness-contract regressions — exactly what a bad merge breaks**), turns-to-completion.
5. **Paired design** — same N scenarios, same seed/model/temperature/source media, differing only in base-instructions config; k≥3 trials/cell for variance. **Win condition:** gate pass-rate and tool-call correctness hold flat while tokens drop. **Red flag:** any drop in tool-call correctness = broken contract fold.
6. **Defer tiers 3–4** (faithfulness + taste judge) — unbuilt, gated on a human-labeled calibration set. The merge question is base-prompt equivalence + token cost, which tiers 1–2 + telemetry answer.

---

## 7. The case against (skeptic angle, fairly represented)

The strongest arguments for **not merging by default**, with honest weighting:

1. **The merge doesn't actually collapse the layers.** `build_initial_context` keeps injecting ~8 other developer sections. The "3→2 layers" framing oversells the structural simplification. **(Solid — verified.)**
2. **The benefit is soft and unmeasured; layer 2 already overrides coding framing by assertion.** No repo artifact measures mis-behavior from residual coding framing. **(Solid — this is the core reason to build the eval first.)**
3. **Personality-templating regressions.** A static override bypasses codex's `{{personality}}` templating (`session/mod.rs:545`) and can double-inject personality via the `has_baked_personality` string-equality check (`session/mod.rs:2748`). **Fairly represented but CONDITIONAL:** the verifier confirmed these fire *only* for personality-capable slugs (`gpt-5.2-codex`/`exp-codex-personality`). Awidat passes the model as `Option<String>` at turn-start and the default appears to be a non-templated slug — so in the actual default config these regressions are **likely inert**. The findings overstated them as categorical "BREAKS"; treat as a config-gated risk to confirm, not a blocker.
4. **Permanent ownership of harness-contract text.** We'd re-audit `apply_patch`/`update_plan`/AGENTS.md/citation prose every refresh. **(Real, but bounded — mitigated by the drift-script + CI-token-pin follow-ups in §5. The "63 parse sites" evidence was a false substring hit and should be discounted.)**
5. **A lower-risk alternative gets ~90% of the benefit:** keep additive by default; use the REPLACE override only *if/when* a contradiction is proven. The config override even correctly nulls `model_messages` to avoid double-injection (`model_info.rs:55`). **(This is largely what the recommendation adopts — merge via override, gated on evidence.)**
6. **Opportunity cost.** The risk register (`docs/risk-register-2026-07-15.md`) ranks 23 items to clear top-down before features; a prompt refactor with no failing behavior driving it isn't on that critical path. **(Legitimate — argues for the *cheap* path: build the small eval, prototype behind a flag, don't invest in a hand-forked prompt.)**
   > The skeptic's supporting claim that "PR #102 landing cleanly proves the seam is low-friction" is **DISCOUNTED** — the verifier found PR #102 is `CONFLICTING`. The broader additive-architecture argument survives; that specific proof point does not.

---

## 8. Recommended next step

**Build the A/B eval seam first, then prototype the merge via config-override behind a flag.** Concretely, the single next action:

> **Add the prompt-swap seam to the eval harness: a `base_instructions` override path (flag on codex-exec, replacing the hardcoded `None` at `exec/src/lib.rs:433`) + a minimal edit-worker driver that runs one pass per (scenario × config) and captures tier-1/tier-2 gate results and `--json` token/tool-call telemetry.**

This turns the decision from vibes into a paired A/B on gate pass-rate + tokens + tool-call correctness. If Config B holds correctness flat and cuts tokens (or measurably improves editorial gate deltas), ship the merge as an in-tree `base_instructions` override — never a fork-edit of `prompt.md` — with the drift-script, SOURCE-note, and CI-token-pin follow-ups from §5, and confirm the runtime model slug isn't a personality-templated one before flipping it on. If Config B shows any tool-call regression, keep the three-layer additive design and spend the effort on the risk register instead.
