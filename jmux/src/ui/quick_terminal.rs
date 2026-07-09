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

/// Stable window id for the drop-down window, so it's a singleton and can be
/// excluded from session save/restore (it's recreated on demand, never
/// persisted as a normal window).
pub fn quick_window_id() -> uuid::Uuid {
    uuid::Uuid::from_u128(0x0c3de9a1_5b2f_4c6d_8e10_000000000001)
}

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
             (rebuild with --features jmux/quick-terminal and install gtk4-layer-shell)"
        );
    }
}

#[cfg(feature = "quick-terminal")]
mod imp;

#[cfg(feature = "quick-terminal")]
pub mod portal;

/// Register the quick-terminal global shortcut via the GlobalShortcuts portal,
/// using the GApplication's own D-Bus connection (so KDE shows its permission
/// prompt). Runs on the GTK main thread. Safe to call at startup and again when
/// settings change — it's idempotent and a no-op when the feature is off or the
/// quick terminal is disabled. Pass the application (its D-Bus connection is
/// used).
pub fn register_global_shortcut(
    app: &impl gtk4::prelude::IsA<gtk4::gio::Application>,
    shared: std::sync::Arc<crate::app::SharedState>,
) {
    #[cfg(feature = "quick-terminal")]
    {
        use gtk4::prelude::ApplicationExt;
        match app.as_ref().dbus_connection() {
            Some(conn) => portal::register(&conn, shared),
            None => tracing::warn!(
                "quick terminal: no D-Bus connection available to register the global shortcut"
            ),
        }
    }
    #[cfg(not(feature = "quick-terminal"))]
    {
        let _ = (app.as_ref(), shared);
    }
}
