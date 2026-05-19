//! Background ffmpeg render-job manager.
//!
//! Modeled on the corpus survey:
//! - Crush's `BackgroundShellManager` (`harnesses/crush/internal/agent/
//!   tools/job_output.go`): in-process map of `JobHandle`s, stdout/stderr
//!   captured into `tokio::sync::watch` channels for non-blocking snapshot
//!   reads.
//! - Codex's `agent_jobs.rs`: 30-min default timeout, 250ms status-poll
//!   cadence, child kill on cancellation.
//!
//! What lives here: the spawn-and-track machinery + status struct. The
//! `start_render` / `poll_render` MCP tool handlers live in `awidat-core`
//! and consume this crate's surface. Frame-strip extraction reuses
//! [`crate::ffmpeg::extract_frame`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{RwLock, watch};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::ffmpeg::{FfmpegError, ffmpeg_path};
use crate::progress::{ProgressSnapshot, parse_progress_line};

/// Server-allocated job id. Lower-case hex of 8 bytes — short enough for
/// the model to copy-paste, long enough that collisions are negligible.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct JobId(pub String);

impl JobId {
    fn fresh() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        // Process-monotonic counter mixed with an epoch nibble. Not
        // cryptographic — just unique across jobs in one session.
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self(format!("{:08x}{:08x}", epoch as u32, n as u32))
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Coarse state of a render job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Spawned, waiting for first ffmpeg output.
    Queued,
    /// Producing frames.
    Running,
    /// Exited 0; output file written.
    Done,
    /// Exited non-zero or transport failure.
    Failed,
    /// Cancelled mid-flight via the JobManager API.
    Cancelled,
}

/// What `poll_render` sees. Cheap snapshot, no allocations behind the
/// scenes (the underlying watch channel keeps a fresh value without
/// blocking the producer).
#[derive(Debug, Clone, Serialize)]
pub struct JobStatus {
    /// Job id (echo).
    pub id: JobId,
    /// Coarse state.
    pub state: JobState,
    /// 0..=100, computed from `time_done_s / total_duration_s` if both
    /// known.
    pub progress_pct: Option<f64>,
    /// Frames processed so far (parsed from ffmpeg `frame=`).
    pub frames_done: Option<u64>,
    /// Source duration in seconds (set when the job was started; stays
    /// constant).
    pub total_duration_s: Option<f64>,
    /// Output time the encoder has reached (parsed from `time=`).
    pub time_done_s: Option<f64>,
    /// Speed factor (parsed from `speed=`).
    pub speed: Option<f64>,
    /// ETA in seconds, if computable.
    pub eta_s: Option<f64>,
    /// Tail-truncated stderr (~16KB cap). Errors live at the end of
    /// ffmpeg's stderr, never the middle.
    pub log_excerpt: String,
    /// Full command argv used to launch ffmpeg, including the ffmpeg
    /// binary path as argv[0]. Exposed for reproducible render evidence.
    pub command_argv: Vec<String>,
    /// Path the output is being written to. Always set.
    pub output_path: PathBuf,
    /// Wall-clock at `start_render` (server-local).
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Exit code if the process has exited; None while running.
    pub exit_code: Option<i32>,
}

/// Errors from the JobManager.
#[derive(Debug, Error)]
pub enum JobError {
    /// No job with that id (or it was reaped).
    #[error("no such render job: {0}")]
    UnknownJob(JobId),
    /// ffmpeg binary couldn't be located.
    #[error("ffmpeg unavailable: {0}")]
    Ffmpeg(#[from] FfmpegError),
    /// Spawn failed.
    #[error("failed to spawn ffmpeg: {0}")]
    Spawn(String),
    /// Job already cancelled / done.
    #[error("job {0} is already terminal ({1:?}); cannot cancel")]
    AlreadyTerminal(JobId, JobState),
    /// Active render cap reached before a new ffmpeg process was spawned.
    #[error("render job capacity reached: {active_jobs}/{max_running} active jobs")]
    AtCapacity {
        /// Configured active render cap.
        max_running: usize,
        /// Active queued/running jobs at the time of rejection.
        active_jobs: usize,
    },
}

/// A render-job spec the JobManager can run. Pre-built ffmpeg argv so the
/// caller decides codec/preset/output path; the manager only spawns.
#[derive(Debug, Clone)]
pub struct RenderJobSpec {
    /// argv for the ffmpeg invocation, NOT including the binary itself.
    /// Example: `["-y", "-i", "raw/x.mp4", "-c:v", "libx264", "-crf",
    /// "23", "renders/preview.mp4"]`.
    pub args: Vec<String>,
    /// Total source duration in seconds, if known. Used to compute
    /// progress percentage and ETA.
    pub total_duration_s: Option<f64>,
    /// Working directory for ffmpeg. Usually the project root so relative
    /// `-i` and output paths resolve correctly.
    pub cwd: Option<PathBuf>,
    /// Output path the model can read after `Done`. Used to resolve frame-
    /// strip extraction at poll time.
    pub output_path: PathBuf,
    /// Non-fatal render planning limitations. These are explicit records of
    /// animation/effect metadata that was ignored so export callers can
    /// report partial parity instead of silently dropping it.
    pub limitations: Vec<RenderPlanLimitation>,
}

/// Non-fatal render planning limitation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RenderPlanLimitation {
    /// Stable machine-readable limitation kind.
    pub kind: String,
    /// Stable animation id when the limitation came from an animation record.
    pub animation_id: Option<String>,
    /// Target clip id when available.
    pub clip_id: Option<String>,
    /// Target parameter path when available.
    pub parameter: Option<String>,
    /// User-facing explanation.
    pub message: String,
}

/// Cap on the in-memory log buffer per job. Tail-truncated.
const STDERR_LOG_CAP_BYTES: usize = 16 * 1024;

/// Default timeout per render job. After this, the manager kills the
/// child and marks the job Failed. 30 minutes mirrors Codex's
/// `DEFAULT_AGENT_JOB_ITEM_TIMEOUT`.
pub const DEFAULT_JOB_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Hold time for the watch channel before the JobHandle is reaped after
/// completion. Lets the model poll once more after `Done` arrives.
const POST_TERMINAL_RETENTION: Duration = Duration::from_secs(5 * 60);

/// In-process job tracker. Cheaply cloneable.
#[derive(Clone)]
pub struct JobManager {
    inner: Arc<JobManagerInner>,
}

struct JobManagerInner {
    jobs: RwLock<HashMap<JobId, JobHandle>>,
    max_running: usize,
}

/// Default number of concurrently active render jobs.
pub const DEFAULT_MAX_RUNNING_JOBS: usize = 2;

/// Per-job tracking record.
struct JobHandle {
    rx: watch::Receiver<JobStatus>,
    cancel: CancellationToken,
}

impl JobManager {
    /// Construct an empty manager.
    pub fn new() -> Self {
        Self::with_max_running(configured_max_running_jobs())
    }

    /// Construct an empty manager with an explicit active render cap.
    pub fn with_max_running(max_running: usize) -> Self {
        Self {
            inner: Arc::new(JobManagerInner {
                jobs: RwLock::new(HashMap::new()),
                max_running,
            }),
        }
    }

    /// Spawn an ffmpeg invocation per `spec`. Returns the freshly-allocated
    /// [`JobId`] immediately; the job runs in the background.
    pub async fn start(&self, spec: RenderJobSpec) -> Result<JobId, JobError> {
        let bin = ffmpeg_path()?;
        let id = JobId::fresh();
        let started_at = chrono::Utc::now();
        let command_argv = std::iter::once(bin.to_string_lossy().into_owned())
            .chain(spec.args.iter().cloned())
            .collect();

        let initial = JobStatus {
            id: id.clone(),
            state: JobState::Queued,
            progress_pct: None,
            frames_done: None,
            total_duration_s: spec.total_duration_s,
            time_done_s: None,
            speed: None,
            eta_s: None,
            log_excerpt: String::new(),
            command_argv,
            output_path: spec.output_path.clone(),
            started_at,
            exit_code: None,
        };

        let (tx, rx) = watch::channel(initial.clone());
        let cancel = CancellationToken::new();

        let mut cmd = Command::new(&bin);
        cmd.args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }

        let mut jobs = self.inner.jobs.write().await;
        let active_jobs = active_job_count(&jobs);
        if active_jobs >= self.inner.max_running {
            return Err(JobError::AtCapacity {
                max_running: self.inner.max_running,
                active_jobs,
            });
        }

        let child = cmd
            .spawn()
            .map_err(|e| JobError::Spawn(format!("{bin}: {e}", bin = bin.display())))?;

        let runner_cancel = cancel.clone();
        let manager = self.clone();
        let id_for_runner = id.clone();
        tokio::spawn(async move {
            run_job(child, tx, runner_cancel, spec.total_duration_s).await;
            // Grace window so one more poll after terminal state still works.
            tokio::time::sleep(POST_TERMINAL_RETENTION).await;
            manager.inner.jobs.write().await.remove(&id_for_runner);
            debug!(job = %id_for_runner, "render job retention expired; reaped");
        });

        jobs.insert(id.clone(), JobHandle { rx, cancel });
        Ok(id)
    }

    /// Read the latest [`JobStatus`] for `id`. Non-blocking; reflects the
    /// most recent watch-channel value.
    pub async fn status(&self, id: &JobId) -> Result<JobStatus, JobError> {
        let jobs = self.inner.jobs.read().await;
        let h = jobs
            .get(id)
            .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
        Ok(h.rx.borrow().clone())
    }

    /// Cancel an in-flight job. The runner will set state to `Cancelled`
    /// and kill the child process. No-op (returns AlreadyTerminal) if
    /// already done/failed.
    pub async fn cancel(&self, id: &JobId) -> Result<(), JobError> {
        let jobs = self.inner.jobs.read().await;
        let h = jobs
            .get(id)
            .ok_or_else(|| JobError::UnknownJob(id.clone()))?;
        let state = h.rx.borrow().state;
        if matches!(
            state,
            JobState::Done | JobState::Failed | JobState::Cancelled
        ) {
            return Err(JobError::AlreadyTerminal(id.clone(), state));
        }
        h.cancel.cancel();
        Ok(())
    }

    /// Number of jobs the manager is currently tracking. Includes jobs in
    /// the post-terminal grace window.
    pub async fn job_count(&self) -> usize {
        self.inner.jobs.read().await.len()
    }
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}

fn configured_max_running_jobs() -> usize {
    std::env::var("AWIDAT_MAX_RENDER_JOBS")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_RUNNING_JOBS)
}

fn active_job_count(jobs: &HashMap<JobId, JobHandle>) -> usize {
    jobs.values()
        .filter(|handle| {
            matches!(
                handle.rx.borrow().state,
                JobState::Queued | JobState::Running
            )
        })
        .count()
}

async fn run_job(
    mut child: tokio::process::Child,
    tx: watch::Sender<JobStatus>,
    cancel: CancellationToken,
    total_duration_s: Option<f64>,
) {
    let stderr = match child.stderr.take() {
        Some(s) => s,
        None => {
            warn!("ffmpeg stderr pipe missing");
            tx.send_modify(|s| {
                s.state = JobState::Failed;
                s.log_excerpt = "stderr pipe missing".into();
            });
            return;
        }
    };

    let mut reader = BufReader::new(stderr).lines();
    let mut snapshot = ProgressSnapshot::default();
    let mut log_buf = TailBuffer::new(STDERR_LOG_CAP_BYTES);
    let started = Instant::now();

    let mut became_running = false;

    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let _ = child.kill().await;
                let _ = drain_remaining(&mut reader, &mut log_buf, Duration::from_secs(2)).await;
                tx.send_modify(|s| {
                    s.state = JobState::Cancelled;
                    s.log_excerpt = log_buf.snapshot();
                });
                return;
            }
            line = reader.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        if !became_running {
                            became_running = true;
                            tx.send_modify(|s| s.state = JobState::Running);
                        }
                        log_buf.push(&line);
                        log_buf.push("\n");
                        parse_progress_line(&line, &mut snapshot);
                        let pct = snapshot.percent(total_duration_s);
                        let eta = snapshot.eta(total_duration_s).map(|d| d.as_secs_f64());
                        tx.send_modify(|s| {
                            s.frames_done = snapshot.frames_done;
                            s.time_done_s = snapshot.time_done_s;
                            s.speed = snapshot.speed;
                            s.progress_pct = pct;
                            s.eta_s = eta;
                            s.log_excerpt = log_buf.snapshot();
                        });
                    }
                    Ok(None) => break,
                    Err(e) => {
                        log_buf.push(&format!("\n[awidat-render: stderr read error: {e}]\n"));
                        break;
                    }
                }
            }
            _ = tokio::time::sleep_until((started + DEFAULT_JOB_TIMEOUT).into()) => {
                let _ = child.kill().await;
                let _ = drain_remaining(&mut reader, &mut log_buf, Duration::from_secs(2)).await;
                log_buf.push(&format!(
                    "\n[awidat-render: timed out after {DEFAULT_JOB_TIMEOUT:?}]\n"
                ));
                tx.send_modify(|s| {
                    s.state = JobState::Failed;
                    s.log_excerpt = log_buf.snapshot();
                });
                return;
            }
        }
    }

    // EOF on stderr — wait for exit.
    let exit = match timeout(Duration::from_secs(5), child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => {
            log_buf.push(&format!("\n[awidat-render: wait error: {e}]\n"));
            tx.send_modify(|s| {
                s.state = JobState::Failed;
                s.log_excerpt = log_buf.snapshot();
            });
            return;
        }
        Err(_) => {
            let _ = child.kill().await;
            tx.send_modify(|s| {
                s.state = JobState::Failed;
                s.log_excerpt = log_buf.snapshot();
            });
            return;
        }
    };

    let final_state = if exit.success() {
        JobState::Done
    } else {
        JobState::Failed
    };
    tx.send_modify(|s| {
        s.state = final_state;
        s.exit_code = exit.code();
        s.log_excerpt = log_buf.snapshot();
        if matches!(final_state, JobState::Done) {
            s.progress_pct = Some(100.0);
            s.eta_s = Some(0.0);
        }
    });
}

async fn drain_remaining(
    reader: &mut tokio::io::Lines<BufReader<tokio::process::ChildStderr>>,
    buf: &mut TailBuffer,
    drain_window: Duration,
) -> Result<(), std::io::Error> {
    let _ = timeout(drain_window, async {
        while let Ok(Some(line)) = reader.next_line().await {
            buf.push(&line);
            buf.push("\n");
        }
    })
    .await;
    Ok(())
}

/// Tail-truncating byte buffer. Push appends; on overflow, oldest bytes
/// are evicted.
struct TailBuffer {
    bytes: Vec<u8>,
    cap: usize,
}

impl TailBuffer {
    fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(cap),
            cap,
        }
    }
    fn push(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let total = self.bytes.len() + bytes.len();
        if total <= self.cap {
            self.bytes.extend_from_slice(bytes);
            return;
        }
        let drop = total - self.cap;
        if drop >= self.bytes.len() {
            self.bytes.clear();
            let start = bytes.len().saturating_sub(self.cap);
            self.bytes.extend_from_slice(&bytes[start..]);
        } else {
            self.bytes.drain(..drop);
            self.bytes.extend_from_slice(bytes);
        }
    }
    fn snapshot(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_id_is_unique_across_calls() {
        let a = JobId::fresh();
        let b = JobId::fresh();
        assert_ne!(a, b);
        assert_eq!(a.0.len(), 16);
    }

    #[test]
    fn tail_buffer_keeps_only_last_n_bytes() {
        let mut buf = TailBuffer::new(8);
        buf.push("abcd");
        buf.push("efgh");
        buf.push("ij");
        assert_eq!(buf.snapshot(), "cdefghij");
    }

    #[test]
    fn tail_buffer_oversize_single_push() {
        let mut buf = TailBuffer::new(4);
        buf.push("abcdefghij");
        assert_eq!(buf.snapshot(), "ghij");
    }

    #[test]
    fn job_state_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&JobState::Done).unwrap(), "\"done\"");
        assert_eq!(
            serde_json::to_string(&JobState::Cancelled).unwrap(),
            "\"cancelled\""
        );
    }

    #[tokio::test]
    async fn unknown_job_id_errors() {
        let m = JobManager::new();
        let err = m.status(&JobId("nope".into())).await.unwrap_err();
        assert!(matches!(err, JobError::UnknownJob(_)));
    }

    #[tokio::test]
    async fn manager_rejects_start_when_active_job_cap_is_reached_before_spawn() {
        let m = JobManager::with_max_running(0);
        let spec = RenderJobSpec {
            args: vec!["-version".into()],
            total_duration_s: None,
            cwd: None,
            output_path: PathBuf::from("renders/out.mp4"),
            limitations: Vec::new(),
        };

        let err = m.start(spec).await.unwrap_err();

        assert!(matches!(
            err,
            JobError::AtCapacity {
                max_running: 0,
                active_jobs: 0
            }
        ));
        assert_eq!(m.job_count().await, 0);
    }

    #[tokio::test]
    async fn start_and_status_roundtrip_with_real_ffmpeg() {
        if ffmpeg_path().is_err() {
            return; // no ffmpeg, skip silently
        }
        let dir = tempfile::tempdir().unwrap();
        let out_path = dir.path().join("test.mp4");
        let m = JobManager::new();
        let spec = RenderJobSpec {
            args: vec![
                "-y".into(),
                "-f".into(),
                "lavfi".into(),
                "-i".into(),
                "color=c=red:s=160x120:d=1".into(),
                "-c:v".into(),
                "libx264".into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
                out_path.to_string_lossy().into_owned(),
            ],
            total_duration_s: Some(1.0),
            cwd: Some(dir.path().to_path_buf()),
            output_path: out_path.clone(),
            limitations: Vec::new(),
        };
        let id = m.start(spec).await.unwrap();
        // Poll up to 15s for terminal state.
        let mut final_state = JobState::Queued;
        for _ in 0..150 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let s = m.status(&id).await.unwrap();
            if matches!(
                s.state,
                JobState::Done | JobState::Failed | JobState::Cancelled
            ) {
                final_state = s.state;
                break;
            }
        }
        assert_eq!(
            final_state,
            JobState::Done,
            "job should complete; status={:?}",
            m.status(&id).await.unwrap()
        );
        assert!(out_path.exists(), "output file must exist");
    }
}
