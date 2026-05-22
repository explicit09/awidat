//! Failure packet creation for acceptance runs.

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;

/// Package the most useful artifacts from a failed acceptance run.
pub fn package_failure(
    scorecard_or_run_dir: &Path,
    packet_base: Option<&Path>,
) -> Result<FailurePacketReport> {
    let scorecard_path = resolve_scorecard_path(scorecard_or_run_dir)?;
    let scorecard: Value = serde_json::from_slice(
        &fs::read(&scorecard_path).with_context(|| format!("read {}", scorecard_path.display()))?,
    )
    .with_context(|| format!("parse {}", scorecard_path.display()))?;
    let scenario_id = string_field(&scorecard, "scenario_id").unwrap_or("unknown_scenario");
    let objective = string_field(&scorecard, "objective")
        .unwrap_or("acceptance scenario objective was not recorded");
    let status = string_field(&scorecard, "status").unwrap_or("unknown");
    let score = scorecard.get("score").and_then(Value::as_f64);
    let output_path = string_field(&scorecard, "output_path").map(str::to_string);
    let failing_gates = failing_gates(&scorecard);
    let packet_root = create_packet_root(packet_base, scenario_id)?;

    let mut artifacts = Vec::new();
    copy_artifact(
        &mut artifacts,
        "scorecard_json",
        &scorecard_path,
        &packet_root.join("scorecard.json"),
    )?;
    for (name, pointer, file_name) in [
        (
            "artifact_bundle_json",
            "/artifacts/artifact_bundle_json",
            "artifact_bundle.json",
        ),
        (
            "render_manifest",
            "/artifacts/render_manifest",
            "render.render-manifest.json",
        ),
        ("final_edl", "/artifacts/final_edl", "final_edl.edl"),
        (
            "edit_manifest",
            "/artifacts/edit_manifest",
            "edit_manifest.json",
        ),
        ("ffprobe_json", "/artifacts/ffprobe_json", "ffprobe.json"),
        ("silence_json", "/artifacts/silence_json", "silence.json"),
        (
            "blackdetect_json",
            "/artifacts/blackdetect_json",
            "blackdetect.json",
        ),
        (
            "transcript_json",
            "/artifacts/transcript_json",
            "transcript.json",
        ),
        (
            "edl_generator_stdout",
            "/artifacts/edl_generator_stdout",
            "edl_generator_stdout.txt",
        ),
        (
            "edl_generator_stderr",
            "/artifacts/edl_generator_stderr",
            "edl_generator_stderr.txt",
        ),
    ] {
        if let Some(source) = scorecard.pointer(pointer).and_then(Value::as_str) {
            copy_artifact(
                &mut artifacts,
                name,
                Path::new(source),
                &packet_root.join(file_name),
            )?;
        }
    }
    if let Some(path) = &output_path {
        let packet_path = packet_root.join("rendered_output_path.txt");
        fs::write(&packet_path, format!("{path}\n"))
            .with_context(|| format!("write {}", packet_path.display()))?;
        artifacts.push(FailurePacketArtifact {
            name: "rendered_output".into(),
            source_path: Some(path.clone()),
            packet_path: Some(packet_path.to_string_lossy().into_owned()),
            status: "referenced".into(),
            size_bytes: Path::new(path)
                .metadata()
                .ok()
                .map(|metadata| metadata.len()),
        });
    }

    let summary_path = packet_root.join("failure_summary.md");
    fs::write(
        &summary_path,
        failure_summary(
            scenario_id,
            objective,
            status,
            score,
            output_path.as_deref(),
            &failing_gates,
        ),
    )
    .with_context(|| format!("write {}", summary_path.display()))?;
    artifacts.push(FailurePacketArtifact {
        name: "failure_summary".into(),
        source_path: None,
        packet_path: Some(summary_path.to_string_lossy().into_owned()),
        status: "written".into(),
        size_bytes: summary_path.metadata().ok().map(|metadata| metadata.len()),
    });

    let mut report = FailurePacketReport {
        generated_at: Utc::now().to_rfc3339(),
        scenario_id: scenario_id.into(),
        objective: objective.into(),
        status: status.into(),
        score,
        packet_root,
        manifest_path: PathBuf::new(),
        artifacts,
        failing_gates,
    };
    let manifest_path = report.packet_root.join("failure_packet_manifest.json");
    report.manifest_path = manifest_path.clone();
    fs::write(&manifest_path, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("write {}", manifest_path.display()))?;

    Ok(report)
}

/// Machine-readable packet creation report.
#[derive(Debug, Serialize)]
pub struct FailurePacketReport {
    /// RFC3339 generation time.
    pub generated_at: String,
    /// Scenario id from the scorecard.
    pub scenario_id: String,
    /// Scenario objective from the scorecard.
    pub objective: String,
    /// Scorecard status.
    pub status: String,
    /// Scorecard score, when available.
    pub score: Option<f64>,
    /// Created packet directory.
    pub packet_root: PathBuf,
    /// Packet manifest path.
    pub manifest_path: PathBuf,
    /// Copied or referenced artifacts.
    pub artifacts: Vec<FailurePacketArtifact>,
    /// Failing gates summarized for review.
    pub failing_gates: Vec<FailureGateSummary>,
}

/// One artifact copied or referenced in the packet.
#[derive(Debug, Serialize)]
pub struct FailurePacketArtifact {
    /// Stable artifact name.
    pub name: String,
    /// Source artifact path when one exists.
    pub source_path: Option<String>,
    /// Packet-local path when copied or written.
    pub packet_path: Option<String>,
    /// copied, referenced, written, or missing.
    pub status: String,
    /// Source or written size.
    pub size_bytes: Option<u64>,
}

/// Compact failing-gate row for humans and agents.
#[derive(Debug, Serialize)]
pub struct FailureGateSummary {
    /// Gate name.
    pub name: String,
    /// Gate severity.
    pub severity: String,
    /// Gate message.
    pub message: String,
    /// Timestamp or source range hints found in observed evidence.
    pub ranges: Vec<String>,
}

fn resolve_scorecard_path(input: &Path) -> Result<PathBuf> {
    if input.is_file() {
        return Ok(input.to_path_buf());
    }
    if input.is_dir() {
        for candidate in [
            input.join("artifacts").join("scorecard.json"),
            input.join("scorecard.json"),
        ] {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        bail!("no scorecard.json found under {}", input.display());
    }
    bail!(
        "scorecard or run directory does not exist: {}",
        input.display()
    );
}

fn copy_artifact(
    artifacts: &mut Vec<FailurePacketArtifact>,
    name: &str,
    source: &Path,
    destination: &Path,
) -> Result<()> {
    let metadata = source.metadata().ok();
    if metadata.is_some() {
        fs::copy(source, destination)
            .with_context(|| format!("copy {} to {}", source.display(), destination.display()))?;
        artifacts.push(FailurePacketArtifact {
            name: name.into(),
            source_path: Some(source.to_string_lossy().into_owned()),
            packet_path: Some(destination.to_string_lossy().into_owned()),
            status: "copied".into(),
            size_bytes: metadata.map(|metadata| metadata.len()),
        });
    } else {
        artifacts.push(FailurePacketArtifact {
            name: name.into(),
            source_path: Some(source.to_string_lossy().into_owned()),
            packet_path: Some(destination.to_string_lossy().into_owned()),
            status: "missing".into(),
            size_bytes: None,
        });
    }
    Ok(())
}

fn failing_gates(scorecard: &Value) -> Vec<FailureGateSummary> {
    scorecard
        .get("gates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|gate| gate.get("status").and_then(Value::as_str) == Some("fail"))
        .map(|gate| FailureGateSummary {
            name: string_field(gate, "name").unwrap_or("unknown_gate").into(),
            severity: string_field(gate, "severity").unwrap_or("unknown").into(),
            message: string_field(gate, "message").unwrap_or("").into(),
            ranges: collect_range_hints(gate.get("observed").unwrap_or(&Value::Null)),
        })
        .collect()
}

fn collect_range_hints(value: &Value) -> Vec<String> {
    let mut ranges = BTreeSet::new();
    collect_range_hints_inner(value, &mut ranges);
    ranges.into_iter().collect()
}

fn collect_range_hints_inner(value: &Value, ranges: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let (Some(start), Some(end)) = (
                object.get("start_s").and_then(Value::as_f64),
                object.get("end_s").and_then(Value::as_f64),
            ) {
                ranges.insert(format!("{start:.3}-{end:.3}s"));
            }
            if let (Some(start), Some(end)) = (
                object.get("source_start_s").and_then(Value::as_f64),
                object.get("source_end_s").and_then(Value::as_f64),
            ) {
                ranges.insert(format!("source {start:.3}-{end:.3}s"));
            }
            for nested in object.values() {
                collect_range_hints_inner(nested, ranges);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_range_hints_inner(item, ranges);
            }
        }
        _ => {}
    }
}

fn failure_summary(
    scenario_id: &str,
    objective: &str,
    status: &str,
    score: Option<f64>,
    output_path: Option<&str>,
    failing_gates: &[FailureGateSummary],
) -> String {
    let score_text = score
        .map(|score| format!("{score:.1}"))
        .unwrap_or_else(|| "n/a".into());
    let mut summary = String::new();
    summary.push_str("# Failure Summary\n\n");
    summary.push_str(&format!("Scenario: `{scenario_id}`\n\n"));
    summary.push_str(&format!("Objective: {objective}\n\n"));
    summary.push_str(&format!("Status: `{status}`\n\n"));
    summary.push_str(&format!("Score: {score_text}\n\n"));
    if let Some(path) = output_path {
        summary.push_str(&format!("Rendered output: `{path}`\n\n"));
    }
    summary.push_str("## Failing Gates\n\n");
    if failing_gates.is_empty() {
        summary.push_str("No failing gates were recorded in the scorecard.\n\n");
    } else {
        for gate in failing_gates {
            let ranges = if gate.ranges.is_empty() {
                "no timestamp hints".into()
            } else {
                gate.ranges.join(", ")
            };
            summary.push_str(&format!(
                "- `{}` ({}) - {}. Ranges: {}.\n",
                gate.name, gate.severity, gate.message, ranges
            ));
        }
        summary.push('\n');
    }
    summary.push_str("## Where To Look First\n\n");
    summary.push_str("1. Open `scorecard.json` for the verdict and failing gate details.\n");
    summary.push_str(
        "2. Compare `final_edl.edl` and `edit_manifest.json` around the listed ranges.\n",
    );
    summary.push_str("3. Use `silence.json`, `blackdetect.json`, and `ffprobe.json` to separate media pathologies from edit-decision errors.\n");
    summary.push_str("4. Open the rendered output path only around flagged timestamps.\n");
    summary
}

fn create_packet_root(packet_base: Option<&Path>, scenario_id: &str) -> Result<PathBuf> {
    static PACKET_COUNTER: AtomicU64 = AtomicU64::new(0);

    let base = packet_base.map(PathBuf::from).unwrap_or_else(|| {
        std::env::var("AWIDAT_EVAL_FAILURE_PACKETS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from("target")
                    .join("awidat-eval")
                    .join("failure-packets")
            })
    });
    let base = if base.is_absolute() {
        base
    } else {
        std::env::current_dir()
            .context("resolve current directory for failure packet artifacts")?
            .join(base)
    };
    let scenario_root = base.join(sanitize_path_segment(scenario_id));
    fs::create_dir_all(&scenario_root)?;
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    for _ in 0..100 {
        let counter = PACKET_COUNTER.fetch_add(1, Ordering::Relaxed);
        let packet_root = scenario_root.join(format!(
            "{}-p{}-{counter:04}",
            timestamp,
            std::process::id()
        ));
        match fs::create_dir(&packet_root) {
            Ok(()) => return Ok(packet_root),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create failure packet root"),
        }
    }
    anyhow::bail!(
        "failed to create a unique failure packet root under {}",
        scenario_root.display()
    )
}

fn string_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

fn sanitize_path_segment(raw: &str) -> String {
    raw.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}
