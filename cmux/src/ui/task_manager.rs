//! Task Manager window — shows CPU/memory usage for all workspace terminal processes.

use std::rc::Rc;

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::app::{lock_or_recover, AppState};
use crate::model::PanelType;

/// One row of process data displayed in the task manager.
#[derive(Debug, Clone)]
struct ProcessRow {
    workspace_name: String,
    panel_title: String,
    command: String,
    pid: u32,
    cpu_percent: f64,
    rss_mb: f64,
    status: String,
}

/// Sort column for the task manager table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortColumn {
    Workspace,
    Panel,
    Command,
    Cpu,
    Memory,
    Pid,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortDir {
    Asc,
    Desc,
}

/// Create and present the Task Manager window.
pub fn show_task_manager(parent: &adw::ApplicationWindow, state: &Rc<AppState>) {
    let window = adw::ApplicationWindow::builder()
        .title("Task Manager")
        .transient_for(parent)
        .default_width(720)
        .default_height(400)
        .modal(false)
        .build();

    let header_bar = adw::HeaderBar::new();
    header_bar.set_show_end_title_buttons(true);

    // Shared sort state wrapped in Rc<Cell> so timeout callback can capture it.
    let sort_col = Rc::new(std::cell::Cell::new(SortColumn::Cpu));
    let sort_dir = Rc::new(std::cell::Cell::new(SortDir::Desc));

    // Main vertical box.
    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    vbox.append(&header_bar);

    // Scrolled window contains the text-based process table.
    let scrolled = gtk4::ScrolledWindow::new();
    scrolled.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Automatic);
    scrolled.set_vexpand(true);
    scrolled.set_hexpand(true);

    let text_view = gtk4::TextView::new();
    text_view.set_editable(false);
    text_view.set_cursor_visible(false);
    text_view.set_monospace(true);
    text_view.set_left_margin(8);
    text_view.set_right_margin(8);
    text_view.set_top_margin(8);
    text_view.set_bottom_margin(8);
    scrolled.set_child(Some(&text_view));

    // Bottom bar with sort controls.
    let sort_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
    sort_box.set_margin_start(8);
    sort_box.set_margin_end(8);
    sort_box.set_margin_top(4);
    sort_box.set_margin_bottom(4);

    let sort_label = gtk4::Label::new(Some("Sort by:"));
    sort_box.append(&sort_label);

    let columns = [
        ("Workspace", SortColumn::Workspace),
        ("Panel", SortColumn::Panel),
        ("Command", SortColumn::Command),
        ("CPU%", SortColumn::Cpu),
        ("Memory", SortColumn::Memory),
        ("PID", SortColumn::Pid),
    ];

    for (label, col) in &columns {
        let btn = gtk4::Button::with_label(label);
        btn.add_css_class("flat");
        let col = *col;
        let sort_col_c = sort_col.clone();
        let sort_dir_c = sort_dir.clone();
        let state_c = state.clone();
        let tv = text_view.clone();
        btn.connect_clicked(move |_| {
            if sort_col_c.get() == col {
                // Toggle direction.
                sort_dir_c.set(if sort_dir_c.get() == SortDir::Asc {
                    SortDir::Desc
                } else {
                    SortDir::Asc
                });
            } else {
                sort_col_c.set(col);
                sort_dir_c.set(SortDir::Desc);
            }
            let rows = collect_rows(&state_c);
            render_table(&tv, &rows, sort_col_c.get(), sort_dir_c.get());
        });
        sort_box.append(&btn);
    }

    vbox.append(&scrolled);
    vbox.append(&sort_box);
    window.set_content(Some(&vbox));

    // Initial render.
    {
        let rows = collect_rows(state);
        render_table(&text_view, &rows, sort_col.get(), sort_dir.get());
    }

    // Auto-refresh every 2 seconds.
    let tv_weak = text_view.downgrade();
    let window_weak = window.downgrade();
    let state_weak = Rc::downgrade(state);
    let sort_col_c = sort_col.clone();
    let sort_dir_c = sort_dir.clone();
    glib::timeout_add_seconds_local(2, move || {
        // Stop if the window was closed.
        if window_weak.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        let Some(tv) = tv_weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let Some(state) = state_weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let rows = collect_rows(&state);
        render_table(&tv, &rows, sort_col_c.get(), sort_dir_c.get());
        glib::ControlFlow::Continue
    });

    window.present();
}

/// Collect process rows from the tab manager.
///
/// For each terminal panel that has a TTY and a known command (from panel title
/// or the shell process visible in the OS), we build a row. Since the GTK
/// main thread cannot block for 200ms (that would freeze the UI), we use the
/// panel's process title set by ghostty's SET_TITLE action, and we read
/// `/proc` non-blocking for quick per-process stats.
fn collect_rows(state: &Rc<AppState>) -> Vec<ProcessRow> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGE_SIZE) } as u64;
    let page_size = if page_size == 0 { 4096 } else { page_size };

    let ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;
    let ticks_per_sec = if ticks_per_sec == 0 { 100 } else { ticks_per_sec };

    let tm = lock_or_recover(&state.shared.tab_manager);
    let mut rows = Vec::new();

    for ws in tm.iter() {
        for panel in ws.panels.values() {
            if panel.panel_type != PanelType::Terminal {
                continue;
            }
            let Some(ref tty) = panel.tty_name else {
                continue;
            };
            let pts_index: Option<u32> = tty
                .strip_prefix("/dev/pts/")
                .and_then(|n| n.parse().ok());
            let Some(pts_idx) = pts_index else {
                continue;
            };

            // Find the foreground process for this PTY by scanning /proc.
            if let Some(proc_info) = find_foreground_proc(pts_idx, page_size, ticks_per_sec) {
                rows.push(ProcessRow {
                    workspace_name: ws.display_title().to_string(),
                    panel_title: panel.display_title().to_string(),
                    command: proc_info.0,
                    pid: proc_info.1,
                    cpu_percent: proc_info.2,
                    rss_mb: proc_info.3,
                    status: proc_info.4,
                });
            }
        }
    }

    rows
}

/// Find the most-recently-active (highest tick) process on a given pts index.
/// Returns (command, pid, cpu_percent, rss_mb, status) or None.
fn find_foreground_proc(
    pts_idx: u32,
    page_size: u64,
    ticks_per_sec: u64,
) -> Option<(String, u32, f64, f64, String)> {
    let Ok(proc_dir) = std::fs::read_dir("/proc") else {
        return None;
    };

    // Snapshot 1
    let mut snap1: Vec<(u32, u64)> = Vec::new();
    for entry in proc_dir.flatten() {
        let fname = entry.file_name();
        let name = fname.to_string_lossy();
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        let stat_path = format!("/proc/{pid}/stat");
        let Ok(stat) = std::fs::read_to_string(&stat_path) else {
            continue;
        };
        if let Some(tty_nr) = super_parse_tty_nr(&stat) {
            if tty_nr_to_pts(tty_nr) == Some(pts_idx) {
                let ticks = super_parse_cpu_ticks(&stat).unwrap_or(0);
                snap1.push((pid, ticks));
            }
        }
    }

    if snap1.is_empty() {
        return None;
    }

    // Short sleep for CPU measurement — 100ms is acceptable in a timer callback.
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Snapshot 2: re-read the same PIDs (no need to re-scan /proc).
    let interval_secs = 0.1_f64;
    let mut best_pid = snap1[0].0;
    let mut best_delta = 0u64;
    let mut best_rss = 0u64;
    let mut best_comm = String::new();
    let mut best_status = "unknown".to_string();

    for (pid, ticks1) in &snap1 {
        let stat_path = format!("/proc/{pid}/stat");
        let Ok(stat2) = std::fs::read_to_string(&stat_path) else {
            continue;
        };
        let ticks2 = super_parse_cpu_ticks(&stat2).unwrap_or(0);
        let delta = ticks2.saturating_sub(*ticks1);
        if delta >= best_delta {
            best_delta = delta;
            best_pid = *pid;
            best_rss = super_parse_rss_pages(&stat2).unwrap_or(0);
            best_comm = read_proc_comm_inner(*pid).unwrap_or_default();
            best_status = read_proc_status_char(&stat2);
        }
    }

    let cpu_percent = (best_delta as f64 / ticks_per_sec as f64) * 100.0 / interval_secs;
    let rss_mb = (best_rss * page_size) as f64 / (1024.0 * 1024.0);

    Some((
        best_comm,
        best_pid,
        (cpu_percent * 10.0).round() / 10.0,
        (rss_mb * 10.0).round() / 10.0,
        best_status,
    ))
}

fn super_parse_tty_nr(stat: &str) -> Option<u64> {
    let after_comm = stat.rfind(')')?;
    let rest = stat[after_comm + 1..].trim();
    let mut fields = rest.split_ascii_whitespace();
    fields.next()?; // state
    fields.next()?; // ppid
    fields.next()?; // pgrp
    fields.next()?; // session
    let s = fields.next()?;
    s.parse::<i64>().ok().map(|v| v as u64)
}

fn super_parse_cpu_ticks(stat: &str) -> Option<u64> {
    let after_comm = stat.rfind(')')?;
    let rest = stat[after_comm + 1..].trim();
    let mut fields = rest.split_ascii_whitespace();
    for _ in 0..11 {
        fields.next()?;
    }
    let u: u64 = fields.next()?.parse().ok()?;
    let s: u64 = fields.next()?.parse().ok()?;
    Some(u + s)
}

fn super_parse_rss_pages(stat: &str) -> Option<u64> {
    let after_comm = stat.rfind(')')?;
    let rest = stat[after_comm + 1..].trim();
    let mut fields = rest.split_ascii_whitespace();
    for _ in 0..23 {
        fields.next()?;
    }
    fields.next()?.parse().ok()
}

fn tty_nr_to_pts(tty_nr: u64) -> Option<u32> {
    if tty_nr == 0 {
        return None;
    }
    let major = ((tty_nr >> 8) & 0xFF) as u32;
    if major != 136 {
        return None;
    }
    let minor = ((tty_nr & 0xFF) | ((tty_nr >> 12) & 0xFFF00)) as u32;
    Some(minor)
}

fn read_proc_comm_inner(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_proc_status_char(stat: &str) -> String {
    let after_comm = match stat.rfind(')') {
        Some(i) => i,
        None => return "unknown".to_string(),
    };
    let rest = stat[after_comm + 1..].trim();
    match rest.chars().next() {
        Some('R') => "running",
        Some('S') => "sleeping",
        Some('D') => "disk-wait",
        Some('Z') => "zombie",
        Some('T') => "stopped",
        Some('I') => "idle",
        _ => "unknown",
    }
    .to_string()
}

/// Render sorted rows into the TextView buffer.
fn render_table(
    tv: &gtk4::TextView,
    rows: &[ProcessRow],
    sort_col: SortColumn,
    sort_dir: SortDir,
) {
    let mut sorted = rows.to_vec();
    sorted.sort_by(|a, b| {
        let ord = match sort_col {
            SortColumn::Workspace => a.workspace_name.cmp(&b.workspace_name),
            SortColumn::Panel => a.panel_title.cmp(&b.panel_title),
            SortColumn::Command => a.command.cmp(&b.command),
            SortColumn::Cpu => a
                .cpu_percent
                .partial_cmp(&b.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::Memory => a
                .rss_mb
                .partial_cmp(&b.rss_mb)
                .unwrap_or(std::cmp::Ordering::Equal),
            SortColumn::Pid => a.pid.cmp(&b.pid),
        };
        if sort_dir == SortDir::Desc {
            ord.reverse()
        } else {
            ord
        }
    });

    let header = format!(
        "{:<20} {:<16} {:<16} {:>7} {:>10} {:>8}  {}\n{}\n",
        "Workspace",
        "Panel",
        "Command",
        "CPU%",
        "Mem (MB)",
        "PID",
        "Status",
        "-".repeat(92),
    );

    let body: String = sorted
        .iter()
        .map(|r| {
            format!(
                "{:<20} {:<16} {:<16} {:>7.1} {:>10.1} {:>8}  {}\n",
                truncate(&r.workspace_name, 20),
                truncate(&r.panel_title, 16),
                truncate(&r.command, 16),
                r.cpu_percent,
                r.rss_mb,
                r.pid,
                r.status,
            )
        })
        .collect();

    let text = if sorted.is_empty() {
        format!("{header}  (no terminal panels with TTY information)\n")
    } else {
        format!("{header}{body}")
    };

    let buf = tv.buffer();
    buf.set_text(&text);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}
