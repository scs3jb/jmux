//! Keyboard shortcut handling for the main application window.

use std::rc::Rc;

use std::cell::Cell;

use gtk4::prelude::*;
use libadwaita as adw;

use crate::app::{lock_or_recover, AppState};
use crate::model::panel::SplitOrientation;
use crate::model::PanelType;
use crate::ui::{notifications_panel, search_overlay};

#[allow(clippy::too_many_arguments)]
pub(super) fn setup_shortcuts(
    window: &adw::ApplicationWindow,
    state: &Rc<AppState>,
    list_box: &gtk4::ListBox,
    content_box: &gtk4::Box,
    search_bar: &gtk4::Box,
    search_entry: &gtk4::SearchEntry,
    search_count_label: &gtk4::Label,
    search_state: &Rc<search_overlay::SearchState>,
    nav_split_view: &adw::NavigationSplitView,
    sidebar_page: &adw::NavigationPage,
    notif_page: &adw::NavigationPage,
    showing_notifications: &Rc<Cell<bool>>,
    notif_panel: &notifications_panel::NotificationsPanel,
    header: &adw::HeaderBar,
) {
    let controller = gtk4::EventControllerKey::new();

    let state = state.clone();
    let list_box = list_box.clone();
    let content_box = content_box.clone();
    let search_bar = search_bar.clone();
    let search_entry = search_entry.clone();
    let _search_count_label = search_count_label.clone();
    let _search_state = search_state.clone();
    let nav_split_view = nav_split_view.clone();
    let sidebar_page = sidebar_page.clone();
    let notif_page = notif_page.clone();
    let showing_notifications = showing_notifications.clone();
    let notif_panel = notif_panel.clone();
    let header = header.clone();
    let window_weak = window.downgrade();

    controller.connect_key_pressed(move |_controller, keyval, _keycode, modifier| {
        let ctrl = modifier.contains(gdk4::ModifierType::CONTROL_MASK);
        let shift = modifier.contains(gdk4::ModifierType::SHIFT_MASK);
        let alt = modifier.contains(gdk4::ModifierType::ALT_MASK);

        // Check user-configurable shortcuts (notification, tab, find-in-directory).
        {
            let shortcuts = crate::settings::shortcuts::load();
            let key_name = keyval.name().map(|n| n.to_string()).unwrap_or_default();
            // Build the current context for "when" clause evaluation (lock is
            // released before the action bodies, which re-lock the tab manager).
            let when_ctx = {
                use crate::model::PanelType;
                let tm = lock_or_recover(&state.shared.tab_manager);
                if let Some(ws) = tm.selected() {
                    let pt = ws
                        .focused_panel_id
                        .and_then(|pid| ws.panels.get(&pid))
                        .map(|p| p.panel_type);
                    crate::settings::shortcuts::ShortcutContext {
                        terminal_focused: matches!(pt, Some(PanelType::Terminal)),
                        browser_focused: matches!(pt, Some(PanelType::Browser)),
                        editor_focused: matches!(
                            pt,
                            Some(PanelType::Notes) | Some(PanelType::Markdown)
                        ),
                        pane_zoomed: ws.zoomed_panel_id.is_some(),
                    }
                } else {
                    crate::settings::shortcuts::ShortcutContext::default()
                }
            };
            let configurable_actions = [
                "notification.defer_unread",
                "notification.toggle_unread",
                "tab.new",
                "tab.reopen",
                "textbox.focus",
                "notes.open",
                "dock.toggle",
                "close.tab",
                "close.tab.others",
                "find.in_directory",
            ];
            for action in &configurable_actions {
                if let Some(binding) = shortcuts.get(action) {
                    if binding.ctrl == ctrl
                        && binding.shift == shift
                        && binding.alt == alt
                        && binding.key == key_name
                        && shortcuts.when_allows(action, &when_ctx)
                    {
                        match *action {
                            "notification.defer_unread" => {
                                state
                                    .shared
                                    .send_ui_event(crate::app::UiEvent::DeferUnread);
                            }
                            "notification.toggle_unread" => {
                                state
                                    .shared
                                    .send_ui_event(crate::app::UiEvent::ToggleUnread);
                            }
                            // New tab (terminal) in the current pane, inheriting
                            // the focused terminal's working directory.
                            "tab.new" => {
                                {
                                    let mut tm = lock_or_recover(&state.shared.tab_manager);
                                    if let Some(ws) = tm.selected_mut() {
                                        let mut panel = crate::model::Panel::new_terminal();
                                        panel.directory = crate::app::new_tab_directory(ws);
                                        let new_id = panel.id;
                                        ws.panels.insert(new_id, panel);
                                        let target = ws.focused_panel_id.or_else(|| {
                                            ws.layout.all_panel_ids().into_iter().next()
                                        });
                                        if let Some(target) = target {
                                            ws.layout.add_panel_to_pane(target, new_id);
                                        }
                                        ws.previous_focused_panel_id = ws.focused_panel_id;
                                        ws.focused_panel_id = Some(new_id);
                                    }
                                }
                                super::refresh_ui(&list_box, &content_box, &state);
                            }
                            // Open the pane overview grid.
                            "overview.open" => {
                                if let Some(window) = window_weak.upgrade() {
                                    crate::ui::pane_overview::show_pane_overview(&window, &state);
                                }
                            }
                            // Reopen the most recently closed tab.
                            "tab.reopen" => {
                                {
                                    let mut tm = lock_or_recover(&state.shared.tab_manager);
                                    if let Some(ws) = tm.selected_mut() {
                                        ws.reopen_last_closed_panel();
                                    }
                                }
                                super::refresh_ui(&list_box, &content_box, &state);
                            }
                            // Focus the TextBox composer of the focused panel.
                            "textbox.focus" => {
                                let panel_id = {
                                    let tm = lock_or_recover(&state.shared.tab_manager);
                                    tm.selected().and_then(|ws| ws.focused_panel_id)
                                };
                                if let Some(panel_id) = panel_id {
                                    crate::ui::textbox::focus_textbox(panel_id);
                                }
                            }
                            // Open the scope-grouped Notes panel beside the pane.
                            "notes.open" => {
                                crate::ui::command_palette::insert_notes_panel(&state);
                                super::refresh_ui(&list_box, &content_box, &state);
                            }
                            // Toggle the Dock panel for this window.
                            "dock.toggle" => {
                                if let Some(window) = window_weak.upgrade() {
                                    if let Ok(wid) =
                                        uuid::Uuid::parse_str(&window.widget_name())
                                    {
                                        let dir = {
                                            let tm = lock_or_recover(&state.shared.tab_manager);
                                            tm.selected()
                                                .map(|ws| ws.current_directory.clone())
                                                .unwrap_or_default()
                                        };
                                        crate::ui::dock::toggle(wid, &dir, &state);
                                    }
                                }
                            }
                            // Close the focused panel (browser-style Ctrl+W).
                            "close.tab" => {
                                let closed = {
                                    let mut tm = lock_or_recover(&state.shared.tab_manager);
                                    match tm.selected().and_then(|ws| ws.focused_panel_id) {
                                        Some(panel_id) => tm.close_panel(panel_id),
                                        None => false,
                                    }
                                };
                                if closed {
                                    super::request_terminal_focus();
                                    super::refresh_ui(&list_box, &content_box, &state);
                                }
                            }
                            // Close all other panels in the same pane, keeping focus.
                            "close.tab.others" => {
                                let closed = {
                                    let mut tm = lock_or_recover(&state.shared.tab_manager);
                                    if let Some(ws) = tm.selected_mut() {
                                        if let Some(panel_id) = ws.focused_panel_id {
                                            let pane_ids =
                                                ws.layout.find_pane_with_panel_readonly(panel_id);
                                            if let Some(pane_ids) = pane_ids {
                                                let to_close: Vec<uuid::Uuid> = pane_ids
                                                    .iter()
                                                    .filter(|&&pid| pid != panel_id)
                                                    .copied()
                                                    .collect();
                                                for pid in &to_close {
                                                    ws.panels.remove(pid);
                                                    ws.layout.remove_panel(*pid);
                                                }
                                                !to_close.is_empty()
                                            } else {
                                                false
                                            }
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    }
                                };
                                if closed {
                                    super::refresh_ui(&list_box, &content_box, &state);
                                }
                            }
                            // find.in_directory: focus the sidebar file-explorer search entry.
                            // TODO: wire when a file-explorer search entry is added to the sidebar.
                            "find.in_directory" => {
                                tracing::debug!("find.in_directory — file explorer search not yet wired");
                            }
                            _ => {}
                        }
                        return glib::Propagation::Stop;
                    }
                }
            }
        }

        // Alt+Arrow: Directional pane focus
        if alt && !ctrl && !shift {
            use crate::model::panel::Direction;
            let direction = match keyval {
                gdk4::Key::Left => Some(Direction::Left),
                gdk4::Key::Right => Some(Direction::Right),
                gdk4::Key::Up => Some(Direction::Up),
                gdk4::Key::Down => Some(Direction::Down),
                _ => None,
            };
            if let Some(dir) = direction {
                let changed = {
                    let mut tm = lock_or_recover(&state.shared.tab_manager);
                    if let Some(ws) = tm.selected_mut() {
                        if let Some(current) = ws.focused_panel_id {
                            if let Some(neighbor) = ws.layout.neighbor(current, dir) {
                                ws.focus_panel(neighbor)
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if changed {
                    super::refresh_ui(&list_box, &content_box, &state);
                }
                return glib::Propagation::Stop;
            }
        }

        // Ctrl+Alt combinations (no shift)
        if ctrl && alt && !shift {
            // Workspace color presets: Ctrl+Alt+0..9
            // 0 = reset to default, 1-9 = preset colors
            const PRESET_COLORS: &[&str] = &[
                "#ef4444", // 1 red
                "#f97316", // 2 orange
                "#eab308", // 3 yellow
                "#22c55e", // 4 green
                "#14b8a6", // 5 teal
                "#3b82f6", // 6 blue
                "#8b5cf6", // 7 purple
                "#ec4899", // 8 pink
                "#64748b", // 9 slate
            ];
            let color_index: Option<usize> = match keyval {
                gdk4::Key::_0 => Some(0),
                gdk4::Key::_1 => Some(1),
                gdk4::Key::_2 => Some(2),
                gdk4::Key::_3 => Some(3),
                gdk4::Key::_4 => Some(4),
                gdk4::Key::_5 => Some(5),
                gdk4::Key::_6 => Some(6),
                gdk4::Key::_7 => Some(7),
                gdk4::Key::_8 => Some(8),
                gdk4::Key::_9 => Some(9),
                _ => None,
            };
            if let Some(idx) = color_index {
                let new_color = if idx == 0 {
                    None
                } else {
                    PRESET_COLORS.get(idx - 1).map(|s| s.to_string())
                };
                {
                    let mut tm = lock_or_recover(&state.shared.tab_manager);
                    if let Some(ws) = tm.selected_mut() {
                        ws.custom_color = new_color;
                    }
                }
                super::refresh_ui(&list_box, &content_box, &state);
                return glib::Propagation::Stop;
            }

            #[cfg(feature = "webkit")]
            match keyval {
                // Ctrl+Alt+D: Split browser horizontal
                gdk4::Key::d => {
                    if let Some(workspace) =
                        lock_or_recover(&state.shared.tab_manager).selected_mut()
                    {
                        workspace.split(SplitOrientation::Horizontal, PanelType::Browser);
                    }
                    super::refresh_ui(&list_box, &content_box, &state);
                    return glib::Propagation::Stop;
                }
                // Ctrl+Alt+E: Split browser vertical
                gdk4::Key::e => {
                    if let Some(workspace) =
                        lock_or_recover(&state.shared.tab_manager).selected_mut()
                    {
                        workspace.split(SplitOrientation::Vertical, PanelType::Browser);
                    }
                    super::refresh_ui(&list_box, &content_box, &state);
                    return glib::Propagation::Stop;
                }
                // Ctrl+Alt+C: Toggle browser JS console
                gdk4::Key::c => {
                    let panel_id = {
                        let tm = lock_or_recover(&state.shared.tab_manager);
                        tm.selected().and_then(|ws| {
                            ws.focused_panel_id.and_then(|pid| {
                                ws.panels.get(&pid).and_then(|p| {
                                    (p.panel_type == PanelType::Browser).then_some(pid)
                                })
                            })
                        })
                    };
                    if let Some(panel_id) = panel_id {
                        crate::ui::browser_panel::toggle_console(panel_id);
                    }
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
        }

        // Ctrl+Shift+Alt+W: Close other tabs in the current pane
        if ctrl && shift && alt && keyval == gdk4::Key::W {
            let closed = {
                let mut tm = lock_or_recover(&state.shared.tab_manager);
                if let Some(ws) = tm.selected_mut() {
                    if let Some(panel_id) = ws.focused_panel_id {
                        let pane_ids = ws.layout.find_pane_with_panel_readonly(panel_id);
                        if let Some(pane_ids) = pane_ids {
                            let to_close: Vec<uuid::Uuid> = pane_ids
                                .iter()
                                .filter(|&&pid| pid != panel_id)
                                .copied()
                                .collect();
                            for pid in &to_close {
                                ws.panels.remove(pid);
                                ws.layout.remove_panel(*pid);
                            }
                            !to_close.is_empty()
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            if closed {
                super::refresh_ui(&list_box, &content_box, &state);
            }
            return glib::Propagation::Stop;
        }

        match (keyval, ctrl, shift) {
            // Ctrl+Comma: Settings
            (gdk4::Key::comma, true, false) => {
                if let Some(window) = window_weak.upgrade() {
                    let lb = list_box.clone();
                    let cb = content_box.clone();
                    let st = Rc::clone(&state);
                    crate::ui::settings::show_settings(&window, move || {
                        super::refresh_ui(&lb, &cb, &st);
                    });
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+Comma: Reload ghostty config
            (gdk4::Key::comma, true, true) => {
                state
                    .shared
                    .send_ui_event(crate::app::UiEvent::ReloadConfig);
                glib::Propagation::Stop
            }
            // Ctrl+Shift+B: Toggle minimal mode (hide/show titlebar)
            (gdk4::Key::b, true, true) => {
                let visible = !header.is_visible();
                header.set_visible(visible);
                let mut settings = crate::settings::load();
                settings.minimal_mode = !visible;
                let _ = crate::settings::save(&settings);
                glib::Propagation::Stop
            }
            // Ctrl+F: Toggle terminal find, or open browser find if browser panel is focused
            (gdk4::Key::f, true, false) => {
                // Check if the focused panel is a browser — if so, delegate to
                // the browser's built-in find bar rather than the terminal search overlay.
                let focused_is_browser = {
                    let tm = lock_or_recover(&state.shared.tab_manager);
                    tm.selected().is_some_and(|ws| {
                        ws.focused_panel_id.is_some_and(|pid| {
                            ws.panels
                                .get(&pid)
                                .is_some_and(|p| p.panel_type == PanelType::Browser)
                        })
                    })
                };
                #[cfg(feature = "webkit")]
                if focused_is_browser {
                    let panel_id = {
                        let tm = lock_or_recover(&state.shared.tab_manager);
                        tm.selected().and_then(|ws| ws.focused_panel_id)
                    };
                    if let Some(pid) = panel_id {
                        crate::ui::browser_panel::toggle_browser_find(pid);
                    }
                    return glib::Propagation::Stop;
                }
                if !focused_is_browser {
                    if search_bar.is_visible() {
                        search_bar.set_visible(false);
                        // Return focus to terminal content
                        content_box.grab_focus();
                    } else {
                        search_bar.set_visible(true);
                        search_entry.grab_focus();
                    }
                }
                glib::Propagation::Stop
            }
            // Ctrl+E: Use selection for Find — reads primary selection into find bar
            (gdk4::Key::e, true, false) => {
                // On Linux, the primary clipboard holds the current selection
                let search_bar_c = search_bar.clone();
                let search_entry_c = search_entry.clone();
                if let Some(display) = gdk4::Display::default() {
                    let primary = display.primary_clipboard();
                    primary.read_text_async(gio::Cancellable::NONE, move |result| {
                        if let Ok(Some(text)) = result {
                            let text = text.to_string();
                            if !text.is_empty() {
                                if !search_bar_c.is_visible() {
                                    search_bar_c.set_visible(true);
                                }
                                search_entry_c.set_text(&text);
                                search_entry_c.grab_focus();
                            }
                        }
                    });
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+I: Toggle notification panel
            (gdk4::Key::I, true, true) => {
                if showing_notifications.get() {
                    // Switch back to workspaces sidebar
                    nav_split_view.set_sidebar(Some(&sidebar_page));
                    showing_notifications.set(false);
                } else {
                    // Refresh and show notification panel
                    notif_panel.refresh(&state);
                    nav_split_view.set_sidebar(Some(&notif_page));
                    showing_notifications.set(true);
                }
                glib::Propagation::Stop
            }
            // Ctrl+P: All-surfaces search
            (gdk4::Key::p, true, false) => {
                if let Some(window) = window_weak.upgrade() {
                    crate::ui::all_surfaces_search::show_all_surfaces_search(&window, &state);
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+P: Command palette
            (gdk4::Key::P, true, true) => {
                if let Some(window) = window_weak.upgrade() {
                    let lb = list_box.clone();
                    let cb = content_box.clone();
                    let st = state.clone();
                    let on_refresh = Rc::new(move || super::refresh_ui(&lb, &cb, &st));
                    crate::ui::command_palette::show_command_palette(&window, &state, on_refresh);
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+Z: Toggle pane zoom
            (gdk4::Key::Z, true, true) => {
                let changed = {
                    let mut tm = lock_or_recover(&state.shared.tab_manager);
                    if let Some(ws) = tm.selected_mut() {
                        if ws.zoomed_panel_id.is_some() {
                            ws.zoomed_panel_id = None;
                        } else {
                            ws.zoomed_panel_id = ws.focused_panel_id;
                        }
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    super::refresh_ui(&list_box, &content_box, &state);
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+H: Flash focused panel
            (gdk4::Key::H, true, true) => {
                let panel_id = {
                    let tm = lock_or_recover(&state.shared.tab_manager);
                    tm.selected().and_then(|ws| ws.focused_panel_id)
                };
                if let Some(panel_id) = panel_id {
                    state
                        .shared
                        .send_ui_event(crate::app::UiEvent::TriggerFlash { panel_id });
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+B: Toggle sidebar
            (gdk4::Key::B, true, true) => {
                nav_split_view.set_collapsed(!nav_split_view.is_collapsed());
                glib::Propagation::Stop
            }
            // Ctrl+Shift+R: Rename workspace
            (gdk4::Key::R, true, true) => {
                let current_title = {
                    let tm = lock_or_recover(&state.shared.tab_manager);
                    tm.selected().map(|ws| ws.display_title().to_string())
                };
                if let Some(title) = current_title {
                    if let Some(window) = window_weak.upgrade() {
                        super::dialogs::show_rename_dialog(
                            &window,
                            &state,
                            &list_box,
                            &content_box,
                            &title,
                        );
                    }
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+T: New workspace
            // Ctrl+Shift+N: New window
            (gdk4::Key::N, true, true) => {
                state
                    .shared
                    .send_ui_event(crate::app::UiEvent::CreateWindow);
                glib::Propagation::Stop
            }
            // Ctrl+Shift+W: Close workspace
            (gdk4::Key::W, true, true) => {
                let mut tab_manager = lock_or_recover(&state.shared.tab_manager);
                if let Some(index) = tab_manager.selected_index() {
                    tab_manager.remove(index);
                }
                drop(tab_manager);
                super::refresh_ui(&list_box, &content_box, &state);
                glib::Propagation::Stop
            }
            // Ctrl+Shift+Q: Close focused pane
            (gdk4::Key::Q, true, true) => {
                let closed = {
                    let mut tm = lock_or_recover(&state.shared.tab_manager);
                    let panel_id = tm.selected().and_then(|ws| ws.focused_panel_id);
                    if let Some(panel_id) = panel_id {
                        // Capture browser URL before closing.
                        if let Some(ws) = tm.selected() {
                            if let Some(panel) = ws.panels.get(&panel_id) {
                                if panel.panel_type == PanelType::Browser {
                                    if let Some(ref url) = panel.browser_url {
                                        state.shared.push_closed_browser_url(url.clone());
                                    }
                                }
                            }
                        }
                        tm.close_panel(panel_id)
                    } else {
                        false
                    }
                };
                if closed {
                    super::request_terminal_focus();
                    super::refresh_ui(&list_box, &content_box, &state);
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+D: Split horizontal
            (gdk4::Key::D, true, true) => {
                if let Some(workspace) = lock_or_recover(&state.shared.tab_manager).selected_mut() {
                    workspace.split(SplitOrientation::Horizontal, PanelType::Terminal);
                }
                super::refresh_ui(&list_box, &content_box, &state);
                glib::Propagation::Stop
            }
            // Ctrl+Shift+E: Split vertical
            (gdk4::Key::E, true, true) => {
                if let Some(workspace) = lock_or_recover(&state.shared.tab_manager).selected_mut() {
                    workspace.split(SplitOrientation::Vertical, PanelType::Terminal);
                }
                super::refresh_ui(&list_box, &content_box, &state);
                glib::Propagation::Stop
            }
            // Ctrl+Shift+O: Open workspace directory in file manager
            (gdk4::Key::O, true, true) => {
                let dir = {
                    let tm = lock_or_recover(&state.shared.tab_manager);
                    tm.selected().map(|ws| ws.current_directory.clone())
                };
                if let Some(dir) = dir {
                    let path = if dir.is_empty() {
                        std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
                    } else {
                        dir
                    };
                    let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                }
                glib::Propagation::Stop
            }
            // Ctrl+O: Open folder as new workspace (folder picker)
            (gdk4::Key::o, true, false) => {
                state
                    .shared
                    .send_ui_event(crate::app::UiEvent::OpenFolderAsWorkspace);
                glib::Propagation::Stop
            }
            // Ctrl+Shift+Y: Reopen last closed browser panel
            (gdk4::Key::Y, true, true) => {
                state
                    .shared
                    .send_ui_event(crate::app::UiEvent::ReopenClosedBrowser);
                glib::Propagation::Stop
            }
            // Ctrl+Shift+A: Open Task Manager
            (gdk4::Key::A, true, true) => {
                state
                    .shared
                    .send_ui_event(crate::app::UiEvent::OpenTaskManager);
                glib::Propagation::Stop
            }
            // Ctrl+Shift+M: Enter terminal copy mode
            (gdk4::Key::M, true, true) => {
                let panel_id = {
                    let tm = lock_or_recover(&state.shared.tab_manager);
                    tm.selected().and_then(|ws| ws.focused_panel_id)
                };
                if let Some(panel_id) = panel_id {
                    state
                        .shared
                        .send_ui_event(crate::app::UiEvent::CopyMode { panel_id });
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+U: Jump to latest unread
            (gdk4::Key::U, true, true) => {
                if super::event_handler::select_latest_unread(&state) {
                    super::refresh_ui(&list_box, &content_box, &state);
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+[: Focus previous pane
            (gdk4::Key::bracketleft, true, true) => {
                let changed = {
                    let mut tm = lock_or_recover(&state.shared.tab_manager);
                    if let Some(ws) = tm.selected_mut() {
                        if let Some(current) = ws.focused_panel_id {
                            if let Some(prev) = ws.layout.prev_panel_id(current) {
                                ws.focus_panel(prev)
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if changed {
                    super::refresh_ui(&list_box, &content_box, &state);
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+]: Focus next pane
            (gdk4::Key::bracketright, true, true) => {
                let changed = {
                    let mut tm = lock_or_recover(&state.shared.tab_manager);
                    if let Some(ws) = tm.selected_mut() {
                        if let Some(current) = ws.focused_panel_id {
                            if let Some(next) = ws.layout.next_panel_id(current) {
                                ws.focus_panel(next)
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if changed {
                    super::refresh_ui(&list_box, &content_box, &state);
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+PageUp: Move workspace up
            (gdk4::Key::Page_Up, true, true) => {
                let mut tm = lock_or_recover(&state.shared.tab_manager);
                if let Some(idx) = tm.selected_index() {
                    if idx > 0 {
                        tm.move_workspace(idx, idx - 1);
                    }
                }
                drop(tm);
                super::refresh_ui(&list_box, &content_box, &state);
                glib::Propagation::Stop
            }
            // Ctrl+Shift+PageDown: Move workspace down
            (gdk4::Key::Page_Down, true, true) => {
                let mut tm = lock_or_recover(&state.shared.tab_manager);
                if let Some(idx) = tm.selected_index() {
                    if idx + 1 < tm.len() {
                        tm.move_workspace(idx, idx + 1);
                    }
                }
                drop(tm);
                super::refresh_ui(&list_box, &content_box, &state);
                glib::Propagation::Stop
            }
            // Ctrl+Tab: Next workspace
            (gdk4::Key::Tab, true, false) => {
                let mut tm = lock_or_recover(&state.shared.tab_manager);
                tm.select_next(true);
                let ws_id = tm.selected_id();
                drop(tm);
                if let Some(workspace_id) = ws_id {
                    super::event_handler::mark_workspace_read(&state, workspace_id);
                }
                super::request_terminal_focus();
                super::refresh_ui(&list_box, &content_box, &state);
                glib::Propagation::Stop
            }
            // Ctrl+Shift+Tab: Previous workspace
            (gdk4::Key::ISO_Left_Tab, true, true) => {
                let mut tm = lock_or_recover(&state.shared.tab_manager);
                tm.select_previous(true);
                let ws_id = tm.selected_id();
                drop(tm);
                if let Some(workspace_id) = ws_id {
                    super::event_handler::mark_workspace_read(&state, workspace_id);
                }
                super::request_terminal_focus();
                super::refresh_ui(&list_box, &content_box, &state);
                glib::Propagation::Stop
            }
            // Ctrl+1-9: Select workspace by number
            (keyval, true, false)
                if matches!(
                    keyval,
                    gdk4::Key::_1
                        | gdk4::Key::_2
                        | gdk4::Key::_3
                        | gdk4::Key::_4
                        | gdk4::Key::_5
                        | gdk4::Key::_6
                        | gdk4::Key::_7
                        | gdk4::Key::_8
                        | gdk4::Key::_9
                ) =>
            {
                let index = match keyval {
                    gdk4::Key::_1 => 0,
                    gdk4::Key::_2 => 1,
                    gdk4::Key::_3 => 2,
                    gdk4::Key::_4 => 3,
                    gdk4::Key::_5 => 4,
                    gdk4::Key::_6 => 5,
                    gdk4::Key::_7 => 6,
                    gdk4::Key::_8 => 7,
                    gdk4::Key::_9 => 8,
                    _ => unreachable!(),
                };
                if super::event_handler::select_workspace_by_index(&state, index) {
                    super::request_terminal_focus();
                    super::refresh_ui(&list_box, &content_box, &state);
                }
                glib::Propagation::Stop
            }
            // Ctrl+K: Clear screen + scrollback
            (gdk4::Key::k, true, false) => {
                let panel_id = {
                    let tm = lock_or_recover(&state.shared.tab_manager);
                    tm.selected().and_then(|ws| ws.focused_panel_id)
                };
                if let Some(panel_id) = panel_id {
                    state
                        .shared
                        .send_ui_event(crate::app::UiEvent::ClearHistory { panel_id });
                }
                glib::Propagation::Stop
            }
            // Ctrl+G: Find next match
            (gdk4::Key::g, true, false) => {
                if search_bar.is_visible() {
                    search_overlay::trigger_find_next(&state, &search_entry);
                }
                glib::Propagation::Stop
            }
            // Ctrl+Shift+G: Find previous match
            (gdk4::Key::G, true, true) => {
                if search_bar.is_visible() {
                    search_overlay::trigger_find_prev(&state, &search_entry);
                }
                glib::Propagation::Stop
            }
            // Ctrl+Equal/Plus: Increase font size (terminal) or zoom (browser)
            (gdk4::Key::equal, true, false) | (gdk4::Key::plus, true, _) => {
                let info = {
                    let tm = lock_or_recover(&state.shared.tab_manager);
                    tm.selected().and_then(|ws| {
                        ws.focused_panel_id
                            .and_then(|pid| ws.panels.get(&pid).map(|p| (pid, p.panel_type)))
                    })
                };
                if let Some((panel_id, panel_type)) = info {
                    if panel_type == PanelType::Browser {
                        #[cfg(feature = "webkit")]
                        state
                            .shared
                            .send_ui_event(crate::app::UiEvent::BrowserAction {
                                panel_id,
                                action: crate::ui::browser_panel::BrowserActionKind::ZoomIn,
                            });
                    } else if let Some(surface) = state.terminal_cache.borrow().get(&panel_id) {
                        surface.binding_action("increase_font_size:1");
                    }
                }
                glib::Propagation::Stop
            }
            // Ctrl+Minus: Decrease font size (terminal) or zoom (browser)
            (gdk4::Key::minus, true, false) => {
                let info = {
                    let tm = lock_or_recover(&state.shared.tab_manager);
                    tm.selected().and_then(|ws| {
                        ws.focused_panel_id
                            .and_then(|pid| ws.panels.get(&pid).map(|p| (pid, p.panel_type)))
                    })
                };
                if let Some((panel_id, panel_type)) = info {
                    if panel_type == PanelType::Browser {
                        #[cfg(feature = "webkit")]
                        state
                            .shared
                            .send_ui_event(crate::app::UiEvent::BrowserAction {
                                panel_id,
                                action: crate::ui::browser_panel::BrowserActionKind::ZoomOut,
                            });
                    } else if let Some(surface) = state.terminal_cache.borrow().get(&panel_id) {
                        surface.binding_action("decrease_font_size:1");
                    }
                }
                glib::Propagation::Stop
            }
            // Ctrl+0: Reset font size (terminal) or zoom (browser)
            (gdk4::Key::_0, true, false) => {
                let info = {
                    let tm = lock_or_recover(&state.shared.tab_manager);
                    tm.selected().and_then(|ws| {
                        ws.focused_panel_id
                            .and_then(|pid| ws.panels.get(&pid).map(|p| (pid, p.panel_type)))
                    })
                };
                if let Some((panel_id, panel_type)) = info {
                    if panel_type == PanelType::Browser {
                        #[cfg(feature = "webkit")]
                        state
                            .shared
                            .send_ui_event(crate::app::UiEvent::BrowserAction {
                                panel_id,
                                action: crate::ui::browser_panel::BrowserActionKind::SetZoom {
                                    zoom: 1.0,
                                },
                            });
                    } else if let Some(surface) = state.terminal_cache.borrow().get(&panel_id) {
                        surface.binding_action("reset_font_size");
                    }
                }
                glib::Propagation::Stop
            }
            // Ctrl+R: Context-aware reload —
            //   Browser pane focused → reload the web page
            //   Terminal pane focused → reload ghostty configuration
            (gdk4::Key::r, true, false) => {
                let info = {
                    let tm = lock_or_recover(&state.shared.tab_manager);
                    tm.selected().and_then(|ws| {
                        ws.focused_panel_id
                            .and_then(|pid| ws.panels.get(&pid).map(|p| (pid, p.panel_type)))
                    })
                };
                match info {
                    Some((panel_id, PanelType::Browser)) => {
                        #[cfg(feature = "webkit")]
                        state
                            .shared
                            .send_ui_event(crate::app::UiEvent::BrowserAction {
                                panel_id,
                                action: crate::ui::browser_panel::BrowserActionKind::Reload,
                            });
                    }
                    Some((_, _)) => {
                        // Terminal or other pane: reload ghostty config
                        state
                            .shared
                            .send_ui_event(crate::app::UiEvent::ReloadConfig);
                    }
                    None => {}
                }
                glib::Propagation::Stop
            }
            // F2: Rename the focused panel tab
            (gdk4::Key::F2, false, false) => {
                let panel_id = {
                    let tm = lock_or_recover(&state.shared.tab_manager);
                    tm.selected().and_then(|ws| ws.focused_panel_id)
                };
                if let Some(panel_id) = panel_id {
                    if let Some(window) = window_weak.upgrade() {
                        super::dialogs::show_rename_tab_dialog(&window, &state, panel_id);
                    }
                }
                glib::Propagation::Stop
            }
            _ => {
                // Check user-configurable shortcuts that have no hardcoded binding.
                let shortcuts = crate::settings::shortcuts::load();
                if let Some(binding) = shortcuts.get("agent.resume") {
                    let key_str = binding.key.to_lowercase();
                    let key_matches = keyval
                        .name()
                        .map(|n| n.to_lowercase() == key_str)
                        .unwrap_or(false)
                        || keyval
                            .to_unicode()
                            .map(|c| c.to_lowercase().to_string() == key_str)
                            .unwrap_or(false);
                    if key_matches
                        && ctrl == binding.ctrl
                        && shift == binding.shift
                        && alt == binding.alt
                    {
                        state
                            .shared
                            .send_ui_event(crate::app::UiEvent::AgentResume);
                        return glib::Propagation::Stop;
                    }
                }
                glib::Propagation::Proceed
            }
        }
    });

    window.add_controller(controller);
}
