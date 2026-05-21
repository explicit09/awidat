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

#![allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::match_like_matches_macro,
    clippy::needless_borrow,
    clippy::op_ref,
    clippy::type_complexity
)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod app_menu;
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
    // Resolve API keys before any thread spawns and (when stored in the
    // OS keychain) export them so MCP indexer subprocesses inherit them.
    // Idempotent — guarded by a OnceLock.
    secrets::resolve_at_startup();

    let state = AwidatState::default();

    // Dev convenience — preload project_root from env when set.
    if let Ok(p) = std::env::var("AWIDAT_DESKTOP_PROJECT") {
        let buf = PathBuf::from(&p);
        if buf.is_dir() && buf.join("project.otio.json").is_file() {
            *state.project_root.blocking_lock() = Some(buf);
        } else {
            warn!(path = %p, "AWIDAT_DESKTOP_PROJECT is not an awidat project; ignoring");
        }
    }

    let result = tauri::Builder::default()
        .enable_macos_default_menu(false)
        .menu(app_menu::build)
        .on_menu_event(app_menu::handle_menu_event)
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
                commands::project::allow_project_asset_dirs(app.handle(), &project_root);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::turn::start_turn,
            commands::turn::cancel_turn,
            commands::turn::respond_approval,
            commands::turn::respond_user_input,
            app_menu::set_menu_item_enabled,
            commands::project::set_project_root,
            commands::project::current_project_root,
            commands::project::close_project,
            commands::project::init_project,
            commands::project::recent_projects,
            commands::project::cancel_job,
            commands::project::get_project_type,
            commands::project::set_project_type,
            commands::dismissal::list_dismissals,
            commands::dismissal::dismiss_pattern,
            commands::dismissal::undismiss_pattern,
            commands::history::list_chat_sessions,
            commands::history::load_chat_history,
            commands::history::load_chat_session,
            commands::history::start_new_chat_session,
            commands::notes::list_notes,
            commands::notes::upsert_note,
            commands::notes::set_note_status,
            commands::notes::delete_note,
            commands::notes::dismissals_path,
            commands::permission::get_permission_mode,
            commands::permission::set_permission_mode,
            commands::professional::read_professional_lenses,
            commands::professional::read_pre_autonomy_inspection,
            commands::motion::read_motion,
            commands::import::import_local,
            commands::import::import_locals,
            commands::import::import_url,
            commands::index::index_project,
            commands::transcode::transcode_project_proxies,
            commands::transcode::proxy_cache_lifecycle_report,
            commands::media::list_proxies,
            commands::media::media_url_for_path,
            commands::media::proxy_path_for_stem,
            commands::preview::render_transition_preview_frame,
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
            commands::vedit::list_vedit_commits,
            commands::vedit::diff_vedit_refs,
            commands::vedit::changed_vedit_clip_ids,
            commands::vedit::preflight_vedit_merge,
            commands::vedit::list_vedit_tags,
            commands::vedit::tag_vedit_ref,
            commands::vedit::list_vedit_branches,
            commands::vedit::create_vedit_branch,
            commands::vedit::checkout_vedit_branch,
            commands::vedit::show_vedit_commit,
            commands::vedit::blame_vedit_clip,
            commands::vedit::restore_vedit_ref,
        ])
        .run(tauri::generate_context!());

    if let Err(err) = result {
        error!(error = %err, "tauri bootstrap failed");
        std::process::exit(1);
    }
}
