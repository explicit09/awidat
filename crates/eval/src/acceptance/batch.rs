//! Batch runner and summaries for local acceptance fixture directories.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;

use super::case::load_case_from_path;
use super::external_fixture_paths_from_path;
use super::runner::run_case;
use super::scorecard::AcceptanceScorecard;

/// Run a local fixture file or directory and write a batch summary artifact.
pub async fn run_fixture_batch(fixture_path: &Path) -> Result<AcceptanceBatchRun> {
    let fixture_paths = external_fixture_paths_from_path(fixture_path)?;
    let batch_root = create_batch_root()?;
    let mut entries = Vec::new();

    for path in fixture_paths {
        let entry = match load_case_from_path(&path) {
            Ok(case) => match run_case(&case).await {
                Ok(run) => batch_entry_from_scorecard(
                    &path,
                    case.scenario_type.as_deref(),
                    &run.scorecard,
                    &run.scorecard_path,
                ),
                Err(err) => batch_error_entry(
                    &path,
                    Some(&case.id),
                    Some(&case.objective),
                    case.scenario_type.as_deref(),
                    err,
                ),
            },
            Err(err) => batch_error_entry(&path, None, None, None, err),
        };
        entries.push(entry);
    }

    let mut summary = AcceptanceBatchSummary::new(fixture_path, &batch_root, entries);
    let summary_json_path = batch_root.join("batch_summary.json");
    let summary_markdown_path = batch_root.join("batch_summary.md");
    summary.batch_summary_json = summary_json_path.to_string_lossy().into_owned();
    summary.batch_summary_md = summary_markdown_path.to_string_lossy().into_owned();
    fs::write(&summary_json_path, serde_json::to_vec_pretty(&summary)?)
        .with_context(|| format!("write {}", summary_json_path.display()))?;
    fs::write(&summary_markdown_path, format_batch_markdown(&summary))
        .with_context(|| format!("write {}", summary_markdown_path.display()))?;

    Ok(AcceptanceBatchRun {
        summary,
        summary_json_path,
        summary_markdown_path,
    })
}

/// Result of a batch run plus artifact locations.
#[derive(Debug)]
pub struct AcceptanceBatchRun {
    /// Machine-readable summary that was written to disk.
    pub summary: AcceptanceBatchSummary,
    /// Path to `batch_summary.json`.
    pub summary_json_path: PathBuf,
    /// Path to `batch_summary.md`.
    pub summary_markdown_path: PathBuf,
}

/// Machine-readable summary for one acceptance batch.
#[derive(Debug, Serialize)]
pub struct AcceptanceBatchSummary {
    /// RFC3339 generation time.
    pub generated_at: String,
    /// Fixture file or directory that was run.
    pub fixture_root: String,
    /// Batch artifact directory.
    pub batch_root: String,
    /// Number of fixtures attempted.
    pub fixture_count: usize,
    /// Passing fixture count.
    pub passed: usize,
    /// Warning fixture count.
    pub warned: usize,
    /// Failing fixture count.
    pub failed: usize,
    /// Path to this JSON summary.
    pub batch_summary_json: String,
    /// Path to the human-readable Markdown summary.
    pub batch_summary_md: String,
    /// Per-fixture run summaries.
    pub fixtures: Vec<BatchSummaryEntry>,
}

impl AcceptanceBatchSummary {
    fn new(fixture_path: &Path, batch_root: &Path, fixtures: Vec<BatchSummaryEntry>) -> Self {
        let passed = fixtures
            .iter()
            .filter(|entry| entry.status == "pass")
            .count();
        let warned = fixtures
            .iter()
            .filter(|entry| entry.status == "warn")
            .count();
        let failed = fixtures
            .iter()
            .filter(|entry| entry.status == "fail")
            .count();
        Self {
            generated_at: Utc::now().to_rfc3339(),
            fixture_root: fixture_path.to_string_lossy().into_owned(),
            batch_root: batch_root.to_string_lossy().into_owned(),
            fixture_count: fixtures.len(),
            passed,
            warned,
            failed,
            batch_summary_json: String::new(),
            batch_summary_md: String::new(),
            fixtures,
        }
    }
}

/// One fixture row in an acceptance batch summary.
#[derive(Debug, Serialize)]
pub struct BatchSummaryEntry {
    /// Fixture id, or filename fallback when parsing failed.
    pub fixture_id: String,
    /// Fixture objective, or error context when parsing failed.
    pub objective: String,
    /// Scenario behavior category.
    pub scenario_type: String,
    /// Source fixture path.
    pub fixture_path: String,
    /// Scorecard status: pass, warn, or fail.
    pub status: String,
    /// Scorecard score when the fixture ran.
    pub score: Option<f64>,
    /// Scorecard artifact path.
    pub scorecard_path: Option<String>,
    /// Rendered MP4 path.
    pub rendered_output_path: Option<String>,
    /// Artifact bundle path.
    pub artifact_bundle_path: Option<String>,
    /// Highest-signal failing gate names.
    pub top_failing_gates: Vec<String>,
    /// One-line review message.
    pub message: String,
}

pub(crate) fn batch_entry_from_scorecard(
    fixture_path: &Path,
    scenario_type: Option<&str>,
    scorecard: &AcceptanceScorecard,
    scorecard_path: &Path,
) -> BatchSummaryEntry {
    let top_failing_gates = scorecard
        .gates
        .iter()
        .filter(|gate| gate.status == "fail")
        .take(5)
        .map(|gate| gate.name.clone())
        .collect::<Vec<_>>();
    let message = if top_failing_gates.is_empty() {
        format!(
            "score {:.1}/100; scorecard {}",
            scorecard.score,
            scorecard_path.display()
        )
    } else {
        format!(
            "score {:.1}/100; failed gates [{}]; scorecard {}",
            scorecard.score,
            top_failing_gates.join(", "),
            scorecard_path.display()
        )
    };
    BatchSummaryEntry {
        fixture_id: scorecard.scenario_id.clone(),
        objective: scorecard.objective.clone(),
        scenario_type: scenario_type.unwrap_or("unspecified").to_string(),
        fixture_path: fixture_path.to_string_lossy().into_owned(),
        status: scorecard.status.clone(),
        score: Some(scorecard.score),
        scorecard_path: Some(scorecard_path.to_string_lossy().into_owned()),
        rendered_output_path: Some(scorecard.output_path.clone()),
        artifact_bundle_path: Some(scorecard.artifacts.artifact_bundle_json.clone()),
        top_failing_gates,
        message,
    }
}

fn batch_error_entry(
    fixture_path: &Path,
    fixture_id: Option<&str>,
    objective: Option<&str>,
    scenario_type: Option<&str>,
    error: anyhow::Error,
) -> BatchSummaryEntry {
    BatchSummaryEntry {
        fixture_id: fixture_id
            .map(str::to_string)
            .unwrap_or_else(|| fixture_path_stem(fixture_path)),
        objective: objective.unwrap_or("fixture could not be run").to_string(),
        scenario_type: scenario_type.unwrap_or("unspecified").to_string(),
        fixture_path: fixture_path.to_string_lossy().into_owned(),
        status: "fail".into(),
        score: None,
        scorecard_path: None,
        rendered_output_path: None,
        artifact_bundle_path: None,
        top_failing_gates: vec!["scenario_bringup".into()],
        message: format!("{error:#}"),
    }
}

fn format_batch_markdown(summary: &AcceptanceBatchSummary) -> String {
    let mut markdown = String::new();
    markdown.push_str("# Acceptance Batch Summary\n\n");
    markdown.push_str(&format!(
        "Fixture root: `{}`\n\nPassed: {}. Warned: {}. Failed: {}. Total: {}.\n\n",
        summary.fixture_root, summary.passed, summary.warned, summary.failed, summary.fixture_count
    ));
    markdown.push_str("| Fixture | Type | Status | Score | Top failing gates |\n");
    markdown.push_str("| --- | --- | --- | ---: | --- |\n");
    for fixture in &summary.fixtures {
        let score = fixture
            .score
            .map(|score| format!("{score:.1}"))
            .unwrap_or_else(|| "n/a".into());
        let failing = if fixture.top_failing_gates.is_empty() {
            "none".into()
        } else {
            fixture.top_failing_gates.join(", ")
        };
        markdown.push_str(&format!(
            "| `{}` | `{}` | `{}` | {} | {} |\n",
            fixture.fixture_id, fixture.scenario_type, fixture.status, score, failing
        ));
    }
    markdown
}

fn create_batch_root() -> Result<PathBuf> {
    static BATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

    let base = std::env::var("AWIDAT_EVAL_BATCHES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from("target")
                .join("awidat-eval")
                .join("acceptance-batches")
        });
    let base = if base.is_absolute() {
        base
    } else {
        std::env::current_dir()
            .context("resolve current directory for acceptance batch artifacts")?
            .join(base)
    };
    fs::create_dir_all(&base)?;
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    for _ in 0..100 {
        let counter = BATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let batch_root = base.join(format!(
            "{}-p{}-{counter:04}",
            timestamp,
            std::process::id()
        ));
        match fs::create_dir(&batch_root) {
            Ok(()) => return Ok(batch_root),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create acceptance batch root"),
        }
    }
    anyhow::bail!(
        "failed to create a unique acceptance batch root under {}",
        base.display()
    )
}

fn fixture_path_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown-fixture")
        .to_string()
}
