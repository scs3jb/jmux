//! Workspace group handlers for the v2 socket protocol.

use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use crate::app::{lock_or_recover, SharedState};

use super::helpers::*;
use super::Response;

/// Parse a required `group` param (UUID string). Returns Err on missing/invalid.
fn parse_group_param(params: &Value) -> Result<Uuid, ()> {
    params
        .get("group")
        .or_else(|| params.get("group_id"))
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or(())
}

// -----------------------------------------------------------------------
// workspace.group.create
// -----------------------------------------------------------------------

pub(super) fn handle_group_create(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("New Group")
        .to_string();

    let group_id = {
        let mut tm = lock_or_recover(&state.tab_manager);
        // Inherit the window of the currently selected workspace, if any.
        let win = tm.selected().and_then(|ws| ws.window_id);
        tm.create_group(name, win)
    };
    state.notify_ui_refresh();
    Response::success(id, serde_json::json!({"group_id": group_id.to_string()}))
}

// -----------------------------------------------------------------------
// workspace.group.list
// -----------------------------------------------------------------------

pub(super) fn handle_group_list(id: Value, state: &Arc<SharedState>) -> Response {
    let tm = lock_or_recover(&state.tab_manager);
    let groups: Vec<Value> = tm
        .groups()
        .iter()
        .map(|g| {
            let members: Vec<String> = tm
                .iter()
                .filter(|ws| ws.group_id == Some(g.id))
                .map(|ws| ws.id.to_string())
                .collect();
            serde_json::json!({
                "id": g.id.to_string(),
                "name": g.name,
                "color": g.color,
                "collapsed": g.collapsed,
                "unread_count": tm.group_unread_count(g.id),
                "workspace_ids": members,
            })
        })
        .collect();
    Response::success(id, serde_json::json!({"groups": groups}))
}

// -----------------------------------------------------------------------
// workspace.group.assign
// -----------------------------------------------------------------------

pub(super) fn handle_group_assign(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let ws_id = match parse_workspace_param(params) {
        Ok(Some(v)) => v,
        Ok(None) => {
            // Fall back to the selected workspace.
            match lock_or_recover(&state.tab_manager).selected_id() {
                Some(v) => v,
                None => return Response::error(id, "not_found", "No workspace specified"),
            }
        }
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };

    // `group: null` (or absent) ungroups; otherwise parse the target group.
    let group_id = match params.get("group").or_else(|| params.get("group_id")) {
        Some(v) if v.is_null() => None,
        Some(v) => match v.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
            Some(g) => Some(g),
            None => return Response::error(id, "invalid_params", "Invalid group UUID"),
        },
        None => None,
    };

    let ok = {
        let mut tm = lock_or_recover(&state.tab_manager);
        tm.assign_to_group(ws_id, group_id)
    };
    if ok {
        state.notify_ui_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Workspace or group not found")
    }
}

// -----------------------------------------------------------------------
// workspace.group.rename
// -----------------------------------------------------------------------

pub(super) fn handle_group_rename(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let group_id = match parse_group_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid group UUID"),
    };
    let Some(name) = params.get("name").and_then(|v| v.as_str()) else {
        return Response::error(id, "invalid_params", "Provide 'name'");
    };
    let ok = lock_or_recover(&state.tab_manager).rename_group(group_id, name);
    if ok {
        state.notify_ui_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Group not found")
    }
}

// -----------------------------------------------------------------------
// workspace.group.collapse
// -----------------------------------------------------------------------

pub(super) fn handle_group_collapse(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let group_id = match parse_group_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid group UUID"),
    };
    let collapsed = params.get("collapsed").and_then(|v| v.as_bool());
    let result = lock_or_recover(&state.tab_manager).set_group_collapsed(group_id, collapsed);
    match result {
        Some(new_state) => {
            state.notify_ui_refresh();
            Response::success(id, serde_json::json!({"collapsed": new_state}))
        }
        None => Response::error(id, "not_found", "Group not found"),
    }
}

// -----------------------------------------------------------------------
// workspace.group.color
// -----------------------------------------------------------------------

pub(super) fn handle_group_color(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let group_id = match parse_group_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid group UUID"),
    };
    // `color: null` or empty clears it.
    let color = params
        .get("color")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let ok = lock_or_recover(&state.tab_manager).set_group_color(group_id, color);
    if ok {
        state.notify_ui_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Group not found")
    }
}

// -----------------------------------------------------------------------
// workspace.group.delete
// -----------------------------------------------------------------------

pub(super) fn handle_group_delete(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let group_id = match parse_group_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid group UUID"),
    };
    let ok = lock_or_recover(&state.tab_manager).remove_group(group_id);
    if ok {
        state.notify_ui_refresh();
        Response::success(id, serde_json::json!({"ok": true}))
    } else {
        Response::error(id, "not_found", "Group not found")
    }
}
