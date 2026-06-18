//! Main application window using AdwNavigationSplitView.

pub(crate) mod dialogs;
mod event_handler;
mod shortcuts;
mod styling;

use std::cell::Cell;
use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::app::{lock_or_recover, AppState, UiEvent};
use crate::model::panel::{Panel, SplitOrientation};
use crate::model::{PanelType, Workspace};
use crate::ui::{notifications_panel, search_overlay, sidebar, split_view};

// Re-export the public dialog function.
pub use dialogs::show_rename_tab_dialog;

/// Create an application window with per-window ID for multi-window support.
pub fn create_window(
    app: &adw::Application,
    state: &Rc<AppState>,
    window_id: uuid::Uuid,
    ui_events: UnboundedReceiver<UiEvent>,
    chromeless: bool,
) -> adw::ApplicationWindow {
    styling::install_css();

    // Use saved window geometry if available, otherwise defaults
    let (width, height) = lock_or_recover(&state.shared.window_sizes)
        .get(&window_id)
        .copied()
        .unwrap_or((1280, 860));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("cmux")
        .default_width(width)
        .default_height(height)
        .build();
    window.set_widget_name(&window_id.to_string());

    let split_view = adw::NavigationSplitView::new();
    let sidebar_settings = &crate::settings::load().sidebar;
    let sidebar_width = if sidebar_settings.width > 0 {
        sidebar_settings.width as f64
    } else {
        280.0
    };
    // Honor the configured width as the actual maximum so it doesn't get forced
    // wider on low-resolution screens. Let it shrink below that on narrow windows.
    split_view.set_min_sidebar_width(sidebar_width.min(180.0));
    split_view.set_max_sidebar_width(sidebar_width);
    split_view.set_vexpand(true);
    split_view.set_hexpand(true);

    let sidebar_widgets = sidebar::create_sidebar(state);
    let list_box = sidebar_widgets.list_box.clone();
    let sidebar_page = adw::NavigationPage::new(&sidebar_widgets.root, "Workspaces");
    split_view.set_sidebar(Some(&sidebar_page));

    let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content_box.set_hexpand(true);
    content_box.set_vexpand(true);
    rebuild_content(&content_box, state, Some(window_id));

    // Search overlay wraps the content area
    let search = search_overlay::create_search_overlay(&content_box.clone().upcast(), state);
    let search_bar = search.search_bar.clone();
    let search_entry = search.entry.clone();
    let search_count_label = search.count_label.clone();
    let search_state = search.state.clone();

    // Dock — right-side terminal controls from dock.json, placed beside the
    // workspace content.
    let dock_dir = {
        let tm = crate::app::lock_or_recover(&state.shared.tab_manager);
        tm.selected()
            .map(|ws| ws.current_directory.clone())
            .unwrap_or_default()
    };
    let dock_box = crate::ui::dock::create_dock(window_id, &dock_dir, state);
    let content_with_dock = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    content_with_dock.set_hexpand(true);
    content_with_dock.set_vexpand(true);
    search.overlay.set_hexpand(true);
    content_with_dock.append(&search.overlay);
    content_with_dock.append(&dock_box);

    let content_page = adw::NavigationPage::new(&content_with_dock, "Terminal");
    split_view.set_content(Some(&content_page));

    // Notification panel — replaces sidebar when toggled
    let notif_panel = notifications_panel::create_notifications_panel(state);
    let notif_root = notif_panel.root.clone();
    let notif_page = adw::NavigationPage::new(&notif_root, "Notifications");
    let showing_notifications: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // Toast overlay must be created before binding events so it can be passed in
    let toast_overlay = adw::ToastOverlay::new();

    let header = adw::HeaderBar::new();
    // Minimal mode hides the header bar entirely for a distraction-free terminal.
    if crate::settings::load().minimal_mode {
        header.set_visible(false);
    }
    // A chromeless (quick-terminal) window keeps the full UI — sidebar, header,
    // tabs and global buttons — but drops the OS-style window controls
    // (close/maximize), since a layer-shell drop-down isn't a normal window.
    if chromeless {
        header.set_show_start_title_buttons(false);
        header.set_show_end_title_buttons(false);
    }

    bind_sidebar_selection(&list_box, &content_box, state);
    event_handler::bind_shared_state_updates(
        &list_box,
        &content_box,
        &window,
        state,
        ui_events,
        &split_view,
        &sidebar_page,
        &notif_page,
        &showing_notifications,
        &notif_panel,
        &toast_overlay,
        &header,
    );
    let initial_title = {
        let tm = lock_or_recover(&state.shared.tab_manager);
        tm.selected()
            .map(|ws| ws.display_title().to_string())
            .unwrap_or_else(|| "cmux".to_string())
    };
    let header_title = gtk4::Label::new(Some(&initial_title));
    header_title.add_css_class("heading");
    header_title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    header.set_title_widget(Some(&header_title));

    let new_ws_btn = gtk4::Button::from_icon_name("tab-new-symbolic");
    new_ws_btn.set_tooltip_text(Some("New Workspace"));
    {
        let state = state.clone();
        let list_box = list_box.clone();
        let content_box = content_box.clone();
        new_ws_btn.connect_clicked(move |_| {
            use crate::settings::PlusButtonAction;
            let settings = crate::settings::load();
            match settings.plus_button_action {
                PlusButtonAction::NewTab => {
                    let mut tm = lock_or_recover(&state.shared.tab_manager);
                    if let Some(ws) = tm.selected_mut() {
                        let new_panel = Panel::new_terminal();
                        let new_id = new_panel.id;
                        if let Some(focused_id) = ws.focused_panel_id {
                            ws.layout.add_panel_to_pane(focused_id, new_id);
                        }
                        ws.panels.insert(new_id, new_panel);
                        ws.previous_focused_panel_id = ws.focused_panel_id;
                        ws.focused_panel_id = Some(new_id);
                    }
                    drop(tm);
                    state.shared.notify_ui_refresh();
                }
                PlusButtonAction::NewWorkspace => {
                    let placement = settings.new_workspace_placement;
                    let workspace = if settings.workspace_cwd_inheritance {
                        let cwd = lock_or_recover(&state.shared.tab_manager)
                            .selected()
                            .map(|ws| ws.current_directory.clone())
                            .unwrap_or_default();
                        if cwd.is_empty() {
                            Workspace::new()
                        } else {
                            Workspace::with_directory(&cwd)
                        }
                    } else {
                        Workspace::new()
                    };
                    lock_or_recover(&state.shared.tab_manager)
                        .add_workspace_with_placement(workspace, placement);
                    refresh_ui(&list_box, &content_box, &state);
                }
            }
        });
    }
    header.pack_start(&new_ws_btn);

    let split_h_btn = gtk4::Button::from_icon_name("view-dual-symbolic");
    split_h_btn.set_tooltip_text(Some("Split Horizontal"));
    {
        let state = state.clone();
        let list_box = list_box.clone();
        let content_box = content_box.clone();
        split_h_btn.connect_clicked(move |_| {
            if let Some(workspace) = lock_or_recover(&state.shared.tab_manager).selected_mut() {
                workspace.split(SplitOrientation::Horizontal, PanelType::Terminal);
            }
            refresh_ui(&list_box, &content_box, &state);
        });
    }
    header.pack_start(&split_h_btn);

    let split_v_btn = gtk4::Button::from_icon_name("view-paged-symbolic");
    split_v_btn.set_tooltip_text(Some("Split Vertical"));
    {
        let state = state.clone();
        let list_box = list_box.clone();
        let content_box = content_box.clone();
        split_v_btn.connect_clicked(move |_| {
            if let Some(workspace) = lock_or_recover(&state.shared.tab_manager).selected_mut() {
                workspace.split(SplitOrientation::Vertical, PanelType::Terminal);
            }
            refresh_ui(&list_box, &content_box, &state);
        });
    }
    header.pack_start(&split_v_btn);

    // Settings button (right side of header bar)
    let settings_btn = gtk4::Button::from_icon_name("preferences-system-symbolic");
    settings_btn.set_tooltip_text(Some("Settings"));
    settings_btn.add_css_class("flat");
    {
        let window_ref = window.clone();
        let list_box = list_box.clone();
        let content_box = content_box.clone();
        let state = Rc::clone(state);
        settings_btn.connect_clicked(move |_| {
            let lb = list_box.clone();
            let cb = content_box.clone();
            let st = Rc::clone(&state);
            super::settings::show_settings(&window_ref, move || {
                refresh_ui(&lb, &cb, &st);
            });
        });
    }
    header.pack_end(&settings_btn);

    // Dock toggle button — reliable even when a terminal grabs the keyboard
    // shortcut (Kitty keyboard protocol).
    let dock_btn = gtk4::Button::from_icon_name("sidebar-show-right-symbolic");
    dock_btn.set_tooltip_text(Some("Toggle Dock"));
    dock_btn.add_css_class("flat");
    {
        let state = Rc::clone(state);
        dock_btn.connect_clicked(move |_| {
            let dir = {
                let tm = crate::app::lock_or_recover(&state.shared.tab_manager);
                tm.selected()
                    .map(|ws| ws.current_directory.clone())
                    .unwrap_or_default()
            };
            crate::ui::dock::toggle(window_id, &dir, &state);
        });
    }
    header.pack_end(&dock_btn);

    // Pane overview button.
    let overview_btn = gtk4::Button::from_icon_name("view-grid-symbolic");
    overview_btn.set_tooltip_text(Some("Pane overview"));
    overview_btn.add_css_class("flat");
    {
        let state = Rc::clone(state);
        let window_ref = window.clone();
        overview_btn.connect_clicked(move |_| {
            crate::ui::pane_overview::show_pane_overview(&window_ref, &state);
        });
    }
    header.pack_end(&overview_btn);

    let outer_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    outer_box.append(&header);
    outer_box.append(&split_view);

    toast_overlay.set_child(Some(&outer_box));
    window.set_content(Some(&toast_overlay));
    shortcuts::setup_shortcuts(
        &window,
        state,
        &list_box,
        &content_box,
        &search_bar,
        &search_entry,
        &search_count_label,
        &search_state,
        &split_view,
        &sidebar_page,
        &notif_page,
        &showing_notifications,
        &notif_panel,
        &header,
    );

    {
        let state = state.clone();
        window.connect_is_active_notify(move |window| {
            let active = window.is_active();
            if let Some(app) = state.ghostty_app.borrow().as_ref() {
                app.set_focus(active);
            }
        });
    }

    // Close/quit handling
    {
        let state = state.clone();
        window.connect_close_request(move |window| {
            let wid = uuid::Uuid::parse_str(&window.widget_name()).ok();

            // Stop all browser WebViews to prevent WebProcess segfault
            // during shutdown (active content like YouTube embeds can
            // crash if torn down abruptly).
            #[cfg(feature = "webkit")]
            super::browser_panel::stop_all_webviews();

            // Clean up per-window state
            if let Some(ref wid) = wid {
                state.shared.remove_ui_event_sender(wid);
                lock_or_recover(&state.shared.window_sizes).remove(wid);
            }

            // If other windows exist, just close this one without confirmation
            let other_windows = window
                .application()
                .map(|app| app.windows().len() > 1)
                .unwrap_or(false);
            if other_windows {
                return glib::Propagation::Proceed;
            }

            // Last window — check for quit confirmation
            let settings = crate::settings::load();
            if !settings.confirm_before_close || !settings.confirm_quit {
                return glib::Propagation::Proceed;
            }

            let terminal_count = {
                let tm = lock_or_recover(&state.shared.tab_manager);
                tm.iter()
                    .flat_map(|ws| ws.panels.values())
                    .filter(|p| p.panel_type == PanelType::Terminal)
                    .count()
            };

            if terminal_count == 0 {
                return glib::Propagation::Proceed;
            }

            let dialog = adw::MessageDialog::new(Some(window), Some("Quit cmux?"), None);
            dialog.add_css_class("cmux-confirm-dialog");
            dialog.set_body(&format!(
                "There {} still active. Are you sure you want to quit?",
                if terminal_count == 1 {
                    "is 1 terminal".to_string()
                } else {
                    format!("are {terminal_count} terminals")
                }
            ));
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("quit", "Quit");
            dialog.set_default_response(Some("cancel"));
            dialog.set_response_appearance("quit", adw::ResponseAppearance::Destructive);

            let window = window.clone();
            dialog.connect_response(None, move |_, response| {
                if response == "quit" {
                    window.destroy();
                }
            });

            dialog.present();
            glib::Propagation::Stop
        });
    }

    window
}

/// Rebuild the content area from the current workspace layout.
/// The cmux window id (stored as the window's widget name) a widget lives in.
fn window_id_of(widget: &gtk4::Widget) -> Option<uuid::Uuid> {
    widget
        .root()
        .and_then(|r| r.downcast::<adw::ApplicationWindow>().ok())
        .and_then(|w| uuid::Uuid::parse_str(&w.widget_name()).ok())
}

pub fn rebuild_content(content_box: &gtk4::Box, state: &Rc<AppState>, window_id: Option<uuid::Uuid>) {
    tracing::debug!("rebuild_content triggered");

    // Which window are we rendering for? Use the explicit id, else derive it
    // from the content box's root window. Lets each window render its own
    // workspace and avoid stealing GL surfaces that live in other windows.
    let win_id = window_id.or_else(|| window_id_of(content_box.upcast_ref::<gtk4::Widget>()));
    let same_window = |w: &gtk4::Widget| -> bool {
        match (win_id, window_id_of(w)) {
            (Some(ours), Some(theirs)) => ours == theirs,
            _ => true,
        }
    };

    // Unparent cached GL surfaces and browser widgets that belong to this window.
    for surface in state.terminal_cache.borrow().values() {
        if let Some(parent) = surface.parent() {
            if !same_window(surface.upcast_ref::<gtk4::Widget>()) {
                continue;
            }
            if let Ok(parent_box) = parent.downcast::<gtk4::Box>() {
                parent_box.remove(surface);
            }
        }
    }
    for browser_widget in state.browser_cache.borrow().values() {
        if browser_widget.parent().is_some() && same_window(browser_widget.upcast_ref::<gtk4::Widget>())
        {
            browser_widget.unparent();
        }
    }

    // Remove all children from the content box.
    while let Some(child) = content_box.first_child() {
        content_box.remove(&child);
    }

    // Clone workspace data out of the lock so we don't hold it during widget construction.
    let workspace_data = {
        let tab_manager = lock_or_recover(&state.shared.tab_manager);
        let selected = match win_id {
            Some(wid) => tab_manager.selected_for_window(wid),
            None => tab_manager.selected(),
        };
        selected.map(|ws| {
            (
                ws.id,
                ws.layout.clone(),
                ws.panels.clone(),
                ws.attention_panel_id,
                ws.zoomed_panel_id,
                ws.focused_panel_id,
            )
        })
    };

    if let Some((id, layout, panels, attention_panel_id, zoomed_panel_id, focused_panel_id)) =
        workspace_data
    {
        let effective_attention = if crate::settings::load().pane_attention_ring {
            attention_panel_id
        } else {
            None
        };
        let widget = if let Some(zoomed_id) = zoomed_panel_id {
            split_view::build_zoomed(zoomed_id, &panels, state)
        } else {
            split_view::build_layout(
                id,
                &layout,
                &panels,
                effective_attention,
                focused_panel_id,
                state,
            )
        };
        content_box.append(&widget);
    } else if super::welcome::should_show_welcome() {
        content_box.append(&super::welcome::build_welcome());
    } else {
        let label = gtk4::Label::new(Some("No workspace selected"));
        label.add_css_class("dim-label");
        content_box.append(&label);
    }
}

fn refresh_ui(list_box: &gtk4::ListBox, content_box: &gtk4::Box, state: &Rc<AppState>) {
    state.prune_terminal_cache();
    state.shared.cleanup_stale_remote_sessions();
    sidebar::refresh_sidebar(list_box, state);
    rebuild_content(content_box, state, None);
    update_window_title(content_box, state);
}

/// Lightweight refresh for metadata-only changes (title, PWD, git branch).
/// Updates the sidebar and window title without touching the content layout,
/// so browser panels are not unparented/reparented.
pub fn refresh_metadata(list_box: &gtk4::ListBox, content_box: &gtk4::Box, state: &Rc<AppState>) {
    tracing::debug!("refresh_metadata: start");
    sidebar::refresh_sidebar(list_box, state);
    update_focus_visuals(content_box, state);
    update_window_title(content_box, state);
    tracing::debug!("refresh_metadata: done");
}

/// Re-apply focus-dependent visuals (split-region dim + per-pane inactive
/// overlay) in place, without rebuilding the content layout.
///
/// The split-region opacity and the inactive-pane overlays are decided at build
/// time from `focused_panel_id`; on a focus change we only run a metadata
/// refresh (a full rebuild would churn the GLArea and swallow input), so this
/// walks the existing widget tree and updates those visuals directly. Without
/// it, the pane that was focused when the layout was built stays bright while
/// the others stay dimmed regardless of which pane is actually active.
fn update_focus_visuals(content_box: &gtk4::Box, state: &Rc<AppState>) {
    let focused_str = {
        let tm = lock_or_recover(&state.shared.tab_manager);
        tm.selected()
            .and_then(|ws| ws.focused_panel_id)
            .map(|id| id.to_string())
    };
    let unfocused_opacity = state
        .ghostty_ui_config
        .borrow()
        .unfocused_split_opacity
        .map(|o| o.clamp(0.15, 1.0))
        .filter(|&o| o < 1.0);

    let root: gtk4::Widget = content_box.clone().upcast();
    for_each_descendant(&root, &mut |w| {
        // Split-region dim: bright when this region holds the focused pane.
        if w.has_css_class("pane-region") {
            let region_focused = focused_str
                .as_deref()
                .map(|fid| descendant_named(w, fid))
                .unwrap_or(false);
            let opacity = if region_focused {
                1.0
            } else {
                unfocused_opacity.unwrap_or(1.0)
            };
            w.set_opacity(opacity);
        }
        // Per-pane inactive overlay: shown only when the pane is not focused.
        if w.has_css_class("inactive-pane-overlay") {
            if let Some(pid) = overlay_panel_id(w) {
                let is_focused = focused_str.as_deref() == Some(pid.as_str());
                w.set_visible(!is_focused);
            }
        }
        // Per-pane focused border highlight.
        if w.has_css_class("pane-container") {
            let is_focused = focused_str.as_deref() == Some(w.widget_name().as_str());
            if is_focused {
                w.add_css_class("focused-panel");
            } else {
                w.remove_css_class("focused-panel");
            }
        }
    });
}

/// Walk `widget` and all of its descendants, invoking `f` on each.
fn for_each_descendant(widget: &gtk4::Widget, f: &mut dyn FnMut(&gtk4::Widget)) {
    f(widget);
    let mut child = widget.first_child();
    while let Some(c) = child {
        for_each_descendant(&c, f);
        child = c.next_sibling();
    }
}

/// True if `widget` or any descendant has the GTK widget-name `name`.
fn descendant_named(widget: &gtk4::Widget, name: &str) -> bool {
    if widget.widget_name() == name {
        return true;
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if descendant_named(&c, name) {
            return true;
        }
        child = c.next_sibling();
    }
    false
}

/// For an `inactive-pane-overlay` widget, return the panel id it belongs to —
/// the widget-name of the parent `GtkOverlay`'s main child (the pane container).
fn overlay_panel_id(overlay_child: &gtk4::Widget) -> Option<String> {
    let overlay = overlay_child.parent()?.downcast::<gtk4::Overlay>().ok()?;
    let name = overlay.child()?.widget_name();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn update_window_title(content_box: &gtk4::Box, state: &Rc<AppState>) {
    if let Some(root) = content_box.root() {
        if let Some(window) = root.downcast_ref::<adw::ApplicationWindow>() {
            let titles = {
                let tm = lock_or_recover(&state.shared.tab_manager);
                tm.selected().map(|ws| {
                    let title = ws.display_title();
                    let dir = crate::ui::sidebar::compact_path(&ws.current_directory);
                    (format!("{title} — {dir} — cmux"), title.to_string())
                })
            };
            if let Some((full_title, ws_title)) = titles {
                window.set_title(Some(&full_title));
                if let Some(root) = window.content() {
                    // Unwrap ToastOverlay wrapper if present to reach outer_box
                    let outer = root
                        .clone()
                        .downcast::<adw::ToastOverlay>()
                        .ok()
                        .and_then(|ov| ov.child())
                        .unwrap_or(root);
                    if let Some(hb) = outer.first_child() {
                        if let Some(header) = hb.downcast_ref::<adw::HeaderBar>() {
                            if let Some(tw) = header.title_widget() {
                                if let Some(lbl) = tw.downcast_ref::<gtk4::Label>() {
                                    lbl.set_text(&ws_title);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn bind_sidebar_selection(list_box: &gtk4::ListBox, content_box: &gtk4::Box, state: &Rc<AppState>) {
    let state = state.clone();
    let lb = list_box.clone();
    let content_box = content_box.clone();

    list_box.connect_row_selected(move |_list_box, row| {
        let Some(row) = row else {
            return;
        };

        let index = row.index();
        if index < 0 {
            return;
        }
        if event_handler::select_workspace_by_index(&state, index as usize) {
            refresh_ui(&lb, &content_box, &state);
        }
    });
}
