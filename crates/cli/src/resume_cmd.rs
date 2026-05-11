//! `awidat resume` — list or replay prior sessions.
//!
//! Lists rollout JSONL logs under `state_root()/sessions/`, newest
//! first. With a `<selector>` (id, list-index, or path), opens the TUI
//! with the prior history pre-replayed. The continuation gets a
//! *fresh* JSONL file with `resumed_from = <prior id>` stamped — the
//! original log is never modified, so the user can resume the same
//! session multiple times (each becomes a separate branch).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use awidat_core::Session;
use awidat_core::anthropic::{Client, ClientConfig};
use awidat_core::rollout::{Recorder, SessionMeta};
use tokio::sync::mpsc;

pub fn run(selector: Option<&str>, model_override: Option<&str>) -> Result<()> {
    let state_root = awidat_config::defaults::state_root()
        .ok_or_else(|| anyhow!("could not resolve state root (no $HOME?)"))?;

    let entries = Recorder::list(&state_root)
        .with_context(|| format!("failed to list sessions under {}", state_root.display()))?;

    match selector {
        None => {
            if entries.is_empty() {
                println!(
                    "No sessions found under {}.",
                    state_root.join("sessions").display()
                );
                println!("Sessions are recorded automatically when you run `awidat tui`.");
            } else {
                print_listing(&entries);
            }
            Ok(())
        }
        // `pick` may resolve a session outside the state root, so we
        // call it even when `entries` is empty.
        Some(sel) => {
            let (path, meta) = pick(&entries, sel)?;
            resume_into_tui(path, meta, model_override)
        }
    }
}

fn print_listing(entries: &[(PathBuf, SessionMeta)]) {
    println!("Recent sessions ({}):\n", entries.len().min(20));
    for (i, (path, meta)) in entries.iter().take(20).enumerate() {
        let when = meta.started_at.format("%Y-%m-%d %H:%M:%S");
        let proj = meta.project_root.display();
        println!("  [{:>2}] {}  id={}  proj={}", i + 1, when, &meta.id, proj);
        println!("       {}", path.display());
    }
    println!("\nResume:  awidat resume <list-index|id|path>");
}

fn pick(entries: &[(PathBuf, SessionMeta)], sel: &str) -> Result<(PathBuf, SessionMeta)> {
    // Path mode: re-parse to get meta even if the file isn't under state_root.
    let as_path = PathBuf::from(sel);
    if as_path.is_file() {
        let (m, _hist) = Recorder::resume(&as_path)
            .with_context(|| format!("failed to read {}", as_path.display()))?;
        return Ok((as_path, m));
    }
    // List index (1-based).
    if let Ok(idx) = sel.parse::<usize>()
        && idx >= 1
        && idx <= entries.len()
    {
        let (p, m) = &entries[idx - 1];
        return Ok((p.clone(), m.clone()));
    }
    // Session id (exact or unique prefix).
    let matches: Vec<_> = entries
        .iter()
        .filter(|(_, m)| m.id == sel || m.id.starts_with(sel))
        .collect();
    match matches.len() {
        0 => Err(anyhow!(
            "no session matches '{sel}'. Use `awidat resume` (no args) to list."
        )),
        1 => {
            let (p, m) = matches[0];
            Ok((p.clone(), m.clone()))
        }
        n => Err(anyhow!(
            "'{sel}' is ambiguous — matches {n} sessions. Use a longer prefix or the full id."
        )),
    }
}

fn resume_into_tui(
    log_path: PathBuf,
    meta: SessionMeta,
    model_override: Option<&str>,
) -> Result<()> {
    let project_root = meta.project_root.clone();
    if !project_root.is_dir() {
        return Err(anyhow!(
            "project root '{}' from session no longer exists. Was it deleted?",
            project_root.display()
        ));
    }
    let state_root = awidat_config::defaults::state_root()
        .ok_or_else(|| anyhow!("could not resolve state root"))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;

    if model_override.is_some() {
        eprintln!(
            "note: --model is ignored on resume — the original session's \
             model is used to keep the conversation coherent."
        );
    }

    let log_path_for_async = log_path.clone();
    let session = runtime.block_on(async move {
        let client = Client::from_env_or_keychain(ClientConfig::default()).map_err(|e| {
            anyhow!(
                "failed to build Anthropic client: {e}. Set ANTHROPIC_API_KEY or store \
                 the key in your OS keychain (service 'awidat', account 'anthropic_api_key')."
            )
        })?;
        let registry = crate::tui_cmd::build_full_registry(&meta.model);
        let (approval_tx, approval_rx) = mpsc::channel(8);
        let (user_input_tx, user_input_rx) = mpsc::channel(8);

        let session = Session::resume(client, registry, &log_path_for_async, &state_root)
            .await
            .with_context(|| format!("resume failed for {}", log_path_for_async.display()))?
            .with_approval_channel(approval_tx)
            .with_user_input_channel(user_input_tx);
        anyhow::Ok((Arc::new(session), approval_rx, user_input_rx))
    })?;

    println!(
        "Resuming session {} ({}) — project {}",
        meta.id,
        log_path.display(),
        project_root.display()
    );
    let (session, approval_rx, user_input_rx) = session;
    crate::tui_cmd::run_with_session(session, approval_rx, user_input_rx, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a real `SessionMeta` by writing a single `SessionMeta` line
    /// to a temp file and reading it back through `Recorder::resume`.
    /// Avoids pulling chrono in as a dev-dep just for fixture data.
    fn fake_meta(id: &str) -> (tempfile::TempDir, PathBuf, SessionMeta) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{id}.jsonl"));
        let line = format!(
            "{{\"type\":\"session_meta\",\"id\":\"{id}\",\"project_root\":\"/tmp/x\",\"model\":\"m\",\"started_at\":\"2026-01-01T00:00:00Z\",\"awidat_version\":\"0.0.0\"}}\n"
        );
        std::fs::write(&path, line).unwrap();
        let (m, _) = Recorder::resume(&path).unwrap();
        (dir, path, m)
    }

    #[test]
    fn pick_by_list_index() {
        let (_d1, p1, m1) = fake_meta("aaa111");
        let (_d2, p2, m2) = fake_meta("bbb222");
        let entries = vec![(p1, m1), (p2.clone(), m2)];
        let (p, _) = pick(&entries, "2").unwrap();
        assert_eq!(p, p2);
    }

    #[test]
    fn pick_by_id_prefix() {
        let (_d1, p1, m1) = fake_meta("aaa111");
        let (_d2, p2, m2) = fake_meta("bbb222");
        let entries = vec![(p1, m1), (p2.clone(), m2)];
        let (p, _) = pick(&entries, "bbb").unwrap();
        assert_eq!(p, p2);
    }

    #[test]
    fn pick_ambiguous_prefix_errors() {
        let (_d1, p1, m1) = fake_meta("aaa111");
        let (_d2, p2, m2) = fake_meta("aaa999");
        let entries = vec![(p1, m1), (p2, m2)];
        let err = pick(&entries, "aaa").unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
    }

    #[test]
    fn pick_no_match_errors() {
        let (_d, p, m) = fake_meta("aaa111");
        let entries = vec![(p, m)];
        let err = pick(&entries, "zzz").unwrap_err();
        assert!(err.to_string().contains("no session matches"));
    }

    /// Smoke: empty listing prints the helpful message rather than panicking.
    #[test]
    fn print_listing_empty_is_fine() {
        print_listing(&[]);
    }
}
