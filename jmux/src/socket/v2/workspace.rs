//! Workspace handler functions for the v2 socket protocol.

use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use crate::app::{lock_or_recover, SharedState, UiEvent};
use crate::model::workspace::truncate_str;
use crate::model::Workspace;

use super::helpers::*;
use super::{
    Response, MAX_BRANCH_LEN, MAX_DIRECTORY_LEN, MAX_METHOD_LEN, MAX_NAME_LEN, MAX_STATUS_LEN,
    MAX_SURFACE_INPUT_LEN, MAX_TITLE_LEN, MAX_URL_LEN,
};

// -----------------------------------------------------------------------
// workspace.list
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_list(id: Value, state: &Arc<SharedState>) -> Response {
    let tm = lock_or_recover(&state.tab_manager);
    let workspaces: Vec<Value> = tm
        .iter()
        .enumerate()
        .map(|(i, ws)| {
            let selected = tm.selected_index() == Some(i);
            serde_json::json!({
                "index": i,
                "id": ws.id.to_string(),
                "title": ws.display_title(),
                "description": ws.description,
                "directory": ws.current_directory,
                "panel_count": ws.panels.len(),
                "unread_count": ws.unread_count,
                "latest_notification": ws.latest_notification,
                "attention_panel_id": ws.attention_panel_id.map(|id| id.to_string()),
                "selected": selected,
                "is_selected": selected,
            })
        })
        .collect();

    Response::success(id, serde_json::json!({"workspaces": workspaces}))
}

// -----------------------------------------------------------------------
// workspace.new / workspace.create
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_new(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    create_workspace(id, params, state, false)
}

/// Create a workspace whose initial panel is a browser surface.
/// Accepts an optional `url` param to open immediately.
pub(super) fn handle_workspace_new_browser(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let mut p = params.clone();
    p["browser"] = serde_json::json!(true);
    create_workspace(id, &p, state, false)
}

/// Create a workspace whose initial panel is a git diff viewer.
/// Accepts an optional `directory` param (the repo path; defaults to the
/// caller-provided cwd).
pub(super) fn handle_workspace_new_diff(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let mut p = params.clone();
    p["kind"] = serde_json::json!("diff");
    create_workspace(id, &p, state, false)
}

/// Create a workspace whose initial panel is a project-structure visualizer.
pub(super) fn handle_workspace_new_project(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let mut p = params.clone();
    p["kind"] = serde_json::json!("project");
    create_workspace(id, &p, state, false)
}

/// Create a workspace whose initial panel is an editable notes scratchpad.
pub(super) fn handle_workspace_new_notes(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let mut p = params.clone();
    p["kind"] = serde_json::json!("notes");
    create_workspace(id, &p, state, false)
}

pub(super) fn handle_workspace_create(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    // Read optional layout string before delegating to create_workspace.
    // Accepted values: "single" (default), "horizontal-2", "vertical-2", "grid-4".
    let layout = params
        .get("layout")
        .and_then(|v| v.as_str())
        .unwrap_or("single")
        .to_string();

    let resp = create_workspace(id, params, state, true);

    // If create succeeded and a non-single layout was requested, apply splits.
    if layout != "single" {
        if let Some(ws_id_str) = resp
            .result
            .as_ref()
            .and_then(|r| r.get("workspace_id"))
            .and_then(|v| v.as_str())
        {
            if let Ok(ws_id) = uuid::Uuid::parse_str(ws_id_str) {
                apply_workspace_layout(ws_id, &layout, state);
            }
        }
    }

    resp
}

/// Apply a named layout to a workspace that was just created.
fn apply_workspace_layout(ws_id: uuid::Uuid, layout: &str, state: &Arc<SharedState>) {
    use crate::model::panel::SplitOrientation;
    use crate::model::PanelType;

    match layout {
        "horizontal-2" => {
            let mut tm = lock_or_recover(&state.tab_manager);
            if let Some(ws) = tm.workspace_mut(ws_id) {
                ws.split(SplitOrientation::Horizontal, PanelType::Terminal);
            }
            drop(tm);
            state.notify_ui_refresh();
        }
        "vertical-2" => {
            let mut tm = lock_or_recover(&state.tab_manager);
            if let Some(ws) = tm.workspace_mut(ws_id) {
                ws.split(SplitOrientation::Vertical, PanelType::Terminal);
            }
            drop(tm);
            state.notify_ui_refresh();
        }
        "grid-4" => {
            // Build a 2×2 grid: split horizontal to get top/bottom, then split
            // each half vertically to get four panels.
            {
                let mut tm = lock_or_recover(&state.tab_manager);
                if let Some(ws) = tm.workspace_mut(ws_id) {
                    // First horizontal split → top and bottom rows
                    ws.split(SplitOrientation::Horizontal, PanelType::Terminal);
                    // Second horizontal split on the first panel → creates top-right
                    // (focus was moved by the first split; split again for row 2)
                }
            }
            // Now split each row vertically to produce the 2×2 grid.
            // We have [panel_a, panel_b] after two horizontal splits; we need
            // to visit each and split vertically.
            {
                let panel_ids: Vec<uuid::Uuid> = {
                    let tm = lock_or_recover(&state.tab_manager);
                    tm.workspace(ws_id)
                        .map(|ws| ws.panel_ids().to_vec())
                        .unwrap_or_default()
                };
                // Focus each panel in turn and split it vertically.
                for pid in panel_ids {
                    let mut tm = lock_or_recover(&state.tab_manager);
                    if let Some(ws) = tm.workspace_mut(ws_id) {
                        ws.focus_panel(pid);
                        ws.split(SplitOrientation::Vertical, PanelType::Terminal);
                    }
                }
            }
            state.notify_ui_refresh();
        }
        _ => {
            tracing::warn!("workspace.create: unknown layout '{layout}', using 'single'");
        }
    }
}

pub(super) fn create_workspace(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
    preserve_selection: bool,
) -> Response {
    let directory = params
        .get("directory")
        .or_else(|| params.get("cwd"))
        .and_then(|v| v.as_str())
        .map(|s| truncate_str(s, MAX_DIRECTORY_LEN));
    let title = params
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| truncate_str(s, MAX_TITLE_LEN));

    let mut ws = if let Some(dir) = directory {
        Workspace::with_directory(dir)
    } else {
        Workspace::new()
    };

    if let Some(t) = title {
        ws.custom_title = Some(t.to_string());
    }

    // Set command on the first panel if provided
    let command = params
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if let Some(ref cmd) = command {
        if let Some(panel) = ws.panels.values_mut().next() {
            panel.command = Some(cmd.clone());
        }
    }

    // If a browser workspace is requested, convert the initial panel to a
    // browser surface (optionally opening a URL). `kind: "browser"` or
    // `browser: true` both work.
    let want_browser = params
        .get("browser")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || params.get("kind").and_then(|v| v.as_str()) == Some("browser");
    if want_browser {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| truncate_str(s, MAX_URL_LEN).to_string());
        let pid = ws
            .focused_panel_id
            .or_else(|| ws.panels.keys().next().copied());
        if let Some(panel) = pid.and_then(|pid| ws.panels.get_mut(&pid)) {
            panel.panel_type = crate::model::PanelType::Browser;
            panel.command = None;
            panel.browser_url = url;
        }
    }

    // If a diff workspace is requested, convert the initial panel to a git
    // diff viewer rooted at the workspace directory.
    if params.get("kind").and_then(|v| v.as_str()) == Some("diff") {
        let diff_dir = ws.current_directory.clone();
        // Diff source: "staged" or "branch:<ref>"; absent = working tree.
        let diff_source = params
            .get("source")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let pid = ws
            .focused_panel_id
            .or_else(|| ws.panels.keys().next().copied());
        if let Some(panel) = pid.and_then(|pid| ws.panels.get_mut(&pid)) {
            panel.panel_type = crate::model::PanelType::Diff;
            panel.command = diff_source;
            panel.directory = Some(diff_dir);
            panel.title = Some("Diff".to_string());
        }
    }

    // If a project workspace is requested, convert the initial panel to a
    // project-structure visualizer rooted at the workspace directory.
    if params.get("kind").and_then(|v| v.as_str()) == Some("project") {
        let proj_dir = ws.current_directory.clone();
        let pid = ws
            .focused_panel_id
            .or_else(|| ws.panels.keys().next().copied());
        if let Some(panel) = pid.and_then(|pid| ws.panels.get_mut(&pid)) {
            panel.panel_type = crate::model::PanelType::Project;
            panel.command = None;
            panel.directory = Some(proj_dir);
            panel.title = Some("Project".to_string());
        }
    }

    // If a notes workspace is requested, convert the initial panel to an
    // editable notes scratchpad backed by a file.
    if params.get("kind").and_then(|v| v.as_str()) == Some("notes") {
        let file = params
            .get("file")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(crate::ui::notes_panel::default_notes_path);
        let pid = ws
            .focused_panel_id
            .or_else(|| ws.panels.keys().next().copied());
        if let Some(panel) = pid.and_then(|pid| ws.panels.get_mut(&pid)) {
            panel.panel_type = crate::model::PanelType::Notes;
            panel.command = None;
            panel.markdown_file = Some(file);
            panel.title = Some("Notes".to_string());
        }
    }

    let ws_id = ws.id;
    let mut tab_manager = lock_or_recover(&state.tab_manager);
    let previously_selected = if preserve_selection {
        tab_manager.selected_id()
    } else {
        None
    };
    let placement = crate::settings::load().new_workspace_placement;
    tab_manager.add_workspace_with_placement(ws, placement);
    if let Some(selected_id) = previously_selected {
        let _ = tab_manager.select_by_id(selected_id);
    }
    drop(tab_manager);
    state.notify_ui_refresh();

    Response::success(
        id,
        serde_json::json!({
            "workspace_id": ws_id.to_string(),
            "workspace": ws_id.to_string()
        }),
    )
}

// -----------------------------------------------------------------------
// workspace.create_ssh
// -----------------------------------------------------------------------

pub(crate) fn handle_workspace_create_ssh(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    if !crate::settings::load().remote_ssh_enabled {
        return Response::error(id, "disabled", "Remote SSH is disabled in settings");
    }

    let destination = match params.get("destination").and_then(|v| v.as_str()) {
        Some(d) => d,
        None => return Response::error(id, "invalid_params", "destination is required"),
    };
    if destination.is_empty() || destination.starts_with('-') {
        return Response::error(id, "invalid_params", "Invalid SSH destination");
    }

    let port = params
        .get("port")
        .and_then(|v| v.as_u64())
        .and_then(|p| u16::try_from(p).ok());
    let identity = params
        .get("identity")
        .and_then(|v| v.as_str())
        .map(String::from);
    let agent_forward = params
        .get("agent_forward")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Build SSH command (shell-escape user-supplied values to prevent injection)
    let mut ssh_cmd = "ssh".to_string();
    // Forward the panel/workspace id to the remote (LC_ names ride stock sshd
    // `AcceptEnv LANG LC_*`) so the remote `claude` wrapper can attribute its
    // session-id report to this tab. Harmless where the host doesn't accept it.
    ssh_cmd += " -o SendEnv=LC_JMUX_PANEL_ID -o SendEnv=LC_JMUX_WORKSPACE_ID";
    if agent_forward {
        ssh_cmd += " -A";
    }
    if let Some(p) = port {
        ssh_cmd += &format!(" -p {}", p);
    }
    if let Some(ref i) = identity {
        ssh_cmd += &format!(" -i {}", shell_escape::escape(i.into()));
    }
    ssh_cmd += &format!(" {}", shell_escape::escape(destination.into()));

    // Build remote config
    let remote_config = crate::remote::session::RemoteConfig {
        destination: destination.to_string(),
        port,
        identity,
        ssh_options: Vec::new(),
        agent_forward,
        remote_daemon_path: None,
    };

    // Create workspace with SSH command and title
    let mut create_params = params.clone();
    create_params["command"] = serde_json::json!(ssh_cmd);
    if create_params
        .get("title")
        .and_then(|v| v.as_str())
        .is_none()
    {
        create_params["title"] = serde_json::json!(destination);
    }

    let no_focus = params
        .get("no_focus")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let response = create_workspace(id, &create_params, state, no_focus);

    // Store remote config on the workspace
    if response.ok {
        if let Some(ws_id_str) = response
            .result
            .as_ref()
            .and_then(|r| r.get("workspace_id"))
            .and_then(|v| v.as_str())
        {
            if let Ok(ws_id) = Uuid::parse_str(ws_id_str) {
                {
                    let mut tm = lock_or_recover(&state.tab_manager);
                    if let Some(ws) = tm.workspace_mut(ws_id) {
                        ws.remote_config = Some(remote_config);
                    }
                }
                // Trigger remote connection
                state.send_ui_event(crate::app::UiEvent::RemoteConnect {
                    workspace_id: ws_id,
                });
            }
        }
    }

    response
}

// -----------------------------------------------------------------------
// workspace.remote_status
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_remote_status(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = parse_workspace_param(params).ok().flatten();

    let tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.workspace(wid)
    } else {
        tm.selected()
    };

    let Some(ws) = ws else {
        return Response::error(id, "not_found", "No workspace found");
    };

    let config = ws.remote_config.as_ref().map(|c| {
        serde_json::json!({
            "destination": c.destination,
            "port": c.port,
            "identity": c.identity,
        })
    });

    let state_json = ws.remote_state.as_ref().map(|s| match s {
        crate::remote::session::RemoteState::Disconnected => serde_json::json!("disconnected"),
        crate::remote::session::RemoteState::Connecting => serde_json::json!("connecting"),
        crate::remote::session::RemoteState::Connected {
            proxy_port,
            daemon_version,
        } => {
            serde_json::json!({
                "state": "connected",
                "proxy_port": proxy_port,
                "daemon_version": daemon_version,
            })
        }
        crate::remote::session::RemoteState::Error(msg) => serde_json::json!({
            "state": "error",
            "message": msg,
        }),
    });

    Response::success(
        id,
        serde_json::json!({
            "is_remote": ws.remote_config.is_some(),
            "config": config,
            "remote_state": state_json,
        }),
    )
}

// -----------------------------------------------------------------------
// workspace.select
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_select(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let index = match parse_usize_param(&id, params, "index") {
        Ok(index) => index,
        Err(response) => return response,
    };
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };

    let mut tm = lock_or_recover(&state.tab_manager);

    let selected = if let Some(idx) = index {
        tm.select(idx)
    } else if let Some(wid) = ws_id {
        tm.select_by_id(wid)
    } else {
        return Response::error(
            id,
            "invalid_params",
            "Provide 'index' or 'workspace'/'workspace_id'",
        );
    };

    if selected {
        let selected_workspace = tm.selected_id();
        drop(tm);
        if let Some(workspace_id) = selected_workspace {
            mark_workspace_read(state, workspace_id);
        }
        state.notify_ui_refresh();
        Response::success(id, serde_json::json!({"selected": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

// -----------------------------------------------------------------------
// workspace.next / workspace.previous / workspace.last
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_next(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let wrap = params.get("wrap").and_then(|v| v.as_bool()).unwrap_or(true);
    let selected_workspace = {
        let mut tm = lock_or_recover(&state.tab_manager);
        tm.select_next(wrap);
        tm.selected_id()
    };
    if let Some(workspace_id) = selected_workspace {
        mark_workspace_read(state, workspace_id);
    }
    state.notify_ui_refresh();
    Response::success(id, serde_json::json!({"ok": true}))
}

pub(super) fn handle_workspace_previous(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let wrap = params.get("wrap").and_then(|v| v.as_bool()).unwrap_or(true);
    let selected_workspace = {
        let mut tm = lock_or_recover(&state.tab_manager);
        tm.select_previous(wrap);
        tm.selected_id()
    };
    if let Some(workspace_id) = selected_workspace {
        mark_workspace_read(state, workspace_id);
    }
    state.notify_ui_refresh();
    Response::success(id, serde_json::json!({"ok": true}))
}

pub(super) fn handle_workspace_last(id: Value, state: &Arc<SharedState>) -> Response {
    let selected_workspace = {
        let mut tm = lock_or_recover(&state.tab_manager);
        tm.select_last();
        tm.selected_id()
    };
    if let Some(workspace_id) = selected_workspace {
        mark_workspace_read(state, workspace_id);
    }
    state.notify_ui_refresh();
    Response::success(id, serde_json::json!({"ok": true}))
}

// -----------------------------------------------------------------------
// workspace.focus_back / workspace.focus_forward
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_focus_back(id: Value, state: &Arc<SharedState>) -> Response {
    let selected = {
        let mut tm = lock_or_recover(&state.tab_manager);
        tm.focus_back()
    };
    if let Some(workspace_id) = selected {
        mark_workspace_read(state, workspace_id);
        state.notify_ui_refresh();
        Response::success(
            id,
            serde_json::json!({"workspace_id": workspace_id.to_string()}),
        )
    } else {
        Response::error(id, "no_history", "No earlier workspace in focus history")
    }
}

pub(super) fn handle_workspace_focus_forward(id: Value, state: &Arc<SharedState>) -> Response {
    let selected = {
        let mut tm = lock_or_recover(&state.tab_manager);
        tm.focus_forward()
    };
    if let Some(workspace_id) = selected {
        mark_workspace_read(state, workspace_id);
        state.notify_ui_refresh();
        Response::success(
            id,
            serde_json::json!({"workspace_id": workspace_id.to_string()}),
        )
    } else {
        Response::error(id, "no_history", "No later workspace in focus history")
    }
}

// -----------------------------------------------------------------------
// workspace.reopen_closed
// -----------------------------------------------------------------------

/// `workspace.open_history` (`jmux history`) — open a History pane.
pub(super) fn handle_workspace_open_history(id: Value, state: &Arc<SharedState>) -> Response {
    open_side_panel(id, state, crate::model::Panel::new_history())
}

/// `workspace.open_vault` (`jmux vault`) — open a Vault pane.
pub(super) fn handle_workspace_open_vault(id: Value, state: &Arc<SharedState>) -> Response {
    open_side_panel(id, state, crate::model::Panel::new_vault())
}

/// Insert `panel` as a split beside the focused pane of the active workspace.
fn open_side_panel(id: Value, state: &Arc<SharedState>, panel: crate::model::Panel) -> Response {
    let panel_id = panel.id;
    let opened = {
        let mut tm = lock_or_recover(&state.tab_manager);
        match tm.selected_mut() {
            Some(ws) => {
                ws.insert_panel(panel, crate::model::panel::SplitOrientation::Horizontal);
                ws.focused_panel_id = Some(panel_id);
                true
            }
            None => false,
        }
    };
    if opened {
        state.notify_ui_refresh();
        Response::success(id, serde_json::json!({"panel_id": panel_id.to_string()}))
    } else {
        Response::error(id, "not_found", "No active workspace")
    }
}

/// `workspace.reopen_closed_tab` (`jmux reopen-tab`) — reopen the most recently
/// closed tab in the active workspace.
pub(super) fn handle_workspace_reopen_closed_tab(id: Value, state: &Arc<SharedState>) -> Response {
    let reopened = {
        let mut tm = lock_or_recover(&state.tab_manager);
        tm.selected_mut().and_then(|ws| ws.reopen_last_closed_panel())
    };
    match reopened {
        Some(panel_id) => {
            state.notify_ui_refresh();
            Response::success(id, serde_json::json!({"panel_id": panel_id.to_string()}))
        }
        None => Response::error(id, "empty", "No recently closed tab to reopen"),
    }
}

/// `workspace.clear_closed` (`jmux clear-closed`) — clear the recently-closed
/// workspace history (the History pane's "Clear Closed").
pub(super) fn handle_workspace_clear_closed(id: Value, state: &Arc<SharedState>) -> Response {
    lock_or_recover(&state.tab_manager).clear_closed();
    state.notify_ui_refresh();
    Response::success(id, serde_json::json!({"cleared": true}))
}

pub(super) fn handle_workspace_reopen_closed(id: Value, state: &Arc<SharedState>) -> Response {
    let reopened = {
        let mut tm = lock_or_recover(&state.tab_manager);
        tm.reopen_last_closed()
    };
    match reopened {
        Some(ws_id) => {
            state.notify_ui_refresh();
            Response::success(id, serde_json::json!({"workspace_id": ws_id.to_string()}))
        }
        None => Response::error(id, "empty", "No recently closed workspace to reopen"),
    }
}

// -----------------------------------------------------------------------
// workspace.hibernate / workspace.wake
// -----------------------------------------------------------------------

/// Resolve the focused panel of the target workspace (param) or the selected one.
fn resolve_focused_panel(params: &Value, state: &Arc<SharedState>) -> Option<Uuid> {
    let tm = lock_or_recover(&state.tab_manager);
    let ws = match parse_workspace_param(params) {
        Ok(Some(wid)) => tm.workspace(wid),
        _ => tm.selected(),
    };
    ws.and_then(|w| w.focused_panel_id)
}

pub(super) fn handle_workspace_hibernate(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let toggle = params
        .get("toggle")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let Some(pid) = resolve_focused_panel(params, state) else {
        return Response::error(id, "not_found", "No focused panel to hibernate");
    };
    let now_hibernated = if toggle && state.is_hibernated(&pid) {
        state.wake_panel(pid);
        false
    } else {
        state.hibernate_panel(pid)
    };
    state.notify_ui_refresh();
    if now_hibernated || toggle {
        Response::success(id, serde_json::json!({"hibernated": now_hibernated}))
    } else {
        Response::error(
            id,
            "failed",
            "Could not locate the agent process to hibernate",
        )
    }
}

pub(super) fn handle_workspace_wake(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let Some(pid) = resolve_focused_panel(params, state) else {
        return Response::error(id, "not_found", "No focused panel to wake");
    };
    state.wake_panel(pid);
    state.notify_ui_refresh();
    Response::success(id, serde_json::json!({"hibernated": false}))
}

/// `workspace.agent_monitor` — toggle (or explicitly `{"enable": bool}`) the
/// read-only sub-agent monitor panes on the selected workspace. Returns the
/// resulting state and how many monitor panes are live after the sync.
pub(super) fn handle_workspace_agent_monitor(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let enable = params.get("enable").and_then(|v| v.as_bool());
    let result = {
        let mut tm = lock_or_recover(&state.tab_manager);
        tm.selected_mut().map(|ws| {
            ws.subagent_monitor = enable.unwrap_or(!ws.subagent_monitor);
            crate::agent_monitor::sync_workspace(ws);
            let panes = ws
                .panels
                .values()
                .filter(|p| p.panel_type == crate::model::PanelType::AgentMonitor)
                .count();
            (ws.subagent_monitor, panes)
        })
    };
    match result {
        Some((enabled, panes)) => {
            state.notify_ui_refresh();
            Response::success(
                id,
                serde_json::json!({"enabled": enabled, "monitor_panes": panes}),
            )
        }
        None => Response::error(id, "not_found", "No selected workspace"),
    }
}

// -----------------------------------------------------------------------
// workspace.latest_unread
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_latest_unread(id: Value, state: &Arc<SharedState>) -> Response {
    let selected_workspace = {
        let mut tm = lock_or_recover(&state.tab_manager);
        tm.select_latest_unread()
    };

    if let Some(workspace_id) = selected_workspace {
        mark_workspace_read(state, workspace_id);
        state.notify_ui_refresh();
        Response::success(
            id,
            serde_json::json!({
                "workspace_id": workspace_id.to_string(),
                "workspace": workspace_id.to_string(),
                "selected": true
            }),
        )
    } else {
        Response::error(id, "not_found", "No unread workspace")
    }
}

// -----------------------------------------------------------------------
// workspace.close
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_close(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let index = match parse_usize_param(&id, params, "index") {
        Ok(index) => index,
        Err(response) => return response,
    };
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };

    let removed = {
        let mut tm = lock_or_recover(&state.tab_manager);
        if let Some(idx) = index {
            tm.remove(idx).is_some()
        } else if let Some(wid) = ws_id {
            tm.remove_by_id(wid).is_some()
        } else if let Some(idx) = tm.selected_index() {
            tm.remove(idx).is_some()
        } else {
            false
        }
    };

    if removed {
        state.notify_ui_refresh();
        Response::success(id, serde_json::json!({"closed": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

// -----------------------------------------------------------------------
// workspace.set_status / workspace.clear_status / workspace.list_status
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_set_status(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let key = params.get("key").and_then(|v| v.as_str());
    let value = params.get("value").and_then(|v| v.as_str());
    let icon = params.get("icon").and_then(|v| v.as_str());
    let color = params.get("color").and_then(|v| v.as_str());
    let url = params.get("url").and_then(|v| v.as_str());

    let (Some(key), Some(value)) = (key, value) else {
        return Response::error(id, "invalid_params", "Provide 'key' and 'value'");
    };

    let updated = {
        let mut tm = lock_or_recover(&state.tab_manager);
        let ws = if let Some(wid) = ws_id {
            tm.workspace_mut(wid)
        } else {
            tm.selected_mut()
        };

        if let Some(ws) = ws {
            ws.set_status_with_url(key, value, icon, color, url);
            true
        } else {
            false
        }
    };

    if updated {
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

pub(super) fn handle_workspace_clear_status(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.workspace_mut(wid)
    } else {
        tm.selected_mut()
    };
    if let Some(ws) = ws {
        ws.status_entries.clear();
        // An agent finishing is a good moment to auto-name an untitled workspace.
        let auto_name = ws.custom_title.is_none();
        let ws_id_for_name = ws.id;
        drop(tm);
        state.notify_metadata_refresh();
        if auto_name && crate::settings::load().ai_auto_naming {
            let state = state.clone();
            std::thread::spawn(move || {
                let _ = ai_name_workspace_core(&state, ws_id_for_name);
            });
        }
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

pub(super) fn handle_workspace_list_status(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.iter().find(|ws| ws.id == wid)
    } else {
        tm.selected()
    };
    if let Some(ws) = ws {
        let entries: Vec<Value> = ws
            .status_entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "key": e.key,
                    "value": e.value,
                    "icon": e.icon,
                    "color": e.color,
                    "url": e.url,
                    "timestamp": e.timestamp,
                })
            })
            .collect();
        Response::success(id, serde_json::json!({"entries": entries}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

// -----------------------------------------------------------------------
// workspace.report_git
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_report_git(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let branch = params.get("branch").and_then(|v| v.as_str());
    let is_dirty = params
        .get("is_dirty")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let surface_id = params
        .get("surface")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());

    let Some(branch) = branch else {
        return Response::error(id, "invalid_params", "Provide 'branch'");
    };

    let git_branch = if branch.is_empty() {
        None
    } else {
        Some(crate::model::panel::GitBranch {
            branch: truncate_str(branch, MAX_BRANCH_LEN).to_string(),
            is_dirty,
        })
    };

    let updated = {
        let mut tm = lock_or_recover(&state.tab_manager);
        let ws = if let Some(wid) = ws_id {
            tm.workspace_mut(wid)
        } else {
            tm.selected_mut()
        };

        if let Some(ws) = ws {
            // Always update workspace-level branch
            ws.git_branch = git_branch.clone();
            // Also update per-panel branch when surface ID is provided
            if let Some(pid) = surface_id {
                if let Some(panel) = ws.panels.get_mut(&pid) {
                    panel.git_branch = git_branch;
                }
            }
            true
        } else {
            false
        }
    };

    if updated {
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

// -----------------------------------------------------------------------
// workspace.set_progress / workspace.clear_progress
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_set_progress(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let value = params.get("value").and_then(|v| v.as_f64());
    let label = params.get("label").and_then(|v| v.as_str());

    let updated = {
        let mut tm = lock_or_recover(&state.tab_manager);
        let ws = if let Some(wid) = ws_id {
            tm.workspace_mut(wid)
        } else {
            tm.selected_mut()
        };

        if let Some(ws) = ws {
            if let Some(value) = value {
                ws.progress = Some(crate::model::workspace::Progress {
                    value,
                    label: label.map(|s| s.to_string()),
                });
            } else {
                ws.progress = None;
            }
            true
        } else {
            false
        }
    };

    if updated {
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

pub(super) fn handle_workspace_clear_progress(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.workspace_mut(wid)
    } else {
        tm.selected_mut()
    };
    if let Some(ws) = ws {
        ws.progress = None;
        drop(tm);
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

// -----------------------------------------------------------------------
// workspace.append_log / workspace.clear_log / workspace.list_log
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_append_log(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let message = params.get("message").and_then(|v| v.as_str());
    let level = params
        .get("level")
        .and_then(|v| v.as_str())
        .unwrap_or("info");
    let source = params.get("source").and_then(|v| v.as_str());

    let Some(message) = message else {
        return Response::error(id, "invalid_params", "Provide 'message'");
    };

    let updated = {
        let mut tm = lock_or_recover(&state.tab_manager);
        let ws = if let Some(wid) = ws_id {
            tm.workspace_mut(wid)
        } else {
            tm.selected_mut()
        };

        if let Some(ws) = ws {
            ws.append_log(message, level, source);
            true
        } else {
            false
        }
    };

    if updated {
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

pub(super) fn handle_workspace_clear_log(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.workspace_mut(wid)
    } else {
        tm.selected_mut()
    };
    if let Some(ws) = ws {
        ws.log_entries.clear();
        drop(tm);
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

pub(super) fn handle_workspace_list_log(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.iter().find(|ws| ws.id == wid)
    } else {
        tm.selected()
    };
    if let Some(ws) = ws {
        let entries: Vec<Value> = ws
            .log_entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "message": e.message,
                    "level": e.level,
                    "source": e.source,
                    "timestamp": e.timestamp,
                })
            })
            .collect();
        Response::success(id, serde_json::json!({"entries": entries}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

// -----------------------------------------------------------------------
// Metadata entry handlers
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_report_meta(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let key = params.get("key").and_then(|v| v.as_str());
    let value = params.get("value").and_then(|v| v.as_str());
    let (Some(key), Some(value)) = (key, value) else {
        return Response::error(id, "invalid_params", "Provide 'key' and 'value'");
    };

    let icon = params.get("icon").and_then(|v| v.as_str());
    let color = params.get("color").and_then(|v| v.as_str());
    let url = params.get("url").and_then(|v| v.as_str());
    let priority = params
        .get("priority")
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok())
        .unwrap_or(0);
    let format = match params.get("format").and_then(|v| v.as_str()) {
        Some("markdown") => crate::model::workspace::MetadataFormat::Markdown,
        _ => crate::model::workspace::MetadataFormat::Plain,
    };

    let updated = {
        let mut tm = lock_or_recover(&state.tab_manager);
        let ws = if let Some(wid) = ws_id {
            tm.workspace_mut(wid)
        } else {
            tm.selected_mut()
        };
        if let Some(ws) = ws {
            ws.set_metadata(key, value, icon, color, url, priority, format);
            true
        } else {
            false
        }
    };

    if updated {
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

pub(super) fn handle_workspace_clear_meta(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let key = params.get("key").and_then(|v| v.as_str());

    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.workspace_mut(wid)
    } else {
        tm.selected_mut()
    };
    if let Some(ws) = ws {
        if let Some(key) = key {
            ws.clear_metadata(key);
        } else {
            ws.metadata_entries.clear();
        }
        drop(tm);
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

pub(super) fn handle_workspace_list_meta(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.iter().find(|ws| ws.id == wid)
    } else {
        tm.selected()
    };
    if let Some(ws) = ws {
        let mut entries: Vec<&crate::model::workspace::MetadataEntry> =
            ws.metadata_entries.iter().collect();
        entries.sort_by(|a, b| {
            b.priority.cmp(&a.priority).then(
                a.timestamp
                    .partial_cmp(&b.timestamp)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        let entries: Vec<Value> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "key": e.key,
                    "value": e.value,
                    "icon": e.icon,
                    "color": e.color,
                    "url": e.url,
                    "priority": e.priority,
                    "format": e.format,
                    "timestamp": e.timestamp,
                })
            })
            .collect();
        Response::success(id, serde_json::json!({"entries": entries}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

// -----------------------------------------------------------------------
// Metadata block handlers
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_report_meta_block(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let key = params.get("key").and_then(|v| v.as_str());
    let content = params.get("content").and_then(|v| v.as_str());
    let (Some(key), Some(content)) = (key, content) else {
        return Response::error(id, "invalid_params", "Provide 'key' and 'content'");
    };
    let priority = params
        .get("priority")
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok())
        .unwrap_or(0);

    let updated = {
        let mut tm = lock_or_recover(&state.tab_manager);
        let ws = if let Some(wid) = ws_id {
            tm.workspace_mut(wid)
        } else {
            tm.selected_mut()
        };
        if let Some(ws) = ws {
            ws.set_metadata_block(key, content, priority);
            true
        } else {
            false
        }
    };

    if updated {
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

pub(super) fn handle_workspace_clear_meta_block(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let key = params.get("key").and_then(|v| v.as_str());

    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.workspace_mut(wid)
    } else {
        tm.selected_mut()
    };
    if let Some(ws) = ws {
        if let Some(key) = key {
            ws.clear_metadata_block(key);
        } else {
            ws.metadata_blocks.clear();
        }
        drop(tm);
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

pub(super) fn handle_workspace_list_meta_blocks(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.iter().find(|ws| ws.id == wid)
    } else {
        tm.selected()
    };
    if let Some(ws) = ws {
        let mut blocks: Vec<&crate::model::workspace::MetadataBlock> =
            ws.metadata_blocks.iter().collect();
        blocks.sort_by(|a, b| {
            b.priority.cmp(&a.priority).then(
                a.timestamp
                    .partial_cmp(&b.timestamp)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        let blocks: Vec<Value> = blocks
            .iter()
            .map(|b| {
                serde_json::json!({
                    "key": b.key,
                    "content": b.content,
                    "priority": b.priority,
                    "timestamp": b.timestamp,
                })
            })
            .collect();
        Response::success(id, serde_json::json!({"blocks": blocks}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

// -----------------------------------------------------------------------
// workspace.current
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_current(id: Value, state: &Arc<SharedState>) -> Response {
    let tm = lock_or_recover(&state.tab_manager);
    if let Some(ws) = tm.selected() {
        let index = tm.selected_index().unwrap_or(0);
        Response::success(
            id,
            serde_json::json!({
                "index": index,
                "id": ws.id.to_string(),
                "title": ws.display_title(),
                "directory": ws.current_directory,
                "panel_count": ws.panels.len(),
                "focused_panel_id": ws.focused_panel_id.map(|id| id.to_string()),
            }),
        )
    } else {
        Response::error(id, "not_found", "No workspace selected")
    }
}

// -----------------------------------------------------------------------
// workspace.rename
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_rename(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let title = params.get("title").and_then(|v| v.as_str());

    let Some(title) = title else {
        return Response::error(id, "invalid_params", "Provide 'title'");
    };

    let updated = {
        let mut tm = lock_or_recover(&state.tab_manager);
        let ws = if let Some(wid) = ws_id {
            tm.workspace_mut(wid)
        } else {
            tm.selected_mut()
        };

        if let Some(ws) = ws {
            ws.custom_title = Some(truncate_str(title, MAX_TITLE_LEN).to_string());
            true
        } else {
            false
        }
    };

    if updated {
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

pub(super) fn handle_workspace_set_description(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    // An empty/absent description clears it.
    let description = params
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| truncate_str(s, MAX_DIRECTORY_LEN).to_string())
        .filter(|s| !s.trim().is_empty());

    let updated = {
        let mut tm = lock_or_recover(&state.tab_manager);
        let ws = if let Some(wid) = ws_id {
            tm.workspace_mut(wid)
        } else {
            tm.selected_mut()
        };
        if let Some(ws) = ws {
            ws.description = description;
            true
        } else {
            false
        }
    };

    if updated {
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

// -----------------------------------------------------------------------
// workspace.ai_name (AI-generated title from the focused terminal transcript)
// -----------------------------------------------------------------------

/// Read a workspace's focused terminal, ask the model for a title, and apply it.
/// Blocking (HTTP + scrollback read) — run on a tokio worker or its own thread,
/// never the GTK main thread.
pub(super) fn ai_name_workspace_core(
    state: &Arc<SharedState>,
    ws_id: uuid::Uuid,
) -> Result<String, String> {
    let panel_id = {
        let mut tm = lock_or_recover(&state.tab_manager);
        let ws = tm.workspace_mut(ws_id).ok_or("Workspace not found")?;
        ws.focused_panel_id.ok_or("No focused panel to read")?
    };

    let (tx, rx) = tokio::sync::oneshot::channel();
    state.send_ui_event(crate::app::UiEvent::ReadText {
        panel_id,
        scrollback: true,
        lines: Some(150),
        reply: tx,
    });
    let transcript = rx
        .blocking_recv()
        .ok()
        .flatten()
        .ok_or("Could not read terminal contents")?;

    let title = crate::ai::generate_workspace_title(&transcript)?;
    let truncated = truncate_str(&title, MAX_TITLE_LEN).to_string();
    {
        let mut tm = lock_or_recover(&state.tab_manager);
        if let Some(ws) = tm.workspace_mut(ws_id) {
            ws.custom_title = Some(truncated.clone());
        }
    }
    state.notify_metadata_refresh();
    Ok(truncated)
}

pub(super) fn handle_workspace_ai_name(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id_opt = params
        .get("workspace")
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok());

    let ws_id = {
        let mut tm = lock_or_recover(&state.tab_manager);
        let ws = match ws_id_opt {
            Some(wid) => tm.workspace_mut(wid),
            None => tm.selected_mut(),
        };
        match ws {
            Some(ws) => ws.id,
            None => return Response::error(id, "not_found", "Workspace not found"),
        }
    };

    match ai_name_workspace_core(state, ws_id) {
        Ok(title) => Response::success(id, serde_json::json!({ "title": title })),
        Err(e) => Response::error(id, "ai_error", &e),
    }
}

// -----------------------------------------------------------------------
// workspace.reorder_workspaces (batch reorder by name/index)
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_reorder_workspaces(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let names = match params.get("workspaces").and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect::<Vec<_>>(),
        None => return Response::error(id, "invalid_params", "Provide 'workspaces' array of names"),
    };

    if names.is_empty() {
        return Response::error(id, "invalid_params", "'workspaces' array must not be empty");
    }

    let mut tm = lock_or_recover(&state.tab_manager);

    // Build the target order: resolve each name to a workspace index.
    // Names that match no workspace are rejected immediately.
    let mut target_ids: Vec<uuid::Uuid> = Vec::with_capacity(names.len());
    for name in &names {
        // Try numeric index first
        if let Ok(idx) = name.parse::<usize>() {
            match tm.get(idx) {
                Some(ws) => target_ids.push(ws.id),
                None => {
                    return Response::error(
                        id,
                        "not_found",
                        &format!("No workspace at index {idx}"),
                    )
                }
            }
        } else {
            // Match by display title (case-sensitive, first match)
            match tm.iter().find(|ws| ws.display_title() == *name) {
                Some(ws) => target_ids.push(ws.id),
                None => {
                    return Response::error(
                        id,
                        "not_found",
                        &format!("No workspace named '{name}'"),
                    )
                }
            }
        }
    }

    // Deduplicate while preserving first-occurrence order
    {
        let mut seen = std::collections::HashSet::new();
        target_ids.retain(|id| seen.insert(*id));
    }

    // Build the new workspace order: listed workspaces first (in requested
    // order), then any remaining workspaces that were not mentioned.
    let all_ids: Vec<uuid::Uuid> = tm.iter().map(|ws| ws.id).collect();
    let mut new_order: Vec<uuid::Uuid> = target_ids.clone();
    for ws_id in &all_ids {
        if !new_order.contains(ws_id) {
            new_order.push(*ws_id);
        }
    }

    // Apply the order via sequential move_workspace calls (stable sort by
    // placing each element at its desired position left-to-right).
    for (desired_idx, ws_id) in new_order.iter().enumerate() {
        if let Some(current_idx) = tm.workspace_index(*ws_id) {
            if current_idx != desired_idx {
                tm.move_workspace(current_idx, desired_idx);
            }
        }
    }

    drop(tm);
    state.notify_ui_refresh();

    Response::success(
        id,
        serde_json::json!({"reordered": new_order.iter().map(|id| id.to_string()).collect::<Vec<_>>()}),
    )
}

// -----------------------------------------------------------------------
// workspace.reorder
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_reorder(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let from = match parse_usize_param(&id, params, "from") {
        Ok(v) => v,
        Err(response) => return response,
    };
    let to = match parse_usize_param(&id, params, "to") {
        Ok(v) => v,
        Err(response) => return response,
    };

    let (Some(from), Some(to)) = (from, to) else {
        return Response::error(id, "invalid_params", "Provide 'from' and 'to'");
    };

    let moved = lock_or_recover(&state.tab_manager).move_workspace(from, to);
    if moved {
        state.notify_ui_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "invalid_params", "Invalid workspace indices")
    }
}

// -----------------------------------------------------------------------
// workspace.action
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_action(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let action = params.get("action").and_then(|v| v.as_str());

    let Some(action) = action else {
        return Response::error(id, "invalid_params", "Provide 'action'");
    };

    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.workspace_mut(wid)
    } else {
        tm.selected_mut()
    };

    let Some(ws) = ws else {
        return Response::error(id, "not_found", "No workspace");
    };

    let ws_id = ws.id;
    match action {
        "pin" => ws.is_pinned = true,
        "unpin" => ws.is_pinned = false,
        "toggle_pin" => ws.is_pinned = !ws.is_pinned,
        "mark_read" => {
            ws.mark_notifications_read();
            drop(tm);
            mark_workspace_read(state, ws_id);
            state.notify_metadata_refresh();
            return Response::success(id, serde_json::json!({"ok": true}));
        }
        "mark_unread" => {
            ws.unread_count = ws.unread_count.max(1);
            drop(tm);
            state.notify_metadata_refresh();
            return Response::success(id, serde_json::json!({"ok": true}));
        }
        "clear_name" => {
            ws.custom_title = None;
            drop(tm);
            state.notify_metadata_refresh();
            return Response::success(id, serde_json::json!({"ok": true}));
        }
        "set_color" => {
            let color = params.get("color").and_then(|v| v.as_str());
            let Some(color) = color else {
                return Response::error(
                    id,
                    "invalid_params",
                    "set_color requires 'color' param",
                );
            };
            ws.custom_color = Some(truncate_str(color, MAX_STATUS_LEN).to_string());
            drop(tm);
            state.notify_metadata_refresh();
            return Response::success(id, serde_json::json!({"ok": true}));
        }
        "clear_color" => {
            ws.custom_color = None;
            drop(tm);
            state.notify_metadata_refresh();
            return Response::success(id, serde_json::json!({"ok": true}));
        }
        "rename" => {
            let title = params.get("title").and_then(|v| v.as_str());
            let Some(title) = title else {
                return Response::error(
                    id,
                    "invalid_params",
                    "rename requires 'title' param",
                );
            };
            ws.custom_title = Some(truncate_str(title, MAX_METHOD_LEN).to_string());
            drop(tm);
            state.notify_metadata_refresh();
            return Response::success(id, serde_json::json!({"ok": true}));
        }
        "move_up" => {
            let idx = tm.workspace_index(ws_id).unwrap_or(0);
            drop(tm);
            let new_idx = idx.saturating_sub(1);
            let mut tm = lock_or_recover(&state.tab_manager);
            tm.move_workspace(idx, new_idx);
            drop(tm);
            state.notify_ui_refresh();
            return Response::success(id, serde_json::json!({"index": new_idx}));
        }
        "move_down" => {
            let idx = tm.workspace_index(ws_id).unwrap_or(0);
            let len = tm.len();
            drop(tm);
            let new_idx = (idx + 1).min(len - 1);
            let mut tm = lock_or_recover(&state.tab_manager);
            tm.move_workspace(idx, new_idx);
            drop(tm);
            state.notify_ui_refresh();
            return Response::success(id, serde_json::json!({"index": new_idx}));
        }
        "move_top" => {
            let idx = tm.workspace_index(ws_id).unwrap_or(0);
            drop(tm);
            let mut tm = lock_or_recover(&state.tab_manager);
            tm.move_workspace(idx, 0);
            drop(tm);
            state.notify_ui_refresh();
            return Response::success(id, serde_json::json!({"index": 0}));
        }
        "close_others" => {
            drop(tm);
            let mut tm = lock_or_recover(&state.tab_manager);
            let count = tm.close_others(ws_id);
            drop(tm);
            state.notify_ui_refresh();
            return Response::success(id, serde_json::json!({"closed": count}));
        }
        "close_above" => {
            drop(tm);
            let mut tm = lock_or_recover(&state.tab_manager);
            let count = tm.close_above(ws_id);
            drop(tm);
            state.notify_ui_refresh();
            return Response::success(id, serde_json::json!({"closed": count}));
        }
        "close_below" => {
            drop(tm);
            let mut tm = lock_or_recover(&state.tab_manager);
            let count = tm.close_below(ws_id);
            drop(tm);
            state.notify_ui_refresh();
            return Response::success(id, serde_json::json!({"closed": count}));
        }
        _ => {
            return Response::error(
                id,
                "invalid_params",
                "Unknown action. Use: pin, unpin, toggle_pin, mark_read, mark_unread, clear_name, set_color, clear_color, rename, move_up, move_down, move_top, close_others, close_above, close_below",
            )
        }
    }

    let pinned = ws.is_pinned;
    drop(tm);
    state.notify_metadata_refresh();

    Response::success(id, serde_json::json!({"is_pinned": pinned}))
}

// -----------------------------------------------------------------------
// workspace.report_pwd
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_report_pwd(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let directory = match params.get("directory").and_then(|v| v.as_str()) {
        Some(d) => truncate_str(d, MAX_DIRECTORY_LEN).to_string(),
        None => return Response::error(id, "invalid_params", "Provide 'directory'"),
    };

    let panel_id = params
        .get("panel")
        .or_else(|| params.get("surface"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());

    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };

    let mut tm = lock_or_recover(&state.tab_manager);

    // Find the workspace: by panel, by workspace ID, or selected
    let ws = if let Some(pid) = panel_id {
        tm.find_workspace_with_panel_mut(pid)
    } else if let Some(wid) = ws_id {
        tm.workspace_mut(wid)
    } else {
        tm.selected_mut()
    };

    let Some(ws) = ws else {
        return Response::error(id, "not_found", "No workspace");
    };

    // Set panel directory if a panel was specified
    if let Some(pid) = panel_id {
        if let Some(panel) = ws.panels.get_mut(&pid) {
            panel.directory = Some(directory.clone());
        }
        // If this is the focused panel, also update workspace directory
        if ws.focused_panel_id == Some(pid) {
            ws.current_directory = directory;
        }
    } else {
        // No panel specified — update workspace directory
        ws.current_directory = directory;
    }

    drop(tm);
    state.notify_metadata_refresh();
    Response::success(id, serde_json::json!({"ok": true}))
}

// -----------------------------------------------------------------------
// workspace.report_ports / workspace.clear_ports
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_report_ports(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ports: Vec<u16> = match params.get("ports").and_then(|v| v.as_array()) {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_u64().and_then(|n| u16::try_from(n).ok()))
            .take(256)
            .collect(),
        None => return Response::error(id, "invalid_params", "Provide 'ports' array"),
    };

    let panel_id = params
        .get("panel")
        .or_else(|| params.get("surface"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());

    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(pid) = panel_id {
        tm.find_workspace_with_panel_mut(pid)
    } else {
        tm.selected_mut()
    };

    let Some(ws) = ws else {
        return Response::error(id, "not_found", "No workspace");
    };

    let target_panel_id = panel_id.or(ws.focused_panel_id);
    if let Some(pid) = target_panel_id {
        if let Some(panel) = ws.panels.get_mut(&pid) {
            panel.listening_ports = ports;
        }
    }

    drop(tm);
    state.notify_metadata_refresh();
    Response::success(id, serde_json::json!({"ok": true}))
}

pub(super) fn handle_workspace_clear_ports(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let panel_id = params
        .get("panel")
        .or_else(|| params.get("surface"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());

    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(pid) = panel_id {
        tm.find_workspace_with_panel_mut(pid)
    } else {
        tm.selected_mut()
    };

    let Some(ws) = ws else {
        return Response::error(id, "not_found", "No workspace");
    };

    let target_panel_id = panel_id.or(ws.focused_panel_id);
    if let Some(pid) = target_panel_id {
        if let Some(panel) = ws.panels.get_mut(&pid) {
            panel.listening_ports.clear();
        }
    }

    drop(tm);
    state.notify_metadata_refresh();
    Response::success(id, serde_json::json!({"ok": true}))
}

// -----------------------------------------------------------------------
// workspace.report_tty
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_report_tty(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let tty = match params.get("tty").and_then(|v| v.as_str()) {
        Some(t) => truncate_str(t, 256).to_string(),
        None => return Response::error(id, "invalid_params", "Provide 'tty'"),
    };

    let panel_id = params
        .get("panel")
        .or_else(|| params.get("surface"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());

    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(pid) = panel_id {
        tm.find_workspace_with_panel_mut(pid)
    } else {
        tm.selected_mut()
    };

    let Some(ws) = ws else {
        return Response::error(id, "not_found", "No workspace");
    };

    let target_panel_id = panel_id.or(ws.focused_panel_id);
    if let Some(pid) = target_panel_id {
        if let Some(panel) = ws.panels.get_mut(&pid) {
            panel.tty_name = Some(tty);
        }
    }

    drop(tm);
    Response::success(id, serde_json::json!({"ok": true}))
}

/// Record the exact agent session id the shell `claude` wrapper pinned for a
/// tab (`report_agent_session claude <uuid> --panel=<id>`), so a restored tab
/// can `claude --resume <uuid>` into the same conversation. Sent over the unix
/// socket locally and, from a remote host, via the relay. Only Claude Code is
/// resumed by id today; other agents are accepted-and-ignored for forward
/// compatibility. The id must be a valid UUID (Claude's session id form).
pub(super) fn handle_workspace_report_agent_session(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let agent = params
        .get("agent")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let session_id = match params
        .get("session_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s.trim()).ok())
    {
        Some(u) => u.to_string(),
        None => return Response::error(id, "invalid_params", "Provide a UUID 'session_id'"),
    };

    // Only Claude is resumable by session id right now; ack others without
    // storing so the wrapper can stay agent-agnostic.
    if !agent.contains("claude") {
        return Response::success(id, serde_json::json!({"ok": true, "ignored": true}));
    }

    let panel_id = params
        .get("panel")
        .or_else(|| params.get("surface"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());

    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(pid) = panel_id {
        tm.find_workspace_with_panel_mut(pid)
    } else {
        tm.selected_mut()
    };

    let Some(ws) = ws else {
        return Response::error(id, "not_found", "No workspace");
    };

    let target_panel_id = panel_id.or(ws.focused_panel_id);
    if let Some(pid) = target_panel_id {
        if let Some(panel) = ws.panels.get_mut(&pid) {
            panel.agent_session_id = Some(session_id);
        }
    }

    drop(tm);
    Response::success(id, serde_json::json!({"ok": true}))
}

// -----------------------------------------------------------------------
// workspace.ports_kick (no-op for API parity)
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_ports_kick(id: Value) -> Response {
    Response::success(id, serde_json::json!({"ok": true}))
}

// -----------------------------------------------------------------------
// workspace.report_pr / workspace.report_pr_checks
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_report_pr(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let status = params.get("status").and_then(|v| v.as_str());
    let url = params.get("url").and_then(|v| v.as_str());

    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.workspace_mut(wid)
    } else {
        tm.selected_mut()
    };

    let Some(ws) = ws else {
        return Response::error(id, "not_found", "No workspace");
    };

    ws.pr_status = status.map(|s| truncate_str(s, MAX_STATUS_LEN).to_string());
    ws.pr_url = url.map(|s| truncate_str(s, MAX_URL_LEN).to_string());

    drop(tm);
    state.notify_metadata_refresh();
    Response::success(id, serde_json::json!({"updated": true}))
}

pub(super) fn handle_workspace_report_pr_checks(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let checks = params.get("checks").and_then(|v| v.as_array());

    let Some(checks_arr) = checks else {
        return Response::error(id, "invalid_params", "Provide 'checks' array");
    };

    let parsed: Vec<crate::model::workspace::PrCheck> = checks_arr
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?;
            let conclusion = item
                .get("conclusion")
                .and_then(|v| v.as_str())
                .unwrap_or("PENDING");
            Some(crate::model::workspace::PrCheck {
                name: truncate_str(name, MAX_NAME_LEN).to_string(),
                conclusion: conclusion.to_uppercase(),
            })
        })
        .take(20) // Cap at 20 checks
        .collect();

    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.workspace_mut(wid)
    } else {
        tm.selected_mut()
    };

    let Some(ws) = ws else {
        return Response::error(id, "not_found", "No workspace");
    };

    ws.pr_checks = parsed;
    drop(tm);
    state.notify_metadata_refresh();
    Response::success(id, serde_json::json!({"updated": true}))
}

// -----------------------------------------------------------------------
// workspace.set_imessage_mode
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_set_imessage_mode(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let enabled = match params.get("enabled").and_then(|v| v.as_bool()) {
        Some(b) => b,
        None => return Response::error(id, "invalid_params", "Provide 'enabled' (bool)"),
    };

    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.workspace_mut(wid)
    } else {
        tm.selected_mut()
    };

    if let Some(ws) = ws {
        ws.imessage_mode = enabled;
        drop(tm);
        state.notify_metadata_refresh();
        Response::success(id, serde_json::json!({"ok": true, "imessage_mode": enabled}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

// -----------------------------------------------------------------------
// workspace.move_to_window
// -----------------------------------------------------------------------

pub(super) fn handle_workspace_move_to_window(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let target_window = params
        .get("window")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok());

    let Some(target_window) = target_window else {
        return Response::error(id, "invalid_params", "Provide 'window' (UUID)");
    };

    let mut tm = lock_or_recover(&state.tab_manager);
    let ws = if let Some(wid) = ws_id {
        tm.workspace_mut(wid)
    } else {
        tm.selected_mut()
    };

    if let Some(ws) = ws {
        ws.window_id = Some(target_window);
        drop(tm);
        state.notify_ui_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace not found")
    }
}

// -----------------------------------------------------------------------
// settings.open
// -----------------------------------------------------------------------

pub(super) fn handle_settings_open(id: Value, state: &Arc<SharedState>) -> Response {
    state.send_ui_event(UiEvent::OpenSettings);
    Response::success(id, serde_json::json!({"opened": true}))
}

// -----------------------------------------------------------------------
// settings.reload
// -----------------------------------------------------------------------

/// Re-apply the theme from settings and notify ghostty of the current
/// dark/light state.  Useful for scripting or after editing settings.json
/// directly when the Settings window is not open.
pub(super) fn handle_settings_reload(id: Value, state: &Arc<SharedState>) -> Response {
    state.send_ui_event(UiEvent::ReloadTheme);
    Response::success(id, serde_json::json!({"reloaded": true}))
}

// -----------------------------------------------------------------------
// agent.fork_conversation
// -----------------------------------------------------------------------

pub(super) fn handle_agent_fork_conversation(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    // Optional message to pre-populate in the new terminal.
    let message = params
        .get("message")
        .and_then(|v| v.as_str())
        .map(|s| truncate_str(s, MAX_SURFACE_INPUT_LEN).to_string());

    // Optional workspace name.
    let workspace_name = params
        .get("workspace_name")
        .or_else(|| params.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| truncate_str(s, MAX_TITLE_LEN).to_string());

    // Inherit cwd from the currently selected workspace.
    let cwd = {
        let tm = lock_or_recover(&state.tab_manager);
        tm.selected()
            .map(|ws| ws.current_directory.clone())
            .unwrap_or_default()
    };

    // Build create params.
    let mut create_params = serde_json::json!({});
    if !cwd.is_empty() {
        create_params["directory"] = serde_json::json!(cwd);
    }
    if let Some(ref name) = workspace_name {
        create_params["title"] = serde_json::json!(name);
    }

    // Create the new workspace (preserve selection so the source stays selected).
    let create_resp = create_workspace(id.clone(), &create_params, state, true);
    if !create_resp.ok {
        return create_resp;
    }

    let ws_id_str = match create_resp
        .result
        .as_ref()
        .and_then(|r| r.get("workspace_id"))
        .and_then(|v| v.as_str())
    {
        Some(s) => s.to_string(),
        None => return Response::error(id, "internal", "workspace_id missing from create response"),
    };

    // If a message was provided, send it to the new workspace's terminal.
    if let Some(ref msg) = message {
        if let Ok(ws_id) = Uuid::parse_str(&ws_id_str) {
            let panel_id = {
                let tm = lock_or_recover(&state.tab_manager);
                tm.workspace(ws_id)
                    .and_then(|ws| ws.focused_panel_id.or_else(|| ws.panel_ids().into_iter().next()))
            };
            if let Some(panel_id) = panel_id {
                state.send_ui_event(crate::app::UiEvent::SendInput {
                    panel_id,
                    text: msg.clone(),
                });
            }
        }
    }

    Response::success(
        id,
        serde_json::json!({
            "workspace_id": ws_id_str,
            "workspace": ws_id_str,
        }),
    )
}

// -----------------------------------------------------------------------
// agent.spawn_subagent
// -----------------------------------------------------------------------

pub(super) fn handle_agent_spawn_subagent(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let parent_panel_id = match params
        .get("parent_panel_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
    {
        Some(pid) => pid,
        None => {
            return Response::error(
                id,
                "invalid_params",
                "Provide 'parent_panel_id' (UUID of parent panel)",
            )
        }
    };

    let cli_name = params
        .get("cli_name")
        .and_then(|v| v.as_str())
        .map(|s| truncate_str(s, MAX_NAME_LEN).to_string());

    let working_directory = params
        .get("working_directory")
        .and_then(|v| v.as_str())
        .map(|s| truncate_str(s, MAX_DIRECTORY_LEN).to_string());

    // Find the workspace containing the parent panel
    let (ws_id, parent_dir) = {
        let tm = lock_or_recover(&state.tab_manager);
        if let Some(ws) = tm.find_workspace_with_panel(parent_panel_id) {
            let dir = working_directory
                .clone()
                .or_else(|| {
                    ws.panels
                        .get(&parent_panel_id)
                        .and_then(|p| p.directory.clone())
                })
                .unwrap_or_else(|| ws.current_directory.clone());
            (ws.id, dir)
        } else {
            return Response::error(id, "not_found", "Parent panel not found");
        }
    };

    // Create a new terminal panel as a subagent sibling
    let mut new_panel = crate::model::panel::Panel::new_terminal();
    new_panel.parent_panel_id = Some(parent_panel_id);
    new_panel.directory = Some(parent_dir.clone());
    if let Some(ref cli) = cli_name {
        new_panel.title = Some(format!("{cli} (subagent)"));
    }
    let new_panel_id = new_panel.id;

    {
        let mut tm = lock_or_recover(&state.tab_manager);
        if let Some(ws) = tm.workspace_mut(ws_id) {
            // Set focused panel to parent so split is adjacent
            ws.focused_panel_id = Some(parent_panel_id);
            ws.insert_panel(new_panel, crate::model::panel::SplitOrientation::Horizontal);
        } else {
            return Response::error(id, "not_found", "Workspace no longer exists");
        }
    }

    state.notify_ui_refresh();
    Response::success(
        id,
        serde_json::json!({
            "panel_id": new_panel_id.to_string(),
            "workspace_id": ws_id.to_string(),
        }),
    )
}

// -----------------------------------------------------------------------
// app.focus_override.set
// -----------------------------------------------------------------------

pub(super) fn handle_app_focus_override(
    id: Value,
    params: &Value,
    _state: &Arc<SharedState>,
) -> Response {
    let _active = params
        .get("active")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // Focus override is a macOS-specific feature (NSApp.isActive override).
    // On Linux/GTK this is a no-op but we accept the command for protocol parity.
    Response::success(id, serde_json::json!({"ok": true}))
}

// -----------------------------------------------------------------------
// app.simulate_active
// -----------------------------------------------------------------------

pub(super) fn handle_app_simulate_active(
    id: Value,
    _params: &Value,
    _state: &Arc<SharedState>,
) -> Response {
    // Simulate app activation — on Linux/GTK this is a no-op for protocol parity.
    Response::success(id, serde_json::json!({"ok": true}))
}

// -----------------------------------------------------------------------
// workspace.files
// -----------------------------------------------------------------------

/// List the directory tree of the workspace's `current_directory` (or an
/// explicit `path` param) up to `depth` levels (default 2, max 5).
///
/// Request:
/// ```json
/// {"id":1,"method":"workspace.files","params":{"path":"/optional","depth":2}}
/// ```
///
/// Response:
/// ```json
/// {"ok":true,"result":{"path":"/home/user/project","entries":[
///   {"name":"src","path":"…/src","type":"directory","children":[…]},
///   {"name":"main.rs","path":"…/main.rs","type":"file"}
/// ]}}
/// ```
pub(super) fn handle_workspace_files(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };

    // Resolve the workspace and check for SSH
    let (root_path, is_remote) = {
        let tm = lock_or_recover(&state.tab_manager);
        let ws = if let Some(wid) = ws_id {
            tm.workspace(wid)
        } else {
            tm.selected()
        };
        let Some(ws) = ws else {
            return Response::error(id, "not_found", "No workspace found");
        };
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| ws.current_directory.clone());
        (path, ws.remote_config.is_some())
    };

    if is_remote {
        return Response::error(
            id,
            "not_supported",
            "File browsing is not available for SSH workspaces",
        );
    }

    let depth = params
        .get("depth")
        .and_then(|v| v.as_u64())
        .map(|d| d.min(5) as u32)
        .unwrap_or(2);

    let entries = files_tree(&root_path, depth, 0);

    Response::success(
        id,
        serde_json::json!({
            "path": root_path,
            "entries": entries,
        }),
    )
}

fn files_tree(dir_path: &str, max_depth: u32, current_depth: u32) -> Vec<Value> {
    use std::path::Path;

    if current_depth >= max_depth {
        return Vec::new();
    }

    let Ok(read_dir) = std::fs::read_dir(Path::new(dir_path)) else {
        return Vec::new();
    };

    let mut dirs: Vec<(String, String)> = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();

    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let full = entry.path().to_string_lossy().into_owned();
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => dirs.push((name, full)),
            Ok(_) => files.push((name, full)),
            Err(_) => {}
        }
    }

    dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    files.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    let mut result = Vec::new();

    for (name, full) in dirs {
        let children = files_tree(&full, max_depth, current_depth + 1);
        result.push(serde_json::json!({
            "name": name,
            "path": full,
            "type": "directory",
            "children": children,
        }));
    }

    for (name, full) in files {
        result.push(serde_json::json!({
            "name": name,
            "path": full,
            "type": "file",
        }));
    }

    result
}
