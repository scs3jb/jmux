//! Layer-shell drop-down quick terminal window (feature `quick-terminal`).
//!
//! Reuses `create_window` (chromeless: no header, collapsed sidebar) and turns
//! the window into a top-anchored `wlr-layer-shell` overlay. Showing/hiding
//! animates the layer-shell top margin so it slides down from / up past the top
//! edge of the screen. Layer surfaces have no compositor decorations, so there
//! are no maximize/close buttons by construction.

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
    current_margin: i32,
    /// Slide tween duration (ms), read from settings once at creation so the
    /// hide path doesn't touch disk on every toggle.
    animation_ms: u32,
    /// Bumped on each slide; an in-flight tween stops when it's superseded.
    generation: u64,
}

pub fn handle(action: QuickTermAction, app: &gtk4::Application, state: &Rc<AppState>) {
    let Some(adw_app) = app.downcast_ref::<adw::Application>().cloned() else {
        tracing::warn!("quick terminal: application is not an adw::Application");
        return;
    };
    ensure_window(&adw_app, state);
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

/// Create the drop-down window on first use (hidden, off-screen above the top).
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

    // Top-anchored, full-width overlay layer surface (no decorations).
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("cmux-quick-terminal"));
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Right, true);
    window.set_anchor(Edge::Bottom, false);
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    window.set_default_height(height);
    window.set_height_request(height);
    window.set_margin(Edge::Top, -height); // hidden above the screen
    window.set_visible(false);

    install_resize_handle(&window);
    watch_monitor();

    QUICK.with(|q| {
        *q.borrow_mut() = Some(QuickState {
            window,
            visible: false,
            height,
            current_margin: -height,
            animation_ms: cfg.animation_ms,
            generation: 0,
        });
    });
}

/// Smallest height the drop-down can be dragged to (px).
const MIN_HEIGHT: i32 = 120;
/// Thickness of the visible grab strip along the bottom edge (px).
const RESIZE_GRIP_PX: i32 = 6;
/// How close to the bottom edge a press must start to begin a resize (px).
const RESIZE_ZONE_PX: i32 = 12;

/// Resize the drop-down to `height` px, re-clamping to the current monitor, and
/// keep it correctly positioned: parked at `-height` above the top edge while
/// hidden, flush at margin 0 while shown. Used by the drag handle and by the
/// monitor-change watcher.
fn set_height(qs: &mut QuickState, height: i32) {
    let max_h = monitor_height().unwrap_or_else(|| height.max(MIN_HEIGHT));
    let height = height.clamp(MIN_HEIGHT, max_h.max(MIN_HEIGHT));
    qs.height = height;
    qs.window.set_default_height(height);
    qs.window.set_height_request(height);
    let margin = if qs.visible { 0 } else { -height };
    qs.current_margin = margin;
    qs.window.set_margin(Edge::Top, margin);
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
        let Some(height) = QUICK.with(|q| q.borrow().as_ref().map(|s| s.height)) else {
            return;
        };
        let mon_h = monitor_height().unwrap_or(1080).max(1);
        let fraction = (height as f32 / mon_h as f32).clamp(0.1, 1.0);
        let mut settings = crate::settings::load();
        settings.quick_terminal.height_fraction = fraction;
        if let Err(e) = crate::settings::save(&settings) {
            tracing::warn!("quick terminal: failed to persist height: {e}");
        }
    });
    outer_box.add_controller(drag);
}

/// Re-fit the drop-down whenever the monitor geometry or layout changes, so it
/// keeps matching the screen after the desktop resolution is changed.
fn watch_monitor() {
    let Some(display) = gtk4::gdk::Display::default() else {
        return;
    };

    let refit: Rc<dyn Fn()> = Rc::new(|| {
        let fraction = crate::settings::load().quick_terminal.height_fraction;
        let height = height_for_fraction(fraction);
        QUICK.with(|q| {
            if let Some(qs) = q.borrow_mut().as_mut() {
                set_height(qs, height);
            }
        });
    });

    let monitors = display.monitors();

    // A resolution change on an existing output updates that monitor's geometry.
    for i in 0..monitors.n_items() {
        if let Some(mon) = monitors
            .item(i)
            .and_then(|m| m.downcast::<gtk4::gdk::Monitor>().ok())
        {
            let refit = refit.clone();
            mon.connect_geometry_notify(move |_| refit());
        }
    }

    // Outputs being added/removed/replaced changes the monitor list; re-fit from
    // whichever monitor is now primary.
    monitors.connect_items_changed(move |_, _, _, _| refit());
}

fn slide_in(qs: &mut QuickState) {
    qs.visible = true;
    qs.window.set_keyboard_mode(KeyboardMode::OnDemand);
    if !qs.window.is_visible() {
        // First show: map just above the top edge (off-screen) so the
        // compositor's one-time window-open effect plays out of view. The
        // surface then STAYS mapped — hide only slides it off-screen — so every
        // later show is a pure margin slide with no re-map and no open effect
        // (which otherwise reads as an expand-from-center).
        qs.current_margin = -qs.height;
        qs.window.set_margin(Edge::Top, -qs.height);
        qs.window.set_visible(true);
    }
    qs.window.present(); // raise + focus (no re-map once already mapped)
    slide(qs, 0);
}

fn slide_out(qs: &mut QuickState) {
    qs.visible = false;
    slide(qs, -qs.height);
}

/// Animate the layer-shell top margin from its current value to `to` with a
/// glib-timeout tween (ease-out cubic). Used for both show (slide down, `to` 0)
/// and hide (slide up, `to` -height). The window stays mapped throughout
/// (off-screen above the top edge when hidden). A frame-clock-driven adw
/// animation does not run reliably on a just-mapped layer surface, so we step
/// the margin manually, recording the live position each frame so a toggle
/// mid-animation reverses smoothly.
fn slide(qs: &mut QuickState, to: i32) {
    let from = qs.current_margin;
    qs.generation += 1;
    let generation = qs.generation;
    let duration = qs.animation_ms.max(1) as f64;
    let win = qs.window.clone();
    let hide_at_end = to < 0;
    let start = std::time::Instant::now();
    glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        let t = (start.elapsed().as_secs_f64() * 1000.0 / duration).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        let v = (from as f64 + (to - from) as f64 * eased).round() as i32;
        // Stop if a newer slide superseded this one (or the window is gone);
        // otherwise record the live margin so a reversing toggle starts here.
        let superseded = QUICK.with(|q| match q.borrow_mut().as_mut() {
            Some(s) if s.generation == generation => {
                s.current_margin = v;
                false
            }
            _ => true,
        });
        if superseded {
            return glib::ControlFlow::Break;
        }
        win.set_margin(Edge::Top, v);
        if t >= 1.0 {
            if hide_at_end {
                // Stay mapped (parked off-screen) so the next show is a slide,
                // not a re-map; just release the keyboard so the hidden console
                // doesn't keep eating input.
                win.set_keyboard_mode(KeyboardMode::None);
            }
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

fn monitor_height() -> Option<i32> {
    let display = gtk4::gdk::Display::default()?;
    let monitor = display
        .monitors()
        .item(0)?
        .downcast::<gtk4::gdk::Monitor>()
        .ok()?;
    Some(monitor.geometry().height())
}

/// Pixel height for the configured monitor-height `fraction`, falling back to a
/// 1080p assumption when the monitor can't be queried.
fn height_for_fraction(fraction: f32) -> i32 {
    let mon_h = monitor_height().unwrap_or(1080);
    ((mon_h as f32) * fraction.clamp(0.1, 1.0)).round() as i32
}
