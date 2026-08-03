#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use montage_config::{IndexerResourceClass, McpServer, McpServerKind};
use montage_index::{AssetInput, PairOutcome, asset_fingerprint, run, sidecar_path};
use montage_mcp::ClientInfo;
use montage_proto::index::AssetId;
use serde::{Deserialize, Serialize};

fn main() {
    if let Err(error) = tokio::runtime::Runtime::new()
        .and_then(|runtime| runtime.block_on(real_main()).map_err(std::io::Error::other))
    {
        eprintln!("montage-index-skip-perf: {error}");
        std::process::exit(1);
    }
}

async fn real_main() -> Result<(), String> {
    let args = Args::parse(std::env::args().skip(1).collect())?;
    let fixture = prepare_fixture(
        &args.work_dir,
        FixtureConfig {
            assets: args.assets,
            indexers: args.indexers,
            sidecar_mib: args.sidecar_mib,
        },
    )?;

    for _ in 0..args.warmups {
        dispatch_fixture(&fixture).await?;
    }

    let mut samples_ms = Vec::with_capacity(args.samples);
    let mut correctness = None;
    for _ in 0..args.samples {
        let started = Instant::now();
        let sample_correctness = dispatch_fixture(&fixture).await?;
        samples_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        correctness = Some(sample_correctness);
    }

    let provenance = report_provenance(&fixture.root, args.warmups);
    let report = BenchmarkReport {
        configuration: Configuration {
            label: args.label,
            assets: args.assets,
            indexers: args.indexers,
            sidecar_mib: args.sidecar_mib,
            warmups: args.warmups,
            samples: args.samples,
            fixture_root: provenance.fixture_root,
            cache_state: provenance.cache_state,
        },
        machine: Machine {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            parallelism: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
            build_profile: provenance.build_profile,
        },
        correctness: correctness.ok_or_else(|| "no benchmark samples ran".to_string())?,
        statistics: summarize_samples(&samples_ms),
        samples_ms,
    };
    let json =
        serde_json::to_vec_pretty(&report).map_err(|error| format!("serialize report: {error}"))?;
    write_report_atomically(&args.output, &json)?;
    println!("{}", args.output.display());
    Ok(())
}

#[derive(Debug)]
struct Args {
    work_dir: PathBuf,
    output: PathBuf,
    label: String,
    assets: usize,
    indexers: usize,
    sidecar_mib: usize,
    warmups: usize,
    samples: usize,
}

impl Args {
    fn parse(raw: Vec<String>) -> Result<Self, String> {
        let mut args = Self {
            work_dir: std::env::temp_dir().join("montage-index-skip-perf"),
            output: std::env::temp_dir().join("montage-index-skip-perf.json"),
            label: "default".into(),
            assets: 4,
            indexers: 2,
            sidecar_mib: 1,
            warmups: 1,
            samples: 5,
        };
        let mut index = 0;
        while index < raw.len() {
            let flag = &raw[index];
            index += 1;
            let value = |flag: &str| {
                raw.get(index)
                    .cloned()
                    .ok_or_else(|| format!("{flag} requires a value"))
            };
            match flag.as_str() {
                "--work-dir" => args.work_dir = PathBuf::from(value("--work-dir")?),
                "--output" => args.output = PathBuf::from(value("--output")?),
                "--label" => args.label = value("--label")?,
                "--assets" => args.assets = parse_positive("--assets", &value("--assets")?)?,
                "--indexers" => {
                    args.indexers = parse_positive("--indexers", &value("--indexers")?)?
                }
                "--sidecar-mib" => {
                    args.sidecar_mib = parse_positive("--sidecar-mib", &value("--sidecar-mib")?)?
                }
                "--warmups" => args.warmups = parse_positive("--warmups", &value("--warmups")?)?,
                "--samples" => args.samples = parse_positive("--samples", &value("--samples")?)?,
                "-h" | "--help" => return Err(usage()),
                other => return Err(format!("unknown argument {other:?}\n{}", usage())),
            }
            if matches!(
                flag.as_str(),
                "--work-dir"
                    | "--output"
                    | "--label"
                    | "--assets"
                    | "--indexers"
                    | "--sidecar-mib"
                    | "--warmups"
                    | "--samples"
            ) {
                index += 1;
            }
        }
        Ok(args)
    }
}

fn parse_positive(flag: &str, value: &str) -> Result<usize, String> {
    let count = value
        .parse::<usize>()
        .map_err(|error| format!("invalid {flag} {value:?}: {error}"))?;
    if count == 0 {
        return Err(format!("{flag} must be greater than zero"));
    }
    Ok(count)
}

fn usage() -> String {
    "usage: montage-index-skip-perf [--work-dir <dir>] [--output <file>] [--label <name>] [--assets <count>] [--indexers <count>] [--sidecar-mib <count>] [--warmups <count>] [--samples <count>]".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FixtureConfig {
    assets: usize,
    indexers: usize,
    sidecar_mib: usize,
}

struct Fixture {
    root: PathBuf,
    config: FixtureConfig,
    assets: Vec<AssetInput>,
    servers: Vec<McpServer>,
}

fn prepare_fixture(work_dir: &Path, config: FixtureConfig) -> Result<Fixture, String> {
    let root = fixture_root(work_dir, &config);
    if root.exists() {
        return read_fixture(&root, config);
    }
    let _guard = acquire_fixture_guard(work_dir, &config)?;
    if root.exists() {
        return read_fixture(&root, config);
    }

    let temporary_root = unique_temporary_path(&root)?;
    let result = build_fixture(&temporary_root, &config)
        .and_then(|_| publish_fixture(&temporary_root, &root));
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&temporary_root);
        return Err(error);
    }
    Ok(fixture_from_parts(root, config))
}

fn fixture_root(work_dir: &Path, config: &FixtureConfig) -> PathBuf {
    work_dir.join(format!(
        "assets-{}-indexers-{}-sidecar-{}-mib",
        config.assets, config.indexers, config.sidecar_mib
    ))
}

fn read_fixture(root: &Path, config: FixtureConfig) -> Result<Fixture, String> {
    let metadata_path = root.join("fixture.json");
    let existing = fs::read(&metadata_path)
        .map_err(|error| format!("read {}: {error}", metadata_path.display()))?;
    let existing: FixtureConfig = serde_json::from_slice(&existing)
        .map_err(|error| format!("parse {}: {error}", metadata_path.display()))?;
    if existing != config {
        return Err(format!(
            "fixture configuration mismatch at {}",
            metadata_path.display()
        ));
    }
    Ok(fixture_from_parts(root.to_path_buf(), config))
}

fn build_fixture(root: &Path, config: &FixtureConfig) -> Result<(), String> {
    fs::create_dir_all(root.join("raw")).map_err(|error| format!("create fixture: {error}"))?;
    let fixture = fixture_from_parts(root.to_path_buf(), config.clone());
    for asset in &fixture.assets {
        fs::write(
            &asset.path,
            format!("montage skip benchmark asset {}\n", asset.id),
        )
        .map_err(|error| format!("write {}: {error}", asset.path.display()))?;
    }
    for server in &fixture.servers {
        for asset in &fixture.assets {
            let sidecar = sidecar_path(&fixture.root, &server.name, &asset.id)
                .map_err(|error| format!("sidecar path: {error}"))?;
            write_sidecar(
                &sidecar,
                &server.name,
                &asset.id,
                &asset_fingerprint(&asset.path)
                    .map_err(|error| format!("fingerprint {}: {error}", asset.path.display()))?,
                fixture.config.sidecar_mib * 1024 * 1024,
            )?;
        }
    }
    let metadata = serde_json::to_vec_pretty(&config)
        .map_err(|error| format!("serialize fixture metadata: {error}"))?;
    let metadata_path = root.join("fixture.json");
    fs::write(&metadata_path, metadata)
        .map_err(|error| format!("write {}: {error}", metadata_path.display()))?;
    Ok(())
}

#[derive(Debug)]
struct FixtureGuard {
    path: PathBuf,
}

impl Drop for FixtureGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_fixture_guard(work_dir: &Path, config: &FixtureConfig) -> Result<FixtureGuard, String> {
    fs::create_dir_all(work_dir)
        .map_err(|error| format!("create {}: {error}", work_dir.display()))?;
    let fixture = fixture_root(work_dir, config);
    let name = fixture
        .file_name()
        .ok_or_else(|| format!("fixture has no name: {}", fixture.display()))?;
    let path = work_dir.join(format!(".{}.lock", name.to_string_lossy()));
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(_) => Ok(FixtureGuard { path }),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Err(format!(
            "fixture build already in progress: {}",
            fixture.display()
        )),
        Err(error) => Err(format!("create {}: {error}", path.display())),
    }
}

fn unique_temporary_path(path: &Path) -> Result<PathBuf, String> {
    let parent = output_directory(path);
    let name = path
        .file_name()
        .ok_or_else(|| format!("path has no file name: {}", path.display()))?;
    Ok(parent.join(format!(
        ".{}.tmp-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        timestamp_nanos()
    )))
}

fn timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn publish_fixture(temporary: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err(format!("fixture already exists: {}", destination.display()));
    }
    fs::rename(temporary, destination).map_err(|error| {
        format!(
            "publish fixture {} -> {}: {error}",
            temporary.display(),
            destination.display()
        )
    })
}

fn output_directory(output: &Path) -> PathBuf {
    output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn write_report_atomically(output: &Path, contents: &[u8]) -> Result<(), String> {
    let parent = output_directory(output);
    fs::create_dir_all(&parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let temporary = unique_temporary_path(output)?;
    if let Err(error) = fs::write(&temporary, contents) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("write {}: {error}", temporary.display()));
    }
    if let Err(error) = fs::rename(&temporary, output) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "publish report {} -> {}: {error}",
            temporary.display(),
            output.display()
        ));
    }
    Ok(())
}

fn fixture_from_parts(root: PathBuf, config: FixtureConfig) -> Fixture {
    let assets = (0..config.assets)
        .map(|index| {
            let id = AssetId::new(format!("raw/asset-{index:02}.txt"));
            AssetInput {
                path: root.join(id.as_str()),
                id,
            }
        })
        .collect();
    let servers = (0..config.indexers)
        .map(|index| McpServer {
            name: format!("benchmark-{index:02}"),
            command: root
                .join(format!("not-an-indexer-{index:02}"))
                .display()
                .to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            kind: McpServerKind::Indexer,
            enabled: true,
            depends_on: Vec::new(),
            resource_class: IndexerResourceClass::Light,
            indexer_group: None,
        })
        .collect();
    Fixture {
        root,
        config,
        assets,
        servers,
    }
}

fn write_sidecar(
    path: &Path,
    indexer: &str,
    asset_id: &AssetId,
    fingerprint: &str,
    minimum_bytes: usize,
) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("sidecar has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let file = File::create(path).map_err(|error| format!("create {}: {error}", path.display()))?;
    let mut writer = BufWriter::new(file);
    let prefix = format!(
        "{{\"indexer\":{},\"indexer_version\":\"benchmark\",\"schema_version\":\"1\",\"asset_id\":{},\"asset_sha256\":{},\"produced_at\":\"2026-08-03T00:00:00Z\",\"data\":{{\"segments\":[",
        serde_json::to_string(indexer).map_err(|error| error.to_string())?,
        serde_json::to_string(asset_id.as_str()).map_err(|error| error.to_string())?,
        serde_json::to_string(fingerprint).map_err(|error| error.to_string())?,
    );
    let segment = r#"{"start":0.0,"end":1.0,"text":"representative segment"}"#;
    writer
        .write_all(prefix.as_bytes())
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    let mut bytes_written = prefix.len();
    let mut first = true;
    while bytes_written < minimum_bytes {
        if !first {
            writer
                .write_all(b",")
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            bytes_written += 1;
        }
        writer
            .write_all(segment.as_bytes())
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        bytes_written += segment.len();
        first = false;
    }
    writer
        .write_all(b"]}}")
        .and_then(|_| writer.flush())
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    Ok(())
}

#[derive(Debug, Serialize)]
struct Correctness {
    expected_pairs: usize,
    skipped: usize,
    wrote: usize,
    failed: usize,
    skipped_dependencies: usize,
}

async fn dispatch_fixture(fixture: &Fixture) -> Result<Correctness, String> {
    let report = run(
        &fixture.root,
        &fixture.servers,
        &fixture.assets,
        ClientInfo {
            name: "montage-index-skip-perf".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        fixture.config.indexers,
        None,
    )
    .await
    .map_err(|error| format!("dispatch fixture: {error}"))?;
    let mut skipped = 0;
    let mut wrote = 0;
    let mut failed = 0;
    let mut skipped_dependencies = 0;
    for outcome in report.outcomes {
        match outcome {
            PairOutcome::Skipped { .. } => skipped += 1,
            PairOutcome::Wrote { .. } => wrote += 1,
            PairOutcome::Failed { .. } => failed += 1,
            PairOutcome::SkippedDep { .. } => skipped_dependencies += 1,
        }
    }
    let correctness = Correctness {
        expected_pairs: fixture.config.assets * fixture.config.indexers,
        skipped,
        wrote,
        failed,
        skipped_dependencies,
    };
    if correctness.skipped != correctness.expected_pairs
        || correctness.wrote != 0
        || correctness.failed != 0
        || correctness.skipped_dependencies != 0
    {
        return Err(format!(
            "expected every pair to skip, got skipped={} wrote={} failed={} skipped_dependencies={}",
            correctness.skipped,
            correctness.wrote,
            correctness.failed,
            correctness.skipped_dependencies
        ));
    }
    Ok(correctness)
}

#[derive(Debug, Serialize)]
struct Configuration {
    label: String,
    assets: usize,
    indexers: usize,
    sidecar_mib: usize,
    warmups: usize,
    samples: usize,
    fixture_root: String,
    cache_state: String,
}

#[derive(Debug, Serialize)]
struct Machine {
    os: &'static str,
    arch: &'static str,
    parallelism: usize,
    build_profile: &'static str,
}

struct ReportProvenance {
    fixture_root: String,
    cache_state: String,
    build_profile: &'static str,
}

fn report_provenance(fixture_root: &Path, warmups: usize) -> ReportProvenance {
    ReportProvenance {
        fixture_root: fixture_root.display().to_string(),
        cache_state: format!("warm after {warmups} warmups"),
        build_profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    }
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    configuration: Configuration,
    machine: Machine,
    correctness: Correctness,
    samples_ms: Vec<f64>,
    statistics: Statistics,
}

#[derive(Debug, serde::Serialize)]
struct Statistics {
    median_ms: f64,
    p95_ms: f64,
    mad_ms: f64,
    min_ms: f64,
    max_ms: f64,
}

fn summarize_samples(samples: &[f64]) -> Statistics {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    if sorted.is_empty() {
        return Statistics {
            median_ms: 0.0,
            p95_ms: 0.0,
            mad_ms: 0.0,
            min_ms: 0.0,
            max_ms: 0.0,
        };
    }

    let median_ms = median(&sorted);
    let mut deviations: Vec<_> = sorted
        .iter()
        .map(|sample| (sample - median_ms).abs())
        .collect();
    deviations.sort_by(f64::total_cmp);
    let p95_index = (sorted.len() * 95).div_ceil(100).saturating_sub(1);

    Statistics {
        median_ms,
        p95_ms: sorted[p95_index],
        mad_ms: median(&deviations),
        min_ms: sorted[0],
        max_ms: sorted[sorted.len() - 1],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_uses_nearest_rank_p95_and_median_absolute_deviation() {
        let stats = summarize_samples(&[10.0, 12.0, 11.0, 50.0, 13.0]);
        assert_eq!(stats.median_ms, 12.0);
        assert_eq!(stats.p95_ms, 50.0);
        assert_eq!(stats.mad_ms, 1.0);
    }

    #[test]
    fn args_reject_zero_counts_and_unknown_arguments() {
        for argument in [
            "--assets",
            "--indexers",
            "--sidecar-mib",
            "--warmups",
            "--samples",
        ] {
            let error = Args::parse(vec![argument.into(), "0".into()]).expect_err("zero rejected");
            assert!(error.contains("must be greater than zero"));
        }
        let error = Args::parse(vec!["--unknown".into()]).expect_err("unknown rejected");
        assert!(error.contains("unknown argument"));
    }

    #[test]
    fn relative_output_filename_uses_current_directory() {
        assert_eq!(
            output_directory(Path::new("report.json")),
            PathBuf::from(".")
        );
    }

    #[test]
    fn fixture_guard_is_exclusive_and_releases() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = FixtureConfig {
            assets: 2,
            indexers: 2,
            sidecar_mib: 1,
        };
        let guard = acquire_fixture_guard(temp.path(), &config).expect("first guard");
        let error = acquire_fixture_guard(temp.path(), &config).expect_err("second guard blocked");
        assert!(error.contains("fixture build already in progress"));
        drop(guard);
        acquire_fixture_guard(temp.path(), &config).expect("guard released");
    }

    #[test]
    fn fixture_and_report_publication_rename_hidden_temps() {
        let temp = tempfile::tempdir().expect("tempdir");
        let temporary_fixture = temp.path().join(".fixture.tmp");
        let fixture = temp.path().join("fixture");
        std::fs::create_dir(&temporary_fixture).expect("temporary fixture");
        std::fs::write(temporary_fixture.join("fixture.json"), b"complete").expect("metadata");
        publish_fixture(&temporary_fixture, &fixture).expect("publish fixture");
        assert!(fixture.join("fixture.json").is_file());
        assert!(!temporary_fixture.exists());

        let output = temp.path().join("report.json");
        write_report_atomically(&output, b"{\"complete\":true}").expect("publish report");
        assert_eq!(
            std::fs::read(&output).expect("report"),
            b"{\"complete\":true}"
        );
        let unexpected_temp = temp.path().join(".report.json.tmp");
        assert!(!unexpected_temp.exists());
    }

    #[test]
    fn report_provenance_records_fixture_cache_and_build_profile() {
        let provenance = report_provenance(Path::new("/bench/fixture"), 3);
        assert_eq!(provenance.fixture_root, "/bench/fixture");
        assert_eq!(provenance.cache_state, "warm after 3 warmups");
        assert!(matches!(provenance.build_profile, "debug" | "release"));
    }

    #[test]
    fn fixture_sidecars_match_asset_fingerprints_and_requested_size() {
        let temp = tempfile::tempdir().expect("tempdir");
        let fixture = prepare_fixture(
            temp.path(),
            FixtureConfig {
                assets: 2,
                indexers: 2,
                sidecar_mib: 1,
            },
        )
        .expect("fixture");

        for server in &fixture.servers {
            for asset in &fixture.assets {
                let sidecar =
                    sidecar_path(&fixture.root, &server.name, &asset.id).expect("sidecar path");
                let value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&sidecar).expect("sidecar bytes"))
                        .expect("sidecar JSON");
                assert_eq!(
                    value["asset_sha256"],
                    asset_fingerprint(&asset.path).expect("fingerprint")
                );
                assert!(
                    sidecar.metadata().expect("sidecar metadata").len() >= 1024 * 1024,
                    "{} was smaller than one MiB",
                    sidecar.display()
                );
            }
        }
    }

    #[tokio::test]
    async fn benchmark_run_skips_every_pair_without_launching_indexers() {
        let temp = tempfile::tempdir().expect("tempdir");
        let fixture = prepare_fixture(
            temp.path(),
            FixtureConfig {
                assets: 2,
                indexers: 2,
                sidecar_mib: 1,
            },
        )
        .expect("fixture");

        let correctness = dispatch_fixture(&fixture).await.expect("dispatch");

        assert_eq!(correctness.skipped, 4);
        assert_eq!(correctness.wrote, 0);
        assert_eq!(correctness.failed, 0);
        assert_eq!(correctness.skipped_dependencies, 0);
    }
}
