//! Media-pane view-state plumbing. The frontend pushes the current
//! playback position into AwidatState whenever the user scrubs or
//! play state changes; `start_turn` reads it and prefixes a context
//! line onto user input so the agent knows what's on screen.

use tauri::State;

use crate::state::{AwidatState, ViewState};

/// Frontend → backend push of the media pane's current state.
/// Called from MediaPane on scrub / play / pause / select.
#[tauri::command]
pub async fn set_view_state(
    state: State<'_, AwidatState>,
    stem: String,
    current_time_s: f64,
    is_playing: bool,
) -> Result<(), String> {
    if stem.is_empty() {
        // Empty stem = no asset selected. Clear the slot so the
        // next turn doesn't carry stale context.
        *state.view_state.lock().await = None;
        return Ok(());
    }
    *state.view_state.lock().await = Some(ViewState {
        stem,
        current_time_s,
        is_playing,
    });
    Ok(())
}

/// Build a one-line context prefix to prepend to user input. Keeps
/// the line terse and bracketed so the agent recognizes it as
/// metadata rather than user text.
pub fn format_view_context(view: &ViewState) -> String {
    let mm_ss = format_mm_ss(view.current_time_s);
    let verb = if view.is_playing { "watching" } else { "viewing" };
    format!(
        "[user is {verb} {} at {mm_ss}]",
        view.stem
    )
}

fn format_mm_ss(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".into();
    }
    let total = seconds as u64;
    let m = total / 60;
    let s = total % 60;
    format!("{m}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_mm_ss_typical() {
        assert_eq!(format_mm_ss(0.0), "0:00");
        assert_eq!(format_mm_ss(3.5), "0:03");
        assert_eq!(format_mm_ss(65.9), "1:05");
        assert_eq!(format_mm_ss(599.0), "9:59");
        assert_eq!(format_mm_ss(3600.0), "60:00");
    }

    #[test]
    fn format_mm_ss_handles_bogus() {
        assert_eq!(format_mm_ss(f64::NAN), "0:00");
        assert_eq!(format_mm_ss(-1.0), "0:00");
        assert_eq!(format_mm_ss(f64::INFINITY), "0:00");
    }

    #[test]
    fn format_view_context_includes_stem_and_time() {
        let v = ViewState {
            stem: "ep1".into(),
            current_time_s: 23.0,
            is_playing: true,
        };
        assert_eq!(format_view_context(&v), "[user is watching ep1 at 0:23]");
    }

    #[test]
    fn format_view_context_paused_uses_viewing() {
        let v = ViewState {
            stem: "ep1".into(),
            current_time_s: 0.0,
            is_playing: false,
        };
        assert_eq!(format_view_context(&v), "[user is viewing ep1 at 0:00]");
    }
}
