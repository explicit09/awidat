//! Sound-pass measurements: loudness stats parsed from ffmpeg's ebur128
//! summary. Measurement runs wherever ffmpeg runs; parsing and gating are
//! deterministic and CI-safe here.

/// Integrated loudness + true peak for one rendered program.
#[derive(Debug, Clone, Copy)]
pub struct LoudnessStats {
    /// EBU R128 integrated loudness, LUFS.
    pub integrated_lufs: f64,
    /// True peak, dBFS.
    pub true_peak_db: f64,
}

/// Parse the trailing summary block of
/// `ffmpeg -af ebur128=peak=true -f null -` stderr output.
pub fn parse_ebur128(text: &str) -> Option<LoudnessStats> {
    let mut integrated = None;
    let mut peak = None;
    let mut in_integrated = false;
    let mut in_peak = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("Integrated loudness") {
            in_integrated = true;
            in_peak = false;
        } else if t.starts_with("True peak") {
            in_peak = true;
            in_integrated = false;
        } else if in_integrated && t.starts_with("I:") {
            integrated = t
                .trim_start_matches("I:")
                .trim()
                .trim_end_matches("LUFS")
                .trim()
                .parse::<f64>()
                .ok();
        } else if in_peak && t.starts_with("Peak:") {
            peak = t
                .trim_start_matches("Peak:")
                .trim()
                .trim_end_matches("dBFS")
                .trim()
                .parse::<f64>()
                .ok();
        }
    }
    Some(LoudnessStats {
        integrated_lufs: integrated?,
        true_peak_db: peak?,
    })
}
