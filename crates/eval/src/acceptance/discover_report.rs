//! Human-readable formatting for acceptance discovery reports.

use super::discover::DiscoveryReport;

/// Format a compact human-readable discovery report.
pub fn format_discovery_report(report: &DiscoveryReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Acceptance discovery: {} files scanned under {}\n",
        report.scanned_files, report.source_root
    ));
    for candidate in report.candidates.iter().take(12) {
        out.push_str(&format!(
            "- score {:.1}: {} ({:.1}s, {}x{}, silences: {}) -> {}\n",
            candidate.score,
            candidate.path,
            candidate.duration_s.unwrap_or(0.0),
            candidate.width.unwrap_or(0),
            candidate.height.unwrap_or(0),
            candidate.long_silences.len(),
            candidate.suggested_behavior
        ));
    }
    if !report.transcript_candidates.is_empty() {
        out.push_str("Transcript candidates:\n");
        for candidate in report.transcript_candidates.iter().take(12) {
            let source = candidate
                .source_path
                .as_deref()
                .map(|path| format!(" source={path}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "- score {:.1}: {}{source} {:.3}-{:.3}s {} -> {}\n",
                candidate.score,
                candidate.asset_id,
                candidate.start_s,
                candidate.end_s,
                candidate.snippet,
                candidate.suggested_behavior
            ));
        }
    }
    if !report.errors.is_empty() {
        out.push_str(&format!("Errors: {}\n", report.errors.len()));
    }
    out
}
