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

/// J/L-cut coverage at speaker-turn boundaries: how much of the dialogue
/// grammar carries an audio lead or trail (the split-edit polish that
/// distinguishes professional cuts from hard butt-cuts).
#[derive(Debug, Clone)]
pub struct SplitEditStats {
    covered: usize,
    total: usize,
}

impl SplitEditStats {
    /// Build from `(boundary_time_secs, has_audio_lead_or_trail)` pairs —
    /// one entry per speaker-turn boundary on the timeline.
    pub fn from_boundaries(boundaries: &[(f64, bool)]) -> Self {
        Self {
            covered: boundaries.iter().filter(|(_, c)| *c).count(),
            total: boundaries.len(),
        }
    }

    /// Fraction of speaker-turn boundaries carrying a J/L-cut (0 when
    /// there are no boundaries).
    pub fn coverage(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.covered as f64 / self.total as f64
        }
    }

    /// Number of speaker-turn boundaries considered.
    pub fn total(&self) -> usize {
        self.total
    }
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
