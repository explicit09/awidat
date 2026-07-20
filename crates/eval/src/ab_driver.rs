//! A/B comparison driver: run the same scenario under two `codex-exec`
//! prompt configurations and score both with the existing deterministic
//! tiers, so a prompt-merge decision (see
//! `docs/decision-prompt-layer-merge-2026-07-19.md` §6/§8) is data-driven
//! instead of vibes-driven.
//!
//! Scope: this is comparison infrastructure only. It runs one edit-worker
//! pass per (scenario, config) and scores the result — it is explicitly
//! **not** the full autonomous edit→verify→fix loop (that stays deferred).
//!
//! # What this module does not (and cannot) prove
//!
//! The [`CodexExecRunner`] trait exists so the whole pipeline — command
//! construction, JSONL telemetry parsing, gate scoring, paired comparison —
//! can be exercised against a [`FakeCodexExecRunner`] with zero model calls,
//! zero auth, and zero cost. That is sufficient to prove the *plumbing* is
//! correct. It is **not** sufficient to prove anything about actual model
//! behavior under Config A vs Config B — a real run additionally needs:
//!
//! - A live `codex-exec` binary, valid ChatGPT/API auth, and network access.
//! - The real model actually invoked (token counts, tool-call choices, and
//!   turn counts from a fake are fixture data, not model output).
//! - Real source media for the scenario's `source` field — mechanical/tier-1
//!   checks here run against whatever OTIO or ffprobe evidence the attempt
//!   actually produces, which for a real run means a real render.
//! - Real indexer sidecars (audio-energy-mcp, frame-quality-mcp,
//!   composition-mcp) for the tier-2 measurable tier — see
//!   [`ScoredAttempt::measurable`] doc comment for the precise gap.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    HouseProfile, MeasurableReport, MechanicalReport, PacingStats, Scenario, gates, inspect_otio,
    load_cut_times,
};

/// One named prompt configuration: which `codex-exec` instruction-override
/// flags to pass, if any. `None` for a field means "omit the flag", which
/// is byte-for-byte stock `codex-exec` behavior for that instruction tier
/// (see the fork edit documented in `vendor/codex-rs/SOURCE`, patch #10).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbConfig {
    /// Human-readable config name, e.g. `"A-stock"` / `"B-merged"`.
    pub name: String,
    /// File passed as `--base-instructions-file`, or `None` to omit the
    /// flag (stock `models-manager/prompt.md`).
    pub base_instructions_file: Option<PathBuf>,
    /// File passed as `--developer-instructions-file`, or `None` to omit
    /// the flag.
    pub developer_instructions_file: Option<PathBuf>,
    /// Extra raw `-c key=value` config overrides applied to every
    /// invocation of this config (model and sandbox mode are already
    /// covered by [`build_invocation`] — this is for config-specific
    /// extras only).
    #[serde(default)]
    pub extra_config_overrides: Vec<String>,
}

/// A named pair of configs plus the model/sandbox settings shared by both
/// arms of the comparison. Deserializes from the manifest format documented
/// in `crates/eval/fixtures/ab/manifest.example.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbManifest {
    /// Model slug passed to every invocation via `--model`.
    pub model: String,
    /// Number of trials per (scenario, config) cell. `1` is a smoke run;
    /// `3+` is needed for any claim about variance.
    #[serde(default = "default_trials")]
    pub trials: u32,
    /// Config A — normally the stock/control arm (leave instruction files
    /// unset to run byte-identical to today's `codex-exec` default).
    pub config_a: AbConfig,
    /// Config B — the experimental arm under test.
    pub config_b: AbConfig,
}

fn default_trials() -> u32 {
    1
}

/// Errors loading or validating an A/B manifest.
#[derive(Debug, Error)]
pub enum AbManifestError {
    /// Reading the manifest file failed.
    #[error("reading A/B manifest {}: {source}", path.display())]
    Read {
        /// Manifest path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The manifest was not valid TOML.
    #[error("invalid A/B manifest TOML: {0}")]
    Toml(#[from] toml::de::Error),
    /// A parsed manifest violates a driver invariant.
    #[error("invalid A/B manifest: {0}")]
    Contract(String),
}

impl AbManifest {
    /// Load and validate an A/B manifest from a TOML file.
    pub fn from_toml_file(path: impl AsRef<Path>) -> Result<Self, AbManifestError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| AbManifestError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let manifest: Self = toml::from_str(&text)?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), AbManifestError> {
        if self.model.trim().is_empty() {
            return Err(AbManifestError::Contract("model must not be empty".into()));
        }
        if self.trials == 0 {
            return Err(AbManifestError::Contract(
                "trials must be greater than zero".into(),
            ));
        }
        for config in [&self.config_a, &self.config_b] {
            if config.name.trim().is_empty() {
                return Err(AbManifestError::Contract(
                    "config name must not be empty".into(),
                ));
            }
        }
        if self.config_a.name == self.config_b.name {
            return Err(AbManifestError::Contract(
                "config_a and config_b must have distinct names".into(),
            ));
        }
        Ok(())
    }
}

/// One fully-constructed invocation of the `codex-exec` binary, before
/// spawning. Kept as data (not a running process) so command construction
/// is independently testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexExecInvocation {
    /// Path to the `codex-exec` binary (or a test double in unit tests).
    pub binary: PathBuf,
    /// Full argv, excluding argv\[0\].
    pub args: Vec<String>,
    /// Working directory the process should run in (the scenario's project
    /// root / source path).
    pub cwd: PathBuf,
}

/// Build the `codex-exec` argv for one (scenario, config) pass.
///
/// Mirrors the headless invocation shape `crates/agent-cli` uses (model,
/// sandbox/approval flags suitable for unattended runs, ephemeral session)
/// plus `--json` for JSONL telemetry and, for a non-stock config, the
/// `--base-instructions-file` / `--developer-instructions-file` seam added
/// in `vendor/codex-rs/exec/src/cli.rs` (fork patch #10, see
/// `vendor/codex-rs/SOURCE`).
pub fn build_invocation(
    codex_exec_binary: impl Into<PathBuf>,
    manifest: &AbManifest,
    config: &AbConfig,
    scenario: &Scenario,
) -> CodexExecInvocation {
    let mut args = vec![
        "--json".to_string(),
        "--skip-git-repo-check".to_string(),
        "--ephemeral".to_string(),
        "--sandbox".to_string(),
        "workspace-write".to_string(),
        "--model".to_string(),
        manifest.model.clone(),
    ];

    if let Some(path) = &config.base_instructions_file {
        args.push("--base-instructions-file".to_string());
        args.push(path.display().to_string());
    }
    if let Some(path) = &config.developer_instructions_file {
        args.push("--developer-instructions-file".to_string());
        args.push(path.display().to_string());
    }
    for kv in &config.extra_config_overrides {
        args.push("-c".to_string());
        args.push(kv.clone());
    }

    // Positional prompt argument last, matching `codex exec [OPTIONS] [PROMPT]`.
    args.push(scenario.task.clone());

    CodexExecInvocation {
        binary: codex_exec_binary.into(),
        args,
        cwd: scenario.source.clone(),
    }
}

/// Outcome of one `codex-exec` invocation: exit status plus the captured
/// `--json` JSONL stream. Produced identically by the real subprocess
/// runner and [`FakeCodexExecRunner`], so downstream parsing/scoring code
/// cannot tell them apart.
#[derive(Debug, Clone)]
pub struct CodexExecOutput {
    /// True when the process exited successfully.
    pub success: bool,
    /// Raw stdout, expected to be JSONL when `--json` was passed.
    pub stdout: String,
    /// Raw stderr, for diagnostics only.
    pub stderr: String,
}

/// Something that can run one `codex-exec` invocation and return its
/// output. The production impl ([`SubprocessCodexExecRunner`]) spawns a
/// real process; [`FakeCodexExecRunner`] replays a fixture so the rest of
/// the pipeline is unit-testable without auth, network, or cost.
pub trait CodexExecRunner {
    /// Execute one invocation and capture its output.
    fn run(&self, invocation: &CodexExecInvocation) -> Result<CodexExecOutput, RunnerError>;
}

/// Failure spawning or waiting on a `codex-exec` process.
#[derive(Debug, Error)]
pub enum RunnerError {
    /// The process could not be spawned (binary missing, permissions, …).
    #[error("spawning {}: {source}", binary.display())]
    Spawn {
        /// Binary path attempted.
        binary: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Spawns the real `codex-exec` binary as a subprocess. **Never used by
/// this crate's own tests** — those exercise [`FakeCodexExecRunner`]
/// instead, per the constraint that building/testing montage-eval must not
/// invoke a live model.
#[derive(Debug, Default)]
pub struct SubprocessCodexExecRunner;

impl CodexExecRunner for SubprocessCodexExecRunner {
    fn run(&self, invocation: &CodexExecInvocation) -> Result<CodexExecOutput, RunnerError> {
        let output = Command::new(&invocation.binary)
            .args(&invocation.args)
            .current_dir(&invocation.cwd)
            .output()
            .map_err(|source| RunnerError::Spawn {
                binary: invocation.binary.clone(),
                source,
            })?;
        Ok(CodexExecOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// A canned [`CodexExecRunner`] for tests: returns a fixed
/// [`CodexExecOutput`] regardless of the invocation, or one selected by
/// inspecting the invocation's `--base-instructions-file` argument (the
/// one flag [`build_invocation`] sets that reliably distinguishes Config A
/// from Config B). Lets unit tests drive the full pipeline — command
/// construction through paired comparison — against fixture JSONL and a
/// fixture OTIO output, without spawning anything.
#[derive(Debug, Default)]
pub struct FakeCodexExecRunner {
    /// Output selected when the invocation's `--base-instructions-file`
    /// argument equals this path (compared as a string, so tests can use
    /// any placeholder path without the file needing to exist).
    pub by_base_instructions_file: std::collections::BTreeMap<String, CodexExecOutput>,
    /// Output returned when no `--base-instructions-file` flag is present
    /// (Config A / stock) or no keyed entry matches.
    pub default_output: Option<CodexExecOutput>,
}

impl FakeCodexExecRunner {
    /// A fake that always returns the same output regardless of config.
    pub fn constant(output: CodexExecOutput) -> Self {
        Self {
            by_base_instructions_file: std::collections::BTreeMap::new(),
            default_output: Some(output),
        }
    }
}

impl CodexExecRunner for FakeCodexExecRunner {
    fn run(&self, invocation: &CodexExecInvocation) -> Result<CodexExecOutput, RunnerError> {
        let base_instructions_arg = invocation
            .args
            .iter()
            .position(|arg| arg == "--base-instructions-file")
            .and_then(|idx| invocation.args.get(idx + 1));

        let selected = base_instructions_arg
            .and_then(|path| self.by_base_instructions_file.get(path))
            .cloned()
            .or_else(|| self.default_output.clone());

        selected.ok_or_else(|| RunnerError::Spawn {
            binary: invocation.binary.clone(),
            source: std::io::Error::other("FakeCodexExecRunner has no output configured"),
        })
    }
}

// ---------------------------------------------------------------------
// JSONL telemetry
// ---------------------------------------------------------------------

/// Minimal local mirror of `codex-exec`'s `--json` JSONL event shapes
/// (`vendor/codex-rs/exec/src/exec_events.rs`). Deliberately not a
/// dependency on the `codex-exec` crate itself: pulling in `codex-core`'s
/// full build graph just for these wire types would couple montage-eval's
/// build time to the entire vendored agent, for a handful of fields we
/// read once per line. Field names and `#[serde(tag = "type", ...)]`
/// shapes are kept in lockstep with the upstream enum by hand; a drift
/// here would show up as telemetry parsing silently zeroing out, which
/// [`parse_telemetry`]'s unit tests guard against using real-shaped
/// fixture JSONL.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawThreadEvent {
    #[serde(rename = "turn.completed")]
    TurnCompleted { usage: RawUsage },
    #[serde(rename = "turn.failed")]
    TurnFailed {
        #[serde(default)]
        error: Option<RawThreadError>,
    },
    #[serde(rename = "item.completed")]
    ItemCompleted { item: RawThreadItem },
    /// Every other event type (`thread.started`, `turn.started`,
    /// `item.started`, `item.updated`, …) is telemetry-irrelevant here.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    cached_input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    reasoning_output_tokens: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct RawThreadError {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RawThreadItem {
    #[serde(rename = "type")]
    item_type: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    error: Option<RawToolCallError>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawToolCallError {
    #[serde(default)]
    message: String,
}

/// Telemetry rolled up from one `codex-exec --json` JSONL stream.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Telemetry {
    /// Sum of `input_tokens + cached_input_tokens + output_tokens +
    /// reasoning_output_tokens` across every `turn.completed` event. Usage
    /// is cumulative-per-turn in the wire format, so this is the total
    /// spend across the whole run.
    pub total_tokens: i64,
    /// Number of turns that completed.
    pub turns_completed: u32,
    /// Number of turns that failed (`turn.failed` events).
    pub turns_failed: u32,
    /// Number of completed tool-call items (`mcp_tool_call` /
    /// `collab_tool_call` items with `type` present).
    pub tool_calls: u32,
    /// Number of completed `command_execution` items (built-in shell).
    /// High shell counts on an editing scenario are a doctrine-violation
    /// signal: the prompt bans shell for producing the edited artifact,
    /// so an arm that edits via shell instead of MCP tools shows up here
    /// even when `tool_calls` is zero.
    pub shell_calls: u32,
    /// Number of completed `file_change` items (`apply_patch` edits).
    /// Direct file edits to `project.otio.json` are banned by the same
    /// doctrine; a nonzero count on an editing scenario deserves a look
    /// at the raw stream.
    pub file_changes: u32,
    /// Number of completed tool-call items whose status was `failed` or
    /// which carried a structured `error` — the harness-contract
    /// regression signal called out in the decision doc.
    pub tool_call_errors: u32,
    /// Raw error messages collected from `turn.failed` and failed tool
    /// calls, for debugging a regression.
    pub error_messages: Vec<String>,
}

/// Error parsing a `--json` JSONL stream.
#[derive(Debug, Error)]
pub enum TelemetryError {
    /// A line was not valid JSON, or did not match the expected event
    /// shape. Non-fatal lines (blank lines) are skipped, not errors.
    #[error("line {line}: {source}")]
    Line {
        /// 1-indexed line number.
        line: usize,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
}

/// Parse a `codex-exec --json` JSONL stream into rolled-up [`Telemetry`].
///
/// Unknown/unhandled event types are ignored rather than rejected — the
/// JSONL contract emits several event kinds (`thread.started`,
/// `item.started`, `item.updated`, …) this driver has no use for, and a
/// forward-compatible parser should not fail closed on new event kinds
/// codex-exec adds later.
pub fn parse_telemetry(jsonl: &str) -> Result<Telemetry, TelemetryError> {
    let mut telemetry = Telemetry::default();
    for (idx, line) in jsonl.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let event: RawThreadEvent =
            serde_json::from_str(line).map_err(|source| TelemetryError::Line {
                line: idx + 1,
                source,
            })?;
        match event {
            RawThreadEvent::TurnCompleted { usage } => {
                telemetry.turns_completed += 1;
                telemetry.total_tokens += usage.input_tokens
                    + usage.cached_input_tokens
                    + usage.output_tokens
                    + usage.reasoning_output_tokens;
            }
            RawThreadEvent::TurnFailed { error } => {
                telemetry.turns_failed += 1;
                if let Some(error) = error {
                    telemetry.error_messages.push(error.message);
                }
            }
            RawThreadEvent::ItemCompleted { item } => {
                match item.item_type.as_str() {
                    "command_execution" => telemetry.shell_calls += 1,
                    "file_change" => telemetry.file_changes += 1,
                    _ => {}
                }
                let is_tool_call = matches!(
                    item.item_type.as_str(),
                    "mcp_tool_call" | "collab_tool_call"
                );
                if is_tool_call {
                    telemetry.tool_calls += 1;
                    let failed_status = item.status.as_deref() == Some("failed");
                    if failed_status || item.error.is_some() {
                        telemetry.tool_call_errors += 1;
                        if let Some(error) = item.error {
                            telemetry.error_messages.push(error.message);
                        }
                    }
                }
            }
            RawThreadEvent::Other => {}
        }
    }
    Ok(telemetry)
}

// ---------------------------------------------------------------------
// Scoring one attempt
// ---------------------------------------------------------------------

/// What one (scenario, config, trial) pass produced, resolved to concrete
/// paths this driver can score. A real run derives these from the
/// project the agent edited; a fixture-backed test constructs one
/// directly.
#[derive(Debug, Clone)]
pub struct AttemptOutput {
    /// Path to the resulting OTIO timeline (or `.cuts` sidecar) the tier-1
    /// mechanical check parses.
    pub otio_path: Option<PathBuf>,
    /// Cut-boundary times (seconds) for the pacing gates, when available.
    pub cut_times: Option<Vec<f64>>,
    /// Program duration in seconds, required alongside `cut_times` to
    /// build [`PacingStats`].
    pub duration_secs: Option<f64>,
}

impl AttemptOutput {
    /// Build an [`AttemptOutput`] by reading cut times back from a
    /// `.cuts` sidecar file via [`load_cut_times`], the same reader the
    /// golden suite uses. `otio_path` is recorded as-is (parsed lazily by
    /// [`score_attempt`]).
    pub fn from_cuts_sidecar(
        otio_path: Option<PathBuf>,
        cuts_sidecar: impl AsRef<Path>,
        duration_secs: f64,
    ) -> Result<Self, crate::CutsIoError> {
        let cut_times = load_cut_times(cuts_sidecar)?;
        Ok(Self {
            otio_path,
            cut_times: Some(cut_times),
            duration_secs: Some(duration_secs),
        })
    }
}

/// Deterministic score for one (scenario, config, trial) attempt: the
/// existing tier-1/pacing gates run over whatever the attempt produced,
/// plus the run's telemetry.
#[derive(Debug, Serialize)]
pub struct ScoredAttempt {
    /// Config name this attempt ran under.
    pub config_name: String,
    /// 1-based trial index within the (scenario, config) cell.
    pub trial: u32,
    /// Wall-clock duration of the `codex-exec` invocation.
    pub wall_time: Duration,
    /// Whether the `codex-exec` process itself exited successfully.
    pub process_success: bool,
    /// Telemetry parsed from the `--json` JSONL stream.
    pub telemetry: Telemetry,
    /// Tier-1 mechanical report, when an OTIO output was available to
    /// inspect.
    pub mechanical: Option<MechanicalReport>,
    /// Tier-2 measurable report. Always `None` in this driver today: it
    /// requires real indexer sidecar JSON (audio-energy-mcp,
    /// frame-quality-mcp, composition-mcp) that only exists after a real
    /// render + indexer pass runs against real media. A fake `codex-exec`
    /// cannot fabricate that evidence faithfully, so callers that want
    /// tier-2 coverage must supply [`crate::SidecarEvidence`] out of band
    /// and call [`crate::evaluate_measurable`] themselves; this field is
    /// kept as a documented gap rather than silently scored against fake
    /// data.
    pub measurable: Option<MeasurableReport>,
    /// Cold-open pacing gate, when cut times + a profile were available.
    pub cold_open_gate: Option<gates::GateReport>,
    /// Floor pacing gate, when cut times + a profile + archetype were
    /// available.
    pub floor_gate: Option<gates::GateReport>,
    /// Raw JSONL stream, carried for artifact persistence only — skipped
    /// during serialization so `attempt_summary.json` stays a summary.
    #[serde(skip)]
    pub raw_stdout: String,
    /// Raw stderr, same handling as `raw_stdout`.
    #[serde(skip)]
    pub raw_stderr: String,
}

impl ScoredAttempt {
    /// True when every gate that actually ran (mechanical + pacing) passed.
    /// Gates that were skipped (no evidence available) do not count
    /// against pass rate — a missing artifact is a driver/harness problem,
    /// surfaced separately via `process_success` and `telemetry`, not a
    /// silent gate failure.
    pub fn gates_passed(&self) -> bool {
        self.mechanical.as_ref().is_none_or(|r| r.passed)
            && self.cold_open_gate.as_ref().is_none_or(|g| g.passed)
            && self.floor_gate.as_ref().is_none_or(|g| g.passed)
    }
}

/// The identity + telemetry outcome of one `codex-exec` pass, bundled so
/// [`score_attempt`] stays under clippy's argument-count limit and callers
/// pass one cohesive value instead of a long positional list.
#[derive(Debug)]
pub struct RunOutcome {
    /// Config name this pass ran under.
    pub config_name: String,
    /// 1-based trial index within the (scenario, config) cell.
    pub trial: u32,
    /// Wall-clock duration of the invocation.
    pub wall_time: Duration,
    /// Whether the `codex-exec` process exited successfully.
    pub process_success: bool,
    /// Telemetry parsed from the run's `--json` JSONL stream.
    pub telemetry: Telemetry,
    /// The raw `--json` JSONL stream the telemetry was parsed from.
    /// Persisted verbatim by [`write_attempt_artifacts`] so a surprising
    /// rollup (e.g. zero tool calls on a mutated project) can be
    /// diagnosed from evidence instead of re-running a paid session.
    pub raw_stdout: String,
    /// Raw stderr from the invocation, for the same diagnostic purpose.
    pub raw_stderr: String,
}

/// Score one attempt's output against the existing tier-1 + pacing gates.
/// Reuses `mechanical::inspect_otio`, `PacingStats`, and `gates::{cold_open,
/// floor}` — no gate logic is reimplemented here.
#[allow(clippy::too_many_arguments)] // orthogonal inputs; a struct would add indirection without grouping anything that travels together
pub fn score_attempt(
    outcome: RunOutcome,
    output: &AttemptOutput,
    profile: Option<&HouseProfile>,
    archetype: Option<&str>,
) -> ScoredAttempt {
    let mechanical = output.otio_path.as_ref().map(inspect_otio);

    let pacing_stats = match (&output.cut_times, output.duration_secs) {
        (Some(cuts), Some(duration)) => PacingStats::from_cut_times(cuts, duration).ok(),
        _ => None,
    };

    let (cold_open_gate, floor_gate) = match (&pacing_stats, profile) {
        (Some(stats), Some(profile)) => {
            let cold_open = gates::cold_open(stats, profile);
            let floor = archetype.map(|archetype| gates::floor(stats, profile, archetype));
            (Some(cold_open), floor)
        }
        _ => (None, None),
    };

    ScoredAttempt {
        config_name: outcome.config_name,
        trial: outcome.trial,
        wall_time: outcome.wall_time,
        process_success: outcome.process_success,
        telemetry: outcome.telemetry,
        mechanical,
        measurable: None,
        cold_open_gate,
        floor_gate,
        raw_stdout: outcome.raw_stdout,
        raw_stderr: outcome.raw_stderr,
    }
}

// ---------------------------------------------------------------------
// Paired comparison
// ---------------------------------------------------------------------

/// Aggregate stats for one config's trials on one scenario.
#[derive(Debug, Clone, Serialize)]
pub struct ConfigSummary {
    /// Config name.
    pub config_name: String,
    /// Number of trials rolled up.
    pub trials: u32,
    /// Fraction of trials where every scored gate passed (0.0–1.0).
    pub gate_pass_rate: f64,
    /// Fraction of tool calls across all trials that were NOT errors
    /// (1.0 = perfect harness-contract compliance). `None` when no tool
    /// calls were observed in any trial.
    pub tool_call_correctness: Option<f64>,
    /// Mean total tokens across trials.
    pub mean_total_tokens: f64,
    /// Mean turns-to-completion across trials.
    pub mean_turns_completed: f64,
}

impl ConfigSummary {
    fn from_attempts(config_name: &str, attempts: &[ScoredAttempt]) -> Self {
        let trials = attempts.len() as u32;
        let passed = attempts.iter().filter(|a| a.gates_passed()).count();
        let gate_pass_rate = if trials == 0 {
            0.0
        } else {
            passed as f64 / trials as f64
        };

        let total_tool_calls: u32 = attempts.iter().map(|a| a.telemetry.tool_calls).sum();
        let total_tool_call_errors: u32 =
            attempts.iter().map(|a| a.telemetry.tool_call_errors).sum();
        let tool_call_correctness = if total_tool_calls == 0 {
            None
        } else {
            Some(1.0 - (total_tool_call_errors as f64 / total_tool_calls as f64))
        };

        let mean = |f: fn(&ScoredAttempt) -> f64| -> f64 {
            if attempts.is_empty() {
                0.0
            } else {
                attempts.iter().map(f).sum::<f64>() / attempts.len() as f64
            }
        };
        let mean_total_tokens = mean(|a| a.telemetry.total_tokens as f64);
        let mean_turns_completed = mean(|a| f64::from(a.telemetry.turns_completed));

        Self {
            config_name: config_name.to_string(),
            trials,
            gate_pass_rate,
            tool_call_correctness,
            mean_total_tokens,
            mean_turns_completed,
        }
    }
}

/// Paired A-vs-B comparison for one scenario.
#[derive(Debug, Clone, Serialize)]
pub struct PairedComparison {
    /// Scenario identifier being compared.
    pub scenario_id: String,
    /// Config A's aggregate stats.
    pub config_a: ConfigSummary,
    /// Config B's aggregate stats.
    pub config_b: ConfigSummary,
    /// `true` when B holds gate pass-rate and tool-call correctness flat
    /// or better relative to A. This is the decision doc's win condition
    /// (§6.5) minus the token-reduction requirement, which callers should
    /// check separately via `mean_total_tokens` since "flat or better" on
    /// cost is a judgment call, not a hard pass/fail.
    pub b_holds_correctness: bool,
}

/// Build a paired comparison from all attempts run for one scenario across
/// both configs.
pub fn compare(
    scenario_id: &str,
    config_a_name: &str,
    config_a_attempts: &[ScoredAttempt],
    config_b_name: &str,
    config_b_attempts: &[ScoredAttempt],
) -> PairedComparison {
    let config_a = ConfigSummary::from_attempts(config_a_name, config_a_attempts);
    let config_b = ConfigSummary::from_attempts(config_b_name, config_b_attempts);

    let tool_call_correctness_holds = match (
        config_a.tool_call_correctness,
        config_b.tool_call_correctness,
    ) {
        (Some(a), Some(b)) => b >= a,
        // If neither config produced tool calls to measure, correctness
        // trivially "holds" (nothing regressed); if only one side has
        // data, be conservative and call it not holding — that asymmetry
        // itself is worth a human look.
        (None, None) => true,
        _ => false,
    };
    let b_holds_correctness =
        config_b.gate_pass_rate >= config_a.gate_pass_rate && tool_call_correctness_holds;

    PairedComparison {
        scenario_id: scenario_id.to_string(),
        config_a,
        config_b,
        b_holds_correctness,
    }
}

// ---------------------------------------------------------------------
// End-to-end drive of one scenario across both configs
// ---------------------------------------------------------------------

/// Everything needed to run one scenario through both configs and produce
/// a [`PairedComparison`], via any [`CodexExecRunner`] (real or fake).
pub struct AbDriver<'a, R: CodexExecRunner> {
    /// Runner used for every invocation (subprocess in production, fake in
    /// tests).
    pub runner: &'a R,
    /// Path to the `codex-exec` binary. Ignored by [`FakeCodexExecRunner`]
    /// but still recorded on the constructed invocation for logging.
    pub codex_exec_binary: PathBuf,
    /// House profile used for pacing gates, if any.
    pub profile: Option<HouseProfile>,
    /// Archetype used for the floor gate, if any.
    pub archetype: Option<String>,
    /// Called with `(config_name, trial)` before each attempt runs.
    /// Production uses this to restore the scenario project to a pristine
    /// snapshot: attempts mutate the project on disk, so without a reset
    /// every attempt after the first starts from the previous attempt's
    /// edits and the paired comparison is confounded. `None` skips the
    /// hook (fine for fakes that never touch a project).
    pub pre_attempt: Option<PreAttemptHook<'a>>,
}

/// Pre-attempt callback: `(config_name, trial)`, invoked before each
/// (config, trial) pass. See [`AbDriver::pre_attempt`].
pub type PreAttemptHook<'a> = &'a dyn Fn(&str, u32);

/// Failure running the A/B driver end-to-end.
#[derive(Debug, Error)]
pub enum AbDriveError {
    /// The runner failed to execute an invocation.
    #[error(transparent)]
    Runner(#[from] RunnerError),
    /// Telemetry could not be parsed from the JSONL stream.
    #[error(transparent)]
    Telemetry(#[from] TelemetryError),
}

impl<'a, R: CodexExecRunner> AbDriver<'a, R> {
    /// Run `trials` passes of `scenario` under `config`, scoring each
    /// attempt against `output_for_trial` (a caller-supplied function
    /// resolving what artifact the attempt should be scored against — in
    /// production this reads back whatever the agent produced in the
    /// project; in tests it returns a fixture [`AttemptOutput`]).
    pub fn run_config(
        &self,
        manifest: &AbManifest,
        config: &AbConfig,
        scenario: &Scenario,
        output_for_trial: impl Fn(u32) -> AttemptOutput,
    ) -> Result<Vec<ScoredAttempt>, AbDriveError> {
        let mut attempts = Vec::with_capacity(manifest.trials as usize);
        for trial in 1..=manifest.trials {
            if let Some(pre_attempt) = self.pre_attempt {
                pre_attempt(&config.name, trial);
            }
            let invocation =
                build_invocation(self.codex_exec_binary.clone(), manifest, config, scenario);
            let started = Instant::now();
            let result = self.runner.run(&invocation)?;
            let wall_time = started.elapsed();
            let telemetry = parse_telemetry(&result.stdout)?;
            let output = output_for_trial(trial);
            let outcome = RunOutcome {
                config_name: config.name.clone(),
                trial,
                wall_time,
                process_success: result.success,
                telemetry,
                raw_stdout: result.stdout,
                raw_stderr: result.stderr,
            };
            attempts.push(score_attempt(
                outcome,
                &output,
                self.profile.as_ref(),
                self.archetype.as_deref(),
            ));
        }
        Ok(attempts)
    }

    /// Run both configs for one scenario and produce the paired comparison.
    pub fn run_scenario(
        &self,
        manifest: &AbManifest,
        scenario: &Scenario,
        output_for_trial: impl Fn(&str, u32) -> AttemptOutput,
    ) -> Result<(Vec<ScoredAttempt>, Vec<ScoredAttempt>, PairedComparison), AbDriveError> {
        let a_name = manifest.config_a.name.clone();
        let b_name = manifest.config_b.name.clone();
        let a_attempts = self.run_config(manifest, &manifest.config_a, scenario, |trial| {
            output_for_trial(&a_name, trial)
        })?;
        let b_attempts = self.run_config(manifest, &manifest.config_b, scenario, |trial| {
            output_for_trial(&b_name, trial)
        })?;
        let comparison = compare(
            &scenario.id,
            &manifest.config_a.name,
            &a_attempts,
            &manifest.config_b.name,
            &b_attempts,
        );
        Ok((a_attempts, b_attempts, comparison))
    }
}

// ---------------------------------------------------------------------
// Artifact writing (attempt_N convention, reusing run_artifacts)
// ---------------------------------------------------------------------

/// Write one scored attempt's evidence into the scenario's
/// `attempt_<trial>/evidence/` directory, following the same immutable
/// `attempt_N` convention `run_artifacts::RunArtifacts` uses for the
/// golden/product lanes. Writes `telemetry.json` and, when present,
/// `mechanical.json` / gate reports.
pub fn write_attempt_artifacts(
    run: &crate::RunArtifacts,
    scored: &ScoredAttempt,
) -> Result<crate::AttemptArtifacts, AbArtifactError> {
    let attempt = run.create_attempt(scored.trial)?;
    let evidence_dir = attempt.evidence_dir();

    write_json(&evidence_dir.join("telemetry.json"), &scored.telemetry)?;
    if let Some(mechanical) = &scored.mechanical {
        write_json(&evidence_dir.join("mechanical.json"), mechanical)?;
    }
    if let Some(gate) = &scored.cold_open_gate {
        write_json(&evidence_dir.join("cold_open_gate.json"), gate)?;
    }
    if let Some(gate) = &scored.floor_gate {
        write_json(&evidence_dir.join("floor_gate.json"), gate)?;
    }
    write_json(&evidence_dir.join("attempt_summary.json"), scored)?;

    // Raw stream + stderr, verbatim: the only way to diagnose a
    // surprising rollup without re-running a paid model session.
    write_raw(&evidence_dir.join("raw_stream.jsonl"), &scored.raw_stdout)?;
    if !scored.raw_stderr.is_empty() {
        write_raw(&evidence_dir.join("raw_stderr.log"), &scored.raw_stderr)?;
    }

    Ok(attempt)
}

fn write_raw(path: &Path, contents: &str) -> Result<(), AbArtifactError> {
    std::fs::write(path, contents).map_err(|source| AbArtifactError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Error writing A/B artifacts.
#[derive(Debug, Error)]
pub enum AbArtifactError {
    /// Creating the underlying `attempt_N` directory failed.
    #[error(transparent)]
    RunArtifact(#[from] crate::RunArtifactError),
    /// Serializing or writing an evidence file failed.
    #[error("writing {}: {source}", path.display())]
    Io {
        /// Evidence file path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), AbArtifactError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| AbArtifactError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
    })?;
    let mut file = std::fs::File::create(path).map_err(|source| AbArtifactError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(&bytes)
        .map_err(|source| AbArtifactError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(())
}

/// Write the scenario-level paired comparison as `comparison.json` at the
/// scenario's run root.
pub fn write_comparison(
    run: &crate::RunArtifacts,
    comparison: &PairedComparison,
) -> Result<(), AbArtifactError> {
    write_json(&run.root().join("comparison.json"), comparison)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Scenario;

    const SCENARIO_YAML: &str = r#"
id: podcast_dead_air_basic_001
category: podcast
tool: auto-cutter
source: corpus/podcast/source.mp4
task: Remove dead air.
hard_gates:
  playable: true
soft_gates:
  declared_playbook: auto-cutter
  min_style_score: 0.8
repair:
  policy: while_improving
  safety_ceiling: 10
guards:
  allow_scenario_edits: false
  allow_threshold_edits: false
  max_files_changed: 6
  max_lines_changed: 400
"#;

    fn scenario() -> Scenario {
        Scenario::from_yaml_str(SCENARIO_YAML)
            .unwrap_or_else(|error| panic!("test scenario should load: {error}"))
    }

    fn manifest_a_stock_b_merged() -> AbManifest {
        AbManifest {
            model: "gpt-5.2-codex".to_string(),
            trials: 1,
            config_a: AbConfig {
                name: "A-stock".to_string(),
                base_instructions_file: None,
                developer_instructions_file: None,
                extra_config_overrides: Vec::new(),
            },
            config_b: AbConfig {
                name: "B-merged".to_string(),
                base_instructions_file: Some(PathBuf::from("fixtures/ab/config-b-base.md")),
                developer_instructions_file: Some(PathBuf::from(
                    "fixtures/ab/config-b-developer.md",
                )),
                extra_config_overrides: vec!["show_raw_agent_reasoning=true".to_string()],
            },
        }
    }

    // -----------------------------------------------------------------
    // Command construction
    // -----------------------------------------------------------------

    #[test]
    fn config_a_omits_instruction_override_flags() {
        let manifest = manifest_a_stock_b_merged();
        let invocation = build_invocation(
            "/usr/local/bin/codex-exec",
            &manifest,
            &manifest.config_a,
            &scenario(),
        );

        assert!(
            !invocation
                .args
                .contains(&"--base-instructions-file".to_string())
        );
        assert!(
            !invocation
                .args
                .contains(&"--developer-instructions-file".to_string())
        );
        assert!(invocation.args.contains(&"--json".to_string()));
        assert!(invocation.args.contains(&"gpt-5.2-codex".to_string()));
        // The scenario's task text is the final positional prompt.
        assert_eq!(
            invocation.args.last(),
            Some(&"Remove dead air.".to_string())
        );
    }

    #[test]
    fn config_b_sets_both_instruction_override_flags_and_extra_overrides() {
        let manifest = manifest_a_stock_b_merged();
        let invocation = build_invocation(
            "/usr/local/bin/codex-exec",
            &manifest,
            &manifest.config_b,
            &scenario(),
        );

        let base_idx = invocation
            .args
            .iter()
            .position(|a| a == "--base-instructions-file")
            .unwrap_or_else(|| panic!("config B must set --base-instructions-file"));
        assert_eq!(
            invocation.args[base_idx + 1],
            "fixtures/ab/config-b-base.md"
        );

        let dev_idx = invocation
            .args
            .iter()
            .position(|a| a == "--developer-instructions-file")
            .unwrap_or_else(|| panic!("config B must set --developer-instructions-file"));
        assert_eq!(
            invocation.args[dev_idx + 1],
            "fixtures/ab/config-b-developer.md"
        );

        // Extra config overrides ride as `-c key=value` pairs.
        let dash_c_idx = invocation
            .args
            .iter()
            .position(|a| a == "-c")
            .unwrap_or_else(|| panic!("extra_config_overrides must produce a -c flag"));
        assert_eq!(
            invocation.args[dash_c_idx + 1],
            "show_raw_agent_reasoning=true"
        );
    }

    #[test]
    fn invocation_cwd_is_the_scenario_source() {
        let manifest = manifest_a_stock_b_merged();
        let invocation = build_invocation("codex-exec", &manifest, &manifest.config_a, &scenario());
        assert_eq!(invocation.cwd, PathBuf::from("corpus/podcast/source.mp4"));
    }

    // -----------------------------------------------------------------
    // JSONL telemetry parsing
    // -----------------------------------------------------------------

    /// Real-shaped fixture JSONL: two turns, one tool call that succeeds,
    /// one tool call that fails with a structured error. Field names
    /// mirror `vendor/codex-rs/exec/src/exec_events.rs` exactly so a
    /// schema drift in the upstream wire format would show up here as a
    /// parse/count mismatch, not a silent zero.
    const FIXTURE_JSONL: &str = r#"
{"type":"thread.started","thread_id":"th_1"}
{"type":"turn.started"}
{"type":"item.started","item":{"id":"item_1","type":"mcp_tool_call","server":"montage","tool":"apply_edl","status":"in_progress"}}
{"type":"item.completed","item":{"id":"item_1","type":"mcp_tool_call","server":"montage","tool":"apply_edl","status":"completed"}}
{"type":"turn.completed","usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":300,"reasoning_output_tokens":50}}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"item_2","type":"mcp_tool_call","server":"montage","tool":"apply_edl","status":"failed","error":{"message":"invalid clip_uuid"}}}
{"type":"turn.completed","usage":{"input_tokens":500,"cached_input_tokens":0,"output_tokens":150,"reasoning_output_tokens":0}}
"#;

    #[test]
    fn parses_token_totals_and_turn_counts_from_fixture_jsonl() {
        let telemetry = parse_telemetry(FIXTURE_JSONL)
            .unwrap_or_else(|e| panic!("telemetry should parse: {e}"));

        assert_eq!(telemetry.turns_completed, 2);
        assert_eq!(telemetry.turns_failed, 0);
        // (1000+200+300+50) + (500+0+150+0) = 1550 + 650 = 2200
        assert_eq!(telemetry.total_tokens, 2200);
    }

    #[test]
    fn parses_tool_call_counts_and_errors_from_fixture_jsonl() {
        let telemetry = parse_telemetry(FIXTURE_JSONL)
            .unwrap_or_else(|e| panic!("telemetry should parse: {e}"));

        assert_eq!(telemetry.tool_calls, 2);
        assert_eq!(telemetry.tool_call_errors, 1);
        assert_eq!(
            telemetry.error_messages,
            vec!["invalid clip_uuid".to_string()]
        );
    }

    #[test]
    fn shell_and_file_change_items_are_counted_separately_from_tool_calls() {
        // Real streams from the smoke run: an agent that edits the
        // project via shell/apply_patch instead of MCP tools produces
        // zero `mcp_tool_call` items while still mutating the timeline.
        // These counters make that doctrine violation visible in the
        // rollup instead of requiring the raw stream.
        let jsonl = r#"
{"type":"item.completed","item":{"id":"i1","type":"command_execution","command":"ls raw/","status":"completed"}}
{"type":"item.completed","item":{"id":"i2","type":"command_execution","command":"python3 edit.py","status":"completed"}}
{"type":"item.completed","item":{"id":"i3","type":"file_change","status":"completed"}}
{"type":"item.completed","item":{"id":"i4","type":"mcp_tool_call","server":"montage","tool":"view_episode","status":"completed"}}
{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0}}
"#;
        let telemetry =
            parse_telemetry(jsonl).unwrap_or_else(|e| panic!("telemetry should parse: {e}"));

        assert_eq!(telemetry.shell_calls, 2);
        assert_eq!(telemetry.file_changes, 1);
        assert_eq!(telemetry.tool_calls, 1);
        assert_eq!(telemetry.tool_call_errors, 0);
    }

    #[test]
    fn pre_attempt_hook_fires_before_every_config_trial_pair() {
        use std::cell::RefCell;

        let mut manifest = manifest_a_stock_b_merged();
        manifest.trials = 2;
        let scenario = scenario();
        let runner = FakeCodexExecRunner::constant(CodexExecOutput {
            success: true,
            stdout: FIXTURE_JSONL.to_string(),
            stderr: String::new(),
        });

        let seen: RefCell<Vec<(String, u32)>> = RefCell::new(Vec::new());
        let record = |config: &str, trial: u32| {
            seen.borrow_mut().push((config.to_string(), trial));
        };
        let driver = AbDriver {
            runner: &runner,
            codex_exec_binary: PathBuf::from("codex-exec"),
            profile: None,
            archetype: None,
            pre_attempt: Some(&record),
        };

        driver
            .run_scenario(&manifest, &scenario, |_config, _trial| AttemptOutput {
                otio_path: None,
                cut_times: None,
                duration_secs: None,
            })
            .unwrap_or_else(|e| panic!("fake-backed run should succeed: {e}"));

        assert_eq!(
            seen.into_inner(),
            vec![
                ("A-stock".to_string(), 1),
                ("A-stock".to_string(), 2),
                ("B-merged".to_string(), 1),
                ("B-merged".to_string(), 2),
            ]
        );
    }

    #[test]
    fn turn_failed_events_are_counted_and_captured() {
        let jsonl = r#"{"type":"turn.failed","error":{"message":"context window exceeded"}}"#;
        let telemetry =
            parse_telemetry(jsonl).unwrap_or_else(|e| panic!("telemetry should parse: {e}"));

        assert_eq!(telemetry.turns_failed, 1);
        assert_eq!(telemetry.turns_completed, 0);
        assert_eq!(
            telemetry.error_messages,
            vec!["context window exceeded".to_string()]
        );
    }

    #[test]
    fn unknown_event_types_are_ignored_not_rejected() {
        let jsonl = r#"{"type":"some.future.event","payload":{"whatever":true}}"#;
        let telemetry =
            parse_telemetry(jsonl).unwrap_or_else(|e| panic!("telemetry should parse: {e}"));
        assert_eq!(telemetry.turns_completed, 0);
        assert_eq!(telemetry.tool_calls, 0);
    }

    #[test]
    fn blank_lines_are_skipped() {
        let jsonl = "\n\n{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"cached_input_tokens\":0,\"output_tokens\":0,\"reasoning_output_tokens\":0}}\n\n";
        let telemetry =
            parse_telemetry(jsonl).unwrap_or_else(|e| panic!("telemetry should parse: {e}"));
        assert_eq!(telemetry.turns_completed, 1);
    }

    #[test]
    fn malformed_json_line_is_a_reported_error() {
        let jsonl = "not json at all";
        let error = match parse_telemetry(jsonl) {
            Ok(_) => panic!("malformed line must be an error"),
            Err(error) => error,
        };
        match error {
            TelemetryError::Line { line, .. } => assert_eq!(line, 1),
        }
    }

    // -----------------------------------------------------------------
    // Gate scoring over fixture output (reuses existing scorers)
    // -----------------------------------------------------------------

    #[test]
    fn malformed_otio_output_fails_the_mechanical_gate() {
        let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let otio_path = temp.path().join("output.otio");
        std::fs::write(&otio_path, b"not json").unwrap_or_else(|e| panic!("write fixture: {e}"));

        let output = AttemptOutput {
            otio_path: Some(otio_path),
            cut_times: None,
            duration_secs: None,
        };
        let telemetry = parse_telemetry(FIXTURE_JSONL).unwrap_or_else(|e| panic!("{e}"));
        let scored = score_attempt(test_outcome("B-merged", telemetry), &output, None, None);

        assert!(scored.mechanical.as_ref().is_some_and(|r| !r.passed));
        assert!(!scored.gates_passed());
    }

    #[test]
    fn missing_output_evidence_skips_rather_than_fails_gates() {
        // A driver that has no OTIO output yet (e.g. telemetry-only smoke
        // run) must not silently count as a gate failure — that would
        // conflate "we didn't check" with "we checked and it's bad".
        let output = AttemptOutput {
            otio_path: None,
            cut_times: None,
            duration_secs: None,
        };
        let telemetry = parse_telemetry(FIXTURE_JSONL).unwrap_or_else(|e| panic!("{e}"));
        let scored = score_attempt(test_outcome("A-stock", telemetry), &output, None, None);

        assert!(scored.mechanical.is_none());
        assert!(scored.cold_open_gate.is_none());
        assert!(scored.floor_gate.is_none());
        assert!(scored.gates_passed());
    }

    #[test]
    fn pacing_gates_run_when_cut_times_and_profile_are_available() {
        let profile = HouseProfile::builtin("doac")
            .unwrap_or_else(|| panic!("builtin doac profile must exist"));
        // Sparse cuts across a 10-minute program should trip the floor
        // gate for the "informational" archetype (fixtures/profiles/doac.json
        // defines a floor for it).
        let output = AttemptOutput {
            otio_path: None,
            cut_times: Some(vec![1.0, 2.0, 3.0]),
            duration_secs: Some(600.0),
        };
        let telemetry = Telemetry::default();
        let scored = score_attempt(
            test_outcome("A-stock", telemetry),
            &output,
            Some(&profile),
            Some("informational"),
        );

        assert!(scored.cold_open_gate.is_some());
        assert!(scored.floor_gate.is_some());
        // Only 3 cuts across 10 minutes should trip both cold-open and
        // floor for this profile's thresholds.
        assert!(!scored.gates_passed());
    }

    /// A `RunOutcome` for scoring tests, with fixed wall time / success.
    fn test_outcome(config_name: &str, telemetry: Telemetry) -> RunOutcome {
        RunOutcome {
            config_name: config_name.to_string(),
            trial: 1,
            wall_time: Duration::from_secs(1),
            process_success: true,
            telemetry,
            raw_stdout: String::new(),
            raw_stderr: String::new(),
        }
    }

    // -----------------------------------------------------------------
    // Paired comparison
    // -----------------------------------------------------------------

    fn passing_attempt(
        config_name: &str,
        trial: u32,
        tool_calls: u32,
        tool_call_errors: u32,
        tokens: i64,
    ) -> ScoredAttempt {
        ScoredAttempt {
            config_name: config_name.to_string(),
            trial,
            wall_time: Duration::from_secs(1),
            process_success: true,
            telemetry: Telemetry {
                total_tokens: tokens,
                turns_completed: 2,
                tool_calls,
                tool_call_errors,
                ..Telemetry::default()
            },
            mechanical: None,
            measurable: None,
            cold_open_gate: None,
            floor_gate: None,
            raw_stdout: String::new(),
            raw_stderr: String::new(),
        }
    }

    #[test]
    fn b_holds_correctness_when_gate_rate_and_tool_calls_are_flat_or_better() {
        let a = vec![passing_attempt("A-stock", 1, 10, 1, 5000)];
        let b = vec![passing_attempt("B-merged", 1, 10, 0, 3000)];

        let comparison = compare("scenario-1", "A-stock", &a, "B-merged", &b);

        assert!(comparison.b_holds_correctness);
        assert_eq!(comparison.config_a.tool_call_correctness, Some(0.9));
        assert_eq!(comparison.config_b.tool_call_correctness, Some(1.0));
        assert!(comparison.config_b.mean_total_tokens < comparison.config_a.mean_total_tokens);
    }

    #[test]
    fn b_does_not_hold_correctness_when_tool_call_errors_regress() {
        let a = vec![passing_attempt("A-stock", 1, 10, 0, 5000)];
        let b = vec![passing_attempt("B-merged", 1, 10, 3, 3000)];

        let comparison = compare("scenario-1", "A-stock", &a, "B-merged", &b);

        assert!(!comparison.b_holds_correctness);
    }

    #[test]
    fn b_does_not_hold_correctness_when_gate_pass_rate_regresses() {
        let mut a_pass = passing_attempt("A-stock", 1, 5, 0, 1000);
        a_pass.mechanical = Some(crate::MechanicalReport {
            passed: true,
            checks: Vec::new(),
        });
        let mut b_fail = passing_attempt("B-merged", 1, 5, 0, 1000);
        b_fail.mechanical = Some(crate::MechanicalReport {
            passed: false,
            checks: Vec::new(),
        });

        let comparison = compare("scenario-1", "A-stock", &[a_pass], "B-merged", &[b_fail]);

        assert_eq!(comparison.config_a.gate_pass_rate, 1.0);
        assert_eq!(comparison.config_b.gate_pass_rate, 0.0);
        assert!(!comparison.b_holds_correctness);
    }

    // -----------------------------------------------------------------
    // End-to-end pipeline against a fake codex-exec (no auth, no model,
    // no network, no cost — proves the wiring, not model behavior).
    // -----------------------------------------------------------------

    #[test]
    fn end_to_end_pipeline_runs_against_fake_runner_and_writes_artifacts() {
        let manifest = manifest_a_stock_b_merged();
        let scenario = scenario();

        let passing_output = CodexExecOutput {
            success: true,
            stdout: FIXTURE_JSONL.to_string(),
            stderr: String::new(),
        };
        let mut runner = FakeCodexExecRunner::constant(passing_output);
        // Give config B (identified by its --base-instructions-file arg) a
        // distinct fixture so the paired comparison isn't trivially equal.
        let b_output = CodexExecOutput {
            success: true,
            stdout: r#"{"type":"turn.completed","usage":{"input_tokens":10,"cached_input_tokens":0,"output_tokens":10,"reasoning_output_tokens":0}}"#
                .to_string(),
            stderr: String::new(),
        };
        runner
            .by_base_instructions_file
            .insert("fixtures/ab/config-b-base.md".to_string(), b_output);

        let driver = AbDriver {
            runner: &runner,
            codex_exec_binary: PathBuf::from("codex-exec"),
            profile: None,
            archetype: None,
            pre_attempt: None,
        };

        let (a_attempts, b_attempts, comparison) = driver
            .run_scenario(&manifest, &scenario, |_config_name, _trial| AttemptOutput {
                otio_path: None,
                cut_times: None,
                duration_secs: None,
            })
            .unwrap_or_else(|e| panic!("A/B run against fake runner should succeed: {e}"));

        assert_eq!(a_attempts.len(), 1);
        assert_eq!(b_attempts.len(), 1);
        // Config A got the FIXTURE_JSONL (2 turns, 2200 tokens); config B
        // got the smaller single-turn fixture (20 tokens).
        assert_eq!(a_attempts[0].telemetry.total_tokens, 2200);
        assert_eq!(b_attempts[0].telemetry.total_tokens, 20);
        assert_eq!(comparison.config_a.mean_total_tokens, 2200.0);
        assert_eq!(comparison.config_b.mean_total_tokens, 20.0);

        // Artifacts: run_artifacts conventions produce attempt_N dirs with
        // evidence, and this driver writes telemetry.json + a summary into
        // each, plus a scenario-root comparison.json.
        let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let run = crate::RunArtifacts::create(temp.path(), "ab-run-a", &scenario)
            .unwrap_or_else(|e| panic!("run artifacts should create: {e}"));
        let attempt = write_attempt_artifacts(&run, &a_attempts[0])
            .unwrap_or_else(|e| panic!("attempt artifacts should write: {e}"));
        assert!(attempt.evidence_dir().join("telemetry.json").is_file());
        assert!(
            attempt
                .evidence_dir()
                .join("attempt_summary.json")
                .is_file()
        );

        write_comparison(&run, &comparison)
            .unwrap_or_else(|e| panic!("comparison should write: {e}"));
        assert!(run.root().join("comparison.json").is_file());
    }

    #[test]
    fn ab_manifest_loads_from_toml_fixture() {
        let manifest_path = format!(
            "{}/fixtures/ab/manifest.example.toml",
            env!("CARGO_MANIFEST_DIR")
        );
        let manifest = AbManifest::from_toml_file(&manifest_path)
            .unwrap_or_else(|e| panic!("example manifest should load: {e}"));

        assert_eq!(manifest.config_a.name, "A-stock");
        assert!(manifest.config_a.base_instructions_file.is_none());
        assert_eq!(manifest.config_b.name, "B-merged-placeholder");
        assert!(manifest.config_b.base_instructions_file.is_some());
        assert!(manifest.config_b.developer_instructions_file.is_some());
    }

    #[test]
    fn ab_manifest_rejects_duplicate_config_names() {
        let toml = r#"
model = "gpt-5.2-codex"
trials = 1

[config_a]
name = "same"

[config_b]
name = "same"
"#;
        let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let path = temp.path().join("manifest.toml");
        std::fs::write(&path, toml).unwrap_or_else(|e| panic!("write: {e}"));

        let error = match AbManifest::from_toml_file(&path) {
            Ok(_) => panic!("duplicate names must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("distinct names"));
    }
}
