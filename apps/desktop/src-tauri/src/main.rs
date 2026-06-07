//! Montage desktop binary entrypoint. Delegates to the lib crate's
//! `run()`; the split is required by Tauri's Windows builds.

// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    montage_desktop_lib::run()
}
