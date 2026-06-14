//! Diff panel — a git "CodeView" diff viewer.
//!
//! Renders `git diff` (optionally `--staged`) for a directory into a
//! monospace TextView with colored add/remove/hunk lines. Uses plain GTK
//! widgets (no WebKit), so it works in every build configuration.

use std::cell::Cell;
use std::process::Command;
use std::rc::Rc;

use gtk4::prelude::*;

/// Create a diff panel widget rendering `git diff` for `dir`.
///
/// Layout:
/// ```text
/// VBox:
///   ├─ toolbar (HBox): [icon] [label] [spacer] [Staged toggle] [reload]
///   └─ ScrolledWindow → TextView (monospace, colored diff)
/// ```
/// What the diff panel shows. Encoded in the panel's `command` field:
/// absent/empty = working tree, "staged" = index, "branch:<ref>" = vs <ref>.
enum DiffSpec {
    Working,
    Staged,
    Branch(String),
}

fn parse_spec(source: Option<&str>) -> DiffSpec {
    let s = source.unwrap_or("");
    if s == "staged" {
        DiffSpec::Staged
    } else if let Some(r) = s.strip_prefix("branch:") {
        DiffSpec::Branch(r.to_string())
    } else {
        DiffSpec::Working
    }
}

pub fn create_diff_widget(
    panel_id: uuid::Uuid,
    dir: Option<&str>,
    source: Option<&str>,
    is_attention_source: bool,
) -> gtk4::Widget {
    let initial_spec = parse_spec(source);
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

    let icon = gtk4::Image::from_icon_name("media-flash-symbolic");
    icon.set_pixel_size(16);
    toolbar.append(&icon);

    let label = gtk4::Label::new(Some("git diff"));
    label.add_css_class("dim-label");
    label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
    label.set_tooltip_text(Some(&dir));
    toolbar.append(&label);

    let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    toolbar.append(&spacer);

    let staged_toggle = gtk4::ToggleButton::with_label("Staged");
    staged_toggle.add_css_class("flat");
    staged_toggle.set_tooltip_text(Some("Show staged changes (git diff --staged)"));
    toolbar.append(&staged_toggle);

    let reload_btn = gtk4::Button::from_icon_name("view-refresh-symbolic");
    reload_btn.add_css_class("flat");
    reload_btn.set_tooltip_text(Some("Reload"));
    toolbar.append(&reload_btn);

    container.append(&toolbar);

    // ── Diff view ──
    let text_view = gtk4::TextView::new();
    text_view.set_editable(false);
    text_view.set_cursor_visible(false);
    text_view.set_monospace(true);
    text_view.set_wrap_mode(gtk4::WrapMode::None);
    text_view.add_css_class("diff-view");
    text_view.set_left_margin(8);
    text_view.set_right_margin(8);
    text_view.set_top_margin(4);

    let buffer = text_view.buffer();
    install_diff_tags(&buffer);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);
    scrolled.set_child(Some(&text_view));
    container.append(&scrolled);

    // Branch mode renders a fixed `git diff <ref>` and hides the staged toggle.
    if let DiffSpec::Branch(ref r) = initial_spec {
        let r = r.clone();
        staged_toggle.set_visible(false);
        label.set_text(&format!("git diff {r}"));
        render_diff(&buffer, &dir, &DiffSpec::Branch(r.clone()));
        let buffer2 = buffer.clone();
        let dir2 = dir.clone();
        reload_btn.connect_clicked(move |_| {
            render_diff(&buffer2, &dir2, &DiffSpec::Branch(r.clone()));
        });
        return container.upcast();
    }

    // Working-tree / staged mode with the toggle.
    let staged = Rc::new(Cell::new(matches!(initial_spec, DiffSpec::Staged)));
    staged_toggle.set_active(staged.get());

    let spec_of = |s: bool| if s { DiffSpec::Staged } else { DiffSpec::Working };
    render_diff(&buffer, &dir, &spec_of(staged.get()));

    {
        let buffer = buffer.clone();
        let dir = dir.clone();
        let staged = staged.clone();
        staged_toggle.connect_toggled(move |btn| {
            staged.set(btn.is_active());
            render_diff(&buffer, &dir, &spec_of(staged.get()));
        });
    }
    {
        let buffer = buffer.clone();
        let dir = dir.clone();
        let staged = staged.clone();
        reload_btn.connect_clicked(move |_| {
            render_diff(&buffer, &dir, &spec_of(staged.get()));
        });
    }

    container.upcast()
}

/// Install the text tags used to color diff lines.
fn install_diff_tags(buffer: &gtk4::TextBuffer) {
    let table = buffer.tag_table();
    let add = gtk4::TextTag::builder()
        .name("diff-add")
        .foreground("#2ea043")
        .build();
    let remove = gtk4::TextTag::builder()
        .name("diff-remove")
        .foreground("#f85149")
        .build();
    let hunk = gtk4::TextTag::builder()
        .name("diff-hunk")
        .foreground("#58a6ff")
        .build();
    let meta = gtk4::TextTag::builder()
        .name("diff-meta")
        .weight(700)
        .build();
    table.add(&add);
    table.add(&remove);
    table.add(&hunk);
    table.add(&meta);
}

/// Run `git diff` (optionally `--staged`) in `dir` and render it into `buffer`
/// with per-line coloring.
fn render_diff(buffer: &gtk4::TextBuffer, dir: &str, spec: &DiffSpec) {
    buffer.set_text("");

    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).arg("--no-pager").arg("diff");
    match spec {
        DiffSpec::Working => {}
        DiffSpec::Staged => {
            cmd.arg("--staged");
        }
        DiffSpec::Branch(r) => {
            cmd.arg(r);
        }
    }
    let output = cmd.output();

    let empty_msg = match spec {
        DiffSpec::Staged => "No staged changes.",
        DiffSpec::Branch(_) => "No differences from that ref.",
        DiffSpec::Working => "No changes in the working tree.",
    };

    let text = match output {
        Ok(out) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout).to_string();
            if s.trim().is_empty() {
                buffer.set_text(empty_msg);
                return;
            }
            s
        }
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            buffer.set_text(&format!("git diff failed:\n{err}"));
            return;
        }
        Err(e) => {
            buffer.set_text(&format!("Failed to run git: {e}"));
            return;
        }
    };

    // Append each line with the appropriate tag.
    for line in text.split_inclusive('\n') {
        let tag = if line.starts_with("@@") {
            Some("diff-hunk")
        } else if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff ") {
            Some("diff-meta")
        } else if line.starts_with('+') {
            Some("diff-add")
        } else if line.starts_with('-') {
            Some("diff-remove")
        } else {
            None
        };
        let mut end = buffer.end_iter();
        match tag {
            Some(tag_name) => buffer.insert_with_tags_by_name(&mut end, line, &[tag_name]),
            None => buffer.insert(&mut end, line),
        }
    }
}
