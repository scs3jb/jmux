//! Quake-style drop-down "quick terminal".
//!
//! A chromeless overlay window anchored to the top edge of the screen that
//! slides in/out, toggled by a global hotkey or the `quick_terminal.*` socket
//! methods. The actual window uses the `wlr-layer-shell` protocol (via
//! gtk4-layer-shell) and is therefore compiled only when the `quick-terminal`
//! cargo feature is enabled; default builds expose the dispatcher as a no-op so
//! the rest of the app (settings, socket, CLI) works everywhere.

use std::rc::Rc;

use crate::app::{AppState, QuickTermAction};

/// Handle a quick-terminal action on the GTK main thread. `app` is any live
/// application handle (used to create the drop-down window on first use).
pub fn handle(action: QuickTermAction, app: &gtk4::Application, state: &Rc<AppState>) {
    #[cfg(feature = "quick-terminal")]
    {
        imp::handle(action, app, state);
    }
    #[cfg(not(feature = "quick-terminal"))]
    {
        let _ = (action, app, state);
        tracing::warn!(
            "quick terminal requested, but this build lacks the 'quick-terminal' feature \
             (rebuild with --features cmux/quick-terminal and install gtk4-layer-shell)"
        );
    }
}

#[cfg(feature = "quick-terminal")]
mod imp;

#[cfg(feature = "quick-terminal")]
pub mod portal;

/// Spawn the GlobalShortcuts portal listener on the current tokio runtime, so
/// the configured hotkey toggles the quick terminal system-wide. No-op in
/// builds without the `quick-terminal` feature.
pub fn spawn_global_shortcut(shared: std::sync::Arc<crate::app::SharedState>) {
    #[cfg(feature = "quick-terminal")]
    {
        tokio::spawn(async move { portal::run(shared).await });
    }
    #[cfg(not(feature = "quick-terminal"))]
    {
        let _ = shared;
    }
}
