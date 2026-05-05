//! Awidat desktop app — Tauri backend.
//!
//! Imports `awidat-core` (agent loop) and `awidat-desktop-protocol`
//! (frontend wire types). Exposes Tauri commands that build a
//! `Session`, drive `run_turn`, translate `SessionEvent` into
//! protocol [`Item`](awidat_desktop_protocol::Item)s, and route
//! approval / user-input responses back to the agent loop. Also
//! handles project lifecycle (open / new / recents) and — in
//! upcoming commits — asset import + indexing with progress.
//!
//! # Module layout
//!
//! - `state.rs` — `AwidatState` (the type Tauri threads through commands)
//! - `events.rs` — Tauri channel name constants + `emit_item` helper
//! - `session.rs` — `build_session` / tool registry
//! - `bridges.rs` — long-lived approval / user-input forwarders
//! - `commands/turn.rs` — `start_turn`, `cancel_turn`, `respond_*`
//! - `commands/project.rs` — `set_project_root`, `init_project`,
//!   `current_project_root`, `recent_projects`

mod bridges;
mod commands;
mod events;
mod session;
mod state;

use std::path::PathBuf;

use state::AwidatState;
use tracing::{error, warn};

/// Tauri entrypoint. Called from `main.rs`.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = AwidatState::default();

    // Pre-populate project_root from env if set, for dev convenience.
    if let Ok(p) = std::env::var("AWIDAT_DESKTOP_PROJECT") {
        let buf = PathBuf::from(&p);
        if buf.is_dir() {
            *state.project_root.blocking_lock() = Some(buf);
        } else {
            warn!(path = %p, "AWIDAT_DESKTOP_PROJECT is not a directory; ignoring");
        }
    }

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::turn::start_turn,
            commands::turn::cancel_turn,
            commands::turn::respond_approval,
            commands::turn::respond_user_input,
            commands::project::set_project_root,
            commands::project::current_project_root,
            commands::project::init_project,
            commands::project::recent_projects,
        ])
        .run(tauri::generate_context!());

    if let Err(err) = result {
        error!(error = %err, "tauri bootstrap failed");
        std::process::exit(1);
    }
}
