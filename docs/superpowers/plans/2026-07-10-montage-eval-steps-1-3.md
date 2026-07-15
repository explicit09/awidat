# Montage Eval Steps 1-3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the deterministic montage-eval harness contracts and tier-1/tier-2 validators while preserving the picture and sound gates already implemented on `eval-gates`.

**Architecture:** `montage-eval` remains a small Rust library and CLI. Scenario parsing, immutable run artifacts, scorecards, and validator reports are separate modules; the CLI composes them and never lets a worker decide pass/fail. Existing editorial `gates`, `profile`, and `sound` modules remain production-facing inputs rather than being replaced.

**Tech Stack:** Rust, serde/serde_json/serde_yaml, Montage OTIO and render-manifest types, ffprobe/ffmpeg subprocesses, cargo integration tests.

## Global Constraints

- The deterministic Rust driver owns queueing, pass/fail, improvement, best-version retention, checkpoint/resume, and anti-gaming enforcement.
- Tiers 1 and 2 contain no LLM calls.
- Existing audio-energy, frame-quality, and composition sidecars are read rather than recomputed.
- Existing `montage validate` and render-manifest formats are reused.
- CI entrypoints remain `--ci --product --golden --json`, `--stress --json`, and `--live --json --fail-on-skip`.
- Builds use `CARGO_TARGET_DIR` on the external drive when it is mounted; otherwise use the existing local target without deleting it.
- Do not implement tier 3, workers, SQLite, tier 4, exemplar ingestion, or campaign progression in this plan.

---

### Task 1: Scenario, run-folder, scorecard, and CLI contracts

**Files:**
- Create: `crates/eval/src/scenario.rs`
- Create: `crates/eval/src/run_artifacts.rs`
- Create: `crates/eval/src/scorecard.rs`
- Create: `crates/eval/tests/scenario.rs`
- Create: `crates/eval/tests/run_artifacts.rs`
- Create: `crates/eval/tests/scorecard.rs`
- Modify: `crates/eval/src/lib.rs`
- Modify: `crates/eval/src/main.rs`
- Modify: `crates/eval/Cargo.toml`

**Interfaces:**
- Produces: `Scenario::from_yaml_file(path) -> Result<Scenario, ScenarioError>` with typed `hard_gates`, `soft_gates`, `repair`, and `guards`.
- Produces: `RunArtifacts::create(root, run_id, scenario) -> Result<RunArtifacts, RunArtifactError>` and `attempt(number) -> AttemptArtifacts`.
- Produces: `Scorecard::write(path) -> Result<(), ScorecardError>` with typed tiers, defects, stop reason, and next action.
- Consumes: existing CLI flags and `suite::run_golden`.

- [ ] **Step 1: Write failing scenario loader tests**

```rust
#[test]
fn loads_the_spec_scenario_contract() {
    let scenario = Scenario::from_yaml_file(fixture("dead_air_basic.yaml"))
        .unwrap_or_else(|error| panic!("scenario should load: {error}"));
    assert_eq!(scenario.id, "podcast_dead_air_basic_001");
    assert_eq!(scenario.repair.safety_ceiling, 10);
    assert!(!scenario.guards.allow_scenario_edits);
}

#[test]
fn rejects_a_zero_safety_ceiling() {
    let error = Scenario::from_yaml_str(VALID.replace("safety_ceiling: 10", "safety_ceiling: 0"))
        .expect_err("zero attempts must fail closed");
    assert!(error.to_string().contains("safety_ceiling"));
}
```

- [ ] **Step 2: Run the scenario test and confirm RED**

Run: `cargo test -p montage-eval --test scenario`

Expected: compile failure because `montage_eval::Scenario` does not exist.

- [ ] **Step 3: Implement the minimal typed YAML loader**

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub id: String,
    pub category: String,
    pub tool: String,
    pub source: PathBuf,
    pub task: String,
    pub hard_gates: HardGates,
    pub soft_gates: SoftGates,
    pub repair: RepairPolicy,
    pub guards: Guards,
}

impl Scenario {
    pub fn from_yaml_str(input: impl AsRef<str>) -> Result<Self, ScenarioError> {
        let scenario: Self = serde_yaml::from_str(input.as_ref())?;
        scenario.validate()?;
        Ok(scenario)
    }
}
```

- [ ] **Step 4: Run the scenario tests and confirm GREEN**

Run: `cargo test -p montage-eval --test scenario`

Expected: all scenario tests pass.

- [ ] **Step 5: Write failing run-folder tests**

```rust
#[test]
fn creates_the_spec_run_layout_without_overwriting_attempts() {
    let run = RunArtifacts::create(temp.path(), "run-001", &scenario())
        .unwrap_or_else(|error| panic!("run should be created: {error}"));
    assert!(run.task_path().is_file());
    assert!(run.input_manifest_path().is_file());
    let attempt = run.create_attempt(1).unwrap_or_else(|error| panic!("attempt: {error}"));
    assert!(attempt.root().ends_with("attempt_1"));
    assert!(run.create_attempt(1).is_err());
}
```

- [ ] **Step 6: Run the run-folder test and confirm RED**

Run: `cargo test -p montage-eval --test run_artifacts`

Expected: compile failure because `RunArtifacts` does not exist.

- [ ] **Step 7: Implement immutable run artifacts**

```rust
pub fn create_attempt(&self, number: u32) -> Result<AttemptArtifacts, RunArtifactError> {
    let root = self.root.join(format!("attempt_{number}"));
    std::fs::create_dir(&root).map_err(|source| RunArtifactError::Create { path: root.clone(), source })?;
    std::fs::create_dir(root.join("evidence"))?;
    Ok(AttemptArtifacts { root })
}
```

Use `create_dir`, not `create_dir_all`, for an attempt so an existing attempt fails closed instead of being overwritten.

- [ ] **Step 8: Run run-folder tests and confirm GREEN**

Run: `cargo test -p montage-eval --test run_artifacts`

Expected: all run artifact tests pass.

- [ ] **Step 9: Write failing scorecard round-trip tests**

```rust
#[test]
fn writes_the_machine_readable_scorecard_contract() {
    let card = failing_scorecard();
    card.write(&path).unwrap_or_else(|error| panic!("scorecard: {error}"));
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(value["tiers"]["mechanical"], "pass");
    assert_eq!(value["blocking_failures"][0]["code"], "SILENCE_TOO_LONG");
    assert_eq!(value["next_action"], "repair");
}
```

- [ ] **Step 10: Run the scorecard test and confirm RED**

Run: `cargo test -p montage-eval --test scorecard`

Expected: compile failure because `Scorecard` does not exist.

- [ ] **Step 11: Implement scorecard serialization and atomic write**

```rust
pub fn write(&self, path: impl AsRef<Path>) -> Result<(), ScorecardError> {
    let path = path.as_ref();
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}
```

- [ ] **Step 12: Run scorecard tests and confirm GREEN**

Run: `cargo test -p montage-eval --test scorecard`

Expected: all scorecard tests pass.

- [ ] **Step 13: Make CLI lane reporting explicit and fail closed for required skips**

Add a typed lane result (`passed`, `failed`, or `skipped` with reason). Preserve golden execution. `--fail-on-skip` returns failure when any requested lane is skipped; `--product`, `--stress`, and `--live` must not be silently represented as successful work.

- [ ] **Step 14: Verify Task 1**

Run: `cargo test -p montage-eval`

Expected: all montage-eval tests pass.

Run: `cargo run -p montage-eval -- --live --json --fail-on-skip`

Expected: non-zero exit with JSON identifying `live` as skipped until a live runner is implemented.

---

### Task 2: Tier-1 mechanical validator

**Files:**
- Create: `crates/eval/src/mechanical.rs`
- Create: `crates/eval/tests/mechanical.rs`
- Create: `crates/eval/fixtures/mechanical/ffprobe-valid.json`
- Create: `crates/eval/fixtures/mechanical/ffprobe-missing-audio.json`
- Modify: `crates/eval/src/lib.rs`
- Modify: `crates/eval/Cargo.toml`

**Interfaces:**
- Consumes: `Scenario`, `AttemptArtifacts`, `montage_render::RenderExecutionManifest`, and Montage OTIO parsing.
- Produces: `MechanicalReport { passed: bool, checks: Vec<CheckResult> }`.
- Produces: pure parsers for ffprobe JSON and render manifests plus a command runner boundary for `ffprobe` and `montage validate`.

- [ ] **Step 1: Write failing ffprobe contract tests**

```rust
#[test]
fn requires_playable_audio_and_video_streams() {
    let valid = inspect_ffprobe(include_str!("../fixtures/mechanical/ffprobe-valid.json"), &gates())
        .unwrap_or_else(|error| panic!("valid fixture: {error}"));
    assert!(valid.passed);
    let missing = inspect_ffprobe(include_str!("../fixtures/mechanical/ffprobe-missing-audio.json"), &gates())
        .unwrap_or_else(|error| panic!("valid fixture: {error}"));
    assert!(!missing.passed);
    assert!(missing.checks.iter().any(|check| check.code == "AUDIO_STREAM_MISSING"));
}
```

- [ ] **Step 2: Confirm RED**

Run: `cargo test -p montage-eval --test mechanical requires_playable_audio_and_video_streams`

Expected: compile failure because `inspect_ffprobe` does not exist.

- [ ] **Step 3: Implement strict ffprobe JSON parsing**

Parse `format.duration`, video width/height/fps/codec, and audio/video stream presence. Reject missing, non-finite, or non-positive measurements; compare aspect ratio to the scenario contract with a documented one-pixel tolerance.

- [ ] **Step 4: Confirm GREEN**

Run: `cargo test -p montage-eval --test mechanical requires_playable_audio_and_video_streams`

Expected: test passes.

- [ ] **Step 5: Write failing OTIO and manifest tests**

```rust
#[test]
fn rejects_manifest_with_missing_declared_output() {
    let report = inspect_manifest(&manifest_path, &output_path)
        .unwrap_or_else(|error| panic!("manifest should parse: {error}"));
    assert!(!report.passed);
    assert!(report.checks.iter().any(|check| check.code == "MANIFEST_OUTPUT_MISSING"));
}
```

- [ ] **Step 6: Confirm RED**

Run: `cargo test -p montage-eval --test mechanical rejects_manifest_with_missing_declared_output`

Expected: compile failure because `inspect_manifest` does not exist.

- [ ] **Step 7: Implement OTIO, manifest, and command checks**

Use Montage's OTIO reader for structural parse, `montage_render::read_render_manifest` for schema validation, and explicit subprocess status/stdout/stderr capture for `ffprobe` and `montage validate`. A missing executable is a blocking defect, not a skip, for a requested tier-1 run.

- [ ] **Step 8: Verify Task 2**

Run: `cargo test -p montage-eval --test mechanical`

Expected: all mechanical validator tests pass without requiring media or external binaries.

---

### Task 3: Tier-2 measurable validator

**Files:**
- Create: `crates/eval/src/measurable.rs`
- Create: `crates/eval/tests/measurable.rs`
- Create: `crates/eval/fixtures/measurable/audio-energy.json`
- Create: `crates/eval/fixtures/measurable/frame-quality.json`
- Create: `crates/eval/fixtures/measurable/composition.json`
- Modify: `crates/eval/src/lib.rs`

**Interfaces:**
- Consumes: scenario hard gates, rendered output path, audio-energy sidecar, frame-quality sidecar, and composition sidecar.
- Produces: `MeasurableReport { passed: bool, checks: Vec<CheckResult> }`.
- Produces: parsers for ffmpeg `silencedetect`, `blackdetect`, and `freezedetect` stderr.

- [ ] **Step 1: Write failing sidecar parser tests**

```rust
#[test]
fn reads_existing_sidecars_without_rederiving_signals() {
    let evidence = SidecarEvidence::load(&paths())
        .unwrap_or_else(|error| panic!("sidecars should load: {error}"));
    assert_eq!(evidence.audio.integrated_lufs, -14.2);
    assert_eq!(evidence.audio.true_peak_dbfs, -1.4);
    assert!(evidence.composition.verification.passed);
    assert!(evidence.frame_quality.thumbnail_candidates[0].thumbnail_score > 0.8);
}
```

- [ ] **Step 2: Confirm RED**

Run: `cargo test -p montage-eval --test measurable reads_existing_sidecars_without_rederiving_signals`

Expected: compile failure because `SidecarEvidence` does not exist.

- [ ] **Step 3: Implement strict sidecar deserialization**

Model only the fields needed by gates. Preserve unknown fields for forward compatibility, but fail on missing required measurements when the scenario enables the corresponding gate.

- [ ] **Step 4: Confirm GREEN**

Run: `cargo test -p montage-eval --test measurable reads_existing_sidecars_without_rederiving_signals`

Expected: test passes.

- [ ] **Step 5: Write failing ffmpeg detector parser tests**

```rust
#[test]
fn parses_silence_black_and_freeze_windows() {
    assert_eq!(parse_silencedetect(SILENCE_LOG), vec![Span::new(12.0, 14.4)]);
    assert_eq!(parse_blackdetect(BLACK_LOG), vec![Span::new(30.0, 31.2)]);
    assert_eq!(parse_freezedetect(FREEZE_LOG), vec![Span::new(42.0, 45.0)]);
}
```

- [ ] **Step 6: Confirm RED**

Run: `cargo test -p montage-eval --test measurable parses_silence_black_and_freeze_windows`

Expected: compile failure because detector parsers do not exist.

- [ ] **Step 7: Implement detector parsing and threshold evaluation**

Pair start/end records deterministically, reject unmatched/non-finite spans, and compare the longest span to the scenario threshold. Invoke ffmpeg once with the configured filters when evaluating real media; persist raw stderr and parsed JSON in the attempt folder.

- [ ] **Step 8: Verify Task 3**

Run: `cargo test -p montage-eval --test measurable`

Expected: all measurable tests pass without requiring media or external binaries.

---

### Task 4: Integrated verification checkpoint

**Files:**
- Modify only files required by compiler or lint feedback from Tasks 1-3.

**Interfaces:**
- Consumes: Tasks 1-3.
- Produces: a CI-safe deterministic foundation for later evidence packets and workers.

- [ ] **Step 1: Run the crate gate**

Run: `cargo test -p montage-eval`

Expected: all tests pass.

- [ ] **Step 2: Run formatting and diff hygiene**

Run: `cargo fmt --all -- --check`

Expected: success.

Run: `git diff --check`

Expected: success.

- [ ] **Step 3: Run clippy for the touched crate**

Run: `cargo clippy -p montage-eval --all-targets -- -D warnings`

Expected: success.

- [ ] **Step 4: Exercise the existing CI command**

Run: `cargo run -p montage-eval -- --ci --product --golden --json`

Expected: JSON reports golden results and truthfully identifies any unimplemented requested lane; command success follows the documented CI lane policy rather than silently treating skips as passes.

## Checkpoint before later build-order work

Do not plan or implement tier 4 until the user decides whether its judge must pass a small human-labeled golden calibration set before influencing repairs. Before seeding campaigns beyond step 6, also confirm whether Stage 0 indexer reliability remains ahead of podcast/auto-cutter.
