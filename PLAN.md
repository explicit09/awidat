# PLAN.md — Terminal-First, Agent-Native Video Editing Harness (v1)

> Status: load-bearing build doc. Decisions, not options. Hard constraints (Rust core, Ratatui TUI, MCP for tools, text/JSON project format) are inputs to this plan, not subjects of debate.
>
> Audience: solo founder (Tadiwa) + the upstream Claude that will pair on the build.
>
> Read order: §1 thesis → §2 architecture → §6 tool surface → §15 build order. The rest is implementation detail keyed to those four.

---

## 1. Thesis & non-goals

**Thesis.** Long-form spoken video (podcasts, interviews, conference talks) is a domain where the agent's editorial judgment — not its mechanical reach — is the load-bearing capability. The bet is that an agent given (a) a text-first project format, (b) a clean tool surface, and (c) terminal access to the full Unix media-tooling ecosystem will out-edit a GUI-bolted-on competitor like Descript, because the GUI pre-defines what the agent can reach for and the terminal does not. The substrate (CLI engine, project format, MCP tool registry, Ratatui reference TUI) is open. The orchestrator prompts, the consumer GUI, hosted rendering, and the latest premium AI features are closed. The flagship consumer product (a podcast episode producer) ships in v1 alongside the substrate, the way Claude.ai ships alongside the API. See `conversation.txt:288-329` for the agreed framing.

**Non-goals (v1).** We are explicitly not building:

| Out of scope | Why |
| --- | --- |
| Feature-film color grading | Pro Hollywood workflow, wrong buyer, wrong tooling depth |
| Music videos / beat-driven cuts | Taste depends on rhythm modeling we don't have |
| Mainstream TikTok / Reels short-form | Opus Clip / Submagic own the wedge |
| Enterprise media (legal discovery, surveillance) | Wrong sales motion for indie |
| Generative video (text-to-video, gen-extend) | Different product entirely |
| Multi-user collaboration / cloud editing | v2+ closed product |
| GUI-as-source-of-truth | Antithesis of the architecture |

Per `conversation.txt:392-411`: realistic outcome is "best editor for long-form spoken content," not "next Descript." If the v1 demo only does silence removal, we have failed — the agent must make ≥3 non-mechanical editorial decisions per session (which moments are highlights, where to cut between them, whether b-roll fits). See §14.

---

## 2. Architecture (Rust, decided)

### 2.1 Cargo workspace

Modeled on Codex (`_notes/codex.md` §1, §2 / `codex-rs/`) and Goose (`_notes/goose.md` §1 — 8 crates). Eight crates, deliberately:

```
awidat/
├── Cargo.toml                 # workspace root
├── crates/
│   ├── proto/                 # project-format types (OTIO superset), serde models, schema
│   ├── core/                  # agent loop, session, history, compaction, turn diff tracker
│   ├── tools/                 # tool registry, ToolHandler trait, individual tool impls
│   ├── mcp/                   # MCP client (stdio + http), extension manager
│   ├── sandboxing/            # per-OS sandbox (seatbelt / landlock+seccomp)
│   ├── tui/                   # Ratatui app: chat pane, timeline pane, diff view, approval modal
│   ├── cli/                   # clap entry point: awidat init|index|chat|render|skills|validate
│   └── render/                # ffmpeg wrapper, proxy renders, preview job manager
└── skills/                    # bundled reference skills (podcast-episode-producer etc.)
```

**Why eight, not three:** Goose proves modular crates pay for themselves at the boundary where an external implementer wants to swap a piece (someone else's TUI, someone else's sandbox). Eight is the smallest cut where each boundary is real. Codex has ~70 crates; that is over-fitting for our scale.

**Async runtime: tokio.** Per Codex (`codex-rs/Cargo.toml` workspace deps) and Goose. Streaming model events, parallel tool dispatch (`FuturesOrdered` per `_notes/codex.md:55-59`), long-running render jobs all want async. No alternative is even close.

**Single-binary distribution.** `cargo build --release --bin awidat` produces one binary that statically links the engine, tools, sandbox, and TUI. MCP servers (Python) ship separately and are discovered via config — see §5/§6.

### 2.2 Engine + thin client split

Per OpenCode (`_notes/opencode.md` §1) and Cline. **v1: engine + TUI in one binary**, communicating in-process via channels. The HTTP+WebSocket layer lands in v2 when the web GUI ships.

Why deferred: v1 only has one client (the TUI), and compiling the same engine into the binary saves the deployment story. But the *internal* boundary between `core` and `tui` is the same boundary an HTTP server would expose: TUI never calls into core except via a published trait (`AgentSession`) and an event stream. When v2 wraps that trait in axum routes, no engine code changes.

**Rule:** The TUI never reaches into agent state directly. It subscribes to events and submits commands. This is the OpenCode shape (`/packages/opencode/src/server/server.ts:49-136`) implemented as an in-process channel for v1, an HTTP+WS server for v2. Crush's pubsub.Event pattern (`_notes/crush.md` §2 — `pubsub.Event[message.Message]`) is the right Rust shape: a `tokio::broadcast` channel of `SessionEvent` enum variants.

### 2.3 Where state lives

Filesystem-first, the way a git repo is filesystem-first.

| Path | Purpose | Owned by |
| --- | --- | --- |
| `<project>/project.otio.json` | Source of truth (OTIO superset) | Engine writes via `apply_edl` |
| `<project>/edit-plan.json` | Structured edit plan + status | Engine writes; agent updates status |
| `<project>/episode-notes.md` | Agent's running editorial reasoning | Agent appends |
| `<project>/index/<asset>.json` | Footage index sidecar (per asset) | Indexer MCP servers write |
| `<project>/renders/` | Proxy + final renders | Engine writes |
| `<project>/.awidat/session.sqlite` | Conversation history, turn log | Engine |
| `~/.cache/awidat/` | Whisper models, MCP-server caches | Tools |
| `~/.config/awidat/config.toml` | Model provider, API keys, MCP extensions | User |
| `~/.config/awidat/skills/` | User-installed skills | User |

The project directory is fully self-contained and git-friendly. Everything in `.awidat/` is ephemeral and `.gitignore`d by default. This is the OpenCode session-as-SQLite pattern (`_notes/opencode.md` — durable sessions) but scoped to a project.

---

## 3. Project format

**Decision: OTIO superset.** OpenTimelineIO 1.x JSON, with a single namespaced metadata key `awidat` carrying our extensions. No bespoke binary format.

Why OTIO over a custom JSON:
- OTIO is JSON. Diffable, hand-editable, git-friendly — ticks every box from `conversation.txt:64-67`.
- Adapters exist to Premiere, Resolve, FCP — escape hatch when a user outgrows us.
- It's a real spec maintained by the Academy Software Foundation. We don't have to evolve a schema.
- Rust crate `opentimelineio` is workable; we may end up writing our own minimal serde models against the spec rather than depending on the C++ binding. Decision: write our own pure-Rust types in `crates/proto/`, validate against the JSON schema on every read/write.

Why a superset, not raw OTIO:
- We need `awidat.reasoning` per clip ("why did the agent pick this cut?")
- We need `awidat.edit_plan_ref` linking a clip change back to a plan item
- We need `awidat.anchor` — content-locator metadata for `apply_edl` (audio fingerprint, transcript snippet, scene-change marker) — the single most important schema extension. See §6.

Why not a brand-new format:
- The synthesis is explicit (`synthesis.md` §"Project format") that diffability and Git-friendliness are the unfair advantage. OTIO already has those. Inventing a new format buys nothing and pays a long ecosystem tax.

**Project layout.** A project is a directory, not a single file. The directory is a unit a user can `git init`, share, hand-edit. The conversation explicitly endorses this (`conversation.txt:84-88`).

**Schema sketch** (the load-bearing extensions):

```json
{
  "OTIO_SCHEMA": "Timeline.1",
  "name": "ep-014-rough-cut",
  "tracks": { "...standard OTIO..." },
  "metadata": {
    "awidat": {
      "version": "0.1",
      "source_assets": ["raw/ep-014-cam-a.mp4", "raw/ep-014-audio.wav"],
      "anchors": {
        "<clip-uuid>": {
          "transcript_snippet": "and that's when she said the thing about Stripe",
          "scene_change_index": 17,
          "audio_fingerprint_sha": "abc123…",
          "energy_curve_hash": "def456…"
        }
      },
      "reasoning": {
        "<clip-uuid>": "Kept this; energy peak at 4:12 + emotional reaction"
      },
      "edit_plan_id": "plan-2026-05-02-3"
    }
  }
}
```

**Validation hook.** Every `apply_edl` round-trips through `proto::validate()` before the new file lands. Any failure surfaces as `FunctionCallError::RespondToModel` (per `_notes/codex.md` `function_tool.rs:1-11`) so the model self-corrects.

---

## 4. State files alongside the project

Anthropic's long-running pattern (`synthesis.md` §"Long-horizon work" / `sources/anthropic-long-running-harnesses.md`) is the spine. **JSON for state Claude shouldn't overwrite. Markdown for state Claude updates.** Applied:

### 4.1 `edit-plan.json` (JSON, append-status)

```json
{
  "schema": "edit-plan.v1",
  "brief": "90-second highlight clip; emphasis on Stripe segment + laughs",
  "items": [
    {
      "id": "p-001",
      "kind": "select_highlight",
      "source_range": { "asset": "raw/ep-014.mp4", "start_s": 723.4, "end_s": 802.1 },
      "target_position": "intro",
      "rationale": "Best laugh in episode, sets tone",
      "status": "applied",
      "applied_at": "2026-05-02T10:14:33Z",
      "result_clip_uuid": "c-9f2…"
    },
    {
      "id": "p-002",
      "kind": "remove_dead_air",
      "source_range": { "asset": "raw/ep-014.mp4", "start_s": 1207.0, "end_s": 1216.4 },
      "rationale": "9.4s pause after question",
      "status": "pending"
    },
    {
      "id": "p-003",
      "kind": "insert_broll",
      "anchor": { "transcript_snippet": "the city skyline reminded me" },
      "rationale": "Visual reinforcement of recall moment",
      "status": "needs_user_input"
    }
  ]
}
```

Statuses: `pending | applied | rejected | needs_user_input | failed`. The agent updates status fields in place; it does not rewrite the whole file. This mirrors the `feature_list.json` pattern (`synthesis.md` table row 3).

### 4.2 `episode-notes.md` (Markdown, append-only by convention)

The agent's running editorial reasoning. Appended after every meaningful turn. Convention enforced via system prompt:

```markdown
## 2026-05-02 14:33 — Brief check-in
Listened through 0:00–8:00 via transcript + energy curve.
Best emotional beats:
- 4:12 — Stripe story
- 11:34 — laugh after pause
- 27:50 — disagreement, raised voices
Plan: lead with 4:12, bridge to 11:34, close with 27:50.
Open question: does target audience know the Stripe context? If not, add b-roll at 4:08.
```

This is `claude-progress.txt` translated for video. It's where the agent's *taste* is legible, which is essential because that's what we're going to instrument later for taste-learning (per `conversation.txt:152-154`).

### 4.3 `index/` — footage index sidecars (JSON)

One file per source asset: `index/ep-014-cam-a.mp4.json`. Schema in §5. These are written *by indexer MCP servers* (whisper, scenedetect, audio-energy), not by the agent. The agent reads them via `find_moment` and `inspect_clip` tools. Treated as immutable per session (rebuild on source asset change).

---

## 5. The footage index

The single biggest determinant of agent quality. Per the synthesis (`synthesis.md` §"Context management"): "metadata index quality is the rate-limiting step on agent quality." Investing here is investing in the agent's IQ.

### 5.1 v1 contents (per source asset)

| Channel | Producer | Cost | Output shape |
| --- | --- | --- | --- |
| Transcript with word-level timestamps + speaker diarization | `whisper-mcp` (Python, faster-whisper or whisperx) | High (≈ realtime / 4 on M-series) | JSON: words[], segments[], speakers[] |
| Shot boundaries | `scenedetect-mcp` (Python, PySceneDetect) | Low (≈ realtime / 30) | JSON: shots[] with start/end frame, change type |
| Audio energy + silence | `audio-energy-mcp` (Python, librosa or auto-editor) | Low | JSON: rms over 100ms windows, silences[] |
| Topic segmentation | `topic-mcp` (LLM pass over transcript) | Medium (one Claude call) | JSON: topics[] with start/end, label, summary |

### 5.2 v2 candidates (deferred)

- Face / object tags (CLIP or YOLO MCP server)
- Scene captions (vision LLM pass)
- Embedding index for semantic b-roll search (separate vector DB, optional)

### 5.3 Storage

**Sidecar JSON in `<project>/index/`, one file per asset.** Plain files, diffable, easy to rebuild. SQLite is deferred to v2 if query perf demands it; for a 4-hour podcast, JSON files load in tens of milliseconds.

Index files are *not* checked into git by default (regenerable, large). The `.gitignore` template includes `index/`.

### 5.4 Build pipeline

`awidat index <video>` runs all four indexers in parallel via the MCP transport, collects results, writes sidecars. Idempotent: re-running on an unchanged asset is a no-op (hash check on input).

The indexers are **Python MCP servers** per the constraint: ML lives in Python, the engine doesn't host it. Goose's pattern exactly (`_notes/goose.md` §6c "Hybrid approach"). The engine's job is dispatch and result-handling, not running whisper.

---

## 6. Tool surface (the most important section)

**Discipline: 12 tools in v1, not 30.** Each justified by a load-bearing role. Per SWE-agent (`_notes/swe-agent.md` §2): consolidate, don't sprawl. Per Codex (`_notes/codex.md` §2): every tool justifies its existence.

### 6.1 The list

| # | Tool | Shape | Role |
| --- | --- | --- | --- |
| 1 | `apply_edl` | Freeform Lark grammar (envelope) | The single most important tool. See §6.2. |
| 2 | `view_timeline` | `view_timeline(start_s, end_s, lines: u32)` | Windowed timeline reader (per `_notes/swe-agent.md` §2.1) |
| 3 | `find_moment` | `find_moment(query, scope?, limit?)` | Semantic search over the index. Returns `[{asset, start_s, end_s, transcript_snippet, score}]` only |
| 4 | `inspect_clip` | `inspect_clip(clip_uuid \| asset_path)` | Codec/dims/duration/audio waveform thumbnail. Bounded output |
| 5 | `view_frame` | `view_frame(asset, t_s)` | Returns a single frame as image input modality (multimodal) |
| 6 | `list_assets` | `list_assets(scope?: "raw"\|"renders"\|"all")` | Paginated, capped per-entry length |
| 7 | `read_index` | `read_index(asset, channel: "transcript"\|"shots"\|"energy"\|"topics")` | Reads sidecar JSON, returns succinct shape |
| 8 | `start_render` | `start_render(scope: "preview"\|"segment"\|"full", range?)` | Returns `job_id`, async — `_notes/codex.md` §6.1 |
| 9 | `poll_render` | `poll_render(job_id)` | Status + frame strip + log excerpt |
| 10 | `update_plan` | `update_plan(items: [...])` | Per `_notes/codex.md` §2 — TODO list |
| 11 | `request_user_input` | `request_user_input(question, context?)` | "Should this be a hard cut or a 0.3s dissolve?" |
| 12 | `bash` | `bash(command: string[], workdir?, timeout_ms?)` | Sandboxed escape hatch. Restricted argv per safe-list. |

**MCP-backed (not in the binary): the four indexers.** They appear to the agent as additional tools (`whisper.transcribe`, `scenedetect.detect`, etc.) only when the agent needs to (re-)index. Most editing sessions never call them.

**Skipped vs. Codex.** No `web_search`, no `image_generation`, no multi-agent spawn tools (see §8). No `apply_patch` — it's `apply_edl`.

### 6.2 `apply_edl` — the load-bearing tool

This is the `apply_patch` of the video harness. Three properties to mirror, lifted directly from Codex (`_notes/codex.md` §2):

**Property 1: context-locator semantics, not absolute timestamps.** Every clip change identifies its target by content anchors — transcript snippet, audio fingerprint, scene-change index — not by raw frame numbers. A timeline drifts as edits accumulate; "trim 0.5s off the clip whose audio matches `<sha>`" survives an upstream insertion. Frame numbers do not. This is exactly Codex's leading/trailing-context pattern from `apply-patch/src/seek_sequence.rs`.

**Property 2: single envelope for the whole turn's edits.** One `apply_edl` call carries Add/Trim/Move/Delete/InsertBRoll for N clips. Atomic review and undo. Mirrors Codex's `*** Begin Patch ... *** End Patch` structure.

**Property 3: streaming verifier.** As the model types the EDL, parse it incrementally and emit `EdlApplyUpdated` events with the running parsed change set. UI renders a live preview of the proposed timeline state. 500ms throttle, exactly the Codex pattern (`handlers/apply_patch.rs:52, 84-106`).

**Format choice: freeform Lark grammar, not JSON.** Per `_notes/codex.md` §6.2 — "JSON-escaping multi-line content is miserable." The grammar lives in `crates/tools/grammars/edl.lark`. Tool description includes "FREEFORM tool — do not wrap in JSON."

**Sketch of the format:**

```
*** Begin EDL
*** Trim Clip
@@ anchor: transcript_snippet="and that's when she said the thing about Stripe"
- end: 80.4
+ end: 78.9
*** Insert BRoll
@@ anchor: transcript_snippet="the city skyline reminded me"
+ asset: broll/skyline_dusk.mp4
+ duration_s: 2.4
+ position: overlay
*** Delete Clip
@@ anchor: transcript_snippet="um so what i was saying"
*** Move Clip
@@ anchor: clip_uuid=c-9f2…
+ to_position: 4
*** End EDL
```

**Validation pipeline (every `apply_edl` call):**

1. Lark parse → structured `EdlChange` set.
2. Anchor resolution → does each anchor still match the current project? Use audio-fingerprint or transcript-snippet to locate. If not found: `RespondToModel("anchor not found: …; closest match was at <s>")`.
3. Schema validation → frame ranges in bounds, asset paths exist, parameter ranges (gain ∈ [-30, +12] dB, etc.).
4. OTIO round-trip — apply the changes to a clone, validate against OTIO schema. Reject if invalid.
5. Hooks — `pre_apply_edl` user-defined hook can reject (per Codex hooks pattern, `_notes/codex.md` §5).
6. Commit to disk; emit `TimelineDiff` event.

Steps 2–4 are the **linter on edit** pattern from SWE-agent (`_notes/swe-agent.md` §2.2). Bad EDLs never land. Failures route back to the model as `RespondToModel` with actionable strings (per Codex error-string conventions, `function_tool.rs:1-11`).

### 6.3 Other tool design notes

- **`view_timeline` is windowed** (`_notes/swe-agent.md` §2.1). Default window 60 seconds (≈ 80 lines of dense text). Each line: clip-id | source | source-range | duration | annotations. State command (per `_notes/swe-agent.md` §3 "State commands") returns current visible window in every observation so the agent always knows where it is.
- **`find_moment` returns paths/ranges only, no embedded thumbnails.** SWE-agent's grep lesson (`_notes/swe-agent.md` §2.3 "find_file"). One line per match.
- **`start_render` follows the `unified_exec` pattern** (`_notes/codex.md` §6.1) — returns immediately with a job handle. `poll_render` returns status. The model is expected to interleave: kick off a preview render, keep refining, check the render later. Render results carry frame strip (4-8 thumbnails) and ffmpeg stderr excerpt (middle-truncated per `_notes/codex.md` §3 — `EXEC_OUTPUT_MAX_BYTES = 1 MiB`).
- **`bash` has a safe-list.** `ffprobe`, `ffmpeg` (with restricted output paths), `rg`, `find`, `head`, `sort`, etc. Anything unsafe escalates per the orchestrator pattern (§10).
- **Tool errors are `RespondToModel`-default.** Three-way enum copied verbatim from `codex-rs/core/src/function_tool.rs:1-11`. `Fatal` reserved for "project file corrupt" / "MCP server died." See §7.

### 6.4 Per-tool design table (for implementation)

```rust
// crates/tools/src/registry.rs — sketch
pub trait ToolHandler: Send + Sync {
    type Output: ToolOutput + 'static;
    fn name(&self) -> &str;
    fn is_mutating(&self, _inv: &ToolInvocation) -> bool { true } // default safe
    fn handle(&self, inv: ToolInvocation) -> BoxFuture<Result<Self::Output, FunctionCallError>>;
    fn create_diff_consumer(&self) -> Option<Box<dyn ArgumentDiffConsumer>> { None }
}
```

Mirrors `_notes/codex.md` §2 `ToolHandler` trait verbatim. `is_mutating` gates parallel dispatch (mutating calls run sequentially via a turn-level gate; reads run in parallel via `FuturesOrdered`).

---

## 7. Agent loop

### 7.1 Shape — turn-based with streaming inner loop

Copied from Codex (`_notes/codex.md` §1 / `codex-rs/core/src/session/turn.rs:137-665`). Two nested loops:

**Outer turn loop** — one iteration per user message:
1. Run pre-turn hooks.
2. Drain pending user input (the user can type ahead while the agent runs).
3. Build prompt: system + project context fragments (delta-injection, not full re-emit) + history.
4. Call `run_sampling_request`.
5. Decide continue: if model emitted only an assistant message and no pending input, end turn. Otherwise loop.

**Inner sampling loop** — streaming model events:
1. Open the model stream.
2. `loop { match stream.next() { ResponseEvent::OutputItemDone(tool_call) => ..., Completed => break } }`.
3. Tool calls dispatch into a `FuturesOrdered<BoxFuture<ToolResult>>` so non-mutating reads run in parallel; mutating writes wait on a tool-call gate.
4. As tool results arrive, fold them into the response stream so the model sees them in order.

**Cancellation.** `tokio_util::sync::CancellationToken`, `or_cancel` everywhere on hot paths. Render jobs especially must cancel cleanly. Pattern verbatim from `_notes/codex.md` §1 "Interruption."

### 7.2 Error handling — `FunctionCallError` enum

Copy `codex-rs/core/src/function_tool.rs:1-11` directly:

```rust
pub enum FunctionCallError {
    RespondToModel(String),  // route back to model as tool-output, model self-corrects
    MissingLocalShellCallId, // protocol bug; should never reach model
    Fatal(String),           // bubble up, kill the turn
}
```

**Default verdict for any tool error: `RespondToModel`.** "Anchor not found: nearest match was at 78.4s. Try anchor: transcript_snippet=…" is far more useful than crashing. Reserve `Fatal` for: project file corruption, sandbox bootstrap failure, MCP server dead.

Error string discipline (per `_notes/codex.md` §5): short, imperative, actionable. Tells the model how to fix the call, not just "validation failed."

### 7.3 Compaction

Two-point strategy from `_notes/codex.md` §1 (`compact.rs`):

- **Pre-sampling compaction** — if total usage > limit before a sampling request, compact before the next call (`run_pre_sampling_compact`).
- **Mid-turn compaction** — if usage hits limit during a turn and the model still owes a follow-up (`run_auto_compact`), compact in place and reset cache state.

The summary prompt is retargeted for video (per `synthesis.md` §"Context management"): "summarize what edits the agent has made so far and why," capped at 20k tokens. Project state lives in the timeline file, not in the summary — the summary only covers tool-output history and editorial reasoning.

After compaction, the in-memory cache is invalidated and the next turn re-injects fragments.

### 7.4 Per-turn diff tracking

`TimelineDiffTracker` — direct port of Codex's `TurnDiffTracker` (`_notes/codex.md` §3 / `codex-rs/core/src/turn_diff_tracker.rs:25-31`). Snapshots the timeline at session start; every `apply_edl` updates the tracker; the TUI's diff view reads the aggregated diff. This is what lets the human review "what the agent changed in this session" as a single PR-style diff (per `synthesis.md` §"Diffability is the unfair advantage").

---

## 8. Sub-agents and orchestration

The Anthropic-Research vs Cognition disagreement is real and load-bearing. Decision:

### 8.1 Read-only sub-agents — yes

For **footage analysis only**. Dispatched per topic-segment (5–15 min chunks). Each returns a 1–2k token summary: "speaker mentions Stripe at 2:34, gets visibly excited at 4:12, dead air from 1:17–1:24." The orchestrator never sees the raw segment, only summaries. This is the Anthropic Research pattern (`_notes/anthropic-multi-agent-research-system.md` mentioned in synthesis index).

Granularity: **topic-segment**, not scene, not clip. Topics are coarse enough that summaries are useful; scenes are too fine-grained and the dispatch overhead dominates.

### 8.2 Write-capable sub-agents — no

Forbidden in v1. **Single-threaded writes** to the timeline, per Cognition (`_docs/general/cognition-dont-build-multi-agents.md`): "Actions subagent 1 took and the actions subagent 2 took were based on conflicting assumptions." Two sub-agents cutting in parallel will produce inconsistent timelines.

The lead agent serializes all `apply_edl` calls. Read sub-agents propose; the lead disposes.

### 8.3 Architect/Editor split (Aider) — try, with an empirical eval

Per `_notes/aider.md` §2 and `synthesis.md` §"Architecture patterns" — a strong reasoning model proposes the change in prose; a cheaper editor model produces the precise EDL.

For video, this is **delicate**. The Aider Editor only needs to translate a directive into a SEARCH/REPLACE block — a syntactic transformation. The video Editor would need to translate "tighten this segment by ~3s" into a precise EDL with frame-accurate cut points, which requires *taste* — the cut should land on a beat, after a breath, before a gesture. That's not a syntactic step.

**Decision: try the split for v1, but flag as open question.** Use Sonnet as Architect, Haiku as Editor. Eval: produce a 90s highlight clip with split vs. without on five real podcast hours; ask three reviewers to blind-rank. If the split version is materially worse, kill it for v2 and use a single-model loop. Recorded in §16.

---

## 9. Verification stack

Cheap → expensive, gated milestones (per `synthesis.md` §"Verification"). SWE-agent's tier pattern (`_notes/swe-agent.md` §2.2 "linter on edit").

### 9.1 Tier 1 — every edit, mandatory (v1)

Runs synchronously inside `apply_edl`. < 100ms target.
- EDL schema validation (Lark parse).
- Anchor resolution (does the snippet still exist in the source asset?).
- Asset existence check.
- Frame range bounds check.
- OTIO schema round-trip validation.

Pass or rejection routes via `FunctionCallError::RespondToModel`.

### 9.2 Tier 2 — per feature, mandatory (v1)

Runs when the agent declares a plan item complete (sets `status: applied` on an `edit-plan.json` item).
- Proxy render of the affected segment (low-res, single-pass ffmpeg, ≤ 5s for a 30s segment).
- Transcript-vs-cut check: does the cut create a mid-word break? (Run the rendered audio through whisper; compare against expected segment boundaries.)
- Audio sync check: A/V offset within 40ms.
- Auto-result fed back as a tool output. Failures roll the plan item to `failed` with diagnostics.

### 9.3 Tier 3 — milestones, mandatory before "done" (v1)

Before the session emits a final render:
- Full render at target codec.
- Human taste sign-off via TUI viewer (the diff view + render preview pane). The user explicitly approves via the approval modal.

### 9.4 v2 candidates (deferred)

- Critic-LLM watching the render and scoring pacing/energy.
- Beat detection against music bed.
- Per-cut "feels right" learned model (this is the taste-learning track).

### 9.5 Discipline rule

Per the synthesis warning (`synthesis.md` §"What the coding harnesses got wrong" — tests-as-cargo-cult): **the agent is not allowed to lower the bar for verification.** Verification config lives outside the agent's writable scope (see §10). The agent cannot edit `verification.toml` to skip tier-2 checks. Adversarial verification is the rule.

---

## 10. Sandbox & permissions

Per Codex's three-platform pattern (`_notes/codex.md` §4):

### 10.1 Per-OS implementation (v1)

- **macOS**: Apple's seatbelt (`/usr/bin/sandbox-exec`), hardcoded path to defend against PATH shadowing (`_notes/codex.md` §4 — `MACOS_PATH_TO_SEATBELT_EXECUTABLE`). Base policy: `(deny default)` + writable subpath = project directory + render cache. Network policy layered separately.
- **Linux**: bubblewrap + seccomp + Landlock LSM via the self-re-exec helper pattern. Whitelist `/dev/dri` for hardware encode if present.
- **Windows**: out of scope for v1. (Solo founder, two-of-three is fine.)

**Docker is skipped for v1.** Too heavy for an indie local tool. The Codex pattern of native-sandbox-or-bust is the right one for our scale.

### 10.2 Approval modes

Copy Codex's `AskForApproval` enum verbatim:
- `Never` — full autonomy; sandbox failures are terminal.
- `OnFailure` — auto-run sandboxed; on denial, ask the user.
- `OnRequest` — model explicitly requests escalation per call.
- `UnlessTrusted` — ask the user unless the call is on the safe-list.

Default for v1: `OnFailure`. The user is a developer running on their own machine; full autonomy with escalation-on-denial is the right ergonomic.

### 10.3 Try-sandboxed-then-escalate

The orchestrator pattern (`_notes/codex.md` §4 — `orchestrator.rs:126-380`): try sandboxed first, on `SandboxErr::Denied` ask user, retry unsandboxed. Apply to all tool calls; especially relevant for `bash` (user might want to run an arbitrary CLI tool the agent picked up that wasn't on the safe-list).

### 10.4 Trust model documentation

`docs/TRUST.md` (open-source, ships in repo) explicitly states:
- The agent runs locally.
- Project files are read-write inside the sandbox.
- Source assets outside the project are read-only by default.
- Network egress is denied by default; user enables for specific operations (downloading models, calling Anthropic API).
- API keys live in the OS keychain, not in config files.

---

## 11. Skills / recipes

Per Anthropic Skills (`_docs/claude-code/agent-skills.md`, `_notes/codex.md` mention of `skill_instructions.rs`). Three-level progressive disclosure:
- L1: `name` + `description` always in system prompt.
- L2: full `SKILL.md` loaded only when relevant (the model self-selects).
- L3: bundled scripts and files loaded on demand by the script when it runs.

Claude Code v2.1.63 unified skills and slash commands (`_docs/claude-code/skills-and-slash-commands.md`); we adopt the unified shape from day one. A skill *is* a command.

### 11.1 File structure

```
~/.config/awidat/skills/<name>/
  SKILL.md           # YAML frontmatter (name, description, version) + body
  scripts/           # Optional bundled scripts (Python preferred for ML)
  templates/         # Optional templates (e.g. EDL skeletons)
```

### 11.2 v1 reference skills

Bundled with the binary (under `skills/` in the workspace) and copied to user config on first run.

| Skill | Purpose | What's in it |
| --- | --- | --- |
| `podcast-episode-producer` | Port from existing macOS app | SKILL.md describing the editorial style; scripts/intro_outro_detector.py |
| `interview-tightener` | Tighten an interview by 20–30% without losing meaning | SKILL.md + scripts/dead_air_filter.py |
| `trailer-cutter` | Produce a 60–90s highlight clip from a long form | SKILL.md describing pacing rules + scripts/energy_peak_finder.py |
| `b-roll-suggester` | Embedding-based search over a b-roll library | SKILL.md + scripts/embedding_search.py (uses sentence-transformers) |

The `b-roll-suggester` skill is the canonical example of "code where reliability matters more than reasoning" (per `synthesis.md` §"Skills"). The agent doesn't try to do embedding match in its head; it calls the script.

### 11.3 User-extensibility

Users drop new skill folders into `~/.config/awidat/skills/`. The engine discovers them on session start (the available_skills_instructions.rs pattern from Codex). This is the open-ecosystem version of "tool the agent calls."

---

## 12. GUI relationship (v2, not v1)

### 12.1 v1 — Ratatui only

Three panes (per `_notes/crush.md` §1 layout):
- **Chat pane** (center) — message history + tool call rendering with nested spinners. Stream incremental updates per `_notes/crush.md` §2.
- **Timeline pane** (top right or full-width toggle) — windowed view of current OTIO state, ASCII waveform underneath. Diff view overlay shows pending changes from `apply_edl` in real time.
- **Editor pane** (bottom) — user input box, dynamic height (3–15 lines per Crush).

Approval modal (per `_notes/crush.md` §4): popup with diff, three options: Allow / Allow for Session / Deny. Bound to single keys (a/s/d).

ASCII frame preview via [chafa](https://hpjansson.org/chafa/) or sixel for terminals that support it; otherwise just a frame description string.

### 12.2 v2 — web GUI

**Decision: pure web (browser tab opening to localhost:port), not Tauri.** Reasons:
- No native dependencies; one engine binary serves both TUI and web.
- The same web bundle hosts later as part of the closed cloud product.
- Tauri adds platform-specific build pain we don't need.

The engine grows an HTTP+WS server (Hono-style routes per `_notes/opencode.md` §1.2) and a `--serve` flag that starts it and pops a browser tab. The web GUI is a thin React app that subscribes to `SessionEvent` and submits commands.

### 12.3 Hard rule

**The GUI never does what the terminal can't.** Every GUI feature has a CLI equivalent first. This is the conversation's load-bearing rule (`conversation.txt:96-100`).

---

## 13. Open core boundaries

### 13.1 Open from day one (Apache 2.0)

- The OTIO superset spec (`crates/proto/`).
- The CLI engine binary (Rust).
- Tool definitions and the MCP server protocol bindings.
- Reference Python MCP servers (whisper, scenedetect, audio-energy, topic).
- The reference Ratatui TUI.
- Reference skills (podcast-episode-producer, etc.).

License choice: **Apache 2.0** for explicit patent grant and compatibility with both indie and enterprise users. Per the Supabase shape (`conversation.txt:209-227`).

### 13.2 Closed

- Hosted orchestrator (cloud render farm, multi-tenant queue).
- The polished web GUI (the consumer-facing surface).
- Taste/orchestration system prompts (the agent's editorial judgment).
- Premium AI features (vision-heavy ops, latest frontier models).
- Cloud rendering at scale.
- Aggregated taste-learning datasets.

### 13.3 Repo strategy

Single open-source repo `awidat/` for everything in §13.1. The closed components live in private repos (`awidat-orchestrator/`, `awidat-web/`, `awidat-prompts/`) that depend on the open core via cargo and npm registries.

---

## 14. v1 demo (the proof)

Per `conversation.txt:235-245` — the smallest provable thing first.

**Input:** one hour of raw podcast footage (MP4 + separate audio track).

**Output:** a 90-second highlight clip exported as MP4.

**Process:**
1. `awidat init podcast-ep-014/` creates the project directory.
2. `awidat index podcast-ep-014/raw/*.mp4` runs the indexer pipeline. ~5 minutes.
3. `awidat chat podcast-ep-014/` opens the TUI.
4. User: "Make me a 90-second highlight clip. Lead with the funniest moment."
5. Agent (via `podcast-episode-producer` skill): reads the index, identifies candidate moments via `find_moment` and `read_index(channel=energy)`, drafts an EDL via `apply_edl`, renders a preview via `start_render`, presents the diff in TUI.
6. User reviews diff in TUI, approves cuts, types "ship it."
7. Engine renders the final at full resolution and exports `podcast-ep-014/renders/highlight-90s.mp4`.

**Definition of "yes the architecture works":**

The agent makes ≥3 non-mechanical editorial decisions, at least three of which the user agrees with:
1. Which moments are highlights (not just "highest energy" — taste).
2. Where to cut between them (a beat that flows; not just butt-edits).
3. Whether b-roll fits anywhere (and if so, which b-roll).

If the agent only produces silence-removal, this is a fancy macro recorder and we have failed. Per `conversation.txt:136-138`.

---

## 15. Build order — 8 weeks to v1 demo

Each week ships one concrete artifact. Weeks 1–2 are flagged as the highest-risk stretch given Rust ramp uncertainty.

### Week 1 — Skeleton + project format

- Cargo workspace with eight crates created.
- `crates/proto/` — OTIO superset types, serde models, JSON schema validator.
- `crates/cli/` — clap entry point with `init`, `validate`, `version` subcommands.
- **Shippable:** `awidat init <path>` creates a project directory; `awidat validate <path>` round-trips the OTIO file.

### Week 2 — Footage index pipeline

- Python MCP servers: `whisper-mcp`, `scenedetect-mcp`, `audio-energy-mcp`, `topic-mcp`. Each ≤ 200 LOC, stdio transport.
- `crates/mcp/` — minimal MCP client (stdio + JSON-RPC) sufficient to call `tools/list` and `tools/call`.
- `awidat index` subcommand: launches all four MCP servers in parallel, writes sidecars to `<project>/index/`.
- **Shippable:** `awidat index podcast-ep-014/raw/*.mp4` produces full index in `<project>/index/`.

### Week 3 — Agent loop

- `crates/core/` — outer turn loop + inner streaming sampling loop, per Codex shape.
- Anthropic API client (using `anthropic` Rust crate or hand-written; pick whichever is current).
- `FunctionCallError` enum verbatim from Codex.
- Tool registry skeleton with one tool: `bash` (sandbox-stubbed for now).
- `awidat chat <project>` subcommand: text-only REPL.
- **Shippable:** `awidat chat` opens a REPL where you can talk to the agent and it can run `bash` commands.

### Week 4 — Tool surface v1

- Implement `apply_edl` (Lark grammar, anchor resolution, OTIO round-trip validation, streaming arg consumer).
- Implement `view_timeline`, `find_moment`, `read_index`, `inspect_clip`, `view_frame`, `list_assets`, `update_plan`, `request_user_input`.
- Implement `start_render` / `poll_render` via ffmpeg subprocess (proxy renders only).
- `crates/render/` — ffmpeg wrapper.
- Validation hook on `apply_edl`.
- **Shippable:** agent can take an instruction and produce an EDL change that validates and renders a preview.

### Week 5 — Ratatui TUI

- `crates/tui/` — three-pane layout, chat pane with streaming nested spinners (Crush pattern), timeline pane with windowed view, diff view overlay.
- Approval modal (Allow / Allow for Session / Deny).
- Event subscription via `tokio::broadcast` to `core::SessionEvent`.
- Frame preview via `chafa` shell-out (graceful fallback to text).
- **Shippable:** full TUI for the demo flow.

### Week 6 — Skills + first three skills

- `crates/core/skills/` — skill loader, progressive disclosure (L1/L2/L3), system-prompt injection.
- Skills: `podcast-episode-producer`, `interview-tightener`, `b-roll-suggester` (with embedding-search Python script).
- `awidat skills run podcast-episode-producer <project>` end-to-end.
- **Shippable:** the canonical demo flow runs entirely via the skill.

### Week 7 — Verification + sandbox + polish

- `crates/sandboxing/` — macOS seatbelt implementation + Linux landlock helper.
- Try-sandboxed-then-escalate orchestrator (Codex pattern).
- Verification tier 1 (every edit) and tier 2 (per feature).
- Error message polish: every `RespondToModel` error string short, imperative, actionable.
- Hooks: `pre_apply_edl`, `post_apply_edl`, `stop`.
- **Shippable:** agent fails gracefully; errors route back to model and self-correct.

### Week 8 — Demo dry-run

- Run on three real hours of podcast footage from the user's archive.
- Bug fixes, prompt tuning, error-message tuning.
- Record the v1 demo end-to-end.
- **Shippable:** the v1 demo, recorded.

### Risk flag

Weeks 1–2 are the highest risk if Rust ramp is steeper than expected. Mitigation: timebox week 1 to "skeleton compiles, even if `awidat init` is a stub." If week 2 slips, push everything one week and cut Week 7's hooks (move to v1.1). The demo cannot slip — the demo is the noise (`conversation.txt:235-245`).

User's stated Rust experience level: not stated. **Assume intermediate, productive after 2 weeks of ramp.** If it's lower, the realistic timeline is 10 weeks, not 8.

---

## 16. Open questions parking lot

Each entry: name + recommended default + the test that resolves it.

### 16.1 Verification of taste

- **Question:** How do we verify a cut "feels right" beyond human review?
- **Default:** v1 = human review only. Tier 2 catches mechanical failures; humans catch taste failures.
- **Test:** instrument every approved/rejected cut with `(audio energy at cut, scene-change distance to cut, transcript-snippet around cut)` features; after 200 sessions of usage, see if a small classifier predicts approve/reject reliably. If yes, build the taste-critic in v2.

### 16.2 Architect/Editor empirical test

- **Question:** Does Aider's Architect/Editor split help for video, where the Editor needs taste?
- **Default:** v1 ships with the split (Sonnet Architect, Haiku Editor for `apply_edl`).
- **Test:** five hours of real podcast footage; produce a 90s highlight clip with split vs. single-model. Three blind reviewers rank. If split materially worse, kill it. If statistically tied, keep it (the cost savings on Editor calls justify).

### 16.3 Sub-agent granularity

- **Question:** Topic-segment vs. scene vs. clip — what's the right unit for read-only sub-agents?
- **Default:** topic-segment (5–15 min chunks).
- **Test:** measure summary quality + dispatch overhead at three granularities. Pick the one with best summary quality per second of dispatch.

### 16.4 Skills format for editorial style

- **Question:** Can a skill teach "edit like Joe Rogan's editor" vs. "edit like Lex Fridman's editor"? Per `synthesis.md` §"Open questions" #5.
- **Default:** v1 ships generic skills. Style is in user prompts.
- **Test:** v1.5 — collect 10 hours of an editor's edits; produce a style-skill that captures their pacing/cut-density patterns; reviewers rank against the editor's actual output.

### 16.5 Render economics at scale

- **Question:** How do we handle render economics when we move to hosted? Local renders are free; cloud renders are not.
- **Default:** v1 is local-only. The closed cloud product solves it later.
- **Test:** v2 hosted prototype — measure cost per minute of final render at scale; price accordingly.

### 16.6 Planner/worker coordination protocol in markdown (Cursor multi-agent kernels)

- **Question:** Should the planner/worker coordination protocol live in a markdown file the way Cursor's kernels do (per `_docs/cursor/2026-04-multi-agent-kernels.md`)?
- **Default:** likely yes — it's `episode-notes.md` doing double duty, plus `edit-plan.json` as the structured handoff. But we're not running parallel write-workers in v1, so the question is dormant.
- **Test:** if v2 introduces parallel read sub-agents and we see coordination failures, formalize the kernel-in-markdown pattern. Until then, the existing JSON+Markdown pair suffices.

### 16.7 Index depth roadmap

The v1 index (transcript + diarization + shot boundaries + audio energy + topic segmentation) is **table stakes** — Descript-equivalent at best. The actual moat is what we add over the next year. The agent's editorial judgment ("does this moment need b-roll", "is this the laugh to cut on") is bounded by what the index can see. Investing here is investing in the agent's IQ (per `synthesis.md` §"Context management").

Schema in §3 / §5 must be designed so deeper indexers slot in cleanly without breaking sidecar layout or `find_moment` semantics.

**v1.5 candidates** (deeper signals, harder to source, higher leverage):

- **Speaker emotion / fine-grained energy curves** — not just "silence vs. speech" but "speaker's energy spiked at 4:12, dropped at 4:24". Probable approach: voice prosody analysis on top of diarization output.
- **Conversational structure** — who's leading at each moment, who's reacting, where laughs/agreements/disagreements land. Probable approach: LLM pass over diarized transcript with a structured-output schema.
- **Visual moment detection** — gestures, smiles, "speaker looked away from camera" (telegraphs cuts). Probable approach: vision model frame-sample at shot boundaries + 1-per-second.
- **Cross-modal alignment** — when does what someone *says* line up with what they *do*? Probable approach: combined transcript + visual-event stream, joined on timestamp.

**Test for promotion to v1.5:** for any candidate indexer, run the v1 demo with and without it. If the agent's editorial decisions measurably improve (per the "≥3 non-mechanical decisions" bar in §14, or via blind reviewer ranking), promote.

**Design rule for v1:** the indexer pipeline (`crates/mcp/` extension config + `index/` sidecar layout) must accept new indexers as additional MCP servers without engine changes. Treat this as a hard requirement on Week 2's design — if the v1.5 indexers would need a refactor to slot in, the Week 2 design is wrong.

---

## Appendix A — citation crib sheet

Where each major decision came from:

| Decision | Source |
| --- | --- |
| Two-loop turn-based agent | `_notes/codex.md` §1 / `codex-rs/core/src/session/turn.rs:137-665` |
| `FunctionCallError` 3-way enum | `_notes/codex.md` §1 / `codex-rs/core/src/function_tool.rs:1-11` |
| `apply_edl` as `apply_patch` analog | `_notes/codex.md` §6.2 / `codex-rs/core/src/handlers/apply_patch.rs:56-121` |
| Streaming argument diff consumer | `_notes/codex.md` §2 (apply_patch) / `apply_patch.rs:52` |
| Tool output middle-truncation, 1MiB cap | `_notes/codex.md` §3 / `exec.rs:51-72` |
| Compaction at two points (pre + mid-turn) | `_notes/codex.md` §1 / `compact.rs` |
| Sandbox: macOS seatbelt + Linux landlock | `_notes/codex.md` §4 / `sandboxing/src/seatbelt.rs` |
| Try-sandboxed-then-escalate | `_notes/codex.md` §4 / `orchestrator.rs:126-380` |
| Eight-crate Rust workspace | `_notes/goose.md` §1 |
| MCP for all language-agnostic tools | `_notes/goose.md` §2, §6c |
| Hybrid Rust harness + Python ML tools | `_notes/goose.md` §6c |
| Engine + thin client over HTTP+WS (v2) | `_notes/opencode.md` §1 |
| Durable session SQLite | `_notes/opencode.md` §1.2 / Goose `session_manager.rs` |
| TUI three-pane layout, streaming spinners | `_notes/crush.md` §1, §2 |
| Approval modal Allow/Session/Deny | `_notes/crush.md` §4 |
| Architect/Editor split | `_notes/aider.md` §2; `synthesis.md` §"Architecture patterns" |
| Repomap → footage index analog | `synthesis.md` §"Context management" / `_notes/aider.md` §3 |
| Windowed file viewer → `view_timeline` | `_notes/swe-agent.md` §2.1 |
| Linter on edit → `apply_edl` validation pipeline | `_notes/swe-agent.md` §2.2 |
| `find_file` paths-only → `find_moment` shape | `_notes/swe-agent.md` §2.3 |
| Single-threaded writes (no parallel write sub-agents) | `_docs/general/cognition-dont-build-multi-agents.md` |
| Read-only sub-agents per topic-segment | `_notes/anthropic-multi-agent-research-system.md` (via synthesis) |
| `edit-plan.json` (JSON state) + `episode-notes.md` (Markdown narrative) | `synthesis.md` §"Long-horizon work" / `_notes/anthropic-long-running-harnesses.md` |
| Skills format with progressive disclosure | `_docs/claude-code/agent-skills.md`; `synthesis.md` §"Skills" |
| Skills + slash commands unified | `_docs/claude-code/skills-and-slash-commands.md` |
| OTIO superset for project format | `synthesis.md` §"Project format"; `conversation.txt:64-67` |
| Diffability is the unfair advantage | `synthesis.md` §"Project format" |
| Open core: substrate open, orchestrator/GUI closed | `conversation.txt:209-227` |
| Substrate + flagship consumer ship together | `conversation.txt:303-313` |
| Demo as the noise | `conversation.txt:235-245` |
| Non-goals (no feature film, no shorts, no enterprise) | `conversation.txt:74` + competitive analysis |
| Pure web GUI (not Tauri) | this plan §12.2 |

End of plan.
