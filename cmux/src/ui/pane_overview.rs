//! Pane overview — a full-window grid of status tiles for every pane in the
//! active workspace. Each tile shows the pane's type, title, directory, a
//! status dot (busy / idle / attention / browser), and a one-line activity
//! snippet. Clicking a tile (or arrow-keys + Enter) jumps to that pane.

use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use uuid::Uuid;

use crate::app::{lock_or_recover, AppState};
use crate::model::PanelType;

struct TileInfo {
    panel_id: Uuid,
    title: String,
    directory: String,
    activity: String,
    icon: &'static str,
    dot: &'static str,
    dot_class: &'static str,
    focused: bool,
}

/// Show the pane overview for the active workspace.
pub fn show_pane_overview(parent: &adw::ApplicationWindow, state: &Rc<AppState>) {
    let tiles = collect_tiles(state);

    // An in-surface adw::Dialog (not a separate top-level window) so it renders
    // above the content — including the layer-shell quake drop-down, which sits
    // on the overlay layer above any normal window.
    let dialog = adw::Dialog::new();
    dialog.set_title("Overview");
    dialog.set_content_width(900);
    dialog.set_content_height(620);

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let count = tiles.len();
    let attention = tiles.iter().filter(|t| t.dot_class == "overview-dot-attention").count();
    let subtitle = if attention > 0 {
        format!("{count} panes · {attention} need attention")
    } else {
        format!("{count} panes")
    };
    header.set_title_widget(Some(&adw::WindowTitle::new("Overview", &subtitle)));
    toolbar.add_top_bar(&header);

    if tiles.is_empty() {
        let empty = gtk4::Label::new(Some("No panes in this workspace."));
        empty.add_css_class("dim-label");
        empty.set_vexpand(true);
        toolbar.set_content(Some(&empty));
        dialog.set_child(Some(&toolbar));
        dialog.present(Some(parent));
        return;
    }

    let flow = gtk4::FlowBox::new();
    flow.set_selection_mode(gtk4::SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_min_children_per_line(2);
    flow.set_max_children_per_line(4);
    flow.set_row_spacing(12);
    flow.set_column_spacing(12);
    flow.set_margin_start(16);
    flow.set_margin_end(16);
    flow.set_margin_top(16);
    flow.set_margin_bottom(16);

    for tile in &tiles {
        flow.append(&build_tile(tile, state, &dialog));
    }

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled.set_vexpand(true);
    scrolled.set_child(Some(&flow));
    toolbar.set_content(Some(&scrolled));
    dialog.set_child(Some(&toolbar));

    // adw::Dialog closes on Escape natively — no key controller needed.
    dialog.present(Some(parent));
}

fn build_tile(tile: &TileInfo, state: &Rc<AppState>, dialog: &adw::Dialog) -> gtk4::Widget {
    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    vbox.set_size_request(200, 120);
    vbox.add_css_class("overview-tile");

    // Header line: status dot + type icon + title.
    let head = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    let dot = gtk4::Label::new(Some(tile.dot));
    dot.add_css_class(tile.dot_class);
    head.append(&dot);
    let icon = gtk4::Image::from_icon_name(tile.icon);
    icon.set_pixel_size(14);
    head.append(&icon);
    let title = gtk4::Label::new(Some(&tile.title));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.add_css_class("heading");
    head.append(&title);
    vbox.append(&head);

    if !tile.directory.is_empty() {
        let dir = gtk4::Label::new(Some(&tile.directory));
        dir.set_xalign(0.0);
        dir.add_css_class("dim-label");
        dir.add_css_class("caption");
        dir.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        vbox.append(&dir);
    }

    let activity = gtk4::Label::new(Some(if tile.activity.is_empty() {
        "—"
    } else {
        &tile.activity
    }));
    activity.set_xalign(0.0);
    activity.set_yalign(0.0);
    activity.set_vexpand(true);
    activity.set_wrap(true);
    activity.set_lines(2);
    activity.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    activity.add_css_class("dim-label");
    activity.add_css_class("monospace");
    activity.add_css_class("caption");
    vbox.append(&activity);

    let button = gtk4::Button::new();
    button.set_child(Some(&vbox));
    button.add_css_class("overview-tile-button");
    if tile.focused {
        button.add_css_class("overview-tile-focused");
    }
    button.set_tooltip_text(Some(&tile.title));

    let panel_id = tile.panel_id;
    let state = Rc::clone(state);
    let dialog = dialog.clone();
    button.connect_clicked(move |_| {
        {
            let mut tm = lock_or_recover(&state.shared.tab_manager);
            if let Some(ws) = tm.selected_mut() {
                ws.focus_panel(panel_id);
            }
        }
        state.shared.notify_ui_refresh();
        dialog.close();
    });

    button.upcast()
}

fn collect_tiles(state: &Rc<AppState>) -> Vec<TileInfo> {
    let tm = lock_or_recover(&state.shared.tab_manager);
    let Some(ws) = tm.selected() else {
        return Vec::new();
    };
    let focused = ws.focused_panel_id;
    let attention = ws.attention_panel_id;

    ws.layout
        .all_panel_ids()
        .into_iter()
        .filter_map(|pid| {
            let panel = ws.panels.get(&pid)?;
            let is_browser = panel.panel_type == PanelType::Browser;

            // Activity snippet + status.
            let (activity, busy) = if panel.panel_type == PanelType::Terminal {
                let activity = state
                    .terminal_cache
                    .borrow()
                    .get(&pid)
                    .and_then(|s| s.read_screen_text())
                    .map(|t| {
                        t.lines()
                            .rev()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or("")
                            .chars()
                            .take(120)
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                (activity, crate::app::pane_is_busy(pid))
            } else if is_browser {
                (panel.browser_url.clone().unwrap_or_default(), None)
            } else {
                (
                    panel.directory.clone().unwrap_or_default(),
                    None,
                )
            };

            let (dot, dot_class) = if attention == Some(pid) {
                ("●", "overview-dot-attention")
            } else if is_browser {
                ("⬤", "overview-dot-browser")
            } else {
                match busy {
                    Some(true) => ("●", "overview-dot-busy"),
                    Some(false) => ("○", "overview-dot-idle"),
                    None => ("•", "overview-dot-idle"),
                }
            };

            Some(TileInfo {
                panel_id: pid,
                title: panel.display_title().to_string(),
                directory: panel.directory.clone().unwrap_or_default(),
                activity,
                icon: icon_for(panel.panel_type),
                dot,
                dot_class,
                focused: focused == Some(pid),
            })
        })
        .collect()
}

fn icon_for(kind: PanelType) -> &'static str {
    match kind {
        PanelType::Terminal => "utilities-terminal-symbolic",
        PanelType::Browser => "globe-symbolic",
        PanelType::Markdown => "document-open-symbolic",
        PanelType::Diff => "media-flash-symbolic",
        PanelType::Project => "view-list-symbolic",
        PanelType::FilePreview => "text-x-generic-symbolic",
        PanelType::Notes => "accessories-text-editor-symbolic",
        PanelType::History => "document-open-recent-symbolic",
        PanelType::Vault => "drive-multidisk-symbolic",
    }
}
