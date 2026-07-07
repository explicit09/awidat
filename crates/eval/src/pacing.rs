//! Pacing statistics derived from cut boundary times.

use thiserror::Error;

/// Errors constructing pacing statistics.
#[derive(Debug, Error)]
pub enum PacingError {
    /// Program duration must be positive to compute rates.
    #[error("program duration must be positive, got {0}")]
    NonPositiveDuration(f64),
}

/// Cut-boundary statistics for one program.
#[derive(Debug, Clone)]
pub struct PacingStats {
    /// Cut boundary times in seconds, sorted ascending.
    cuts: Vec<f64>,
    duration_secs: f64,
}

impl PacingStats {
    /// Build stats from cut boundary times (seconds) and program duration.
    pub fn from_cut_times(cuts: &[f64], duration_secs: f64) -> Result<Self, PacingError> {
        if duration_secs <= 0.0 {
            return Err(PacingError::NonPositiveDuration(duration_secs));
        }
        let mut cuts = cuts.to_vec();
        cuts.sort_by(f64::total_cmp);
        Ok(Self {
            cuts,
            duration_secs,
        })
    }

    /// Program duration in seconds.
    pub fn duration_secs(&self) -> f64 {
        self.duration_secs
    }

    /// Overall cut rate for the program, in cuts per minute.
    pub fn cuts_per_min(&self) -> f64 {
        self.cuts.len() as f64 / (self.duration_secs / 60.0)
    }

    /// Cut rate within `[start, end)` seconds, in cuts per minute.
    pub fn rate_in_window(&self, start: f64, end: f64) -> f64 {
        let span = end - start;
        if span <= 0.0 {
            return 0.0;
        }
        let n = self.cuts.iter().filter(|&&t| t >= start && t < end).count();
        n as f64 / (span / 60.0)
    }

    /// Minimum cut rate over any `window_secs` stretch (1s scan granularity).
    ///
    /// Programs shorter than the window are rated over their full length.
    pub fn sparsest_window_rate(&self, window_secs: f64) -> f64 {
        if self.duration_secs <= window_secs {
            return self.cuts_per_min();
        }
        let last_start = self.duration_secs - window_secs;
        let mut min_rate = f64::INFINITY;
        let mut start = 0.0;
        while start <= last_start {
            min_rate = min_rate.min(self.rate_in_window(start, start + window_secs));
            start += 1.0;
        }
        min_rate
    }

    /// Index of the minute with the most cuts (first on ties); `None` when
    /// the program has no cuts.
    pub fn peak_minute(&self) -> Option<usize> {
        if self.cuts.is_empty() {
            return None;
        }
        let minutes = (self.duration_secs / 60.0).ceil() as usize;
        let mut counts = vec![0usize; minutes.max(1)];
        for &t in &self.cuts {
            let idx = ((t / 60.0) as usize).min(counts.len() - 1);
            counts[idx] += 1;
        }
        counts
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(&a.0)))
            .map(|(i, _)| i)
    }
}
