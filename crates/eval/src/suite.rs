//! Golden suite: expected gate verdicts for committed real-program
//! fixtures, each scored against its house profile. Run by the
//! `montage-eval` CLI (CI) and by integration tests.

use std::path::Path;

use serde::Serialize;
use thiserror::Error;

use crate::{HouseProfile, PacingError, PacingStats, ProfileError, gates, load_cut_times};

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
    /// A house profile failed to load.
    #[error("profile {name}: {source}")]
    Profile {
        /// Profile name.
        name: &'static str,
        /// Underlying load error.
        #[source]
        source: ProfileError,
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
    profile: &'static str,
    archetype: &'static str,
    gate: &'static str,
    expect_pass: bool,
}

/// Expected verdicts from the 2026-07-06 editorial study (Finding 11).
const EXPECTATIONS: &[Expectation] = &[
    // SaaS Doomsday: zero-cut static composite — the floor rule's reason.
    Expectation {
        fixture: "own_kaQuWEPIv3I.cuts",
        duration_secs: 1786.0,
        profile: "technologia",
        archetype: "informational",
        gate: "floor",
        expect_pass: false,
    },
    Expectation {
        fixture: "own_kaQuWEPIv3I.cuts",
        duration_secs: 1786.0,
        profile: "technologia",
        archetype: "informational",
        gate: "cold_open",
        expect_pass: false,
    },
    // Founder Journey: no cold open, dead zone at 35:45.
    Expectation {
        fixture: "own_4h7bjBsW_Ag.cuts",
        duration_secs: 3274.0,
        profile: "technologia",
        archetype: "informational",
        gate: "cold_open",
        expect_pass: false,
    },
    Expectation {
        fixture: "own_4h7bjBsW_Ag.cuts",
        duration_secs: 3274.0,
        profile: "technologia",
        archetype: "informational",
        gate: "floor",
        expect_pass: false,
    },
    // DOAC Anti-Aging: blitz cold open, clean floor — the reference pass.
    Expectation {
        fixture: "Jk7RAkFN4vk.cuts",
        duration_secs: 4532.0,
        profile: "doac",
        archetype: "informational",
        gate: "cold_open",
        expect_pass: true,
    },
    Expectation {
        fixture: "Jk7RAkFN4vk.cuts",
        duration_secs: 4532.0,
        profile: "doac",
        archetype: "informational",
        gate: "floor",
        expect_pass: true,
    },
    // DOAC Poirier: emotional archetype still opens with a blitz; its
    // floorless archetype tolerates the zero-cut 5-min window.
    Expectation {
        fixture: "DM2jRSgjy1o.cuts",
        duration_secs: 5602.0,
        profile: "doac",
        archetype: "emotional",
        gate: "cold_open",
        expect_pass: true,
    },
    Expectation {
        fixture: "DM2jRSgjy1o.cuts",
        duration_secs: 5602.0,
        profile: "doac",
        archetype: "emotional",
        gate: "floor",
        expect_pass: true,
    },
];

/// Run every golden expectation against fixtures in `fixtures_dir` (which
/// holds `cuts/` and `profiles/` subdirectories).
pub fn run_golden(fixtures_dir: impl AsRef<Path>) -> Result<Vec<SuiteResult>, SuiteError> {
    let dir = fixtures_dir.as_ref();
    let mut results = Vec::with_capacity(EXPECTATIONS.len());
    for exp in EXPECTATIONS {
        let cuts = load_cut_times(dir.join("cuts").join(exp.fixture)).map_err(|source| {
            SuiteError::Fixture {
                name: exp.fixture,
                source,
            }
        })?;
        let stats = PacingStats::from_cut_times(&cuts, exp.duration_secs).map_err(|source| {
            SuiteError::Stats {
                name: exp.fixture,
                source,
            }
        })?;
        let profile = HouseProfile::from_json_file(
            dir.join("profiles").join(format!("{}.json", exp.profile)),
        )
        .map_err(|source| SuiteError::Profile {
            name: exp.profile,
            source,
        })?;
        let report = match exp.gate {
            "cold_open" => gates::cold_open(&stats, &profile),
            _ => gates::floor(&stats, &profile, exp.archetype),
        };
        results.push(SuiteResult {
            case: format!("{}::{}::{}", exp.fixture, exp.profile, exp.gate),
            ok: report.passed == exp.expect_pass,
            detail: report.detail,
        });
    }
    Ok(results)
}
