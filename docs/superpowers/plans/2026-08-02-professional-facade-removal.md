# Professional Facade Removal Plan

**Goal:** Remove the final unused core orchestration facade without touching active professional schema or render engines.

### Task 1: Remove the desktop exposure

- Delete `apps/desktop/src-tauri/src/commands/professional.rs`.
- Remove its module and both Tauri handler registrations.
- Verify neither command name appears in frontend code.

### Task 2: Remove the core facade

- Delete `crates/core/src/professional.rs`.
- Move its live `derive_audio_finishing_state` helper and focused test to `crates/core/src/audio_finishing.rs`.
- Point the two podcast callers at the focused module and remove `pub mod professional` from core.
- Delete `crates/core/tests/professional_orchestration.rs`.
- Verify no `montage_core::professional` references remain.

### Task 3: Verify the boundary

- Run `cargo fmt --all -- --check`.
- Run `cargo check -p montage-core -p montage-desktop --all-targets`.
- Run relevant core and desktop tests.
- Run workspace clippy/check after focused gates.
- Confirm `montage_proto::professional` and `montage_render::professional` remain unchanged.
