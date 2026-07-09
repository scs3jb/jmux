//! Rename dialogs for workspaces and tabs.

use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::app::{lock_or_recover, AppState};

/// Build and present a standard "rename" dialog: a single text entry pre-filled
/// with `current` plus Cancel/Rename responses, shown over `window`. `on_accept`
/// receives the entered text when the user confirms. Centralises the scaffolding
/// shared by the workspace/tab/group/index rename dialogs (in-surface, so it
/// renders above the layer-shell quick-terminal overlay).
pub(crate) fn present_rename_dialog(
    window: &adw::ApplicationWindow,
    heading: &str,
    body: Option<&str>,
    current: &str,
    on_accept: impl Fn(String) + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), body);

    let entry = gtk4::Entry::new();
    entry.set_text(current);
    entry.set_activates_default(true);
    dialog.set_extra_child(Some(&entry));

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("rename", "Rename");
    dialog.set_default_response(Some("rename"));
    dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);

    let entry_cb = entry.clone();
    dialog.connect_response(None, move |_, response| {
        if response == "rename" {
            on_accept(entry_cb.text().to_string());
        }
    });

    dialog.present(Some(window));
    entry.grab_focus();
    entry.select_region(0, -1);
}

/// Show a dialog to rename the currently selected workspace.
pub(super) fn show_rename_dialog(
    window: &adw::ApplicationWindow,
    state: &Rc<AppState>,
    list_box: &gtk4::ListBox,
    content_box: &gtk4::Box,
    current_title: &str,
) {
    let state = state.clone();
    let list_box = list_box.clone();
    let content_box = content_box.clone();
    present_rename_dialog(
        window,
        "Rename Workspace",
        Some("Enter a new name for this workspace:"),
        current_title,
        move |new_name| {
            if new_name.is_empty() {
                return;
            }
            let mut tm = lock_or_recover(&state.shared.tab_manager);
            if let Some(ws) = tm.selected_mut() {
                ws.custom_title = Some(new_name);
            }
            drop(tm);
            super::refresh_ui(&list_box, &content_box, &state);
        },
    );
}

/// Show a dialog to rename a specific panel tab.
pub fn show_rename_tab_dialog(
    window: &adw::ApplicationWindow,
    state: &Rc<AppState>,
    panel_id: uuid::Uuid,
) {
    let current_title = {
        let tm = lock_or_recover(&state.shared.tab_manager);
        tm.find_workspace_with_panel(panel_id)
            .and_then(|ws| ws.panels.get(&panel_id))
            .map(|p| p.display_title().to_string())
            .unwrap_or_default()
    };

    let state = state.clone();
    present_rename_dialog(window, "Rename Tab", None, &current_title, move |new_title| {
        let mut tm = lock_or_recover(&state.shared.tab_manager);
        if let Some(ws) = tm.find_workspace_with_panel_mut(panel_id) {
            if let Some(panel) = ws.panels.get_mut(&panel_id) {
                panel.custom_title = if new_title.is_empty() { None } else { Some(new_title) };
            }
        }
        drop(tm);
        state.shared.notify_ui_refresh();
    });
}

/// Show a dialog to create a new SSH workspace.
pub fn show_ssh_dialog(window: &adw::ApplicationWindow, state: &Rc<AppState>) {
    let dialog = adw::AlertDialog::new(
        Some("New SSH Workspace"),
        Some("Connect to a remote host via SSH"),
    );

    let group = adw::PreferencesGroup::new();

    let dest_row = adw::EntryRow::new();
    dest_row.set_title("Destination");
    dest_row.set_text("user@host");
    group.add(&dest_row);

    let port_row = adw::EntryRow::new();
    port_row.set_title("Port (optional)");
    group.add(&port_row);

    let identity_row = adw::EntryRow::new();
    identity_row.set_title("Identity file (optional)");
    identity_row.set_text("");
    group.add(&identity_row);

    let agent_row = adw::SwitchRow::new();
    agent_row.set_title("Forward SSH agent");
    agent_row.set_subtitle("Forward the local SSH agent to the remote host (ssh -A)");
    agent_row.set_active(false);
    group.add(&agent_row);

    dialog.set_extra_child(Some(&group));

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("connect", "Connect");
    dialog.set_default_response(Some("connect"));
    dialog.set_response_appearance("connect", adw::ResponseAppearance::Suggested);

    let state = state.clone();
    dialog.connect_response(None, move |_dialog, response| {
        if response != "connect" {
            return;
        }
        let destination = dest_row.text().to_string();
        if destination.is_empty() || destination.starts_with('-') {
            return;
        }
        let port: Option<u16> = port_row
            .text()
            .to_string()
            .parse()
            .ok()
            .filter(|&p: &u16| p > 0);
        let identity = {
            let text = identity_row.text().to_string();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        };

        let remote_config = crate::remote::session::RemoteConfig {
            destination: destination.clone(),
            port,
            identity,
            ssh_options: Vec::new(),
            agent_forward: agent_row.is_active(),
            remote_daemon_path: None,
        };

        let mut ws = crate::model::Workspace::new();
        ws.custom_title = Some(destination.clone());
        ws.remote_config = Some(remote_config);
        let ws_id = ws.id;

        {
            let placement = crate::settings::load().new_workspace_placement;
            let mut tm = lock_or_recover(&state.shared.tab_manager);
            tm.add_workspace_with_placement(ws, placement);
            tm.select_by_id(ws_id);
        }

        state
            .shared
            .send_ui_event(crate::app::UiEvent::RemoteConnect {
                workspace_id: ws_id,
            });
        state.shared.notify_ui_refresh();
    });

    dialog.present(Some(window));
}
