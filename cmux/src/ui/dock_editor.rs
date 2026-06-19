//! Dock editor — a GUI for the global `~/.config/cmux/dock.json` controls.
//!
//! Each control exposes `id`, `title`, and `command` fields; existing `cwd` /
//! `height` values are preserved. Saves on close.

use std::cell::Cell;
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::ui::dock::{self, DockControl};

struct ControlRow {
    expander: adw::ExpanderRow,
    id: adw::EntryRow,
    title: adw::EntryRow,
    command: adw::EntryRow,
    removed: Rc<Cell<bool>>,
    /// Original control, to preserve fields the editor doesn't expose.
    orig: Option<DockControl>,
}

/// Show the dock-controls editor. `on_saved` runs after a successful save.
pub fn show_dock_editor(parent: &adw::ApplicationWindow, on_saved: impl Fn() + 'static) {
    // In-surface adw::PreferencesDialog (not a top-level window) so it renders
    // above the content — including the layer-shell quake drop-down.
    let window = adw::PreferencesDialog::new();
    window.set_title("Dock Controls");

    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::new();
    group.set_title("Controls");
    group.set_description(Some(
        "Each control runs a command in its own terminal in the Dock. Saved to \
         ~/.config/cmux/dock.json.",
    ));

    let rows: Rc<std::cell::RefCell<Vec<ControlRow>>> =
        Rc::new(std::cell::RefCell::new(Vec::new()));

    // "Add Control" button in the group header.
    let add_btn = gtk4::Button::from_icon_name("list-add-symbolic");
    add_btn.add_css_class("flat");
    add_btn.set_tooltip_text(Some("Add control"));
    {
        let group = group.clone();
        let rows = rows.clone();
        add_btn.connect_clicked(move |_| {
            add_control_row(&group, &rows, None);
        });
    }
    group.set_header_suffix(Some(&add_btn));

    page.add(&group);
    window.add(&page);

    // Populate from the existing global config.
    for control in dock::load_global() {
        add_control_row(&group, &rows, Some(control));
    }

    // Save on close.
    {
        let rows = rows.clone();
        window.connect_closed(move |_| {
            let controls = collect(&rows);
            match dock::save_global(&controls) {
                Ok(()) => on_saved(),
                Err(e) => tracing::error!("Failed to save dock.json: {e}"),
            }
        });
    }

    window.present(Some(parent));
}

fn add_control_row(
    group: &adw::PreferencesGroup,
    rows: &Rc<std::cell::RefCell<Vec<ControlRow>>>,
    control: Option<DockControl>,
) {
    let expander = adw::ExpanderRow::new();
    let init_id = control.as_ref().map(|c| c.id.clone()).unwrap_or_default();
    expander.set_title(if init_id.is_empty() {
        "New control"
    } else {
        &init_id
    });

    let id_row = adw::EntryRow::new();
    id_row.set_title("ID");
    id_row.set_text(&init_id);
    expander.add_row(&id_row);

    let title_row = adw::EntryRow::new();
    title_row.set_title("Title");
    title_row.set_text(control.as_ref().and_then(|c| c.title.as_deref()).unwrap_or(""));
    expander.add_row(&title_row);

    let command_row = adw::EntryRow::new();
    command_row.set_title("Command");
    command_row.set_text(control.as_ref().map(|c| c.command.as_str()).unwrap_or(""));
    expander.add_row(&command_row);

    // Keep the expander title in sync with the ID field.
    {
        let expander = expander.clone();
        id_row.connect_changed(move |e| {
            let t = e.text();
            expander.set_title(if t.is_empty() { "New control" } else { t.as_str() });
        });
    }

    let removed = Rc::new(Cell::new(false));
    let remove_btn = gtk4::Button::from_icon_name("user-trash-symbolic");
    remove_btn.add_css_class("flat");
    remove_btn.set_valign(gtk4::Align::Center);
    remove_btn.set_tooltip_text(Some("Remove control"));
    {
        let group = group.clone();
        let expander = expander.clone();
        let removed = removed.clone();
        remove_btn.connect_clicked(move |_| {
            removed.set(true);
            group.remove(&expander);
        });
    }
    expander.add_suffix(&remove_btn);

    group.add(&expander);
    rows.borrow_mut().push(ControlRow {
        expander,
        id: id_row,
        title: title_row,
        command: command_row,
        removed,
        orig: control,
    });
}

fn collect(rows: &Rc<std::cell::RefCell<Vec<ControlRow>>>) -> Vec<DockControl> {
    rows.borrow()
        .iter()
        .filter(|r| !r.removed.get())
        .filter_map(|r| {
            let id = r.id.text().trim().to_string();
            let command = r.command.text().trim().to_string();
            if id.is_empty() || command.is_empty() {
                return None;
            }
            let title = r.title.text().trim().to_string();
            Some(DockControl {
                id,
                title: (!title.is_empty()).then_some(title),
                command,
                // Preserve fields the editor doesn't expose.
                cwd: r.orig.as_ref().and_then(|c| c.cwd.clone()),
                height: r.orig.as_ref().and_then(|c| c.height),
            })
        })
        .collect()
}
