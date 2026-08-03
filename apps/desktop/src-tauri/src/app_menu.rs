//! Native desktop menu construction and event forwarding.

use serde::{Deserialize, Serialize};
use tauri::menu::{
    AboutMetadata, Menu, MenuBuilder, MenuEvent, MenuItem, MenuItemBuilder, MenuItemKind,
    SubmenuBuilder,
};
use tauri::{AppHandle, Emitter, State, Wry};

/// Mirrors `apps/desktop/src/protocol/index.ts`.
pub const MENU_COMMAND_EVENT: &str = "montage://menu-command";

pub mod id {
    pub const NEW_PROJECT: &str = "project:new";
    pub const OPEN_PROJECT: &str = "project:open";
    pub const OPEN_RECENT: &str = "project:open_recent";
    pub const CLOSE_PROJECT: &str = "project:close";
    pub const IMPORT_FILES: &str = "asset:import_files";
    pub const IMPORT_URL: &str = "asset:import_url";
    pub const RUN_INDEXERS: &str = "project:run_indexers";
    pub const EXPORT_TIMELINE: &str = "timeline:export";
    pub const REVEAL_PROJECT: &str = "project:reveal";
    pub const UNDO: &str = "edit:undo";
    pub const REDO: &str = "edit:redo";
    pub const SPLIT_CLIP: &str = "edit:split_clip";
    pub const DELETE_CLIP: &str = "edit:delete_clip";
    pub const ACCEPT_PROPOSAL: &str = "proposal:accept";
    pub const REJECT_PROPOSAL: &str = "proposal:reject";
    pub const VIEW_MEDIA: &str = "view:media";
    pub const VIEW_TIMELINE: &str = "view:timeline";
    pub const VIEW_PROPERTIES: &str = "view:properties";
    pub const VIEW_NOTES: &str = "view:notes";
    pub const VIEW_SCOPES: &str = "view:scopes";
    pub const VIEW_SIDEBAR: &str = "view:sidebar";
    pub const VIEW_CHAT: &str = "view:chat";
    pub const VIEW_TRANSCRIPT: &str = "view:transcript";
    pub const VIEW_EDITS: &str = "view:edits";
    pub const TIMELINE_ZOOM_IN: &str = "timeline:zoom_in";
    pub const TIMELINE_ZOOM_OUT: &str = "timeline:zoom_out";
    pub const TIMELINE_ZOOM_FIT: &str = "timeline:zoom_fit";
    pub const TOGGLE_FULLSCREEN: &str = "window:toggle_fullscreen";
    pub const NAV_PROJECT: &str = "nav:project";
    pub const NAV_WORKSPACE: &str = "nav:workspace";
    pub const NAV_AGENT: &str = "nav:agent";
    pub const NAV_MEDIA: &str = "nav:media";
    pub const NAV_REVIEW: &str = "nav:review";
    pub const NAV_DELIVER: &str = "nav:deliver";
    pub const NAV_SETTINGS: &str = "nav:settings";
    pub const HELP_DOCS: &str = "help:docs";
    pub const HELP_SHORTCUTS: &str = "help:shortcuts";
    pub const HELP_LOGS: &str = "help:logs";
    pub const HELP_REPORT_ISSUE: &str = "help:report_issue";
}

const DISABLED_SETTINGS: &str = "app:settings";
const DISABLED_UPDATES: &str = "app:check_updates";

#[derive(Debug, Clone, Serialize)]
pub struct MenuCommandPayload {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MenuItemEnabled {
    pub id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuPlatform {
    Macos,
    Windows,
}

impl MenuPlatform {
    fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Windows
        }
    }
}

pub fn build(app: &AppHandle<Wry>) -> tauri::Result<Menu<Wry>> {
    match MenuPlatform::current() {
        MenuPlatform::Macos => build_macos(app),
        MenuPlatform::Windows => build_windows(app),
    }
}

pub fn handle_menu_event(app: &AppHandle<Wry>, event: MenuEvent) {
    let id = event.id().as_ref();
    if frontend_command_ids().contains(&id) {
        if let Err(e) = app.emit(
            MENU_COMMAND_EVENT,
            MenuCommandPayload { id: id.to_string() },
        ) {
            tracing::warn!(error = %e, id, "emit menu command failed");
        }
    }
}

#[tauri::command]
pub async fn set_menu_item_enabled(
    app: AppHandle<Wry>,
    _state: State<'_, crate::state::MontageState>,
    states: Vec<MenuItemEnabled>,
) -> Result<(), String> {
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    for state in states {
        set_enabled_recursive(
            menu.items().map_err(|e| e.to_string())?,
            &state.id,
            state.enabled,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn set_enabled_recursive(
    items: Vec<MenuItemKind<Wry>>,
    id: &str,
    enabled: bool,
) -> tauri::Result<bool> {
    let mut changed = false;
    for item in items {
        if item.id() == &id {
            set_item_enabled(&item, enabled)?;
            changed = true;
        }
        if let MenuItemKind::Submenu(submenu) = item {
            changed |= set_enabled_recursive(submenu.items()?, id, enabled)?;
        }
    }
    Ok(changed)
}

fn set_item_enabled(item: &MenuItemKind<Wry>, enabled: bool) -> tauri::Result<()> {
    match item {
        MenuItemKind::MenuItem(i) => i.set_enabled(enabled),
        MenuItemKind::Submenu(i) => i.set_enabled(enabled),
        MenuItemKind::Predefined(_) => Ok(()),
        MenuItemKind::Check(i) => i.set_enabled(enabled),
        MenuItemKind::Icon(i) => i.set_enabled(enabled),
    }
}

fn build_macos(app: &AppHandle<Wry>) -> tauri::Result<Menu<Wry>> {
    let app_menu = SubmenuBuilder::new(app, "Montage")
        .about_with_text("About Montage", Some(about_metadata()))
        .item(&disabled(app, DISABLED_SETTINGS, "Settings…")?)
        .item(&disabled(app, DISABLED_UPDATES, "Check for Updates…")?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit_with_text("Quit Montage")
        .build()?;

    MenuBuilder::new(app)
        .item(&app_menu)
        .item(&file_menu(app, "Reveal Project in Finder", false, true)?)
        .item(&edit_menu(app)?)
        .item(&view_menu(app)?)
        .item(&window_menu(app)?)
        .item(&help_menu(app, false)?)
        .build()
}

fn build_windows(app: &AppHandle<Wry>) -> tauri::Result<Menu<Wry>> {
    MenuBuilder::new(app)
        .item(&file_menu(
            app,
            "Reveal Project in File Explorer",
            true,
            false,
        )?)
        .item(&edit_menu(app)?)
        .item(&view_menu(app)?)
        .item(&help_menu(app, true)?)
        .build()
}

fn file_menu(
    app: &AppHandle<Wry>,
    reveal_label: &str,
    include_exit: bool,
    use_ellipsis: bool,
) -> tauri::Result<tauri::menu::Submenu<Wry>> {
    let ellipsis = if use_ellipsis { "…" } else { "" };
    let new_project = item(
        app,
        id::NEW_PROJECT,
        &format!("New Project{ellipsis}"),
        true,
        Some("CmdOrCtrl+N"),
    )?;
    let open_project = item(
        app,
        id::OPEN_PROJECT,
        &format!("Open Project{ellipsis}"),
        true,
        Some("CmdOrCtrl+O"),
    )?;
    let open_recent = item(app, id::OPEN_RECENT, "Open Recent", true, None)?;
    let close_project = item(
        app,
        id::CLOSE_PROJECT,
        "Close Project",
        false,
        Some("CmdOrCtrl+W"),
    )?;
    let reveal_project = item(app, id::REVEAL_PROJECT, reveal_label, false, None)?;
    let import_files = item(
        app,
        id::IMPORT_FILES,
        &format!("Import Files{ellipsis}"),
        false,
        Some("CmdOrCtrl+I"),
    )?;
    let import_url = item(
        app,
        id::IMPORT_URL,
        &format!("Import URL{ellipsis}"),
        false,
        Some("CmdOrCtrl+Shift+I"),
    )?;
    let export_timeline = item(
        app,
        id::EXPORT_TIMELINE,
        &format!("Export Timeline{ellipsis}"),
        false,
        Some("CmdOrCtrl+E"),
    )?;

    let mut builder = SubmenuBuilder::new(app, "File")
        .item(&new_project)
        .item(&open_project)
        .item(&open_recent)
        .separator()
        .item(&close_project)
        .item(&reveal_project)
        .separator()
        .item(&import_files)
        .item(&import_url)
        .separator()
        .item(&export_timeline);

    if include_exit {
        builder = builder.separator().quit_with_text("Exit");
    }

    builder.build()
}

fn edit_menu(app: &AppHandle<Wry>) -> tauri::Result<tauri::menu::Submenu<Wry>> {
    let undo = item(app, id::UNDO, "Undo", false, None)?;
    let redo = item(app, id::REDO, "Redo", false, None)?;
    let split = item(app, id::SPLIT_CLIP, "Split Clip at Playhead", false, None)?;
    let delete_clip = item(
        app,
        id::DELETE_CLIP,
        "Delete Selected Clip",
        false,
        Some("Delete"),
    )?;
    let accept = item(
        app,
        id::ACCEPT_PROPOSAL,
        "Accept Proposal",
        false,
        Some("Enter"),
    )?;
    let reject = item(
        app,
        id::REJECT_PROPOSAL,
        "Reject Proposal",
        false,
        Some("Esc"),
    )?;

    SubmenuBuilder::new(app, "Edit")
        .item(&undo)
        .item(&redo)
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .separator()
        .item(&split)
        .item(&delete_clip)
        .separator()
        .item(&accept)
        .item(&reject)
        .build()
}

fn view_menu(app: &AppHandle<Wry>) -> tauri::Result<tauri::menu::Submenu<Wry>> {
    let nav_project = item(app, id::NAV_PROJECT, "Project Overview", true, None)?;
    let nav_workspace = item(app, id::NAV_WORKSPACE, "Workspace", true, None)?;
    let nav_media = item(app, id::NAV_MEDIA, "Media / Import", true, None)?;
    let nav_review = item(app, id::NAV_REVIEW, "Review", true, None)?;
    let nav_deliver = item(app, id::NAV_DELIVER, "Delivery", true, None)?;
    let nav_settings = item(app, id::NAV_SETTINGS, "Settings", true, None)?;

    let timeline = item(app, id::VIEW_TIMELINE, "Timeline", false, None)?;
    let transcript = item(app, id::VIEW_TRANSCRIPT, "Transcript", false, None)?;
    let notes = item(app, id::VIEW_NOTES, "Notes", false, None)?;
    let run_indexers = item(app, id::RUN_INDEXERS, "Run Indexers", false, None)?;
    let zoom_in = item(
        app,
        id::TIMELINE_ZOOM_IN,
        "Zoom Timeline In",
        false,
        Some("CmdOrCtrl+="),
    )?;
    let zoom_out = item(
        app,
        id::TIMELINE_ZOOM_OUT,
        "Zoom Timeline Out",
        false,
        Some("CmdOrCtrl+-"),
    )?;
    let zoom_fit = item(
        app,
        id::TIMELINE_ZOOM_FIT,
        "Fit Timeline",
        false,
        Some("CmdOrCtrl+0"),
    )?;
    let fullscreen = item(
        app,
        id::TOGGLE_FULLSCREEN,
        "Toggle Full Screen",
        true,
        Some("F11"),
    )?;

    SubmenuBuilder::new(app, "View")
        .item(&nav_project)
        .item(&nav_workspace)
        .item(&nav_media)
        .item(&nav_review)
        .item(&nav_deliver)
        .item(&nav_settings)
        .separator()
        .item(&timeline)
        .item(&transcript)
        .item(&notes)
        .separator()
        .item(&run_indexers)
        .separator()
        .item(&zoom_in)
        .item(&zoom_out)
        .item(&zoom_fit)
        .separator()
        .item(&fullscreen)
        .build()
}

fn window_menu(app: &AppHandle<Wry>) -> tauri::Result<tauri::menu::Submenu<Wry>> {
    SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize_with_text("Zoom")
        .separator()
        .bring_all_to_front()
        .build()
}

fn help_menu(
    app: &AppHandle<Wry>,
    include_about: bool,
) -> tauri::Result<tauri::menu::Submenu<Wry>> {
    let mut builder = SubmenuBuilder::new(app, "Help")
        .item(&item(
            app,
            id::HELP_DOCS,
            if include_about {
                "Documentation"
            } else {
                "Montage Help"
            },
            true,
            None,
        )?)
        .item(&item(
            app,
            id::HELP_SHORTCUTS,
            "Keyboard Shortcuts",
            true,
            None,
        )?)
        .item(&item(app, id::HELP_LOGS, "Open Logs", true, None)?)
        .item(&item(
            app,
            id::HELP_REPORT_ISSUE,
            "Report Issue",
            true,
            None,
        )?);

    if include_about {
        builder = builder
            .separator()
            .about_with_text("About Montage", Some(about_metadata()));
    }

    builder.build()
}

fn item(
    app: &AppHandle<Wry>,
    id: &str,
    label: &str,
    enabled: bool,
    accelerator: Option<&str>,
) -> tauri::Result<MenuItem<Wry>> {
    let mut builder = MenuItemBuilder::with_id(id, label).enabled(enabled);
    if let Some(accelerator) = accelerator {
        builder = builder.accelerator(accelerator);
    }
    builder.build(app)
}

fn disabled(app: &AppHandle<Wry>, id: &str, label: &str) -> tauri::Result<MenuItem<Wry>> {
    item(app, id, label, false, None)
}

fn about_metadata() -> AboutMetadata<'static> {
    AboutMetadata {
        name: Some("Montage".into()),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        authors: Some(vec!["Montage Authors".into()]),
        comments: Some("Collaborative video editor with an agent inside".into()),
        license: Some("Apache-2.0".into()),
        ..Default::default()
    }
}

fn frontend_command_ids() -> &'static [&'static str] {
    &[
        id::NEW_PROJECT,
        id::OPEN_PROJECT,
        id::OPEN_RECENT,
        id::CLOSE_PROJECT,
        id::IMPORT_FILES,
        id::IMPORT_URL,
        id::RUN_INDEXERS,
        id::EXPORT_TIMELINE,
        id::REVEAL_PROJECT,
        id::UNDO,
        id::REDO,
        id::SPLIT_CLIP,
        id::DELETE_CLIP,
        id::ACCEPT_PROPOSAL,
        id::REJECT_PROPOSAL,
        id::VIEW_MEDIA,
        id::VIEW_TIMELINE,
        id::VIEW_PROPERTIES,
        id::VIEW_NOTES,
        id::VIEW_SCOPES,
        id::VIEW_SIDEBAR,
        id::VIEW_CHAT,
        id::VIEW_TRANSCRIPT,
        id::VIEW_EDITS,
        id::TIMELINE_ZOOM_IN,
        id::TIMELINE_ZOOM_OUT,
        id::TIMELINE_ZOOM_FIT,
        id::TOGGLE_FULLSCREEN,
        id::NAV_PROJECT,
        id::NAV_WORKSPACE,
        id::NAV_AGENT,
        id::NAV_MEDIA,
        id::NAV_REVIEW,
        id::NAV_DELIVER,
        id::NAV_SETTINGS,
        id::HELP_DOCS,
        id::HELP_SHORTCUTS,
        id::HELP_LOGS,
        id::HELP_REPORT_ISSUE,
    ]
}

#[cfg(test)]
fn top_level_menu_labels(platform: MenuPlatform) -> Vec<&'static str> {
    match platform {
        MenuPlatform::Macos => vec!["Montage", "File", "Edit", "View", "Window", "Help"],
        MenuPlatform::Windows => vec!["File", "Edit", "View", "Help"],
    }
}

#[cfg(test)]
fn edit_menu_labels() -> Vec<&'static str> {
    vec![
        "Cut",
        "Copy",
        "Paste",
        "Select All",
        "Undo",
        "Redo",
        "Split Clip at Playhead",
        "Delete Selected Clip",
        "Accept Proposal",
        "Reject Proposal",
    ]
}

#[cfg(test)]
fn view_menu_labels() -> Vec<&'static str> {
    vec![
        "Project Overview",
        "Workspace",
        "Media / Import",
        "Review",
        "Delivery",
        "Settings",
        "Timeline",
        "Transcript",
        "Notes",
        "Run Indexers",
        "Zoom Timeline In",
        "Zoom Timeline Out",
        "Fit Timeline",
        "Toggle Full Screen",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_labels_follow_platform_conventions() {
        // The bar used to carry seven workspace-style menus (Project /
        // Workspace / Agent / Media / Review / Deliver / Settings) on
        // top of File/Edit/View/Window/Help. They duplicated File and
        // View items and made the bar feel cluttered. Consolidated
        // into the platform-standard set; nav items now live under
        // View, import/export/reveal under File, accept/reject under
        // Edit.
        assert_eq!(
            top_level_menu_labels(MenuPlatform::Macos),
            vec!["Montage", "File", "Edit", "View", "Window", "Help"]
        );
        assert_eq!(
            top_level_menu_labels(MenuPlatform::Windows),
            vec!["File", "Edit", "View", "Help"]
        );
    }

    #[test]
    fn frontend_commands_include_core_actions() {
        let ids = frontend_command_ids();
        for required in [
            id::NEW_PROJECT,
            id::OPEN_PROJECT,
            id::IMPORT_FILES,
            id::IMPORT_URL,
            id::RUN_INDEXERS,
            id::EXPORT_TIMELINE,
            id::DELETE_CLIP,
            id::ACCEPT_PROPOSAL,
            id::REJECT_PROPOSAL,
            id::TIMELINE_ZOOM_IN,
            id::TOGGLE_FULLSCREEN,
            id::NAV_PROJECT,
            id::NAV_WORKSPACE,
            id::NAV_AGENT,
            id::NAV_MEDIA,
            id::NAV_REVIEW,
            id::NAV_DELIVER,
            id::NAV_SETTINGS,
        ] {
            assert!(ids.contains(&required), "missing {required}");
        }
    }

    #[test]
    fn edit_menu_contains_only_functional_items() {
        let labels = edit_menu_labels();
        for removed in ["Trim Start to Playhead", "Trim End to Playhead"] {
            assert!(!labels.contains(&removed), "{removed} should not be shown");
        }
        for kept in [
            "Cut",
            "Copy",
            "Paste",
            "Select All",
            "Undo",
            "Redo",
            "Split Clip at Playhead",
            "Delete Selected Clip",
            "Accept Proposal",
            "Reject Proposal",
        ] {
            assert!(labels.contains(&kept), "{kept} should stay visible");
        }
    }

    #[test]
    fn view_menu_omits_legacy_noop_panes() {
        let labels = view_menu_labels();
        for removed in [
            "Agent Rail",
            "Media Viewer",
            "Properties",
            "Color Scopes",
            "Sidebar",
            "Chat",
            "Edit History",
        ] {
            assert!(!labels.contains(&removed), "{removed} should not be shown");
        }
        for kept in [
            "Project Overview",
            "Workspace",
            "Media / Import",
            "Review",
            "Delivery",
            "Settings",
            "Transcript",
            "Notes",
            "Run Indexers",
            "Zoom Timeline In",
            "Zoom Timeline Out",
            "Fit Timeline",
            "Toggle Full Screen",
        ] {
            assert!(labels.contains(&kept), "{kept} should stay visible");
        }
    }

    #[test]
    fn help_menu_commands_are_frontend_routed() {
        let ids = frontend_command_ids();
        for required in [
            id::HELP_DOCS,
            id::HELP_SHORTCUTS,
            id::HELP_LOGS,
            id::HELP_REPORT_ISSUE,
        ] {
            assert!(ids.contains(&required), "missing {required}");
        }
    }
}
