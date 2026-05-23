//! Per-state-root SQLite index over the JSONL session logs.
//!
//! Codex calls this layer their `codex-state` crate. We collapse it
//! into `awidat-core` because the only writers and readers are the
//! `Recorder` (writes new rows; updates message_count + last_active)
//! and the desktop `list_chat_sessions` command (reads).
//!
//! What this exists for:
//!
//! 1. **O(1) listing.** Walking JSONL files + parsing the first
//!    `SessionMeta` line scales linearly with session count. The
//!    desktop chat-history rail was already taking ~200ms to list
//!    20+ sessions; growing.
//!
//! 2. **Status column for corrupt sessions.** When a session is
//!    detected as having an unmatched `TurnStarted` on resume (i.e.
//!    the previous run crashed mid-turn), the registry marks it
//!    `corrupt`. The chat history UI can grey those out instead of
//!    silently swallowing them.
//!
//! 3. **Atomic activity tracking.** Multiple processes / multiple
//!    recorders writing to the same DB are serialized by SQLite's
//!    own locking; consumers don't have to coordinate.
//!
//! Schema lives in this file (one table). Backfill: on first open of
//! a state root that has session JSONLs but no DB, we walk the
//! filesystem once and INSERT rows. Subsequent boots skip the scan.
//!
//! Concurrency model: rusqlite + WAL mode. Reads don't block writes
//! and vice versa. Each call opens a Connection from a pool; we use
//! a Mutex around a single Connection because our write rate is
//! dominated by `record_message` calls (which only bump
//! message_count) and contention is irrelevant in practice.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

/// Lifecycle status for a recorded session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Session is the current recorder; messages still arriving.
    Active,
    /// Recorder shut down cleanly; the log is complete.
    Completed,
    /// Resume detected unmatched TurnStarted / parse errors / missing
    /// SessionMeta. Listing UI should warn / dim the entry.
    Corrupt,
}

impl SessionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Corrupt => "corrupt",
        }
    }
    fn from_str(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "corrupt" => Self::Corrupt,
            _ => Self::Completed,
        }
    }
}

/// One row in the `sessions` table.
#[derive(Debug, Clone)]
pub struct SessionRow {
    /// Session id — matches the `SessionMeta.id` in the JSONL log.
    pub id: String,
    /// Absolute path of the project the session was opened against.
    pub project_root: PathBuf,
    /// Absolute path of the JSONL rollout log on disk.
    pub log_path: PathBuf,
    /// Model name the session was recorded with.
    pub model: String,
    /// When the session was first created.
    pub started_at: DateTime<Utc>,
    /// When this session last received an activity update.
    pub last_active_at: DateTime<Utc>,
    /// Number of `Message` records appended to the JSONL.
    pub message_count: usize,
    /// Lifecycle status. See [`SessionStatus`].
    pub status: SessionStatus,
    /// User-renamed title. `None` falls back to the title derived
    /// from the first user message in the JSONL.
    pub custom_title: Option<String>,
}

/// SQLite-backed session registry. Cheap to clone (internally an Arc).
#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl SessionRegistry {
    /// Open or create the registry at `<state_root>/sessions.db`.
    /// Idempotent — schema is created on first call and skipped
    /// after that.
    pub fn open(state_root: &Path) -> rusqlite::Result<Self> {
        std::fs::create_dir_all(state_root).map_err(|e| {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                Some(format!("create state root {}: {e}", state_root.display())),
            )
        })?;
        let path = state_root.join("sessions.db");
        let conn = Connection::open(&path)?;
        // WAL mode lets readers see committed writes without blocking.
        // synchronous=NORMAL trades a tiny durability window for ~2x
        // write throughput; acceptable for a per-turn metadata
        // bump (the JSONL is the load-bearing record, not this).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS sessions (
                id              TEXT PRIMARY KEY,
                project_root    TEXT NOT NULL,
                log_path        TEXT NOT NULL UNIQUE,
                model           TEXT NOT NULL,
                started_at      TEXT NOT NULL,
                last_active_at  TEXT NOT NULL,
                message_count   INTEGER NOT NULL DEFAULT 0,
                status          TEXT NOT NULL DEFAULT 'active'
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS sessions_project_started_at
             ON sessions(project_root, started_at DESC)",
            [],
        )?;
        // Migration: add custom_title column on existing DBs. SQLite
        // doesn't support `IF NOT EXISTS` for ADD COLUMN, so probe
        // pragma table_info first.
        let has_custom_title = {
            let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            let mut found = false;
            for name in rows.flatten() {
                if name == "custom_title" {
                    found = true;
                    break;
                }
            }
            found
        };
        if !has_custom_title {
            conn.execute("ALTER TABLE sessions ADD COLUMN custom_title TEXT", [])?;
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
            path,
        })
    }

    /// Insert a new session row. Called by `Recorder::create` (and
    /// `create_resumed`) when a fresh JSONL is opened. Status starts
    /// as `Active`; transitions to `Completed` on `Recorder::shutdown`
    /// or to `Corrupt` when resume detects a problem.
    pub fn create_session(
        &self,
        id: &str,
        project_root: &Path,
        log_path: &Path,
        model: &str,
        started_at: DateTime<Utc>,
    ) -> rusqlite::Result<()> {
        let conn = self.inner.lock().expect("registry mutex poisoned");
        conn.execute(
            "INSERT OR REPLACE INTO sessions
             (id, project_root, log_path, model, started_at, last_active_at, message_count, status)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                id,
                project_root.to_string_lossy(),
                log_path.to_string_lossy(),
                model,
                started_at.to_rfc3339(),
                started_at.to_rfc3339(),
                0i64,
                SessionStatus::Active.as_str(),
            ],
        )?;
        Ok(())
    }

    /// Bump `message_count` and `last_active_at`. Called from the
    /// recorder's writer task after each message line is flushed.
    pub fn record_activity(
        &self,
        id: &str,
        message_count_delta: i64,
        at: DateTime<Utc>,
    ) -> rusqlite::Result<()> {
        let conn = self.inner.lock().expect("registry mutex poisoned");
        conn.execute(
            "UPDATE sessions
             SET message_count = message_count + ?, last_active_at = ?
             WHERE id = ?",
            params![message_count_delta, at.to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Mark a session terminal-status. Idempotent.
    pub fn set_status(&self, id: &str, status: SessionStatus) -> rusqlite::Result<()> {
        let conn = self.inner.lock().expect("registry mutex poisoned");
        conn.execute(
            "UPDATE sessions SET status = ? WHERE id = ?",
            params![status.as_str(), id],
        )?;
        Ok(())
    }

    /// List sessions whose `project_root` matches `project_root`,
    /// newest first by `started_at`. Use `None` to list across all
    /// projects (e.g. for diagnostics).
    pub fn list(&self, project_root: Option<&Path>) -> rusqlite::Result<Vec<SessionRow>> {
        let conn = self.inner.lock().expect("registry mutex poisoned");
        let mut stmt;
        let rows = if let Some(pr) = project_root {
            stmt = conn.prepare(
                "SELECT id, project_root, log_path, model, started_at, last_active_at,
                        message_count, status, custom_title
                 FROM sessions
                 WHERE project_root = ?
                 ORDER BY started_at DESC",
            )?;
            stmt.query_map([pr.to_string_lossy().to_string()], row_to_session)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt = conn.prepare(
                "SELECT id, project_root, log_path, model, started_at, last_active_at,
                        message_count, status, custom_title
                 FROM sessions
                 ORDER BY started_at DESC",
            )?;
            stmt.query_map([], row_to_session)?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    /// Look up a single session by id.
    pub fn get(&self, id: &str) -> rusqlite::Result<Option<SessionRow>> {
        let conn = self.inner.lock().expect("registry mutex poisoned");
        conn.query_row(
            "SELECT id, project_root, log_path, model, started_at, last_active_at,
                    message_count, status, custom_title
             FROM sessions WHERE id = ?",
            [id],
            row_to_session,
        )
        .optional()
    }

    /// Look up a session by its on-disk log path. Used by the desktop
    /// rename/delete commands which only receive the path.
    pub fn get_by_log_path(&self, log_path: &Path) -> rusqlite::Result<Option<SessionRow>> {
        let conn = self.inner.lock().expect("registry mutex poisoned");
        conn.query_row(
            "SELECT id, project_root, log_path, model, started_at, last_active_at,
                    message_count, status, custom_title
             FROM sessions WHERE log_path = ?",
            [log_path.to_string_lossy().to_string()],
            row_to_session,
        )
        .optional()
    }

    /// Persist a user-supplied title override for a session. Passing
    /// `None` clears the override and falls back to the derived title.
    pub fn set_custom_title(&self, id: &str, title: Option<&str>) -> rusqlite::Result<()> {
        let conn = self.inner.lock().expect("registry mutex poisoned");
        conn.execute(
            "UPDATE sessions SET custom_title = ? WHERE id = ?",
            params![title, id],
        )?;
        Ok(())
    }

    /// Remove a session row. The caller is responsible for deleting
    /// the corresponding JSONL log on disk; this method only drops
    /// the metadata row.
    pub fn delete_session(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.inner.lock().expect("registry mutex poisoned");
        conn.execute("DELETE FROM sessions WHERE id = ?", [id])?;
        Ok(())
    }

    /// On-disk path of the SQLite file. Useful for diagnostics.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    let id: String = row.get(0)?;
    let project_root: String = row.get(1)?;
    let log_path: String = row.get(2)?;
    let model: String = row.get(3)?;
    let started_at: String = row.get(4)?;
    let last_active_at: String = row.get(5)?;
    let message_count: i64 = row.get(6)?;
    let status: String = row.get(7)?;
    let custom_title: Option<String> = row.get(8).ok();
    Ok(SessionRow {
        id,
        project_root: PathBuf::from(project_root),
        log_path: PathBuf::from(log_path),
        model,
        started_at: DateTime::parse_from_rfc3339(&started_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        last_active_at: DateTime::parse_from_rfc3339(&last_active_at)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        message_count: message_count.max(0) as usize,
        status: SessionStatus::from_str(&status),
        custom_title,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_creates_schema() {
        let dir = tempfile::tempdir().unwrap();
        let reg = SessionRegistry::open(dir.path()).unwrap();
        assert!(reg.path().exists());
        // List on empty DB returns empty.
        let rows = reg.list(None).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn create_session_and_list_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let reg = SessionRegistry::open(dir.path()).unwrap();
        let when = Utc::now();
        reg.create_session(
            "id1",
            Path::new("/projects/A"),
            Path::new("/state/A/log1.jsonl"),
            "claude-opus-4-7",
            when,
        )
        .unwrap();
        reg.create_session(
            "id2",
            Path::new("/projects/A"),
            Path::new("/state/A/log2.jsonl"),
            "claude-opus-4-7",
            when + chrono::Duration::seconds(10),
        )
        .unwrap();
        reg.create_session(
            "id3",
            Path::new("/projects/B"),
            Path::new("/state/B/log1.jsonl"),
            "claude-opus-4-7",
            when + chrono::Duration::seconds(20),
        )
        .unwrap();
        // Filter by project A — newest first.
        let rows = reg.list(Some(Path::new("/projects/A"))).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "id2");
        assert_eq!(rows[1].id, "id1");
        // No filter — all three.
        let rows = reg.list(None).unwrap();
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn record_activity_bumps_message_count() {
        let dir = tempfile::tempdir().unwrap();
        let reg = SessionRegistry::open(dir.path()).unwrap();
        reg.create_session(
            "id1",
            Path::new("/p"),
            Path::new("/p/log.jsonl"),
            "m",
            Utc::now(),
        )
        .unwrap();
        reg.record_activity("id1", 1, Utc::now()).unwrap();
        reg.record_activity("id1", 2, Utc::now()).unwrap();
        let row = reg.get("id1").unwrap().unwrap();
        assert_eq!(row.message_count, 3);
    }

    #[test]
    fn set_status_transitions() {
        let dir = tempfile::tempdir().unwrap();
        let reg = SessionRegistry::open(dir.path()).unwrap();
        reg.create_session(
            "id1",
            Path::new("/p"),
            Path::new("/p/log.jsonl"),
            "m",
            Utc::now(),
        )
        .unwrap();
        let row = reg.get("id1").unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Active);
        reg.set_status("id1", SessionStatus::Completed).unwrap();
        let row = reg.get("id1").unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Completed);
        reg.set_status("id1", SessionStatus::Corrupt).unwrap();
        let row = reg.get("id1").unwrap().unwrap();
        assert_eq!(row.status, SessionStatus::Corrupt);
    }
}
