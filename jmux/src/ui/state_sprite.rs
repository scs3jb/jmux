//! Animated Claude-state sprites — the deck octopi, rendered small in the UI.
//!
//! The three GIFs are the same sprites ~/src/deck shows on the e-ink deck
//! (working = hammering an anvil, needs-input = holding a sparkler, waiting =
//! typing at a laptop), pre-keyed to a transparent background by deck's sprite
//! pipeline (see jmux/assets/README.md). This module decodes them once, then
//! hands out per-row animated `gtk4::Image`s whose size tracks the UI font.
//!
//! The frame iterator is shared per state and driven by wall-clock time, so a
//! sprite keeps its animation phase across sidebar row rebuilds (which happen
//! up to 5×/s while Claude's title spinner is ticking).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gdk_pixbuf::{PixbufAnimation, PixbufAnimationIter};
use gtk4::prelude::*;

use crate::app::AppState;
use crate::model::claude_state::{classify, is_shell_title, ClaudeState};
use crate::model::{PanelType, Workspace};

const WORKING_GIF: &[u8] = include_bytes!("../../assets/working.gif");
const NEEDS_INPUT_GIF: &[u8] = include_bytes!("../../assets/needs_input.gif");
const WAITING_GIF: &[u8] = include_bytes!("../../assets/waiting.gif");

/// Sprite height in line-heights of the reference widget's font, so the
/// octopus scales with the UI font like any text would.
const SPRITE_LINE_HEIGHTS: f64 = 2.0;

/// The most urgent Claude state across a workspace's agent panes (non-shell
/// terminals), or `None` when idle / no agent is present. Mirrors deck's
/// per-session `max()` aggregation.
pub fn workspace_claude_state(workspace: &Workspace, state: &Rc<AppState>) -> Option<ClaudeState> {
    workspace
        .layout
        .all_panel_ids()
        .into_iter()
        .filter_map(|pid| {
            let panel = workspace.panels.get(&pid)?;
            if panel.panel_type != PanelType::Terminal {
                return None;
            }
            let raw_title = panel.title.as_deref().unwrap_or("");
            if is_shell_title(raw_title) {
                return None;
            }
            // A hibernated agent is paused, not working.
            if state.shared.is_hibernated(&pid) {
                return None;
            }
            let text = state
                .terminal_cache
                .borrow()
                .get(&pid)
                .and_then(|s| s.read_screen_text())?;
            classify(&text, raw_title)
        })
        .max()
}

/// Build a small animated sprite for `claude_state`, sized against
/// `reference`'s font so it scales with the UI. The animation runs on a glib
/// timer that stops itself once the widget is dropped (e.g. the sidebar row
/// was rebuilt).
pub fn sprite_image(claude_state: ClaudeState, reference: &impl IsA<gtk4::Widget>) -> gtk4::Image {
    let image = gtk4::Image::new();
    image.set_pixel_size(sprite_px(reference.as_ref()));
    image.set_tooltip_text(Some(match claude_state {
        ClaudeState::Working => "Claude is working",
        ClaudeState::NeedsInput => "Claude needs your input",
        ClaudeState::Waiting => "Claude is waiting on a background task",
    }));
    image.add_css_class("claude-sprite");

    if let Some(iter) = shared_iter(claude_state) {
        iter.advance(std::time::SystemTime::now());
        image.set_paintable(Some(&gdk4::Texture::for_pixbuf(&iter.pixbuf())));
        schedule_frame(&image, iter);
    }
    image
}

/// Sprite size in px: a couple of line-heights of the reference font.
fn sprite_px(reference: &gtk4::Widget) -> i32 {
    let metrics = reference.pango_context().metrics(None, None);
    let line_px = (metrics.ascent() + metrics.descent()) as f64 / gtk4::pango::SCALE as f64;
    (line_px * SPRITE_LINE_HEIGHTS).round().max(16.0) as i32
}

fn schedule_frame(image: &gtk4::Image, iter: PixbufAnimationIter) {
    // None means a single-frame image; clamp pathological delays.
    let Some(delay) = iter.delay_time() else {
        return;
    };
    let delay = delay.max(Duration::from_millis(20));
    let weak = image.downgrade();
    glib::timeout_add_local_once(delay, move || {
        let Some(image) = weak.upgrade() else {
            return; // row was rebuilt/closed — stop animating
        };
        // The iter is shared and time-driven: advancing from several rows is
        // idempotent (everyone lands on the frame for "now").
        iter.advance(std::time::SystemTime::now());
        image.set_paintable(Some(&gdk4::Texture::for_pixbuf(&iter.pixbuf())));
        schedule_frame(&image, iter);
    });
}

/// Decode each GIF once per process and keep one wall-clock-driven frame
/// iterator per state, shared by every widget showing that sprite.
fn shared_iter(claude_state: ClaudeState) -> Option<PixbufAnimationIter> {
    thread_local! {
        static CACHE: RefCell<HashMap<&'static str, Option<PixbufAnimationIter>>> =
            RefCell::new(HashMap::new());
    }
    let (key, bytes): (&'static str, &'static [u8]) = match claude_state {
        ClaudeState::Working => ("working", WORKING_GIF),
        ClaudeState::NeedsInput => ("needs_input", NEEDS_INPUT_GIF),
        ClaudeState::Waiting => ("waiting", WAITING_GIF),
    };
    CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| {
                let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_static(bytes));
                PixbufAnimation::from_stream(&stream, gio::Cancellable::NONE)
                    .map_err(|e| tracing::warn!("failed to decode {key} sprite: {e}"))
                    .ok()
                    .map(|anim| anim.iter(Some(std::time::SystemTime::now())))
            })
            .clone()
    })
}
