//! Per-project dismissal memory for editorial findings.
//!
//! When the user clicks "Dismiss" on an EditorialNote, the
//! corresponding pattern lands in `<project>/.awidat/dismissed
//! _patterns.json`. The agent loads this file at session start and
//! the editorial-finding tools (find_dead_air, find_filler_words,
//! find_false_starts) consult it before emitting findings — so a
//! pattern the user already rejected once doesn't re-appear next
//! session.
//!
//! **Bucket coarseness is deliberate.** Dismissing one specific
//! "um at 0:42" doesn't dismiss every "um" — but dismissing
//! "fillers (basic list)" does. The user shouldn't have to dismiss
//! 200 individual fillers to tell the agent they don't care about
//! filler cleanup on this project. The bucket vocabulary keeps the
//! number of distinct dismissal records small + the matching
//! cheap.
//!
//! v1 buckets:
//! - `silence_under_2s` / `silence_2s_to_5s` / `silence_over_5s`
//! - `filler_basic` (default um/uh list) / `filler_aggressive` (with
//!   discourse markers)
//! - `false_start`
//!
//! Future buckets land here as new editorial-finding kinds ship.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Filename inside `.awidat/` that holds the dismissed pattern list.
pub const DISMISSAL_FILE: &str = "dismissed_patterns.json";

/// Coarse bucket id for dismissed editorial-finding patterns. The
/// matcher checks `kind` equality against this enum; it does NOT
/// match by per-finding fields like specific source ranges. Coarse
/// buckets keep the dismissal list small and the loop usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DismissalBucket {
    /// Silence trims for ranges shorter than 2 seconds. Surface
    /// behaviour: dismissing this hides every silence finding whose
    /// duration < 2.0s on this project from now on.
    SilenceUnder2s,
    /// Silence trims 2.0 ≤ duration < 5.0 seconds.
    Silence2sTo5s,
    /// Silence trims 5+ seconds — usually significant dead air.
    /// Dismissing this means the user wants those silences kept
    /// (deliberate pauses, breath beats, intentional rhythm).
    SilenceOver5s,
    /// All filler-word findings using the default basic list
    /// (um/uh/etc). User isn't doing filler-word cleanup on this
    /// project.
    FillerBasic,
    /// Filler findings using the aggressive list (also includes
    /// discourse markers). Dismissing this only suppresses the
    /// aggressive-mode findings; basic-list still surfaces.
    FillerAggressive,
    /// Restart-marker / false-start findings.
    FalseStart,
    /// Stock-broll opportunity findings from `find_broll_opportunities`
    /// (Phase 3). Dismissing this means the user doesn't want b-roll
    /// suggestions on this project — usually because the project is
    /// face-only (the cutaway aesthetic doesn't fit).
    BrollOpportunity,
}

impl DismissalBucket {
    /// Bucket a silence duration into the right enum variant.
    pub fn for_silence(duration_s: f64) -> Self {
        if duration_s < 2.0 {
            Self::SilenceUnder2s
        } else if duration_s < 5.0 {
            Self::Silence2sTo5s
        } else {
            Self::SilenceOver5s
        }
    }

    /// Bucket a filler-word finding by whether the match used the
    /// aggressive list (discourse markers) or the basic list.
    pub fn for_filler(aggressive: bool) -> Self {
        if aggressive {
            Self::FillerAggressive
        } else {
            Self::FillerBasic
        }
    }
}

/// One dismissal record. Carries the bucket the user dismissed plus
/// when. `dismissed_at` is informational; the matcher only looks at
/// `bucket`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DismissalRecord {
    /// Which kind of finding this dismissal covers.
    pub bucket: DismissalBucket,
    /// ISO-8601 timestamp of when the user dismissed.
    pub dismissed_at: String,
}

/// On-disk shape: a versioned wrapper around the records list so
/// future-us can grow the schema (per-asset scopes, expiry, etc)
/// without breaking the v1 file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DismissalFile {
    /// Schema version. Currently `1`.
    pub version: u32,
    /// All dismissed patterns, ordered by `dismissed_at` ascending.
    /// Order doesn't matter to the matcher — `is_dismissed` checks
    /// for *any* matching record.
    pub records: Vec<DismissalRecord>,
}

impl DismissalFile {
    /// Empty file (no dismissals).
    pub fn empty() -> Self {
        Self {
            version: 1,
            records: Vec::new(),
        }
    }

    /// True iff `bucket` has been dismissed on this project. v1: a
    /// single record matches; we don't have per-asset or per-time
    /// scope yet.
    pub fn is_dismissed(&self, bucket: DismissalBucket) -> bool {
        self.records.iter().any(|r| r.bucket == bucket)
    }

    /// Add a new dismissal. No-op if the bucket is already
    /// dismissed (the file should never grow duplicates).
    pub fn add(&mut self, bucket: DismissalBucket) {
        if self.is_dismissed(bucket) {
            return;
        }
        self.records.push(DismissalRecord {
            bucket,
            dismissed_at: chrono::Utc::now().to_rfc3339(),
        });
    }

    /// Remove a previously-dismissed bucket — for the user's "undo
    /// dismissal" affordance. No-op if the bucket isn't on file.
    pub fn remove(&mut self, bucket: DismissalBucket) {
        self.records.retain(|r| r.bucket != bucket);
    }
}

/// Path to the dismissal file for a given project.
pub fn dismissal_file_path(project_root: &Path) -> PathBuf {
    project_root.join(".awidat").join(DISMISSAL_FILE)
}

/// Load dismissed patterns. Returns an empty file when missing or
/// when the file fails to parse — both are recoverable: the user
/// just sees no dismissals applied (which is the safe default).
/// Logs a warning when parse fails so corrupted state surfaces.
pub fn load_dismissals(project_root: &Path) -> DismissalFile {
    let path = dismissal_file_path(project_root);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return DismissalFile::empty(),
    };
    match serde_json::from_slice::<DismissalFile>(&bytes) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "failed to parse dismissed_patterns.json — treating as empty",
            );
            DismissalFile::empty()
        }
    }
}

/// Persist dismissed patterns to disk. Creates the `.awidat/`
/// directory if missing. Returns an error string on any I/O or
/// serialize failure.
pub fn save_dismissals(project_root: &Path, file: &DismissalFile) -> Result<(), String> {
    let path = dismissal_file_path(project_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create .awidat: {e}"))?;
    }
    let json = serde_json::to_vec_pretty(file).map_err(|e| format!("serialize dismissals: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("write dismissals: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_buckets_split_at_thresholds() {
        assert_eq!(
            DismissalBucket::for_silence(0.5),
            DismissalBucket::SilenceUnder2s
        );
        assert_eq!(
            DismissalBucket::for_silence(1.99),
            DismissalBucket::SilenceUnder2s
        );
        assert_eq!(
            DismissalBucket::for_silence(2.0),
            DismissalBucket::Silence2sTo5s
        );
        assert_eq!(
            DismissalBucket::for_silence(4.99),
            DismissalBucket::Silence2sTo5s
        );
        assert_eq!(
            DismissalBucket::for_silence(5.0),
            DismissalBucket::SilenceOver5s
        );
        assert_eq!(
            DismissalBucket::for_silence(60.0),
            DismissalBucket::SilenceOver5s
        );
    }

    #[test]
    fn filler_buckets_split_by_aggressive_flag() {
        assert_eq!(
            DismissalBucket::for_filler(false),
            DismissalBucket::FillerBasic
        );
        assert_eq!(
            DismissalBucket::for_filler(true),
            DismissalBucket::FillerAggressive
        );
    }

    #[test]
    fn add_and_check_dismissal() {
        let mut f = DismissalFile::empty();
        assert!(!f.is_dismissed(DismissalBucket::FillerBasic));
        f.add(DismissalBucket::FillerBasic);
        assert!(f.is_dismissed(DismissalBucket::FillerBasic));
        // Different bucket isn't dismissed.
        assert!(!f.is_dismissed(DismissalBucket::FalseStart));
    }

    #[test]
    fn add_is_idempotent() {
        let mut f = DismissalFile::empty();
        f.add(DismissalBucket::SilenceUnder2s);
        f.add(DismissalBucket::SilenceUnder2s);
        f.add(DismissalBucket::SilenceUnder2s);
        assert_eq!(f.records.len(), 1);
    }

    #[test]
    fn remove_drops_bucket() {
        let mut f = DismissalFile::empty();
        f.add(DismissalBucket::FillerBasic);
        f.add(DismissalBucket::FalseStart);
        f.remove(DismissalBucket::FillerBasic);
        assert!(!f.is_dismissed(DismissalBucket::FillerBasic));
        assert!(f.is_dismissed(DismissalBucket::FalseStart));
    }

    #[test]
    fn load_returns_empty_when_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let f = load_dismissals(dir.path());
        assert!(f.records.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = DismissalFile::empty();
        f.add(DismissalBucket::Silence2sTo5s);
        f.add(DismissalBucket::FillerBasic);
        save_dismissals(dir.path(), &f).unwrap();

        let back = load_dismissals(dir.path());
        assert!(back.is_dismissed(DismissalBucket::Silence2sTo5s));
        assert!(back.is_dismissed(DismissalBucket::FillerBasic));
        assert!(!back.is_dismissed(DismissalBucket::FalseStart));
        assert_eq!(back.version, 1);
    }

    #[test]
    fn load_returns_empty_on_corrupted_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dismissal_file_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not valid json {{{").unwrap();
        // Should fall back to empty rather than panic.
        let f = load_dismissals(dir.path());
        assert!(f.records.is_empty());
    }
}
