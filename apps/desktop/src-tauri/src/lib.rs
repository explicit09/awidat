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
mod secrets;
mod session;
mod state;

use std::path::PathBuf;

use state::AwidatState;
use tauri::Manager;
use tracing::{error, warn};

/// Tauri entrypoint. Called from `main.rs`.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Resolve API keys (Anthropic, HuggingFace) before any thread
    // spawns. If they live in the OS keychain, also export them to
    // the parent env so MCP indexer subprocesses inherit them.
    // Safe to call any number of times; the OnceLock guards repeats.
    secrets::resolve_at_startup();

    let state = AwidatState::default();

    // Pre-populate project_root from env if set, for dev convenience.
    if let Ok(p) = std::env::var("AWIDAT_DESKTOP_PROJECT") {
        let buf = PathBuf::from(&p);
        if buf.is_dir() && buf.join("project.otio.json").is_file() {
            *state.project_root.blocking_lock() = Some(buf);
        } else {
            warn!(path = %p, "AWIDAT_DESKTOP_PROJECT is not an awidat project; ignoring");
        }
    }

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .manage(state)
        .setup(|app| {
            if let Some(project_root) = app
                .state::<AwidatState>()
                .project_root
                .blocking_lock()
                .clone()
            {
                commands::project::allow_proxies_dir(app.handle(), &project_root);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::turn::start_turn,
            commands::turn::cancel_turn,
            commands::turn::respond_approval,
            commands::turn::respond_user_input,
            commands::project::set_project_root,
            commands::project::current_project_root,
            commands::project::init_project,
            commands::project::recent_projects,
            commands::project::cancel_job,
            commands::project::get_project_type,
            commands::project::set_project_type,
            commands::import::import_local,
            commands::import::import_url,
            commands::index::index_project,
            commands::transcode::transcode_project_proxies,
            commands::media::list_proxies,
            commands::media::proxy_path_for_stem,
            commands::thumbnail::generate_thumbnails_for_asset,
            commands::thumbnail::list_thumbnail_frames,
            commands::waveform::read_waveform,
            commands::silence::read_silences,
            commands::view::set_view_state,
            commands::timeline::read_timeline,
            commands::render::start_timeline_render,
            commands::render::poll_timeline_render,
            commands::render::cancel_timeline_render,
            commands::proposal::accept_proposal,
            commands::proposal::reject_proposal,
            commands::proposal::adjust_proposal,
            commands::proposal::propose_user_edit,
            commands::transcript::read_transcript,
        ])
        .run(tauri::generate_context!());

    if let Err(err) = result {
        error!(error = %err, "tauri bootstrap failed");
        std::process::exit(1);
    }
}
