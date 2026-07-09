//! Project visualizer panel — an Xcode-style overview of a project's
//! structure: a bounded directory tree plus a file-type / size summary.
//!
//! Plain GTK (no WebKit), so it works in every build configuration.

use std::collections::BTreeMap;
use std::path::Path;

use gtk4::prelude::*;

/// Directories that are noise in a structure overview and are skipped.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".venv",
    "venv",
    "__pycache__",
    ".cache",
    ".idea",
    ".gradle",
    "vendor",
    "zig-cache",
    "zig-out",
];

const MAX_DEPTH: usize = 4;
const MAX_ENTRIES: usize = 800;

/// Create a project visualizer widget for `dir`.
pub fn create_project_widget(
    panel_id: uuid::Uuid,
    dir: Option<&str>,
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

    let dir = dir
        .map(String::from)
        .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| ".".to_string()));

    // ── Toolbar ──
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar.add_css_class("browser-nav-bar");
    toolbar.set_margin_start(6);
    toolbar.set_margin_end(6);
    toolbar.set_margin_top(2);
    toolbar.set_margin_bottom(2);

    let icon = gtk4::Image::from_icon_name("view-list-symbolic");
    icon.set_pixel_size(16);
    toolbar.append(&icon);

    let label = gtk4::Label::new(Some(
        Path::new(&dir)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Project"),
    ));
    label.add_css_class("dim-label");
    label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    label.set_tooltip_text(Some(&dir));
    toolbar.append(&label);

    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    toolbar.append(&spacer);

    let reload_btn = gtk4::Button::from_icon_name("view-refresh-symbolic");
    reload_btn.add_css_class("flat");
    reload_btn.set_tooltip_text(Some("Reload"));
    toolbar.append(&reload_btn);

    container.append(&toolbar);

    // ── Body ──
    let text_view = gtk4::TextView::new();
    text_view.set_editable(false);
    text_view.set_cursor_visible(false);
    text_view.set_monospace(true);
    text_view.set_wrap_mode(gtk4::WrapMode::None);
    text_view.set_left_margin(8);
    text_view.set_right_margin(8);
    text_view.set_top_margin(4);

    let buffer = text_view.buffer();
    render_project(&buffer, &dir);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);
    scrolled.set_child(Some(&text_view));
    container.append(&scrolled);

    {
        let buffer = buffer.clone();
        let dir = dir.clone();
        reload_btn.connect_clicked(move |_| render_project(&buffer, &dir));
    }

    container.upcast()
}

/// Render the project tree + summary into `buffer`.
fn render_project(buffer: &gtk4::TextBuffer, dir: &str) {
    let mut out = String::new();
    let mut ext_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut file_count = 0usize;
    let mut dir_count = 0usize;
    let mut truncated = false;

    let root = Path::new(dir);
    out.push_str(&format!("{}\n", root.display()));
    walk(
        root,
        "",
        0,
        &mut out,
        &mut ext_counts,
        &mut file_count,
        &mut dir_count,
        &mut truncated,
    );

    if truncated {
        out.push_str(&format!(
            "\n… tree truncated at {MAX_ENTRIES} entries / depth {MAX_DEPTH}\n"
        ));
    }

    // Summary section.
    out.push_str(&format!(
        "\nSummary: {file_count} files, {dir_count} directories\n"
    ));
    if !ext_counts.is_empty() {
        let mut by_count: Vec<(&String, &usize)> = ext_counts.iter().collect();
        by_count.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        out.push_str("Top file types:\n");
        for (ext, count) in by_count.into_iter().take(12) {
            let label = if ext.is_empty() { "(no ext)" } else { ext };
            out.push_str(&format!("  {label:<12} {count}\n"));
        }
    }

    buffer.set_text(&out);
}

#[allow(clippy::too_many_arguments)]
fn walk(
    dir: &Path,
    prefix: &str,
    depth: usize,
    out: &mut String,
    ext_counts: &mut BTreeMap<String, usize>,
    file_count: &mut usize,
    dir_count: &mut usize,
    truncated: &mut bool,
) {
    if depth >= MAX_DEPTH || *truncated {
        return;
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().collect(),
        Err(_) => return,
    };
    // Directories first, then files; alphabetical within each.
    entries.sort_by_key(|e| {
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        (!is_dir, e.file_name().to_string_lossy().to_lowercase())
    });

    let n = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        if *file_count + *dir_count >= MAX_ENTRIES {
            *truncated = true;
            return;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') && depth == 0 && name != ".github" {
            // Skip most top-level dotfiles/dirs from the visual tree, but still
            // surface hidden config dirs one level deep would be noisy.
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let is_last = i + 1 == n;
        let branch = if is_last { "└── " } else { "├── " };
        out.push_str(&format!("{prefix}{branch}{name}\n"));

        if is_dir {
            *dir_count += 1;
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            let child_prefix = format!("{prefix}{}", if is_last { "    " } else { "│   " });
            walk(
                &entry.path(),
                &child_prefix,
                depth + 1,
                out,
                ext_counts,
                file_count,
                dir_count,
                truncated,
            );
        } else {
            *file_count += 1;
            let ext = Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            *ext_counts.entry(ext).or_insert(0) += 1;
        }
    }
}
