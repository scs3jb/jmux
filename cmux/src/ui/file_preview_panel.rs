//! File preview panel — a read-only viewer for an arbitrary file.
//!
//! Images and videos render inline (mirroring cmux's Finder, whose headline use
//! case is reviewing screenshots and demo videos agents produce); everything
//! else falls back to a monospace TextView. Markdown uses the dedicated panel.

use std::path::Path;

use gtk4::prelude::*;

/// Maximum bytes read for the preview (avoid loading huge files).
const PREVIEW_LIMIT: usize = 2 * 1024 * 1024;

/// What kind of inline preview a path gets, based on its extension.
#[derive(Clone, Copy, PartialEq)]
enum PreviewKind {
    Image,
    Video,
    Text,
}

fn preview_kind(path: &str) -> PreviewKind {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "tiff" | "avif" => {
            PreviewKind::Image
        }
        "mp4" | "webm" | "mkv" | "mov" | "avi" | "m4v" | "ogv" => PreviewKind::Video,
        _ => PreviewKind::Text,
    }
}

/// Create a file-preview widget for `path`.
pub fn create_file_preview_widget(
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

    let path = path.map(String::from).unwrap_or_default();

    // ── Toolbar ──
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar.add_css_class("browser-nav-bar");
    toolbar.set_margin_start(6);
    toolbar.set_margin_end(6);
    toolbar.set_margin_top(2);
    toolbar.set_margin_bottom(2);

    let kind = preview_kind(&path);
    let icon_name = match kind {
        PreviewKind::Image => "image-x-generic-symbolic",
        PreviewKind::Video => "video-x-generic-symbolic",
        PreviewKind::Text => "text-x-generic-symbolic",
    };
    let icon = gtk4::Image::from_icon_name(icon_name);
    icon.set_pixel_size(16);
    toolbar.append(&icon);

    let label = gtk4::Label::new(
        Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .or(Some("File")),
    );
    label.add_css_class("dim-label");
    label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    label.set_tooltip_text(Some(&path));
    toolbar.append(&label);

    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    toolbar.append(&spacer);

    let open_btn = gtk4::Button::from_icon_name("document-edit-symbolic");
    open_btn.add_css_class("flat");
    open_btn.set_tooltip_text(Some("Open in editor ($EDITOR / xdg-open)"));
    toolbar.append(&open_btn);

    let reload_btn = gtk4::Button::from_icon_name("view-refresh-symbolic");
    reload_btn.add_css_class("flat");
    reload_btn.set_tooltip_text(Some("Reload"));
    toolbar.append(&reload_btn);

    container.append(&toolbar);

    // ── Body ──
    match kind {
        PreviewKind::Image => {
            let picture = gtk4::Picture::for_filename(&path);
            picture.set_can_shrink(true);
            picture.set_hexpand(true);
            picture.set_vexpand(true);
            let scrolled = gtk4::ScrolledWindow::new();
            scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
            scrolled.set_hexpand(true);
            scrolled.set_vexpand(true);
            scrolled.set_child(Some(&picture));
            container.append(&scrolled);
            let path = path.clone();
            reload_btn.connect_clicked(move |_| {
                picture.set_filename(Some(&path));
            });
        }
        PreviewKind::Video => {
            let video = gtk4::Video::for_filename(Some(&path));
            video.set_autoplay(false);
            video.set_hexpand(true);
            video.set_vexpand(true);
            container.append(&video);
            let path = path.clone();
            reload_btn.connect_clicked(move |_| {
                video.set_filename(Some(&path));
            });
        }
        PreviewKind::Text => {
            let text_view = gtk4::TextView::new();
            text_view.set_editable(false);
            text_view.set_cursor_visible(false);
            text_view.set_monospace(true);
            text_view.set_wrap_mode(gtk4::WrapMode::None);
            text_view.set_left_margin(8);
            text_view.set_right_margin(8);
            text_view.set_top_margin(4);

            let buffer = text_view.buffer();
            render_file(&buffer, &path);

            let scrolled = gtk4::ScrolledWindow::new();
            scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
            scrolled.set_hexpand(true);
            scrolled.set_vexpand(true);
            scrolled.set_child(Some(&text_view));
            container.append(&scrolled);

            let path = path.clone();
            reload_btn.connect_clicked(move |_| render_file(&buffer, &path));
        }
    }

    {
        let path = path.clone();
        open_btn.connect_clicked(move |_| {
            let editor = std::env::var("EDITOR").ok().filter(|e| !e.is_empty());
            let _ = match editor {
                Some(ed) => std::process::Command::new(ed).arg(&path).spawn(),
                None => std::process::Command::new("xdg-open").arg(&path).spawn(),
            };
        });
    }

    container.upcast()
}

/// Read `path` and render it into `buffer` (bounded; binary-safe).
fn render_file(buffer: &gtk4::TextBuffer, path: &str) {
    use std::io::Read;
    if path.is_empty() {
        buffer.set_text("No file specified.");
        return;
    }
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            buffer.set_text(&format!("Failed to open {path}:\n{e}"));
            return;
        }
    };
    let mut buf = vec![0u8; PREVIEW_LIMIT];
    let n = file.read(&mut buf).unwrap_or(0);
    buf.truncate(n);
    // Binary detection: NUL byte in the read window.
    if buf.contains(&0) {
        buffer.set_text(&format!("(binary file — {n} bytes shown of preview window)"));
        return;
    }
    let text = String::from_utf8_lossy(&buf);
    buffer.set_text(&text);
    if n == PREVIEW_LIMIT {
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, "\n\n… preview truncated (file larger than 2 MB)\n");
    }
}
