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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use vedit_core::commit::{Author, Commit};
use vedit_core::diff;
use vedit_core::otio;
use vedit_core::repo::Repo as VeditRepo;

mod animation_diff;
pub use animation_diff::AnimationChange;

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
        self.inner.root.parent().unwrap_or_else(|| Path::new("/"))
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
    let head_target = match repo
        .inner
        .branch_target(DEFAULT_BRANCH)
        .map_err(vedit_err)?
    {
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

/// Identity stamped on a commit. Wrapper-type around vedit's `Author`
/// so callers don't reach into `vedit_core::commit` (see module-level
/// rule 1: no `pub use vedit_core::*`).
///
/// Multi-seat editing, user-authored notes, and meaningful blame all
/// rely on this carrying a real person's name when one is available.
/// When `None` is passed to a `*_as` commit entry point, the default
/// resolver kicks in — see [`resolve_commit_author`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitAuthor {
    /// Display name, e.g. "Alice".
    pub name: String,
    /// Email, e.g. `alice@example.com`.
    pub email: String,
}

impl CommitAuthor {
    fn into_vedit(self) -> Author {
        Author {
            name: self.name,
            email: self.email,
        }
    }

    fn from_vedit(a: Author) -> Self {
        Self {
            name: a.name,
            email: a.email,
        }
    }

    /// Resolve a [`CommitAuthor`] from the runtime env (`AWIDAT_USER_NAME`
    /// + `AWIDAT_USER_EMAIL`). Returns `None` when either is unset or
    /// blank, so callers can explicitly fall back to the default-resolver
    /// chain by passing `None` into `*_as` entry points.
    ///
    /// Useful at handler entry points (e.g. the desktop `apply_edl`
    /// write path) where the call site wants to stamp a real identity on
    /// the commit but does not yet have a richer in-process identity
    /// source (no seat-holder struct, no Tauri identity state).
    pub fn from_env() -> Option<Self> {
        Self::from_env_with(|k| std::env::var(k).ok())
    }

    /// Same as [`CommitAuthor::from_env`] but takes the env source as a
    /// callback so tests can drive the env-var pathway without mutating
    /// process-global state (Rust 2024 requires `unsafe` for
    /// `std::env::set_var`, which is forbidden workspace-wide).
    pub fn from_env_with<F>(env_lookup: F) -> Option<Self>
    where
        F: Fn(&str) -> Option<String>,
    {
        let name = env_lookup(ENV_USER_NAME)?;
        let email = env_lookup(ENV_USER_EMAIL)?;
        let name = name.trim();
        let email = email.trim();
        if name.is_empty() || email.is_empty() {
            return None;
        }
        Some(Self {
            name: name.to_string(),
            email: email.to_string(),
        })
    }
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
///
/// This shim preserves the pre-author-override signature for existing
/// call sites. The author is resolved by [`resolve_commit_author`]
/// (env vars `AWIDAT_USER_NAME` / `AWIDAT_USER_EMAIL`, falling back
/// to the "awidat agent" default). To stamp an explicit identity,
/// call [`commit_current_timeline_as`].
pub fn commit_current_timeline(
    repo: &Repo,
    header: &str,
    agent_reasoning: Option<&str>,
) -> Result<CommitOutcome, VcError> {
    commit_current_timeline_as(repo, header, agent_reasoning, None)
}

/// Same as [`commit_current_timeline`] but lets the caller stamp an
/// explicit identity on the commit. Used by code paths that know the
/// user (desktop session, multi-seat note authoring). Passing `None`
/// is identical to [`commit_current_timeline`] — env vars then default.
pub fn commit_current_timeline_as(
    repo: &Repo,
    header: &str,
    agent_reasoning: Option<&str>,
    author_override: Option<CommitAuthor>,
) -> Result<CommitOutcome, VcError> {
    let bytes = std::fs::read(&repo.project_otio)
        .map_err(|e| VcError::Project(format!("reading {}: {e}", repo.project_otio.display())))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| VcError::Project(format!("parsing {}: {e}", repo.project_otio.display())))?;

    let timeline_hash = repo.inner.write_timeline(&value).map_err(vedit_err)?;
    let message = format_commit_message(header, agent_reasoning);
    let author = resolve_commit_author(author_override);
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

/// Command-history-style metadata embedded in Awidat-authored vedit commit messages.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ActionMetadata {
    /// Source actor for the envelope when known.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
    /// Operations landed by the envelope, in source order.
    #[serde(default)]
    pub operations: Vec<crate::edl::AppliedOpMetadata>,
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

fn format_commit_message_with_action_metadata(
    header: &str,
    agent_reasoning: Option<&str>,
    action_metadata: Option<&ActionMetadata>,
) -> String {
    let mut message = format_commit_message(header, agent_reasoning);
    let Some(action_metadata) = action_metadata else {
        return message;
    };
    if action_metadata.operations.is_empty() {
        return message;
    }
    if !message.contains("\n\n") {
        message.push_str("\n\n");
    } else {
        message.push('\n');
    }
    let metadata_json = serde_json::to_string(action_metadata).unwrap_or_else(|_| "{}".to_string());
    message.push_str("Action metadata: ");
    message.push_str(&metadata_json);
    message
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
    auto_commit_apply_as(repo, op_descriptions, agent_reasoning, None)
}

/// Same as [`auto_commit_apply`] but lets the caller stamp an explicit
/// identity on the commit. Hot path: the desktop's apply_edl handler
/// knows the seat-holder; the agent's apply_edl handler may not.
pub fn auto_commit_apply_as(
    repo: &Repo,
    op_descriptions: &[String],
    agent_reasoning: Option<&str>,
    author_override: Option<CommitAuthor>,
) -> Result<CommitOutcome, VcError> {
    auto_commit_apply_as_with_metadata(repo, op_descriptions, agent_reasoning, author_override, None)
}

/// Auto-commit an apply_edl envelope with optional structured action metadata.
pub fn auto_commit_apply_with_metadata(
    repo: &Repo,
    op_descriptions: &[String],
    agent_reasoning: Option<&str>,
    action_metadata: Option<&ActionMetadata>,
) -> Result<CommitOutcome, VcError> {
    auto_commit_apply_as_with_metadata(repo, op_descriptions, agent_reasoning, None, action_metadata)
}

/// Auto-commit an apply_edl envelope with optional author attribution and metadata.
pub fn auto_commit_apply_as_with_metadata(
    repo: &Repo,
    op_descriptions: &[String],
    agent_reasoning: Option<&str>,
    author_override: Option<CommitAuthor>,
    action_metadata: Option<&ActionMetadata>,
) -> Result<CommitOutcome, VcError> {
    let header = compose_auto_header(op_descriptions);
    let bytes = std::fs::read(&repo.project_otio)
        .map_err(|e| VcError::Project(format!("reading {}: {e}", repo.project_otio.display())))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| VcError::Project(format!("parsing {}: {e}", repo.project_otio.display())))?;

    let timeline_hash = repo.inner.write_timeline(&value).map_err(vedit_err)?;
    let message =
        format_commit_message_with_action_metadata(&header, agent_reasoning, action_metadata);
    let author = resolve_commit_author(author_override);
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

/// Default author when no override or env config is present. Generic
/// on purpose — when the agent is acting on the user's behalf and
/// nobody has declared themselves, the commit is the agent's, not a
/// fake personal stamp.
fn default_awidat_author() -> Author {
    Author {
        name: "awidat agent".to_string(),
        email: "agent@awidat.local".to_string(),
    }
}

/// Environment variables consulted as a runtime fallback for commit
/// attribution. Both must be set; a half-configured pair (just the
/// name, or just the email) is treated as not configured so blame
/// views never end up with a real name and a stale or guessed email.
const ENV_USER_NAME: &str = "AWIDAT_USER_NAME";
const ENV_USER_EMAIL: &str = "AWIDAT_USER_EMAIL";

/// Resolve which `Author` to stamp on a commit, in priority order:
///
/// 1. `author_override` — the call-site identity (multi-seat editing,
///    user-authored notes, anything that already knows the user).
/// 2. `AWIDAT_USER_NAME` + `AWIDAT_USER_EMAIL` env vars — useful for
///    CLI / TUI sessions where the user is identifiable from process
///    env (`git`-style configuration).
/// 3. The "awidat agent" default — anonymous attribution, matches
///    pre-slice behavior for backward compat.
///
/// Kept private so the priority chain stays a single decision point;
/// callers go through `commit_current_timeline[_as]` / `merge_refs[_as]`
/// rather than picking an author themselves.
fn resolve_commit_author(author_override: Option<CommitAuthor>) -> Author {
    resolve_commit_author_with_env(author_override, |k| std::env::var(k).ok())
}

/// Same priority chain as [`resolve_commit_author`], but takes the env
/// source as a callback so tests can drive the env-var pathway without
/// mutating process-global state (Rust 2024 requires `unsafe` for
/// `std::env::set_var`, which is forbidden workspace-wide).
fn resolve_commit_author_with_env<F>(author_override: Option<CommitAuthor>, env_lookup: F) -> Author
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(explicit) = author_override {
        return explicit.into_vedit();
    }
    if let (Some(name), Some(email)) = (env_lookup(ENV_USER_NAME), env_lookup(ENV_USER_EMAIL)) {
        let name = name.trim();
        let email = email.trim();
        if !name.is_empty() && !email.is_empty() {
            return Author {
                name: name.to_string(),
                email: email.to_string(),
            };
        }
    }
    default_awidat_author()
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
    let before_value = match from_resolved {
        Some(hash) => Some(read_timeline_value_at_commit(repo, &hash)?),
        None => None,
    };
    let after_value = match to_resolved {
        Some(hash) => Some(read_timeline_value_at_commit(repo, &hash)?),
        None => None,
    };
    let (changes, animation_changes) =
        diff_timeline_values(before_value.as_ref(), after_value.as_ref())?;

    Ok(CommittedDiff {
        from_ref: from_ref.unwrap_or(SESSION_START_BRANCH).to_string(),
        to_ref: to_ref.unwrap_or("HEAD").to_string(),
        changes,
        animation_changes,
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
            Err(_) => read_tag_target(repo, r)
                .map(Some)
                .ok_or_else(|| VcError::UnknownRef(r.to_string())),
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

fn resolve_ref(repo: &Repo, refstr: &str) -> Result<String, VcError> {
    match repo.inner.resolve(refstr) {
        Ok(hash) => Ok(hash),
        Err(_) => {
            read_tag_target(repo, refstr).ok_or_else(|| VcError::UnknownRef(refstr.to_string()))
        }
    }
}

fn read_timeline_value_at_commit(
    repo: &Repo,
    commit_hash: &str,
) -> Result<serde_json::Value, VcError> {
    let commit: Commit = repo.inner.read_commit(commit_hash).map_err(vedit_err)?;
    repo.inner
        .read_timeline(&commit.timeline)
        .map_err(vedit_err)
}

fn diff_timeline_values(
    before_value: Option<&serde_json::Value>,
    after_value: Option<&serde_json::Value>,
) -> Result<(Vec<diff::Change>, Vec<AnimationChange>), VcError> {
    let before_timeline = before_value
        .map(otio::parse_timeline)
        .transpose()
        .map_err(vedit_err)?;
    let after_timeline = after_value
        .map(otio::parse_timeline)
        .transpose()
        .map_err(vedit_err)?;

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
    let animation_changes = animation_diff::diff_parameter_animations(before_value, after_value)?;
    Ok((changes, animation_changes))
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
    /// Awidat animation metadata changes omitted by vedit-core's structural model.
    pub animation_changes: Vec<AnimationChange>,
}

impl CommittedDiff {
    /// Number of structural changes in the diff.
    pub fn len(&self) -> usize {
        self.changes.len() + self.animation_changes.len()
    }

    /// True iff the structural diff is empty. Note: a metadata-only
    /// commit (e.g. agent reasoning updated, no clip changed) has an
    /// empty structural diff but a non-zero hash difference. Phase B
    /// commit messages will surface this as "metadata only."
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.animation_changes.is_empty()
    }
}

/// Create or update a flat tag under `.vedit/refs/tags/<name>`.
pub fn tag_ref(repo: &Repo, name: &str, refstr: Option<&str>) -> Result<TagRef, VcError> {
    let name = validate_flat_ref_name("tag", name)?;
    let requested_ref = refstr.unwrap_or("HEAD");
    let target = resolve_ref(repo, requested_ref)?;
    let path = tag_path(repo, &name);
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| VcError::Vedit(format!("create refs/tags/: {e}")))?;
    }
    std::fs::write(&path, format!("{target}\n"))
        .map_err(|e| VcError::Vedit(format!("writing tag {name}: {e}")))?;
    Ok(TagRef { name, target })
}

/// List flat tags from `.vedit/refs/tags/`, sorted by name.
pub fn list_tags(repo: &Repo) -> Result<Vec<TagRef>, VcError> {
    let dir = repo.inner.root.join("refs").join("tags");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut tags = Vec::new();
    for entry in
        std::fs::read_dir(&dir).map_err(|e| VcError::Vedit(format!("reading refs/tags/: {e}")))?
    {
        let entry = entry.map_err(|e| VcError::Vedit(format!("reading tag entry: {e}")))?;
        if !entry
            .file_type()
            .map_err(|e| VcError::Vedit(format!("reading tag file type: {e}")))?
            .is_file()
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let target = std::fs::read_to_string(entry.path())
            .map_err(|e| VcError::Vedit(format!("reading tag {name}: {e}")))?
            .trim()
            .to_string();
        tags.push(TagRef { name, target });
    }
    tags.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tags)
}

fn tag_path(repo: &Repo, name: &str) -> PathBuf {
    repo.inner.root.join("refs").join("tags").join(name)
}

fn read_tag_target(repo: &Repo, name: &str) -> Option<String> {
    let Ok(name) = validate_flat_ref_name("tag", name) else {
        return None;
    };
    let path = tag_path(repo, &name);
    if !path.exists() {
        return None;
    }
    std::fs::read_to_string(path)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|target| target.starts_with("sha256:"))
}

fn validate_flat_ref_name(kind: &str, name: &str) -> Result<String, VcError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(VcError::Vedit(format!("{kind} name cannot be empty")));
    }
    if name == "." || name == ".." {
        return Err(VcError::Vedit(format!("{kind} name cannot be {name:?}")));
    }
    if name
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
    {
        return Err(VcError::Vedit(format!(
            "{kind} name {name:?} must be flat ASCII using letters, digits, '.', '-', or '_'"
        )));
    }
    Ok(name.to_string())
}

/// One named vedit tag.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TagRef {
    /// Flat tag name under `.vedit/refs/tags/`.
    pub name: String,
    /// Commit hash the tag points at.
    pub target: String,
}

/// Create a user-facing branch/alternate pointing at a ref.
pub fn create_branch(
    repo: &Repo,
    name: &str,
    start_ref: Option<&str>,
) -> Result<BranchRef, VcError> {
    let start_ref = start_ref.unwrap_or("HEAD");
    let target = resolve_ref(repo, start_ref)?;
    let created = repo.inner.create_branch(name, &target).map_err(vedit_err)?;
    let current = repo.inner.current_branch().map_err(vedit_err)?;
    Ok(BranchRef {
        name: name.trim().to_string(),
        target: created,
        is_current: current.as_deref() == Some(name.trim()),
    })
}

/// List user-facing branches/alternates from `.vedit/refs/heads/`.
pub fn list_branches(repo: &Repo) -> Result<Vec<BranchRef>, VcError> {
    let current = repo.inner.current_branch().map_err(vedit_err)?;
    let branches = repo.inner.list_branches().map_err(vedit_err)?;
    Ok(branches
        .into_iter()
        .map(|(name, target)| {
            let is_current = current.as_deref() == Some(name.as_str());
            BranchRef {
                name,
                target,
                is_current,
            }
        })
        .collect())
}

/// Switch HEAD to an existing branch and restore the working timeline
/// to that branch's committed snapshot.
pub fn checkout_branch(repo: &Repo, name: &str) -> Result<CheckoutOutcome, VcError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(VcError::Vedit("branch name cannot be empty".to_string()));
    }
    let target = repo
        .inner
        .branch_target(name)
        .map_err(vedit_err)?
        .ok_or_else(|| VcError::UnknownRef(name.to_string()))?;
    let restored = restore_working_timeline(repo, name)?;
    repo.inner.switch_branch(name).map_err(vedit_err)?;
    Ok(CheckoutOutcome {
        branch: name.to_string(),
        commit_hash: target,
        timeline_hash: restored.timeline_hash,
    })
}

/// One vedit branch/alternate.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BranchRef {
    /// Flat branch name under `.vedit/refs/heads/`.
    pub name: String,
    /// Commit hash the branch points at.
    pub target: String,
    /// Whether HEAD currently points at this branch.
    pub is_current: bool,
}

/// Result of checking out a vedit branch.
#[derive(Debug, Clone)]
pub struct CheckoutOutcome {
    /// Branch HEAD now points at.
    pub branch: String,
    /// Commit hash restored into `project.otio.json`.
    pub commit_hash: String,
    /// Timeline object hash restored into `project.otio.json`.
    pub timeline_hash: String,
}

/// Show one commit and its semantic diff from its first parent.
pub fn show_commit(repo: &Repo, refstr: &str) -> Result<CommitDetails, VcError> {
    let commit_hash = resolve_ref(repo, refstr)?;
    let commit = repo.inner.read_commit(&commit_hash).map_err(vedit_err)?;
    let diff = diff_commit_against_parent(repo, &commit_hash, &commit)?;
    Ok(CommitDetails {
        commit_hash,
        timestamp: commit.timestamp,
        header: header_line(&commit.message),
        action_metadata: parse_action_metadata(&commit.message),
        full_message: commit.message,
        timeline_hash: commit.timeline,
        parents: commit.parents,
        diff,
    })
}

fn diff_commit_against_parent(
    repo: &Repo,
    commit_hash: &str,
    commit: &Commit,
) -> Result<CommittedDiff, VcError> {
    let before_value = commit
        .parents
        .first()
        .map(|parent| read_timeline_value_at_commit(repo, parent))
        .transpose()?;
    let after_value = Some(
        repo.inner
            .read_timeline(&commit.timeline)
            .map_err(vedit_err)?,
    );
    let (changes, animation_changes) =
        diff_timeline_values(before_value.as_ref(), after_value.as_ref())?;
    Ok(CommittedDiff {
        from_ref: commit
            .parents
            .first()
            .cloned()
            .unwrap_or_else(|| "<empty>".to_string()),
        to_ref: commit_hash.to_string(),
        changes,
        animation_changes,
    })
}

/// Commit details plus the local diff for that commit.
#[derive(Debug, Clone)]
pub struct CommitDetails {
    /// Resolved commit hash.
    pub commit_hash: String,
    /// ISO-8601 timestamp.
    pub timestamp: String,
    /// First line of the message.
    pub header: String,
    /// Optional structured action metadata embedded by Awidat auto-commit.
    pub action_metadata: Option<ActionMetadata>,
    /// Complete commit message.
    pub full_message: String,
    /// Timeline object hash.
    pub timeline_hash: String,
    /// Parent commit hashes.
    pub parents: Vec<String>,
    /// Diff from the first parent to this commit.
    pub diff: CommittedDiff,
}

/// Project the first-parent log onto changes touching one clip.
pub fn blame_clip(
    repo: &Repo,
    clip_id: &str,
    start_ref: Option<&str>,
    limit: usize,
) -> Result<Vec<BlameEntry>, VcError> {
    let clip_id = clip_id.trim();
    if clip_id.is_empty() {
        return Err(VcError::Vedit("clip id cannot be empty".to_string()));
    }
    let start_resolved = start_ref.map(|r| resolve_ref(repo, r)).transpose()?;
    let entries = repo
        .inner
        .log(start_resolved.as_deref())
        .map_err(vedit_err)?;
    let mut out = Vec::new();
    for (commit_hash, commit) in entries.into_iter().take(limit) {
        let diff = diff_commit_against_parent(repo, &commit_hash, &commit)?;
        let changes = diff
            .changes
            .into_iter()
            .filter(|change| change_touches_clip(change, clip_id))
            .collect::<Vec<_>>();
        let animation_changes = diff
            .animation_changes
            .into_iter()
            .filter(|change| animation_change_touches_clip(change, clip_id))
            .collect::<Vec<_>>();
        if changes.is_empty() && animation_changes.is_empty() {
            continue;
        }
        out.push(BlameEntry {
            commit_hash,
            timestamp: commit.timestamp,
            header: header_line(&commit.message),
            action_metadata: parse_action_metadata(&commit.message),
            full_message: commit.message,
            timeline_hash: commit.timeline,
            parents: commit.parents,
            changes,
            animation_changes,
        });
    }
    Ok(out)
}

/// Return stable clip/media identifiers touched by a committed diff.
pub fn changed_clip_ids(diff: &CommittedDiff) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    for change in &diff.changes {
        collect_change_clip_ids(change, &mut ids);
    }
    for change in &diff.animation_changes {
        collect_animation_change_clip_ids(change, &mut ids);
    }
    ids
}

/// Read-only preflight for the first bounded merge surface.
///
/// This intentionally does not merge or move refs. It reports whether
/// both sides changed any of the same stable clip/media identifiers
/// since their common ancestor, which is the conflict boundary the
/// local-first merge plan can safely enforce before a full OTIO
/// three-way merge exists.
pub fn merge_preflight(
    repo: &Repo,
    source_ref: &str,
    target_ref: Option<&str>,
) -> Result<MergePreflight, VcError> {
    let source_ref = source_ref.trim();
    if source_ref.is_empty() {
        return Err(VcError::Vedit("source ref cannot be empty".to_string()));
    }
    let target_ref = target_ref.unwrap_or("HEAD").trim();
    if target_ref.is_empty() {
        return Err(VcError::Vedit("target ref cannot be empty".to_string()));
    }

    let source_commit = resolve_ref(repo, source_ref)?;
    let target_commit = resolve_ref(repo, target_ref)?;
    let merge_base = common_ancestor(repo, &source_commit, &target_commit)?;

    let source_diff = diff_refs(repo, Some(&merge_base), Some(&source_commit))?;
    let target_diff = diff_refs(repo, Some(&merge_base), Some(&target_commit))?;
    let source_ids = changed_clip_ids(&source_diff);
    let target_ids = changed_clip_ids(&target_diff);
    let overlapping_clip_ids = source_ids
        .intersection(&target_ids)
        .cloned()
        .collect::<Vec<_>>();

    Ok(MergePreflight {
        source_ref: source_ref.to_string(),
        target_ref: target_ref.to_string(),
        source_commit,
        target_commit,
        merge_base,
        is_mergeable: overlapping_clip_ids.is_empty(),
        source_changed_clip_ids: source_ids.into_iter().collect(),
        target_changed_clip_ids: target_ids.into_iter().collect(),
        overlapping_clip_ids,
        source_change_count: source_diff.len(),
        target_change_count: target_diff.len(),
    })
}

fn common_ancestor(repo: &Repo, left: &str, right: &str) -> Result<String, VcError> {
    let left_ancestors = ancestor_distances(repo, left)?;
    let right_ancestors = ancestor_distances(repo, right)?;
    left_ancestors
        .iter()
        .filter_map(|(hash, left_distance)| {
            right_ancestors.get(hash).map(|right_distance| {
                (
                    left_distance + right_distance,
                    *left_distance,
                    hash.to_string(),
                )
            })
        })
        .min()
        .map(|(_, _, hash)| hash)
        .ok_or_else(|| VcError::Vedit(format!("no common ancestor found for {left} and {right}")))
}

fn ancestor_distances(repo: &Repo, start: &str) -> Result<BTreeMap<String, usize>, VcError> {
    let mut distances = BTreeMap::new();
    let mut stack = vec![(start.to_string(), 0usize)];
    while let Some((hash, distance)) = stack.pop() {
        if distances
            .get(&hash)
            .is_some_and(|existing| *existing <= distance)
        {
            continue;
        }
        distances.insert(hash.clone(), distance);
        let commit: Commit = repo.inner.read_commit(&hash).map_err(vedit_err)?;
        for parent in commit.parents {
            stack.push((parent, distance + 1));
        }
    }
    Ok(distances)
}

/// Result of checking whether a source ref can be safely merged into a
/// target ref under the strict non-overlapping-clip rule.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MergePreflight {
    /// Source ref requested by the caller.
    pub source_ref: String,
    /// Target ref requested by the caller. Defaults to `HEAD`.
    pub target_ref: String,
    /// Resolved source commit hash.
    pub source_commit: String,
    /// Resolved target commit hash.
    pub target_commit: String,
    /// Common ancestor used as the diff base for both sides.
    pub merge_base: String,
    /// True when no clip/media ids overlap between source and target changes.
    pub is_mergeable: bool,
    /// Sorted clip/media ids changed by the source since the merge base.
    pub source_changed_clip_ids: Vec<String>,
    /// Sorted clip/media ids changed by the target since the merge base.
    pub target_changed_clip_ids: Vec<String>,
    /// Sorted ids changed on both sides. Non-empty means conflict.
    pub overlapping_clip_ids: Vec<String>,
    /// Source structural plus animation change count since the merge base.
    pub source_change_count: usize,
    /// Target structural plus animation change count since the merge base.
    pub target_change_count: usize,
}

/// Execute the approved bounded merge rule: merge only when source and
/// target changed non-overlapping clip/media ids. The current
/// implementation overlays source-changed clip objects onto the target
/// timeline and writes a two-parent commit on the target branch.
pub fn merge_refs(
    repo: &Repo,
    source_ref: &str,
    target_ref: Option<&str>,
) -> Result<MergeOutcome, VcError> {
    merge_refs_as(repo, source_ref, target_ref, None)
}

/// Same as [`merge_refs`] but stamps an explicit identity on the merge
/// commit. Passing `None` falls back to [`resolve_commit_author`]
/// (env vars, then the "awidat agent" default).
pub fn merge_refs_as(
    repo: &Repo,
    source_ref: &str,
    target_ref: Option<&str>,
    author_override: Option<CommitAuthor>,
) -> Result<MergeOutcome, VcError> {
    let preflight = merge_preflight(repo, source_ref, target_ref)?;
    if !preflight.is_mergeable {
        return Err(VcError::Vedit(format!(
            "merge conflicts on overlapping clip ids: {}",
            preflight.overlapping_clip_ids.join(", ")
        )));
    }

    checkout_merge_target(repo, &preflight.target_ref, &preflight.target_commit)?;
    let source_value = read_timeline_value_at_commit(repo, &preflight.source_commit)?;
    let mut merged_value = read_timeline_value_at_commit(repo, &preflight.target_commit)?;
    overlay_source_changed_clips(
        &mut merged_value,
        &source_value,
        &preflight.source_changed_clip_ids,
    )?;

    let pretty = serde_json::to_vec_pretty(&merged_value)
        .map_err(|e| VcError::Project(format!("serializing merged timeline: {e}")))?;
    std::fs::write(&repo.project_otio, pretty)
        .map_err(|e| VcError::Project(format!("writing {}: {e}", repo.project_otio.display())))?;
    let timeline_hash = repo
        .inner
        .write_timeline(&merged_value)
        .map_err(vedit_err)?;
    let message = format_commit_message(
        &format!(
            "Merge {} into {}",
            preflight.source_ref, preflight.target_ref
        ),
        Some(
            "Merged only after bounded preflight confirmed non-overlapping changed clip/media ids.",
        ),
    );
    let parents = vec![
        preflight.target_commit.clone(),
        preflight.source_commit.clone(),
    ];
    let commit_hash = repo
        .inner
        .commit_with_parents(
            &timeline_hash,
            parents.clone(),
            resolve_commit_author(author_override),
            &message,
        )
        .map_err(vedit_err)?;

    Ok(MergeOutcome {
        commit_hash,
        timeline_hash,
        message,
        source_ref: preflight.source_ref,
        target_ref: preflight.target_ref,
        source_commit: preflight.source_commit,
        target_commit: preflight.target_commit,
        merge_base: preflight.merge_base,
        parents,
        source_changed_clip_ids: preflight.source_changed_clip_ids,
        target_changed_clip_ids: preflight.target_changed_clip_ids,
    })
}

fn checkout_merge_target(
    repo: &Repo,
    target_ref: &str,
    target_commit: &str,
) -> Result<(), VcError> {
    let current = repo.inner.current_branch().map_err(vedit_err)?;
    if target_ref == "HEAD" {
        return Ok(());
    }
    if current.as_deref() == Some(target_ref) {
        return Ok(());
    }
    let target_branch = repo
        .inner
        .branch_target(target_ref)
        .map_err(vedit_err)?
        .filter(|branch_target| branch_target == target_commit);
    if target_branch.is_none() {
        return Err(VcError::Vedit(format!(
            "bounded merge target {target_ref:?} must be an existing branch or HEAD"
        )));
    }
    repo.inner.switch_branch(target_ref).map_err(vedit_err)
}

fn overlay_source_changed_clips(
    target: &mut serde_json::Value,
    source: &serde_json::Value,
    changed_ids: &[String],
) -> Result<(), VcError> {
    let changed_ids = changed_ids.iter().cloned().collect::<BTreeSet<_>>();
    if changed_ids.is_empty() {
        return Ok(());
    }
    let source_clips = collect_matching_clips(source, &changed_ids);
    if source_clips.is_empty() {
        return Err(VcError::Vedit(
            "bounded merge source changed ids did not resolve to mergeable clip objects"
                .to_string(),
        ));
    }
    for source_clip in source_clips {
        let ids = clip_value_ids(&source_clip);
        if !replace_matching_clip(target, &ids, source_clip.clone()) {
            return Err(VcError::Vedit(format!(
                "bounded merge cannot apply source clip change for ids: {}",
                ids.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }
    }
    Ok(())
}

fn collect_matching_clips(
    timeline: &serde_json::Value,
    wanted_ids: &BTreeSet<String>,
) -> Vec<serde_json::Value> {
    let mut clips = Vec::new();
    if let Some(tracks) = timeline
        .get("tracks")
        .and_then(|tracks| tracks.get("children"))
        .and_then(serde_json::Value::as_array)
    {
        for track in tracks {
            if let Some(children) = track.get("children").and_then(serde_json::Value::as_array) {
                for child in children {
                    if is_clip_value(child) && !clip_value_ids(child).is_disjoint(wanted_ids) {
                        clips.push(child.clone());
                    }
                }
            }
        }
    }
    clips
}

fn replace_matching_clip(
    timeline: &mut serde_json::Value,
    wanted_ids: &BTreeSet<String>,
    replacement: serde_json::Value,
) -> bool {
    let Some(tracks) = timeline
        .get_mut("tracks")
        .and_then(|tracks| tracks.get_mut("children"))
        .and_then(serde_json::Value::as_array_mut)
    else {
        return false;
    };
    for track in tracks {
        let Some(children) = track
            .get_mut("children")
            .and_then(serde_json::Value::as_array_mut)
        else {
            continue;
        };
        for child in children {
            if is_clip_value(child) && !clip_value_ids(child).is_disjoint(wanted_ids) {
                *child = replacement;
                return true;
            }
        }
    }
    false
}

fn is_clip_value(value: &serde_json::Value) -> bool {
    value
        .get("OTIO_SCHEMA")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|schema| schema.starts_with("Clip."))
}

fn clip_value_ids(value: &serde_json::Value) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    collect_optional_id(
        value.get("name").and_then(serde_json::Value::as_str),
        &mut ids,
    );
    collect_optional_id(
        value
            .get("media_reference")
            .and_then(|media| media.get("target_url"))
            .and_then(serde_json::Value::as_str),
        &mut ids,
    );
    ids
}

/// Result of a successful bounded merge.
#[derive(Debug, Clone)]
pub struct MergeOutcome {
    /// New two-parent merge commit hash.
    pub commit_hash: String,
    /// Timeline object hash written by the merge commit.
    pub timeline_hash: String,
    /// Final commit message written to the object.
    pub message: String,
    /// Source ref requested by the caller.
    pub source_ref: String,
    /// Target ref requested by the caller.
    pub target_ref: String,
    /// Resolved source commit hash.
    pub source_commit: String,
    /// Resolved target commit hash.
    pub target_commit: String,
    /// Common ancestor used by preflight.
    pub merge_base: String,
    /// Commit parents, target first then source.
    pub parents: Vec<String>,
    /// Sorted source changed ids accepted by preflight.
    pub source_changed_clip_ids: Vec<String>,
    /// Sorted target changed ids accepted by preflight.
    pub target_changed_clip_ids: Vec<String>,
}

fn change_touches_clip(change: &diff::Change, clip_id: &str) -> bool {
    match change {
        diff::Change::Trimmed { clip, .. }
        | diff::Change::Added { clip, .. }
        | diff::Change::Removed { clip, .. }
        | diff::Change::EffectsChanged { clip, .. }
        | diff::Change::Replaced { clip, .. } => clip_ref_matches(clip, clip_id),
        diff::Change::Moved {
            clip,
            after_neighbor,
            before_neighbor,
            ..
        } => {
            clip_ref_matches(clip, clip_id)
                || after_neighbor
                    .as_ref()
                    .is_some_and(|clip| clip_ref_matches(clip, clip_id))
                || before_neighbor
                    .as_ref()
                    .is_some_and(|clip| clip_ref_matches(clip, clip_id))
        }
        diff::Change::TransitionAdded {
            between_before,
            between_after,
            ..
        }
        | diff::Change::TransitionRemoved {
            between_before,
            between_after,
            ..
        } => {
            between_before
                .as_ref()
                .is_some_and(|clip| clip_ref_matches(clip, clip_id))
                || between_after
                    .as_ref()
                    .is_some_and(|clip| clip_ref_matches(clip, clip_id))
        }
        diff::Change::TrackAdded { .. } | diff::Change::TrackRemoved { .. } => false,
    }
}

fn clip_ref_matches(clip: &diff::ClipRef, clip_id: &str) -> bool {
    clip.name == clip_id || clip.media_reference.as_deref() == Some(clip_id)
}

fn animation_change_touches_clip(change: &AnimationChange, clip_id: &str) -> bool {
    let mut ids = BTreeSet::new();
    collect_animation_change_clip_ids(change, &mut ids);
    ids.contains(clip_id)
}

fn collect_change_clip_ids(change: &diff::Change, ids: &mut BTreeSet<String>) {
    match change {
        diff::Change::Trimmed { clip, .. }
        | diff::Change::Added { clip, .. }
        | diff::Change::Removed { clip, .. }
        | diff::Change::EffectsChanged { clip, .. } => collect_clip_ref_ids(clip, ids),
        diff::Change::Replaced {
            clip,
            before_media,
            after_media,
            ..
        } => {
            collect_clip_ref_ids(clip, ids);
            collect_optional_id(before_media.as_deref(), ids);
            collect_optional_id(after_media.as_deref(), ids);
        }
        diff::Change::Moved {
            clip,
            after_neighbor,
            before_neighbor,
            ..
        } => {
            collect_clip_ref_ids(clip, ids);
            collect_optional_clip_ref_ids(after_neighbor.as_ref(), ids);
            collect_optional_clip_ref_ids(before_neighbor.as_ref(), ids);
        }
        diff::Change::TransitionAdded {
            between_before,
            between_after,
            ..
        }
        | diff::Change::TransitionRemoved {
            between_before,
            between_after,
            ..
        } => {
            collect_optional_clip_ref_ids(between_before.as_ref(), ids);
            collect_optional_clip_ref_ids(between_after.as_ref(), ids);
        }
        diff::Change::TrackAdded { .. } | diff::Change::TrackRemoved { .. } => {}
    }
}

fn collect_clip_ref_ids(clip: &diff::ClipRef, ids: &mut BTreeSet<String>) {
    collect_optional_id(Some(&clip.name), ids);
    collect_optional_id(clip.media_reference.as_deref(), ids);
}

fn collect_optional_clip_ref_ids(clip: Option<&diff::ClipRef>, ids: &mut BTreeSet<String>) {
    if let Some(clip) = clip {
        collect_clip_ref_ids(clip, ids);
    }
}

fn collect_animation_change_clip_ids(change: &AnimationChange, ids: &mut BTreeSet<String>) {
    match change {
        AnimationChange::AnimationAdded { animation }
        | AnimationChange::AnimationRemoved { animation } => {
            collect_animation_target_clip_id(&animation.target, ids);
        }
        AnimationChange::AnimationUpdated {
            target,
            field_changes,
            ..
        } => {
            collect_animation_target_clip_id(target, ids);
            for field_change in field_changes {
                if field_change.field == "target" {
                    collect_animation_target_value_clip_id(&field_change.before, ids);
                    collect_animation_target_value_clip_id(&field_change.after, ids);
                }
            }
        }
    }
}

fn collect_animation_target_value_clip_id(value: &serde_json::Value, ids: &mut BTreeSet<String>) {
    if let Some(target) = value.as_str() {
        collect_animation_target_clip_id(target, ids);
    }
}

fn collect_animation_target_clip_id(target: &str, ids: &mut BTreeSet<String>) {
    let Some(rest) = target.strip_prefix("clip:") else {
        return;
    };
    let clip_id = rest.split('/').next().unwrap_or_default();
    collect_optional_id(Some(clip_id), ids);
}

fn collect_optional_id(id: Option<&str>, ids: &mut BTreeSet<String>) {
    let Some(id) = id.map(str::trim).filter(|id| !id.is_empty()) else {
        return;
    };
    ids.insert(id.to_string());
}

/// One blame projection entry.
#[derive(Debug, Clone)]
pub struct BlameEntry {
    /// Commit hash where this clip changed.
    pub commit_hash: String,
    /// ISO-8601 timestamp.
    pub timestamp: String,
    /// First line of the commit message.
    pub header: String,
    /// Optional structured action metadata embedded by Awidat auto-commit.
    pub action_metadata: Option<ActionMetadata>,
    /// Complete commit message.
    pub full_message: String,
    /// Timeline hash for this commit.
    pub timeline_hash: String,
    /// Parent commit hashes.
    pub parents: Vec<String>,
    /// Matching structural changes.
    pub changes: Vec<diff::Change>,
    /// Matching animation metadata changes.
    pub animation_changes: Vec<AnimationChange>,
}

/// Restore the project's working `project.otio.json` to the timeline
/// stored at `refstr`. This does not move HEAD by itself; callers that
/// want an auditable revert should call [`commit_current_timeline`]
/// after the restore succeeds.
pub fn restore_working_timeline(repo: &Repo, refstr: &str) -> Result<RestoreOutcome, VcError> {
    let commit_hash = resolve_ref(repo, refstr)?;
    let commit: Commit = repo.inner.read_commit(&commit_hash).map_err(vedit_err)?;
    let timeline_value = repo
        .inner
        .read_timeline(&commit.timeline)
        .map_err(vedit_err)?;
    let pretty = serde_json::to_vec_pretty(&timeline_value)
        .map_err(|e| VcError::Project(format!("serializing restored timeline: {e}")))?;
    std::fs::write(&repo.project_otio, pretty)
        .map_err(|e| VcError::Project(format!("writing {}: {e}", repo.project_otio.display())))?;

    Ok(RestoreOutcome {
        requested_ref: refstr.to_string(),
        commit_hash,
        timeline_hash: commit.timeline,
    })
}

/// Result of restoring the working timeline to a committed snapshot.
#[derive(Debug, Clone)]
pub struct RestoreOutcome {
    /// Ref string requested by the caller.
    pub requested_ref: String,
    /// Resolved commit hash.
    pub commit_hash: String,
    /// Timeline object hash restored into `project.otio.json`.
    pub timeline_hash: String,
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
            action_metadata: parse_action_metadata(&commit.message),
            full_message: commit.message,
            timeline_hash: commit.timeline,
            parents: commit.parents,
            author: CommitAuthor::from_vedit(commit.author),
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
    /// Optional structured action metadata embedded by Awidat auto-commit.
    pub action_metadata: Option<ActionMetadata>,
    /// Full message body for "show this commit" deep dives.
    pub full_message: String,
    /// `sha256:...` of the timeline this commit points at.
    pub timeline_hash: String,
    /// Parent commit hashes (1 = normal, 0 = initial, 2+ = merge).
    pub parents: Vec<String>,
    /// Identity stamped on the commit. Backward-compat: pre-slice
    /// commits read back as `awidat agent <agent@awidat.local>`.
    pub author: CommitAuthor,
}

fn header_line(message: &str) -> String {
    message.lines().next().unwrap_or("").trim().to_string()
}

fn parse_action_metadata(message: &str) -> Option<ActionMetadata> {
    message.lines().find_map(|line| {
        let json = line.trim().strip_prefix("Action metadata: ")?;
        serde_json::from_str(json).ok()
    })
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
        write_otio_with_clip_duration(path, clip_name, source_url, 240.0);
    }

    fn write_otio_with_clip_duration(
        path: &Path,
        clip_name: &str,
        source_url: &str,
        duration_frames: f64,
    ) {
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
                                "value": duration_frames,
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

    fn write_otio_with_two_clip_durations(path: &Path, clip_a_frames: f64, clip_b_frames: f64) {
        let v = serde_json::json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": "test",
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "name": "tracks",
                "children": [{
                    "OTIO_SCHEMA": "Track.1",
                    "name": "V1",
                    "kind": "Video",
                    "children": [
                        {
                            "OTIO_SCHEMA": "Clip.2",
                            "name": "shot-a",
                            "source_range": {
                                "OTIO_SCHEMA": "TimeRange.1",
                                "start_time": {
                                    "OTIO_SCHEMA": "RationalTime.1",
                                    "value": 0.0,
                                    "rate": 24.0
                                },
                                "duration": {
                                    "OTIO_SCHEMA": "RationalTime.1",
                                    "value": clip_a_frames,
                                    "rate": 24.0
                                }
                            },
                            "media_reference": {
                                "OTIO_SCHEMA": "ExternalReference.1",
                                "target_url": "raw/a.mp4"
                            }
                        },
                        {
                            "OTIO_SCHEMA": "Clip.2",
                            "name": "shot-b",
                            "source_range": {
                                "OTIO_SCHEMA": "TimeRange.1",
                                "start_time": {
                                    "OTIO_SCHEMA": "RationalTime.1",
                                    "value": 0.0,
                                    "rate": 24.0
                                },
                                "duration": {
                                    "OTIO_SCHEMA": "RationalTime.1",
                                    "value": clip_b_frames,
                                    "rate": 24.0
                                }
                            },
                            "media_reference": {
                                "OTIO_SCHEMA": "ExternalReference.1",
                                "target_url": "raw/b.mp4"
                            }
                        }
                    ]
                }]
            }
        });
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }

    fn write_otio_with_animation_keyframes(
        path: &Path,
        animation_id: &str,
        keyframes: Vec<serde_json::Value>,
    ) {
        let v = serde_json::json!({
            "OTIO_SCHEMA": "Timeline.1",
            "name": "test",
            "metadata": {
                "awidat": {
                    "version": "0.1",
                    "parameter_animations": [{
                        "id": animation_id,
                        "target": {
                            "kind": "clip_parameter",
                            "clip_id": "title-1",
                            "parameter": "title.opacity"
                        },
                        "keyframes": keyframes,
                        "rationale": "Fade in title."
                    }]
                }
            },
            "tracks": {
                "OTIO_SCHEMA": "Stack.1",
                "name": "tracks",
                "children": []
            }
        });
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
    }

    fn write_otio_with_animation(
        path: &Path,
        animation_id: &str,
        second_keyframe_time_s: f64,
        easing: &str,
    ) {
        write_otio_with_animation_keyframes(
            path,
            animation_id,
            vec![
                serde_json::json!({
                    "time_s": 0.0,
                    "value": 0.0,
                    "interpolation": "linear",
                    "easing": easing
                }),
                serde_json::json!({
                    "time_s": second_keyframe_time_s,
                    "value": 1.0,
                    "interpolation": "linear",
                    "easing": "linear"
                }),
            ],
        );
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
        let outcome =
            commit_current_timeline(&repo, "Initial commit", Some("Brand-new project.")).unwrap();
        assert!(outcome.commit_hash.starts_with("sha256:"));
        assert!(outcome.timeline_hash.starts_with("sha256:"));
        assert!(outcome.message.contains("Initial commit"));
        assert!(
            outcome
                .message
                .contains("Agent reasoning: Brand-new project.")
        );
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
        let m = format_commit_message("Trim X by 1.8s", Some("User asked for tighter pacing."));
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
        let session_ref = dir
            .path()
            .join(".vedit")
            .join("refs")
            .join("heads")
            .join(SESSION_START_BRANCH);
        assert!(!session_ref.exists());
    }

    #[test]
    fn ensure_session_tag_points_at_head_after_first_commit() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(&dir.path().join("project.otio.json"), "v1");
        let repo = open_or_init(dir.path()).unwrap();
        let outcome = commit_current_timeline(&repo, "Initial", None).unwrap();
        ensure_session_tag(&repo).unwrap();

        let session_ref = dir
            .path()
            .join(".vedit")
            .join("refs")
            .join("heads")
            .join(SESSION_START_BRANCH);
        let contents = std::fs::read_to_string(&session_ref).unwrap();
        assert!(contents.trim() == outcome.commit_hash, "{contents:?}");
    }

    #[test]
    fn tag_ref_writes_and_lists_named_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(&dir.path().join("project.otio.json"), "v1");
        let repo = open_or_init(dir.path()).unwrap();
        let outcome = commit_current_timeline(&repo, "Initial", None).unwrap();

        let tag = tag_ref(&repo, "client-review-v1", None).unwrap();
        assert_eq!(tag.name, "client-review-v1");
        assert_eq!(tag.target, outcome.commit_hash);

        let tags = list_tags(&repo).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "client-review-v1");
        assert_eq!(tags[0].target, outcome.commit_hash);
    }

    #[test]
    fn create_branch_writes_and_lists_named_alternate() {
        let dir = tempfile::tempdir().unwrap();
        write_minimal_otio(&dir.path().join("project.otio.json"), "v1");
        let repo = open_or_init(dir.path()).unwrap();
        let outcome = commit_current_timeline(&repo, "Initial", None).unwrap();

        let branch = create_branch(&repo, "alt-tight", None).unwrap();
        assert_eq!(branch.name, "alt-tight");
        assert_eq!(branch.target, outcome.commit_hash);
        assert!(!branch.is_current);

        let branches = list_branches(&repo).unwrap();
        assert!(
            branches.iter().any(|branch| branch.name == DEFAULT_BRANCH
                && branch.target == outcome.commit_hash
                && branch.is_current),
            "expected main branch to remain current: {branches:?}"
        );
        assert!(
            branches
                .iter()
                .any(|branch| branch.name == "alt-tight" && branch.target == outcome.commit_hash),
            "expected alt-tight branch in list: {branches:?}"
        );
    }

    #[test]
    fn checkout_branch_switches_head_and_restores_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_minimal_otio(&otio, "v1");
        let repo = open_or_init(dir.path()).unwrap();
        let first = commit_current_timeline(&repo, "Initial", None).unwrap();
        create_branch(&repo, "alt-tight", Some(&first.commit_hash)).unwrap();

        write_minimal_otio(&otio, "v2");
        commit_current_timeline(&repo, "Main update", None).unwrap();

        let checkout = checkout_branch(&repo, "alt-tight").unwrap();
        assert_eq!(checkout.branch, "alt-tight");
        assert_eq!(checkout.commit_hash, first.commit_hash);
        assert_eq!(
            repo.inner.current_branch().unwrap().as_deref(),
            Some("alt-tight")
        );

        let restored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&otio).unwrap()).unwrap();
        assert_eq!(restored["name"].as_str(), Some("v1"));
    }

    #[test]
    fn show_commit_returns_parent_diff() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_otio_with_clip_duration(&otio, "shot-a", "raw/foo.mp4", 240.0);
        let repo = open_or_init(dir.path()).unwrap();
        commit_current_timeline(&repo, "Initial", None).unwrap();

        write_otio_with_clip_duration(&otio, "shot-a", "raw/foo.mp4", 120.0);
        let second =
            commit_current_timeline(&repo, "Trim shot-a", Some("Tighter pacing.")).unwrap();

        let details = show_commit(&repo, &second.commit_hash).unwrap();
        assert_eq!(details.header, "Trim shot-a");
        assert_eq!(details.parents.len(), 1);
        assert!(
            details
                .diff
                .changes
                .iter()
                .any(|change| matches!(change, diff::Change::Trimmed { clip, .. } if clip.name == "shot-a")),
            "expected show diff to include shot-a trim: {:?}",
            details.diff.changes
        );
    }

    #[test]
    fn blame_clip_projects_history_onto_clip() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_otio_with_clip_duration(&otio, "shot-a", "raw/foo.mp4", 240.0);
        let repo = open_or_init(dir.path()).unwrap();
        commit_current_timeline(&repo, "Initial", None).unwrap();

        write_otio_with_clip_duration(&otio, "shot-a", "raw/foo.mp4", 120.0);
        commit_current_timeline(&repo, "Trim shot-a", Some("Tighter pacing.")).unwrap();

        let entries = blame_clip(&repo, "shot-a", None, 20).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].header, "Trim shot-a");
        assert!(
            entries[0]
                .changes
                .iter()
                .any(|change| matches!(change, diff::Change::Trimmed { clip, .. } if clip.name == "shot-a")),
            "expected blame to include shot-a trim: {:?}",
            entries[0].changes
        );
    }

    #[test]
    fn changed_clip_ids_collects_structural_and_animation_targets() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_otio_with_clip_duration(&otio, "shot-a", "raw/foo.mp4", 240.0);
        let repo = open_or_init(dir.path()).unwrap();
        let first = commit_current_timeline(&repo, "Initial", None).unwrap();

        write_otio_with_clip_duration(&otio, "shot-a", "raw/foo.mp4", 120.0);
        let second = commit_current_timeline(&repo, "Trim shot-a", None).unwrap();

        let diff = diff_refs(&repo, Some(&first.commit_hash), Some(&second.commit_hash)).unwrap();
        let ids = changed_clip_ids(&diff);
        assert!(
            ids.contains("shot-a"),
            "expected structural clip id: {ids:?}"
        );
        assert!(
            ids.contains("raw/foo.mp4"),
            "expected structural media reference: {ids:?}"
        );

        write_otio_with_animation(&otio, "title-fade", 0.5, "ease_out");
        let animation_first = commit_current_timeline(&repo, "Initial animation", None).unwrap();
        write_otio_with_animation(&otio, "title-fade", 0.3, "ease_out_expo");
        let animation_second = commit_current_timeline(&repo, "Tighten title fade", None).unwrap();

        let diff = diff_refs(
            &repo,
            Some(&animation_first.commit_hash),
            Some(&animation_second.commit_hash),
        )
        .unwrap();
        let ids = changed_clip_ids(&diff);
        assert!(
            ids.contains("title-1"),
            "expected animation target clip id: {ids:?}"
        );
    }

    #[test]
    fn merge_preflight_allows_non_overlapping_branch_clip_changes() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_otio_with_two_clip_durations(&otio, 240.0, 240.0);
        let repo = open_or_init(dir.path()).unwrap();
        let base = commit_current_timeline(&repo, "Initial", None).unwrap();
        create_branch(&repo, "alt-tight", Some(&base.commit_hash)).unwrap();

        write_otio_with_two_clip_durations(&otio, 240.0, 120.0);
        commit_current_timeline(&repo, "Trim shot-b on main", None).unwrap();

        checkout_branch(&repo, "alt-tight").unwrap();
        write_otio_with_two_clip_durations(&otio, 120.0, 240.0);
        commit_current_timeline(&repo, "Trim shot-a on alternate", None).unwrap();

        let preflight = merge_preflight(&repo, "alt-tight", Some(DEFAULT_BRANCH)).unwrap();

        assert!(preflight.is_mergeable);
        assert_eq!(preflight.merge_base, base.commit_hash);
        assert_eq!(
            preflight.source_changed_clip_ids,
            vec!["raw/a.mp4".to_string(), "shot-a".to_string()]
        );
        assert_eq!(
            preflight.target_changed_clip_ids,
            vec!["raw/b.mp4".to_string(), "shot-b".to_string()]
        );
        assert!(preflight.overlapping_clip_ids.is_empty());
    }

    #[test]
    fn merge_refs_writes_two_parent_commit_for_non_overlapping_clip_changes() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_otio_with_two_clip_durations(&otio, 240.0, 240.0);
        let repo = open_or_init(dir.path()).unwrap();
        let base = commit_current_timeline(&repo, "Initial", None).unwrap();
        create_branch(&repo, "alt-tight", Some(&base.commit_hash)).unwrap();

        write_otio_with_two_clip_durations(&otio, 240.0, 120.0);
        let main = commit_current_timeline(&repo, "Trim shot-b on main", None).unwrap();

        checkout_branch(&repo, "alt-tight").unwrap();
        write_otio_with_two_clip_durations(&otio, 120.0, 240.0);
        let alternate = commit_current_timeline(&repo, "Trim shot-a on alternate", None).unwrap();

        let merge = merge_refs(&repo, "alt-tight", Some(DEFAULT_BRANCH)).unwrap();

        assert_eq!(merge.source_commit, alternate.commit_hash);
        assert_eq!(merge.target_commit, main.commit_hash);
        assert_eq!(merge.parents, vec![main.commit_hash, alternate.commit_hash]);
        assert_eq!(
            repo.inner.current_branch().unwrap().as_deref(),
            Some(DEFAULT_BRANCH)
        );

        let merged_commit = repo.inner.read_commit(&merge.commit_hash).unwrap();
        assert_eq!(merged_commit.parents, merge.parents);

        let merged: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&otio).unwrap()).unwrap();
        let children = merged["tracks"]["children"][0]["children"]
            .as_array()
            .unwrap();
        assert_eq!(children[0]["name"].as_str(), Some("shot-a"));
        assert_eq!(
            children[0]["source_range"]["duration"]["value"].as_f64(),
            Some(120.0)
        );
        assert_eq!(children[1]["name"].as_str(), Some("shot-b"));
        assert_eq!(
            children[1]["source_range"]["duration"]["value"].as_f64(),
            Some(120.0)
        );
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
            diff.changes
                .iter()
                .any(|c| matches!(c, diff::Change::TrackAdded { .. })),
            "expected at least one TrackAdded, got: {:?}",
            diff.changes
        );
    }

    #[test]
    fn diff_refs_reports_animation_timing_and_easing_changes() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_otio_with_animation(&otio, "title-fade", 0.5, "ease_out");
        let repo = open_or_init(dir.path()).unwrap();
        commit_current_timeline(&repo, "Initial animation", None).unwrap();
        ensure_session_tag(&repo).unwrap();

        write_otio_with_animation(&otio, "title-fade", 0.3, "ease_out_expo");
        commit_current_timeline(
            &repo,
            "Tighten title fade",
            Some("Accepted a snappier title fade."),
        )
        .unwrap();

        let diff = diff_refs(&repo, None, None).unwrap();
        assert_eq!(diff.animation_changes.len(), 1);
        let serde_json::Value::Object(change) =
            serde_json::to_value(&diff.animation_changes[0]).unwrap()
        else {
            panic!("animation change should serialize to an object");
        };
        assert_eq!(
            change.get("op").and_then(|value| value.as_str()),
            Some("animation_updated")
        );
        let segment_changes = change
            .get("segment_changes")
            .and_then(|value| value.as_array())
            .unwrap();
        assert!(segment_changes.iter().any(|change| {
            change["field"] == "easing"
                && change["before"] == "ease_out"
                && change["after"] == "ease_out_expo"
        }));
        assert!(segment_changes.iter().any(|change| {
            change["field"] == "duration_s" && change["before"] == 0.5 && change["after"] == 0.3
        }));
    }

    #[test]
    fn diff_refs_reports_inserted_keyframe_by_time_not_index_drift() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_otio_with_animation_keyframes(
            &otio,
            "beat-pulse",
            vec![
                serde_json::json!({"time_s": 0.0, "value": 1.0, "interpolation": "linear", "easing": "linear"}),
                serde_json::json!({"time_s": 1.0, "value": 1.0, "interpolation": "linear", "easing": "linear"}),
            ],
        );
        let repo = open_or_init(dir.path()).unwrap();
        commit_current_timeline(&repo, "Initial animation", None).unwrap();
        ensure_session_tag(&repo).unwrap();

        write_otio_with_animation_keyframes(
            &otio,
            "beat-pulse",
            vec![
                serde_json::json!({"time_s": 0.0, "value": 1.0, "interpolation": "linear", "easing": "linear"}),
                serde_json::json!({"time_s": 0.5, "value": 1.08, "interpolation": "linear", "easing": "linear"}),
                serde_json::json!({"time_s": 1.0, "value": 1.0, "interpolation": "linear", "easing": "linear"}),
            ],
        );
        commit_current_timeline(&repo, "Insert beat pulse", None).unwrap();

        let diff = diff_refs(&repo, None, None).unwrap();
        let value = serde_json::to_value(&diff.animation_changes[0]).unwrap();
        let keyframe_changes = value["keyframe_changes"].as_array().unwrap();
        assert!(
            keyframe_changes
                .iter()
                .any(|change| { change["op"] == "added" && change["time_s"] == 0.5 })
        );
        assert!(
            !keyframe_changes
                .iter()
                .any(|change| { change["op"] == "modified" && change["time_s"] == 1.0 })
        );
    }

    #[test]
    fn diff_refs_reports_spring_params_at_segment_level() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_otio_with_animation_keyframes(
            &otio,
            "settle",
            vec![
                serde_json::json!({"time_s": 0.0, "value": 1.0, "interpolation": "linear", "easing": "linear"}),
                serde_json::json!({"time_s": 0.4, "value": 1.1, "interpolation": "linear", "easing": "linear"}),
            ],
        );
        let repo = open_or_init(dir.path()).unwrap();
        commit_current_timeline(&repo, "Initial animation", None).unwrap();
        ensure_session_tag(&repo).unwrap();

        write_otio_with_animation_keyframes(
            &otio,
            "settle",
            vec![
                serde_json::json!({"time_s": 0.0, "value": 1.0, "interpolation": "spring", "easing": "linear", "spring": {"mass": 1.0, "stiffness": 180.0, "damping": 18.0}}),
                serde_json::json!({"time_s": 0.4, "value": 1.1, "interpolation": "linear", "easing": "linear"}),
            ],
        );
        commit_current_timeline(&repo, "Spring settle", None).unwrap();

        let diff = diff_refs(&repo, None, None).unwrap();
        let value = serde_json::to_value(&diff.animation_changes[0]).unwrap();
        let segment_changes = value["segment_changes"].as_array().unwrap();
        assert!(segment_changes.iter().any(|change| {
            change["field"] == "spring_params"
                && change["after"]["stiffness"] == 180.0
                && change["after"]["damping"] == 18.0
        }));
    }

    #[test]
    fn diff_refs_summary_lists_all_changed_easing_segments() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_otio_with_animation_keyframes(
            &otio,
            "beat-pulse",
            vec![
                serde_json::json!({"time_s": 0.0, "value": 1.0, "interpolation": "linear", "easing": "ease_out"}),
                serde_json::json!({"time_s": 0.2, "value": 1.1, "interpolation": "linear", "easing": "ease_out"}),
                serde_json::json!({"time_s": 0.5, "value": 1.0, "interpolation": "linear", "easing": "ease_out"}),
                serde_json::json!({"time_s": 0.7, "value": 1.1, "interpolation": "linear", "easing": "linear"}),
            ],
        );
        let repo = open_or_init(dir.path()).unwrap();
        commit_current_timeline(&repo, "Initial animation", None).unwrap();
        ensure_session_tag(&repo).unwrap();

        write_otio_with_animation_keyframes(
            &otio,
            "beat-pulse",
            vec![
                serde_json::json!({"time_s": 0.0, "value": 1.0, "interpolation": "linear", "easing": "ease_out_expo"}),
                serde_json::json!({"time_s": 0.2, "value": 1.1, "interpolation": "linear", "easing": "ease_out_expo"}),
                serde_json::json!({"time_s": 0.5, "value": 1.0, "interpolation": "linear", "easing": "ease_out_expo"}),
                serde_json::json!({"time_s": 0.7, "value": 1.1, "interpolation": "linear", "easing": "linear"}),
            ],
        );
        commit_current_timeline(&repo, "Sharpen pulse easing", None).unwrap();

        let diff = diff_refs(&repo, None, None).unwrap();
        let value = serde_json::to_value(&diff.animation_changes[0]).unwrap();
        let summary = value["summary"].as_str().unwrap();
        assert!(summary.contains("changed easing on segments 0, 1, 2"));
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
        assert!(
            entries[1].action_metadata.is_none(),
            "legacy vedit messages should remain readable without action metadata"
        );
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
    fn restore_working_timeline_writes_committed_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let otio = dir.path().join("project.otio.json");
        write_minimal_otio(&otio, "v1");

        let repo = open_or_init(dir.path()).unwrap();
        let first = commit_current_timeline(&repo, "v1", None).unwrap();
        write_minimal_otio(&otio, "v2");
        commit_current_timeline(&repo, "v2", None).unwrap();

        let restored = restore_working_timeline(&repo, &first.commit_hash).unwrap();
        assert_eq!(restored.requested_ref, first.commit_hash);
        assert_eq!(restored.commit_hash, first.commit_hash);
        assert_eq!(restored.timeline_hash, first.timeline_hash);

        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&otio).unwrap()).unwrap();
        assert_eq!(value["name"].as_str(), Some("v1"));
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
        assert_eq!(
            h,
            "Trim drone_shot -1.8s; Insert BRoll over skyline reference"
        );
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
        assert!(
            outcome
                .message
                .starts_with("Trim clip-0 by 1.8s; Insert BRoll over c-skyline")
        );
        assert!(
            outcome
                .message
                .contains("Agent reasoning: User asked for tighter pacing")
        );
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

    // ---- author-resolver priority chain --------------------------------
    // Exercises `resolve_commit_author_with_env` directly because Rust
    // 2024's `std::env::set_var` is `unsafe` and the workspace forbids
    // unsafe code. Driving the env via a callback keeps the test
    // deterministic without touching process-global state.

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'static {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn resolver_prefers_explicit_override_over_env_and_default() {
        let env = env_from(&[(ENV_USER_NAME, "Bob"), (ENV_USER_EMAIL, "bob@example.com")]);
        let carol = CommitAuthor {
            name: "Carol".to_string(),
            email: "carol@example.com".to_string(),
        };
        let resolved = resolve_commit_author_with_env(Some(carol), env);
        assert_eq!(resolved.name, "Carol");
        assert_eq!(resolved.email, "carol@example.com");
    }

    #[test]
    fn resolver_uses_env_vars_when_no_override_present() {
        let env = env_from(&[(ENV_USER_NAME, "Bob"), (ENV_USER_EMAIL, "bob@example.com")]);
        let resolved = resolve_commit_author_with_env(None, env);
        assert_eq!(resolved.name, "Bob");
        assert_eq!(resolved.email, "bob@example.com");
    }

    #[test]
    fn resolver_falls_back_to_default_when_env_partial_or_missing() {
        // Both unset -> default.
        let none_env = |_: &str| None::<String>;
        let resolved = resolve_commit_author_with_env(None, none_env);
        assert_eq!(resolved.name, "awidat agent");
        assert_eq!(resolved.email, "agent@awidat.local");

        // Only the name set -> still default; we refuse to invent an
        // email or pair a real name with a missing one.
        let half_env = env_from(&[(ENV_USER_NAME, "Bob")]);
        let resolved = resolve_commit_author_with_env(None, half_env);
        assert_eq!(resolved.name, "awidat agent");
        assert_eq!(resolved.email, "agent@awidat.local");

        // Whitespace-only env values -> treat as unset.
        let ws_env = env_from(&[(ENV_USER_NAME, "   "), (ENV_USER_EMAIL, "  ")]);
        let resolved = resolve_commit_author_with_env(None, ws_env);
        assert_eq!(resolved.name, "awidat agent");
        assert_eq!(resolved.email, "agent@awidat.local");
    }

    #[test]
    fn resolver_trims_env_values() {
        let env = env_from(&[
            (ENV_USER_NAME, "  Bob  "),
            (ENV_USER_EMAIL, "\tbob@example.com\n"),
        ]);
        let resolved = resolve_commit_author_with_env(None, env);
        assert_eq!(resolved.name, "Bob");
        assert_eq!(resolved.email, "bob@example.com");
    }
}
