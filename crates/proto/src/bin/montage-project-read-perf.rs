#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

//! Manual macOS/APFS A/B evidence harness for `Project::read`.
//!
//! The controller never measures itself. It creates deterministic projects,
//! then starts a fresh supplied baseline or candidate helper for each sample.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use chrono::{DateTime, SecondsFormat, Utc};
use montage_proto::ProtoError;
use montage_proto::index::{AssetId, IndexerEntry, Manifest};
use montage_proto::otio::{Clip, Stack, StackChild, Timeline, Track, TrackChild, TrackKind};
use montage_proto::project::{EditPlanItem, Project, files};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const PROTOCOL: &str = "montage-project-read-perf-v1";
const DEFAULT_CLIP_COUNTS: &[usize] = &[100, 1_000, 5_000];
const DEFAULT_WARMUPS: usize = 3;
const DEFAULT_SAMPLES: usize = 15;
const MIB: f64 = 1024.0 * 1024.0;
const TIME_PATH: &str = "/usr/bin/time";
const HELPER_LC_ALL: &str = "C";
const HELPER_TZ: &str = "UTC";
const BUILD_CARGO_PROFILE: &str = env!("MONTAGE_PROJECT_READ_CARGO_PROFILE");

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let result = if raw.first().is_some_and(|arg| arg == "--helper") {
        helper_main(&raw[1..])
    } else if raw.first().is_some_and(|arg| arg == "--verify-reports") {
        verify_reports_main(&raw)
    } else if raw.first().is_some_and(|arg| arg == "--help") {
        println!("{}", usage());
        Ok(())
    } else {
        controller_main(&raw)
    };
    if let Err(error) = result {
        eprintln!("montage-project-read-perf: {error}");
        std::process::exit(1);
    }
}

fn usage() -> &'static str {
    "usage: montage-project-read-perf --baseline <binary> --candidate <binary> \\
  --baseline-source <repo> --candidate-source <repo> [--work-dir <dir>] \\
  [--report-dir <dir>] [--label <label>] [--clips 100,1000,5000] \\
  [--warmups 3] [--samples 15] [--smoke] [--allow-dirty-source]\n\
montage-project-read-perf --verify-reports <first-report.json> <second-report.json>"
}

fn controller_main(raw: &[String]) -> Result<(), String> {
    let args = ControllerArgs::parse(raw)?;
    let baseline_executable = canonical_executable(&args.baseline, "baseline")?;
    let candidate_executable = canonical_executable(&args.candidate, "candidate")?;
    let controller_executable = std::env::current_exe()
        .map_err(|error| format!("locate controller executable: {error}"))?;
    let controller_executable = canonical_executable(&controller_executable, "controller")?;
    require_macos()?;
    if !args.smoke
        && (args.clip_counts.as_slice() != DEFAULT_CLIP_COUNTS
            || args.warmups != DEFAULT_WARMUPS
            || args.samples != DEFAULT_SAMPLES)
    {
        return Err(
            "non-default clips, warmups, or samples require --smoke and cannot qualify a change"
                .into(),
        );
    }

    fs::create_dir_all(&args.work_dir)
        .map_err(|error| format!("create {}: {error}", args.work_dir.display()))?;
    fs::create_dir_all(&args.report_dir)
        .map_err(|error| format!("create {}: {error}", args.report_dir.display()))?;
    let work_filesystem = require_apfs(&args.work_dir)?;
    let report_filesystem = require_apfs(&args.report_dir)?;
    let run_dir = create_unique_dir(&args.work_dir, "project-read-run")?;

    let controller = binary_provenance(controller_executable, compiled_identity())?;
    let baseline_identity = read_identity(&baseline_executable, &run_dir, "baseline")?;
    let candidate_identity = read_identity(&candidate_executable, &run_dir, "candidate")?;
    let baseline = binary_provenance(baseline_executable, baseline_identity)?;
    let candidate = binary_provenance(candidate_executable, candidate_identity)?;
    let baseline_source = inspect_source_tree(&args.baseline_source, args.allow_dirty_source)?;
    let candidate_source = inspect_source_tree(&args.candidate_source, args.allow_dirty_source)?;

    verify_binary_matches_source("baseline", &baseline, &baseline_source)?;
    verify_binary_matches_source("candidate", &candidate, &candidate_source)?;
    let source_snapshot = verify_source_snapshots(&args, &baseline_source, &candidate_source)?;
    verify_comparison(&args, &controller, &baseline, &candidate)?;

    let contracts = write_contract_fixtures(&run_dir.join("contracts"))?;
    let baseline_contracts = run_contracts(&baseline.path, &contracts, &run_dir, "baseline")?;
    let candidate_contracts = run_contracts(&candidate.path, &contracts, &run_dir, "candidate")?;
    validate_contracts("baseline", &baseline_contracts)?;
    validate_contracts("candidate", &candidate_contracts)?;
    if baseline_contracts != candidate_contracts {
        return Err("baseline and candidate contract witnesses differ".into());
    }
    assert_contract_inputs_unchanged(&contracts)?;

    let mut sequence = 0_u64;
    let mut runs = Vec::with_capacity(args.clip_counts.len());
    for &clip_count in &args.clip_counts {
        let fixture = write_fixture(
            &run_dir.join("fixtures").join(format!("{clip_count}-clips")),
            clip_count,
        )?;
        let baseline_full_witness = run_contract_helper(
            &baseline.path,
            &fixture.root,
            &run_dir,
            "baseline",
            &format!("fixture-{clip_count}"),
        )?;
        let candidate_full_witness = run_contract_helper(
            &candidate.path,
            &fixture.root,
            &run_dir,
            "candidate",
            &format!("fixture-{clip_count}"),
        )?;
        validate_full_fixture_witness("baseline", clip_count, &baseline_full_witness)?;
        validate_full_fixture_witness("candidate", clip_count, &candidate_full_witness)?;
        if baseline_full_witness != candidate_full_witness {
            return Err(format!(
                "baseline and candidate full typed witnesses differ for {clip_count} clips"
            ));
        }
        let run = run_fixture(
            &args,
            &fixture,
            &run_dir,
            &mut sequence,
            &baseline,
            &candidate,
        )?;
        let final_fingerprint = fingerprint_project_inputs(&fixture.root)?;
        if fixture.fingerprint != final_fingerprint {
            return Err(format!(
                "project-read input changed during the {}-clip run",
                fixture.clip_count
            ));
        }
        runs.push(FixtureRun {
            fixture,
            final_input_fingerprint: final_fingerprint,
            baseline: run.baseline,
            candidate: run.candidate,
            compact_witness: run.compact_witness,
            full_integrity: FullFixtureIntegrity {
                baseline: baseline_full_witness,
                candidate: candidate_full_witness,
            },
        });
    }

    let acceptance = evaluate_acceptance(&runs, args.smoke);
    let generated_at = Utc::now();
    let report_id = unique_nonce();
    let session_id = unique_nonce();
    let report = BenchmarkReport {
        schema_version: 1,
        protocol: PROTOCOL.into(),
        report_id: report_id.clone(),
        session_id,
        generated_at_utc: generated_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        configuration: Configuration {
            label: args.label.clone(),
            clip_counts: args.clip_counts.clone(),
            warmups: args.warmups,
            samples: args.samples,
            smoke: args.smoke,
            allow_dirty_source: args.allow_dirty_source,
            work_dir: args.work_dir.display().to_string(),
            report_dir: args.report_dir.display().to_string(),
        },
        provenance: Provenance {
            controller,
            baseline,
            candidate,
            baseline_source,
            candidate_source,
            source_snapshot,
            helper_environment: HelperEnvironment {
                lc_all: HELPER_LC_ALL.into(),
                tz: HELPER_TZ.into(),
            },
            tools: tool_provenance()?,
            machine: Machine {
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
                parallelism: std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(1),
                work_filesystem,
                report_filesystem,
            },
        },
        contracts: ContractReport {
            fixtures: contracts.fingerprints(),
            baseline: baseline_contracts,
            candidate: candidate_contracts,
        },
        fixtures: runs,
        acceptance,
    };
    let report_path = args.report_dir.join(format!(
        "{}-{}-{}-project-read-ab.json",
        args.label,
        generated_at.format("%Y%m%dT%H%M%S"),
        report_id
    ));
    let json =
        serde_json::to_vec_pretty(&report).map_err(|error| format!("serialize report: {error}"))?;
    write_atomically_new(&report_path, &json)?;
    println!("{}", report_path.display());

    if !args.smoke && !report.acceptance.single_report_gate_passed {
        return Err(format!(
            "acceptance gate did not qualify this report; evidence: {}",
            report_path.display()
        ));
    }
    Ok(())
}

fn require_macos() -> Result<(), String> {
    if std::env::consts::OS != "macos" {
        return Err(
            "this manual controller requires macOS because it records /usr/bin/time -l".into(),
        );
    }
    Ok(())
}

fn canonical_executable(path: &Path, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!(
            "{label} binary must be an absolute executable path; refusing to PATH-search {}",
            path.display()
        ));
    }
    let path = fs::canonicalize(path)
        .map_err(|error| format!("canonicalize {label} binary {}: {error}", path.display()))?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("metadata {label} binary {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "{label} binary is not a regular file: {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!(
            "{label} binary is not executable: {}",
            path.display()
        ));
    }
    Ok(path)
}

#[derive(Debug, Clone)]
struct ControllerArgs {
    baseline: PathBuf,
    candidate: PathBuf,
    baseline_source: PathBuf,
    candidate_source: PathBuf,
    work_dir: PathBuf,
    report_dir: PathBuf,
    label: String,
    clip_counts: Vec<usize>,
    warmups: usize,
    samples: usize,
    smoke: bool,
    allow_dirty_source: bool,
}

impl ControllerArgs {
    fn parse(raw: &[String]) -> Result<Self, String> {
        let mut baseline = None;
        let mut candidate = None;
        let mut baseline_source = None;
        let mut candidate_source = None;
        let mut work_dir = std::env::var_os("MONTAGE_PROJECT_READ_WORK_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("montage-project-read-perf"));
        let mut report_dir = std::env::var_os("MONTAGE_PROJECT_READ_REPORT_DIR").map(PathBuf::from);
        let mut label = "project-read".to_string();
        let mut clip_counts = DEFAULT_CLIP_COUNTS.to_vec();
        let mut warmups = DEFAULT_WARMUPS;
        let mut samples = DEFAULT_SAMPLES;
        let mut smoke = false;
        let mut allow_dirty_source = false;
        let mut index = 0;
        while index < raw.len() {
            let flag = &raw[index];
            index += 1;
            let value = |name: &str, index: &mut usize| {
                let value = raw
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| format!("{name} requires a value"))?;
                *index += 1;
                Ok::<_, String>(value)
            };
            match flag.as_str() {
                "--baseline" => baseline = Some(PathBuf::from(value("--baseline", &mut index)?)),
                "--candidate" => candidate = Some(PathBuf::from(value("--candidate", &mut index)?)),
                "--baseline-source" => {
                    baseline_source = Some(PathBuf::from(value("--baseline-source", &mut index)?))
                }
                "--candidate-source" => {
                    candidate_source = Some(PathBuf::from(value("--candidate-source", &mut index)?))
                }
                "--work-dir" => work_dir = PathBuf::from(value("--work-dir", &mut index)?),
                "--report-dir" => {
                    report_dir = Some(PathBuf::from(value("--report-dir", &mut index)?))
                }
                "--label" => label = value("--label", &mut index)?,
                "--clips" => clip_counts = parse_clip_counts(&value("--clips", &mut index)?)?,
                "--warmups" => {
                    warmups = parse_positive("--warmups", &value("--warmups", &mut index)?)?
                }
                "--samples" => {
                    samples = parse_positive("--samples", &value("--samples", &mut index)?)?
                }
                "--smoke" => smoke = true,
                "--allow-dirty-source" => allow_dirty_source = true,
                "--help" => return Err(usage().into()),
                _ => return Err(format!("unknown argument: {flag}")),
            }
        }
        let baseline = baseline.ok_or_else(|| "--baseline is required".to_string())?;
        let candidate = candidate.ok_or_else(|| "--candidate is required".to_string())?;
        let baseline_source =
            baseline_source.ok_or_else(|| "--baseline-source is required".to_string())?;
        let candidate_source =
            candidate_source.ok_or_else(|| "--candidate-source is required".to_string())?;
        if allow_dirty_source && !smoke {
            return Err("--allow-dirty-source is permitted only with --smoke".into());
        }
        if !is_safe_label(&label) {
            return Err("--label must be a nonempty filesystem-safe token ([A-Za-z0-9._-])".into());
        }
        Ok(Self {
            baseline,
            candidate,
            baseline_source,
            candidate_source,
            report_dir: report_dir.unwrap_or_else(|| work_dir.join("reports")),
            work_dir,
            label,
            clip_counts,
            warmups,
            samples,
            smoke,
            allow_dirty_source,
        })
    }
}

#[derive(Debug, Clone)]
struct VerifyReportsArgs {
    first: PathBuf,
    second: PathBuf,
}

impl VerifyReportsArgs {
    fn parse(raw: &[String]) -> Result<Self, String> {
        if raw
            .first()
            .is_none_or(|argument| argument != "--verify-reports")
            || raw.len() != 3
        {
            return Err("--verify-reports requires exactly two report paths".into());
        }
        let first = PathBuf::from(&raw[1]);
        let second = PathBuf::from(&raw[2]);
        if !first.is_absolute() || !second.is_absolute() {
            return Err("--verify-reports requires two absolute report paths".into());
        }
        Ok(Self { first, second })
    }
}

fn verify_reports_main(raw: &[String]) -> Result<(), String> {
    let args = VerifyReportsArgs::parse(raw)?;
    let first_path = canonical_regular_file(&args.first, "first report")?;
    let second_path = canonical_regular_file(&args.second, "second report")?;
    let first: VerificationReport = read_json_file(&first_path)?;
    let second: VerificationReport = read_json_file(&second_path)?;
    validate_report_for_verification(&first, "first")?;
    validate_report_for_verification(&second, "second")?;
    let distinct_ids: BTreeSet<_> = [
        &first.report_id,
        &first.session_id,
        &second.report_id,
        &second.session_id,
    ]
    .into_iter()
    .collect();
    if distinct_ids.len() != 4 {
        return Err("the two reports must have distinct report IDs and session IDs".into());
    }
    let first_evidence = report_evidence_hash(&first)?;
    let second_evidence = report_evidence_hash(&second)?;
    if first.generated_at_utc == second.generated_at_utc || first_evidence == second_evidence {
        return Err(
            "the two reports must have distinct generation times and raw fixture evidence".into(),
        );
    }
    let first_methodology = report_methodology_hash(&first)?;
    let second_methodology = report_methodology_hash(&second)?;
    if first_methodology != second_methodology {
        return Err(
            "the two reports do not have matching binary, source, and methodology provenance"
                .into(),
        );
    }
    let result = ReportVerificationResult {
        schema_version: 1,
        protocol: PROTOCOL.into(),
        program_acceptance: true,
        report_ids: vec![first.report_id, second.report_id],
        session_ids: vec![first.session_id, second.session_id],
        evidence_sha256: vec![first_evidence, second_evidence],
        methodology_sha256: first_methodology,
    };
    let text = serde_json::to_string(&result)
        .map_err(|error| format!("serialize report verification: {error}"))?;
    println!("{text}");
    Ok(())
}

fn canonical_regular_file(path: &Path, label: &str) -> Result<PathBuf, String> {
    let path = fs::canonicalize(path)
        .map_err(|error| format!("canonicalize {label} {}: {error}", path.display()))?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("metadata {label} {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{label} is not a regular file: {}", path.display()));
    }
    Ok(path)
}

#[derive(Debug, Serialize, Deserialize)]
struct VerificationReport {
    schema_version: u32,
    protocol: String,
    report_id: String,
    session_id: String,
    generated_at_utc: String,
    configuration: VerificationConfiguration,
    provenance: VerificationProvenance,
    contracts: ContractReport,
    fixtures: Vec<FixtureRun>,
    acceptance: VerificationAcceptance,
}

#[derive(Debug, Serialize, Deserialize)]
struct VerificationConfiguration {
    clip_counts: Vec<usize>,
    warmups: usize,
    samples: usize,
    smoke: bool,
    allow_dirty_source: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct VerificationProvenance {
    controller: VerificationBinary,
    baseline: VerificationBinary,
    candidate: VerificationBinary,
    baseline_source: VerificationSourceTree,
    candidate_source: VerificationSourceTree,
    source_snapshot: VerificationSourceSnapshot,
    helper_environment: VerificationHelperEnvironment,
    tools: VerificationToolProvenance,
    machine: Machine,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VerificationBinary {
    path: PathBuf,
    sha256: String,
    identity: HelperIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VerificationSourceTree {
    root: PathBuf,
    git_head: String,
    tracked_tree_sha256: String,
    full_repository_dirty: bool,
    full_git_status_sha256: String,
    relevant_dirty_paths: Vec<String>,
    source_files: Vec<VerificationSourceFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VerificationSourceFile {
    relative_path: String,
    sha256: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct VerificationSourceSnapshot {
    differing_tracked_paths: Vec<String>,
    qualifying_policy: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct VerificationHelperEnvironment {
    lc_all: String,
    tz: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct VerificationToolProvenance {
    time_path: String,
    time_sha256: String,
    rustc_version_verbose: String,
    cargo_version: String,
    macos_version: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct VerificationAcceptance {
    single_report_gate_passed: bool,
    program_acceptance: bool,
}

#[derive(Serialize)]
struct ReportVerificationResult {
    schema_version: u32,
    protocol: String,
    program_acceptance: bool,
    report_ids: Vec<String>,
    session_ids: Vec<String>,
    evidence_sha256: Vec<String>,
    methodology_sha256: String,
}

fn validate_report_for_verification(
    report: &VerificationReport,
    label: &str,
) -> Result<(), String> {
    if report.schema_version != 1 || report.protocol != PROTOCOL {
        return Err(format!(
            "{label} report has an unsupported report schema or protocol"
        ));
    }
    if report.report_id.is_empty()
        || report.session_id.is_empty()
        || report.generated_at_utc.is_empty()
    {
        return Err(format!(
            "{label} report is missing a report ID, session ID, or generation time"
        ));
    }
    DateTime::parse_from_rfc3339(&report.generated_at_utc)
        .map_err(|error| format!("{label} report has an invalid generation time: {error}"))?;
    if report.configuration.smoke
        || report.configuration.allow_dirty_source
        || report.configuration.clip_counts.as_slice() != DEFAULT_CLIP_COUNTS
        || report.configuration.warmups != DEFAULT_WARMUPS
        || report.configuration.samples != DEFAULT_SAMPLES
    {
        return Err(format!(
            "{label} report is not a full clean default-methodology run"
        ));
    }
    if !report.acceptance.single_report_gate_passed || report.acceptance.program_acceptance {
        return Err(format!(
            "{label} report does not contain a passing single-report gate"
        ));
    }
    if report.provenance.baseline_source.full_repository_dirty
        || report.provenance.candidate_source.full_repository_dirty
    {
        return Err(format!("{label} report used a dirty source snapshot"));
    }
    validate_source_snapshot_diff_paths(
        &report.provenance.source_snapshot.differing_tracked_paths,
        false,
    )
    .map_err(|error| format!("{label} report {error}"))?;
    if report.provenance.helper_environment.lc_all != HELPER_LC_ALL
        || report.provenance.helper_environment.tz != HELPER_TZ
    {
        return Err(format!(
            "{label} report did not pin LC_ALL=C and TZ=UTC for helpers"
        ));
    }
    if report.provenance.controller.path != report.provenance.candidate.path
        || report.provenance.controller.sha256 != report.provenance.candidate.sha256
        || report.provenance.controller.identity != report.provenance.candidate.identity
    {
        return Err(format!(
            "{label} report controller is not the candidate binary"
        ));
    }
    if report.provenance.baseline.sha256 == report.provenance.candidate.sha256 {
        return Err(format!(
            "{label} report compares identical baseline and candidate binaries"
        ));
    }
    if report.provenance.baseline.identity.project_source_sha256
        == report.provenance.candidate.identity.project_source_sha256
        || report.provenance.baseline_source.git_head == report.provenance.candidate_source.git_head
        || report.provenance.baseline_source.tracked_tree_sha256
            == report.provenance.candidate_source.tracked_tree_sha256
    {
        return Err(format!(
            "{label} report does not contain distinct baseline and candidate project sources"
        ));
    }
    if report.fixtures.len() != DEFAULT_CLIP_COUNTS.len() {
        return Err(format!(
            "{label} report does not contain raw evidence for every required fixture"
        ));
    }
    verify_helper_identity_pair(
        &report.provenance.baseline.identity,
        &report.provenance.candidate.identity,
    )
    .map_err(|error| format!("{label} report {error}"))?;
    if report.provenance.baseline.identity.profile != "release"
        || report.provenance.candidate.identity.profile != "release"
    {
        return Err(format!(
            "{label} report does not use literal release binaries"
        ));
    }
    verify_report_binary_matches_source(
        label,
        "baseline",
        &report.provenance.baseline,
        &report.provenance.baseline_source,
    )?;
    verify_report_binary_matches_source(
        label,
        "candidate",
        &report.provenance.candidate,
        &report.provenance.candidate_source,
    )?;
    verify_live_binary(label, "baseline", &report.provenance.baseline)?;
    verify_live_binary(label, "candidate", &report.provenance.candidate)?;
    verify_live_source(label, "baseline", &report.provenance.baseline_source)?;
    verify_live_source(label, "candidate", &report.provenance.candidate_source)?;
    validate_contracts("reported baseline", &report.contracts.baseline)?;
    validate_contracts("reported candidate", &report.contracts.candidate)?;
    if report.contracts.baseline != report.contracts.candidate {
        return Err(format!("{label} report contract witnesses differ by arm"));
    }
    validate_report_fixtures(report, label)?;
    Ok(())
}

fn verify_live_binary(
    report_label: &str,
    arm: &str,
    binary: &VerificationBinary,
) -> Result<(), String> {
    let path = canonical_executable(&binary.path, &format!("{report_label} report {arm}"))?;
    if path != binary.path || sha256_file(&path)? != binary.sha256 {
        return Err(format!(
            "{report_label} report {arm} binary path or SHA-256 no longer matches live evidence"
        ));
    }
    let live_identity = read_live_identity(&path, report_label, arm)?;
    if live_identity != binary.identity {
        return Err(format!(
            "{report_label} report {arm} live helper identity does not match the report"
        ));
    }
    Ok(())
}

fn read_live_identity(
    binary: &Path,
    report_label: &str,
    arm: &str,
) -> Result<HelperIdentity, String> {
    let output = std::env::temp_dir().join(format!(
        "montage-project-read-verify-identity-{}-{}.json",
        std::process::id(),
        unique_nonce()
    ));
    let process = Command::new(binary)
        .env("LC_ALL", HELPER_LC_ALL)
        .env("TZ", HELPER_TZ)
        .args(["--helper", "--identity", "--output"])
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            format!("spawn {report_label} report {arm} live identity helper: {error}")
        })?;
    if !process.status.success() {
        return Err(format!(
            "{report_label} report {arm} live identity helper exited {}: {}",
            process.status,
            String::from_utf8_lossy(&process.stderr).trim()
        ));
    }
    let identity = read_json_file(&output)?;
    fs::remove_file(&output)
        .map_err(|error| format!("remove live identity output {}: {error}", output.display()))?;
    Ok(identity)
}

fn verify_live_source(
    report_label: &str,
    arm: &str,
    source: &VerificationSourceTree,
) -> Result<(), String> {
    let root = fs::canonicalize(&source.root).map_err(|error| {
        format!(
            "canonicalize {report_label} report {arm} source {}: {error}",
            source.root.display()
        )
    })?;
    let paths_match = root == source.root
        && source.source_files.len() == COMPARISON_SOURCE_FILES.len()
        && COMPARISON_SOURCE_FILES.iter().all(|relative_path| {
            source.source_files.iter().any(|file| {
                file.relative_path == *relative_path
                    && sha256_file(&root.join(relative_path))
                        .is_ok_and(|actual| actual == file.sha256)
            })
        });
    if !paths_match {
        return Err(format!(
            "{report_label} report {arm} source files no longer match the live snapshot"
        ));
    }
    Ok(())
}

fn validate_report_fixtures(report: &VerificationReport, label: &str) -> Result<(), String> {
    let mut clip_counts = Vec::with_capacity(report.fixtures.len());
    let mut sequences = BTreeSet::new();
    for run in &report.fixtures {
        let clip_count = run.fixture.clip_count;
        clip_counts.push(clip_count);
        if run.fixture.fingerprint != run.final_input_fingerprint {
            return Err(format!(
                "{label} report {clip_count}-clip input fingerprint changed"
            ));
        }
        validate_compact_witness(&run.compact_witness, clip_count, "report", "summary", 0)?;
        validate_full_fixture_witness(
            "reported baseline",
            clip_count,
            &run.full_integrity.baseline,
        )?;
        validate_full_fixture_witness(
            "reported candidate",
            clip_count,
            &run.full_integrity.candidate,
        )?;
        if run.full_integrity.baseline != run.full_integrity.candidate {
            return Err(format!(
                "{label} report {clip_count}-clip full typed witnesses differ by arm"
            ));
        }
        validate_report_arm(
            label,
            clip_count,
            "baseline",
            &run.baseline,
            &run.compact_witness,
            &mut sequences,
        )?;
        validate_report_arm(
            label,
            clip_count,
            "candidate",
            &run.candidate,
            &run.compact_witness,
            &mut sequences,
        )?;
    }
    if clip_counts.as_slice() != DEFAULT_CLIP_COUNTS {
        return Err(format!(
            "{label} report fixture evidence is not ordered 100/1000/5000"
        ));
    }
    if !evaluate_acceptance(&report.fixtures, false).single_report_gate_passed {
        return Err(format!(
            "{label} report raw fixture evidence does not recompute to a passing gate"
        ));
    }
    validate_report_sequence_schedule(report, label)?;
    Ok(())
}

fn validate_report_sequence_schedule(
    report: &VerificationReport,
    label: &str,
) -> Result<(), String> {
    let mut expected_sequence = 0_u64;
    for run in &report.fixtures {
        for round in 0..(DEFAULT_WARMUPS + DEFAULT_SAMPLES) {
            let (phase, phase_round) = if round < DEFAULT_WARMUPS {
                ("warmup", round)
            } else {
                ("timed", round - DEFAULT_WARMUPS)
            };
            for arm_name in alternating_arms(round) {
                let arm = if arm_name == "baseline" {
                    &run.baseline
                } else {
                    &run.candidate
                };
                let sample = if phase == "warmup" {
                    &arm.warmups[phase_round]
                } else {
                    &arm.timed_samples[phase_round]
                };
                if sample.sequence != expected_sequence {
                    return Err(format!(
                        "{label} report does not preserve the alternating sample schedule at sequence {expected_sequence}"
                    ));
                }
                expected_sequence += 1;
            }
        }
    }
    Ok(())
}

fn validate_report_arm(
    report_label: &str,
    clip_count: usize,
    arm_name: &str,
    arm: &ArmResults,
    expected_witness: &CompactWitness,
    sequences: &mut BTreeSet<u64>,
) -> Result<(), String> {
    if arm.warmups.len() != DEFAULT_WARMUPS || arm.timed_samples.len() != DEFAULT_SAMPLES {
        return Err(format!(
            "{report_label} report {clip_count}-clip {arm_name} has the wrong sample counts"
        ));
    }
    for (phase, samples) in [("warmup", &arm.warmups), ("timed", &arm.timed_samples)] {
        for (round, sample) in samples.iter().enumerate() {
            if sample.arm != arm_name
                || sample.phase != phase
                || sample.round != round
                || &sample.witness != expected_witness
                || !sample.child_wall_ms.is_finite()
                || sample.child_wall_ms <= 0.0
                || !sample.time_l_real_ms.is_finite()
                || sample.time_l_real_ms < 0.0
                || sample.time_l_peak_rss_bytes == 0
                || sample.time_l_stderr_sha256.len() != 64
                || !sequences.insert(sample.sequence)
            {
                return Err(format!(
                    "{report_label} report {clip_count}-clip {arm_name} {phase} sample {round} is invalid"
                ));
            }
        }
    }
    let recomputed = ArmResults::from_samples(arm.warmups.clone(), arm.timed_samples.clone());
    if recomputed.child_wall_ms != arm.child_wall_ms
        || recomputed.time_l_peak_rss_bytes != arm.time_l_peak_rss_bytes
    {
        return Err(format!(
            "{report_label} report {clip_count}-clip {arm_name} summaries do not match raw samples"
        ));
    }
    Ok(())
}

fn verify_report_binary_matches_source(
    report_label: &str,
    arm: &str,
    binary: &VerificationBinary,
    source: &VerificationSourceTree,
) -> Result<(), String> {
    for (relative_path, compiled_hash) in [
        (
            "crates/proto/src/project.rs",
            &binary.identity.project_source_sha256,
        ),
        (
            "crates/proto/src/bin/montage-project-read-perf.rs",
            &binary.identity.benchmark_source_sha256,
        ),
        (
            "crates/proto/build.rs",
            &binary.identity.build_script_source_sha256,
        ),
        (
            "crates/proto/Cargo.toml",
            &binary.identity.proto_cargo_toml_sha256,
        ),
        ("Cargo.toml", &binary.identity.workspace_cargo_toml_sha256),
        (".cargo/config.toml", &binary.identity.cargo_config_sha256),
        ("Cargo.lock", &binary.identity.cargo_lock_sha256),
    ] {
        let actual = source
            .source_files
            .iter()
            .find(|file| file.relative_path == relative_path)
            .map(|file| &file.sha256)
            .ok_or_else(|| {
                format!("{report_label} report {arm} source is missing {relative_path}")
            })?;
        if actual != compiled_hash {
            return Err(format!(
                "{report_label} report {arm} source does not match its binary for {relative_path}"
            ));
        }
    }
    Ok(())
}

fn report_methodology_hash(report: &VerificationReport) -> Result<String, String> {
    #[derive(Serialize)]
    struct Methodology<'a> {
        configuration: &'a VerificationConfiguration,
        provenance: &'a VerificationProvenance,
    }
    canonical_typed_signature(&Methodology {
        configuration: &report.configuration,
        provenance: &report.provenance,
    })
}

fn report_evidence_hash(report: &VerificationReport) -> Result<String, String> {
    let mut measurements = Vec::new();
    for run in &report.fixtures {
        for (arm_name, arm) in [("baseline", &run.baseline), ("candidate", &run.candidate)] {
            for sample in arm.warmups.iter().chain(&arm.timed_samples) {
                measurements.push((
                    run.fixture.clip_count,
                    arm_name,
                    sample.phase.as_str(),
                    sample.round,
                    sample.sequence,
                    sample.child_wall_ms.to_bits(),
                    sample.time_l_real_ms.to_bits(),
                    sample.time_l_peak_rss_bytes,
                ));
            }
        }
    }
    canonical_typed_signature(&measurements)
}

fn is_safe_label(label: &str) -> bool {
    !label.is_empty()
        && label != "."
        && label != ".."
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn parse_clip_counts(value: &str) -> Result<Vec<usize>, String> {
    let mut counts = Vec::new();
    for raw in value.split(',') {
        let count = parse_positive("--clips", raw)?;
        if counts.contains(&count) {
            return Err("--clips must not repeat a clip count".into());
        }
        counts.push(count);
    }
    if counts.is_empty() {
        return Err("--clips requires at least one positive count".into());
    }
    Ok(counts)
}

fn parse_positive(flag: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{flag} must be a positive integer"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct HelperIdentity {
    protocol: String,
    package_version: String,
    profile: String,
    cargo_optimization_level: String,
    cargo_debug: String,
    cargo_target: String,
    cargo_rustc_version_verbose: String,
    build_environment_sha256: String,
    target_os: String,
    target_arch: String,
    project_source_sha256: String,
    benchmark_source_sha256: String,
    build_script_source_sha256: String,
    proto_cargo_toml_sha256: String,
    workspace_cargo_toml_sha256: String,
    cargo_config_sha256: String,
    cargo_lock_sha256: String,
}

fn compiled_identity() -> HelperIdentity {
    HelperIdentity {
        protocol: PROTOCOL.into(),
        package_version: env!("CARGO_PKG_VERSION").into(),
        profile: BUILD_CARGO_PROFILE.into(),
        cargo_optimization_level: env!("MONTAGE_PROJECT_READ_OPT_LEVEL").into(),
        cargo_debug: env!("MONTAGE_PROJECT_READ_DEBUG").into(),
        cargo_target: env!("MONTAGE_PROJECT_READ_TARGET").into(),
        cargo_rustc_version_verbose: env!("MONTAGE_PROJECT_READ_RUSTC_VV").into(),
        build_environment_sha256: env!("MONTAGE_PROJECT_READ_BUILD_ENV_SHA256").into(),
        target_os: std::env::consts::OS.into(),
        target_arch: std::env::consts::ARCH.into(),
        project_source_sha256: sha256_bytes(include_bytes!("../project.rs")),
        benchmark_source_sha256: sha256_bytes(include_bytes!("montage-project-read-perf.rs")),
        build_script_source_sha256: sha256_bytes(include_bytes!("../../build.rs")),
        proto_cargo_toml_sha256: sha256_bytes(include_bytes!("../../Cargo.toml")),
        workspace_cargo_toml_sha256: sha256_bytes(include_bytes!("../../../../Cargo.toml")),
        cargo_config_sha256: sha256_bytes(include_bytes!("../../../../.cargo/config.toml")),
        cargo_lock_sha256: sha256_bytes(include_bytes!("../../../../Cargo.lock")),
    }
}

enum HelperRequest {
    Identity { output: PathBuf },
    Compact { project: PathBuf, output: PathBuf },
    Contract { project: PathBuf, output: PathBuf },
}

fn helper_main(raw: &[String]) -> Result<(), String> {
    let request = parse_helper_request(raw)?;
    match request {
        HelperRequest::Identity { output } => write_helper_output(&output, &compiled_identity()),
        HelperRequest::Compact { project, output } => {
            write_helper_output(&output, &compact_witness(&project))
        }
        HelperRequest::Contract { project, output } => {
            write_helper_output(&output, &read_witness(&project))
        }
    }
}

fn parse_helper_request(raw: &[String]) -> Result<HelperRequest, String> {
    let mut identity = false;
    let mut contract = false;
    let mut project = None;
    let mut output = None;
    let mut index = 0;
    while index < raw.len() {
        let flag = &raw[index];
        index += 1;
        let value = |name: &str, index: &mut usize| {
            let value = raw
                .get(*index)
                .cloned()
                .ok_or_else(|| format!("{name} requires a value"))?;
            *index += 1;
            Ok::<_, String>(value)
        };
        match flag.as_str() {
            "--identity" => identity = true,
            "--contract" => contract = true,
            "--project" => project = Some(PathBuf::from(value("--project", &mut index)?)),
            "--output" => output = Some(PathBuf::from(value("--output", &mut index)?)),
            _ => return Err(format!("unknown helper argument: {flag}")),
        }
    }
    let output = output.ok_or_else(|| "--output is required".to_string())?;
    if identity {
        if project.is_some() || contract {
            return Err("--identity cannot be combined with project read options".into());
        }
        return Ok(HelperRequest::Identity { output });
    }
    let project = project.ok_or_else(|| "--project is required".to_string())?;
    if contract {
        Ok(HelperRequest::Contract { project, output })
    } else {
        Ok(HelperRequest::Compact { project, output })
    }
}

fn write_helper_output<T: Serialize>(output: &Path, value: &T) -> Result<(), String> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| format!("serialize helper witness: {error}"))?;
    write_atomically_new(output, &bytes)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CompactWitness {
    outcome: String,
    timeline_schema: Option<String>,
    timeline_name: Option<String>,
    timeline_track_count: usize,
    timeline_clip_count: usize,
    timeline_marker_count: usize,
    edit_plan_version: Option<String>,
    edit_plan_item_count: usize,
    manifest_version: Option<String>,
    manifest_indexer_count: usize,
    manifest_asset_count: usize,
    warnings: Vec<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ContractWitness {
    outcome: String,
    typed_timeline_signature: Option<String>,
    typed_edit_plan_signature: Option<String>,
    typed_manifest_signature: Option<String>,
    timeline_clip_count: usize,
    timeline_marker_count: usize,
    warnings: Vec<String>,
    warnings_signature: String,
    error: Option<String>,
}

fn compact_witness(root: &Path) -> CompactWitness {
    match Project::read(root) {
        Ok(project) => {
            let (timeline_track_count, timeline_clip_count) = count_timeline(&project.timeline);
            let timeline_marker_count = project
                .timeline
                .metadata
                .montage
                .as_ref()
                .map_or(0, |metadata| metadata.timeline_markers.len());
            CompactWitness {
                outcome: "ok".into(),
                timeline_schema: Some(project.timeline.otio_schema),
                timeline_name: Some(project.timeline.name),
                timeline_track_count,
                timeline_clip_count,
                timeline_marker_count,
                edit_plan_version: Some(project.edit_plan.version.clone()),
                edit_plan_item_count: project.edit_plan.items.len(),
                manifest_version: project
                    .manifest
                    .as_ref()
                    .map(|manifest| manifest.version.clone()),
                manifest_indexer_count: project
                    .manifest
                    .as_ref()
                    .map_or(0, |manifest| manifest.indexers.len()),
                manifest_asset_count: project.manifest.as_ref().map_or(0, |manifest| {
                    manifest
                        .indexers
                        .iter()
                        .map(|indexer| indexer.assets.len())
                        .sum()
                }),
                warnings: warning_signatures(&project.warnings),
                error: None,
            }
        }
        Err(error) => CompactWitness {
            outcome: "error".into(),
            timeline_schema: None,
            timeline_name: None,
            timeline_track_count: 0,
            timeline_clip_count: 0,
            timeline_marker_count: 0,
            edit_plan_version: None,
            edit_plan_item_count: 0,
            manifest_version: None,
            manifest_indexer_count: 0,
            manifest_asset_count: 0,
            warnings: Vec::new(),
            error: Some(error_signature(&error)),
        },
    }
}

fn read_witness(root: &Path) -> ContractWitness {
    match Project::read(root) {
        Ok(project) => {
            let (_, timeline_clip_count) = count_timeline(&project.timeline);
            let timeline_marker_count = project
                .timeline
                .metadata
                .montage
                .as_ref()
                .map_or(0, |metadata| metadata.timeline_markers.len());
            let warnings = warning_signatures(&project.warnings);
            let warnings_signature = canonical_signature(&Value::Array(
                warnings.iter().cloned().map(Value::String).collect(),
            ));
            let signatures = (|| {
                Ok::<_, String>((
                    canonical_timeline_signature(&project.timeline)?,
                    canonical_typed_signature(&project.edit_plan)?,
                    canonical_typed_signature(&project.manifest)?,
                ))
            })();
            match signatures {
                Ok((
                    typed_timeline_signature,
                    typed_edit_plan_signature,
                    typed_manifest_signature,
                )) => ContractWitness {
                    outcome: "ok".into(),
                    typed_timeline_signature: Some(typed_timeline_signature),
                    typed_edit_plan_signature: Some(typed_edit_plan_signature),
                    typed_manifest_signature: Some(typed_manifest_signature),
                    timeline_clip_count,
                    timeline_marker_count,
                    warnings,
                    warnings_signature,
                    error: None,
                },
                Err(error) => ContractWitness {
                    outcome: "error".into(),
                    typed_timeline_signature: None,
                    typed_edit_plan_signature: None,
                    typed_manifest_signature: None,
                    timeline_clip_count: 0,
                    timeline_marker_count: 0,
                    warnings: Vec::new(),
                    warnings_signature: String::new(),
                    error: Some(format!("WitnessSerialization|{error}")),
                },
            }
        }
        Err(error) => ContractWitness {
            outcome: "error".into(),
            typed_timeline_signature: None,
            typed_edit_plan_signature: None,
            typed_manifest_signature: None,
            timeline_clip_count: 0,
            timeline_marker_count: 0,
            warnings: Vec::new(),
            warnings_signature: canonical_signature(&Value::Array(Vec::new())),
            error: Some(error_signature(&error)),
        },
    }
}

fn count_timeline(timeline: &Timeline) -> (usize, usize) {
    count_stack(&timeline.tracks)
}

fn count_stack(stack: &Stack) -> (usize, usize) {
    let mut tracks = 0;
    let mut clips = 0;
    for child in &stack.children {
        match child {
            StackChild::Track(track) => {
                tracks += 1;
                let (_, child_clips) = count_track(track);
                clips += child_clips;
            }
            StackChild::Stack(child_stack) => {
                let (child_tracks, child_clips) = count_stack(child_stack);
                tracks += child_tracks;
                clips += child_clips;
            }
            StackChild::Clip(_) => clips += 1,
            StackChild::Gap(_) => {}
        }
    }
    (tracks, clips)
}

fn count_track(track: &Track) -> (usize, usize) {
    let mut tracks = 0;
    let mut clips = 0;
    for child in &track.children {
        match child {
            TrackChild::Clip(_) => clips += 1,
            TrackChild::Stack(stack) => {
                let (child_tracks, child_clips) = count_stack(stack);
                tracks += child_tracks;
                clips += child_clips;
            }
            TrackChild::Gap(_) | TrackChild::Transition(_) => {}
        }
    }
    (tracks, clips)
}

fn warning_signatures(warnings: &[montage_proto::otio::SchemaWarning]) -> Vec<String> {
    warnings
        .iter()
        .map(|warning| {
            format!(
                "{}|{}|{}|{}",
                warning.schema, warning.path, warning.expected_major, warning.found_major
            )
        })
        .collect()
}

fn error_signature(error: &ProtoError) -> String {
    match error {
        ProtoError::UnknownOtioSchema { schema, path, .. } => {
            format!("UnknownOtioSchema|{schema}|{}", path.as_str())
        }
        ProtoError::MalformedOtioSchema { schema, path, .. } => {
            format!("MalformedOtioSchema|{schema}|{}", path.as_str())
        }
        ProtoError::Validation { path, message, .. } => {
            format!("Validation|{}|{message}", path.as_str())
        }
        ProtoError::Json { line, column, .. } => format!("Json|{line}|{column}"),
        ProtoError::Io { source, .. } => format!("Io|{:?}", source.kind()),
    }
}

fn canonical_timeline_signature(timeline: &Timeline) -> Result<String, String> {
    canonical_typed_signature(timeline)
}

fn canonical_typed_signature(value: &impl Serialize) -> Result<String, String> {
    let value = serde_json::to_value(value).map_err(|error| error.to_string())?;
    Ok(canonical_signature(&value))
}

fn canonical_signature(value: &Value) -> String {
    let mut text = String::new();
    if canonical_json(value, &mut text).is_err() {
        return String::new();
    }
    sha256_bytes(text.as_bytes())
}

fn canonical_json(value: &Value, output: &mut String) -> Result<(), String> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => output.push_str(
            &serde_json::to_string(value)
                .map_err(|error| format!("encode JSON string: {error}"))?,
        ),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                canonical_json(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(
                    &serde_json::to_string(key)
                        .map_err(|error| format!("encode JSON key: {error}"))?,
                );
                output.push(':');
                let value = values
                    .get(key)
                    .ok_or_else(|| format!("missing canonical JSON key: {key}"))?;
                canonical_json(value, output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Fixture {
    root: PathBuf,
    clip_count: usize,
    fingerprint: InputFingerprint,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct InputFingerprint {
    sha256: String,
    files: Vec<InputFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct InputFile {
    relative_path: String,
    bytes: u64,
    sha256: String,
}

fn write_fixture(root: &Path, clip_count: usize) -> Result<Fixture, String> {
    if root.exists() {
        return Err(format!("refusing to overwrite fixture {}", root.display()));
    }
    let mut project =
        Project::init(root).map_err(|error| format!("init {}: {error}", root.display()))?;
    project.timeline = Timeline::empty("project-read-fixture");
    let mut track = Track::empty("V1", TrackKind::Video);
    track.children.reserve(clip_count);
    for index in 0..clip_count {
        track
            .children
            .push(TrackChild::Clip(Clip::empty(format!("clip-{index:05}"))));
    }
    project
        .timeline
        .tracks
        .children
        .push(StackChild::Track(track));
    project.edit_plan.brief = Some("Deterministic Project::read benchmark fixture".into());
    let mut extra = serde_json::Map::new();
    extra.insert(
        "asset".into(),
        Value::String("raw/episode-001-cam-a.mp4".into()),
    );
    extra.insert("in_s".into(), serde_json::json!(12.0));
    extra.insert("out_s".into(), serde_json::json!(24.0));
    project.edit_plan.items.push(EditPlanItem {
        id: "fixture-trim-001".into(),
        kind: "trim".into(),
        status: "pending".into(),
        extra,
    });
    let last_run = DateTime::parse_from_rfc3339("2025-01-02T03:04:05Z")
        .map_err(|error| format!("parse fixed fixture timestamp: {error}"))?
        .with_timezone(&Utc);
    let mut manifest = Manifest::empty();
    manifest.indexers.push(IndexerEntry {
        name: "fixture-transcript".into(),
        version: "1.0.0".into(),
        schema_version: "0.1".into(),
        assets: vec![
            AssetId::new("raw/episode-001-cam-a.mp4"),
            AssetId::new("raw/episode-001-cam-b.mp4"),
        ],
        last_run,
    });
    project.manifest = Some(manifest);
    project
        .write(root)
        .map_err(|error| format!("write fixture {}: {error}", root.display()))?;
    Ok(Fixture {
        root: root.to_path_buf(),
        clip_count,
        fingerprint: fingerprint_project_inputs(root)?,
    })
}

fn fingerprint_project_inputs(root: &Path) -> Result<InputFingerprint, String> {
    let relative_paths = [files::OTIO, files::EDIT_PLAN, "index/manifest.json"];
    let mut files = Vec::with_capacity(relative_paths.len());
    let mut hasher = Sha256::new();
    hasher.update(b"montage-project-read-input-v1\0");
    for relative_path in relative_paths {
        let path = root.join(relative_path);
        let metadata =
            fs::metadata(&path).map_err(|error| format!("metadata {}: {error}", path.display()))?;
        let sha256 = sha256_file(&path)?;
        hasher.update((relative_path.len() as u64).to_le_bytes());
        hasher.update(relative_path.as_bytes());
        hasher.update(metadata.len().to_le_bytes());
        hasher.update(sha256.as_bytes());
        files.push(InputFile {
            relative_path: relative_path.into(),
            bytes: metadata.len(),
            sha256,
        });
    }
    Ok(InputFingerprint {
        sha256: format!("{:x}", hasher.finalize()),
        files,
    })
}

#[derive(Debug, Clone)]
struct ContractFixtures {
    valid: Fixture,
    forward_clip_99: Fixture,
    recoverable_marker: Fixture,
    unknown_schema: Fixture,
}

impl ContractFixtures {
    fn fingerprints(&self) -> ContractFixtureFingerprints {
        ContractFixtureFingerprints {
            valid: self.valid.fingerprint.clone(),
            forward_clip_99: self.forward_clip_99.fingerprint.clone(),
            recoverable_marker: self.recoverable_marker.fingerprint.clone(),
            unknown_schema: self.unknown_schema.fingerprint.clone(),
        }
    }
}

fn write_contract_fixtures(root: &Path) -> Result<ContractFixtures, String> {
    let valid = write_fixture(&root.join("valid"), 2)?;
    let mut forward_clip_99 = write_fixture(&root.join("forward-clip-99"), 2)?;
    mutate_timeline(&forward_clip_99.root, |timeline| {
        timeline["tracks"]["children"][0]["children"][0]["OTIO_SCHEMA"] =
            Value::String("Clip.99".into());
    })?;
    forward_clip_99.fingerprint = fingerprint_project_inputs(&forward_clip_99.root)?;

    let mut recoverable_marker = write_fixture(&root.join("recoverable-marker"), 2)?;
    mutate_timeline(&recoverable_marker.root, |timeline| {
        timeline["metadata"]["montage"]["timeline_markers"] = serde_json::json!([
            {"name": "bad-marker", "source_time_s": 12.0}
        ]);
    })?;
    recoverable_marker.fingerprint = fingerprint_project_inputs(&recoverable_marker.root)?;

    let mut unknown_schema = write_fixture(&root.join("unknown-schema"), 2)?;
    mutate_timeline(&unknown_schema.root, |timeline| {
        timeline["tracks"]["children"][0]["OTIO_SCHEMA"] = Value::String("UnknownNode.1".into());
    })?;
    unknown_schema.fingerprint = fingerprint_project_inputs(&unknown_schema.root)?;

    Ok(ContractFixtures {
        valid,
        forward_clip_99,
        recoverable_marker,
        unknown_schema,
    })
}

fn mutate_timeline(root: &Path, mutation: impl FnOnce(&mut Value)) -> Result<(), String> {
    let path = root.join(files::OTIO);
    let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let mut timeline: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse {}: {error}", path.display()))?;
    mutation(&mut timeline);
    let bytes = serde_json::to_vec_pretty(&timeline)
        .map_err(|error| format!("serialize {}: {error}", path.display()))?;
    fs::write(&path, bytes).map_err(|error| format!("write {}: {error}", path.display()))
}

fn assert_contract_inputs_unchanged(contracts: &ContractFixtures) -> Result<(), String> {
    for fixture in [
        &contracts.valid,
        &contracts.forward_clip_99,
        &contracts.recoverable_marker,
        &contracts.unknown_schema,
    ] {
        if fixture.fingerprint != fingerprint_project_inputs(&fixture.root)? {
            return Err(format!(
                "contract input changed: {}",
                fixture.root.display()
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ContractFixtureFingerprints {
    valid: InputFingerprint,
    forward_clip_99: InputFingerprint,
    recoverable_marker: InputFingerprint,
    unknown_schema: InputFingerprint,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ContractWitnesses {
    valid: ContractWitness,
    forward_clip_99: ContractWitness,
    recoverable_marker: ContractWitness,
    unknown_schema: ContractWitness,
}

fn run_contracts(
    binary: &Path,
    contracts: &ContractFixtures,
    run_dir: &Path,
    arm: &str,
) -> Result<ContractWitnesses, String> {
    Ok(ContractWitnesses {
        valid: run_contract_helper(binary, &contracts.valid.root, run_dir, arm, "valid")?,
        forward_clip_99: run_contract_helper(
            binary,
            &contracts.forward_clip_99.root,
            run_dir,
            arm,
            "forward-clip-99",
        )?,
        recoverable_marker: run_contract_helper(
            binary,
            &contracts.recoverable_marker.root,
            run_dir,
            arm,
            "recoverable-marker",
        )?,
        unknown_schema: run_contract_helper(
            binary,
            &contracts.unknown_schema.root,
            run_dir,
            arm,
            "unknown-schema",
        )?,
    })
}

fn run_contract_helper(
    binary: &Path,
    project: &Path,
    run_dir: &Path,
    arm: &str,
    name: &str,
) -> Result<ContractWitness, String> {
    let output = run_dir
        .join("helper-results")
        .join(format!("contract-{arm}-{name}-{}.json", unique_nonce()));
    let status = Command::new(binary)
        .env("LC_ALL", HELPER_LC_ALL)
        .env("TZ", HELPER_TZ)
        .args(["--helper", "--contract", "--project"])
        .arg(project)
        .arg("--output")
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("spawn {arm} contract helper: {error}"))?;
    if !status.status.success() {
        return Err(format!(
            "{arm} contract helper {name} exited {}: {}",
            status.status,
            String::from_utf8_lossy(&status.stderr).trim()
        ));
    }
    read_json_file(&output)
}

fn validate_contracts(arm: &str, contracts: &ContractWitnesses) -> Result<(), String> {
    let valid = &contracts.valid;
    if valid.outcome != "ok"
        || valid.timeline_clip_count != 2
        || valid.timeline_marker_count != 0
        || !valid.warnings.is_empty()
        || valid.error.is_some()
        || valid.typed_timeline_signature.is_none()
        || valid.typed_edit_plan_signature.is_none()
        || valid.typed_manifest_signature.is_none()
    {
        return Err(format!("{arm} valid-project contract failed: {valid:?}"));
    }
    let forward = &contracts.forward_clip_99;
    if forward.outcome != "ok"
        || forward.typed_timeline_signature != valid.typed_timeline_signature
        || forward.typed_edit_plan_signature != valid.typed_edit_plan_signature
        || forward.typed_manifest_signature != valid.typed_manifest_signature
        || forward.warnings != vec!["Clip.99|tracks.children[0].children[0]|2|99".to_string()]
        || forward.error.is_some()
    {
        return Err(format!(
            "{arm} Clip.99 forward-compat contract failed: {forward:?}"
        ));
    }
    let recovered = &contracts.recoverable_marker;
    if recovered.outcome != "ok"
        || recovered.typed_timeline_signature != valid.typed_timeline_signature
        || recovered.typed_edit_plan_signature != valid.typed_edit_plan_signature
        || recovered.typed_manifest_signature != valid.typed_manifest_signature
        || recovered.timeline_marker_count != 0
        || !recovered.warnings.is_empty()
        || recovered.error.is_some()
    {
        return Err(format!(
            "{arm} malformed timeline-marker recovery contract failed: {recovered:?}"
        ));
    }
    let unknown = &contracts.unknown_schema;
    if unknown.outcome != "error"
        || unknown.error.as_deref() != Some("UnknownOtioSchema|UnknownNode.1|tracks.children[0]")
        || unknown.typed_timeline_signature.is_some()
        || unknown.typed_edit_plan_signature.is_some()
        || unknown.typed_manifest_signature.is_some()
    {
        return Err(format!(
            "{arm} unknown-schema hard-failure contract failed: {unknown:?}"
        ));
    }
    Ok(())
}

fn validate_full_fixture_witness(
    arm: &str,
    clip_count: usize,
    witness: &ContractWitness,
) -> Result<(), String> {
    if witness.outcome != "ok"
        || witness.timeline_clip_count != clip_count
        || witness.timeline_marker_count != 0
        || !witness.warnings.is_empty()
        || witness.error.is_some()
        || witness.typed_timeline_signature.is_none()
        || witness.typed_edit_plan_signature.is_none()
        || witness.typed_manifest_signature.is_none()
    {
        return Err(format!(
            "{arm} {clip_count}-clip full typed witness failed: {witness:?}"
        ));
    }
    Ok(())
}

fn read_identity(binary: &Path, run_dir: &Path, arm: &str) -> Result<HelperIdentity, String> {
    let output = run_dir
        .join("helper-results")
        .join(format!("identity-{arm}-{}.json", unique_nonce()));
    let process = Command::new(binary)
        .env("LC_ALL", HELPER_LC_ALL)
        .env("TZ", HELPER_TZ)
        .args(["--helper", "--identity", "--output"])
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("spawn {arm} identity helper: {error}"))?;
    if !process.status.success() {
        return Err(format!(
            "{arm} identity helper exited {}: {}",
            process.status,
            String::from_utf8_lossy(&process.stderr).trim()
        ));
    }
    read_json_file(&output)
}

fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("parse {}: {error}", path.display()))
}

#[derive(Debug)]
struct MeasuredFixture {
    baseline: ArmResults,
    candidate: ArmResults,
    compact_witness: CompactWitness,
}

fn run_fixture(
    args: &ControllerArgs,
    fixture: &Fixture,
    run_dir: &Path,
    sequence: &mut u64,
    baseline: &BinaryProvenance,
    candidate: &BinaryProvenance,
) -> Result<MeasuredFixture, String> {
    let mut baseline_warmups = Vec::with_capacity(args.warmups);
    let mut candidate_warmups = Vec::with_capacity(args.warmups);
    let mut baseline_timed = Vec::with_capacity(args.samples);
    let mut candidate_timed = Vec::with_capacity(args.samples);
    let mut expected = None;
    for round in 0..(args.warmups + args.samples) {
        let phase = if round < args.warmups {
            "warmup"
        } else {
            "timed"
        };
        let phase_round = if round < args.warmups {
            round
        } else {
            round - args.warmups
        };
        for name in alternating_arms(round) {
            let (binary, is_baseline) = if name == "baseline" {
                (&baseline.path, true)
            } else {
                (&candidate.path, false)
            };
            let sample = run_timed_helper(TimedHelperRequest {
                binary,
                project: &fixture.root,
                run_dir,
                clip_count: fixture.clip_count,
                phase,
                round: phase_round,
                arm: name,
                sequence: *sequence,
            })?;
            *sequence += 1;
            validate_compact_witness(
                &sample.witness,
                fixture.clip_count,
                name,
                phase,
                phase_round,
            )?;
            if let Some(expected_witness) = &expected {
                if expected_witness != &sample.witness {
                    return Err(format!(
                        "{}-clip {name} {phase} {phase_round} witness differs from the first successful read",
                        fixture.clip_count
                    ));
                }
            } else {
                expected = Some(sample.witness.clone());
            }
            match (phase, is_baseline) {
                ("warmup", true) => baseline_warmups.push(sample),
                ("warmup", false) => candidate_warmups.push(sample),
                ("timed", true) => baseline_timed.push(sample),
                ("timed", false) => candidate_timed.push(sample),
                _ => return Err("unknown benchmark phase".into()),
            }
        }
    }
    let compact_witness =
        expected.ok_or_else(|| "no helper witnesses were produced".to_string())?;
    Ok(MeasuredFixture {
        baseline: ArmResults::from_samples(baseline_warmups, baseline_timed),
        candidate: ArmResults::from_samples(candidate_warmups, candidate_timed),
        compact_witness,
    })
}

fn alternating_arms(round: usize) -> [&'static str; 2] {
    if round.is_multiple_of(2) {
        ["baseline", "candidate"]
    } else {
        ["candidate", "baseline"]
    }
}

fn validate_compact_witness(
    witness: &CompactWitness,
    clip_count: usize,
    arm: &str,
    phase: &str,
    round: usize,
) -> Result<(), String> {
    if witness.outcome != "ok"
        || witness.timeline_schema.as_deref() != Some("Timeline.1")
        || witness.timeline_name.as_deref() != Some("project-read-fixture")
        || witness.timeline_track_count != 1
        || witness.timeline_clip_count != clip_count
        || witness.timeline_marker_count != 0
        || witness.edit_plan_version.as_deref() != Some(montage_proto::EDIT_PLAN_VERSION)
        || witness.edit_plan_item_count != 1
        || witness.manifest_version.as_deref() != Some(montage_proto::INDEX_MANIFEST_VERSION)
        || witness.manifest_indexer_count != 1
        || witness.manifest_asset_count != 2
        || !witness.warnings.is_empty()
        || witness.error.is_some()
    {
        return Err(format!(
            "{arm} {phase} {round} compact witness failed: {witness:?}"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Sample {
    sequence: u64,
    arm: String,
    phase: String,
    round: usize,
    child_wall_ms: f64,
    time_l_real_ms: f64,
    time_l_peak_rss_bytes: u64,
    time_l_stderr_sha256: String,
    witness: CompactWitness,
}

struct TimedHelperRequest<'a> {
    binary: &'a Path,
    project: &'a Path,
    run_dir: &'a Path,
    clip_count: usize,
    phase: &'a str,
    round: usize,
    arm: &'a str,
    sequence: u64,
}

fn run_timed_helper(request: TimedHelperRequest<'_>) -> Result<Sample, String> {
    let output = request.run_dir.join("helper-results").join(format!(
        "{}-{}-{}-{}-{}-{}.json",
        request.clip_count,
        request.phase,
        request.round,
        request.arm,
        request.sequence,
        unique_nonce()
    ));
    let started = Instant::now();
    let process = Command::new(TIME_PATH)
        .env("LC_ALL", HELPER_LC_ALL)
        .env("TZ", HELPER_TZ)
        .arg("-l")
        .arg(request.binary)
        .args(["--helper", "--project"])
        .arg(request.project)
        .arg("--output")
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| {
            format!(
                "spawn {} {} helper through {TIME_PATH}: {error}",
                request.arm, request.phase
            )
        })?;
    let child_wall_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let time_l_stderr = String::from_utf8_lossy(&process.stderr);
    if !process.status.success() {
        return Err(format!(
            "{} {} helper exited {}: {}",
            request.arm,
            request.phase,
            process.status,
            time_l_stderr.trim()
        ));
    }
    let time_l_peak_rss_bytes = parse_time_l_peak_rss(&time_l_stderr)?;
    let time_l_real_ms = parse_time_l_real_ms(&time_l_stderr)?;
    let witness = read_json_file(&output)?;
    Ok(Sample {
        sequence: request.sequence,
        arm: request.arm.into(),
        phase: request.phase.into(),
        round: request.round,
        child_wall_ms,
        time_l_real_ms,
        time_l_peak_rss_bytes,
        time_l_stderr_sha256: sha256_bytes(time_l_stderr.as_bytes()),
        witness,
    })
}

fn parse_time_l_peak_rss(output: &str) -> Result<u64, String> {
    output
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_suffix("maximum resident set size")
                .and_then(|value| value.trim().parse::<u64>().ok())
        })
        .ok_or_else(|| {
            format!("could not parse maximum resident set size from {TIME_PATH} -l output")
        })
}

fn parse_time_l_real_ms(output: &str) -> Result<f64, String> {
    for line in output.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        for pair in fields.windows(2) {
            if pair[1] == "real" {
                let seconds = pair[0]
                    .parse::<f64>()
                    .map_err(|error| format!("parse {TIME_PATH} real time: {error}"))?;
                return Ok(seconds * 1_000.0);
            }
        }
    }
    Err(format!(
        "could not parse real time from {TIME_PATH} -l output"
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArmResults {
    warmups: Vec<Sample>,
    timed_samples: Vec<Sample>,
    child_wall_ms: Distribution,
    time_l_peak_rss_bytes: Distribution,
}

impl ArmResults {
    fn from_samples(warmups: Vec<Sample>, timed_samples: Vec<Sample>) -> Self {
        let wall_samples: Vec<f64> = timed_samples
            .iter()
            .map(|sample| sample.child_wall_ms)
            .collect();
        let rss_samples: Vec<f64> = timed_samples
            .iter()
            .map(|sample| sample.time_l_peak_rss_bytes as f64)
            .collect();
        Self {
            warmups,
            timed_samples,
            child_wall_ms: summarize_distribution(&wall_samples),
            time_l_peak_rss_bytes: summarize_distribution(&rss_samples),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Distribution {
    median: f64,
    p95: f64,
    mad: f64,
    min: f64,
    max: f64,
}

fn summarize_distribution(samples: &[f64]) -> Distribution {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median_value = median(&sorted);
    let mut deviations: Vec<f64> = sorted
        .iter()
        .map(|value| (value - median_value).abs())
        .collect();
    deviations.sort_by(f64::total_cmp);
    let p95_index = (sorted.len() * 95).div_ceil(100).saturating_sub(1);
    Distribution {
        median: median_value,
        p95: sorted[p95_index],
        mad: median(&deviations),
        min: sorted[0],
        max: sorted[sorted.len() - 1],
    }
}

fn median(sorted: &[f64]) -> f64 {
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FixtureRun {
    fixture: Fixture,
    final_input_fingerprint: InputFingerprint,
    baseline: ArmResults,
    candidate: ArmResults,
    compact_witness: CompactWitness,
    full_integrity: FullFixtureIntegrity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FullFixtureIntegrity {
    baseline: ContractWitness,
    candidate: ContractWitness,
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    schema_version: u32,
    protocol: String,
    report_id: String,
    session_id: String,
    generated_at_utc: String,
    configuration: Configuration,
    provenance: Provenance,
    contracts: ContractReport,
    fixtures: Vec<FixtureRun>,
    acceptance: Acceptance,
}

#[derive(Debug, Serialize)]
struct Configuration {
    label: String,
    clip_counts: Vec<usize>,
    warmups: usize,
    samples: usize,
    smoke: bool,
    allow_dirty_source: bool,
    work_dir: String,
    report_dir: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ContractReport {
    fixtures: ContractFixtureFingerprints,
    baseline: ContractWitnesses,
    candidate: ContractWitnesses,
}

#[derive(Debug, Serialize)]
struct Acceptance {
    single_report_gate_passed: bool,
    program_acceptance: bool,
    smoke_not_qualifying: bool,
    independent_reports_required: usize,
    independent_report_rule: String,
    one_k_is_corroboration_only: bool,
    has_required_fixture_sizes: bool,
    no_p95_regression_at_any_size: bool,
    five_k_latency: ImprovementCheck,
    five_k_rss: ImprovementCheck,
    per_size: Vec<SizeComparison>,
}

#[derive(Debug, Serialize)]
struct ImprovementCheck {
    applicable: bool,
    passed: bool,
    absolute_improvement: f64,
    percent_improvement: f64,
    required_absolute_improvement: f64,
    required_percent_improvement: f64,
    non_worse_median_latency: bool,
    non_worse_p95_latency: bool,
}

#[derive(Debug, Serialize)]
struct SizeComparison {
    clip_count: usize,
    p95_latency_non_worse: bool,
    median_latency_delta_ms: f64,
    median_latency_percent_improvement: f64,
    median_rss_delta_mib: f64,
    median_rss_percent_improvement: f64,
}

fn evaluate_acceptance(runs: &[FixtureRun], smoke: bool) -> Acceptance {
    let per_size: Vec<_> = runs.iter().map(size_comparison).collect();
    let no_p95_regression_at_any_size = per_size
        .iter()
        .all(|comparison| comparison.p95_latency_non_worse);
    let five_k = runs.iter().find(|run| run.fixture.clip_count == 5_000);
    let five_k_latency = five_k.map_or_else(
        || ImprovementCheck::not_applicable(10.0, 10.0),
        |run| {
            let baseline = &run.baseline.child_wall_ms;
            let candidate = &run.candidate.child_wall_ms;
            let absolute = baseline.median - candidate.median;
            let percent = percent_improvement(baseline.median, candidate.median);
            let non_worse_p95 = candidate.p95 <= baseline.p95;
            ImprovementCheck {
                applicable: true,
                passed: absolute >= 10.0 && percent >= 10.0 && non_worse_p95,
                absolute_improvement: absolute,
                percent_improvement: percent,
                required_absolute_improvement: 10.0,
                required_percent_improvement: 10.0,
                non_worse_median_latency: candidate.median <= baseline.median,
                non_worse_p95_latency: non_worse_p95,
            }
        },
    );
    let five_k_rss = five_k.map_or_else(
        || ImprovementCheck::not_applicable(10.0 * MIB, 25.0),
        |run| {
            let baseline_rss = &run.baseline.time_l_peak_rss_bytes;
            let candidate_rss = &run.candidate.time_l_peak_rss_bytes;
            let baseline_latency = &run.baseline.child_wall_ms;
            let candidate_latency = &run.candidate.child_wall_ms;
            let absolute = baseline_rss.median - candidate_rss.median;
            let percent = percent_improvement(baseline_rss.median, candidate_rss.median);
            let non_worse_median_latency = candidate_latency.median <= baseline_latency.median;
            let non_worse_p95_latency = candidate_latency.p95 <= baseline_latency.p95;
            ImprovementCheck {
                applicable: true,
                passed: absolute >= 10.0 * MIB
                    && percent >= 25.0
                    && non_worse_median_latency
                    && non_worse_p95_latency,
                absolute_improvement: absolute,
                percent_improvement: percent,
                required_absolute_improvement: 10.0 * MIB,
                required_percent_improvement: 25.0,
                non_worse_median_latency,
                non_worse_p95_latency,
            }
        },
    );
    let has_required_fixture_sizes = DEFAULT_CLIP_COUNTS
        .iter()
        .all(|required| runs.iter().any(|run| run.fixture.clip_count == *required));
    let single_report_gate_passed = !smoke
        && has_required_fixture_sizes
        && no_p95_regression_at_any_size
        && (five_k_latency.passed || five_k_rss.passed);
    Acceptance {
        single_report_gate_passed,
        program_acceptance: false,
        smoke_not_qualifying: smoke,
        independent_reports_required: 2,
        independent_report_rule: "Use --verify-reports with two independently generated full reports that each set single_report_gate_passed=true.".into(),
        one_k_is_corroboration_only: true,
        has_required_fixture_sizes,
        no_p95_regression_at_any_size,
        five_k_latency,
        five_k_rss,
        per_size,
    }
}

impl ImprovementCheck {
    fn not_applicable(
        required_absolute_improvement: f64,
        required_percent_improvement: f64,
    ) -> Self {
        Self {
            applicable: false,
            passed: false,
            absolute_improvement: 0.0,
            percent_improvement: 0.0,
            required_absolute_improvement,
            required_percent_improvement,
            non_worse_median_latency: false,
            non_worse_p95_latency: false,
        }
    }
}

fn size_comparison(run: &FixtureRun) -> SizeComparison {
    let baseline_latency = &run.baseline.child_wall_ms;
    let candidate_latency = &run.candidate.child_wall_ms;
    let baseline_rss = &run.baseline.time_l_peak_rss_bytes;
    let candidate_rss = &run.candidate.time_l_peak_rss_bytes;
    SizeComparison {
        clip_count: run.fixture.clip_count,
        p95_latency_non_worse: candidate_latency.p95 <= baseline_latency.p95,
        median_latency_delta_ms: baseline_latency.median - candidate_latency.median,
        median_latency_percent_improvement: percent_improvement(
            baseline_latency.median,
            candidate_latency.median,
        ),
        median_rss_delta_mib: (baseline_rss.median - candidate_rss.median) / MIB,
        median_rss_percent_improvement: percent_improvement(
            baseline_rss.median,
            candidate_rss.median,
        ),
    }
}

fn percent_improvement(baseline: f64, candidate: f64) -> f64 {
    if baseline > 0.0 {
        (baseline - candidate) / baseline * 100.0
    } else {
        0.0
    }
}

#[derive(Debug, Clone, Serialize)]
struct BinaryProvenance {
    path: PathBuf,
    sha256: String,
    identity: HelperIdentity,
}

fn binary_provenance(path: PathBuf, identity: HelperIdentity) -> Result<BinaryProvenance, String> {
    let metadata =
        fs::metadata(&path).map_err(|error| format!("metadata {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "benchmark binary is not a file: {}",
            path.display()
        ));
    }
    Ok(BinaryProvenance {
        sha256: sha256_file(&path)?,
        path,
        identity,
    })
}

#[derive(Debug, Clone, Serialize)]
struct SourceTreeProvenance {
    root: PathBuf,
    git_head: String,
    tracked_tree_sha256: String,
    full_repository_dirty: bool,
    full_git_status_sha256: String,
    relevant_dirty_paths: Vec<String>,
    source_files: Vec<SourceFile>,
}

#[derive(Debug, Clone, Serialize)]
struct SourceFile {
    relative_path: String,
    sha256: String,
}

const COMPARISON_SOURCE_FILES: &[&str] = &[
    "crates/proto/src/project.rs",
    "crates/proto/src/bin/montage-project-read-perf.rs",
    "crates/proto/build.rs",
    "crates/proto/Cargo.toml",
    "Cargo.toml",
    ".cargo/config.toml",
    "Cargo.lock",
];

fn inspect_source_tree(
    root: &Path,
    allow_dirty_source: bool,
) -> Result<SourceTreeProvenance, String> {
    let root = fs::canonicalize(root)
        .map_err(|error| format!("canonicalize source {}: {error}", root.display()))?;
    let git_head = git_stdout(&root, &["rev-parse", "HEAD"])?;
    let tracked_tree = git_stdout(&root, &["ls-tree", "-r", "--full-tree", "HEAD"])?;
    let full_status = git_stdout(&root, &["status", "--porcelain"])?;
    let mut status_args = vec!["status", "--porcelain", "--"];
    status_args.extend(COMPARISON_SOURCE_FILES.iter().copied());
    let relevant_status = git_stdout(&root, &status_args)?;
    let relevant_dirty_paths = porcelain_paths(&relevant_status);
    if !allow_dirty_source && !full_status.is_empty() {
        return Err(format!(
            "comparison source is dirty at {}; commit it or use --allow-dirty-source only for a smoke run: {}",
            root.display(),
            full_status.trim()
        ));
    }
    let source_files = COMPARISON_SOURCE_FILES
        .iter()
        .map(|relative_path| {
            let path = root.join(relative_path);
            Ok(SourceFile {
                relative_path: (*relative_path).into(),
                sha256: sha256_file(&path)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(SourceTreeProvenance {
        root,
        git_head,
        tracked_tree_sha256: sha256_bytes(tracked_tree.as_bytes()),
        full_repository_dirty: !full_status.is_empty(),
        full_git_status_sha256: sha256_bytes(full_status.as_bytes()),
        relevant_dirty_paths,
        source_files,
    })
}

#[derive(Debug, Clone, Serialize)]
struct SourceSnapshotComparison {
    differing_tracked_paths: Vec<String>,
    qualifying_policy: String,
}

fn verify_source_snapshots(
    args: &ControllerArgs,
    baseline: &SourceTreeProvenance,
    candidate: &SourceTreeProvenance,
) -> Result<SourceSnapshotComparison, String> {
    let baseline_entries = tracked_tree_entries(&baseline.root)?;
    let candidate_entries = tracked_tree_entries(&candidate.root)?;
    let paths: BTreeSet<_> = baseline_entries
        .keys()
        .chain(candidate_entries.keys())
        .cloned()
        .collect();
    let differing_tracked_paths: Vec<String> = paths
        .into_iter()
        .filter(|path| baseline_entries.get(path) != candidate_entries.get(path))
        .collect();
    validate_source_snapshot_diff_paths(&differing_tracked_paths, args.smoke)?;
    Ok(SourceSnapshotComparison {
        differing_tracked_paths,
        qualifying_policy: if args.smoke {
            "smoke permits identical source snapshots or a project.rs-only difference".into()
        } else {
            "qualifying comparisons require tracked source snapshots to differ only in crates/proto/src/project.rs".into()
        },
    })
}

fn validate_source_snapshot_diff_paths(paths: &[String], smoke: bool) -> Result<(), String> {
    let project_read = "crates/proto/src/project.rs";
    let project_read_only = paths.len() == 1 && paths[0] == project_read;
    let valid = if smoke {
        paths.is_empty() || project_read_only
    } else {
        project_read_only
    };
    if valid {
        return Ok(());
    }
    let expected = if smoke {
        "no differences or only crates/proto/src/project.rs"
    } else {
        "only crates/proto/src/project.rs"
    };
    Err(format!(
        "baseline and candidate tracked source snapshots must differ in {expected}; found: {}",
        if paths.is_empty() {
            "no paths".into()
        } else {
            paths.join(", ")
        }
    ))
}

fn tracked_tree_entries(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let output = git_stdout(root, &["ls-tree", "-r", "--full-tree", "HEAD"])?;
    let mut entries = BTreeMap::new();
    for line in output.lines() {
        let (descriptor, path) = line
            .split_once('\t')
            .ok_or_else(|| format!("parse git tree entry in {}: {line}", root.display()))?;
        if entries.insert(path.into(), descriptor.into()).is_some() {
            return Err(format!(
                "duplicate tracked source path in {}: {path}",
                root.display()
            ));
        }
    }
    Ok(entries)
}

fn git_stdout(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("spawn git in {}: {error}", root.display()))?;
    if !output.status.success() {
        return Err(format!(
            "git {:?} in {} exited {}: {}",
            args,
            root.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

fn porcelain_paths(status: &str) -> Vec<String> {
    status
        .lines()
        .filter_map(|line| line.get(3..))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn verify_binary_matches_source(
    arm: &str,
    binary: &BinaryProvenance,
    source: &SourceTreeProvenance,
) -> Result<(), String> {
    for (relative_path, compiled_hash) in [
        (
            "crates/proto/src/project.rs",
            &binary.identity.project_source_sha256,
        ),
        (
            "crates/proto/src/bin/montage-project-read-perf.rs",
            &binary.identity.benchmark_source_sha256,
        ),
        (
            "crates/proto/build.rs",
            &binary.identity.build_script_source_sha256,
        ),
        (
            "crates/proto/Cargo.toml",
            &binary.identity.proto_cargo_toml_sha256,
        ),
        ("Cargo.toml", &binary.identity.workspace_cargo_toml_sha256),
        (".cargo/config.toml", &binary.identity.cargo_config_sha256),
        ("Cargo.lock", &binary.identity.cargo_lock_sha256),
    ] {
        let actual = source
            .source_files
            .iter()
            .find(|file| file.relative_path == relative_path)
            .map(|file| &file.sha256)
            .ok_or_else(|| format!("{arm} source is missing {relative_path}"))?;
        if actual != compiled_hash {
            return Err(format!(
                "{arm} binary source mismatch for {relative_path}: supplied source tree does not match the prebuilt binary"
            ));
        }
    }
    Ok(())
}

fn verify_comparison(
    args: &ControllerArgs,
    controller: &BinaryProvenance,
    baseline: &BinaryProvenance,
    candidate: &BinaryProvenance,
) -> Result<(), String> {
    verify_helper_identity_pair(&baseline.identity, &candidate.identity)?;
    if controller.sha256 != candidate.sha256 {
        return Err("controller must have the exact candidate binary SHA-256".into());
    }
    if controller.identity != candidate.identity {
        return Err("controller must be the exact candidate binary so fixture writing provenance matches the candidate".into());
    }
    if !args.smoke
        && (baseline.identity.profile != "release" || candidate.identity.profile != "release")
    {
        return Err(
            "a qualifying comparison requires release baseline and candidate binaries".into(),
        );
    }
    if !args.smoke && baseline.sha256 == candidate.sha256 {
        return Err("baseline and candidate binaries are identical; use --smoke for a baseline-vs-itself exercise".into());
    }
    Ok(())
}

fn verify_helper_identity_pair(
    baseline: &HelperIdentity,
    candidate: &HelperIdentity,
) -> Result<(), String> {
    let identities = [baseline, candidate];
    if identities
        .iter()
        .any(|identity| identity.protocol != PROTOCOL)
    {
        return Err(
            "baseline or candidate does not implement the required project-read helper protocol"
                .into(),
        );
    }
    if baseline.benchmark_source_sha256 != candidate.benchmark_source_sha256
        || baseline.proto_cargo_toml_sha256 != candidate.proto_cargo_toml_sha256
        || baseline.build_script_source_sha256 != candidate.build_script_source_sha256
        || baseline.workspace_cargo_toml_sha256 != candidate.workspace_cargo_toml_sha256
        || baseline.cargo_lock_sha256 != candidate.cargo_lock_sha256
        || baseline.target_os != candidate.target_os
        || baseline.target_arch != candidate.target_arch
        || baseline.profile != candidate.profile
        || baseline.cargo_optimization_level != candidate.cargo_optimization_level
        || baseline.cargo_debug != candidate.cargo_debug
        || baseline.cargo_target != candidate.cargo_target
        || baseline.cargo_rustc_version_verbose != candidate.cargo_rustc_version_verbose
        || baseline.build_environment_sha256 != candidate.build_environment_sha256
        || baseline.cargo_config_sha256 != candidate.cargo_config_sha256
    {
        return Err("baseline and candidate have incomparable harness, lockfile, target, or profile provenance".into());
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct Provenance {
    controller: BinaryProvenance,
    baseline: BinaryProvenance,
    candidate: BinaryProvenance,
    baseline_source: SourceTreeProvenance,
    candidate_source: SourceTreeProvenance,
    source_snapshot: SourceSnapshotComparison,
    helper_environment: HelperEnvironment,
    tools: ToolProvenance,
    machine: Machine,
}

#[derive(Debug, Clone, Serialize)]
struct HelperEnvironment {
    lc_all: String,
    tz: String,
}

#[derive(Debug, Serialize)]
struct ToolProvenance {
    time_path: String,
    time_sha256: String,
    rustc_version_verbose: String,
    cargo_version: String,
    macos_version: String,
}

fn tool_provenance() -> Result<ToolProvenance, String> {
    Ok(ToolProvenance {
        time_path: TIME_PATH.into(),
        time_sha256: sha256_file(Path::new(TIME_PATH))?,
        rustc_version_verbose: command_stdout("rustc", &["-Vv"])?,
        cargo_version: command_stdout("cargo", &["-V"])?,
        macos_version: command_stdout("sw_vers", &[])?,
    })
}

fn command_stdout(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("spawn {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!("{program} exited {}", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[derive(Debug, Serialize, Deserialize)]
struct Machine {
    os: String,
    arch: String,
    parallelism: usize,
    work_filesystem: Filesystem,
    report_filesystem: Filesystem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Filesystem {
    file_system_personality: String,
    device: String,
    mount: String,
}

fn require_apfs(path: &Path) -> Result<Filesystem, String> {
    let filesystem = filesystem(path)?;
    if !filesystem
        .file_system_personality
        .eq_ignore_ascii_case("APFS")
    {
        return Err(format!(
            "{} is on {}; this controller requires APFS",
            path.display(),
            filesystem.file_system_personality
        ));
    }
    Ok(filesystem)
}

fn filesystem(path: &Path) -> Result<Filesystem, String> {
    let df = Command::new("df")
        .args(["-P"])
        .arg(path)
        .output()
        .map_err(|error| format!("spawn df for {}: {error}", path.display()))?;
    if !df.status.success() {
        return Err(format!("df for {} exited {}", path.display(), df.status));
    }
    let line = String::from_utf8_lossy(&df.stdout)
        .lines()
        .last()
        .ok_or_else(|| format!("df produced no filesystem row for {}", path.display()))?
        .to_string();
    let fields: Vec<_> = line.split_whitespace().collect();
    if fields.len() < 6 {
        return Err(format!(
            "could not parse df row for {}: {line}",
            path.display()
        ));
    }
    let device = fields[0].to_string();
    let mount = fields[5..].join(" ");
    let diskutil = Command::new("diskutil")
        .args(["info", &mount])
        .output()
        .map_err(|error| format!("spawn diskutil for {mount}: {error}"))?;
    if !diskutil.status.success() {
        return Err(format!("diskutil info {mount} exited {}", diskutil.status));
    }
    let file_system_personality = String::from_utf8_lossy(&diskutil.stdout)
        .lines()
        .find_map(|line| line.trim().strip_prefix("File System Personality:"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("diskutil did not report filesystem type for {mount}"))?;
    Ok(Filesystem {
        file_system_personality,
        device,
        mount,
    })
}

fn create_unique_dir(root: &Path, prefix: &str) -> Result<PathBuf, String> {
    for attempt in 0..100_u32 {
        let path = root.join(format!("{prefix}-{}-{attempt}", unique_nonce()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("create {}: {error}", path.display())),
        }
    }
    Err(format!(
        "could not create a unique {prefix} directory under {}",
        root.display()
    ))
}

fn unique_nonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos}-{}-{counter}", std::process::id())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_atomically_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("no parent for {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    if path.exists() {
        return Err(format!("refusing to overwrite evidence {}", path.display()));
    }
    let temporary = parent.join(format!(
        ".{}-{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project-read-evidence"),
        unique_nonce()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("create {}: {error}", temporary.display()))?;
    file.write_all(bytes)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| {
        format!(
            "rename {} to {}: {error}",
            temporary.display(),
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_explicit_binary_and_source_pairs() {
        let error = ControllerArgs::parse(&[]).expect_err("missing comparison inputs must fail");
        assert!(error.contains("--baseline"));
    }

    #[test]
    fn parser_reserves_dirty_sources_for_smoke_and_rejects_unsafe_labels() {
        let mut dirty_escape = required_args();
        dirty_escape.push("--allow-dirty-source".into());
        let error = ControllerArgs::parse(&dirty_escape)
            .expect_err("only an explicit smoke may allow a dirty source");
        assert!(error.contains("--smoke"));

        let mut unsafe_label = required_args();
        unsafe_label.extend(["--label".into(), "not/a-token".into()]);
        let error =
            ControllerArgs::parse(&unsafe_label).expect_err("slash is unsafe in a report label");
        assert!(error.contains("--label"));

        let parsed = ControllerArgs::parse(&required_args()).expect("parse defaults");
        assert_eq!(parsed.clip_counts, DEFAULT_CLIP_COUNTS);
        assert_eq!(parsed.warmups, DEFAULT_WARMUPS);
        assert_eq!(parsed.samples, DEFAULT_SAMPLES);
    }

    #[test]
    fn executable_paths_must_be_absolute_before_a_helper_can_spawn() {
        let error = canonical_executable(Path::new("montage-project-read-perf"), "candidate")
            .expect_err("a bare command name must not be PATH-searched");
        assert!(error.contains("absolute"));
    }

    #[test]
    fn verifier_requires_exactly_two_report_paths() {
        let error = VerifyReportsArgs::parse(&["--verify-reports".into(), "/one.json".into()])
            .expect_err("one report cannot establish independent evidence");
        assert!(error.contains("two report paths"));
    }

    #[test]
    fn verifier_accepts_two_distinct_matching_full_report_summaries() {
        let root = unique_test_dir("verifier");
        let first_path = root.join("first.json");
        let second_path = root.join("second.json");
        let artifacts = verification_artifacts(&root);
        let first = serde_json::to_vec(&verification_report(
            "report-one",
            "session-one",
            "2026-08-04T00:00:01Z",
            80.0,
            &artifacts,
        ))
        .expect("serialize first verifier report");
        let second = serde_json::to_vec(&verification_report(
            "report-two",
            "session-two",
            "2026-08-04T00:00:02Z",
            79.0,
            &artifacts,
        ))
        .expect("serialize second verifier report");
        write_atomically_new(&first_path, &first).expect("write first verifier report");
        write_atomically_new(&second_path, &second).expect("write second verifier report");

        verify_reports_main(&[
            "--verify-reports".into(),
            first_path.display().to_string(),
            second_path.display().to_string(),
        ])
        .expect("two distinct matching full reports must aggregate");
        fs::remove_dir_all(&root).expect("remove verifier test root");
    }

    #[test]
    fn verifier_rejects_reports_from_different_machines() {
        let root = unique_test_dir("machine-verifier");
        let first_path = root.join("first.json");
        let second_path = root.join("second.json");
        let artifacts = verification_artifacts(&root);
        let first = verification_report(
            "report-one",
            "session-one",
            "2026-08-04T00:00:01Z",
            80.0,
            &artifacts,
        );
        let mut second = verification_report(
            "report-two",
            "session-two",
            "2026-08-04T00:00:02Z",
            79.0,
            &artifacts,
        );
        second.provenance.machine.parallelism = 10;
        write_atomically_new(
            &first_path,
            &serde_json::to_vec(&first).expect("serialize first report"),
        )
        .expect("write first report");
        write_atomically_new(
            &second_path,
            &serde_json::to_vec(&second).expect("serialize second report"),
        )
        .expect("write second report");

        let error = verify_reports_main(&[
            "--verify-reports".into(),
            first_path.display().to_string(),
            second_path.display().to_string(),
        ])
        .expect_err("reports from different machines must not establish acceptance");
        assert!(error.contains("matching binary, source, and methodology provenance"));
        fs::remove_dir_all(&root).expect("remove machine-verifier test root");
    }

    #[test]
    fn verifier_rejects_cosmetically_edited_copied_measurements() {
        let root = unique_test_dir("copied-verifier-evidence");
        let artifacts = verification_artifacts(&root);
        let first = verification_report(
            "report-one",
            "session-one",
            "2026-08-04T00:00:01Z",
            80.0,
            &artifacts,
        );
        let mut second = verification_report(
            "report-two",
            "session-two",
            "2026-08-04T00:00:02Z",
            79.0,
            &artifacts,
        );
        second.fixtures = first.fixtures.clone();
        second.fixtures[0].fixture.root = PathBuf::from("/cosmetically-edited-root");
        second.fixtures[0].baseline.timed_samples[0].time_l_stderr_sha256 = "b".repeat(64);
        let first_path = root.join("first.json");
        let second_path = root.join("second.json");
        write_atomically_new(
            &first_path,
            &serde_json::to_vec(&first).expect("serialize first report"),
        )
        .expect("write first report");
        write_atomically_new(
            &second_path,
            &serde_json::to_vec(&second).expect("serialize second report"),
        )
        .expect("write second report");
        let error = verify_reports_main(&[
            "--verify-reports".into(),
            first_path.display().to_string(),
            second_path.display().to_string(),
        ])
        .expect_err("cosmetically edited copied measurements must not establish independence");
        assert!(error.contains("raw fixture evidence"));
        fs::remove_dir_all(&root).expect("remove copied-evidence root");
    }

    #[test]
    fn verifier_rejects_a_report_without_a_project_source_delta() {
        let root = unique_test_dir("same-project-source");
        let artifacts = verification_artifacts(&root);
        let mut report = verification_report(
            "report-one",
            "session-one",
            "2026-08-04T00:00:01Z",
            80.0,
            &artifacts,
        );
        report.provenance.baseline.identity.project_source_sha256 = report
            .provenance
            .candidate
            .identity
            .project_source_sha256
            .clone();
        let error = validate_report_for_verification(&report, "synthetic")
            .expect_err("identical project source hashes must fail closed");
        assert!(error.contains("distinct baseline and candidate project sources"));
        fs::remove_dir_all(&root).expect("remove same-project-source root");
    }

    #[test]
    fn helper_identity_rejects_different_build_settings() {
        let baseline = compiled_identity();
        let mut candidate = baseline.clone();
        candidate.build_environment_sha256 = "different-build-settings".into();
        assert!(verify_helper_identity_pair(&baseline, &candidate).is_err());
    }

    #[test]
    fn qualifying_source_snapshots_may_differ_only_in_project_read() {
        assert!(
            validate_source_snapshot_diff_paths(&["crates/proto/src/project.rs".into()], false,)
                .is_ok()
        );
        assert!(validate_source_snapshot_diff_paths(&[], false).is_err());
        assert!(validate_source_snapshot_diff_paths(&["Cargo.lock".into()], false).is_err());
        assert!(validate_source_snapshot_diff_paths(&[], true).is_ok());
    }

    #[test]
    fn helper_identity_uses_the_exact_cargo_profile_embedded_by_build_script() {
        assert_eq!(compiled_identity().profile, BUILD_CARGO_PROFILE);
    }

    #[test]
    fn porcelain_parser_keeps_the_first_dirty_path() {
        assert_eq!(
            porcelain_paths(" M Cargo.lock\n?? crates/proto/src/bin/\n"),
            vec!["Cargo.lock", "crates/proto/src/bin/"]
        );
    }

    #[test]
    fn summary_uses_nearest_rank_p95_and_median_absolute_deviation() {
        let summary = summarize_distribution(&[1.0, 3.0, 5.0, 7.0, 100.0]);
        assert_eq!(summary.median, 5.0);
        assert_eq!(summary.p95, 100.0);
        assert_eq!(summary.mad, 2.0);
        assert_eq!(summary.min, 1.0);
        assert_eq!(summary.max, 100.0);
    }

    #[test]
    fn alternates_baseline_and_candidate_order_by_round() {
        assert_eq!(alternating_arms(0), ["baseline", "candidate"]);
        assert_eq!(alternating_arms(1), ["candidate", "baseline"]);
    }

    #[test]
    fn acceptance_requires_5k_gain_and_no_p95_regression_at_every_required_size() {
        let five_k_only = vec![gate_run(
            5_000,
            100.0,
            120.0,
            100.0 * MIB,
            80.0,
            120.0,
            100.0 * MIB,
        )];
        assert!(!evaluate_acceptance(&five_k_only, false).single_report_gate_passed);

        let one_k_only = vec![
            gate_run(100, 100.0, 120.0, 100.0 * MIB, 100.0, 120.0, 100.0 * MIB),
            gate_run(1_000, 100.0, 120.0, 100.0 * MIB, 80.0, 120.0, 100.0 * MIB),
            gate_run(5_000, 100.0, 120.0, 100.0 * MIB, 100.0, 120.0, 100.0 * MIB),
        ];
        assert!(!evaluate_acceptance(&one_k_only, false).single_report_gate_passed);

        let p95_regression = vec![
            gate_run(100, 100.0, 120.0, 100.0 * MIB, 100.0, 121.0, 100.0 * MIB),
            gate_run(1_000, 100.0, 120.0, 100.0 * MIB, 100.0, 120.0, 100.0 * MIB),
            gate_run(5_000, 100.0, 120.0, 100.0 * MIB, 80.0, 120.0, 100.0 * MIB),
        ];
        let rejected = evaluate_acceptance(&p95_regression, false);
        assert!(!rejected.no_p95_regression_at_any_size);
        assert!(!rejected.single_report_gate_passed);

        let qualifying = vec![
            gate_run(100, 100.0, 120.0, 100.0 * MIB, 100.0, 120.0, 100.0 * MIB),
            gate_run(1_000, 100.0, 120.0, 100.0 * MIB, 95.0, 120.0, 100.0 * MIB),
            gate_run(5_000, 100.0, 120.0, 100.0 * MIB, 80.0, 120.0, 100.0 * MIB),
        ];
        assert!(evaluate_acceptance(&qualifying, false).single_report_gate_passed);

        let rss_qualifying = vec![
            gate_run(100, 100.0, 120.0, 100.0 * MIB, 100.0, 120.0, 100.0 * MIB),
            gate_run(1_000, 100.0, 120.0, 100.0 * MIB, 100.0, 120.0, 100.0 * MIB),
            gate_run(5_000, 100.0, 120.0, 100.0 * MIB, 100.0, 120.0, 70.0 * MIB),
        ];
        let rss_acceptance = evaluate_acceptance(&rss_qualifying, false);
        assert!(rss_acceptance.five_k_rss.passed);
        assert!(rss_acceptance.single_report_gate_passed);
    }

    #[test]
    fn acceptance_honors_exact_latency_and_rss_threshold_boundaries() {
        let exact_latency = qualifying_runs(90.0, 90.0, 100.0 * MIB);
        let latency_acceptance = evaluate_acceptance(&exact_latency, false);
        assert!(latency_acceptance.five_k_latency.passed);

        let just_short_latency = qualifying_runs(90.01, 90.01, 100.0 * MIB);
        assert!(
            !evaluate_acceptance(&just_short_latency, false)
                .five_k_latency
                .passed
        );

        let exact_rss = qualifying_runs(100.0, 100.0, 30.0 * MIB);
        let rss_acceptance = evaluate_acceptance(&exact_rss, false);
        assert!(rss_acceptance.five_k_rss.passed);

        let just_short_rss = qualifying_runs(100.0, 100.0, 30.01 * MIB);
        assert!(
            !evaluate_acceptance(&just_short_rss, false)
                .five_k_rss
                .passed
        );
    }

    #[test]
    fn production_written_fixture_is_deterministic_and_has_requested_clip_count() {
        let root = unique_test_dir("fixture");
        let first = write_fixture(&root.join("first"), 3).expect("write first fixture");
        let second = write_fixture(&root.join("second"), 3).expect("write second fixture");

        assert_eq!(first.fingerprint.sha256, second.fingerprint.sha256);
        assert_eq!(first.clip_count, 3);
        let witness = read_witness(&first.root);
        validate_full_fixture_witness("test", 3, &witness).expect("full typed fixture witness");
        assert_eq!(witness.timeline_clip_count, 3);
        assert!(witness.typed_edit_plan_signature.is_some());
        assert!(witness.typed_manifest_signature.is_some());
        let project = Project::read(&first.root).expect("read deterministic production fixture");
        assert_eq!(project.edit_plan.items.len(), 1);
        assert_eq!(
            project.manifest.expect("fixture manifest").indexers.len(),
            1
        );
        fs::remove_dir_all(&root).expect("remove test fixture root");
    }

    #[test]
    fn contract_fixtures_preserve_forward_recovery_and_hard_failure_signatures() {
        let root = unique_test_dir("contracts");
        let contracts = write_contract_fixtures(&root).expect("write contracts");

        let valid = read_witness(&contracts.valid.root);
        assert_eq!(valid.outcome, "ok");
        assert_eq!(valid.timeline_clip_count, 2);
        assert!(valid.warnings.is_empty());

        let forward = read_witness(&contracts.forward_clip_99.root);
        assert_eq!(forward.outcome, "ok");
        assert_eq!(forward.timeline_clip_count, 2);
        assert_eq!(
            forward.warnings,
            vec!["Clip.99|tracks.children[0].children[0]|2|99".to_string()]
        );

        let recovered = read_witness(&contracts.recoverable_marker.root);
        assert_eq!(recovered.outcome, "ok");
        assert_eq!(recovered.timeline_marker_count, 0);

        let unknown = read_witness(&contracts.unknown_schema.root);
        assert_eq!(unknown.outcome, "error");
        assert_eq!(
            unknown.error,
            Some("UnknownOtioSchema|UnknownNode.1|tracks.children[0]".to_string())
        );
        fs::remove_dir_all(&root).expect("remove test contract root");
    }

    fn unique_test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("montage-project-read-{label}-{}", unique_nonce()))
    }

    fn required_args() -> Vec<String> {
        [
            "--baseline",
            "/baseline",
            "--candidate",
            "/candidate",
            "--baseline-source",
            "/baseline-source",
            "--candidate-source",
            "/candidate-source",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    fn gate_run(
        clip_count: usize,
        baseline_median_ms: f64,
        baseline_p95_ms: f64,
        baseline_rss: f64,
        candidate_median_ms: f64,
        candidate_p95_ms: f64,
        candidate_rss: f64,
    ) -> FixtureRun {
        let fingerprint = InputFingerprint {
            sha256: "fixture".into(),
            files: Vec::new(),
        };
        let full_witness = valid_full_witness(clip_count);
        FixtureRun {
            fixture: Fixture {
                root: PathBuf::from("/fixture"),
                clip_count,
                fingerprint: fingerprint.clone(),
            },
            final_input_fingerprint: fingerprint,
            baseline: gate_arm(baseline_median_ms, baseline_p95_ms, baseline_rss),
            candidate: gate_arm(candidate_median_ms, candidate_p95_ms, candidate_rss),
            compact_witness: CompactWitness {
                outcome: "ok".into(),
                timeline_schema: Some("Timeline.1".into()),
                timeline_name: Some("project-read-fixture".into()),
                timeline_track_count: 1,
                timeline_clip_count: clip_count,
                timeline_marker_count: 0,
                edit_plan_version: Some(montage_proto::EDIT_PLAN_VERSION.into()),
                edit_plan_item_count: 1,
                manifest_version: Some(montage_proto::INDEX_MANIFEST_VERSION.into()),
                manifest_indexer_count: 1,
                manifest_asset_count: 2,
                warnings: Vec::new(),
                error: None,
            },
            full_integrity: FullFixtureIntegrity {
                baseline: full_witness.clone(),
                candidate: full_witness,
            },
        }
    }

    fn valid_full_witness(clip_count: usize) -> ContractWitness {
        ContractWitness {
            outcome: "ok".into(),
            typed_timeline_signature: Some(format!("timeline-{clip_count}")),
            typed_edit_plan_signature: Some("edit-plan".into()),
            typed_manifest_signature: Some("manifest".into()),
            timeline_clip_count: clip_count,
            timeline_marker_count: 0,
            warnings: Vec::new(),
            warnings_signature: canonical_signature(&Value::Array(Vec::new())),
            error: None,
        }
    }

    fn gate_arm(median_ms: f64, p95_ms: f64, rss: f64) -> ArmResults {
        ArmResults {
            warmups: Vec::new(),
            timed_samples: Vec::new(),
            child_wall_ms: Distribution {
                median: median_ms,
                p95: p95_ms,
                mad: 0.0,
                min: median_ms,
                max: p95_ms,
            },
            time_l_peak_rss_bytes: Distribution {
                median: rss,
                p95: rss,
                mad: 0.0,
                min: rss,
                max: rss,
            },
        }
    }

    fn qualifying_runs(
        candidate_median_ms: f64,
        candidate_p95_ms: f64,
        candidate_rss: f64,
    ) -> Vec<FixtureRun> {
        vec![
            gate_run(100, 100.0, 100.0, 100.0 * MIB, 100.0, 100.0, 100.0 * MIB),
            gate_run(1_000, 100.0, 100.0, 100.0 * MIB, 100.0, 100.0, 100.0 * MIB),
            gate_run(
                5_000,
                100.0,
                100.0,
                40.0 * MIB,
                candidate_median_ms,
                candidate_p95_ms,
                candidate_rss,
            ),
        ]
    }

    fn verification_report(
        report_id: &str,
        session_id: &str,
        generated_at_utc: &str,
        candidate_median_ms: f64,
        artifacts: &VerificationArtifacts,
    ) -> VerificationReport {
        VerificationReport {
            schema_version: 1,
            protocol: PROTOCOL.into(),
            report_id: report_id.into(),
            session_id: session_id.into(),
            generated_at_utc: generated_at_utc.into(),
            configuration: VerificationConfiguration {
                clip_counts: DEFAULT_CLIP_COUNTS.to_vec(),
                warmups: DEFAULT_WARMUPS,
                samples: DEFAULT_SAMPLES,
                smoke: false,
                allow_dirty_source: false,
            },
            provenance: VerificationProvenance {
                controller: artifacts.candidate_binary.clone(),
                baseline: artifacts.baseline_binary.clone(),
                candidate: artifacts.candidate_binary.clone(),
                baseline_source: artifacts.baseline_source.clone(),
                candidate_source: artifacts.candidate_source.clone(),
                source_snapshot: VerificationSourceSnapshot {
                    differing_tracked_paths: vec!["crates/proto/src/project.rs".into()],
                    qualifying_policy: "project.rs only".into(),
                },
                helper_environment: VerificationHelperEnvironment {
                    lc_all: HELPER_LC_ALL.into(),
                    tz: HELPER_TZ.into(),
                },
                tools: VerificationToolProvenance {
                    time_path: TIME_PATH.into(),
                    time_sha256: "time-binary".into(),
                    rustc_version_verbose: "rustc".into(),
                    cargo_version: "cargo".into(),
                    macos_version: "macOS".into(),
                },
                machine: Machine {
                    os: "macos".into(),
                    arch: "aarch64".into(),
                    parallelism: 8,
                    work_filesystem: Filesystem {
                        file_system_personality: "APFS".into(),
                        device: "/dev/disk1s1".into(),
                        mount: "/".into(),
                    },
                    report_filesystem: Filesystem {
                        file_system_personality: "APFS".into(),
                        device: "/dev/disk2s1".into(),
                        mount: "/Volumes/Benchmarks".into(),
                    },
                },
            },
            contracts: verification_contract_report(),
            fixtures: verification_runs(report_id, candidate_median_ms),
            acceptance: VerificationAcceptance {
                single_report_gate_passed: true,
                program_acceptance: false,
            },
        }
    }

    struct VerificationArtifacts {
        baseline_binary: VerificationBinary,
        candidate_binary: VerificationBinary,
        baseline_source: VerificationSourceTree,
        candidate_source: VerificationSourceTree,
    }

    fn verification_artifacts(root: &Path) -> VerificationArtifacts {
        let mut candidate_identity = compiled_identity();
        candidate_identity.profile = "release".into();
        let mut baseline_identity = candidate_identity.clone();
        let baseline_project = b"baseline project source";
        baseline_identity.project_source_sha256 = sha256_bytes(baseline_project);
        let baseline_source = write_verification_source(
            &root.join("baseline-source"),
            &baseline_identity,
            baseline_project,
            "baseline-head",
            "baseline-tree",
        );
        let candidate_source = write_verification_source(
            &root.join("candidate-source"),
            &candidate_identity,
            include_bytes!("../project.rs"),
            "candidate-head",
            "candidate-tree",
        );
        VerificationArtifacts {
            baseline_binary: write_verification_binary(
                &root.join("baseline-helper"),
                baseline_identity,
            ),
            candidate_binary: write_verification_binary(
                &root.join("candidate-helper"),
                candidate_identity,
            ),
            baseline_source,
            candidate_source,
        }
    }

    fn write_verification_binary(path: &Path, identity: HelperIdentity) -> VerificationBinary {
        let identity_json = serde_json::to_string(&identity).expect("serialize helper identity");
        let identity_shell = identity_json.replace('\'', "'\\''");
        let script = format!(
            "#!/bin/sh\noutput=''\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = '--output' ]; then shift; output=$1; fi\n  shift\ndone\n[ -n \"$output\" ] || exit 2\n/usr/bin/printf '%s' '{identity_shell}' > \"$output\"\n"
        );
        fs::write(path, script).expect("write verification binary");
        let mut permissions = fs::metadata(path)
            .expect("verification binary metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("make verification binary executable");
        let path = fs::canonicalize(path).expect("canonicalize verification binary");
        VerificationBinary {
            sha256: sha256_file(&path).expect("hash verification binary"),
            path,
            identity,
        }
    }

    fn write_verification_source(
        root: &Path,
        identity: &HelperIdentity,
        project_source: &[u8],
        git_head: &str,
        tracked_tree_sha256: &str,
    ) -> VerificationSourceTree {
        let files: [(&str, &[u8]); 7] = [
            ("crates/proto/src/project.rs", project_source),
            (
                "crates/proto/src/bin/montage-project-read-perf.rs",
                include_bytes!("montage-project-read-perf.rs"),
            ),
            ("crates/proto/build.rs", include_bytes!("../../build.rs")),
            (
                "crates/proto/Cargo.toml",
                include_bytes!("../../Cargo.toml"),
            ),
            ("Cargo.toml", include_bytes!("../../../../Cargo.toml")),
            (
                ".cargo/config.toml",
                include_bytes!("../../../../.cargo/config.toml"),
            ),
            ("Cargo.lock", include_bytes!("../../../../Cargo.lock")),
        ];
        for (relative_path, bytes) in files {
            let path = root.join(relative_path);
            fs::create_dir_all(path.parent().expect("verification source parent"))
                .expect("create verification source parent");
            fs::write(path, bytes).expect("write verification source file");
        }
        VerificationSourceTree {
            root: fs::canonicalize(root).expect("canonicalize verification source"),
            git_head: git_head.into(),
            tracked_tree_sha256: tracked_tree_sha256.into(),
            full_repository_dirty: false,
            full_git_status_sha256: sha256_bytes(b""),
            relevant_dirty_paths: Vec::new(),
            source_files: verification_source_files(identity),
        }
    }

    fn verification_runs(report_id: &str, candidate_median_ms: f64) -> Vec<FixtureRun> {
        let mut runs = qualifying_runs(candidate_median_ms, 100.0, 40.0 * MIB);
        let mut sequence = 0_u64;
        for run in &mut runs {
            run.fixture.root =
                PathBuf::from(format!("/fixture/{report_id}/{}", run.fixture.clip_count));
            run.fixture.fingerprint.sha256 = format!("fixture-{report_id}");
            run.final_input_fingerprint = run.fixture.fingerprint.clone();
            populate_verification_run(run, &mut sequence);
        }
        runs
    }

    fn populate_verification_run(run: &mut FixtureRun, sequence: &mut u64) {
        let mut ignored_sequence = 0_u64;
        populate_verification_arm(
            &mut run.baseline,
            "baseline",
            &run.compact_witness,
            &mut ignored_sequence,
        );
        populate_verification_arm(
            &mut run.candidate,
            "candidate",
            &run.compact_witness,
            &mut ignored_sequence,
        );
        for round in 0..(DEFAULT_WARMUPS + DEFAULT_SAMPLES) {
            let (phase, phase_round) = if round < DEFAULT_WARMUPS {
                ("warmup", round)
            } else {
                ("timed", round - DEFAULT_WARMUPS)
            };
            for arm_name in alternating_arms(round) {
                let arm = if arm_name == "baseline" {
                    &mut run.baseline
                } else {
                    &mut run.candidate
                };
                let sample = if phase == "warmup" {
                    &mut arm.warmups[phase_round]
                } else {
                    &mut arm.timed_samples[phase_round]
                };
                sample.sequence = *sequence;
                *sequence += 1;
            }
        }
    }

    fn populate_verification_arm(
        arm: &mut ArmResults,
        arm_name: &str,
        witness: &CompactWitness,
        sequence: &mut u64,
    ) {
        let sample = |phase: &str, round: usize, child_wall_ms: f64, sequence: &mut u64| {
            let sample = Sample {
                sequence: *sequence,
                arm: arm_name.into(),
                phase: phase.into(),
                round,
                child_wall_ms,
                time_l_real_ms: 0.0,
                time_l_peak_rss_bytes: arm.time_l_peak_rss_bytes.median as u64,
                time_l_stderr_sha256: "a".repeat(64),
                witness: witness.clone(),
            };
            *sequence += 1;
            sample
        };
        arm.warmups = (0..DEFAULT_WARMUPS)
            .map(|round| sample("warmup", round, arm.child_wall_ms.median, sequence))
            .collect();
        arm.timed_samples = (0..DEFAULT_SAMPLES)
            .map(|round| {
                let wall = if round + 1 == DEFAULT_SAMPLES {
                    arm.child_wall_ms.p95
                } else {
                    arm.child_wall_ms.median
                };
                sample("timed", round, wall, sequence)
            })
            .collect();
    }

    fn verification_contract_report() -> ContractReport {
        let valid = valid_full_witness(2);
        let mut forward = valid.clone();
        forward.warnings = vec!["Clip.99|tracks.children[0].children[0]|2|99".into()];
        forward.warnings_signature = canonical_signature(&Value::Array(
            forward
                .warnings
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        ));
        let recovered = valid.clone();
        let unknown = ContractWitness {
            outcome: "error".into(),
            typed_timeline_signature: None,
            typed_edit_plan_signature: None,
            typed_manifest_signature: None,
            timeline_clip_count: 0,
            timeline_marker_count: 0,
            warnings: Vec::new(),
            warnings_signature: canonical_signature(&Value::Array(Vec::new())),
            error: Some("UnknownOtioSchema|UnknownNode.1|tracks.children[0]".into()),
        };
        let witnesses = ContractWitnesses {
            valid,
            forward_clip_99: forward,
            recoverable_marker: recovered,
            unknown_schema: unknown,
        };
        let fingerprint = InputFingerprint {
            sha256: "contract-fixture".into(),
            files: Vec::new(),
        };
        ContractReport {
            fixtures: ContractFixtureFingerprints {
                valid: fingerprint.clone(),
                forward_clip_99: fingerprint.clone(),
                recoverable_marker: fingerprint.clone(),
                unknown_schema: fingerprint,
            },
            baseline: witnesses.clone(),
            candidate: witnesses,
        }
    }

    fn verification_source_files(identity: &HelperIdentity) -> Vec<VerificationSourceFile> {
        vec![
            VerificationSourceFile {
                relative_path: "crates/proto/src/project.rs".into(),
                sha256: identity.project_source_sha256.clone(),
            },
            VerificationSourceFile {
                relative_path: "crates/proto/src/bin/montage-project-read-perf.rs".into(),
                sha256: identity.benchmark_source_sha256.clone(),
            },
            VerificationSourceFile {
                relative_path: "crates/proto/build.rs".into(),
                sha256: identity.build_script_source_sha256.clone(),
            },
            VerificationSourceFile {
                relative_path: "crates/proto/Cargo.toml".into(),
                sha256: identity.proto_cargo_toml_sha256.clone(),
            },
            VerificationSourceFile {
                relative_path: "Cargo.toml".into(),
                sha256: identity.workspace_cargo_toml_sha256.clone(),
            },
            VerificationSourceFile {
                relative_path: ".cargo/config.toml".into(),
                sha256: identity.cargo_config_sha256.clone(),
            },
            VerificationSourceFile {
                relative_path: "Cargo.lock".into(),
                sha256: identity.cargo_lock_sha256.clone(),
            },
        ]
    }
}
