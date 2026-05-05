//! Tauri commands. Grouped by concern; lib.rs's `run()` registers
//! them all via `generate_handler!`.

pub mod auto_insert;
pub mod import;
pub mod index;
pub mod media;
pub mod project;
pub mod render;
pub mod timeline;
pub mod transcode;
pub mod turn;
pub mod view;
