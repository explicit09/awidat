//! Per-project rejection feedback log (Wave 5 C2).
//!
//! When a user rejects a Brief proposal (cut / broll / audio / …) we
//! persist a structured record under
//! `<project>/.awidat/feedback.jsonl`. The localStorage History store
//! remains the UI source of truth; this file is the AGENT-facing source
//! — Wave 5 C3 will inject recent entries into the next agent turn so
//! the feedback loop stops being silent.
//!
//! # Format
//!
//! JSONL (one JSON object per line):
//!
//! ```jsonl
//! {"ts":1717123456,"medium":"cut","title":"Trim 0:12 — 0:12.42","rationale":"Silence","reason":"Too aggressive"}
//! {"ts":1717123478,"medium":"broll","title":"Insert B-roll 0:14","rationale":null,"reason":null}
//! ```
//!
//! - `ts` — unix seconds; backend can sort/filter on it.
//! - `medium`, `title`, `ts` — always present.
//! - `rationale`, `reason` — nullable. Silent rejects log
//!   `reason: null` so we can still see frequency patterns.
//!
//! # Append vs. read
//!
//! Writes are append-only — a crash mid-write loses at most one entry,
//! never corrupts earlier lines. Reads parse all lines, drop unparsable
//! ones (forward-compat for future field bumps), sort newest-first, and
//! cap at `MAX_ENTRIES`. If the on-disk file exceeds the cap, we rewrite
//! it to the most recent N atomically (tempfile + rename in the same
//! directory so the rename is a filesystem-level swap).
//!
//! # Path validation
//!
//! `project_path` must be absolute and a directory. Same guards as
//! `agents_md.rs` — defends against accidental relative-path callers
//! that would silently resolve against the desktop process's cwd.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Hard cap on retained entries per project. Mirrors the History
/// store's per-project LRU intent (newer entries supplant older ones)
/// but lives on disk so the backend can read without going through the
/// browser. Picked at 200 to give the agent enough recency to spot
/// patterns ("the user has rejected the last 5 cut proposals") without
/// blowing up the file.
pub const MAX_ENTRIES: usize = 200;

const DIR_NAME: &str = ".awidat";
const FILE_NAME: &str = "feedback.jsonl";

/// One row in the JSONL log. Mirrors the fields a downstream agent
/// turn needs to summarise the user's recent rejections.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FeedbackEntry {
    /// Unix epoch seconds.
    pub ts: i64,
    /// Brief medium bucket — `cut`, `broll`, `audio`, `color`, …
    pub medium: String,
    /// One-line summary the user saw on the Brief row.
    pub title: String,
    /// Agent's "why" for the proposal. `null` when the original
    /// proposal omitted it.
    #[serde(default)]
    pub rationale: Option<String>,
    /// Reason the user supplied at reject time. `null` when the user
    /// rejected without picking / typing one — we still log the row so
    /// frequency analysis (silent-reject rate per medium) stays
    /// possible.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Append one entry to `<project>/.awidat/feedback.jsonl`. Creates the
/// `.awidat/` directory and the file when missing. Returns once the
/// bytes are flushed to disk.
///
/// We use `OpenOptions { append: true, create: true }` so concurrent
/// writers (in practice: a fast double-click on reject) get serialised
/// at the OS level — each `write_all` for our short line stays
/// atomic on POSIX. JSONL is recovery-friendly: a writer that crashes
/// mid-line leaves a partial line that the reader silently discards.
#[tauri::command]
pub async fn append_feedback(project_path: String, entry: FeedbackEntry) -> Result<(), String> {
    let root = validate_project_root(&project_path)?;
    let dir = root.join(DIR_NAME);
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("create {DIR_NAME}/: {e}"))?;
    let file_path = dir.join(FILE_NAME);

    let line = serialize_line(&entry)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
        .await
        .map_err(|e| format!("open {FILE_NAME}: {e}"))?;
    file.write_all(line.as_bytes())
        .await
        .map_err(|e| format!("write {FILE_NAME}: {e}"))?;
    file.flush()
        .await
        .map_err(|e| format!("flush {FILE_NAME}: {e}"))?;
    Ok(())
}

/// Read the last `limit` entries (capped at `MAX_ENTRIES`) newest
/// first. Missing file → empty Vec; the agent should treat "no log
/// yet" as "no signal", never as an error.
///
/// When the on-disk file exceeds `MAX_ENTRIES` we rewrite it to the
/// most recent `MAX_ENTRIES` atomically — a tempfile in the same
/// directory followed by `rename`. That keeps the log self-pruning
/// without a separate compactor.
#[tauri::command]
pub async fn read_feedback(
    project_path: String,
    limit: usize,
) -> Result<Vec<FeedbackEntry>, String> {
    let root = validate_project_root(&project_path)?;
    let dir = root.join(DIR_NAME);
    let file_path = dir.join(FILE_NAME);

    let raw = match fs::read_to_string(&file_path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read {FILE_NAME}: {e}")),
    };

    let parsed = parse_jsonl(&raw);
    let needs_truncation = parsed.len() > MAX_ENTRIES;
    let truncated = if needs_truncation {
        // Keep the most-recent MAX_ENTRIES (the *tail* of the
        // append-only file). Rewrite the file before returning so the
        // log self-prunes on every read.
        let start = parsed.len() - MAX_ENTRIES;
        parsed[start..].to_vec()
    } else {
        parsed
    };

    if needs_truncation {
        rewrite_atomically(&dir, &file_path, &truncated).await?;
    }

    // Newest-first slice, bounded by the caller's limit (and MAX_ENTRIES).
    let effective_limit = limit.min(MAX_ENTRIES);
    let mut newest_first: Vec<FeedbackEntry> = truncated.into_iter().rev().collect();
    if newest_first.len() > effective_limit {
        newest_first.truncate(effective_limit);
    }
    Ok(newest_first)
}

/// Synchronous read of the most recent `limit` entries (capped at
/// [`MAX_ENTRIES`]) newest-first. Used by [`crate::codex_session`] at
/// session-launch time so the agent's L1 context can include a "Recent
/// feedback" fragment without spinning up a separate async task.
///
/// Mirrors the read side of [`read_feedback`] but stays sync, returns
/// an empty Vec on any I/O / parse trouble (a missing log isn't an
/// error here either — the agent should treat "no log yet" as "no
/// signal"), and skips the self-pruning rewrite — that's a write
/// concern owned by the async tauri command path.
///
/// `project_root` must be absolute; we don't re-validate here because
/// the caller (`codex_session::launch`) already has the validated
/// `project_root` it constructed the bridge with.
pub fn load_recent_feedback_sync(project_root: &Path, limit: usize) -> Vec<FeedbackEntry> {
    let file_path = project_root.join(DIR_NAME).join(FILE_NAME);
    let raw = match std::fs::read_to_string(&file_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!(
                path = %file_path.display(),
                error = %err,
                "feedback.jsonl read failed; treating as empty"
            );
            return Vec::new();
        }
    };
    let parsed = parse_jsonl(&raw);
    // Cap at MAX_ENTRIES first (keeps the slice cheap when the log has
    // grown past the on-disk cap before the async path got a chance to
    // self-prune), then bound by the caller's limit.
    let effective_limit = limit.min(MAX_ENTRIES);
    let mut newest_first: Vec<FeedbackEntry> = parsed.into_iter().rev().collect();
    if newest_first.len() > effective_limit {
        newest_first.truncate(effective_limit);
    }
    newest_first
}

/// Serialise an entry to a single JSONL line (trailing newline
/// included). Splitting this out keeps the append path testable
/// without filesystem setup and makes the "one entry = one line"
/// invariant explicit.
fn serialize_line(entry: &FeedbackEntry) -> Result<String, String> {
    let mut line =
        serde_json::to_string(entry).map_err(|e| format!("serialize feedback entry: {e}"))?;
    // Reject any entry whose serialised form already contains a newline
    // — JSONL's contract is "one object per line", and a stray \n in a
    // title or reason would split the entry across lines on read. In
    // practice `serde_json::to_string` escapes \n inside string
    // fields, but the guard is cheap and the failure mode (silent
    // corruption) is worth defending.
    if line.contains('\n') {
        return Err("feedback entry contains literal newline after serialisation".into());
    }
    line.push('\n');
    Ok(line)
}

/// Parse a JSONL blob, dropping unparsable / blank lines.
///
/// Forward-compat: a future schema bump that adds a required field
/// will fail the parse on old rows; we silently drop them rather than
/// erroring the whole read, so the agent sees the parseable subset
/// instead of nothing.
fn parse_jsonl(raw: &str) -> Vec<FeedbackEntry> {
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<FeedbackEntry>(line).ok())
        .collect()
}

/// Rewrite the log file to the supplied entries via tempfile + rename
/// in the same directory. POSIX `rename` is atomic for same-filesystem
/// moves so a crash mid-rewrite leaves either the old file or the new
/// file — never a half-written one.
async fn rewrite_atomically(
    dir: &Path,
    final_path: &Path,
    entries: &[FeedbackEntry],
) -> Result<(), String> {
    // Build the full new contents up-front so the temp file is written
    // in a single syscall (cheap for ≤200 small JSON objects).
    let mut buf = String::with_capacity(entries.len() * 96);
    for e in entries {
        let line = serialize_line(e)?;
        buf.push_str(&line);
    }

    let tmp_path = dir.join(format!("{FILE_NAME}.tmp"));
    {
        // Scope the file handle so it's closed before we rename. On
        // Windows a still-open handle would block the rename; macOS /
        // Linux tolerate it but explicit drop is clearer.
        let mut tmp = fs::File::create(&tmp_path)
            .await
            .map_err(|e| format!("create {FILE_NAME}.tmp: {e}"))?;
        tmp.write_all(buf.as_bytes())
            .await
            .map_err(|e| format!("write {FILE_NAME}.tmp: {e}"))?;
        tmp.flush()
            .await
            .map_err(|e| format!("flush {FILE_NAME}.tmp: {e}"))?;
    }
    fs::rename(&tmp_path, final_path)
        .await
        .map_err(|e| format!("rename {FILE_NAME}.tmp → {FILE_NAME}: {e}"))?;
    Ok(())
}

/// Same guard as `agents_md.rs::validate_project_root`. Refuse
/// relative paths and non-directories so a misconfigured caller can't
/// silently write into the desktop process's cwd.
fn validate_project_root(project_path: &str) -> Result<PathBuf, String> {
    if project_path.is_empty() {
        return Err("project_path is empty".into());
    }
    let buf = PathBuf::from(project_path);
    if !buf.is_absolute() {
        return Err(format!("project_path must be absolute: {project_path}"));
    }
    if !Path::new(&buf).is_dir() {
        return Err(format!("not a directory: {project_path}"));
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(ts: i64, medium: &str, title: &str, reason: Option<&str>) -> FeedbackEntry {
        FeedbackEntry {
            ts,
            medium: medium.into(),
            title: title.into(),
            rationale: Some("because".into()),
            reason: reason.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn append_creates_dir_and_file_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let entry = mk(1, "cut", "Trim 0:12", Some("Too aggressive"));
        append_feedback(tmp.path().to_string_lossy().into_owned(), entry.clone())
            .await
            .unwrap();
        let file = tmp.path().join(DIR_NAME).join(FILE_NAME);
        assert!(file.is_file(), "feedback.jsonl was not created");
        let on_disk = std::fs::read_to_string(&file).unwrap();
        // Exactly one line, trailing newline.
        assert_eq!(on_disk.lines().count(), 1);
        assert!(on_disk.ends_with('\n'));
        // Round-trip.
        let parsed: FeedbackEntry = serde_json::from_str(on_disk.trim()).unwrap();
        assert_eq!(parsed, entry);
    }

    #[tokio::test]
    async fn append_is_additive_across_restarts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_string_lossy().into_owned();
        append_feedback(path.clone(), mk(1, "cut", "a", Some("r1")))
            .await
            .unwrap();
        // "Restart" — function-scoped state only; we just call again.
        append_feedback(path.clone(), mk(2, "broll", "b", None))
            .await
            .unwrap();
        append_feedback(path.clone(), mk(3, "audio", "c", Some("r3")))
            .await
            .unwrap();
        let file = tmp.path().join(DIR_NAME).join(FILE_NAME);
        let on_disk = std::fs::read_to_string(&file).unwrap();
        assert_eq!(on_disk.lines().count(), 3, "{on_disk}");
        // Lines parse independently.
        for line in on_disk.lines() {
            serde_json::from_str::<FeedbackEntry>(line).unwrap();
        }
    }

    #[tokio::test]
    async fn read_returns_empty_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let out = read_feedback(tmp.path().to_string_lossy().into_owned(), 50)
            .await
            .unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn read_returns_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_string_lossy().into_owned();
        // Append in oldest→newest order.
        append_feedback(path.clone(), mk(100, "cut", "first", None))
            .await
            .unwrap();
        append_feedback(path.clone(), mk(200, "broll", "second", None))
            .await
            .unwrap();
        append_feedback(path.clone(), mk(300, "audio", "third", None))
            .await
            .unwrap();
        let out = read_feedback(path, 50).await.unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].ts, 300);
        assert_eq!(out[1].ts, 200);
        assert_eq!(out[2].ts, 100);
    }

    #[tokio::test]
    async fn read_honours_limit_bound() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_string_lossy().into_owned();
        for i in 0..10 {
            append_feedback(path.clone(), mk(i, "cut", "x", None))
                .await
                .unwrap();
        }
        let out = read_feedback(path, 3).await.unwrap();
        assert_eq!(out.len(), 3);
        // Newest is ts=9, then 8, then 7.
        assert_eq!(out[0].ts, 9);
        assert_eq!(out[1].ts, 8);
        assert_eq!(out[2].ts, 7);
    }

    #[tokio::test]
    async fn read_caps_at_max_entries_and_truncates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_string_lossy().into_owned();
        // 201 entries — one over the cap.
        let total = MAX_ENTRIES + 1;
        for i in 0..(total as i64) {
            append_feedback(path.clone(), mk(i, "cut", "x", None))
                .await
                .unwrap();
        }
        // Asking for far more than the cap still returns at most MAX_ENTRIES.
        let out = read_feedback(path.clone(), 10_000).await.unwrap();
        assert_eq!(out.len(), MAX_ENTRIES);
        // Newest entry is the highest ts (total - 1).
        assert_eq!(out[0].ts, (total - 1) as i64);
        // Oldest entry in the returned slice is `total - MAX_ENTRIES`.
        assert_eq!(out[MAX_ENTRIES - 1].ts, (total - MAX_ENTRIES) as i64,);

        // On-disk file was rewritten to the kept MAX_ENTRIES (atomic
        // truncation). Verify by reading bytes and counting lines.
        let file = tmp.path().join(DIR_NAME).join(FILE_NAME);
        let on_disk = std::fs::read_to_string(&file).unwrap();
        assert_eq!(on_disk.lines().count(), MAX_ENTRIES);
        // Tempfile was cleaned up.
        let tmp_path = tmp.path().join(DIR_NAME).join(format!("{FILE_NAME}.tmp"));
        assert!(!tmp_path.exists(), "tempfile leaked after rewrite");

        // Subsequent appends continue from the trimmed state — no
        // accidental hole, no double-write.
        append_feedback(path.clone(), mk(9999, "broll", "after", None))
            .await
            .unwrap();
        let out2 = read_feedback(path, MAX_ENTRIES).await.unwrap();
        assert_eq!(out2.len(), MAX_ENTRIES);
        assert_eq!(
            out2[0].ts, 9999,
            "new append landed on top after truncation"
        );
    }

    #[tokio::test]
    async fn read_skips_blank_and_unparsable_lines() {
        // Hand-craft a JSONL file with one valid line, one blank, one
        // garbage. The reader must drop the garbage and return the one
        // valid row.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(DIR_NAME);
        std::fs::create_dir_all(&dir).unwrap();
        let good = serde_json::to_string(&mk(42, "cut", "ok", Some("r"))).unwrap();
        let contents = format!("{good}\n\n{{not json\n");
        std::fs::write(dir.join(FILE_NAME), contents).unwrap();

        let out = read_feedback(tmp.path().to_string_lossy().into_owned(), 50)
            .await
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].ts, 42);
    }

    #[tokio::test]
    async fn null_rationale_and_reason_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_string_lossy().into_owned();
        let entry = FeedbackEntry {
            ts: 5,
            medium: "cut".into(),
            title: "silent reject".into(),
            rationale: None,
            reason: None,
        };
        append_feedback(path.clone(), entry.clone()).await.unwrap();
        let out = read_feedback(path, 10).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], entry);
        // And the on-disk shape uses explicit nulls (so the agent can
        // distinguish "no reason" from "missing field").
        let on_disk = std::fs::read_to_string(tmp.path().join(DIR_NAME).join(FILE_NAME)).unwrap();
        assert!(on_disk.contains("\"rationale\":null"), "{on_disk}");
        assert!(on_disk.contains("\"reason\":null"), "{on_disk}");
    }

    #[tokio::test]
    async fn append_refuses_relative_path() {
        let err = append_feedback("relative/path".into(), mk(1, "cut", "x", None))
            .await
            .unwrap_err();
        assert!(err.contains("must be absolute"), "got: {err}");
    }

    #[tokio::test]
    async fn read_refuses_empty_path() {
        let err = read_feedback(String::new(), 10).await.unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn serialize_line_rejects_embedded_newlines() {
        // serde_json escapes \n inside strings, so the on-the-wire form
        // never contains a literal newline. This test pins that
        // invariant — if a future change swaps in a serialiser that
        // does emit raw newlines, we want to know.
        let entry = FeedbackEntry {
            ts: 1,
            medium: "cut".into(),
            title: "first line\nsecond line".into(),
            rationale: None,
            reason: None,
        };
        let line = serialize_line(&entry).unwrap();
        // Exactly one trailing newline; the embedded \n is escaped to \\n.
        assert_eq!(line.matches('\n').count(), 1);
        assert!(line.ends_with('\n'));
        assert!(line.contains("\\n"), "{line}");
    }

    // ---- Wave 5 C3 — sync read helper for codex_session ----

    #[test]
    fn load_recent_feedback_sync_returns_empty_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let out = load_recent_feedback_sync(tmp.path(), 10);
        assert!(out.is_empty());
    }

    #[test]
    fn load_recent_feedback_sync_returns_empty_when_dir_missing() {
        // No `.awidat/` subdir at all — the missing-file branch fires.
        let tmp = tempfile::tempdir().unwrap();
        let out = load_recent_feedback_sync(tmp.path(), 5);
        assert!(out.is_empty());
    }

    #[test]
    fn load_recent_feedback_sync_returns_newest_first() {
        // Hand-write the JSONL file so we don't depend on the async
        // append path here.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(DIR_NAME);
        std::fs::create_dir_all(&dir).unwrap();
        let rows = [
            mk(100, "cut", "first", None),
            mk(200, "broll", "second", None),
            mk(300, "audio", "third", None),
        ];
        let buf = rows
            .iter()
            .map(|e| serialize_line(e).unwrap())
            .collect::<String>();
        std::fs::write(dir.join(FILE_NAME), buf).unwrap();
        let out = load_recent_feedback_sync(tmp.path(), 10);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].ts, 300);
        assert_eq!(out[1].ts, 200);
        assert_eq!(out[2].ts, 100);
    }

    #[test]
    fn load_recent_feedback_sync_honours_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(DIR_NAME);
        std::fs::create_dir_all(&dir).unwrap();
        let buf = (0..10)
            .map(|i| serialize_line(&mk(i, "cut", "x", None)).unwrap())
            .collect::<String>();
        std::fs::write(dir.join(FILE_NAME), buf).unwrap();

        let out = load_recent_feedback_sync(tmp.path(), 3);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].ts, 9);
        assert_eq!(out[1].ts, 8);
        assert_eq!(out[2].ts, 7);
    }

    #[test]
    fn load_recent_feedback_sync_caps_at_max_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(DIR_NAME);
        std::fs::create_dir_all(&dir).unwrap();
        // Pre-cap the file just above the limit so we exercise the
        // MAX_ENTRIES cap regardless of what `limit` the caller asks
        // for. (Sync path does NOT rewrite the file — it just bounds
        // its slice.)
        let total = MAX_ENTRIES + 5;
        let buf = (0..(total as i64))
            .map(|i| serialize_line(&mk(i, "cut", "x", None)).unwrap())
            .collect::<String>();
        std::fs::write(dir.join(FILE_NAME), buf).unwrap();

        // Caller asks for way more than MAX_ENTRIES; sync still bounds.
        let out = load_recent_feedback_sync(tmp.path(), 10_000);
        assert_eq!(out.len(), MAX_ENTRIES);
        assert_eq!(out[0].ts, (total - 1) as i64);
    }

    #[test]
    fn load_recent_feedback_sync_skips_garbage_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(DIR_NAME);
        std::fs::create_dir_all(&dir).unwrap();
        let good = serde_json::to_string(&mk(42, "cut", "ok", Some("r"))).unwrap();
        let contents = format!("{good}\n\n{{not json\n");
        std::fs::write(dir.join(FILE_NAME), contents).unwrap();
        let out = load_recent_feedback_sync(tmp.path(), 50);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].ts, 42);
    }

    #[test]
    fn parse_jsonl_is_forward_compat() {
        // A future schema that adds a required field would fail to
        // deserialise old rows; we drop them silently so the agent
        // still sees the parseable subset. Simulate by feeding the
        // parser a row that's missing `medium` (currently required).
        let raw = r#"{"ts":1,"medium":"cut","title":"a"}
{"ts":2,"title":"missing medium"}
{"ts":3,"medium":"broll","title":"b"}
"#;
        let out = parse_jsonl(raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].ts, 1);
        assert_eq!(out[1].ts, 3);
    }
}
