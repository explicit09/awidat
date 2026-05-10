//! Awidat per-OS sandbox.
//!
//! Per `PLAN.md` §10 / §15. The shape:
//!
//!   - **macOS**: Apple's seatbelt via `/usr/bin/sandbox-exec` with
//!     a generated profile. Hardcoded path defends against PATH
//!     shadowing per Codex `MACOS_PATH_TO_SEATBELT_EXECUTABLE`.
//!   - **Linux**: bubblewrap + landlock self-re-exec. Stubbed in
//!     v1; the API surface compiles but `run_sandboxed` returns
//!     a "not yet wired" error so the orchestrator can fall back
//!     to the unsandboxed path with user approval.
//!   - **Windows**: out of scope.
//!
//! Single public type — [`Sandbox`] — with one method, [`run`].
//! Callers compose policies via [`Policy`]. The `try-sandboxed-
//! then-escalate` orchestrator pattern lives in `awidat-core`,
//! NOT here; this crate is a thin platform wrapper.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use thiserror::Error;

/// Hardcoded path to macOS sandbox-exec. Used so a malicious PATH
/// can't shadow the real binary.
#[cfg(target_os = "macos")]
const MACOS_SEATBELT: &str = "/usr/bin/sandbox-exec";

/// What's allowed in the sandbox.
///
/// We deny by default and explicitly grant the minimum needed for
/// the call. Per PLAN.md §10.1, the canonical policy for awidat is:
///   - Read-only access to source assets (`raw/`)
///   - Read-write access to project subdirs (`renders/`, `index/`,
///     `.awidat/`, the OTIO timeline file)
///   - Network: deny by default; explicit opt-in for API calls
///   - All other paths: deny
#[derive(Debug, Clone)]
pub struct Policy {
    /// Project root the agent is editing. The sandbox allows
    /// read-write inside this directory tree.
    pub project_root: PathBuf,
    /// Additional read-only paths. Source assets that live outside
    /// the project tree (e.g. a symlinked raw/ folder pointing at a
    /// shared library) belong here.
    pub read_only_paths: Vec<PathBuf>,
    /// Whether to allow network egress. Most edit operations don't
    /// need it; the renderer doesn't need it; only the LLM API
    /// client + indexer model downloads do, and those run outside
    /// the sandbox.
    pub allow_network: bool,
}

impl Policy {
    /// Strict default: project-root writable, no extra reads, no
    /// network. Render + bash + indexer-shellout calls use this.
    pub fn for_project(project_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
            read_only_paths: Vec::new(),
            allow_network: false,
        }
    }

    /// Add a read-only path. Useful when the project's `raw/`
    /// folder symlinks out to a shared media library.
    pub fn with_read_only(mut self, path: impl Into<PathBuf>) -> Self {
        self.read_only_paths.push(path.into());
        self
    }

    /// Allow network egress. Use sparingly.
    pub fn with_network(mut self) -> Self {
        self.allow_network = true;
        self
    }
}

/// Errors a sandboxed run can produce. The orchestrator distinguishes
/// `Denied` (try-sandboxed-then-escalate's escalation trigger) from
/// other failures (genuine errors that even unsandboxed wouldn't fix).
#[derive(Debug, Error)]
pub enum SandboxErr {
    /// The sandbox blocked the call. The orchestrator should ask the
    /// user whether to escalate (re-run unsandboxed) or abort.
    #[error("sandbox denied the call: {reason}")]
    Denied {
        /// Human-readable reason (parsed from the sandbox's stderr
        /// when possible). On macOS this is typically a syscall name
        /// or a path that the policy didn't permit.
        reason: String,
    },
    /// The sandbox itself failed to launch (e.g. seatbelt binary
    /// missing, or the generated policy was malformed). Distinct from
    /// `Denied` because escalation won't help.
    #[error("sandbox launch failed: {0}")]
    LaunchFailed(String),
    /// Linux v1: not yet implemented.
    #[error(
        "Linux sandbox not yet wired. Falling back to unsandboxed execution \
         is the orchestrator's responsibility — see awidat-core's \
         run-sandboxed-or-escalate flow."
    )]
    LinuxNotImplemented,
    /// I/O while spawning or waiting on the child.
    #[error("sandbox I/O: {0}")]
    Io(#[from] std::io::Error),
}

/// Output of a sandboxed run. The actual command's exit status,
/// stdout, stderr, captured to caller.
#[derive(Debug, Clone)]
pub struct SandboxedOutput {
    /// Process exit status of the sandboxed command (NOT the
    /// sandbox wrapper itself — caller cares about the wrapped
    /// command's exit, not seatbelt's).
    pub status: ExitStatus,
    /// Captured stdout.
    pub stdout: Vec<u8>,
    /// Captured stderr.
    pub stderr: Vec<u8>,
}

/// Sandbox runner. Stateless — caller constructs one per call.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sandbox;

impl Sandbox {
    /// Run `argv` under a sandbox honoring `policy`. Returns
    /// `SandboxedOutput` on completion (success OR non-zero exit
    /// of the wrapped command). Returns `SandboxErr` only when
    /// the sandbox itself denied or failed.
    ///
    /// On macOS: invokes `sandbox-exec -p <policy> argv...`.
    /// On Linux: returns `SandboxErr::LinuxNotImplemented` so the
    /// orchestrator can decide whether to fall back unsandboxed.
    pub fn run(&self, argv: &[&str], policy: &Policy) -> Result<SandboxedOutput, SandboxErr> {
        if argv.is_empty() {
            return Err(SandboxErr::LaunchFailed(
                "sandbox: argv must be non-empty".into(),
            ));
        }
        #[cfg(target_os = "macos")]
        {
            (*self).run_macos(argv, policy)
        }
        #[cfg(target_os = "linux")]
        {
            let _ = (argv, policy);
            Err(SandboxErr::LinuxNotImplemented)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = (argv, policy);
            Err(SandboxErr::LaunchFailed(
                "this OS is not supported by awidat-sandboxing".into(),
            ))
        }
    }

    #[cfg(target_os = "macos")]
    fn run_macos(self, argv: &[&str], policy: &Policy) -> Result<SandboxedOutput, SandboxErr> {
        let profile = render_seatbelt_profile(policy);
        let mut cmd = Command::new(MACOS_SEATBELT);
        cmd.arg("-p").arg(&profile);
        cmd.args(argv);
        let output = cmd.output()?;
        // sandbox-exec exits 65 + writes "Sandbox ... was denied" to
        // stderr when a policy violation occurs. We surface that as
        // SandboxErr::Denied; everything else is a normal exit
        // (which the caller decides what to do with).
        if let Some(reason) = detect_macos_denial(&output.stderr, output.status.code()) {
            return Err(SandboxErr::Denied { reason });
        }
        Ok(SandboxedOutput {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

/// Render the seatbelt SBPL profile for `policy`.
///
/// Format reference: Apple's `(version 1)` SBPL dialect. Codex's
/// profile generator is the reference; ours is narrower because
/// awidat's surface is smaller (no compiler invocation, no git
/// rebases). The base policy denies everything; we explicitly
/// allow:
///   - Process spawn (so the wrapped command can fork helpers
///     like ffmpeg's child encoders)
///   - Read everywhere (overly permissive but tools need to read
///     /etc/, system frameworks, etc.; restricting reads on macOS
///     causes more bugs than it prevents and the threat model
///     here isn't "agent reads /etc/passwd")
///   - Write only inside `policy.project_root` and `/tmp/`
///   - Network: forbidden unless `policy.allow_network`
#[cfg(target_os = "macos")]
fn render_seatbelt_profile(policy: &Policy) -> String {
    let project_root = policy
        .project_root
        .canonicalize()
        .unwrap_or_else(|_| policy.project_root.clone());
    let mut s = String::from("(version 1)\n");
    s.push_str("(deny default)\n");
    // Process / signal — without these, even sandbox-exec's child
    // launching breaks.
    s.push_str("(allow process-fork)\n");
    s.push_str("(allow process-exec)\n");
    s.push_str("(allow signal (target self))\n");
    s.push_str("(allow signal (target children))\n");
    // Reads: liberal. Restricting reads on macOS causes more bugs
    // than it prevents (see comment above).
    s.push_str("(allow file-read*)\n");
    s.push_str("(allow sysctl-read)\n");
    s.push_str("(allow mach-lookup)\n");
    s.push_str("(allow iokit-open)\n");
    // Writes: only inside the project + /tmp + the macOS-standard
    // tempdir tree. Everything else is denied.
    s.push_str(&format!(
        "(allow file-write* (subpath \"{}\"))\n",
        project_root.display()
    ));
    s.push_str("(allow file-write* (subpath \"/tmp\"))\n");
    s.push_str("(allow file-write* (subpath \"/private/tmp\"))\n");
    s.push_str("(allow file-write* (subpath \"/private/var/folders\"))\n");
    if policy.allow_network {
        s.push_str("(allow network*)\n");
    } else {
        // Loopback only; Anthropic API calls go through the parent
        // process, not the sandboxed child.
        s.push_str("(allow network-bind (local ip))\n");
        s.push_str("(allow network-inbound (local ip))\n");
    }
    s
}

/// Parse macOS sandbox-exec stderr to recognize a denial vs a normal
/// error. Returns `Some(reason)` on denial, `None` otherwise.
#[cfg(target_os = "macos")]
fn detect_macos_denial(stderr: &[u8], exit_code: Option<i32>) -> Option<String> {
    let s = String::from_utf8_lossy(stderr);
    // sandbox-exec uses several phrasings depending on macOS version:
    //   "Sandbox: <cmd>(<pid>) deny ..."
    //   "deny(1) <op>"
    //   "Operation not permitted" (when the sandbox propagates EPERM)
    // We match the first canonical phrasing, fall back to exit-65
    // detection (the documented sandbox-policy-violation code).
    if s.contains("deny ") || s.contains("Operation not permitted") {
        // Extract the first line containing the denial reason for
        // a tighter user-facing message.
        let reason = s
            .lines()
            .find(|l| l.contains("deny ") || l.contains("Operation not permitted"))
            .unwrap_or("policy violation")
            .trim()
            .to_string();
        return Some(reason);
    }
    if exit_code == Some(65) {
        return Some("policy violation (sandbox-exec exit 65)".to_string());
    }
    None
}

/// Returns the sandbox backend that would be selected on the current
/// platform. Useful for diagnostics + the `awidat doctor` command.
pub fn current_backend() -> &'static str {
    if cfg!(target_os = "macos") {
        "seatbelt"
    } else if cfg!(target_os = "linux") {
        "landlock+seccomp (stub)"
    } else {
        "unsupported"
    }
}

/// Check whether the sandbox backend is actually usable on this host.
/// Different from `current_backend()` — this verifies the backing
/// binary / kernel feature is present.
pub fn is_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        Path::new(MACOS_SEATBELT).is_file()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_returns_known_string() {
        let backend = current_backend();
        assert!(matches!(
            backend,
            "seatbelt" | "landlock+seccomp (stub)" | "unsupported"
        ));
    }

    #[test]
    fn policy_builder_round_trips() {
        let p = Policy::for_project("/proj")
            .with_read_only("/shared/library")
            .with_network();
        assert_eq!(p.project_root, PathBuf::from("/proj"));
        assert_eq!(p.read_only_paths, vec![PathBuf::from("/shared/library")]);
        assert!(p.allow_network);
    }

    #[test]
    fn empty_argv_is_a_launch_error() {
        let sb = Sandbox;
        let policy = Policy::for_project("/tmp");
        let err = sb.run(&[], &policy).unwrap_err();
        assert!(matches!(err, SandboxErr::LaunchFailed(_)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_profile_includes_project_subpath() {
        let policy = Policy::for_project("/tmp/awidat-test");
        let profile = render_seatbelt_profile(&policy);
        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("/tmp/awidat-test"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_runs_a_real_command_in_sandbox() {
        // Smoke: `echo hello` should work fine inside the sandbox
        // since echo is a process-exec call against /bin/echo and
        // produces stdout — both allowed by our profile.
        let sb = Sandbox;
        let dir = tempfile::tempdir().unwrap();
        let policy = Policy::for_project(dir.path());
        let out = sb
            .run(&["/bin/echo", "hello"], &policy)
            .expect("echo should run cleanly under sandbox");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_denies_writes_outside_project_root() {
        // Try to write a file at /etc/awidat-sandbox-test (outside
        // the policy's writable subtree). Should fail. We use
        // `/bin/sh -c "echo ... > /etc/..."` so the redirection
        // happens inside the sandboxed process, not the parent
        // shell.
        let sb = Sandbox;
        let dir = tempfile::tempdir().unwrap();
        let policy = Policy::for_project(dir.path());
        let result = sb.run(
            &[
                "/bin/sh",
                "-c",
                "echo blocked > /etc/awidat-sandbox-test 2>&1",
            ],
            &policy,
        );
        // Two acceptable outcomes:
        //   - SandboxErr::Denied (we recognized the denial in stderr)
        //   - SandboxedOutput with non-zero status + no file written
        // Both prove the sandbox blocked the write.
        match result {
            Ok(out) => {
                assert!(
                    !out.status.success(),
                    "expected non-zero exit, got {:?}",
                    out.status
                );
                assert!(
                    !std::path::Path::new("/etc/awidat-sandbox-test").exists(),
                    "sandbox didn't actually block the write"
                );
            }
            Err(SandboxErr::Denied { reason }) => {
                assert!(!reason.is_empty());
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_returns_not_implemented() {
        let sb = Sandbox;
        let policy = Policy::for_project("/tmp");
        let err = sb.run(&["/bin/echo", "x"], &policy).unwrap_err();
        assert!(matches!(err, SandboxErr::LinuxNotImplemented));
    }
}
