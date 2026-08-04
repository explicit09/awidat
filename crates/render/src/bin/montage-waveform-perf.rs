#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

//! Real-FFmpeg benchmark for the production waveform generator.
//!
//! The parent process samples a fresh helper process and every descendant,
//! including ffmpeg. Keeping `generate_waveform` in the helper leaves the
//! production implementation free of benchmark-only telemetry.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use chrono::{SecondsFormat, Utc};
use montage_render::{FfmpegError, ffmpeg_path, generate_waveform};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

const DEFAULT_DURATION_S: u64 = 2 * 60 * 60;
const BUCKETS: usize = 2048;
const WARMUPS: usize = 1;
const SAMPLES: usize = 7;
const WAVEFORM_SAMPLE_RATE_HZ: usize = 8000;
const DURATION_TOLERANCE_S: f64 = 1.0 / WAVEFORM_SAMPLE_RATE_HZ as f64;
const CANCELLATION_DELAY: Duration = Duration::from_millis(1);
const RSS_SAMPLE_INTERVAL: Duration = Duration::from_millis(10);
const RSS_SAMPLE_MAX_GAP: Duration = Duration::from_millis(100);
const HELPER_TERMINATION_GRACE: Duration = Duration::from_millis(500);
const HELPER_TERMINATION_POLL_INTERVAL: Duration = Duration::from_millis(10);

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let result = if raw.first().is_some_and(|arg| arg == "--helper") {
        helper_main(&raw[1..])
    } else {
        real_main(&raw)
    };
    if let Err(error) = result {
        eprintln!("montage-waveform-perf: {error}");
        std::process::exit(1);
    }
}

fn real_main(raw: &[String]) -> Result<(), String> {
    let args = Args::parse(raw)?;
    fs::create_dir_all(&args.work_dir)
        .map_err(|error| format!("create {}: {error}", args.work_dir.display()))?;
    fs::create_dir_all(&args.evidence_dir)
        .map_err(|error| format!("create {}: {error}", args.evidence_dir.display()))?;

    let ffmpeg = ffmpeg_path().map_err(|error| error.to_string())?;
    let fixture = prepare_fixture(&args, &ffmpeg)?;
    let oracle = build_reference_oracle(Path::new(&fixture.path), &args.work_dir, &ffmpeg)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("build runtime: {error}"))?;
    let contracts =
        run_contract_probes(&runtime, &args.work_dir, &ffmpeg, Path::new(&fixture.path))?;

    let executable =
        std::env::current_exe().map_err(|error| format!("current executable: {error}"))?;
    let warmup = run_helper_sample(
        &executable,
        Path::new(&fixture.path),
        &args.work_dir,
        "warmup",
    )?;
    assert_matches_oracle(&oracle, &warmup.result, "warmup")?;
    let baseline = warmup.result.clone();
    let mut samples = Vec::with_capacity(SAMPLES);
    for index in 1..=SAMPLES {
        let sample = run_helper_sample(
            &executable,
            Path::new(&fixture.path),
            &args.work_dir,
            &format!("sample-{index}"),
        )?;
        assert_same_waveform(&baseline, &sample.result, index)?;
        assert_matches_oracle(&oracle, &sample.result, &format!("sample {index}"))?;
        samples.push(sample);
    }

    let wall_samples_ms: Vec<f64> = samples.iter().map(|sample| sample.wall_ms).collect();
    let peak_rss_bytes: Vec<f64> = samples
        .iter()
        .map(|sample| sample.process_tree_peak_rss_bytes as f64)
        .collect();
    let peak_cpu_ms: Vec<f64> = samples
        .iter()
        .map(|sample| sample.process_tree_peak_cpu_ms as f64)
        .collect();
    let generated_at = Utc::now();
    let report = BenchmarkReport {
        schema_version: 2,
        generated_at_utc: generated_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        configuration: Configuration {
            label: args.label,
            duration_s: args.duration_s,
            buckets: BUCKETS,
            warmups: WARMUPS,
            samples: SAMPLES,
            work_dir: args.work_dir.display().to_string(),
            evidence_dir: args.evidence_dir.display().to_string(),
        },
        fixture,
        decoder_ffmpeg: baseline.decoder_ffmpeg.clone(),
        reference_oracle: oracle.evidence,
        provenance: report_provenance(&args.work_dir, &args.evidence_dir, &executable)?,
        contracts,
        warmup,
        samples,
        statistics: Statistics {
            wall_ms: summarize_distribution(&wall_samples_ms),
            peak_rss_bytes: summarize_distribution(&peak_rss_bytes),
            peak_cpu_ms: summarize_distribution(&peak_cpu_ms),
        },
        correctness: Correctness {
            canonical_sha256: baseline.canonical_sha256,
            duration_bits: baseline.duration_bits,
            bucket_count: baseline.bucket_bits.len(),
            every_timed_sample_equal: true,
        },
        disk_io: DiskIo {
            available: false,
            reason: "portable per-process disk I/O accounting is unavailable; use platform tooling beside this report".into(),
        },
    };
    let timestamp = format!(
        "{}-{:09}Z",
        generated_at.format("%Y%m%dT%H%M%S"),
        generated_at.timestamp_subsec_nanos()
    );
    let output = args.evidence_dir.join(format!(
        "{}-{timestamp}-{}-waveform-performance.json",
        report.configuration.label,
        std::process::id()
    ));
    let json =
        serde_json::to_vec_pretty(&report).map_err(|error| format!("serialize report: {error}"))?;
    write_atomically_new(&output, &json)?;
    println!("{}", output.display());
    Ok(())
}

fn helper_main(raw: &[String]) -> Result<(), String> {
    let args = HelperArgs::parse(raw)?;
    // `generate_waveform` calls this same cached resolver in this helper, so
    // this is the decoder binary it will actually spawn rather than fixture
    // generator provenance inherited from the parent process.
    let decoder_ffmpeg = decoder_ffmpeg_provenance()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("build helper runtime: {error}"))?;
    let waveform = runtime
        .block_on(generate_waveform(
            &args.asset,
            args.buckets,
            CancellationToken::new(),
        ))
        .map_err(|error| format!("generate waveform: {error}"))?;
    if waveform.buckets.len() != args.buckets {
        return Err(format!(
            "expected {} waveform buckets, got {}",
            args.buckets,
            waveform.buckets.len()
        ));
    }
    let bucket_bits: Vec<u32> = waveform
        .buckets
        .iter()
        .map(|bucket| bucket.to_bits())
        .collect();
    validate_waveform_invariants(&bucket_bits, args.buckets)?;
    let result = HelperResult {
        duration_s: waveform.duration_s,
        duration_bits: duration_bits_hex(waveform.duration_s),
        canonical_sha256: canonical_waveform_sha256(waveform.duration_s, &bucket_bits),
        bucket_bits,
        decoder_ffmpeg,
    };
    let json =
        serde_json::to_vec(&result).map_err(|error| format!("serialize helper result: {error}"))?;
    write_atomically(&args.output, &json)
}

#[derive(Debug)]
struct Args {
    work_dir: PathBuf,
    evidence_dir: PathBuf,
    label: String,
    duration_s: u64,
}

impl Args {
    fn parse(raw: &[String]) -> Result<Self, String> {
        let mut work_dir = std::env::var_os("MONTAGE_WAVEFORM_PERF_WORK_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("montage-waveform-perf"));
        let mut evidence_dir =
            std::env::var_os("MONTAGE_WAVEFORM_PERF_EVIDENCE_DIR").map(PathBuf::from);
        let mut label = "default".to_string();
        let mut duration_s = DEFAULT_DURATION_S;
        let mut index = 0;
        while index < raw.len() {
            let flag = &raw[index];
            index += 1;
            let value = |flag: &str, index: &mut usize| {
                let value = raw
                    .get(*index)
                    .cloned()
                    .ok_or_else(|| format!("{flag} requires a value"))?;
                *index += 1;
                Ok::<_, String>(value)
            };
            match flag.as_str() {
                "--work-dir" => work_dir = PathBuf::from(value("--work-dir", &mut index)?),
                "--evidence-dir" => {
                    evidence_dir = Some(PathBuf::from(value("--evidence-dir", &mut index)?))
                }
                "--label" => label = value("--label", &mut index)?,
                "--duration-s" => {
                    duration_s =
                        parse_positive("--duration-s", &value("--duration-s", &mut index)?)?
                }
                _ => return Err(format!("unknown argument: {flag}")),
            }
        }
        if !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err("--label must use only letters, digits, '-' or '_'".into());
        }
        Ok(Self {
            evidence_dir: evidence_dir.unwrap_or_else(|| work_dir.join("evidence")),
            work_dir,
            label,
            duration_s,
        })
    }
}

#[derive(Debug)]
struct HelperArgs {
    asset: PathBuf,
    buckets: usize,
    output: PathBuf,
}

impl HelperArgs {
    fn parse(raw: &[String]) -> Result<Self, String> {
        let mut asset = None;
        let mut buckets = None;
        let mut output = None;
        let mut index = 0;
        while index < raw.len() {
            let flag = &raw[index];
            index += 1;
            let value = raw
                .get(index)
                .cloned()
                .ok_or_else(|| format!("{flag} requires a value"))?;
            index += 1;
            match flag.as_str() {
                "--asset" => asset = Some(PathBuf::from(value)),
                "--buckets" => buckets = Some(parse_positive("--buckets", &value)? as usize),
                "--output" => output = Some(PathBuf::from(value)),
                _ => return Err(format!("unknown helper argument: {flag}")),
            }
        }
        Ok(Self {
            asset: asset.ok_or_else(|| "--asset is required".to_string())?,
            buckets: buckets.ok_or_else(|| "--buckets is required".to_string())?,
            output: output.ok_or_else(|| "--output is required".to_string())?,
        })
    }
}

fn parse_positive(flag: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{flag} must be a positive integer"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HelperResult {
    duration_s: f64,
    duration_bits: String,
    canonical_sha256: String,
    bucket_bits: Vec<u32>,
    decoder_ffmpeg: DecoderFfmpeg,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DecoderFfmpeg {
    path: String,
    version: String,
}

#[derive(Debug, Serialize)]
struct Sample {
    name: String,
    wall_ms: f64,
    process_tree_peak_rss_bytes: u64,
    process_tree_peak_cpu_ms: u64,
    rss_sample_count: usize,
    rss_sample_interval_ms: u64,
    max_rss_sample_gap_ms: f64,
    helper_pid: u32,
    result: HelperResult,
}

fn run_helper_sample(
    executable: &Path,
    asset: &Path,
    work_dir: &Path,
    name: &str,
) -> Result<Sample, String> {
    let result_path = work_dir.join("helper-results").join(format!("{name}.json"));
    if result_path.exists() {
        fs::remove_file(&result_path)
            .map_err(|error| format!("remove stale {}: {error}", result_path.display()))?;
    }
    let parent = result_path
        .parent()
        .ok_or_else(|| format!("no parent for {}", result_path.display()))?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let started = Instant::now();
    let mut command = Command::new(executable);
    command
        .args([
            "--helper",
            "--asset",
            &asset.display().to_string(),
            "--buckets",
            &BUCKETS.to_string(),
            "--output",
            &result_path.display().to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    configure_helper_process_group(&mut command);
    let child = command
        .spawn()
        .map_err(|error| format!("spawn helper: {error}"))?;
    let measured = monitor_process_tree(child, started)?;
    if !measured.status.success() {
        return Err(format!("{name} helper exited {}", measured.status));
    }
    let bytes = fs::read(&result_path)
        .map_err(|error| format!("read {}: {error}", result_path.display()))?;
    let result: HelperResult = serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse {}: {error}", result_path.display()))?;
    if result.bucket_bits.len() != BUCKETS
        || canonical_waveform_sha256(result.duration_s, &result.bucket_bits)
            != result.canonical_sha256
        || duration_bits_hex(result.duration_s) != result.duration_bits
    {
        return Err(format!("{name} helper result failed canonical validation"));
    }
    Ok(Sample {
        name: name.into(),
        wall_ms: measured.wall.as_secs_f64() * 1_000.0,
        process_tree_peak_rss_bytes: measured.peak_rss_bytes,
        process_tree_peak_cpu_ms: measured.peak_cpu_time.as_millis() as u64,
        rss_sample_count: measured.sample_count,
        rss_sample_interval_ms: RSS_SAMPLE_INTERVAL.as_millis() as u64,
        max_rss_sample_gap_ms: measured.max_sample_gap.as_secs_f64() * 1_000.0,
        helper_pid: measured.pid,
        result,
    })
}

struct MeasuredChild {
    status: std::process::ExitStatus,
    wall: Duration,
    peak_rss_bytes: u64,
    peak_cpu_time: Duration,
    sample_count: usize,
    max_sample_gap: Duration,
    pid: u32,
}

fn monitor_process_tree(child: Child, started: Instant) -> Result<MeasuredChild, String> {
    monitor_process_tree_with_sampler(child, started, sample_process_tree_usage)
}

fn monitor_process_tree_with_sampler<F>(
    mut child: Child,
    started: Instant,
    mut sampler: F,
) -> Result<MeasuredChild, String>
where
    F: FnMut(u32) -> Result<TreeUsage, String>,
{
    let result = monitor_process_tree_inner(&mut child, started, &mut sampler);
    if let Err(error) = result {
        terminate_and_reap_helper_process_group(&mut child)
            .map_err(|cleanup| format!("{error}; helper cleanup failed: {cleanup}"))?;
        return Err(error);
    }
    result
}

fn monitor_process_tree_inner(
    child: &mut Child,
    started: Instant,
    sampler: &mut impl FnMut(u32) -> Result<TreeUsage, String>,
) -> Result<MeasuredChild, String> {
    let pid = child.id();
    let mut peak_rss_bytes = 0;
    let mut peak_cpu_time = Duration::ZERO;
    let mut sample_count = 0;
    let mut last_sample = None;
    let mut max_sample_gap = Duration::ZERO;
    loop {
        let sample_started = Instant::now();
        if let Some(previous) = last_sample.replace(sample_started) {
            max_sample_gap = max_sample_gap.max(sample_started.duration_since(previous));
        }
        let usage = sampler(pid)?;
        if usage.rss_bytes > 0 {
            sample_count += 1;
            peak_rss_bytes = peak_rss_bytes.max(usage.rss_bytes);
        }
        peak_cpu_time = peak_cpu_time.max(usage.cpu_time);
        if helper_has_exited_without_reaping(pid)? {
            if sample_count == 0 {
                return Err("process-tree RSS sampler never observed the helper".into());
            }
            if max_sample_gap > RSS_SAMPLE_MAX_GAP {
                return Err(format!(
                    "process-tree RSS sampling gap {:.2}ms exceeded {}ms",
                    max_sample_gap.as_secs_f64() * 1_000.0,
                    RSS_SAMPLE_MAX_GAP.as_millis()
                ));
            }
            #[cfg(unix)]
            if !helper_process_group_descendants(pid)?.is_empty() {
                return Err("helper exited with lingering process-group descendants".into());
            }
            let status = child
                .wait()
                .map_err(|error| format!("reap helper: {error}"))?;
            return Ok(MeasuredChild {
                status,
                wall: started.elapsed(),
                peak_rss_bytes,
                peak_cpu_time,
                sample_count,
                max_sample_gap,
                pid,
            });
        }
        let elapsed = sample_started.elapsed();
        if elapsed < RSS_SAMPLE_INTERVAL {
            thread::sleep(RSS_SAMPLE_INTERVAL - elapsed);
        }
    }
}

fn configure_helper_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        command.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = command;
}

fn terminate_and_reap_helper_process_group(child: &mut Child) -> Result<(), String> {
    #[cfg(unix)]
    {
        let process_group = child.id();
        finish_helper_process_group_cleanup(
            |signal| signal_helper_process_group(process_group, signal),
            |grace| wait_for_helper_process_group_quiescent(process_group, grace),
            || reap_helper_child_with_timeout(child, HELPER_TERMINATION_GRACE),
        )
    }
    #[cfg(not(unix))]
    {
        child
            .kill()
            .map_err(|error| format!("terminate helper: {error}"))?;
        reap_helper_child_with_timeout(child, HELPER_TERMINATION_GRACE)
    }
}

#[cfg(unix)]
fn finish_helper_process_group_cleanup<S, W, R>(
    mut signal: S,
    mut wait_for_quiescence: W,
    reap_leader: R,
) -> Result<(), String>
where
    S: FnMut(&str) -> Result<(), String>,
    W: FnMut(Duration) -> Result<bool, String>,
    R: FnOnce() -> Result<(), String>,
{
    let term_error = signal("TERM").err();
    let after_term = wait_for_quiescence(HELPER_TERMINATION_GRACE);
    let (kill_error, after_final_signal) = if matches!(&after_term, Ok(true)) {
        (None, after_term)
    } else {
        (
            signal("KILL").err(),
            wait_for_quiescence(HELPER_TERMINATION_GRACE),
        )
    };
    let signal_errors = [term_error, kill_error]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let signal_context = if signal_errors.is_empty() {
        String::new()
    } else {
        format!("; cleanup signal failures: {}", signal_errors.join("; "))
    };

    match after_final_signal {
        Ok(true) => {
            reap_leader()?;
            if signal_errors.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "helper process group cleanup signal failures: {}",
                    signal_errors.join("; ")
                ))
            }
        }
        Ok(false) => Err(format!(
            "helper process group remained live after final signal{signal_context}"
        )),
        Err(error) => Err(format!(
            "helper process group final quiescence check failed: {error}{signal_context}"
        )),
    }
}

fn reap_helper_child_with_timeout(child: &mut Child, grace: Duration) -> Result<(), String> {
    let deadline = Instant::now() + grace;
    loop {
        match child
            .try_wait()
            .map_err(|error| format!("reap helper: {error}"))?
        {
            Some(_) => return Ok(()),
            None if Instant::now() >= deadline => {
                return Err("helper process did not exit after final signal".into());
            }
            None => thread::sleep(HELPER_TERMINATION_POLL_INTERVAL),
        }
    }
}

#[cfg(unix)]
fn wait_for_helper_process_group_quiescent(
    process_group: u32,
    grace: Duration,
) -> Result<bool, String> {
    let deadline = Instant::now() + grace;
    while !helper_process_group_live_members(process_group)?.is_empty() {
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(HELPER_TERMINATION_POLL_INTERVAL);
    }
    Ok(true)
}

#[cfg(unix)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessGroupMember {
    pid: u32,
    state: String,
}

#[cfg(unix)]
fn helper_process_group_members(process_group: u32) -> Result<Vec<ProcessGroupMember>, String> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,pgid=,stat="])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("sample helper process group {process_group}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "sample helper process group {process_group} exited {}",
            output.status
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let pgid = fields.next()?.parse::<u32>().ok()?;
            let state = fields.next()?.to_string();
            (pgid == process_group).then_some(ProcessGroupMember { pid, state })
        })
        .collect())
}

#[cfg(unix)]
fn helper_process_group_descendants(process_group: u32) -> Result<Vec<u32>, String> {
    Ok(helper_process_group_members(process_group)?
        .into_iter()
        .filter(|member| member.pid != process_group)
        .map(|member| member.pid)
        .collect())
}

#[cfg(unix)]
fn helper_process_group_live_members(process_group: u32) -> Result<Vec<u32>, String> {
    Ok(helper_process_group_members(process_group)?
        .into_iter()
        .filter(|member| !member.state.starts_with('Z'))
        .map(|member| member.pid)
        .collect())
}

fn helper_has_exited_without_reaping(pid: u32) -> Result<bool, String> {
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("sample helper state {pid}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "sample helper state {pid} exited {}",
            output.status
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .any(|state| state.starts_with('Z')))
}

#[cfg(unix)]
fn signal_helper_process_group(process_group: u32, signal: &str) -> Result<(), String> {
    let signal_number = match signal {
        "TERM" => libc::SIGTERM,
        "KILL" => libc::SIGKILL,
        _ => return Err(format!("unsupported helper process group signal {signal}")),
    };
    let process_group = i32::try_from(process_group)
        .map_err(|_| format!("helper process group ID {process_group} exceeds i32"))?;
    let target = process_group
        .checked_neg()
        .ok_or_else(|| format!("invalid helper process group ID {process_group}"))?;
    // SAFETY: a negative, checked process-group ID targets only the child group we created.
    if unsafe { libc::kill(target, signal_number) } == -1 {
        return Err(format!(
            "signal {signal} helper process group {process_group}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PsEntry {
    pid: u32,
    ppid: u32,
    rss_kib: u64,
    cpu_time: Duration,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TreeUsage {
    rss_bytes: u64,
    cpu_time: Duration,
}

fn sample_process_tree_usage(root_pid: u32) -> Result<TreeUsage, String> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,rss=,time="])
        .output()
        .map_err(|error| format!("spawn ps for RSS sample: {error}"))?;
    if !output.status.success() {
        return Err(format!("ps for RSS sample exited {}", output.status));
    }
    Ok(aggregate_tree_usage(
        root_pid,
        &parse_ps_table(&String::from_utf8_lossy(&output.stdout)),
    ))
}

fn parse_ps_table(text: &str) -> Vec<PsEntry> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some(PsEntry {
                pid: fields.next()?.parse().ok()?,
                ppid: fields.next()?.parse().ok()?,
                rss_kib: fields.next()?.parse().ok()?,
                cpu_time: parse_cpu_time(fields.next()?)?,
            })
        })
        .collect()
}

fn parse_cpu_time(value: &str) -> Option<Duration> {
    let (days, clock) = match value.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, value),
    };
    let fields: Vec<&str> = clock.split(':').collect();
    let (hours, minutes, seconds) = match fields.as_slice() {
        [minutes, seconds] => (0, minutes.parse::<u64>().ok()?, *seconds),
        [hours, minutes, seconds] => (
            hours.parse::<u64>().ok()?,
            minutes.parse::<u64>().ok()?,
            *seconds,
        ),
        _ => return None,
    };
    let (whole_seconds, fractional_seconds) = seconds
        .split_once('.')
        .map_or((seconds, ""), |(whole, fractional)| (whole, fractional));
    if fractional_seconds.len() > 9 || !fractional_seconds.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let seconds = whole_seconds.parse::<u64>().ok()?;
    let nanos = if fractional_seconds.is_empty() {
        0
    } else {
        let mut padded = fractional_seconds.to_string();
        padded.extend(std::iter::repeat_n('0', 9 - padded.len()));
        padded.parse::<u32>().ok()?
    };
    let total_seconds = days
        .checked_mul(24)?
        .checked_add(hours)?
        .checked_mul(60)?
        .checked_add(minutes)?
        .checked_mul(60)?
        .checked_add(seconds)?;
    Duration::from_secs(total_seconds).checked_add(Duration::from_nanos(u64::from(nanos)))
}

fn aggregate_tree_usage(root_pid: u32, entries: &[PsEntry]) -> TreeUsage {
    let mut seen = HashSet::from([root_pid]);
    loop {
        let mut changed = false;
        for entry in entries {
            if seen.contains(&entry.ppid) && seen.insert(entry.pid) {
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    entries
        .iter()
        .filter(|entry| seen.contains(&entry.pid))
        .fold(TreeUsage::default(), |usage, entry| TreeUsage {
            rss_bytes: usage
                .rss_bytes
                .saturating_add(entry.rss_kib.saturating_mul(1024)),
            cpu_time: usage.cpu_time.saturating_add(entry.cpu_time),
        })
}

#[derive(Debug, Serialize)]
struct Fixture {
    path: String,
    duration_s: u64,
    generated: bool,
    sha256: String,
    size_bytes: u64,
    ffmpeg_path: String,
    ffmpeg_version: String,
    generator_argv: Vec<String>,
    filesystem: Filesystem,
}

#[derive(Debug, Serialize, Deserialize)]
struct FixtureMetadata {
    duration_s: u64,
    generator_argv: Vec<String>,
    ffmpeg_path: String,
    ffmpeg_version: String,
    sha256: String,
}

fn prepare_fixture(args: &Args, ffmpeg: &Path) -> Result<Fixture, String> {
    let root = args.work_dir.join("fixture");
    fs::create_dir_all(&root).map_err(|error| format!("create {}: {error}", root.display()))?;
    let _guard = acquire_waveform_fixture_guard(&root, args.duration_s)?;
    let path = root.join(format!("mixed-aac-{}s.m4a", args.duration_s));
    let metadata_path = root.join(format!("fixture-{}s.json", args.duration_s));
    let argv = fixture_argv(args.duration_s, &path);
    recover_incomplete_fixture_pair(&path, &metadata_path)?;
    let (generated, metadata) = if path.exists() {
        let metadata: FixtureMetadata = serde_json::from_slice(
            &fs::read(&metadata_path)
                .map_err(|error| format!("read {}: {error}", metadata_path.display()))?,
        )
        .map_err(|error| format!("parse {}: {error}", metadata_path.display()))?;
        if metadata.duration_s != args.duration_s || metadata.generator_argv != argv {
            return Err(format!(
                "fixture metadata differs from requested deterministic fixture: {}",
                metadata_path.display()
            ));
        }
        let actual_sha256 = sha256_file(&path)?;
        if metadata.sha256 != actual_sha256 {
            return Err(format!(
                "fixture SHA differs from metadata: {}",
                path.display()
            ));
        }
        (false, metadata)
    } else {
        let temporary_path = temporary_sibling(&path, "fixture")?;
        let temporary_argv = fixture_argv(args.duration_s, &temporary_path);
        let output = match Command::new(ffmpeg).args(&temporary_argv).output() {
            Ok(output) => output,
            Err(error) => {
                let _ = fs::remove_file(&temporary_path);
                return Err(format!("spawn fixture ffmpeg: {error}"));
            }
        };
        if !output.status.success() {
            let _ = fs::remove_file(&temporary_path);
            return Err(format!(
                "fixture ffmpeg exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let publication = (|| {
            let sha256 = sha256_file(&temporary_path)?;
            let metadata = FixtureMetadata {
                duration_s: args.duration_s,
                // Keep the stable published path in the reproducible command;
                // generation differs only by writing its output to a sibling
                // temporary path before the atomic rename below.
                generator_argv: argv,
                ffmpeg_path: ffmpeg.display().to_string(),
                ffmpeg_version: ffmpeg_version(ffmpeg)?,
                sha256,
            };
            let metadata_bytes = serde_json::to_vec_pretty(&metadata)
                .map_err(|error| format!("serialize fixture metadata: {error}"))?;
            fs::rename(&temporary_path, &path).map_err(|error| {
                format!(
                    "publish fixture {} as {}: {error}",
                    temporary_path.display(),
                    path.display()
                )
            })?;
            write_atomically(&metadata_path, &metadata_bytes)?;
            Ok::<_, String>(metadata)
        })();
        if publication.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        let metadata = publication?;
        (true, metadata)
    };
    let size_bytes = fs::metadata(&path)
        .map_err(|error| format!("metadata {}: {error}", path.display()))?
        .len();
    Ok(Fixture {
        path: path.display().to_string(),
        duration_s: args.duration_s,
        generated,
        sha256: metadata.sha256,
        size_bytes,
        ffmpeg_path: metadata.ffmpeg_path,
        ffmpeg_version: metadata.ffmpeg_version,
        generator_argv: metadata.generator_argv,
        filesystem: filesystem(&path),
    })
}

#[derive(Debug)]
struct FixturePublicationGuard {
    lock: File,
}

impl Drop for FixturePublicationGuard {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}

fn acquire_waveform_fixture_guard(
    root: &Path,
    duration_s: u64,
) -> Result<FixturePublicationGuard, String> {
    fs::create_dir_all(root).map_err(|error| format!("create {}: {error}", root.display()))?;
    let path = root.join(format!(".fixture-{duration_s}s.lock"));
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let lock = options
        .open(&path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    let metadata = lock
        .metadata()
        .map_err(|error| format!("metadata {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("fixture lock is not a file: {}", path.display()));
    }
    match lock.try_lock() {
        Ok(()) => Ok(FixturePublicationGuard { lock }),
        Err(std::fs::TryLockError::WouldBlock) => Err(format!(
            "fixture publication already in progress: {}",
            root.display()
        )),
        Err(std::fs::TryLockError::Error(error)) => {
            Err(format!("lock {}: {error}", path.display()))
        }
    }
}

fn recover_incomplete_fixture_pair(path: &Path, metadata_path: &Path) -> Result<bool, String> {
    let path_exists = path
        .try_exists()
        .map_err(|error| format!("inspect {}: {error}", path.display()))?;
    let metadata_exists = metadata_path
        .try_exists()
        .map_err(|error| format!("inspect {}: {error}", metadata_path.display()))?;
    if path_exists == metadata_exists {
        return Ok(false);
    }
    for incomplete in [path, metadata_path] {
        if incomplete
            .try_exists()
            .map_err(|error| format!("inspect {}: {error}", incomplete.display()))?
        {
            fs::remove_file(incomplete)
                .map_err(|error| format!("remove incomplete {}: {error}", incomplete.display()))?;
        }
    }
    Ok(true)
}

fn fixture_argv(duration_s: u64, output: &Path) -> Vec<String> {
    let duration = duration_s.to_string();
    vec![
        "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
        &format!("sine=frequency=97:sample_rate=48000:duration={duration}"), "-f", "lavfi", "-i",
        &format!("sine=frequency=443:sample_rate=48000:duration={duration}"), "-f", "lavfi", "-i",
        &format!("anoisesrc=color=pink:sample_rate=48000:duration={duration}:seed=42"), "-filter_complex",
        "[0:a]volume=0.24[a0];[1:a]volume=0.16[a1];[2:a]volume=0.05[a2];[a0][a1][a2]amix=inputs=3:normalize=0,aformat=channel_layouts=stereo[a]",
        "-map", "[a]", "-c:a", "aac", "-b:a", "128k", "-threads", "1", "-movflags", "+faststart",
        "-metadata", "creation_time=1970-01-01T00:00:00Z", "-f", "ipod",
        &output.display().to_string(),
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn ffmpeg_version(ffmpeg: &Path) -> Result<String, String> {
    let output = Command::new(ffmpeg)
        .arg("-version")
        .output()
        .map_err(|error| format!("spawn {} -version: {error}", ffmpeg.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} -version exited {}",
            ffmpeg.display(),
            output.status
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("unknown")
        .to_string())
}

fn decoder_ffmpeg_provenance() -> Result<DecoderFfmpeg, String> {
    let path = ffmpeg_path().map_err(|error| error.to_string())?;
    Ok(DecoderFfmpeg {
        version: ffmpeg_version(&path)?,
        path: path.display().to_string(),
    })
}

fn build_reference_oracle(
    asset: &Path,
    work_dir: &Path,
    ffmpeg: &Path,
) -> Result<ReferenceOracle, String> {
    let oracle_dir = work_dir.join("reference-oracle");
    fs::create_dir_all(&oracle_dir)
        .map_err(|error| format!("create {}: {error}", oracle_dir.display()))?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let decoded_path = oracle_dir.join(format!("decoded-{}-{suffix}.f32le", std::process::id()));
    let decoder_argv = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-nostats".to_string(),
        "-y".to_string(),
        "-i".to_string(),
        asset.display().to_string(),
        "-map".to_string(),
        "0:a:0?".to_string(),
        "-ac".to_string(),
        "1".to_string(),
        "-ar".to_string(),
        WAVEFORM_SAMPLE_RATE_HZ.to_string(),
        "-f".to_string(),
        "f32le".to_string(),
        decoded_path.display().to_string(),
    ];
    let output = Command::new(ffmpeg)
        .args(&decoder_argv)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("spawn reference FFmpeg: {error}"))?;
    if !output.status.success() {
        let _ = fs::remove_file(&decoded_path);
        return Err(format!(
            "reference FFmpeg exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let decoded_bytes = fs::metadata(&decoded_path)
        .map_err(|error| format!("metadata {}: {error}", decoded_path.display()))?
        .len();
    if decoded_bytes == 0 || decoded_bytes % 4 != 0 {
        let _ = fs::remove_file(&decoded_path);
        return Err(format!(
            "reference FFmpeg produced {decoded_bytes} bytes, not complete non-empty f32le frames"
        ));
    }
    let decoded_sample_count = usize::try_from(decoded_bytes / 4)
        .map_err(|_| "reference decoded sample count does not fit usize".to_string())?;
    let bucket_result =
        reference_bucket_bits_from_f32le(&decoded_path, decoded_sample_count, BUCKETS);
    let remove_result = fs::remove_file(&decoded_path)
        .map_err(|error| format!("remove {}: {error}", decoded_path.display()));
    let bucket_bits = bucket_result?;
    remove_result?;

    let duration_s = decoded_sample_count as f64 / WAVEFORM_SAMPLE_RATE_HZ as f64;
    let invariants = validate_waveform_invariants(&bucket_bits, BUCKETS)?;
    let canonical_sha256 = canonical_waveform_sha256(duration_s, &bucket_bits);
    Ok(ReferenceOracle {
        evidence: ReferenceOracleEvidence {
            method: "independent direct FFmpeg f32le decode to a temporary file, followed by benchmark-owned exact peak bucketing",
            decoder_ffmpeg: DecoderFfmpeg {
                path: ffmpeg.display().to_string(),
                version: ffmpeg_version(ffmpeg)?,
            },
            decoder_argv,
            sample_rate_hz: WAVEFORM_SAMPLE_RATE_HZ,
            decoded_sample_count,
            duration_s,
            duration_bits: duration_bits_hex(duration_s),
            duration_tolerance_s: DURATION_TOLERANCE_S,
            canonical_sha256,
            invariants: OracleInvariants {
                decoded_f32_frames_complete: true,
                decoded_samples_finite: true,
                buckets: invariants,
                duration_derived_from_decoded_sample_count: true,
            },
        },
        bucket_bits,
    })
}

fn reference_bucket_bits_from_f32le(
    path: &Path,
    sample_count: usize,
    bucket_count: usize,
) -> Result<Vec<u32>, String> {
    if bucket_count == 0 || sample_count == 0 {
        return Ok(Vec::new());
    }
    let file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut buffer = [0_u8; 64 * 1024];
    let mut out = Vec::with_capacity(bucket_count);
    for index in 0..bucket_count {
        let (start, end) = reference_bucket_range(sample_count, bucket_count, index);
        let byte_offset = u64::try_from(start)
            .ok()
            .and_then(|value| value.checked_mul(4))
            .ok_or_else(|| "reference byte offset overflow".to_string())?;
        reader
            .seek(SeekFrom::Start(byte_offset))
            .map_err(|error| format!("seek {}: {error}", path.display()))?;
        let mut remaining = (end - start)
            .checked_mul(4)
            .ok_or_else(|| "reference bucket byte count overflow".to_string())?;
        let mut peak = 0.0_f32;
        while remaining > 0 {
            let count = remaining.min(buffer.len());
            reader
                .read_exact(&mut buffer[..count])
                .map_err(|error| format!("read {}: {error}", path.display()))?;
            for frame in buffer[..count].chunks_exact(4) {
                let sample = f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
                if !sample.is_finite() {
                    return Err("reference FFmpeg emitted a non-finite f32 sample".into());
                }
                peak = peak.max(sample.abs());
            }
            remaining -= count;
        }
        out.push(peak.min(1.0).to_bits());
    }
    Ok(out)
}

#[cfg(test)]
fn reference_bucket_bits(samples: &[f32], bucket_count: usize) -> Vec<u32> {
    if bucket_count == 0 || samples.is_empty() {
        return Vec::new();
    }
    (0..bucket_count)
        .map(|index| {
            let (start, end) = reference_bucket_range(samples.len(), bucket_count, index);
            samples[start..end]
                .iter()
                .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
                .min(1.0)
                .to_bits()
        })
        .collect()
}

fn reference_bucket_range(
    sample_count: usize,
    bucket_count: usize,
    index: usize,
) -> (usize, usize) {
    let start = (sample_count * index) / bucket_count;
    let end = (sample_count * (index + 1)) / bucket_count;
    if start < end {
        (start, end)
    } else {
        let start = start.min(sample_count - 1);
        (start, start + 1)
    }
}

fn validate_waveform_invariants(
    bucket_bits: &[u32],
    expected_bucket_count: usize,
) -> Result<WaveformInvariants, String> {
    if bucket_bits.len() != expected_bucket_count {
        return Err(format!(
            "expected {expected_bucket_count} buckets, got {}",
            bucket_bits.len()
        ));
    }
    let buckets: Vec<f32> = bucket_bits.iter().copied().map(f32::from_bits).collect();
    let finite = buckets.iter().all(|bucket| bucket.is_finite());
    let within_unit_interval = buckets.iter().all(|bucket| (0.0..=1.0).contains(bucket));
    let nonzero = buckets.iter().any(|bucket| *bucket > 0.0);
    let mixed_signal = bucket_bits.iter().copied().collect::<HashSet<_>>().len() > 1;
    if !finite || !within_unit_interval || !nonzero || !mixed_signal {
        return Err(format!(
            "waveform invariants failed: finite={finite}, within_unit_interval={within_unit_interval}, nonzero={nonzero}, mixed_signal={mixed_signal}"
        ));
    }
    Ok(WaveformInvariants {
        finite,
        within_unit_interval,
        nonzero,
        mixed_signal,
    })
}

fn duration_bits_hex(duration_s: f64) -> String {
    format!("{:016x}", duration_s.to_bits())
}

fn run_contract_probes(
    runtime: &tokio::runtime::Runtime,
    work_dir: &Path,
    ffmpeg: &Path,
    long_fixture: &Path,
) -> Result<Contracts, String> {
    let contracts_dir = work_dir.join("contracts");
    fs::create_dir_all(&contracts_dir)
        .map_err(|error| format!("create {}: {error}", contracts_dir.display()))?;
    let no_audio = contracts_dir.join("video-only.mp4");
    let output = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=16x16:rate=1:duration=1",
            "-an",
            "-c:v",
            "mpeg4",
        ])
        .arg(&no_audio)
        .output()
        .map_err(|error| format!("spawn no-audio fixture ffmpeg: {error}"))?;
    if !output.status.success() {
        return Err(format!("no-audio fixture ffmpeg exited {}", output.status));
    }
    let no_audio_waveform = runtime
        .block_on(generate_waveform(
            &no_audio,
            BUCKETS,
            CancellationToken::new(),
        ))
        .map_err(|error| format!("no-audio contract: {error}"))?;
    let bad_input = contracts_dir.join("bad-input.m4a");
    fs::write(&bad_input, b"not media")
        .map_err(|error| format!("write {}: {error}", bad_input.display()))?;
    let bad_input_rejected = runtime
        .block_on(generate_waveform(
            &bad_input,
            BUCKETS,
            CancellationToken::new(),
        ))
        .is_err();
    let cancellation_rejected = runtime.block_on(async {
        let fixture = long_fixture.to_path_buf();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let task =
            tokio::spawn(async move { generate_waveform(&fixture, BUCKETS, worker_cancel).await });
        tokio::task::yield_now().await;
        tokio::time::sleep(CANCELLATION_DELAY).await;
        cancel.cancel();
        task.await
            .map_err(|error| format!("cancellation probe join: {error}"))
            .map(|result| {
                matches!(
                    result,
                    Err(FfmpegError::NonZero {
                        code: -1,
                        ref stderr_tail,
                    }) if stderr_tail == "cancelled"
                )
            })
    })?;
    if !no_audio_waveform.buckets.is_empty()
        || no_audio_waveform.duration_s != 0.0
        || !bad_input_rejected
        || !cancellation_rejected
    {
        return Err("waveform contract probe failed".into());
    }
    Ok(Contracts {
        no_audio_returns_empty: true,
        bad_input_rejected: true,
        cancellation_rejected: true,
        cancellation_delay_ms: CANCELLATION_DELAY.as_millis() as u64,
    })
}

fn assert_matches_oracle(
    oracle: &ReferenceOracle,
    result: &HelperResult,
    name: &str,
) -> Result<(), String> {
    validate_waveform_invariants(&result.bucket_bits, BUCKETS)?;
    let duration_delta_s = (oracle.evidence.duration_s - result.duration_s).abs();
    if duration_delta_s > DURATION_TOLERANCE_S
        || oracle.evidence.duration_bits != result.duration_bits
        || oracle.bucket_bits != result.bucket_bits
        || oracle.evidence.canonical_sha256 != result.canonical_sha256
        || oracle.evidence.decoder_ffmpeg != result.decoder_ffmpeg
    {
        return Err(format!(
            "{name} differs from the independent reference oracle (duration delta {duration_delta_s:.9}s; tolerance {DURATION_TOLERANCE_S:.9}s)"
        ));
    }
    Ok(())
}

fn assert_same_waveform(
    baseline: &HelperResult,
    sample: &HelperResult,
    index: usize,
) -> Result<(), String> {
    if baseline.duration_bits != sample.duration_bits
        || baseline.bucket_bits != sample.bucket_bits
        || baseline.canonical_sha256 != sample.canonical_sha256
        || baseline.decoder_ffmpeg != sample.decoder_ffmpeg
    {
        return Err(format!(
            "sample {index} waveform bits or decoder provenance differ from the warmup baseline"
        ));
    }
    Ok(())
}

fn canonical_waveform_sha256(duration_s: f64, bucket_bits: &[u32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"montage-waveform-v1\0");
    hasher.update(duration_s.to_bits().to_le_bytes());
    hasher.update((bucket_bits.len() as u64).to_le_bytes());
    for bucket in bucket_bits {
        hasher.update(bucket.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[derive(Debug, Serialize)]
struct Statistics {
    wall_ms: Distribution,
    peak_rss_bytes: Distribution,
    peak_cpu_ms: Distribution,
}

#[derive(Debug, Serialize)]
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
    let median_ms = median(&sorted);
    let mut deviations: Vec<f64> = sorted
        .iter()
        .map(|value| (value - median_ms).abs())
        .collect();
    deviations.sort_by(f64::total_cmp);
    let p95_index = (sorted.len() * 95).div_ceil(100).saturating_sub(1);
    Distribution {
        median: median_ms,
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

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    schema_version: u32,
    generated_at_utc: String,
    configuration: Configuration,
    fixture: Fixture,
    decoder_ffmpeg: DecoderFfmpeg,
    reference_oracle: ReferenceOracleEvidence,
    provenance: Provenance,
    contracts: Contracts,
    warmup: Sample,
    samples: Vec<Sample>,
    statistics: Statistics,
    correctness: Correctness,
    disk_io: DiskIo,
}

#[derive(Debug, Serialize)]
struct Configuration {
    label: String,
    duration_s: u64,
    buckets: usize,
    warmups: usize,
    samples: usize,
    work_dir: String,
    evidence_dir: String,
}

#[derive(Debug, Serialize)]
struct Correctness {
    canonical_sha256: String,
    duration_bits: String,
    bucket_count: usize,
    every_timed_sample_equal: bool,
}

#[derive(Debug, Serialize)]
struct Contracts {
    no_audio_returns_empty: bool,
    bad_input_rejected: bool,
    cancellation_rejected: bool,
    cancellation_delay_ms: u64,
}

#[derive(Debug)]
struct ReferenceOracle {
    evidence: ReferenceOracleEvidence,
    bucket_bits: Vec<u32>,
}

#[derive(Debug, Serialize)]
struct ReferenceOracleEvidence {
    method: &'static str,
    decoder_ffmpeg: DecoderFfmpeg,
    decoder_argv: Vec<String>,
    sample_rate_hz: usize,
    decoded_sample_count: usize,
    duration_s: f64,
    duration_bits: String,
    duration_tolerance_s: f64,
    canonical_sha256: String,
    invariants: OracleInvariants,
}

#[derive(Debug, Serialize)]
struct OracleInvariants {
    decoded_f32_frames_complete: bool,
    decoded_samples_finite: bool,
    buckets: WaveformInvariants,
    duration_derived_from_decoded_sample_count: bool,
}

#[derive(Debug, Serialize)]
struct WaveformInvariants {
    finite: bool,
    within_unit_interval: bool,
    nonzero: bool,
    mixed_signal: bool,
}

#[derive(Debug, Serialize)]
struct DiskIo {
    available: bool,
    reason: String,
}

#[derive(Debug, Serialize)]
struct Provenance {
    git: GitProvenance,
    source: SourceProvenance,
    build: BuildProvenance,
    cache: CacheProvenance,
    machine: Machine,
}

#[derive(Debug, Serialize)]
struct GitProvenance {
    head: Option<String>,
    dirty: Option<String>,
}

#[derive(Debug, Serialize)]
struct SourceProvenance {
    ffmpeg_rs_sha256: String,
    benchmark_source_sha256: String,
}

#[derive(Debug, Serialize)]
struct BuildProvenance {
    package_version: String,
    profile: &'static str,
    executable: String,
    executable_sha256: String,
    cargo_lock_sha256: String,
    rustc_version_verbose: String,
    cargo_version: String,
    rustup_toolchain: Option<String>,
}

#[derive(Debug, Serialize)]
struct CacheProvenance {
    helper_processes: &'static str,
    fixture: &'static str,
}

#[derive(Debug, Serialize)]
struct Machine {
    os: &'static str,
    arch: &'static str,
    parallelism: usize,
    work_filesystem: Filesystem,
    evidence_filesystem: Filesystem,
}

#[derive(Debug, Serialize)]
struct Filesystem {
    filesystem_type: Option<String>,
    device: Option<String>,
    mount: Option<String>,
}

fn report_provenance(
    work_dir: &Path,
    evidence_dir: &Path,
    executable: &Path,
) -> Result<Provenance, String> {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cargo_lock = workspace_root.join("Cargo.lock");
    Ok(Provenance {
        git: GitProvenance {
            head: git_output(&["rev-parse", "HEAD"]),
            dirty: git_output(&["status", "--porcelain"]),
        },
        source: SourceProvenance {
            ffmpeg_rs_sha256: sha256_bytes(include_bytes!("../ffmpeg.rs")),
            benchmark_source_sha256: sha256_bytes(include_bytes!("montage-waveform-perf.rs")),
        },
        build: BuildProvenance {
            package_version: env!("CARGO_PKG_VERSION").into(),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            executable: executable.display().to_string(),
            executable_sha256: sha256_file(executable)?,
            cargo_lock_sha256: sha256_file(&cargo_lock)?,
            rustc_version_verbose: command_stdout("rustc", &["-Vv"])?,
            cargo_version: command_stdout("cargo", &["-V"])?,
            rustup_toolchain: std::env::var("RUSTUP_TOOLCHAIN").ok(),
        },
        cache: CacheProvenance {
            helper_processes: "fresh process for warmup and each timed sample",
            fixture: "fixture generation excluded from timing; OS page cache is not cleared",
        },
        machine: Machine {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            parallelism: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
            work_filesystem: filesystem(work_dir),
            evidence_filesystem: filesystem(evidence_dir),
        },
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

fn git_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn filesystem(path: &Path) -> Filesystem {
    let df = Command::new("df")
        .args(["-P"])
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
    let (device, mount) = df.as_deref().map(parse_df_output).unwrap_or((None, None));
    Filesystem {
        filesystem_type: filesystem_type(mount.as_deref().map(Path::new).unwrap_or(path)),
        device,
        mount,
    }
}

fn parse_df_output(text: &str) -> (Option<String>, Option<String>) {
    let fields: Vec<&str> = text
        .lines()
        .last()
        .map(str::split_whitespace)
        .map(Iterator::collect)
        .unwrap_or_default();
    if fields.len() < 6 {
        return (None, None);
    }
    (Some(fields[0].to_string()), Some(fields[5..].join(" ")))
}

#[cfg(target_os = "macos")]
fn filesystem_type(path: &Path) -> Option<String> {
    let output = Command::new("diskutil")
        .arg("info")
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.trim().strip_prefix("File System Personality:"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(not(target_os = "macos"))]
fn filesystem_type(path: &Path) -> Option<String> {
    Command::new("stat")
        .args(["-f", "-c", "%T"])
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
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

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("no parent for {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let temporary = temporary_sibling(path, "report")?;
    fs::write(&temporary, bytes)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "rename {} to {}: {error}",
            temporary.display(),
            path.display()
        ));
    }
    Ok(())
}

fn temporary_sibling(path: &Path, fallback_name: &str) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("no parent for {}", path.display()))?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(fallback_name);
    let process_id = std::process::id();
    let name = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map_or_else(
            || format!(".{stem}-{process_id}-{suffix}.tmp"),
            |extension| format!(".{stem}-{process_id}-{suffix}.tmp.{extension}"),
        );
    Ok(parent.join(name))
}

fn write_atomically_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("no parent for {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary = parent.join(format!(
        ".{}-{}-{suffix}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("report"),
        std::process::id()
    ));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    let link_result = fs::hard_link(&temporary, path);
    let remove_result = fs::remove_file(&temporary);
    link_result.map_err(|error| {
        format!(
            "publish new report {} without overwrite: {error}",
            path.display()
        )
    })?;
    remove_result.map_err(|error| format!("remove {}: {error}", temporary.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn helper_cleanup_signals_before_reaping_the_group_leader() {
        use std::cell::RefCell;
        use std::collections::VecDeque;

        let events = RefCell::new(Vec::<String>::new());
        let observations = RefCell::new(VecDeque::from([false, true]));
        finish_helper_process_group_cleanup(
            |signal| {
                events.borrow_mut().push(signal.into());
                Ok(())
            },
            |_| {
                events.borrow_mut().push("observe".into());
                Ok(observations.borrow_mut().pop_front().unwrap())
            },
            || {
                events.borrow_mut().push("reap".into());
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(
            events.into_inner(),
            vec!["TERM", "observe", "KILL", "observe", "reap"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn helper_cleanup_does_not_reap_when_signaling_or_quiescence_fails() {
        use std::cell::Cell;

        let reaped = Cell::new(false);
        let result = finish_helper_process_group_cleanup(
            |_| Err("cannot signal helper process group".into()),
            |_| Ok(false),
            || {
                reaped.set(true);
                Ok(())
            },
        );
        let final_quiescence_reaped = Cell::new(false);
        let final_quiescence_result = finish_helper_process_group_cleanup(
            |_| Ok(()),
            |_| Ok(false),
            || {
                final_quiescence_reaped.set(true);
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(final_quiescence_result.is_err());
        assert!(
            !reaped.get(),
            "cleanup must not reap before a successful final signal and quiescence check"
        );
        assert!(
            !final_quiescence_reaped.get(),
            "cleanup must not reap after a final non-quiescent observation"
        );
    }

    #[cfg(unix)]
    #[test]
    fn term_ignoring_leader_requires_kill_before_reap() {
        use std::os::unix::process::CommandExt;

        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("ready");
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                "trap '' TERM; : > \"$1\"; while :; do sleep 30; done",
                "term-ignoring-helper",
            ])
            .arg(&ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let mut child = command.spawn().unwrap();
        let process_group = child.id();
        let cleanup = TestProcessGroup::new(process_group);
        wait_for_test_file(&ready);
        assert!(test_process_exists(process_group));

        signal_helper_process_group(process_group, "TERM").unwrap();
        assert!(test_process_exists(process_group));

        assert!(
            !wait_for_helper_process_group_quiescent(process_group, Duration::ZERO).unwrap(),
            "a live TERM-ignoring leader must force SIGKILL before reap"
        );
        finish_helper_process_group_cleanup(
            |signal| signal_helper_process_group(process_group, signal),
            |grace| wait_for_helper_process_group_quiescent(process_group, grace),
            || reap_helper_child_with_timeout(&mut child, HELPER_TERMINATION_GRACE),
        )
        .unwrap();
        assert!(!process_group_exists(process_group));
        drop(cleanup);
    }

    #[cfg(unix)]
    #[test]
    fn monitor_sampling_error_terminates_and_reaps_the_helper_process_group() {
        let directory = tempfile::tempdir().unwrap();
        let descendant_pid = directory.path().join("descendant.pid");
        let mut command = Command::new("/bin/sh");
        command
            .args([
                "-c",
                &format!(
                    "sleep 30 & echo $! > \"{}\" && wait",
                    descendant_pid.display()
                ),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_helper_process_group(&mut command);
        let child = command.spawn().unwrap();
        let process_group = child.id();
        let cleanup = TestProcessGroup::new(process_group);
        let descendant = wait_for_test_pid(&descendant_pid);
        assert!(test_process_exists(descendant));

        let result = monitor_process_tree_with_sampler(child, Instant::now(), |_| {
            Err("intentional sampler failure".into())
        });

        assert!(result.is_err());
        assert!(
            !process_group_exists(process_group),
            "sampling failure left helper process group {process_group} alive"
        );
        assert!(
            !test_process_exists(descendant),
            "sampling failure left descendant {descendant} alive"
        );
        drop(cleanup);
    }

    #[cfg(unix)]
    struct TestProcessGroup(u32);

    #[cfg(unix)]
    impl TestProcessGroup {
        fn new(process_group: u32) -> Self {
            Self(process_group)
        }
    }

    #[cfg(unix)]
    impl Drop for TestProcessGroup {
        fn drop(&mut self) {
            let _ = signal_helper_process_group(self.0, "KILL");
        }
    }

    #[cfg(unix)]
    fn process_group_exists(process_group: u32) -> bool {
        let Ok(process_group) = i32::try_from(process_group) else {
            return false;
        };
        let Some(target) = process_group.checked_neg() else {
            return false;
        };
        // SAFETY: signal zero checks the existence of the specified process group.
        unsafe { libc::kill(target, 0) == 0 }
    }

    #[cfg(unix)]
    fn wait_for_test_pid(path: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Ok(value) = fs::read_to_string(path)
                && let Ok(pid) = value.trim().parse()
            {
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "descendant PID was never recorded"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn wait_for_test_file(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.is_file() {
            assert!(Instant::now() < deadline, "test helper never became ready");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn test_process_exists(pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        // SAFETY: signal zero checks the existence of the specified process.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    #[test]
    fn parses_ps_rows_and_ignores_headers_or_malformed_rows() {
        let entries = parse_ps_table(
            "  PID  PPID  RSS TIME\n  41  1  1024 0:00.01\n  42  41  2048 1:02:03\nnot a row\n",
        );
        assert_eq!(
            entries,
            vec![
                PsEntry {
                    pid: 41,
                    ppid: 1,
                    rss_kib: 1024,
                    cpu_time: Duration::from_millis(10),
                },
                PsEntry {
                    pid: 42,
                    ppid: 41,
                    rss_kib: 2048,
                    cpu_time: Duration::from_secs(3_723),
                },
            ]
        );
    }

    #[test]
    fn parses_ps_cpu_time_with_days_hours_minutes_and_fractional_seconds() {
        assert_eq!(
            parse_cpu_time("2-03:04:05.67"),
            Some(Duration::from_millis(183_845_670))
        );
        assert_eq!(
            parse_cpu_time("04:05.6"),
            Some(Duration::from_millis(245_600))
        );
        assert_eq!(parse_cpu_time("not-a-time"), None);
    }

    #[test]
    fn sums_the_helper_and_all_recursive_descendants_once() {
        let entries = [
            PsEntry {
                pid: 10,
                ppid: 1,
                rss_kib: 4,
                cpu_time: Duration::from_millis(10),
            },
            PsEntry {
                pid: 11,
                ppid: 10,
                rss_kib: 8,
                cpu_time: Duration::from_millis(20),
            },
            PsEntry {
                pid: 12,
                ppid: 11,
                rss_kib: 16,
                cpu_time: Duration::from_millis(30),
            },
            PsEntry {
                pid: 13,
                ppid: 1,
                rss_kib: 32,
                cpu_time: Duration::from_millis(40),
            },
        ];
        let usage = aggregate_tree_usage(10, &entries);
        assert_eq!(usage.rss_bytes, 28 * 1024);
        assert_eq!(usage.cpu_time, Duration::from_millis(60));
    }

    #[test]
    fn summary_uses_nearest_rank_p95_and_median_absolute_deviation() {
        let stats = summarize_distribution(&[10.0, 12.0, 11.0, 50.0, 13.0]);
        assert_eq!(stats.median, 12.0);
        assert_eq!(stats.p95, 50.0);
        assert_eq!(stats.mad, 1.0);
    }

    #[test]
    fn canonical_hash_covers_duration_bits_and_every_bucket_bit() {
        let first = canonical_waveform_sha256(1.0, &[0x3f80_0000, 0x4000_0000]);
        assert_ne!(
            first,
            canonical_waveform_sha256(1.5, &[0x3f80_0000, 0x4000_0000])
        );
        assert_ne!(
            first,
            canonical_waveform_sha256(1.0, &[0x3f80_0001, 0x4000_0000])
        );
    }

    #[test]
    fn duration_bits_are_fixed_width_hex_text() {
        assert_eq!(duration_bits_hex(12.01075), "402805810624dd2f");
        assert_eq!(duration_bits_hex(0.0), "0000000000000000");
    }

    #[test]
    fn parses_df_mount_paths_with_spaces() {
        let output = "Filesystem 512-blocks Used Available Capacity Mounted on\n/dev/disk6s2 1000000 200000 800000 20% /Volumes/My Passport for Mac\n";
        assert_eq!(
            parse_df_output(output),
            (
                Some("/dev/disk6s2".to_string()),
                Some("/Volumes/My Passport for Mac".to_string()),
            )
        );
    }

    #[test]
    fn reference_bucketing_matches_exact_slice_and_fallback_semantics() {
        assert_eq!(
            reference_bucket_bits(&[-0.25, 0.5], 4),
            vec![
                0.25_f32.to_bits(),
                0.25_f32.to_bits(),
                0.5_f32.to_bits(),
                0.5_f32.to_bits(),
            ]
        );
        assert_eq!(
            reference_bucket_bits(&[-1.25, 0.125, -0.75, 0.5], 2),
            vec![1.0_f32.to_bits(), 0.75_f32.to_bits()]
        );
        assert!(reference_bucket_bits(&[], 2).is_empty());
        assert!(reference_bucket_bits(&[0.5], 0).is_empty());
    }

    #[test]
    fn waveform_invariants_require_finite_in_range_nonzero_mixed_signal() {
        let valid = [0.0_f32.to_bits(), 0.25_f32.to_bits(), 0.5_f32.to_bits()];
        let invariants = validate_waveform_invariants(&valid, valid.len()).unwrap();
        assert!(invariants.finite);
        assert!(invariants.within_unit_interval);
        assert!(invariants.nonzero);
        assert!(invariants.mixed_signal);

        assert!(validate_waveform_invariants(&[0.0_f32.to_bits(); 3], 3).is_err());
        assert!(validate_waveform_invariants(&[0.25_f32.to_bits(); 3], 3).is_err());
        assert!(validate_waveform_invariants(&[f32::NAN.to_bits(), 0.5_f32.to_bits()], 2).is_err());
        assert!(validate_waveform_invariants(&[1.25_f32.to_bits(), 0.5_f32.to_bits()], 2).is_err());
    }

    #[test]
    fn rejects_a_timed_sample_from_a_different_decoder() {
        let baseline = HelperResult {
            duration_s: 1.0,
            duration_bits: duration_bits_hex(1.0),
            canonical_sha256: "same".into(),
            bucket_bits: vec![1],
            decoder_ffmpeg: DecoderFfmpeg {
                path: "/first/ffmpeg".into(),
                version: "first".into(),
            },
        };
        let mut sample = baseline.clone();
        sample.decoder_ffmpeg.path = "/second/ffmpeg".into();
        assert!(assert_same_waveform(&baseline, &sample, 1).is_err());
    }

    #[test]
    fn fixture_metadata_is_isolated_by_requested_duration() {
        let work_dir = tempfile::tempdir().unwrap();
        let root = work_dir.path().join("fixture");
        fs::create_dir_all(&root).unwrap();

        let first_duration_s = 2;
        let second_duration_s = 3;
        let first_path = root.join(format!("mixed-aac-{first_duration_s}s.m4a"));
        let second_path = root.join(format!("mixed-aac-{second_duration_s}s.m4a"));
        fs::write(&first_path, b"first fixture").unwrap();
        fs::write(&second_path, b"second fixture").unwrap();

        let first_metadata = FixtureMetadata {
            duration_s: first_duration_s,
            generator_argv: fixture_argv(first_duration_s, &first_path),
            ffmpeg_path: "fixture ffmpeg".into(),
            ffmpeg_version: "fixture version".into(),
            sha256: sha256_file(&first_path).unwrap(),
        };
        let second_metadata = FixtureMetadata {
            duration_s: second_duration_s,
            generator_argv: fixture_argv(second_duration_s, &second_path),
            ffmpeg_path: "fixture ffmpeg".into(),
            ffmpeg_version: "fixture version".into(),
            sha256: sha256_file(&second_path).unwrap(),
        };
        fs::write(
            root.join(format!("fixture-{first_duration_s}s.json")),
            serde_json::to_vec(&first_metadata).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join(format!("fixture-{second_duration_s}s.json")),
            serde_json::to_vec(&second_metadata).unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("fixture.json"),
            serde_json::to_vec(&first_metadata).unwrap(),
        )
        .unwrap();

        let first = prepare_fixture(
            &Args {
                work_dir: work_dir.path().to_path_buf(),
                evidence_dir: work_dir.path().join("evidence"),
                label: "test".into(),
                duration_s: first_duration_s,
            },
            Path::new("ffmpeg-not-needed"),
        )
        .expect("first fixture uses its own metadata");
        let second = prepare_fixture(
            &Args {
                work_dir: work_dir.path().to_path_buf(),
                evidence_dir: work_dir.path().join("evidence"),
                label: "test".into(),
                duration_s: second_duration_s,
            },
            Path::new("ffmpeg-not-needed"),
        )
        .expect("second fixture uses its own metadata");

        assert_eq!(first.duration_s, first_duration_s);
        assert_eq!(second.duration_s, second_duration_s);
    }

    #[test]
    fn incomplete_fixture_pairs_are_recovered_without_masking_complete_corruption() {
        for (media_exists, metadata_exists) in [(true, false), (false, true)] {
            let root = tempfile::tempdir().unwrap();
            let media = root.path().join("fixture.m4a");
            let metadata = root.path().join("fixture.json");
            if media_exists {
                fs::write(&media, b"partial media").unwrap();
            }
            if metadata_exists {
                fs::write(&metadata, b"partial metadata").unwrap();
            }

            assert!(recover_incomplete_fixture_pair(&media, &metadata).unwrap());
            assert!(!media.exists());
            assert!(!metadata.exists());
        }

        let root = tempfile::tempdir().unwrap();
        let media = root.path().join("fixture.m4a");
        let metadata = root.path().join("fixture.json");
        fs::write(&media, b"corrupt media").unwrap();
        fs::write(&metadata, b"corrupt metadata").unwrap();

        assert!(!recover_incomplete_fixture_pair(&media, &metadata).unwrap());
        assert_eq!(fs::read(&media).unwrap(), b"corrupt media");
        assert_eq!(fs::read(&metadata).unwrap(), b"corrupt metadata");
    }

    #[test]
    fn complete_corrupt_fixture_pair_still_fails_closed() {
        let work_dir = tempfile::tempdir().unwrap();
        let root = work_dir.path().join("fixture");
        fs::create_dir_all(&root).unwrap();
        let media = root.join("mixed-aac-3s.m4a");
        let metadata = root.join("fixture-3s.json");
        fs::write(&media, b"corrupt media").unwrap();
        fs::write(&metadata, b"corrupt metadata").unwrap();

        let error = prepare_fixture(
            &Args {
                work_dir: work_dir.path().to_path_buf(),
                evidence_dir: work_dir.path().join("evidence"),
                label: "test".into(),
                duration_s: 3,
            },
            Path::new("ffmpeg-must-not-run"),
        )
        .expect_err("complete corrupt fixture pair rejected");

        assert!(error.contains("parse"));
        assert_eq!(fs::read(&media).unwrap(), b"corrupt media");
        assert_eq!(fs::read(&metadata).unwrap(), b"corrupt metadata");
    }

    #[test]
    fn temporary_fixture_path_preserves_the_media_extension() {
        let final_path = Path::new("/fixture/mixed-aac-3s.m4a");
        let temporary = temporary_sibling(final_path, "fixture").unwrap();

        assert_eq!(temporary.parent(), final_path.parent());
        assert_eq!(temporary.extension(), final_path.extension());
        assert_ne!(temporary, final_path);
    }

    #[test]
    fn fixture_publication_guard_is_exclusive_and_releases() {
        let root = tempfile::tempdir().unwrap();
        let first = acquire_waveform_fixture_guard(root.path(), 3).unwrap();

        let error = acquire_waveform_fixture_guard(root.path(), 3)
            .expect_err("concurrent fixture publication rejected");
        assert!(error.contains("already in progress"));

        drop(first);
        acquire_waveform_fixture_guard(root.path(), 3).expect("guard released");
    }
}
