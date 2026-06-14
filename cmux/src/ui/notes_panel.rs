//! Notes panel — an editable scratchpad backed by a file, auto-saved.
//!
//! Plain GTK (editable TextView). Edits are written back to the file after a
//! short debounce so notes survive restarts. The default file lives under the
//! cmux data dir; an explicit path can be supplied.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4::prelude::*;

/// Debounce before writing edits to disk.
const SAVE_DEBOUNCE_MS: u64 = 800;

/// Default notes file when none is given: `<data>/cmux/notes.md`.
pub fn default_notes_path() -> String {
    let dir = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join("cmux");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("notes.md").to_string_lossy().into_owned()
}

/// Create an editable notes widget backed by `path`.
pub fn create_notes_widget(
    panel_id: uuid::Uuid,
    path: Option<&str>,
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

    let path: PathBuf = path
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default_notes_path()));

    // ── Toolbar ──
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar.add_css_class("browser-nav-bar");
    toolbar.set_margin_start(6);
    toolbar.set_margin_end(6);
    toolbar.set_margin_top(2);
    toolbar.set_margin_bottom(2);

    let icon = gtk4::Image::from_icon_name("accessories-text-editor-symbolic");
    icon.set_pixel_size(16);
    toolbar.append(&icon);

    let label = gtk4::Label::new(
        path.file_name()
            .and_then(|n| n.to_str())
            .or(Some("Notes")),
    );
    label.add_css_class("dim-label");
    label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    label.set_tooltip_text(path.to_str());
    toolbar.append(&label);

    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    toolbar.append(&spacer);

    let status = gtk4::Label::new(Some("saved"));
    status.add_css_class("dim-label");
    status.add_css_class("caption");
    toolbar.append(&status);

    container.append(&toolbar);

    // ── Editor ──
    let text_view = gtk4::TextView::new();
    text_view.set_editable(true);
    text_view.set_monospace(true);
    text_view.set_wrap_mode(gtk4::WrapMode::WordChar);
    text_view.set_left_margin(8);
    text_view.set_right_margin(8);
    text_view.set_top_margin(4);
    text_view.set_bottom_margin(4);

    let buffer = text_view.buffer();
    if let Ok(content) = std::fs::read_to_string(&path) {
        buffer.set_text(&content);
    }

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);
    scrolled.set_child(Some(&text_view));
    container.append(&scrolled);

    // Debounced auto-save on edit.
    let pending = Rc::new(Cell::new(false));
    {
        let buffer_w = buffer.clone();
        let path = path.clone();
        let status = status.clone();
        let pending = pending.clone();
        buffer.connect_changed(move |_| {
            status.set_text("editing…");
            if pending.replace(true) {
                return; // a save is already scheduled
            }
            let buffer_w = buffer_w.clone();
            let path = path.clone();
            let status = status.clone();
            let pending = pending.clone();
            glib::timeout_add_local_once(
                std::time::Duration::from_millis(SAVE_DEBOUNCE_MS),
                move || {
                    pending.set(false);
                    let text = buffer_w
                        .text(&buffer_w.start_iter(), &buffer_w.end_iter(), false)
                        .to_string();
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match std::fs::write(&path, text) {
                        Ok(_) => status.set_text("saved"),
                        Err(e) => status.set_text(&format!("save failed: {e}")),
                    }
                },
            );
        });
    }

    // Flush on focus loss so notes aren't lost if the panel goes away.
    {
        let buffer_w = buffer.clone();
        let path = path.clone();
        let focus = gtk4::EventControllerFocus::new();
        focus.connect_leave(move |_| {
            let text = buffer_w
                .text(&buffer_w.start_iter(), &buffer_w.end_iter(), false)
                .to_string();
            let _ = write_notes(&path, &text);
        });
        text_view.add_controller(focus);
    }

    container.upcast()
}

fn write_notes(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}
