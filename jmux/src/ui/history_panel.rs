//! History pane — a searchable, day-grouped view of recently closed
//! workspaces (with reopen + "Clear Closed") and recently focused workspaces.
//!
//! Mirrors jmux's History pane. Data lives in the in-process `TabManager`
//! (`closed_entries()` / `focus_history()`), so the widget reads it directly
//! and triggers a UI refresh after reopening or selecting a workspace.

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gtk4::prelude::*;
use uuid::Uuid;

use crate::app::{lock_or_recover, AppState};

/// Build the History pane widget.
pub fn create_history_widget(
    panel_id: Uuid,
    state: &Rc<AppState>,
    is_attention_source: bool,
) -> gtk4::Widget {
    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    container.set_hexpand(true);
    container.set_vexpand(true);
    container.add_css_class("panel-shell");
    if is_attention_source {
        container.add_css_class("attention-panel");
    }
    container.set_widget_name(&panel_id.to_string());

    // ── Toolbar ──
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar.add_css_class("browser-nav-bar");
    toolbar.set_margin_start(6);
    toolbar.set_margin_end(6);
    toolbar.set_margin_top(2);
    toolbar.set_margin_bottom(2);

    let icon = gtk4::Image::from_icon_name("document-open-recent-symbolic");
    icon.set_pixel_size(16);
    toolbar.append(&icon);

    let search = gtk4::SearchEntry::new();
    search.set_placeholder_text(Some("Search history…"));
    search.set_hexpand(true);
    toolbar.append(&search);

    let clear_btn = gtk4::Button::from_icon_name("edit-clear-all-symbolic");
    clear_btn.add_css_class("flat");
    clear_btn.set_tooltip_text(Some("Clear closed history"));
    toolbar.append(&clear_btn);

    let reload_btn = gtk4::Button::from_icon_name("view-refresh-symbolic");
    reload_btn.add_css_class("flat");
    reload_btn.set_tooltip_text(Some("Refresh"));
    toolbar.append(&reload_btn);

    container.append(&toolbar);

    // ── Body ──
    let list = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    list.set_margin_start(8);
    list.set_margin_end(8);
    list.set_margin_top(6);
    list.set_margin_bottom(6);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);
    scrolled.set_child(Some(&list));
    container.append(&scrolled);

    populate(&list, state, "");

    {
        let list = list.clone();
        let state = state.clone();
        search.connect_search_changed(move |entry| {
            populate(&list, &state, entry.text().as_str());
        });
    }
    {
        let list = list.clone();
        let state = state.clone();
        let search = search.clone();
        reload_btn.connect_clicked(move |_| {
            populate(&list, &state, search.text().as_str());
        });
    }
    {
        let list = list.clone();
        let state = state.clone();
        let search = search.clone();
        clear_btn.connect_clicked(move |_| {
            lock_or_recover(&state.shared.tab_manager).clear_closed();
            populate(&list, &state, search.text().as_str());
        });
    }

    container.upcast()
}

/// (Re)build the list contents from the current TabManager state, filtered by
/// `filter` (case-insensitive substring on the title).
fn populate(list: &gtk4::Box, state: &Rc<AppState>, filter: &str) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let needle = filter.trim().to_lowercase();
    let matches = |title: &str| needle.is_empty() || title.to_lowercase().contains(&needle);

    // Snapshot the data under a short lock.
    struct Closed {
        id: Uuid,
        title: String,
        at: SystemTime,
    }
    let (closed, focused): (Vec<Closed>, Vec<(Uuid, String)>) = {
        let tm = lock_or_recover(&state.shared.tab_manager);
        // Newest first.
        let closed: Vec<Closed> = tm
            .closed_entries()
            .iter()
            .rev()
            .map(|e| Closed {
                id: e.workspace.id,
                title: if e.title.is_empty() {
                    "Workspace".to_string()
                } else {
                    e.title.clone()
                },
                at: e.closed_at,
            })
            .collect();
        // Focus history → resolve to currently-open workspaces, newest first,
        // de-duplicated.
        let mut seen = std::collections::HashSet::new();
        let mut focused = Vec::new();
        for id in tm.focus_history().iter().rev() {
            if !seen.insert(*id) {
                continue;
            }
            if let Some(ws) = tm.workspace(*id) {
                focused.push((*id, ws.display_title().to_string()));
            }
        }
        (closed, focused)
    };

    let mut any = false;

    // ── Recently closed (day-grouped, reopen on click) ──
    let mut last_day: Option<String> = None;
    for c in closed.iter().filter(|c| matches(&c.title)) {
        any = true;
        let day = day_header(c.at);
        if last_day.as_deref() != Some(day.as_str()) {
            list.append(&section_label(&day));
            last_day = Some(day);
        }
        let row = entry_button("document-symbolic", &c.title, &time_str(c.at));
        let id = c.id;
        let state = state.clone();
        row.connect_clicked(move |_| {
            {
                let mut tm = lock_or_recover(&state.shared.tab_manager);
                tm.reopen_closed(id);
            }
            state.shared.notify_ui_refresh();
        });
        list.append(&row);
    }

    // ── Recently focused (open workspaces; select on click) ──
    let focused: Vec<_> = focused.iter().filter(|(_, t)| matches(t)).collect();
    if !focused.is_empty() {
        list.append(&section_label("Recently focused"));
        for (id, title) in focused {
            any = true;
            let row = entry_button("go-jump-symbolic", title, "open");
            let id = *id;
            let state = state.clone();
            row.connect_clicked(move |_| {
                {
                    let mut tm = lock_or_recover(&state.shared.tab_manager);
                    tm.select_by_id(id);
                }
                state.shared.notify_ui_refresh();
            });
            list.append(&row);
        }
    }

    if !any {
        let empty = gtk4::Label::new(Some(if needle.is_empty() {
            "Nothing closed yet. Closed workspaces appear here."
        } else {
            "No history matches your search."
        }));
        empty.add_css_class("dim-label");
        empty.set_margin_top(12);
        list.append(&empty);
    }
}

fn section_label(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("dim-label");
    label.add_css_class("caption-heading");
    label.set_margin_top(8);
    label.set_margin_bottom(2);
    label
}

fn entry_button(icon: &str, title: &str, meta: &str) -> gtk4::Button {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let img = gtk4::Image::from_icon_name(icon);
    img.set_pixel_size(14);
    row.append(&img);
    let label = gtk4::Label::new(Some(title));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    row.append(&label);
    let meta_label = gtk4::Label::new(Some(meta));
    meta_label.add_css_class("dim-label");
    meta_label.add_css_class("caption");
    row.append(&meta_label);

    let btn = gtk4::Button::new();
    btn.set_child(Some(&row));
    btn.add_css_class("flat");
    btn.set_tooltip_text(Some(title));
    btn
}

/// "Today" / "Yesterday" / "Mon, Jun 9" header for a timestamp.
fn day_header(at: SystemTime) -> String {
    let secs = at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (Ok(dt), Ok(now)) = (
        glib::DateTime::from_unix_local(secs),
        glib::DateTime::now_local(),
    ) else {
        return "Earlier".to_string();
    };
    let same_day = |a: &glib::DateTime, b: &glib::DateTime| {
        a.year() == b.year() && a.day_of_year() == b.day_of_year()
    };
    if same_day(&dt, &now) {
        return "Today".to_string();
    }
    if let Ok(yesterday) = now.add_days(-1) {
        if same_day(&dt, &yesterday) {
            return "Yesterday".to_string();
        }
    }
    dt.format("%a, %b %e")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "Earlier".to_string())
}

/// "HH:MM" for a timestamp.
fn time_str(at: SystemTime) -> String {
    let secs = at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    glib::DateTime::from_unix_local(secs)
        .and_then(|dt| dt.format("%H:%M"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}
