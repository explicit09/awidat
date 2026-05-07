//! Tauri commands. Grouped by concern; lib.rs's `run()` registers
//! them all via `generate_handler!`.

pub mod auto_insert;
pub mod dismissal;
pub mod import;
pub mod index;
pub mod media;
pub mod notes;
pub mod permission;
pub mod project;
pub mod proposal;
pub mod render;
pub mod silence;
pub mod thumbnail;
pub mod timeline;
pub mod transcode;
pub mod transcript;
pub mod turn;
pub mod view;
pub mod waveform;
