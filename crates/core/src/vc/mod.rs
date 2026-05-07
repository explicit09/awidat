//! Version control wrapper around `vedit-core`.
//!
//! Phase A of the vedit integration (see VEDIT_INTEGRATION.md). The rest
//! of awidat reaches version-control through this module — never
//! through `vedit_core::*` directly. That isolation matters because
//! vedit is under active development and we want the surface awidat
//! depends on to be stable, even if vedit's API churns underneath.
//!
//! ## Design rules baked in from day one
//!
//! These mirror the discipline rules for "Phase B mindset in Phase A
//! scope" — start small but lock in the shape Phase B wants.
//!
//! 1. **Wrapper-only access.** No `pub use vedit_core::*`. We import
//!    types into the wrapper namespace and expose what we need. The
//!    rest of awidat sees `awidat_core::vc::Diff`, not
//!    `vedit_core::diff::Vec<Change>`.
//!
//! 2. **Phase-B commit message format from day one.** Even when the
//!    user is hand-triggering a commit via `vedit_commit` in Phase A,
//!    [`format_commit_message`] produces the exact format the apply
//!    pipeline will auto-generate later: machine-parseable header line
//!    + agent-reasoning body. No migration of old commits when Phase B
//!    lifts commit into the apply layer.
//!
//! 3. **Pending-vs-committed naming.** [`PendingDiff`] (turn-local,
//!    in-memory, what the ghost overlay shows) and [`CommittedDiff`]
//!    (the structured diff between two vedit commits) are named so
//!    they coexist intelligibly. Phase B's deletion of the in-memory
//!    turn tracker is mechanical, not a rename.
//!
//! ## What lives here
//!
//! - [`Repo`] — opaque wrapper around `vedit_core::repo::Repo`.
//! - [`open_or_init`] — idempotent constructor. Opens the existing
//!   `.vedit/` if present, initializes a fresh repo otherwise.
//! - [`commit`] — write a timeline + commit it with an awidat-shaped
//!   message. The agent reasoning string is mandatory; this is the
//!   load-bearing audit trail.
//! - [`diff_refs`] — structured diff between two refs (default
//!   `session_start..HEAD`).
//! - [`log`] — last N commits, newest-first.
//! - [`ensure_session_tag`] — stamp a `session-start` branch on the
//!   current HEAD so `diff_refs` defaults work without args.
//!
//! ## What does NOT live here
//!
//! - The agent loop. `vc::commit` is called by tools (Phase A) or by
//!   the apply pipeline (Phase B); the wrapper is stateless.
//! - The TUI / desktop UI. Those subscribe to the same events anyone
//!   else does.
//! - Branch / merge surface. Phase B work; the wrapper grows when the
//!   tool surface does.

use std::path::{Path, PathBuf};

use vedit_core::commit::{Author, Commit};
use vedit_core::diff;
use vedit_core::otio;
use vedit_core::repo::Repo as VeditRepo;

/// Branch name vedit uses by default. Mirrors `vedit_core::repo::DEFAULT_BRANCH`.
/// Re-exported here so callers don't have to reach into vedit-core for it.
pub const DEFAULT_BRANCH: &str = "main";

/// Branch name awidat stamps at session start so `diff_refs` defaults
/// work. The branch points at whatever HEAD was when the session
/// opened; comparing `session-start..HEAD` shows the agent's session
/// changes as one structured diff.
pub const SESSION_START_BRANCH: &str = "session-start";

/// Errors from the version-control wrapper. Wraps vedit-core's
/// `anyhow::Error` for now; we'll narrow when the failure modes
/// stabilize.
#[derive(Debug, thiserror::Error)]
pub enum VcError {
    /// Underlying vedit-core operation failed.
    #[error("version control: {0}")]
    Vedit(String),
    /// Project's `project.otio.json` is missing or unreadable.
    #[error("project timeline file: {0}")]
    Project(String),
    /// Resolving a ref failed (typo, doesn't exist).
    #[error("unknown ref: {0}")]
    UnknownRef(String),
}

/// Convert any vedit-core error (which uses `anyhow::Error` upstream)
/// into our wrapper error. Done as a free function rather than a
/// `From` impl so awidat-core doesn't need to depend on `anyhow` —
/// the wrapper's whole job is hiding vedit's choices, including its
/// error library.
fn vedit_err<E: std::fmt::Display>(e: E) -> VcError {
    VcError::Vedit(format!("{e:#}"))
}

/// Opaque handle on a vedit repository rooted at a project directory.
/// Cheap to clone (the inner `VeditRepo` is small).
#[derive(Debug, Clone)]
pub struct Repo {
    inner: VeditRepo,
    /// Path to the project's `project.otio.json` working copy. Cached so
    /// `commit_current_timeline` doesn't have to be passed it on every
    /// call.
    project_otio: PathBuf,
}

impl Repo {
    /// Project root the repo is rooted at (`<root>/.vedit/`).
    pub fn workdir(&self) -> &Path {
        // VeditRepo.root is `<workdir>/.vedit`; we want the parent.
        self.inner
            .root
            .parent()
            .unwrap_or_else(|| Path::new("/"))
    }

    /// Path to `project.otio.json`.
    pub fn project_otio_path(&self) -> &Path {
        &self.project_otio
    }
}

/// Open an existing vedit repo at `<project_root>/.vedit/`, or initialize
/// a fresh one if none exists. Idempotent: calling this on an
/// already-initialized project just returns the existing repo.
///
/// Returns an error only if the filesystem is misbehaving (missing
/// project root, permissions). Project-shape errors are deferred until
/// callers actually try to commit / diff / log.
pub fn open_or_init(project_root: &Path) -> Result<Repo, VcError> {
    let vedit_dir = project_root.join(".vedit");
    let inner = if vedit_dir.exists() {
        VeditRepo::discover(project_root).map_err(vedit_err)?
    } else {
        VeditRepo::init(project_root).map_err(vedit_err)?
    };
    Ok(Repo {
        inner,
        project_otio: project_root.join("project.otio.json"),
    })
}

/// Stamp a `session-start` branch (or equivalently-named ref) on the
/// current HEAD. Idempotent: if the branch already exists, it's
/// re-pointed at HEAD; if HEAD has no commits yet (fresh repo), this
/// is a no-op (the next commit will land on `main`, which is its own
/// session start).
///
/// Call once at session open. Phase A's `vedit_diff` defaults to
/// `from = session-start, to = HEAD`, and stamping at session open is
/// what makes that default meaningful.
pub fn ensure_session_tag(repo: &Repo) -> Result<(), VcError> {
    // If HEAD has no commits yet, there's nothing to tag. The first
    // commit's branch (main) IS the session start.
    let head_target = match repo.inner.branch_target(DEFAULT_BRANCH).map_err(vedit_err)? {
        Some(h) => h,
        None => return Ok(()),
    };
    // If the branch exists, re-point it; otherwise create it.
    let branch_path = repo
        .inner
        .root
        .join("refs")
        .join("heads")
        .join(SESSION_START_BRANCH);
    if let Some(parent) = branch_path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| VcError::Vedit(format!("create refs/heads/: {e}")))?;
    }
    std::fs::write(&branch_path, format!("{head_target}\n"))
        .map_err(|e| VcError::Vedit(format!("writing {SESSION_START_BRANCH} ref: {e}")))?;
    Ok(())
}

/// Commit the current `project.otio.json` with an awidat-shaped
/// message.
///
/// Phase A: callers are tools (`vedit_commit`). Phase B: callers will
/// also be the apply pipeline, which will pass `agent_reasoning`
/// derived from the turn's reasoning. Either way, the message format
/// is the same — see [`format_commit_message`].
///
/// Returns the new commit hash. Idempotent on identical content:
/// re-committing the same timeline produces the same timeline-hash
/// (vedit content-addresses), but does write a new commit object
/// (different timestamp). Use `is_workdir_dirty` if you want to skip
/// no-op commits.
pub fn commit_current_timeline(
    repo: &Repo,
    header: &str,
    agent_reasoning: Option<&str>,
) -> Result<CommitOutcome, VcError> {
    let bytes = std::fs::read(&repo.project_otio).map_err(|e| {
        VcError::Project(format!(
            "reading {}: {e}",
            repo.project_otio.display()
        ))
    })?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        VcError::Project(format!(
            "parsing {}: {e}",
            repo.project_otio.display()
        ))
    })?;

    let timeline_hash = repo.inner.write_timeline(&value).map_err(vedit_err)?;
    let message = format_commit_message(header, agent_reasoning);
    let author = awidat_author();
    let commit_hash = repo
        .inner
        .commit(&timeline_hash, author, &message)
        .map_err(vedit_err)?;
    Ok(CommitOutcome {
        commit_hash,
        timeline_hash,
        message,
    })
}

/// Result of a successful commit.
#[derive(Debug, Clone)]
pub struct CommitOutcome {
    /// `sha256:...` of the new commit object.
    pub commit_hash: String,
    /// `sha256:...` of the timeline this commit points at. Two commits
    /// with identical content share a timeline hash even though their
    /// commit hashes differ (timestamps).
    pub timeline_hash: String,
    /// Final commit message written to the object (header + body).
    pub message: String,
}

/// Compose a commit message in the format the apply pipeline will
/// auto-generate in Phase B. Header line first, blank line, then
/// agent reasoning (if provided) prefixed by `Agent reasoning:`.
///
/// The header is one short imperative line, no trailing period —
/// same convention as good git commit messages. The body is free
/// text but should reference the editorial decisions made.
///
/// Format example:
///
/// ```text
/// Trim "drone_shot_04" -1.8s; insert b-roll cover at "skyline reference"
///
/// Agent reasoning: User asked for tighter pacing. Trimmed the drone hold
/// per rhythm-preservation rule. Bundled a 3.0s Pexels skyline cover at
/// 12.4s because the speaker referenced "imagine a city skyline" — the
/// cutaway hides the otherwise-dirty mid-motion trim point.
/// ```
pub fn format_commit_message(header: &str, agent_reasoning: Option<&str>) -> String {
    let trimmed_header = header.trim();
    match agent_reasoning {
        Some(body) if !body.trim().is_empty() => {
            format!("{trimmed_header}\n\nAgent reasoning: {}", body.trim())
        }
        _ => trimmed_header.to_string(),
    }
}

/// Phase B auto-commit: snapshot the project after a successful
/// apply_edl envelope, generating an awidat-shaped commit message
/// from the structured op descriptions + the agent's reasoning text.
///
/// This is the entry point both write paths (agent-side and
/// desktop-side) call after their disk write succeeds. Best-effort:
/// the caller logs failures but doesn't fail the apply — vedit going
/// down should never block editing.
///
/// `op_descriptions`: list of `AppliedOp.description` strings, in
///    source order. Used to build the canonical header.
/// `agent_reasoning`: optional turn-level reasoning that explains
///    *why* this change was made. When `None`, only the auto-header
///    is committed (still better than nothing).
pub fn auto_commit_apply(
    repo: &Repo,
    op_descriptions: &[String],
    agent_reasoning: Option<&str>,
) -> Result<CommitOutcome, VcError> {
    let header = compose_auto_header(op_descriptions);
    commit_current_timeline(repo, &header, agent_reasoning)
}

/// Build a one-line header from a list of op descriptions.
/// - 1 op  → use the description verbatim (capitalized).
/// - 2 ops → "X; Y".
/// - 3+ ops → "X; Y; …and N more" (cap header at ~120 chars).
///
/// The descriptions are imperative-shaped already (e.g. "Trim
/// drone_shot_04 by 1.8s", "Insert BRoll over 'skyline reference'"),
/// produced by the apply layer. We just concatenate.
pub fn compose_auto_header(op_descriptions: &[String]) -> String {
    const HEADER_CAP: usize = 120;
    if op_descriptions.is_empty() {
        // Still produces a meaningful commit so the user can see
        // *something* happened — the timeline content hash will
        // differ even if no apply ops "landed" (a no-op envelope
        // shouldn't reach here, but defensive).
        return "apply_edl envelope (no described ops)".to_string();
    }
    let trimmed: Vec<&str> = op_descriptions.iter().map(|s| s.trim()).collect();
    let header = match trimmed.len() {
        1 => trimmed[0].to_string(),
        2 => format!("{}; {}", trimmed[0], trimmed[1]),
        n => {
            let first_two = format!("{}; {}", trimmed[0], trimmed[1]);
            let extra = n - 2;
            format!("{first_two}; …and {extra} more")
        }
    };
    truncate_chars(&header, HEADER_CAP)
}

/// Truncate to at most `max` characters, on a char boundary, with an
/// ellipsis suffix when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Author stamped on every commit awidat writes. Generic on purpose
/// — we don't want vedit telling users this is "their" commit when
/// it's actually the agent acting on their behalf. The user-attribution
/// question (one author per session? one per turn?) is a Phase B
/// question.
fn awidat_author() -> Author {
    Author {
        name: "awidat agent".to_string(),
        email: "agent@awidat.local".to_string(),
    }
}

/// Diff between two refs. Default: `session-start..HEAD`. The agent
/// uses this to answer "what did you change in this session?".
pub fn diff_refs(
    repo: &Repo,
    from_ref: Option<&str>,
    to_ref: Option<&str>,
) -> Result<CommittedDiff, VcError> {
    let from_resolved = resolve_default(repo, from_ref, SESSION_START_BRANCH)?;
    let to_resolved = resolve_default(repo, to_ref, "HEAD")?;

    // If the from ref doesn't exist (no session tag yet, fresh repo),
    // diff the "empty timeline" against the to ref so the result is
    // "everything that's there is new."
    let before_timeline = match from_resolved {
        Some(hash) => Some(read_timeline_at_commit(repo, &hash)?),
        None => None,
    };
    let after_timeline = match to_resolved {
        Some(hash) => Some(read_timeline_at_commit(repo, &hash)?),
        None => None,
    };

    let changes = match (before_timeline.as_ref(), after_timeline.as_ref()) {
        (Some(b), Some(a)) => diff::diff(b, a),
        // No commits at all yet — empty diff is the right answer.
        (None, None) => Vec::new(),
        // No before but there's an after: report everything in
        // `after` as added. vedit's diff() handles this if we pass an
        // empty timeline.
        (None, Some(a)) => diff::diff(&empty_timeline(), a),
        // Symmetric: no after but there's a before — everything was
        // removed. Unlikely in practice but defensive.
        (Some(b), None) => diff::diff(b, &empty_timeline()),
    };

    Ok(CommittedDiff {
        from_ref: from_ref.unwrap_or(SESSION_START_BRANCH).to_string(),
        to_ref: to_ref.unwrap_or("HEAD").to_string(),
        changes,
    })
}

/// Resolve a ref string to a commit hash, with fallback handling for
/// "default refs that may not exist yet" (e.g. session-start in a
/// fresh repo).
fn resolve_default(
    repo: &Repo,
    explicit: Option<&str>,
    default: &str,
) -> Result<Option<String>, VcError> {
    match explicit {
        Some(r) => match repo.inner.resolve(r) {
            Ok(h) => Ok(Some(h)),
            Err(_) => Err(VcError::UnknownRef(r.to_string())),
        },
        None => match repo.inner.resolve(default) {
            Ok(h) => Ok(Some(h)),
            // Fresh repo, no commits: defaults legitimately don't
            // resolve. Return None and let the caller treat it as
            // "compare against the empty timeline."
            Err(_) => Ok(None),
        },
    }
}

fn read_timeline_at_commit(repo: &Repo, commit_hash: &str) -> Result<vedit_core::model::Timeline, VcError> {
    let commit: Commit = repo.inner.read_commit(commit_hash).map_err(vedit_err)?;
    let timeline_value = repo.inner.read_timeline(&commit.timeline).map_err(vedit_err)?;
    let timeline = otio::parse_timeline(&timeline_value).map_err(vedit_err)?;
    Ok(timeline)
}

fn empty_timeline() -> vedit_core::model::Timeline {
    vedit_core::model::Timeline {
        name: String::new(),
        tracks: Vec::new(),
    }
}

/// Structured diff between two refs. Phase B's `diff_view` reads this.
#[derive(Debug, Clone)]
pub struct CommittedDiff {
    /// Ref string the diff is from.
    pub from_ref: String,
    /// Ref string the diff is to.
    pub to_ref: String,
    /// Structured changes — see [`vedit_core::diff::Change`].
    pub changes: Vec<diff::Change>,
}

impl CommittedDiff {
    /// Number of structural changes in the diff.
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// True iff the structural diff is empty. Note: a metadata-only
    /// commit (e.g. agent reasoning updated, no clip changed) has an
    /// empty structural diff but a non-zero hash difference. Phase B
    /// commit messages will surface this as "metadata only."
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Last N commits, newest-first. Returns lightweight entries (hash,
/// short header, timestamp) so the agent can list history without
/// pulling every commit's full body into context.
pub fn log(repo: &Repo, limit: usize) -> Result<Vec<LogEntry>, VcError> {
    let entries = repo.inner.log(None).map_err(vedit_err)?;
    let trimmed = entries
        .into_iter()
        .take(limit)
        .map(|(hash, commit)| LogEntry {
            commit_hash: hash,
            timestamp: commit.timestamp,
            header: header_line(&commit.message),
            full_message: commit.message,
            timeline_hash: commit.timeline,
            parents: commit.parents,
        })
        .collect();
    Ok(trimmed)
}

/// One commit as the agent / UI sees it.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// `sha256:...` of the commit object.
    pub commit_hash: String,
    /// ISO-8601 timestamp.
    pub timestamp: String,
    /// First line of the message — what the UI renders by default.
    pub header: String,
    /// Full message body for "show this commit" deep dives.
    pub full_message: String,
    /// `sha256:...` of the timeline this commit points at.
    pub timeline_hash: String,
    /// Parent commit hashes (1 = normal, 0 = initial, 2+ = merge).
    pub parents: Vec<String>,
}

fn header_line(message: &str) -> String {
    message.lines().next().unwrap_or("").trim().to_string()
}

/// Pending diff — what's NOT yet committed. Distinct concept from
/// [`CommittedDiff`]. The existing per-turn ghost overlay sees a
/// pending diff (turn-local, in-memory, not on disk yet); accepting
/// the proposal moves it into the committed world via [`commit_current_timeline`].
///
/// Phase A doesn't compute this — the existing per-turn diff tracker
/// still owns it. This type is here so Phase B's deletion of the
/// in-memory tracker is mechanical: rename the existing tracker's
/// output to `PendingDiff` and the call sites already match.
#[derive(Debug, Clone, Default)]
pub struct PendingDiff {
    /// Structural changes the user has not yet accepted.
    pub changes: Vec<diff::Change>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_minimal_otio(path: &Path, name: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let v = serde_json::json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": name,
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "name": "tracks",
                "children": []
            }
        });
        std::fs::write(path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }

    fn write_otio_with_clip(path: &Path, clip_name: &str, source_url: &str) {
        let v = serde_json::json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": "test",
            "metadata": {
                "awidat": {
                    "version": "0.1",
                    "anchors": {
                        clip_name: {
                            "transcript_snippet": "hello world"
                        }
                    }
                }
            },
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "name": "tracks",
                "children": [{
                    "OTIO_SCHEMA": "Track.1",
                    "name": "V1",
                    "kind": "Video",
                    "children": [{
                        "OTIO_SCHEMA": "Clip.2",
                        "name": clip_name,
                        "source_range": {
                            "OTIO_SCHEMA": "TimeRange.1",
                            "start_time": {
                                "OTIO_SCHEMA": "RationalTime.1",
                                "value": 0.0,
                                "rate": 24.0
                            },
                            "duration": {
                                "OTIO_SCHEMA": "RationalTime.1",
                                "value": 240.0,
                                "rate": 24.0
                            }
                        },
                        "media_reference": {
                            "OTIO_SCHEMA": "ExternalReference.1",
                            "target_url": source_url
                        }
                    }]
                }]
            }
        });
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }

    #[test]
    fn open_or_init_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(&dir.path().join("project.otio.json"), "test");

        let r1 = open_or_init(dir.path()).unwrap();
        let r2 = open_or_init(dir.path()).unwrap();
        assert_eq!(r1.workdir(), r2.workdir());
        assert!(dir.path().join(".vedit").is_dir());
    }

    #[test]
    fn commit_creates_a_commit_object() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(&dir.path().join("project.otio.json"), "v1");

        let repo = open_or_init(dir.path()).unwrap();
        let outcome = commit_current_timeline(
            &repo,
            "Initial commit",
            Some("Brand-new project."),
        )
        .unwrap();
        assert!(outcome.commit_hash.starts_with("sha256:"));
        assert!(outcome.timeline_hash.starts_with("sha256:"));
        assert!(outcome.message.contains("Initial commit"));
        assert!(outcome.message.contains("Agent reasoning: Brand-new project."));
    }

    #[test]
    fn format_commit_message_no_body_is_just_header() {
        let m = format_commit_message("Trim drone_shot -1.8s", None);
        assert_eq!(m, "Trim drone_shot -1.8s");

        let m_empty = format_commit_message("Trim drone_shot -1.8s", Some(""));
        assert_eq!(m_empty, "Trim drone_shot -1.8s");

        let m_ws = format_commit_message("Trim drone_shot -1.8s", Some("   \n  "));
        assert_eq!(m_ws, "Trim drone_shot -1.8s");
    }

    #[test]
    fn format_commit_message_with_body_uses_canonical_shape() {
        let m = format_commit_message(
            "Trim X by 1.8s",
            Some("User asked for tighter pacing."),
        );
        // Header on line 1, blank line, body line.
        let lines: Vec<&str> = m.lines().collect();
        assert_eq!(lines[0], "Trim X by 1.8s");
        assert_eq!(lines[1], "");
        assert!(lines[2].starts_with("Agent reasoning: "));
        assert!(lines[2].contains("tighter pacing"));
    }

    #[test]
    fn ensure_session_tag_is_no_op_in_fresh_repo() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(&dir.path().join("project.otio.json"), "v1");

        let repo = open_or_init(dir.path()).unwrap();
        // No commits yet — should silently do nothing.
        ensure_session_tag(&repo).unwrap();
        let session_ref =
            dir.path().join(".vedit").join("refs").join("heads").join(SESSION_START_BRANCH);
        assert!(!session_ref.exists());
    }

    #[test]
    fn ensure_session_tag_points_at_head_after_first_commit() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(&dir.path().join("project.otio.json"), "v1");
        let repo = open_or_init(dir.path()).unwrap();
        let outcome = commit_current_timeline(&repo, "Initial", None).unwrap();
        ensure_session_tag(&repo).unwrap();

        let session_ref =
            dir.path().join(".vedit").join("refs").join("heads").join(SESSION_START_BRANCH);
        let contents = std::fs::read_to_string(&session_ref).unwrap();
        assert!(contents.trim() == outcome.commit_hash, "{contents:?}");
    }

    #[test]
    fn diff_refs_reports_added_clip() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_minimal_otio(&otio, "v1");
        let repo = open_or_init(dir.path()).unwrap();
        commit_current_timeline(&repo, "Initial", None).unwrap();
        ensure_session_tag(&repo).unwrap();

        // Replace the OTIO with one containing a clip, commit again.
        write_otio_with_clip(&otio, "shot-a", "raw/foo.mp4");
        commit_current_timeline(&repo, "Add shot-a", Some("First take.")).unwrap();

        let diff = diff_refs(&repo, None, None).unwrap();
        assert_eq!(diff.from_ref, SESSION_START_BRANCH);
        assert_eq!(diff.to_ref, "HEAD");
        assert!(!diff.is_empty(), "{diff:#?}");
        // The session-start commit had an empty `tracks` list; the
        // second commit adds a V1 track containing one clip. vedit's
        // diff reports both changes — at minimum the new track and
        // ideally the new clip too. We assert the track add (the
        // certainty); the clip add is implied.
        assert!(
            diff.changes.iter().any(|c| matches!(c, diff::Change::TrackAdded { .. })),
            "expected at least one TrackAdded, got: {:?}",
            diff.changes
        );
    }

    #[test]
    fn diff_refs_handles_unknown_ref_with_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(&dir.path().join("project.otio.json"), "v1");
        let repo = open_or_init(dir.path()).unwrap();
        commit_current_timeline(&repo, "Initial", None).unwrap();

        let err = diff_refs(&repo, Some("does-not-exist"), None).unwrap_err();
        match err {
            VcError::UnknownRef(r) => assert_eq!(r, "does-not-exist"),
            other => panic!("expected UnknownRef, got {other:?}"),
        }
    }

    #[test]
    fn log_returns_commits_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_minimal_otio(&otio, "v1");
        let repo = open_or_init(dir.path()).unwrap();
        commit_current_timeline(&repo, "First", None).unwrap();
        write_otio_with_clip(&otio, "shot-a", "raw/foo.mp4");
        commit_current_timeline(&repo, "Second", Some("body 2")).unwrap();
        write_otio_with_clip(&otio, "shot-b", "raw/bar.mp4");
        commit_current_timeline(&repo, "Third", None).unwrap();

        let entries = log(&repo, 10).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].header, "Third");
        assert_eq!(entries[1].header, "Second");
        assert!(entries[1].full_message.contains("body 2"));
        assert_eq!(entries[2].header, "First");
        assert!(entries[2].parents.is_empty(), "first commit has no parent");
        assert_eq!(entries[1].parents.len(), 1);
    }

    #[test]
    fn log_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_minimal_otio(&otio, "v1");
        let repo = open_or_init(dir.path()).unwrap();
        for i in 0..5 {
            commit_current_timeline(&repo, &format!("c{i}"), None).unwrap();
        }
        let entries = log(&repo, 3).unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn compose_header_for_one_op_is_verbatim() {
        let h = compose_auto_header(&["Trim drone_shot -1.8s".into()]);
        assert_eq!(h, "Trim drone_shot -1.8s");
    }

    #[test]
    fn compose_header_for_two_ops_joins_with_semicolon() {
        let h = compose_auto_header(&[
            "Trim drone_shot -1.8s".into(),
            "Insert BRoll over skyline reference".into(),
        ]);
        assert_eq!(h, "Trim drone_shot -1.8s; Insert BRoll over skyline reference");
    }

    #[test]
    fn compose_header_for_three_plus_ops_summarizes_tail() {
        let h = compose_auto_header(&[
            "Trim A".into(),
            "Trim B".into(),
            "Trim C".into(),
            "Trim D".into(),
            "Trim E".into(),
        ]);
        assert_eq!(h, "Trim A; Trim B; …and 3 more");
    }

    #[test]
    fn compose_header_truncates_at_120_chars() {
        let long = "x".repeat(200);
        let h = compose_auto_header(&[long.clone()]);
        // 120 chars including the ellipsis.
        assert_eq!(h.chars().count(), 120);
        assert!(h.ends_with('…'));
    }

    #[test]
    fn compose_header_handles_empty_list() {
        let h = compose_auto_header(&[]);
        assert!(!h.is_empty());
        assert!(h.contains("envelope"));
    }

    #[test]
    fn auto_commit_apply_writes_commit_with_generated_header() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(&dir.path().join("project.otio.json"), "v1");
        let repo = open_or_init(dir.path()).unwrap();

        // Simulate a 3-op envelope landing.
        let ops = vec![
            "Trim clip-0 by 1.8s".to_string(),
            "Insert BRoll over c-skyline".to_string(),
            "Set Volume on clip-2 to 0.5".to_string(),
        ];
        let outcome = auto_commit_apply(
            &repo,
            &ops,
            Some("User asked for tighter pacing; bundled b-roll over the dirty cut."),
        )
        .unwrap();
        assert!(outcome.commit_hash.starts_with("sha256:"));
        assert!(outcome.message.starts_with("Trim clip-0 by 1.8s; Insert BRoll over c-skyline"));
        assert!(outcome.message.contains("Agent reasoning: User asked for tighter pacing"));
    }

    #[test]
    fn awidat_metadata_survives_commit_round_trip() {
        // Sanity reprise of the standalone probe — make sure the
        // wrapper preserves the same fidelity as direct vedit-core
        // calls. If this ever fails, the wrapper introduced a
        // regression.
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_otio_with_clip(&otio, "shot-a", "raw/foo.mp4");
        let repo = open_or_init(dir.path()).unwrap();
        let out = commit_current_timeline(&repo, "Initial", None).unwrap();

        // Read the committed timeline back via the inner repo (the
        // wrapper deliberately doesn't expose this — it's an internal
        // sanity check).
        let v = repo.inner.read_timeline(&out.timeline_hash).unwrap();
        assert_eq!(v["metadata"]["awidat"]["version"].as_str(), Some("0.1"));
        assert_eq!(
            v["metadata"]["awidat"]["anchors"]["shot-a"]["transcript_snippet"].as_str(),
            Some("hello world")
        );
    }
}
