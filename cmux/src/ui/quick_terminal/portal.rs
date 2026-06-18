//! Global hotkey for the quick terminal via the `org.freedesktop.portal.
//! GlobalShortcuts` portal (feature `quick-terminal`).
//!
//! Registers the configured hotkey as a portal shortcut (the user can rebind it
//! in their desktop's system settings) and toggles the drop-down when it fires.
//! Runs on the tokio runtime; toggling is marshalled to the GTK thread via the
//! shared UI-event channel. Degrades gracefully when no GlobalShortcuts backend
//! is available (e.g. wlroots/GNOME) — bind `cmux quick-terminal toggle` to a
//! desktop shortcut instead.

use std::sync::Arc;

use futures_util::StreamExt;

use crate::app::{QuickTermAction, SharedState, UiEvent};

const SHORTCUT_ID: &str = "toggle-quick-terminal";

/// Register the hotkey (if the quick terminal is enabled) and toggle on each
/// activation. Never returns while the portal session is alive.
pub async fn run(shared: Arc<SharedState>) {
    let cfg = crate::settings::load().quick_terminal;
    if !cfg.enabled || cfg.hotkey.trim().is_empty() {
        return;
    }
    if let Err(e) = run_inner(shared, cfg.hotkey).await {
        tracing::warn!(
            "quick terminal global shortcut unavailable ({e}); bind \
             `cmux quick-terminal toggle` to a desktop shortcut instead"
        );
    }
}

async fn run_inner(shared: Arc<SharedState>, hotkey: String) -> ashpd::Result<()> {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};

    let global_shortcuts = GlobalShortcuts::new().await?;
    let session = global_shortcuts.create_session().await?;

    let shortcut = NewShortcut::new(SHORTCUT_ID, "Toggle the cmux quick terminal")
        .preferred_trigger(Some(hotkey.as_str()));
    global_shortcuts
        .bind_shortcuts(&session, &[shortcut], None)
        .await?;

    tracing::info!(%hotkey, "quick terminal global shortcut registered");

    let mut activated = global_shortcuts.receive_activated().await?;
    while let Some(activation) = activated.next().await {
        if activation.shortcut_id() == SHORTCUT_ID {
            shared.send_ui_event(UiEvent::QuickTerminal(QuickTermAction::Toggle));
        }
    }
    Ok(())
}
