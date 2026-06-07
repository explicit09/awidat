//! Performance report helpers for real indexing runs.
//!
//! The dispatcher already records per-pair telemetry. This module turns that
//! telemetry plus sidecar shape metadata into stable JSON/Markdown artifacts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use awidat_proto::index::AssetId;
use serde::{Deserialize, Serialize};

use crate::{IndexReport, PairOutcome};

/// Per-run command context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfCommand {
    /// Temporary/project root used for the measured run.
    pub project_root: String,
    /// Artifact directory where report files are written.
    pub output_dir: String,
    /// Dispatcher concurrency.
    pub concurrency: usize,
    /// Included indexer names.
    pub included_indexers: Vec<String>,
    /// Asset ids passed to the dispatcher.
    pub assets: Vec<String>,
}

/// Machine context recorded with the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfMachine {
    /// Operating system.
    pub os: String,
    /// CPU architecture.
    pub arch: String,
    /// Available worker threads.
    pub parallelism: usize,
}

/// Source media metadata from ffprobe or equivalent probing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfMedia {
    /// Absolute source path on disk.
    pub path: String,
    /// Duration in seconds.
    pub duration_s: Option<f64>,
    /// Primary video codec.
    pub video_codec: Option<String>,
    /// Primary audio codec.
    pub audio_codec: Option<String>,
    /// Primary video width.
    pub width: Option<u64>,
    /// Primary video height.
    pub height: Option<u64>,
    /// Average video frame rate string, e.g. `30/1`.
    pub avg_frame_rate: Option<String>,
    /// File size in bytes.
    pub size_bytes: Option<u64>,
    /// Container bit rate in bits per second.
    pub bit_rate: Option<u64>,
}

/// Sidecar-derived output metadata.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct SidecarMetrics {
    /// Sidecar file size.
    pub output_size_bytes: u64,
    /// Generic sampled frame count when present.
    pub frame_count: Option<u64>,
    /// `per_frame` entry count when present.
    pub per_frame_count: Option<u64>,
    /// Audio-energy windows when present.
    pub windows_count: Option<u64>,
    /// Beat count when present.
    pub beats_count: Option<u64>,
    /// Shot count when present.
    pub shots_count: Option<u64>,
    /// Composition/scene region count when present.
    pub regions_count: Option<u64>,
    /// CLIP timestamp count when present.
    pub timestamps_count: Option<u64>,
    /// Sample FPS reported by the indexer when present.
    pub frame_rate_sampled: Option<f64>,
    /// Detection/probe width when present.
    pub detect_width: Option<u64>,
    /// Detection/probe height when present.
    pub detect_height: Option<u64>,
    /// Optional indexer-emitted phase timings in milliseconds.
    pub perf: Option<BTreeMap<String, u128>>,
}

/// One measured indexer/asset pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfPair {
    /// Indexer name.
    pub indexer: String,
    /// Asset id.
    pub asset_id: String,
    /// Outcome: `wrote`, `skipped`, `failed`, or `blocked-by-dep`.
    pub outcome: String,
    /// Failure/block reason when present.
    pub message: Option<String>,
    /// Time spent waiting in the scheduler.
    pub queued_ms: u128,
    /// Child launch + MCP initialize time.
    pub launch_init_ms: u128,
    /// Actual `index_asset` tool runtime. This is the closest current
    /// dispatcher-level proxy for exclusive indexer compute time.
    pub tool_ms: u128,
    /// JSON serialization + sidecar write time.
    pub write_ms: u128,
    /// End-to-end pair wall time from scheduler enqueue. This includes queue.
    pub total_ms: u128,
    /// Peak child RSS in bytes when available.
    pub peak_rss_bytes: Option<u64>,
    /// Sidecar path for wrote/skipped outcomes when known.
    pub sidecar_path: Option<String>,
    /// Output metrics parsed from the sidecar.
    pub sidecar: Option<SidecarMetrics>,
}

/// Aggregate report body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfReportBody {
    /// Total pair count.
    pub pair_count: usize,
    /// Wrote count.
    pub wrote: usize,
    /// Skipped count.
    pub skipped: usize,
    /// Failed count.
    pub failed: usize,
    /// Dependency-blocked count.
    pub dep_skipped: usize,
    /// Slowest pair by total wall time.
    pub max_total_ms: u128,
    /// Slowest actual tool call.
    pub max_tool_ms: u128,
    /// Measured pairs in completion order.
    pub pairs: Vec<PerfPair>,
}

/// Full report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerfReport {
    /// Human label, e.g. `baseline` or `final`.
    pub label: String,
    /// Command context.
    pub command: PerfCommand,
    /// Machine context.
    pub machine: PerfMachine,
    /// Media context.
    pub media: PerfMedia,
    /// Report body.
    pub report: PerfReportBody,
}

/// Build a performance report from dispatcher output.
pub fn build_perf_report(
    label: impl Into<String>,
    command: PerfCommand,
    machine: PerfMachine,
    media: PerfMedia,
    report: &IndexReport,
    project_root: &Path,
) -> PerfReport {
    let pairs: Vec<PerfPair> = report
        .outcomes
        .iter()
        .map(|outcome| perf_pair(outcome, project_root))
        .collect();
    let (skipped, wrote, failed, dep_skipped) = report.counts();
    let max_total_ms = pairs.iter().map(|pair| pair.total_ms).max().unwrap_or(0);
    let max_tool_ms = pairs.iter().map(|pair| pair.tool_ms).max().unwrap_or(0);
    PerfReport {
        label: label.into(),
        command,
        machine,
        media,
        report: PerfReportBody {
            pair_count: pairs.len(),
            wrote,
            skipped,
            failed,
            dep_skipped,
            max_total_ms,
            max_tool_ms,
            pairs,
        },
    }
}

/// Parse common sidecar output metrics. Unknown schemas are tolerated.
pub fn sidecar_metrics(path: &Path) -> Option<SidecarMetrics> {
    let bytes = std::fs::read(path).ok()?;
    let doc: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let data = doc.get("data").unwrap_or(&serde_json::Value::Null);
    Some(SidecarMetrics {
        output_size_bytes: bytes.len() as u64,
        frame_count: u64_field(data, "frame_count"),
        per_frame_count: array_len(data, "per_frame"),
        windows_count: array_len(data, "windows"),
        beats_count: array_len(data, "beats"),
        shots_count: array_len(data, "shots"),
        regions_count: array_len(data, "regions"),
        timestamps_count: array_len(data, "timestamps_s"),
        frame_rate_sampled: f64_field(data, "frame_rate_sampled"),
        detect_width: u64_field(data, "detect_width"),
        detect_height: u64_field(data, "detect_height"),
        perf: perf_map(data),
    })
}

/// Render a compact Markdown report.
pub fn to_markdown(report: &PerfReport) -> String {
    let mut out = String::new();
    out.push_str("# Indexing Performance Report\n\n");
    out.push_str(&format!("- Label: `{}`\n", report.label));
    out.push_str(&format!("- Project: `{}`\n", report.command.project_root));
    out.push_str(&format!("- Source: `{}`\n", report.media.path));
    if let Some(duration) = report.media.duration_s {
        out.push_str(&format!("- Duration: {:.3}s\n", duration));
    }
    if let (Some(width), Some(height)) = (report.media.width, report.media.height) {
        out.push_str(&format!("- Resolution: {}x{}\n", width, height));
    }
    if let Some(codec) = &report.media.video_codec {
        out.push_str(&format!("- Video codec: `{codec}`\n"));
    }
    if let Some(fps) = &report.media.avg_frame_rate {
        out.push_str(&format!("- FPS: `{fps}`\n"));
    }
    if let Some(size) = report.media.size_bytes {
        out.push_str(&format!("- File size: {} bytes\n", size));
    }
    out.push_str(&format!("- Concurrency: {}\n", report.command.concurrency));
    out.push_str(&format!(
        "- Indexers: {}\n\n",
        report.command.included_indexers.join(", ")
    ));
    out.push_str("## Timing Semantics\n\n");
    out.push_str(
        "- `total_ms` is pair wall time from scheduler enqueue and includes `queued_ms`.\n",
    );
    out.push_str("- `tool_ms` is the dispatcher-measured `index_asset` runtime and is the closest current proxy for exclusive indexer compute time.\n");
    out.push_str("- Decode/read/model phases are inside `tool_ms` unless an indexer emits finer-grained sidecar metrics.\n\n");
    out.push_str("## Summary\n\n");
    out.push_str(&format!("- Pairs: {}\n", report.report.pair_count));
    out.push_str(&format!("- Wrote: {}\n", report.report.wrote));
    out.push_str(&format!("- Skipped: {}\n", report.report.skipped));
    out.push_str(&format!("- Failed: {}\n", report.report.failed));
    out.push_str(&format!(
        "- Blocked by dependency: {}\n",
        report.report.dep_skipped
    ));
    out.push_str(&format!(
        "- Slowest total: {} ms\n",
        report.report.max_total_ms
    ));
    out.push_str(&format!(
        "- Slowest tool runtime: {} ms\n\n",
        report.report.max_tool_ms
    ));
    out.push_str("## Pair Timings\n\n");
    out.push_str("| Indexer | Outcome | Total ms | Tool ms | Queue ms | Launch ms | Write ms | Frames | Output bytes | Perf phases |\n");
    out.push_str("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for pair in &report.report.pairs {
        let frames = pair
            .sidecar
            .as_ref()
            .and_then(|m| m.frame_count.or(m.per_frame_count).or(m.timestamps_count))
            .map_or(String::new(), |value| value.to_string());
        let output_size = pair
            .sidecar
            .as_ref()
            .map_or(String::new(), |m| m.output_size_bytes.to_string());
        let perf = pair
            .sidecar
            .as_ref()
            .and_then(|m| m.perf.as_ref())
            .map_or(String::new(), format_perf_phases);
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            pair.indexer,
            pair.outcome,
            pair.total_ms,
            pair.tool_ms,
            pair.queued_ms,
            pair.launch_init_ms,
            pair.write_ms,
            frames,
            output_size,
            perf
        ));
    }
    out
}

fn perf_pair(outcome: &PairOutcome, project_root: &Path) -> PerfPair {
    let telemetry = outcome.telemetry();
    let (indexer, asset, outcome_name, message, path) = match outcome {
        PairOutcome::Wrote {
            indexer,
            asset,
            path,
            ..
        } => (
            indexer.clone(),
            asset.clone(),
            "wrote".to_string(),
            None,
            Some(path.clone()),
        ),
        PairOutcome::Skipped { indexer, asset, .. } => (
            indexer.clone(),
            asset.clone(),
            "skipped".to_string(),
            None,
            existing_sidecar_path(project_root, indexer, asset),
        ),
        PairOutcome::Failed {
            indexer,
            asset,
            message,
            ..
        } => (
            indexer.clone(),
            asset.clone(),
            "failed".to_string(),
            Some(message.clone()),
            None,
        ),
        PairOutcome::SkippedDep {
            indexer,
            asset,
            missing,
            ..
        } => (
            indexer.clone(),
            asset.clone(),
            "blocked-by-dep".to_string(),
            Some(missing.join(", ")),
            None,
        ),
    };
    let sidecar = path.as_deref().and_then(sidecar_metrics);
    PerfPair {
        indexer,
        asset_id: asset.to_string(),
        outcome: outcome_name,
        message,
        queued_ms: millis(telemetry.queued),
        launch_init_ms: millis(telemetry.launch_init),
        tool_ms: millis(telemetry.tool),
        write_ms: millis(telemetry.write),
        total_ms: millis(telemetry.total),
        peak_rss_bytes: telemetry.peak_rss_bytes,
        sidecar_path: path.map(|p| p.display().to_string()),
        sidecar,
    }
}

fn existing_sidecar_path(project_root: &Path, indexer: &str, asset: &AssetId) -> Option<PathBuf> {
    crate::sidecar_path(project_root, indexer, asset)
        .ok()
        .filter(|path| path.exists())
}

fn millis(duration: Duration) -> u128 {
    duration.as_millis()
}

fn array_len(data: &serde_json::Value, key: &str) -> Option<u64> {
    data.get(key)
        .and_then(serde_json::Value::as_array)
        .map(|items| items.len() as u64)
}

fn u64_field(data: &serde_json::Value, key: &str) -> Option<u64> {
    data.get(key).and_then(serde_json::Value::as_u64)
}

fn f64_field(data: &serde_json::Value, key: &str) -> Option<f64> {
    data.get(key).and_then(serde_json::Value::as_f64)
}

fn perf_map(data: &serde_json::Value) -> Option<BTreeMap<String, u128>> {
    let object = data.get("perf")?.as_object()?;
    let mut out = BTreeMap::new();
    for (key, value) in object {
        if let Some(number) = value.as_u64() {
            out.insert(key.clone(), u128::from(number));
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn format_perf_phases(phases: &BTreeMap<String, u128>) -> String {
    phases
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("<br>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PairTelemetry;

    #[test]
    fn report_preserves_total_as_queue_inclusive_wall_time() {
        let dir = tempfile::tempdir().unwrap();
        let asset = AssetId::new("external/a.mp4");
        let report = IndexReport {
            outcomes: vec![PairOutcome::Wrote {
                indexer: "shot".into(),
                asset,
                path: dir.path().join("index/shot/external/a.mp4.json"),
                telemetry: PairTelemetry {
                    queued: Duration::from_millis(148_914),
                    launch_init: Duration::from_millis(503),
                    tool: Duration::from_millis(1_000),
                    write: Duration::ZERO,
                    total: Duration::from_millis(150_495),
                    peak_rss_bytes: None,
                },
            }],
        };
        let perf = build_perf_report(
            "test",
            PerfCommand {
                project_root: dir.path().display().to_string(),
                output_dir: dir.path().display().to_string(),
                concurrency: 2,
                included_indexers: vec!["shot".into()],
                assets: vec!["external/a.mp4".into()],
            },
            PerfMachine {
                os: "test".into(),
                arch: "test".into(),
                parallelism: 1,
            },
            PerfMedia {
                path: "/tmp/a.mp4".into(),
                duration_s: None,
                video_codec: None,
                audio_codec: None,
                width: None,
                height: None,
                avg_frame_rate: None,
                size_bytes: None,
                bit_rate: None,
            },
            &report,
            dir.path(),
        );

        let pair = &perf.report.pairs[0];
        assert_eq!(pair.total_ms, 150_495);
        assert_eq!(pair.queued_ms, 148_914);
        assert_eq!(pair.tool_ms, 1_000);
        assert_eq!(perf.report.max_total_ms, 150_495);
        assert_eq!(perf.report.max_tool_ms, 1_000);
    }

    #[test]
    fn sidecar_metrics_extracts_common_counts_and_output_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clip.json");
        std::fs::write(
            &path,
            r#"{"data":{"frame_count":541,"frame_rate_sampled":0.5,"detect_width":224,"detect_height":224,"timestamps_s":[0.0,2.0]}}"#,
        )
        .unwrap();

        let metrics = sidecar_metrics(&path).unwrap();

        assert_eq!(metrics.frame_count, Some(541));
        assert_eq!(metrics.timestamps_count, Some(2));
        assert_eq!(metrics.frame_rate_sampled, Some(0.5));
        assert_eq!(metrics.detect_width, Some(224));
        assert!(metrics.output_size_bytes > 0);
    }

    #[test]
    fn sidecar_metrics_extracts_perf_phase_timings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("face.json");
        std::fs::write(
            &path,
            r#"{"data":{"frame_count":268,"perf":{"decode_read_ms":1234,"inference_ms":5678,"ignored":"nope"}}}"#,
        )
        .unwrap();

        let metrics = sidecar_metrics(&path).unwrap();
        let perf = metrics.perf.unwrap();

        assert_eq!(perf.get("decode_read_ms"), Some(&1234));
        assert_eq!(perf.get("inference_ms"), Some(&5678));
        assert!(!perf.contains_key("ignored"));
    }
}
