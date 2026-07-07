//! Reading cut-boundary sidecar files (one cut time in seconds per line).

use std::path::Path;

use thiserror::Error;

/// Errors reading a cut-times sidecar.
#[derive(Debug, Error)]
pub enum CutsIoError {
    /// Underlying file read failed.
    #[error("reading cuts file: {0}")]
    Io(#[from] std::io::Error),
    /// A line was not parseable as a float.
    #[error("line {line}: not a cut time: {text:?}")]
    BadLine {
        /// 1-indexed line number.
        line: usize,
        /// Offending line content.
        text: String,
    },
}

/// Load cut boundary times from a sidecar file: one `f64` seconds value per
/// line, blank lines ignored.
pub fn load_cut_times(path: impl AsRef<Path>) -> Result<Vec<f64>, CutsIoError> {
    let content = std::fs::read_to_string(path)?;
    let mut cuts = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let t: f64 = line.parse().map_err(|_| CutsIoError::BadLine {
            line: i + 1,
            text: line.to_string(),
        })?;
        cuts.push(t);
    }
    Ok(cuts)
}
