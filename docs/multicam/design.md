# Agent-Native Multicam — Design Doc

**Status:** Proposal · **Author:** (drafted from code audit, 2026-05-30) · **Scope:** finish & integrate existing multicam primitives into a usable workflow

> ⚠️ **Record correction:** an earlier capability map called multicam "absent." That was wrong. A code audit found the multicam **engine is ~80% built** — a working planner, real waveform sync, an atomic apply path with tests, and free render support. What's missing is **integration and packaging**: the sync stage and the planner don't talk to each other, there's no orchestrating skill, and the planner stage has no unit tests. This doc designs the remaining ~20%.

---

## 1. What already exists (verified)

| Piece | File | State |
|---|---|---|
| **`plan_multicam` tool** — N-camera director: reads diarized transcript + face speaker-map + shot-type + frame-quality + topic sidecars, scores each camera per transcript segment, enforces min-hold, wide-reset at topic changes, emits an `Apply Multicam Plan` EDL fragment | `crates/core/src/tools/plan_multicam.rs`; agent surface `crates/core/src/awidat_mcp/mod.rs:1099` + `awidat_mcp/tools/plan_multicam.rs` | ✅ Working, exposed to agent |
| **`analyze_sync` tool** — waveform cross-correlation: builds per-asset waveforms, lag search (`best_offset`), drift estimate (two-half comparison), confidence + `manual_offset_required`, emits a `Set Sync Group` EDL fragment per candidate | `crates/core/src/tools/analyze_sync.rs`; agent surface `awidat_mcp/mod.rs:1401` | ✅ Working, exposed to agent |
| **`ApplyMulticamPlan` EDL op** — validates decisions (non-empty, finite, monotonic), atomically rebuilds the program track, stamps `multicam_source_asset` / `multicam_decision_index` / `sync_group_id` clip metadata | `crates/core/src/edl/op.rs:291,1064` · apply `edl/apply.rs:4593` (`apply_multicam_plan`, `validate_multicam_decisions`, `build_multicam_program_track`) | ✅ Implemented + **tested** (`apply.rs:12931,12997`) |
| **`SetSyncGroup` EDL op** — stamps `awidat.sync_group` effect (offset_s, speed_factor, confidence) on a clip | `edl/op.rs:408` · apply `edl/apply.rs:6925` (`apply_set_sync_group`) | ✅ Implemented |
| **Render** — a flattened program track is just sequential clips → normal concat path, no special work | `crates/render/src/timeline.rs` (concat) | ✅ Free |
| **Asset role** — `AssetRole::Camera` tags angle sources | `crates/proto/src/professional.rs:220` | ✅ Exists |

**The data the planner stands on** (sidecar `index/<indexer>/<asset>.json`, all real):
- `whisper` → `/data/segments[]` `{start,end,speaker_id}`, `/data/words[]`, `/data/speakers[]`
- `face` → `/data/per_frame[].faces[]` `{face_id,box,gaze_score}`, `/data/speaker_to_face{}`
- `shot` → `/data/shots[]` `{start_s,end_s,type,motion,framing,at_camera_ratio,whip_pan_score…}`
- `frame-quality` → per-frame sharpness/brightness/contrast
- `audio-energy` → `/data/windows[].rms_db` (used by `analyze_sync` via waveform)
- `topic` → `/data/topics[].start_s` (wide-reset boundaries)

---

## 2. The real gaps

> **Progress (2026-05-30):** **G1–G5 all shipped — design complete.**
> - **G1 + G3:** `plan_multicam` reads applied `awidat.sync_group` offsets, scores each camera at its own source time (`sync_offsets` + per-camera `cam_t`), emits `sync_group_id` + `offset_corrected` per decision, warns when no sync groups exist. Mirrored in the MCP port.
> - **G2:** `skills/multicam-director/SKILL.md` — the two-stage sync→direct workflow, guarded by `multicam_director_skill_is_graph_native` in `skill_catalog.rs` (and the generic all-skills consistency test).
> - **G4:** 3 `plan_multicam` unit tests (incl. offset-correction regression) + 4 `analyze_sync` unit tests (`best_offset` recovery, zero-confidence gate, degenerate input, zero-drift) + the catalog guard.
> - **G5:** `capability_metadata.rs::for_tool_name` now classifies `plan_multicam` (read-only evidence, requires transcript/face/shot) and `analyze_sync` (read-only, no index) — non-mutating, no-approval, non-export. Covered by `multicam_tools_are_read_only_evidence`.
> - Full `awidat-core` lib suite (815 tests) + `skill_catalog` (13) + `capability_manifest` (4) green; clippy clean.

### G1 — Sync ↔ planning are disconnected (correctness bug for real shoots) — ✅ DONE
`analyze_sync` computes per-asset **timeline offsets**, and `SetSyncGroup` stamps them — but **`plan_multicam` never reads them.** It looks up every camera's sidecars at the *same* `t_s` (`grep "sync|offset" plan_multicam.rs` → none). That's only correct if all cameras share a timebase (jam-sync / single recorder). For the common case — separate cameras/recorders started at different times — every per-camera `shot_type_at(t_s)` / `speaker_on_asset(t_s)` / `quality_score_at(t_s)` lookup is **wrong by the offset**, so angle scoring silently degrades. This is the single most important fix.

### G2 — No orchestrating skill (feature is undiscoverable / unguided) — ✅ DONE
The three tools (`analyze_sync` → `plan_multicam` → `apply_edl` → `start_render`) exist but nothing sequences them. Every other finishing capability ships as a skill (`color-corrector`, `split-edit-director`, `podcast-editor`) with a `SKILL.md` + tool allowlist that the agent discovers and the catalog test guards. There is **no `skills/multicam-director/`**, so the agent has no recipe and `skill_catalog.rs` doesn't guard the workflow.

### G3 — Planner drops sync provenance — ✅ DONE (shipped with G1)
`MulticamDecision.sync_group_id` exists and `build_multicam_program_track` propagates it to clip metadata, but `plan_multicam` emits decisions with `sync_group_id: null`. So even after syncing, the flattened program clips lose the link to their sync group → weaker `vedit` audit and no path to apply per-source offset at render.

### G4 — No tool-level tests — ✅ DONE
The apply path is tested, but `plan_multicam` (camera scoring, min-hold, topic reset) and `analyze_sync` (`best_offset` correlation, drift, confidence gating) have **zero unit tests**. Both are pure-ish functions over fixture JSON/waveforms — cheap to cover, currently uncovered.

### G5 (secondary) — Capability classification + review UX — ✅ DONE (classification; desktop review UX still deferred)
`capability_metadata.rs::for_tool_name` special-cases `plan_look_regions` (evidence/review treatment) but not `plan_multicam`/`analyze_sync`. And there's no desktop panel to scrub/override angle decisions — agent + EDL review only. Defer.

---

## 3. Design

### 3.1 Target end-to-end flow

```
                ┌─────────────┐
  raw cameras → │  index      │  whisper · face · gaze · shot · frame-quality · topic · audio-energy
  (N assets)    └─────┬───────┘
                      ▼
                ┌─────────────┐   waveform cross-correlation
                │ analyze_sync│   → per-asset {offset_s, confidence, sync_group_id}
                └─────┬───────┘   → emits Set Sync Group EDL  (review low-confidence)
                      ▼
                ┌─────────────┐   apply Set Sync Group  → awidat.sync_group effect per camera
                │  apply_edl  │
                └─────┬───────┘
                      ▼
                ┌─────────────┐   reads offsets (NEW: G1) + 5 sidecars
                │plan_multicam│   → offset-corrected per-camera scoring
                └─────┬───────┘   → decisions carry sync_group_id (G3)
                      ▼            → emits Apply Multicam Plan EDL
                ┌─────────────┐
                │  apply_edl  │   atomic program-track rebuild (existing, tested)
                └─────┬───────┘
                      ▼
                ┌─────────────┐
                │ start_render│   concat of flattened clips (existing, free)
                └─────────────┘
```

### 3.2 G1 — feed sync offsets into the planner

**Read offsets from the OTIO graph**, not from re-running sync, so the planner honors human-reviewed/overridden offsets. After `Set Sync Group` is applied, each camera clip carries the `awidat.sync_group` effect.

Add an offset map keyed by asset, built from the working timeline:

```rust
// plan_multicam.rs — new helper
/// Per-asset timeline offset (seconds) read from applied `awidat.sync_group`
/// effects. asset → offset_s. Cameras start at different times; subtract the
/// offset to convert a *program-timeline* t into each camera's *source* t.
fn sync_offsets(timeline: &Timeline) -> HashMap<String, f64> { /* walk clips, read awidat.sync_group.offset_s by source asset */ }
```

Then thread it through the per-segment lookups (today they pass bare `t_s`):

```rust
let offsets = sync_offsets(&ctx.timeline()?);            // NEW
let cam_t = |asset: &str, t: f64| t - offsets.get(asset).copied().unwrap_or(0.0);
// choose_camera(...) uses cam_t(asset, mid_s) for *each* camera's sidecar lookups
```

`choose_camera` / `speaker_on_asset` / `shot_type_at` / `quality_score_at` change from a single `t_s` to **per-camera** `cam_t(asset, t_s)`. Reference camera (offset 0) is unchanged → backward-compatible for jam-synced shoots.

**Decision:** planner reads *applied* offsets (graph state), so the flow is `analyze_sync → apply Set Sync Group → plan_multicam`. The skill enforces that order. If no sync groups are present, planner falls back to shared-timebase (today's behavior) and notes it in the response so the agent can warn.

### 3.3 G3 — carry sync_group_id through decisions

In `choose_camera`'s result and the emitted decision JSON, populate `sync_group_id` from the chosen camera's `awidat.sync_group` effect:

```rust
decisions.push(serde_json::json!({
    "start_s": seg.start_s, "end_s": seg.end_s,
    "source_asset": choice.asset,
    "sync_group_id": sync_group_of(&choice.asset, &offsets_meta),   // NEW (was absent)
    "speaker": seg.speaker, "reason": choice.reason,
    "metadata": { "traceable_source": true, "min_hold_s": min_hold_s,
                  "offset_corrected": offsets.contains_key(&choice.asset) }   // NEW
}));
```

`build_multicam_program_track` already forwards `sync_group_id` to clip metadata — no apply-side change needed.

### 3.4 G2 — `skills/multicam-director/` skill

New skill mirroring `skills/color-corrector/` structure (`SKILL.md` + `tools_allowlist`):

```
skills/multicam-director/
  SKILL.md          # workflow + verification checkpoints
  scripts/          # optional helper (e.g. summarize angle balance)
```

**`tools_allowlist`:** `["read_index", "analyze_sync", "plan_multicam", "view_frame", "apply_edl", "start_render", "poll_render"]`

**Workflow (SKILL.md):**
1. **Precheck** — confirm ≥2 `Camera`-role assets and that `whisper` (diarized) + `face` + `shot` sidecars exist; if not, instruct to index. (`read_index`)
2. **Sync** — run `analyze_sync`. Auto-apply proposals with `confidence ≥ 0.35`; surface `manual_offset_required` ones for human offset entry before applying `Set Sync Group`.
3. **Plan** — run `plan_multicam` (now offset-aware). Present the angle timeline: per-speaker coverage %, cut count, longest hold, any `offset_corrected:false` cameras (warn).
4. **Review** — `view_frame` at a few cut points to spot-check the chosen angle; let the agent/user tweak `min_hold_s` or override specific decisions.
5. **Apply** — `apply_edl` the `Apply Multicam Plan` fragment (atomic program-track rebuild).
6. **Render** — `start_render(scope=timeline)` → `poll_render`.

**Verification checkpoints:** every decision has a `source_asset` that exists; no gap/overlap (apply-op already validates monotonicity); program track duration == transcript span.

### 3.5 G4 — tests

- `plan_multicam`: fixture project with 2–3 camera sidecars (face `speaker_to_face`, shot `type`, frame-quality) + whisper segments. Assert: speaker-owning camera wins; min-hold suppresses rapid flip; topic boundary forces wide; **offset-corrected lookup picks the right camera when one camera's sidecars are shifted** (the G1 regression net).
- `analyze_sync`: synthetic waveforms with a known lag → `best_offset` recovers it within one bucket; flat/empty → confidence below gate; drift fixture → non-zero `speed_factor`/drift.
- Catalog: extend `crates/core/tests/skill_catalog.rs` with a `multicam_director_skill_is_graph_native` test asserting the skill loads and allowlists the tools above.

### 3.6 G5 — capability metadata (small) — ✅ DONE

Added `plan_multicam` and `analyze_sync` arms to `capability_metadata.rs::for_tool_name` so the agent treats their output as reviewable evidence, not a mutation: both non-mutating, no-approval, non-export. `plan_multicam` advertises `transcript`/`face`/`shot` index deps; `analyze_sync` declares no index dep (reads raw media). The desktop angle-review panel remains deferred (out of scope).

---

## 4. Files to touch

| File | Change | Size |
|---|---|---|
| `crates/core/src/tools/plan_multicam.rs` | G1 offset map + per-camera `cam_t`; G3 emit `sync_group_id`/`offset_corrected`; unit tests | **M** |
| `crates/core/src/awidat_mcp/tools/plan_multicam.rs` | mirror G1/G3 (it's a port of the above) | **S** |
| `skills/multicam-director/SKILL.md` (+ `scripts/`) | new skill | **M** |
| `crates/core/tests/skill_catalog.rs` | guard the new skill + allowlist | **S** |
| `crates/core/src/capability_metadata.rs` | classify the two tools as evidence | **S** |
| `crates/core/src/tools/analyze_sync.rs` | unit tests for `best_offset`/drift/confidence | **S** |
| _No change_ | `edl/op.rs`, `edl/apply.rs` (apply path done+tested), `render/timeline.rs` (concat covers it), OTIO nodes | — |

**Net:** one correctness fix (G1), one skill (G2), two small wiring/metadata changes (G3, G5), and tests (G4). No new EDL ops, no render work, no schema changes.

---

## 5. Scope & phasing

**In scope (v1):**
- Offset-aware planning; sync provenance on decisions; the director skill; tests.
- Audio-led switching (speaker → owning camera) with shot-type/quality tiebreak, min-hold, topic wide-reset — all existing.

**Non-goals (v1):**
- Desktop angle-timeline review UI (agent + EDL review only for now — G5 deferred).
- Live/realtime switching; >1 program track; PiP/quad-split multiview (flatten-to-single-track only).
- Auto camera-grouping types (`CameraGroup`/`AngleMetadata`) in `professional.rs` — current `AssetRole::Camera` + sync_group_id is enough; revisit only if shoots need named angle sets.
- Drift correction beyond a constant `speed_factor` (existing field; no per-frame resync).

**Phasing:**
1. ✅ **G1 + G3 + planner tests** (correctness first — offset fix + provenance, with regression net). *Done 2026-05-30.*
2. ✅ **G2 + catalog test + G4 analyze_sync tests** (skill = usable end-to-end workflow). *Done 2026-05-30.*
3. ✅ **G5** (capability polish). *Done 2026-05-30.*

---

## 6. Open questions

1. **Offset source of truth** — read applied `awidat.sync_group` effects (proposed), or also accept an inline `offsets` arg to `plan_multicam` for dry-run planning before applying sync? (Lean: graph-first, optional arg later.)
2. **Confidence gate** — `analyze_sync` uses 0.35 for `manual_offset_required`; should the skill hard-block apply below it, or allow with a logged warning? (Lean: block auto-apply, allow explicit human override.)
3. **Fallback transparency** — when no sync groups exist, planner assumes shared timebase. Surface this as a first-class `warnings[]` in the tool output so the agent never silently mis-syncs. (Yes.)
