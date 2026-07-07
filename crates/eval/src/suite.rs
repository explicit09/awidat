//! Golden suite: expected gate verdicts for committed real-program
//! fixtures. Run by the `montage-eval` CLI (CI) and by integration tests.

use std::path::Path;

use serde::Serialize;
use thiserror::Error;

use crate::{PacingError, PacingStats, gates, load_cut_times};

/// Errors running the golden suite.
#[derive(Debug, Error)]
pub enum SuiteError {
    /// A fixture file failed to load.
    #[error("fixture {name}: {source}")]
    Fixture {
        /// Fixture file name.
        name: &'static str,
        /// Underlying load error.
        #[source]
        source: crate::CutsIoError,
    },
    /// Stats construction rejected fixture data.
    #[error("fixture {name}: {source}")]
    Stats {
        /// Fixture file name.
        name: &'static str,
        /// Underlying pacing error.
        #[source]
        source: PacingError,
    },
}

/// One golden case outcome.
#[derive(Debug, Clone, Serialize)]
pub struct SuiteResult {
    /// Case identifier.
    pub case: String,
    /// Whether the measured verdict matched the expectation.
    pub ok: bool,
    /// Explanation of the verdict.
    pub detail: String,
}

struct Expectation {
    fixture: &'static str,
    duration_secs: f64,
    gate: &'static str,
    expect_pass: bool,
}

/// Expected verdicts from the 2026-07-06 editorial study (Finding 11).
const EXPECTATIONS: &[Expectation] = &[
    // SaaS Doomsday: zero-cut static composite — the floor rule's reason.
    Expectation {
        fixture: "own_kaQuWEPIv3I.cuts",
        duration_secs: 1786.0,
        gate: "saas_floor",
        expect_pass: false,
    },
    Expectation {
        fixture: "own_kaQuWEPIv3I.cuts",
        duration_secs: 1786.0,
        gate: "cold_open",
        expect_pass: false,
    },
    // Founder Journey: no cold open (peak minute 42), dead zone at 35:45.
    Expectation {
        fixture: "own_4h7bjBsW_Ag.cuts",
        duration_secs: 3274.0,
        gate: "cold_open",
        expect_pass: false,
    },
    Expectation {
        fixture: "own_4h7bjBsW_Ag.cuts",
        duration_secs: 3274.0,
        gate: "saas_floor",
        expect_pass: false,
    },
    // DOAC Anti-Aging: blitz cold open, clean floor — the reference pass.
    Expectation {
        fixture: "Jk7RAkFN4vk.cuts",
        duration_secs: 4532.0,
        gate: "cold_open",
        expect_pass: true,
    },
    Expectation {
        fixture: "Jk7RAkFN4vk.cuts",
        duration_secs: 4532.0,
        gate: "saas_floor",
        expect_pass: true,
    },
    // DOAC Poirier: emotional archetype still opens with a blitz.
    Expectation {
        fixture: "DM2jRSgjy1o.cuts",
        duration_secs: 5602.0,
        gate: "cold_open",
        expect_pass: true,
    },
];

/// Run every golden expectation against fixtures in `fixtures_dir`.
pub fn run_golden(fixtures_dir: impl AsRef<Path>) -> Result<Vec<SuiteResult>, SuiteError> {
    let dir = fixtures_dir.as_ref();
    let mut results = Vec::with_capacity(EXPECTATIONS.len());
    for exp in EXPECTATIONS {
        let cuts = load_cut_times(dir.join(exp.fixture)).map_err(|source| SuiteError::Fixture {
            name: exp.fixture,
            source,
        })?;
        let stats = PacingStats::from_cut_times(&cuts, exp.duration_secs).map_err(|source| {
            SuiteError::Stats {
                name: exp.fixture,
                source,
            }
        })?;
        let report = match exp.gate {
            "cold_open" => gates::cold_open(&stats),
            _ => gates::saas_floor(&stats),
        };
        results.push(SuiteResult {
            case: format!("{}::{}", exp.fixture, exp.gate),
            ok: report.passed == exp.expect_pass,
            detail: report.detail,
        });
    }
    Ok(results)
}
