//! Vault pane — a searchable index of past agent sessions (Claude Code and
//! Codex) that can be resumed in a fresh terminal.
//!
//! Mirrors jmux's Vault. It scans each agent's on-disk session store
//! (`~/.claude/projects/**/*.jsonl`, `~/.codex/sessions/**/*.jsonl`), extracts
//! a title / working directory / preview from the transcript head, and lets you
//! click a session to open a terminal that runs the agent's resume command in
//! the original directory. Search filters by title, directory, and preview.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use gtk4::prelude::*;
use uuid::Uuid;

use crate::app::{lock_or_recover, AppState};
use crate::model::Panel;

/// Maximum bytes read from a session file's head when extracting metadata.
const HEAD_BYTES: usize = 32 * 1024;
/// Cap on the number of session files scanned (newest by mtime win).
const MAX_SESSIONS: usize = 400;

#[derive(Clone)]
struct Session {
    agent: &'static str,
    id: String,
    title: String,
    directory: Option<String>,
    preview: String,
    mtime: SystemTime,
}

impl Session {
    /// Command to resume this session in a shell.
    fn resume_command(&self) -> String {
        match self.agent {
            "Claude Code" => format!("claude --resume {}", self.id),
            "Codex" => format!("codex resume {}", self.id),
            _ => String::new(),
        }
    }
}

/// Build the Vault pane widget.
pub fn create_vault_widget(
    panel_id: Uuid,
    state: &Rc<AppState>,
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

    // ── Toolbar ──
    let toolbar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
    toolbar.add_css_class("browser-nav-bar");
    toolbar.set_margin_start(6);
    toolbar.set_margin_end(6);
    toolbar.set_margin_top(2);
    toolbar.set_margin_bottom(2);

    let icon = gtk4::Image::from_icon_name("drive-multidisk-symbolic");
    icon.set_pixel_size(16);
    toolbar.append(&icon);

    let search = gtk4::SearchEntry::new();
    search.set_placeholder_text(Some("Search agent sessions…"));
    search.set_hexpand(true);
    toolbar.append(&search);

    let reload_btn = gtk4::Button::from_icon_name("view-refresh-symbolic");
    reload_btn.add_css_class("flat");
    reload_btn.set_tooltip_text(Some("Rescan sessions"));
    toolbar.append(&reload_btn);

    container.append(&toolbar);

    // ── Body ──
    let list = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
    list.set_margin_start(8);
    list.set_margin_end(8);
    list.set_margin_top(6);
    list.set_margin_bottom(6);

    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scrolled.set_hexpand(true);
    scrolled.set_vexpand(true);
    scrolled.set_child(Some(&list));
    container.append(&scrolled);

    // Scan once; cache for fast filtering on each keystroke.
    let sessions = Rc::new(scan_sessions());
    populate(&list, state, &sessions, "");

    {
        let list = list.clone();
        let state = state.clone();
        let sessions = sessions.clone();
        search.connect_search_changed(move |entry| {
            populate(&list, &state, &sessions, entry.text().as_str());
        });
    }
    {
        let list = list.clone();
        let state = state.clone();
        let search = search.clone();
        reload_btn.connect_clicked(move |_| {
            let sessions = Rc::new(scan_sessions());
            populate(&list, &state, &sessions, search.text().as_str());
        });
    }

    container.upcast()
}

fn populate(list: &gtk4::Box, state: &Rc<AppState>, sessions: &Rc<Vec<Session>>, filter: &str) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let needle = filter.trim().to_lowercase();
    let matched: Vec<&Session> = sessions
        .iter()
        .filter(|s| {
            needle.is_empty()
                || s.title.to_lowercase().contains(&needle)
                || s.preview.to_lowercase().contains(&needle)
                || s
                    .directory
                    .as_deref()
                    .map(|d| d.to_lowercase().contains(&needle))
                    .unwrap_or(false)
        })
        .collect();

    if matched.is_empty() {
        let msg = if sessions.is_empty() {
            "No agent sessions found.\nRun Claude Code or Codex, then refresh."
        } else {
            "No sessions match your search."
        };
        let empty = gtk4::Label::new(Some(msg));
        empty.add_css_class("dim-label");
        empty.set_margin_top(12);
        list.append(&empty);
        return;
    }

    for s in matched {
        let row = session_button(s);
        let state = state.clone();
        let session = s.clone();
        row.connect_clicked(move |_| {
            open_session(&state, &session);
        });
        list.append(&row);
    }
}

fn session_button(s: &Session) -> gtk4::Button {
    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 1);

    let top = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let badge = gtk4::Label::new(Some(s.agent));
    badge.add_css_class("caption");
    badge.add_css_class("dim-label");
    top.append(&badge);
    let title = gtk4::Label::new(Some(&s.title));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    top.append(&title);
    let when = gtk4::Label::new(Some(&relative_time(s.mtime)));
    when.add_css_class("dim-label");
    when.add_css_class("caption");
    top.append(&when);
    vbox.append(&top);

    if let Some(dir) = &s.directory {
        let dir_label = gtk4::Label::new(Some(dir));
        dir_label.set_xalign(0.0);
        dir_label.add_css_class("dim-label");
        dir_label.add_css_class("caption");
        dir_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        vbox.append(&dir_label);
    }

    let btn = gtk4::Button::new();
    btn.set_child(Some(&vbox));
    btn.add_css_class("flat");
    btn.set_tooltip_text(Some(&format!("Resume: {}", s.resume_command())));
    btn
}

/// Open a terminal in the current workspace that resumes the session.
fn open_session(state: &Rc<AppState>, session: &Session) {
    let command = session.resume_command();
    if command.is_empty() {
        return;
    }
    {
        let mut tm = lock_or_recover(&state.shared.tab_manager);
        if let Some(ws) = tm.selected_mut() {
            let mut panel = Panel::new_terminal();
            panel.command = Some(command);
            panel.custom_title = Some(session.title.clone());
            if let Some(dir) = &session.directory {
                panel.directory = Some(dir.clone());
            }
            let new_id = panel.id;
            ws.panels.insert(new_id, panel);
            let target = ws
                .focused_panel_id
                .or_else(|| ws.layout.all_panel_ids().into_iter().next());
            if let Some(target) = target {
                ws.layout.add_panel_to_pane(target, new_id);
            }
            ws.previous_focused_panel_id = ws.focused_panel_id;
            ws.focused_panel_id = Some(new_id);
        }
    }
    state.shared.notify_ui_refresh();
}

// ── Scanning ────────────────────────────────────────────────────────────

fn scan_sessions() -> Vec<Session> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let mut sessions = Vec::new();

    // Claude Code: ~/.claude/projects/<encoded-cwd>/<session>.jsonl
    let claude = home.join(".claude/projects");
    if claude.is_dir() {
        collect_jsonl(&claude, &mut sessions, "Claude Code", 3);
    }

    // Codex: ~/.codex/sessions/**/<rollout>.jsonl
    let codex = home.join(".codex/sessions");
    if codex.is_dir() {
        collect_jsonl(&codex, &mut sessions, "Codex", 5);
    }

    // Newest first; cap.
    sessions.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    sessions.truncate(MAX_SESSIONS);
    sessions
}

/// Recursively collect `*.jsonl` session files under `root` (bounded depth).
fn collect_jsonl(root: &Path, out: &mut Vec<Session>, agent: &'static str, depth: usize) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            if depth > 0 {
                collect_jsonl(&path, out, agent, depth - 1);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            if let Some(session) = parse_session(&path, agent) {
                out.push(session);
            }
            if out.len() >= MAX_SESSIONS * 2 {
                return;
            }
        }
    }
}

fn parse_session(path: &Path, agent: &'static str) -> Option<Session> {
    use std::io::Read;
    let id = path.file_stem()?.to_str()?.to_string();
    let mtime = std::fs::metadata(path).ok()?.modified().ok()?;

    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; HEAD_BYTES];
    let n = file.read(&mut buf).unwrap_or(0);
    buf.truncate(n);
    let head = String::from_utf8_lossy(&buf);

    let mut directory = None;
    let mut title = None;
    for line in head.lines().take(60) {
        let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if directory.is_none() {
            if let Some(cwd) = val.get("cwd").and_then(|v| v.as_str()) {
                directory = Some(cwd.to_string());
            }
        }
        if title.is_none() {
            if let Some(text) = first_user_text(&val) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    title = Some(trimmed.chars().take(120).collect::<String>());
                }
            }
        }
        if directory.is_some() && title.is_some() {
            break;
        }
    }

    let title = title.unwrap_or_else(|| format!("Session {}", short_id(&id)));
    let preview: String = head.chars().take(2000).collect();
    Some(Session {
        agent,
        id,
        title,
        directory,
        preview,
        mtime,
    })
}

/// Extract the first user-authored text from a transcript event, tolerating the
/// different shapes Claude Code and Codex use.
fn first_user_text(val: &serde_json::Value) -> Option<String> {
    // Role lives either at the top level or under "message".
    let role = val
        .get("role")
        .or_else(|| val.get("message").and_then(|m| m.get("role")))
        .and_then(|v| v.as_str());
    if role != Some("user") {
        return None;
    }
    let content = val
        .get("content")
        .or_else(|| val.get("message").and_then(|m| m.get("content")))
        .or_else(|| val.get("text"))?;
    match content {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(items) => items.iter().find_map(|it| {
            it.get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        }),
        _ => None,
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Coarse "5m ago" / "3h ago" / "2d ago" relative time.
fn relative_time(t: SystemTime) -> String {
    let Ok(elapsed) = t.elapsed() else {
        return String::new();
    };
    let secs = elapsed.as_secs();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}
