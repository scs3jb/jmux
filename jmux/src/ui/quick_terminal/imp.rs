//! Layer-shell drop-down quick terminal window (feature `quick-terminal`).
//!
//! Reuses `create_window` (chromeless: no header, collapsed sidebar) and turns
//! the window into a top-anchored `wlr-layer-shell` overlay. Showing/hiding
//! reveals clipped content within the selected output, never moving the surface
//! into a neighbouring monitor. Layer surfaces have no compositor decorations,
//! so there are no maximize/close buttons by construction.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use libadwaita as adw;
use libadwaita::prelude::AdwApplicationWindowExt;

use crate::app::{lock_or_recover, AppState, QuickTermAction};

use super::quick_window_id;

thread_local! {
    static QUICK: RefCell<Option<QuickState>> = const { RefCell::new(None) };
}

struct QuickState {
    window: adw::ApplicationWindow,
    visible: bool,
    height: i32,
    revealer: gtk4::Revealer,
    content: gtk4::Widget,
    /// Slide duration from the current settings.
    animation_ms: u32,
    monitor: Option<gtk4::gdk::Monitor>,
}

pub fn handle(action: QuickTermAction, app: &gtk4::Application, state: &Rc<AppState>) {
    let app = app.clone();
    let state = Rc::clone(state);
    // Wait for the first desktop query so the initial frame uses the primary
    // output too, rather than briefly mapping on an arbitrary monitor.
    super::display::with_primary(move || handle_ready(action, &app, &state));
}

fn handle_ready(action: QuickTermAction, app: &gtk4::Application, state: &Rc<AppState>) {
    let Some(adw_app) = app.downcast_ref::<adw::Application>().cloned() else {
        tracing::warn!("quick terminal: application is not an adw::Application");
        return;
    };
    ensure_window(&adw_app, state);
    refresh_display();
    QUICK.with(|q| {
        if let Some(qs) = q.borrow_mut().as_mut() {
            let show = match action {
                QuickTermAction::Show => true,
                QuickTermAction::Hide => false,
                QuickTermAction::Toggle => !qs.visible,
            };
            if show {
                slide_in(qs);
                // Route keystrokes to the drop-down terminal right away rather
                // than waiting for the user to click it (present() raises it,
                // this hands ghostty's focus to its surface).
                if let Some(gapp) = state.ghostty_app.borrow().as_ref() {
                    gapp.set_focus(true);
                }
            } else {
                slide_out(qs);
            }
        }
    });
}

/// Create the drop-down window on first use, initially hidden.
fn ensure_window(app: &adw::Application, state: &Rc<AppState>) {
    if QUICK.with(|q| q.borrow().is_some()) {
        return;
    }
    let window_id = quick_window_id();

    // Mirror `open_window`: a per-window event channel + a hosted workspace.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    state.shared.install_ui_event_sender(window_id, tx);
    {
        // Pure quake-daemon mode: this drop-down is the only window, so it shows
        // the normal workspace list and tracks the global selection like any
        // window — no dedicated "Quick Terminal" workspace, no per-window pin.
        // Only seed a default workspace when there are none at all.
        let mut tm = lock_or_recover(&state.shared.tab_manager);
        if tm.iter().next().is_none() && !crate::ui::welcome::should_show_welcome() {
            tm.add_workspace(crate::model::Workspace::new());
        }
    }

    let window = crate::ui::window::create_window(app, state, window_id, rx, true);

    let cfg = crate::settings::load().quick_terminal;
    let height = height_for_fraction(cfg.height_fraction);

    install_resize_handle(&window);
    let (revealer, content) = configure_window(&window, height, cfg.animation_ms);

    QUICK.with(|q| {
        *q.borrow_mut() = Some(QuickState {
            window,
            visible: false,
            height,
            revealer,
            content,
            animation_ms: cfg.animation_ms,
            monitor: None,
        });
    });
    watch_monitor();
    refresh_display();
}

/// Keep the layer surface on its output; animate only clipped child content.
fn configure_window(
    window: &adw::ApplicationWindow,
    height: i32,
    animation_ms: u32,
) -> (gtk4::Revealer, gtk4::Widget) {
    // Top-anchored, full-width overlay layer surface (no decorations).
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("jmux-quick-terminal"));
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Right, true);
    window.set_anchor(Edge::Bottom, false);
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    window.set_default_size(-1, -1);
    window.set_margin(Edge::Top, 0);
    window.set_visible(false);

    let content = window.content().expect("quick-terminal content");
    window.set_content(gtk4::Widget::NONE);
    content.set_height_request(height);
    let revealer = gtk4::Revealer::new();
    revealer.set_transition_type(gtk4::RevealerTransitionType::SlideDown);
    revealer.set_transition_duration(animation_ms);
    revealer.set_child(Some(&content));
    window.set_content(Some(&revealer));
    let weak_window = window.downgrade();
    revealer.connect_child_revealed_notify(move |r| {
        if !r.reveals_child() && !r.is_child_revealed() {
            if let Some(window) = weak_window.upgrade() {
                window.set_visible(false);
            }
        }
    });

    (revealer, content)
}

/// Smallest height the drop-down can be dragged to (px).
const MIN_HEIGHT: i32 = 120;
/// Thickness of the visible grab strip along the bottom edge (px).
const RESIZE_GRIP_PX: i32 = 6;
/// How close to the bottom edge a press must start to begin a resize (px).
const RESIZE_ZONE_PX: i32 = 12;

/// Resize the clipped content, clamped to the selected monitor.
fn set_height(qs: &mut QuickState, height: i32) {
    let max_h = monitor_height().unwrap_or_else(|| height.max(MIN_HEIGHT));
    let height = height.clamp(MIN_HEIGHT, max_h.max(MIN_HEIGHT));
    qs.height = height;
    qs.content.set_height_request(height);
}

/// Add a thin grab strip along the bottom edge plus a drag gesture so the user
/// can resize the drop-down by dragging its lower border. The chosen height is
/// persisted as a fraction of the monitor height, so it survives restarts and
/// keeps matching the screen across resolution changes.
fn install_resize_handle(window: &adw::ApplicationWindow) {
    // Reach the vertical box holding [header, split_view] (built in
    // `window::create_window`): we append the grip below the content and host the
    // gesture on this top-anchored box so the gesture's coordinate origin stays
    // fixed while we change the height.
    let Some(outer_box) = window
        .content()
        .and_then(|c| c.downcast::<adw::ToastOverlay>().ok())
        .and_then(|t| t.child())
        .and_then(|c| c.downcast::<gtk4::Box>().ok())
    else {
        tracing::warn!("quick terminal: content box not found; resize handle unavailable");
        return;
    };

    let grip = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    grip.set_hexpand(true);
    grip.set_height_request(RESIZE_GRIP_PX);
    grip.add_css_class("quick-terminal-resize-grip");
    grip.set_cursor(gtk4::gdk::Cursor::from_name("ns-resize", None).as_ref());
    outer_box.append(&grip);

    let drag = gtk4::GestureDrag::new();
    // Height at the moment the drag starts; live updates move the bottom edge but
    // not this origin, so the offset never feeds back on itself.
    let start_height = Rc::new(Cell::new(0));
    {
        let outer_box = outer_box.clone();
        let start_height = start_height.clone();
        drag.connect_drag_begin(move |gesture, _x, y| {
            // Only treat presses near the bottom edge as a resize; let everything
            // else propagate to the terminal/UI untouched.
            let zone = (outer_box.height() - RESIZE_ZONE_PX).max(0) as f64;
            if y < zone {
                gesture.set_state(gtk4::EventSequenceState::Denied);
                return;
            }
            let h = QUICK.with(|q| q.borrow().as_ref().map(|s| s.height).unwrap_or(0));
            start_height.set(h);
            gesture.set_state(gtk4::EventSequenceState::Claimed);
        });
    }
    {
        let start_height = start_height.clone();
        drag.connect_drag_update(move |_gesture, _ox, oy| {
            let target = start_height.get() + oy.round() as i32;
            QUICK.with(|q| {
                if let Some(qs) = q.borrow_mut().as_mut() {
                    set_height(qs, target);
                }
            });
        });
    }
    drag.connect_drag_end(move |_gesture, _ox, _oy| {
        let Some((height, mon_h)) = QUICK.with(|q| {
            q.borrow()
                .as_ref()
                .map(|s| (s.height, monitor_height().unwrap_or(1080).max(1)))
        }) else {
            return;
        };
        let fraction = (height as f32 / mon_h as f32).clamp(0.1, 1.0);
        let mut settings = crate::settings::load();
        settings.quick_terminal.height_fraction = fraction;
        if let Err(e) = crate::settings::save(&settings) {
            tracing::warn!("quick terminal: failed to persist height: {e}");
        }
    });
    outer_box.add_controller(drag);
}

/// Re-select the output and height when settings or the desktop layout changes.
pub fn refresh_display() {
    let cfg = crate::settings::load().quick_terminal;
    QUICK.with(|q| {
        if let Some(qs) = q.borrow_mut().as_mut() {
            let monitor = super::display::select_monitor(&cfg.preferred_display);
            if monitor != qs.monitor {
                qs.window.set_monitor(monitor.as_ref());
                qs.monitor = monitor;
            }
            qs.animation_ms = cfg.animation_ms;
            qs.revealer.set_transition_duration(cfg.animation_ms);
            let height = height_for_fraction(cfg.height_fraction);
            set_height(qs, height);
        }
    });
}

fn watch_monitor() {
    let Some(display) = gtk4::gdk::Display::default() else {
        return;
    };
    let monitors = display.monitors();
    let watch = |mon: gtk4::gdk::Monitor| {
        mon.connect_geometry_notify(|_| refresh_display());
        mon.connect_connector_notify(|_| refresh_display());
    };
    for i in 0..monitors.n_items() {
        if let Some(mon) = monitors.item(i).and_downcast::<gtk4::gdk::Monitor>() {
            watch(mon);
        }
    }
    monitors.connect_items_changed(move |list, position, _, added| {
        for i in position..position + added {
            if let Some(mon) = list.item(i).and_downcast::<gtk4::gdk::Monitor>() {
                watch(mon);
            }
        }
        refresh_display();
        super::display::update_primary();
        glib::timeout_add_local_once(std::time::Duration::from_millis(400), refresh_display);
    });
    super::display::update_primary();
    // Primary-output changes need not change GDK's monitor list or geometry.
    glib::timeout_add_local(std::time::Duration::from_secs(2), || {
        super::display::update_primary();
        glib::ControlFlow::Continue
    });
}

fn slide_in(qs: &mut QuickState) {
    qs.visible = true;
    qs.window.set_keyboard_mode(KeyboardMode::OnDemand);
    qs.window.present();
    qs.revealer.set_reveal_child(true);
}

fn slide_out(qs: &mut QuickState) {
    qs.visible = false;
    qs.window.set_keyboard_mode(KeyboardMode::None);
    qs.revealer.set_reveal_child(false);
    if qs.animation_ms == 0 {
        qs.window.set_visible(false);
    }
}

/// Use the explicitly selected output, including before the surface is mapped.
fn monitor_height() -> Option<i32> {
    let cfg = crate::settings::load().quick_terminal;
    super::display::select_monitor(&cfg.preferred_display)
        .map(|m| m.geometry().height())
        .filter(|h| *h > 0)
}

fn height_for_fraction(fraction: f32) -> i32 {
    let mon_h = monitor_height().unwrap_or(1080);
    ((mon_h as f32) * fraction.clamp(0.1, 1.0)).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(milliseconds: u64) {
        let main_loop = glib::MainLoop::new(None, false);
        let quit = main_loop.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(milliseconds), move || {
            quit.quit()
        });
        main_loop.run();
    }

    fn wait_for(condition: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !condition() && std::time::Instant::now() < deadline {
            frames(16);
        }
        assert!(
            condition(),
            "desktop did not finish the transition within 3 seconds"
        );
    }

    #[test]
    #[ignore = "requires an unlocked Wayland layer-shell desktop"]
    fn slide_stays_inside_output_and_unmaps_when_hidden() {
        adw::init().expect("desktop GTK initialization");
        let main_loop = glib::MainLoop::new(None, false);
        let quit = main_loop.clone();
        let ready = Rc::new(Cell::new(false));
        let callback_ready = ready.clone();
        super::super::display::with_primary(move || {
            callback_ready.set(true);
            quit.quit();
        });
        if !ready.get() {
            main_loop.run();
        }
        let window = adw::ApplicationWindow::builder().build();
        window.set_content(Some(&gtk4::Label::new(Some(
            "jmux display regression test",
        ))));
        let (revealer, content) = configure_window(&window, 180, 80);
        let monitor = super::super::display::select_monitor("").expect("connected output");
        eprintln!(
            "Testing clipped slide on {}",
            monitor.connector().unwrap_or_default()
        );
        window.set_monitor(Some(&monitor));
        let mut qs = QuickState {
            window: window.clone(),
            visible: false,
            height: 180,
            revealer,
            content,
            animation_ms: 80,
            monitor: Some(monitor.clone()),
        };
        slide_in(&mut qs);
        wait_for(|| qs.revealer.is_child_revealed());
        assert!(window.is_visible());
        assert_eq!(window.margin(Edge::Top), 0);
        let surface = window.surface().expect("mapped surface");
        assert_eq!(
            gtk4::prelude::WidgetExt::display(&window).monitor_at_surface(&surface),
            Some(monitor)
        );
        slide_out(&mut qs);
        wait_for(|| !window.is_visible());
        assert!(!window.is_visible(), "hidden terminal must be unmapped");
        assert_eq!(window.margin(Edge::Top), 0);
        slide_in(&mut qs);
        frames(30);
        slide_out(&mut qs);
        wait_for(|| !window.is_visible());
        assert!(!window.is_visible(), "reversed slide must finish hidden");
        window.destroy();
    }
}
