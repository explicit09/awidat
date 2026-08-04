#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use montage_config::{IndexerResourceClass, McpServer, McpServerKind};
use montage_index::{AssetInput, PairOutcome, asset_fingerprint, run, sidecar_path};
use montage_mcp::ClientInfo;
use montage_proto::index::{AssetId, IndexSidecar};
use serde::de::IgnoredAny;
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
        sidecar_minimum_bytes(args.sidecar_mib)?;
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

fn sidecar_minimum_bytes(sidecar_mib: usize) -> Result<usize, String> {
    sidecar_mib
        .checked_mul(1024 * 1024)
        .ok_or_else(|| format!("--sidecar-mib {sidecar_mib} is too large"))
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
    read_fixture(&root, config)
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
    let fixture = fixture_from_parts(root.to_path_buf(), config);
    validate_fixture(&fixture)?;
    Ok(fixture)
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
                sidecar_minimum_bytes(fixture.config.sidecar_mib)?,
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

fn validate_fixture(fixture: &Fixture) -> Result<(), String> {
    let minimum_bytes = sidecar_minimum_bytes(fixture.config.sidecar_mib)?;
    let minimum_bytes_u64 = u64::try_from(minimum_bytes)
        .map_err(|_| format!("sidecar minimum {minimum_bytes} does not fit in u64"))?;
    for asset in &fixture.assets {
        let expected_fingerprint = asset_fingerprint(&asset.path)
            .map_err(|error| format!("fingerprint {}: {error}", asset.path.display()))?;
        for server in &fixture.servers {
            let path = sidecar_path(&fixture.root, &server.name, &asset.id)
                .map_err(|error| format!("sidecar path: {error}"))?;
            let file =
                File::open(&path).map_err(|error| format!("open {}: {error}", path.display()))?;
            let actual_bytes = file
                .metadata()
                .map_err(|error| format!("metadata {}: {error}", path.display()))?
                .len();
            if actual_bytes < minimum_bytes_u64 {
                return Err(format!(
                    "fixture sidecar {} is smaller than requested: {actual_bytes} < {minimum_bytes_u64} bytes",
                    path.display()
                ));
            }
            let sidecar: IndexSidecar<IgnoredAny> =
                serde_json::from_reader(BufReader::new(file))
                    .map_err(|error| format!("parse {}: {error}", path.display()))?;
            if sidecar.header.indexer != server.name {
                return Err(format!(
                    "fixture sidecar {} indexer mismatch: {:?} != {:?}",
                    path.display(),
                    sidecar.header.indexer,
                    server.name
                ));
            }
            if sidecar.header.asset_id != asset.id {
                return Err(format!(
                    "fixture sidecar {} asset id mismatch: {:?} != {:?}",
                    path.display(),
                    sidecar.header.asset_id,
                    asset.id
                ));
            }
            if sidecar.header.asset_sha256 != expected_fingerprint {
                return Err(format!(
                    "fixture sidecar {} fingerprint mismatch: {:?} != {:?}",
                    path.display(),
                    sidecar.header.asset_sha256,
                    expected_fingerprint
                ));
            }
        }
    }
    Ok(())
}

const FIXTURE_GUARD_MARKER: &[u8] = b"montage-index-skip-perf-v2\n";

#[derive(Debug)]
struct FixtureGuard {
    advisory_lock: File,
    advisory_path: PathBuf,
    legacy_path: PathBuf,
}

impl Drop for FixtureGuard {
    fn drop(&mut self) {
        if validate_marker_bearing_legacy_lock(
            &self.advisory_lock,
            &self.advisory_path,
            &self.legacy_path,
        )
        .is_ok()
        {
            let _ = fs::remove_file(&self.legacy_path);
        }
    }
}

fn acquire_fixture_guard(work_dir: &Path, config: &FixtureConfig) -> Result<FixtureGuard, String> {
    fs::create_dir_all(work_dir)
        .map_err(|error| format!("create {}: {error}", work_dir.display()))?;
    let fixture = fixture_root(work_dir, config);
    let name = fixture
        .file_name()
        .ok_or_else(|| format!("fixture has no name: {}", fixture.display()))?;
    let legacy_path = work_dir.join(format!(".{}.lock", name.to_string_lossy()));
    let advisory_path = work_dir.join(format!(".{}.lock.v2", name.to_string_lossy()));
    let mut advisory_lock = open_advisory_lock(&advisory_path)?;
    match advisory_lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(format!(
                "fixture build already in progress: {}",
                fixture.display()
            ));
        }
        Err(std::fs::TryLockError::Error(error)) => {
            return Err(format!("lock {}: {error}", advisory_path.display()));
        }
    }
    claim_legacy_fixture_barrier(&mut advisory_lock, &advisory_path, &legacy_path)?;
    Ok(FixtureGuard {
        advisory_lock,
        advisory_path,
        legacy_path,
    })
}

fn open_advisory_lock(path: &Path) -> Result<File, String> {
    #[cfg(unix)]
    {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        validate_advisory_lock_path(&lock, path)?;
        Ok(lock)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err("safe advisory fixture locking is unavailable on this platform".into())
    }
}

#[cfg(unix)]
fn validate_advisory_lock_path(lock: &File, path: &Path) -> Result<(), String> {
    let opened = lock
        .metadata()
        .map_err(|error| format!("metadata {}: {error}", path.display()))?;
    let linked = fs::symlink_metadata(path)
        .map_err(|error| format!("metadata {}: {error}", path.display()))?;
    if !opened.file_type().is_file()
        || !linked.file_type().is_file()
        || opened.dev() != linked.dev()
        || opened.ino() != linked.ino()
    {
        return Err(format!("unsafe advisory fixture lock: {}", path.display()));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_unlinked_advisory_lock(
    lock: &File,
    advisory_path: &Path,
    legacy_path: &Path,
) -> Result<(), String> {
    validate_advisory_lock_path(lock, advisory_path)?;
    let metadata = lock
        .metadata()
        .map_err(|error| format!("metadata {}: {error}", advisory_path.display()))?;
    if metadata.nlink() != 1 {
        return Err(format!(
            "unexpected advisory fixture lock link count: {}",
            advisory_path.display()
        ));
    }
    match fs::symlink_metadata(legacy_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(format!(
            "legacy fixture build already in progress: {}",
            legacy_path.display()
        )),
        Err(error) => Err(format!("metadata {}: {error}", legacy_path.display())),
    }
}

#[cfg(not(unix))]
fn validate_unlinked_advisory_lock(
    _lock: &File,
    _advisory_path: &Path,
    _legacy_path: &Path,
) -> Result<(), String> {
    Err("safe advisory fixture locking is unavailable on this platform".into())
}

#[cfg(unix)]
fn validate_marker_bearing_legacy_lock(
    lock: &File,
    advisory_path: &Path,
    legacy_path: &Path,
) -> Result<(), String> {
    validate_advisory_lock_path(lock, advisory_path)?;
    let advisory = lock
        .metadata()
        .map_err(|error| format!("metadata {}: {error}", advisory_path.display()))?;
    let legacy = fs::symlink_metadata(legacy_path)
        .map_err(|error| format!("metadata {}: {error}", legacy_path.display()))?;
    if advisory.nlink() != 2
        || !legacy.file_type().is_file()
        || advisory.dev() != legacy.dev()
        || advisory.ino() != legacy.ino()
        || advisory.len() != FIXTURE_GUARD_MARKER.len() as u64
    {
        return Err(format!(
            "legacy fixture build already in progress: {}",
            legacy_path.display()
        ));
    }
    if read_advisory_marker(lock, advisory_path)? != FIXTURE_GUARD_MARKER {
        return Err(format!(
            "legacy fixture build already in progress: {}",
            legacy_path.display()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn read_advisory_marker(lock: &File, path: &Path) -> Result<Vec<u8>, String> {
    let mut reader = lock
        .try_clone()
        .map_err(|error| format!("clone {}: {error}", path.display()))?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| format!("seek {}: {error}", path.display()))?;
    let mut marker = Vec::new();
    reader
        .read_to_end(&mut marker)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(marker)
}

#[cfg(not(unix))]
fn validate_marker_bearing_legacy_lock(
    _lock: &File,
    _advisory_path: &Path,
    _legacy_path: &Path,
) -> Result<(), String> {
    Err("safe advisory fixture locking is unavailable on this platform".into())
}

fn claim_legacy_fixture_barrier(
    advisory_lock: &mut File,
    advisory_path: &Path,
    legacy_path: &Path,
) -> Result<(), String> {
    match fs::symlink_metadata(legacy_path) {
        Ok(_) => {
            validate_marker_bearing_legacy_lock(advisory_lock, advisory_path, legacy_path)?;
            fs::remove_file(legacy_path)
                .map_err(|error| format!("remove {}: {error}", legacy_path.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("metadata {}: {error}", legacy_path.display())),
    }
    validate_unlinked_advisory_lock(advisory_lock, advisory_path, legacy_path)?;
    advisory_lock
        .set_len(0)
        .and_then(|()| advisory_lock.seek(SeekFrom::Start(0)).map(|_| ()))
        .and_then(|()| advisory_lock.write_all(FIXTURE_GUARD_MARKER))
        .and_then(|()| advisory_lock.sync_all())
        .map_err(|error| format!("write {}: {error}", advisory_path.display()))?;
    validate_unlinked_advisory_lock(advisory_lock, advisory_path, legacy_path)?;
    fs::hard_link(advisory_path, legacy_path).map_err(|error| {
        format!(
            "publish {} -> {}: {error}",
            advisory_path.display(),
            legacy_path.display()
        )
    })?;
    validate_marker_bearing_legacy_lock(advisory_lock, advisory_path, legacy_path)
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
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

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
    fn args_reject_sidecar_sizes_that_overflow_bytes() {
        let error = Args::parse(vec!["--sidecar-mib".into(), usize::MAX.to_string()])
            .expect_err("overflowing sidecar size rejected");
        assert!(error.contains("too large"));
    }

    #[test]
    fn relative_output_filename_uses_current_directory() {
        assert_eq!(
            output_directory(Path::new("report.json")),
            PathBuf::from(".")
        );
    }

    #[test]
    fn fixture_guard_v2_lock_is_exclusive_and_releases() {
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
    fn fixture_guard_recovers_a_marker_bearing_crash_left_legacy_lock() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = FixtureConfig {
            assets: 1,
            indexers: 1,
            sidecar_mib: 1,
        };
        let root = fixture_root(temp.path(), &config);
        let name = root.file_name().expect("fixture name");
        let lock = temp
            .path()
            .join(format!(".{}.lock", name.to_string_lossy()));
        let advisory = temp
            .path()
            .join(format!(".{}.lock.v2", name.to_string_lossy()));
        std::fs::write(&advisory, b"montage-index-skip-perf-v2\n").expect("stale lock");
        std::fs::hard_link(&advisory, &lock).expect("legacy marker link");

        acquire_fixture_guard(temp.path(), &config).expect("stale lock recovered");
        assert!(advisory.is_file());
    }

    #[test]
    fn fixture_guard_refuses_empty_or_unknown_legacy_locks() {
        for contents in [b"".as_slice(), b"legacy sentinel".as_slice()] {
            let temp = tempfile::tempdir().expect("tempdir");
            let config = FixtureConfig {
                assets: 1,
                indexers: 1,
                sidecar_mib: 1,
            };
            let root = fixture_root(temp.path(), &config);
            let name = root.file_name().expect("fixture name");
            let lock = temp
                .path()
                .join(format!(".{}.lock", name.to_string_lossy()));
            std::fs::write(&lock, contents).expect("legacy lock");

            let error = acquire_fixture_guard(temp.path(), &config)
                .expect_err("unknown legacy lock blocks v2 builder");
            assert!(error.contains("legacy fixture build already in progress"));
        }
    }

    #[test]
    fn fixture_guard_removes_its_legacy_barrier_on_drop() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = FixtureConfig {
            assets: 1,
            indexers: 1,
            sidecar_mib: 1,
        };
        let root = fixture_root(temp.path(), &config);
        let name = root.file_name().expect("fixture name");
        let lock = temp
            .path()
            .join(format!(".{}.lock", name.to_string_lossy()));
        let advisory = temp
            .path()
            .join(format!(".{}.lock.v2", name.to_string_lossy()));

        let guard = acquire_fixture_guard(temp.path(), &config).expect("v2 guard");
        assert_eq!(
            std::fs::read(&lock).expect("v2 legacy marker"),
            b"montage-index-skip-perf-v2\n"
        );
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(&lock).expect("legacy metadata").ino(),
            std::fs::metadata(&advisory)
                .expect("advisory metadata")
                .ino()
        );
        drop(guard);
        assert!(!lock.exists());
    }

    #[cfg(unix)]
    #[test]
    fn fixture_guard_rejects_a_symlinked_advisory_lock_without_mutating_its_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = FixtureConfig {
            assets: 1,
            indexers: 1,
            sidecar_mib: 1,
        };
        let root = fixture_root(temp.path(), &config);
        let name = root.file_name().expect("fixture name");
        let advisory = temp
            .path()
            .join(format!(".{}.lock.v2", name.to_string_lossy()));
        let target = temp.path().join("sentinel");
        std::fs::write(&target, b"do not truncate").expect("sentinel");
        symlink(&target, &advisory).expect("advisory symlink");

        assert!(acquire_fixture_guard(temp.path(), &config).is_err());
        assert_eq!(
            std::fs::read(&target).expect("sentinel preserved"),
            b"do not truncate"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fixture_guard_rejects_a_hard_linked_advisory_lock_without_mutating_its_target() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = FixtureConfig {
            assets: 1,
            indexers: 1,
            sidecar_mib: 1,
        };
        let root = fixture_root(temp.path(), &config);
        let name = root.file_name().expect("fixture name");
        let advisory = temp
            .path()
            .join(format!(".{}.lock.v2", name.to_string_lossy()));
        let target = temp.path().join("sentinel");
        std::fs::write(&target, b"do not truncate").expect("sentinel");
        std::fs::hard_link(&target, &advisory).expect("advisory hard link");

        assert!(acquire_fixture_guard(temp.path(), &config).is_err());
        assert_eq!(
            std::fs::read(&target).expect("sentinel preserved"),
            b"do not truncate"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fixture_guard_rejects_a_legacy_fifo_symlink_without_following_it() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = FixtureConfig {
            assets: 1,
            indexers: 1,
            sidecar_mib: 1,
        };
        let root = fixture_root(temp.path(), &config);
        let name = root.file_name().expect("fixture name");
        let legacy = temp
            .path()
            .join(format!(".{}.lock", name.to_string_lossy()));
        let fifo = temp.path().join("legacy-fifo");
        let status = std::process::Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo command");
        assert!(status.success());
        symlink(&fifo, &legacy).expect("legacy symlink");

        let work_dir = temp.path().to_path_buf();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = sender.send(acquire_fixture_guard(&work_dir, &config));
        });
        let result = match receiver.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
                loop {
                    match OpenOptions::new()
                        .write(true)
                        .custom_flags(libc::O_NONBLOCK)
                        .open(&fifo)
                    {
                        Ok(writer) => {
                            drop(writer);
                            break;
                        }
                        Err(error)
                            if error.raw_os_error() == Some(libc::ENXIO)
                                && std::time::Instant::now() < deadline =>
                        {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Err(error) => panic!("bounded fifo writer: {error}"),
                    }
                }
                let _ = receiver
                    .recv_timeout(std::time::Duration::from_millis(200))
                    .expect("unblocked fixture guard");
                worker.join().expect("fixture guard worker");
                panic!("legacy symlink blocked fixture guard");
            }
            Err(error) => panic!("fixture guard result channel: {error}"),
        };
        worker.join().expect("fixture guard worker");

        assert!(result.is_err());
        assert!(
            std::fs::symlink_metadata(&legacy)
                .expect("legacy symlink metadata")
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn fixture_guard_drop_preserves_an_unowned_legacy_symlink() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = FixtureConfig {
            assets: 1,
            indexers: 1,
            sidecar_mib: 1,
        };
        let root = fixture_root(temp.path(), &config);
        let name = root.file_name().expect("fixture name");
        let legacy = temp
            .path()
            .join(format!(".{}.lock", name.to_string_lossy()));
        let target = temp.path().join("sentinel");

        let guard = acquire_fixture_guard(temp.path(), &config).expect("v2 guard");
        std::fs::remove_file(&legacy).expect("remove owned legacy marker");
        std::fs::write(&target, FIXTURE_GUARD_MARKER).expect("sentinel");
        symlink(&target, &legacy).expect("legacy symlink");
        drop(guard);

        assert!(
            std::fs::symlink_metadata(&legacy)
                .expect("legacy symlink metadata")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read(&target).expect("sentinel preserved"),
            FIXTURE_GUARD_MARKER
        );
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

    #[test]
    fn reused_fixture_rejects_a_tiny_replacement_sidecar() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = FixtureConfig {
            assets: 1,
            indexers: 1,
            sidecar_mib: 1,
        };
        let fixture = prepare_fixture(temp.path(), config.clone()).expect("fixture");
        let asset = &fixture.assets[0];
        let server = &fixture.servers[0];
        let sidecar = sidecar_path(&fixture.root, &server.name, &asset.id).expect("sidecar path");
        write_sidecar(
            &sidecar,
            &server.name,
            &asset.id,
            &asset_fingerprint(&asset.path).expect("fingerprint"),
            1,
        )
        .expect("replace sidecar");

        let error = prepare_fixture(temp.path(), config)
            .err()
            .expect("tiny reused sidecar rejected");
        assert!(error.contains("smaller than"));
    }

    #[test]
    fn reused_fixture_rejects_invalid_sidecar_json() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = FixtureConfig {
            assets: 1,
            indexers: 1,
            sidecar_mib: 1,
        };
        let fixture = prepare_fixture(temp.path(), config.clone()).expect("fixture");
        let sidecar = sidecar_path(
            &fixture.root,
            &fixture.servers[0].name,
            &fixture.assets[0].id,
        )
        .expect("sidecar path");
        std::fs::write(&sidecar, vec![b'x'; 1024 * 1024]).expect("replace sidecar");

        let error = prepare_fixture(temp.path(), config)
            .err()
            .expect("invalid reused sidecar rejected");
        assert!(error.contains("parse"));
    }

    #[test]
    fn reused_fixture_rejects_asset_fingerprint_drift() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = FixtureConfig {
            assets: 1,
            indexers: 1,
            sidecar_mib: 1,
        };
        let fixture = prepare_fixture(temp.path(), config.clone()).expect("fixture");
        std::fs::write(&fixture.assets[0].path, b"modified benchmark asset\n")
            .expect("modify asset");

        let error = prepare_fixture(temp.path(), config)
            .err()
            .expect("stale reused sidecar rejected");
        assert!(error.contains("fingerprint"));
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
