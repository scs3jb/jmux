//! System V2 handlers (capabilities, identify, tree, processes).

use std::sync::Arc;

use serde_json::Value;

use crate::app::{lock_or_recover, SharedState};

use super::Response;

// ── /proc helpers ──────────────────────────────────────────────────────────

/// Parse the `tty_nr` field (field 7, 0-indexed field 6) from `/proc/<pid>/stat`.
/// Returns the raw device number encoded as (major << 8 | minor).
fn parse_tty_nr(stat: &str) -> Option<u64> {
    // The comm field `(name)` may contain spaces and parentheses; skip past the
    // last ')' to find the remaining fields.
    let after_comm = stat.rfind(')')?;
    let rest = stat[after_comm + 1..].trim();
    // Fields after comm: state(0) ppid(1) pgrp(2) session(3) tty_nr(4)
    let mut fields = rest.split_ascii_whitespace();
    fields.next()?; // state
    fields.next()?; // ppid
    fields.next()?; // pgrp
    fields.next()?; // session
    let tty_nr_str = fields.next()?;
    tty_nr_str.parse::<i64>().ok().map(|v| v as u64)
}

/// Parse cumulative CPU time (utime + stime, in clock ticks) from `/proc/<pid>/stat`.
fn parse_cpu_ticks(stat: &str) -> Option<u64> {
    let after_comm = stat.rfind(')')?;
    let rest = stat[after_comm + 1..].trim();
    // Fields after comm: state ppid pgrp session tty_nr tpgid flags
    //   minflt cminflt majflt cmajflt utime(12) stime(13)
    let mut fields = rest.split_ascii_whitespace();
    for _ in 0..11 {
        fields.next()?;
    }
    let utime: u64 = fields.next()?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    Some(utime + stime)
}

/// Parse RSS in pages from `/proc/<pid>/stat` (field 24 after comm).
fn parse_rss_pages(stat: &str) -> Option<u64> {
    let after_comm = stat.rfind(')')?;
    let rest = stat[after_comm + 1..].trim();
    // state ppid pgrp session tty_nr tpgid flags minflt cminflt majflt cmajflt
    // utime stime cutime cstime priority nice num_threads itrealvalue starttime
    // vsize rss  →  index 23 (0-based)
    let mut fields = rest.split_ascii_whitespace();
    for _ in 0..23 {
        fields.next()?;
    }
    fields.next()?.parse().ok()
}

/// Parse the process name from `/proc/<pid>/comm` (single line, trimmed).
fn read_proc_comm(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Convert a raw `tty_nr` device number to a pts path like `/dev/pts/N`.
/// Linux encodes pts as: major=136+minor_high bits, minor=pts_index & 0xFF.
/// More precisely: major = (tty_nr >> 8) & 0xFF, minor = (tty_nr & 0xFF) | ((tty_nr >> 12) & 0xFFF00).
fn tty_nr_to_pts_index(tty_nr: u64) -> Option<u32> {
    if tty_nr == 0 {
        return None;
    }
    let major = ((tty_nr >> 8) & 0xFF) as u32;
    if major != 136 {
        // Not a pts device (136 = /dev/pts/*)
        return None;
    }
    let minor = ((tty_nr & 0xFF) | ((tty_nr >> 12) & 0xFFF00)) as u32;
    Some(minor)
}

/// Snapshot of a single process for CPU% calculation.
struct ProcSnapshot {
    pid: u32,
    tty_nr: u64,
    ticks: u64,
    rss_pages: u64,
    comm: String,
}

/// Read one snapshot for every process in `/proc`.
fn snapshot_all_procs() -> Vec<ProcSnapshot> {
    let Ok(proc_dir) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    let mut snaps = Vec::new();
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
        let tty_nr = parse_tty_nr(&stat).unwrap_or(0);
        let ticks = parse_cpu_ticks(&stat).unwrap_or(0);
        let rss_pages = parse_rss_pages(&stat).unwrap_or(0);
        let comm = read_proc_comm(pid).unwrap_or_default();
        snaps.push(ProcSnapshot {
            pid,
            tty_nr,
            ticks,
            rss_pages,
            comm,
        });
    }
    snaps
}

/// Collect per-TTY process info with a brief CPU% measurement interval.
///
/// For each workspace panel that has a `tty_name`, find the foreground
/// process on that TTY (the process with the highest CPU ticks — a simple
/// heuristic that works well for interactive terminal sessions).
fn collect_processes(state: &Arc<SharedState>) -> Vec<Value> {
    // Page size for RSS → MB conversion.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGE_SIZE) } as u64;
    let page_size = if page_size == 0 { 4096 } else { page_size };

    // Clock ticks per second for CPU% calculation.
    let ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;
    let ticks_per_sec = if ticks_per_sec == 0 { 100 } else { ticks_per_sec };

    // Snapshot 1 — before sleep.
    let snap1 = snapshot_all_procs();

    // Sleep 200ms for CPU delta.
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Snapshot 2.
    let snap2 = snapshot_all_procs();

    // Build a map pid → ticks for snapshot1 baseline.
    use std::collections::HashMap;
    let snap1_map: HashMap<u32, u64> = snap1.iter().map(|s| (s.pid, s.ticks)).collect();

    // Collect workspace/panel/tty triples.
    let tty_entries: Vec<(String, String, String, String)> = {
        let tm = lock_or_recover(&state.tab_manager);
        let mut entries = Vec::new();
        for ws in tm.iter() {
            for panel in ws.panels.values() {
                if let Some(ref tty) = panel.tty_name {
                    entries.push((
                        ws.id.to_string(),
                        ws.display_title().to_string(),
                        panel.id.to_string(),
                        tty.clone(),
                    ));
                }
            }
        }
        entries
    };

    // For each panel TTY, find the best foreground process (highest delta ticks).
    let mut results = Vec::new();
    for (ws_id, ws_name, panel_id, tty) in &tty_entries {
        // Parse the pts index from the tty path like /dev/pts/N.
        let pts_index: Option<u32> = tty
            .strip_prefix("/dev/pts/")
            .and_then(|n| n.parse().ok());

        let Some(pts_idx) = pts_index else {
            continue;
        };

        // Find all processes on this TTY.
        let candidates: Vec<_> = snap2
            .iter()
            .filter(|s| tty_nr_to_pts_index(s.tty_nr) == Some(pts_idx))
            .collect();

        if candidates.is_empty() {
            continue;
        }

        // Pick the process with the highest CPU delta (most active).
        let best = candidates
            .iter()
            .max_by_key(|s| {
                let prev = snap1_map.get(&s.pid).copied().unwrap_or(0);
                s.ticks.saturating_sub(prev)
            })
            .unwrap(); // candidates is non-empty

        let delta_ticks = {
            let prev = snap1_map.get(&best.pid).copied().unwrap_or(0);
            best.ticks.saturating_sub(prev)
        };

        // CPU% = (delta_ticks / ticks_per_sec) * 100 over 0.2s interval.
        let cpu_percent = (delta_ticks as f64 / ticks_per_sec as f64) * 100.0 / 0.2;
        let rss_mb = (best.rss_pages * page_size) as f64 / (1024.0 * 1024.0);

        // Status from the stat file.
        let status = {
            let stat_path = format!("/proc/{}/stat", best.pid);
            std::fs::read_to_string(stat_path)
                .ok()
                .and_then(|s| {
                    let after_comm = s.rfind(')')?;
                    let rest = s[after_comm + 1..].trim();
                    let state_char = rest.chars().next()?;
                    Some(match state_char {
                        'R' => "running",
                        'S' => "sleeping",
                        'D' => "disk-wait",
                        'Z' => "zombie",
                        'T' => "stopped",
                        'I' => "idle",
                        _ => "unknown",
                    })
                })
                .unwrap_or("unknown")
        };

        results.push(serde_json::json!({
            "workspace_id": ws_id,
            "workspace_name": ws_name,
            "panel_id": panel_id,
            "tty": tty,
            "pid": best.pid,
            "command": best.comm,
            "cpu_percent": (cpu_percent * 10.0).round() / 10.0,
            "rss_mb": (rss_mb * 10.0).round() / 10.0,
            "status": status,
        }));
    }

    results
}

pub(super) fn handle_system_processes(id: Value, state: &Arc<SharedState>) -> Response {
    let processes = collect_processes(state);
    Response::success(id, serde_json::json!({"processes": processes}))
}

/// `system.task_manager` (`cmux top`) — open the Task Manager window.
pub(super) fn handle_open_task_manager(id: Value, state: &Arc<SharedState>) -> Response {
    state.send_ui_event(crate::app::UiEvent::OpenTaskManager);
    Response::success(id, serde_json::json!({"opened": true}))
}

/// `system.overview` (`cmux overview`) — open the pane overview grid.
pub(super) fn handle_overview(id: Value, state: &Arc<SharedState>) -> Response {
    state.send_ui_event(crate::app::UiEvent::OpenOverview);
    Response::success(id, serde_json::json!({"opened": true}))
}

/// `system.command_palette` (`cmux palette`) — open the command palette.
pub(super) fn handle_command_palette(id: Value, state: &Arc<SharedState>) -> Response {
    state.send_ui_event(crate::app::UiEvent::OpenCommandPalette);
    Response::success(id, serde_json::json!({"opened": true}))
}

/// `system.dock` (`cmux dock`) — show the Dock panel.
pub(super) fn handle_dock(id: Value, state: &Arc<SharedState>) -> Response {
    state.send_ui_event(crate::app::UiEvent::ShowDock);
    Response::success(id, serde_json::json!({"opened": true}))
}

/// `system.run_command` (`cmux run <name>`) — run a cmux.json custom command.
pub(super) fn handle_run_command(id: Value, params: &Value, state: &Arc<SharedState>) -> Response {
    let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
        return Response::error(id, "invalid_params", "Provide 'name'");
    };
    state.send_ui_event(crate::app::UiEvent::RunCustomCommand(name.to_string()));
    Response::success(id, serde_json::json!({"ran": name}))
}

pub(super) fn handle_capabilities(id: Value) -> Response {
    let mut methods: Vec<&str> = vec![
        "system.ping",
        "system.capabilities",
        "system.identify",
        "system.tree",
        "system.processes",
        "system.task_manager",
        "system.overview",
        "system.command_palette",
        "system.dock",
        "system.run_command",
        "workspace.list",
        "workspace.new",
        "workspace.open_history",
        "workspace.open_vault",
        "workspace.reopen_closed_tab",
        "workspace.create",
        "workspace.create_ssh",
        "workspace.remote.status",
        "workspace.select",
        "workspace.next",
        "workspace.previous",
        "workspace.last",
        "workspace.latest_unread",
        "workspace.close",
        "workspace.current",
        "workspace.rename",
        "workspace.ai_name",
        "workspace.reorder",
        "workspace.set_status",
        "workspace.report_git_branch",
        "workspace.set_progress",
        "workspace.clear_progress",
        "workspace.append_log",
        "workspace.clear_status",
        "workspace.list_status",
        "workspace.clear_log",
        "workspace.list_log",
        "workspace.report_meta",
        "workspace.clear_meta",
        "workspace.list_meta",
        "workspace.report_meta_block",
        "workspace.clear_meta_block",
        "workspace.list_meta_blocks",
        "workspace.action",
        "workspace.report_pr",
        "workspace.move_to_window",
        "app.focus_override.set",
        "app.simulate_active",
        "pane.new",
        "pane.split_off",
        "pane.list",
        "pane.focus",
        "pane.close",
        "pane.last",
        "pane.swap",
        "pane.create",
        "pane.resize",
        "pane.focus_direction",
        "pane.break",
        "pane.join",
        "surface.send_input",
        "surface.send_text",
        "surface.list",
        "surface.current",
        "surface.focus",
        "surface.split",
        "surface.close",
        "surface.action",
        "surface.health",
        "surface.send_key",
        "surface.read_text",
        "surface.refresh",
        "surface.clear_history",
        "surface.trigger_flash",
        "surface.move",
        "surface.reorder",
        "surface.create",
        "surface.drag_to_split",
        "tab.action",
        "pane.surfaces",
        "pane.equalize",
        "workspace.report_pwd",
        "workspace.report_ports",
        "workspace.clear_ports",
        "workspace.report_tty",
        "workspace.ports_kick",
        "settings.open",
        "settings.reload",
        "notification.create",
        "notification.create_for_surface",
        "notification.create_for_target",
        "notification.list",
        "notification.clear",
        "markdown.open",
        "window.new",
        "window.list",
        "window.current",
        "window.focus",
        "window.close",
    ];
    #[cfg(feature = "webkit")]
    methods.extend_from_slice(&crate::socket::browser::method_names());
    Response::success(id, serde_json::json!({"methods": methods}))
}

pub(super) fn handle_system_identify(id: Value) -> Response {
    Response::success(
        id,
        serde_json::json!({
            "app": "cmux",
            "platform": "linux",
            "version": env!("CARGO_PKG_VERSION"),
        }),
    )
}

pub(super) fn handle_system_tree(id: Value, state: &Arc<SharedState>) -> Response {
    let tm = lock_or_recover(&state.tab_manager);
    let workspaces: Vec<Value> = tm
        .iter()
        .enumerate()
        .map(|(i, ws)| {
            serde_json::json!({
                "index": i,
                "id": ws.id.to_string(),
                "title": ws.display_title(),
                "selected": tm.selected_index() == Some(i),
                "layout": ws.layout.to_json_tree(&ws.panels),
            })
        })
        .collect();

    Response::success(id, serde_json::json!({"workspaces": workspaces}))
}
