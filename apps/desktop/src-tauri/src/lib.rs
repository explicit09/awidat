//! Awidat desktop app — Tauri backend.
//!
//! Step 8 cut the desktop over to driving the codex engine via a
//! subprocess per turn (`codex-exec --json`). The legacy in-process
//! `awidat-core` `Session` is no longer instantiated from here; the
//! Tauri commands translate codex's JSONL event stream into the
//! desktop wire protocol [`Item`](awidat_desktop_protocol::Item) shape.
//!
//! # Module layout
//!
//! - `state.rs` — `AwidatState` (the type Tauri threads through commands)
//! - `events.rs` — Tauri channel name constants + `emit_item` helper
//! - `codex_runner.rs` — spawn/monitor the `codex-exec` subprocess
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
mod codex_session;
mod commands;
mod generated_media_watcher;
mod events;
mod secrets;
mod state;

use std::path::PathBuf;

use state::AwidatState;
use tauri::Manager;
use tracing::{error, warn};

/// Tauri entrypoint. Called from `main.rs`.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Codex's middleware depth (response streaming, MCP dispatch, sandbox
    // wrappers, hooks) overflows the 2 MB default tokio worker stack on
    // macOS. Codex's own arg0_dispatch_or_else allocates 16 MB stacks
    // for the same reason (vendor/codex-rs/arg0/src/lib.rs:19). Install
    // a matching runtime before Tauri builds its own — async_runtime::set
    // is one-shot and must precede the first Tauri block_on.
    let codex_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 * 1024 * 1024)
        .thread_name("awidat-tokio")
        .build()
        .expect("build tokio runtime with 16 MB worker stacks");
    tauri::async_runtime::set(codex_runtime.handle().clone());
    // Keep the runtime alive for the lifetime of the process. Tauri only
    // holds a Handle; if we drop the Runtime here the worker threads die.
    std::mem::forget(codex_runtime);

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
                // Mirror what set_project_root does for explicit opens:
                // kick the proxy + sidecar backfill so a project loaded
                // from persistence (or AWIDAT_DESKTOP_PROJECT env) gets
                // its preview chain repaired without waiting for the
                // user to re-pick the project. Without this, projects
                // whose proxy never completed (e.g. ran out of disk
                // mid-transcode) stay in "Preview unavailable" until
                // the user closes + reopens.
                commands::transcode::spawn_proxy_backfill_on_load(
                    app.handle().clone(),
                    project_root.clone(),
                );
                commands::transcode::spawn_sidecar_backfill_on_load(
                    app.handle().clone(),
                    project_root,
                );
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
            commands::project::delete_project,
            commands::project::project_size_bytes,
            commands::project::cancel_job,
            commands::project::running_job_ids,
            commands::project::get_project_type,
            commands::project::set_project_type,
            commands::dismissal::list_dismissals,
            commands::dismissal::dismiss_pattern,
            commands::dismissal::undismiss_pattern,
            commands::history::list_chat_sessions,
            commands::history::load_chat_history,
            commands::history::load_chat_session,
            commands::history::start_new_chat_session,
            commands::history::resume_chat_session,
            commands::history::rename_chat_session,
            commands::history::delete_chat_session,
            commands::notes::list_notes,
            commands::notes::upsert_note,
            commands::notes::set_note_status,
            commands::notes::delete_note,
            commands::notes::dismissals_path,
            commands::permission::get_permission_mode,
            commands::permission::set_permission_mode,
            commands::config::read_indexer_config,
            commands::config::set_project_indexer_enabled,
            commands::professional::read_professional_lenses,
            commands::professional::read_pre_autonomy_inspection,
            commands::motion::read_motion,
            commands::import::import_local,
            commands::import::import_locals,
            commands::import::import_url,
            commands::index::index_project,
            commands::index::index_readiness,
            commands::transcode::transcode_project_proxies,
            commands::transcode::proxy_cache_lifecycle_report,
            commands::preview_cache::preview_cache_summary,
            commands::preview_cache::preview_cache_refresh,
            commands::generated_media::list_generated_media,
            commands::media::list_source_media,
            commands::media::list_proxies,
            commands::media::read_media_readiness,
            commands::media::media_url_for_path,
            commands::media::proxy_path_for_stem,
            commands::media::relink_missing_asset,
            commands::preview::render_transition_preview_frame,
            commands::thumbnail::generate_thumbnails_for_asset,
            commands::thumbnail::list_thumbnail_frames,
            commands::waveform::read_waveform,
            commands::indexer_data::read_scenes,
            commands::indexer_data::read_silences,
            commands::indexer_data::read_audio_samples,
            commands::indexer_data::read_faces,
            commands::indexer_data::read_motion_regions,
            commands::indexer_data::read_color_stats,
            commands::view::set_view_state,
            commands::timeline::read_timeline,
            commands::timeline_restore::read_timeline_otio_raw,
            commands::timeline_restore::restore_timeline_otio,
            commands::media::insert_media_on_timeline,
            commands::render::start_timeline_render,
            commands::render::poll_timeline_render,
            commands::render::cancel_timeline_render,
            commands::render::start_reframe_render,
            commands::render::cancel_reframe_render,
            commands::captions::export_caption_sidecars,
            commands::still_export::export_still,
            commands::proposal::accept_proposal,
            commands::proposal::reject_proposal,
            commands::proposal::adjust_proposal,
            commands::proposal::propose_user_edit,
            commands::clip_params::set_clip_volume,
            commands::clip_params::set_clip_speed,
            commands::clip_params::set_clip_fade,
            commands::clip_params::trim_timeline_tail,
            commands::clip_params::insert_timeline_track,
            commands::transcript::read_transcript,
            commands::transcript::rename_speaker,
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
            commands::review::author_local_review_package,
            commands::color_scopes::get_color_scopes,
            commands::skills::list_skills,
            commands::skills::read_skill_body,
            commands::skill_config::read_disabled_skills,
            commands::skill_config::write_disabled_skills,
            commands::agents_md::read_agents_md,
            commands::agents_md::write_agents_md,
        ])
        .build(tauri::generate_context!());

    let app = match result {
        Ok(app) => app,
        Err(err) => {
            error!(error = %err, "tauri bootstrap failed");
            std::process::exit(1);
        }
    };

    // Intercept ExitRequested so the in-flight turn (if any) has a
    // chance to terminate cleanly and the codex bridge drains its
    // pump task before the process dies. Bridge shutdown is async;
    // we block on it so the event-pump's last turn-end emit and
    // JSONRPC drain land before Tauri tears the runtime down.
    app.run(move |handle, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            let state = handle.state::<AwidatState>();
            tauri::async_runtime::block_on(async {
                // Cancel the in-flight turn first so the bridge's
                // pump-task drains a TurnInterrupt-shaped completion
                // rather than getting kicked while a turn is open.
                if let Some(turn) = state.turn.lock().await.take() {
                    turn.cancel.cancel();
                }
                if let Some(session) = state.codex.lock().await.take() {
                    if let Err(e) = session.bridge.shutdown().await {
                        tracing::warn!(error = %e, "codex bridge shutdown returned error");
                    }
                }
                if let Some(watcher) = state.generated_media_watcher.lock().await.take() {
                    watcher.cancel.cancel();
                }
            });
        }
    });
}
