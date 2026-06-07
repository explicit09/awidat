use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{IndexReport, PairOutcome, PairTelemetry};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimingBudget {
    pub queued_ms: u64,
    pub launch_init_ms: u64,
    pub tool_ms: u64,
    pub write_ms: u64,
    pub total_ms: u64,
}

impl TimingBudget {
    pub fn for_indexer(indexer: &str) -> Self {
        let tool_ms = match indexer {
            "audio-energy" => 30_000,
            "scenedetect" => 60_000,
            "topic" => 15_000,
            "composition" => 30_000,
            "frame-quality" => 30_000,
            "shot" => 30_000,
            "clip" | "face" | "gaze" => 120_000,
            "motion" | "silence" => 30_000,
            _ => 60_000,
        };
        Self {
            queued_ms: 2_000,
            launch_init_ms: 10_000,
            tool_ms,
            write_ms: 1_000,
            total_ms: tool_ms + 13_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimingMeasurement {
    pub queued_ms: u64,
    pub launch_init_ms: u64,
    pub tool_ms: u64,
    pub write_ms: u64,
    pub total_ms: u64,
    pub peak_rss_bytes: Option<u64>,
}

impl From<&PairTelemetry> for TimingMeasurement {
    fn from(value: &PairTelemetry) -> Self {
        Self {
            queued_ms: duration_ms(value.queued),
            launch_init_ms: duration_ms(value.launch_init),
            tool_ms: duration_ms(value.tool),
            write_ms: duration_ms(value.write),
            total_ms: duration_ms(value.total),
            peak_rss_bytes: value.peak_rss_bytes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BudgetStatus {
    pub queued_ok: bool,
    pub launch_init_ok: bool,
    pub tool_ok: bool,
    pub write_ok: bool,
    pub total_ok: bool,
}

impl BudgetStatus {
    pub fn all_ok(&self) -> bool {
        self.queued_ok && self.launch_init_ok && self.tool_ok && self.write_ok && self.total_ok
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairTimingRow {
    pub indexer: String,
    pub asset_id: String,
    pub outcome: String,
    pub message: Option<String>,
    pub measured: TimingMeasurement,
    pub budget: TimingBudget,
    pub status: BudgetStatus,
}

impl PairTimingRow {
    pub fn from_outcome(outcome: &PairOutcome) -> Self {
        let (indexer, asset, outcome_label, message, telemetry) = match outcome {
            PairOutcome::Skipped {
                indexer,
                asset,
                telemetry,
            } => (indexer.as_str(), asset, "skipped", None, telemetry),
            PairOutcome::Wrote {
                indexer,
                asset,
                telemetry,
                ..
            } => (indexer.as_str(), asset, "wrote", None, telemetry),
            PairOutcome::Failed {
                indexer,
                asset,
                message,
                telemetry,
            } => (
                indexer.as_str(),
                asset,
                "failed",
                Some(message.clone()),
                telemetry,
            ),
            PairOutcome::SkippedDep {
                indexer,
                asset,
                missing,
                telemetry,
            } => (
                indexer.as_str(),
                asset,
                "blocked-by-dep",
                Some(format!("missing {}", missing.join(", "))),
                telemetry,
            ),
        };
        let measured = TimingMeasurement::from(telemetry);
        let budget = TimingBudget::for_indexer(indexer);
        let status = BudgetStatus {
            queued_ok: measured.queued_ms <= budget.queued_ms,
            launch_init_ok: measured.launch_init_ms <= budget.launch_init_ms,
            tool_ok: measured.tool_ms <= budget.tool_ms,
            write_ok: measured.write_ms <= budget.write_ms,
            total_ok: measured.total_ms <= budget.total_ms,
        };
        Self {
            indexer: indexer.to_string(),
            asset_id: asset.to_string(),
            outcome: outcome_label.to_string(),
            message,
            measured,
            budget,
            status,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PerformanceSummary {
    pub pair_count: usize,
    pub wrote: usize,
    pub skipped: usize,
    pub failed: usize,
    pub dep_skipped: usize,
    pub budget_violations: usize,
    pub max_total_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PerformanceReport {
    pub summary: PerformanceSummary,
    pub pairs: Vec<PairTimingRow>,
}

impl PerformanceReport {
    pub fn from_index_report(report: &IndexReport) -> Self {
        let pairs = report
            .outcomes
            .iter()
            .map(PairTimingRow::from_outcome)
            .collect::<Vec<_>>();
        let (skipped, wrote, failed, dep_skipped) = report.counts();
        let budget_violations = pairs.iter().filter(|row| !row.status.all_ok()).count();
        let max_total_ms = pairs
            .iter()
            .map(|row| row.measured.total_ms)
            .max()
            .unwrap_or(0);
        Self {
            summary: PerformanceSummary {
                pair_count: pairs.len(),
                wrote,
                skipped,
                failed,
                dep_skipped,
                budget_violations,
                max_total_ms,
            },
            pairs,
        }
    }
}

pub fn duration_ms(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    if millis > u64::MAX as u128 {
        u64::MAX
    } else {
        millis as u64
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use awidat_proto::index::AssetId;

    use super::*;

    fn telemetry(total_ms: u64, tool_ms: u64) -> PairTelemetry {
        PairTelemetry {
            queued: Duration::from_millis(12),
            launch_init: Duration::from_millis(34),
            tool: Duration::from_millis(tool_ms),
            write: Duration::from_millis(56),
            total: Duration::from_millis(total_ms),
            peak_rss_bytes: Some(1234),
        }
    }

    #[test]
    fn perf_report_converts_pair_telemetry_to_milliseconds() {
        let report = IndexReport {
            outcomes: vec![PairOutcome::Wrote {
                indexer: "audio-energy".into(),
                asset: AssetId::new("raw/a.mp4"),
                path: PathBuf::from("/tmp/a.json"),
                telemetry: telemetry(321, 123),
            }],
        };

        let perf = PerformanceReport::from_index_report(&report);

        assert_eq!(perf.summary.pair_count, 1);
        assert_eq!(perf.summary.wrote, 1);
        assert_eq!(perf.summary.budget_violations, 0);
        assert_eq!(perf.summary.max_total_ms, 321);
        let row = &perf.pairs[0];
        assert_eq!(row.indexer, "audio-energy");
        assert_eq!(row.asset_id, "raw/a.mp4");
        assert_eq!(row.outcome, "wrote");
        assert_eq!(row.measured.queued_ms, 12);
        assert_eq!(row.measured.launch_init_ms, 34);
        assert_eq!(row.measured.tool_ms, 123);
        assert_eq!(row.measured.write_ms, 56);
        assert_eq!(row.measured.total_ms, 321);
        assert_eq!(row.measured.peak_rss_bytes, Some(1234));
        assert!(row.status.all_ok());
    }

    #[test]
    fn perf_report_flags_target_violations() {
        let report = IndexReport {
            outcomes: vec![PairOutcome::Skipped {
                indexer: "topic".into(),
                asset: AssetId::new("raw/b.mp4"),
                telemetry: telemetry(29_000, 16_000),
            }],
        };

        let perf = PerformanceReport::from_index_report(&report);

        assert_eq!(perf.summary.skipped, 1);
        assert_eq!(perf.summary.budget_violations, 1);
        let row = &perf.pairs[0];
        assert_eq!(row.outcome, "skipped");
        assert!(!row.status.tool_ok);
        assert!(!row.status.total_ok);
        assert!(!row.status.all_ok());
    }
}
