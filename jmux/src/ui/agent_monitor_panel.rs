//! Read-only sub-agent monitor pane.
//!
//! Tails one Claude Code subagent transcript (`agent-<id>.jsonl` under the
//! session's `subagents/` directory) and renders a compact, human-readable
//! progress feed: the task prompt, assistant text, and tool invocations.
//!
//! Deliberately non-interactive — the widgets never take keyboard focus, so
//! all steering of the agents stays with the primary agent's terminal. The
//! panes exist purely to watch what the subagents are doing.

use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

use crate::model::Panel;

/// Poll interval for tailing the transcript file.
const TAIL_INTERVAL_MS: u64 = 750;

/// On (re)build, seed the view from at most this much of the file's tail —
/// enough for context without re-parsing a multi-MB transcript.
const SEED_TAIL_BYTES: u64 = 64 * 1024;

/// Keep the buffer bounded: trim from the top past this many lines.
const MAX_BUFFER_LINES: i32 = 600;

/// Build the monitor widget for an `AgentMonitor` panel.
pub fn create_agent_monitor_widget(panel: &Panel) -> gtk4::Widget {
    let container = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    container.add_css_class("agent-monitor-panel");
    container.set_hexpand(true);
    container.set_vexpand(true);

    // Header: sprite-style icon + description + read-only marker.
    let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    header.set_margin_start(8);
    header.set_margin_end(8);
    header.set_margin_top(4);
    header.set_margin_bottom(4);
    let icon = gtk4::Image::from_icon_name("utilities-system-monitor-symbolic");
    icon.add_css_class("dim-label");
    header.append(&icon);
    let title = gtk4::Label::new(Some(panel.display_title()));
    title.add_css_class("heading");
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.set_hexpand(true);
    title.set_halign(gtk4::Align::Start);
    header.append(&title);
    let status = gtk4::Label::new(Some("read-only"));
    status.add_css_class("dim-label");
    status.add_css_class("caption");
    header.append(&status);
    container.append(&header);
    container.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));

    let text_view = gtk4::TextView::new();
    text_view.set_editable(false);
    text_view.set_cursor_visible(false);
    // The monitor must never take keyboard focus — typing stays with the
    // primary agent's terminal.
    text_view.set_can_focus(false);
    text_view.set_focus_on_click(false);
    text_view.set_wrap_mode(gtk4::WrapMode::WordChar);
    text_view.set_monospace(true);
    text_view.set_left_margin(8);
    text_view.set_right_margin(8);
    text_view.set_top_margin(4);
    text_view.set_bottom_margin(4);
    text_view.add_css_class("agent-monitor-feed");

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);
    scrolled.set_can_focus(false);
    scrolled.set_child(Some(&text_view));
    container.append(&scrolled);

    let Some(path) = panel.markdown_file.clone() else {
        text_view.buffer().set_text("(no transcript path)");
        return container.upcast();
    };

    // Seed from the tail of the file, then poll for appended bytes. The offset
    // lives with this widget instance; a layout rebuild recreates the widget
    // and re-seeds, which is cheap at SEED_TAIL_BYTES.
    let offset = Rc::new(Cell::new(0u64));
    seed_from_tail(&text_view, &path, &offset, &status);

    let weak_view = text_view.downgrade();
    let weak_status = status.downgrade();
    glib::timeout_add_local(
        std::time::Duration::from_millis(TAIL_INTERVAL_MS),
        move || {
            // Widget destroyed (pane closed / layout rebuilt) → stop tailing.
            let Some(view) = weak_view.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let Some(status) = weak_status.upgrade() else {
                return glib::ControlFlow::Break;
            };
            poll_transcript(&view, &path, &offset, &status);
            glib::ControlFlow::Continue
        },
    );

    container.upcast()
}

/// Initialize the view from the last `SEED_TAIL_BYTES` of the transcript.
fn seed_from_tail(
    view: &gtk4::TextView,
    path: &str,
    offset: &Rc<Cell<u64>>,
    status: &gtk4::Label,
) {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(SEED_TAIL_BYTES);
    offset.set(start);
    poll_transcript(view, path, offset, status);
}

/// Read bytes appended since the stored offset, render each complete JSONL
/// line, and append to the buffer (auto-scrolled to the end).
fn poll_transcript(
    view: &gtk4::TextView,
    path: &str,
    offset: &Rc<Cell<u64>>,
    status: &gtk4::Label,
) {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => {
            status.set_text("finished");
            return;
        }
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut pos = offset.get();
    if len < pos {
        // Truncated/rotated — restart from the top.
        pos = 0;
    }
    if len == pos {
        return;
    }
    if file.seek(SeekFrom::Start(pos)).is_err() {
        return;
    }
    let mut new_bytes = Vec::with_capacity((len - pos) as usize);
    if file.read_to_end(&mut new_bytes).is_err() {
        return;
    }

    // Only consume complete lines; a partially-written trailing line stays
    // unconsumed (offset not advanced past it) for the next poll.
    let consumed = match new_bytes.iter().rposition(|&b| b == b'\n') {
        Some(last_newline) => last_newline + 1,
        None => return,
    };
    offset.set(pos + consumed as u64);

    let text = String::from_utf8_lossy(&new_bytes[..consumed]);
    let mut rendered = String::new();
    for line in text.lines() {
        if let Some(entry) = render_transcript_line(line) {
            rendered.push_str(&entry);
            rendered.push('\n');
        }
    }
    if rendered.is_empty() {
        return;
    }

    let buffer = view.buffer();
    buffer.insert(&mut buffer.end_iter(), &rendered);

    // Trim from the top to bound memory.
    let lines = buffer.line_count();
    if lines > MAX_BUFFER_LINES {
        let mut start = buffer.start_iter();
        let mut cut = buffer
            .iter_at_line(lines - MAX_BUFFER_LINES)
            .unwrap_or_else(|| buffer.start_iter());
        buffer.delete(&mut start, &mut cut);
    }

    // Follow the tail.
    let mut end = buffer.end_iter();
    view.scroll_to_iter(&mut end, 0.0, false, 0.0, 1.0);
}

/// Render one JSONL transcript line as a compact feed entry. Returns `None`
/// for lines that aren't useful in a progress feed (tool results, meta rows).
fn render_transcript_line(line: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let message = v.get("message")?;

    match v.get("type").and_then(|t| t.as_str()) {
        Some("user") => {
            // The first user message is the task prompt; later ones are tool
            // results, which the assistant already narrates.
            if v.get("parentUuid").map(|p| p.is_null()).unwrap_or(false) {
                let content = message.get("content")?;
                let text = content.as_str().map(str::to_string).or_else(|| {
                    content
                        .as_array()?
                        .iter()
                        .find_map(|b| b.get("text").and_then(|t| t.as_str()))
                        .map(str::to_string)
                })?;
                Some(format!("● task: {}\n", clip(&text, 400)))
            } else {
                None
            }
        }
        Some("assistant") => {
            let blocks = message.get("content")?.as_array()?;
            let mut out = String::new();
            for block in blocks {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                out.push_str(&clip(trimmed, 1200));
                                out.push('\n');
                            }
                        }
                    }
                    Some("tool_use") => {
                        let name = block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("tool");
                        let detail = block
                            .get("input")
                            .map(summarize_tool_input)
                            .unwrap_or_default();
                        out.push_str(&format!("  ▶ {name}{detail}\n"));
                    }
                    _ => {}
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        }
        _ => None,
    }
}

/// One-line summary of a tool call's input (command, path, or pattern).
fn summarize_tool_input(input: &serde_json::Value) -> String {
    for key in ["command", "file_path", "path", "pattern", "query", "url"] {
        if let Some(val) = input.get(key).and_then(|v| v.as_str()) {
            return format!(": {}", clip(val, 120));
        }
    }
    String::new()
}

/// Clip to `max` characters on a char boundary, appending an ellipsis.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let clipped: String = s.chars().take(max).collect();
    format!("{clipped}…")
}
