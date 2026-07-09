//! Notification V2 handlers.

use std::sync::Arc;

use serde_json::Value;

use crate::app::{lock_or_recover, SharedState};

use super::helpers::parse_workspace_param;
use super::Response;

pub(super) fn handle_notification_create(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let title = crate::model::workspace::truncate_str(
        params
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("jmux"),
        1024,
    );
    let body = crate::model::workspace::truncate_str(
        params.get("body").and_then(|v| v.as_str()).unwrap_or(""),
        8192,
    );
    let workspace_id = match parse_workspace_param(params) {
        Ok(v) => v,
        Err(()) => return Response::error(id, "invalid_params", "Invalid workspace UUID"),
    };
    let panel_id = match params
        .get("surface")
        .or_else(|| params.get("panel"))
        .and_then(|v| if v.is_null() { None } else { Some(v) })
    {
        Some(v) => {
            let Some(s) = v.as_str() else {
                return Response::error(id, "invalid_params", "surface/panel must be a string");
            };
            match uuid::Uuid::parse_str(s) {
                Ok(uuid) => Some(uuid),
                Err(_) => {
                    return Response::error(
                        id,
                        "invalid_params",
                        "Invalid surface/panel UUID format",
                    )
                }
            }
        }
        None => None,
    };
    let send_desktop = params
        .get("send_desktop")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let target = {
        let mut tm = lock_or_recover(&state.tab_manager);
        let target_workspace_id = if let Some(workspace_id) = workspace_id {
            if tm.workspace(workspace_id).is_some() {
                Some(workspace_id)
            } else {
                return Response::error(id, "not_found", "Workspace not found");
            }
        } else if let Some(panel_id) = panel_id {
            tm.find_workspace_with_panel(panel_id).map(|ws| ws.id)
        } else {
            tm.selected_id()
        };

        let Some(target_workspace_id) = target_workspace_id else {
            return Response::error(id, "not_found", "No workspace selected");
        };

        let workspace = tm
            .workspace_mut(target_workspace_id)
            .expect("workspace validated above");
        let resolved_panel_id = panel_id.filter(|id| workspace.panels.contains_key(id));

        // Suppress notifications from nested/sub-agent panels (Task 2).
        // When a top-level agent (e.g. Claude Code) spawns a sub-agent shell,
        // the sub-agent panel has parent_panel_id set.  Hook events from that
        // sub-shell should not pollute the notification panel.
        if let Some(pid) = resolved_panel_id {
            if let Some(panel) = workspace.panels.get(&pid) {
                if panel.parent_panel_id.is_some() {
                    tracing::debug!(
                        %pid,
                        parent = ?panel.parent_panel_id,
                        "notification suppressed for nested agent panel"
                    );
                    return Response::success(
                        id,
                        serde_json::json!({
                            "notified": false,
                            "suppressed": true,
                            "reason": "nested_agent_panel",
                            "workspace": target_workspace_id.to_string(),
                            "workspace_id": target_workspace_id.to_string(),
                            "surface": pid.to_string(),
                        }),
                    );
                }
            }
        }

        // Detect whether the source panel is running a Codex agent, so we can
        // set retain_on_interrupt on the notification (Task 1).
        let is_codex_panel = resolved_panel_id
            .and_then(|pid| workspace.panels.get(&pid))
            .map(|panel| {
                crate::session::snapshot::detect_agent_resume_command(
                    panel.title.as_deref(),
                    panel.command.as_deref(),
                )
                .as_deref()
                == Some("codex")
            })
            .unwrap_or(false);

        workspace.record_notification(title, body, resolved_panel_id);
        (target_workspace_id, resolved_panel_id, is_codex_panel)
    };

    let (target_workspace_id, resolved_panel_id, is_codex_panel) = target;
    lock_or_recover(&state.notifications).add_with_retain(
        title,
        body,
        Some(target_workspace_id),
        resolved_panel_id,
        send_desktop,
        is_codex_panel,
    );

    // Auto-reorder: move notified workspace toward the top (after pinned items)
    let notif_settings = crate::settings::load().notifications;
    if notif_settings.reorder_on_notification {
        let mut tm = lock_or_recover(&state.tab_manager);
        if let Some(ws_idx) = tm.workspace_index(target_workspace_id) {
            // Find the first non-pinned index (skip pinned workspaces at top)
            let first_unpinned = tm.iter().position(|ws| !ws.is_pinned).unwrap_or(0);
            if ws_idx > first_unpinned {
                tm.move_workspace(ws_idx, first_unpinned);
            }
        }
    }

    // Play notification sound if enabled
    if notif_settings.sound_enabled {
        play_notification_sound(&notif_settings);
    }

    state.notify_ui_refresh();

    Response::success(
        id,
        serde_json::json!({
            "notified": true,
            "workspace": target_workspace_id.to_string(),
            "workspace_id": target_workspace_id.to_string(),
            "surface": resolved_panel_id.map(|panel_id| panel_id.to_string()),
        }),
    )
}

fn play_notification_sound(settings: &crate::settings::NotificationSettings) {
    // Use custom command if set, otherwise fall back to paplay with a freedesktop sound
    if let Some(ref cmd) = settings.custom_command {
        // Split into program + args to avoid shell injection (no sh -c).
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return;
        }
        let program = parts[0].to_string();
        let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
        std::thread::spawn(move || {
            let _ = std::process::Command::new(&program)
                .args(&args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    } else {
        std::thread::spawn(|| {
            // Try paplay (PulseAudio) with a freedesktop notification sound
            let _ = std::process::Command::new("paplay")
                .arg("/usr/share/sounds/freedesktop/stereo/message-new-instant.oga")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
    }
}

pub(super) fn handle_notification_list(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let unread_only = params
        .get("unread")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let store = lock_or_recover(&state.notifications);
    let notifications: Vec<Value> = store
        .all()
        .iter()
        .filter(|n| !unread_only || !n.is_read)
        .map(|n| {
            serde_json::json!({
                "id": n.id.to_string(),
                "title": n.title,
                "body": n.body,
                "workspace_id": n.source_workspace_id.map(|id| id.to_string()),
                "panel_id": n.source_panel_id.map(|id| id.to_string()),
                "timestamp": n.timestamp,
                "is_read": n.is_read,
            })
        })
        .collect();
    Response::success(
        id,
        serde_json::json!({
            "notifications": notifications,
            "count": notifications.len(),
        }),
    )
}

pub(super) fn handle_notification_clear(id: Value, state: &Arc<SharedState>) -> Response {
    lock_or_recover(&state.notifications).clear();
    state.notify_ui_refresh();
    Response::success(id, serde_json::json!({"ok": true}))
}

/// Parse a notification UUID from params["id"].
fn parse_notification_id(params: &Value) -> Result<uuid::Uuid, Response> {
    let s = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Response::error(Value::Null, "invalid_params", "Missing notification id"))?;
    uuid::Uuid::parse_str(s)
        .map_err(|_| Response::error(Value::Null, "invalid_params", "Invalid notification UUID"))
}

pub(super) fn handle_notification_mark_read(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let notif_id = match parse_notification_id(params) {
        Ok(v) => v,
        Err(mut e) => {
            e.id = id;
            return e;
        }
    };
    lock_or_recover(&state.notifications).mark_read(notif_id);
    state.notify_ui_refresh();
    Response::success(id, serde_json::json!({"ok": true}))
}

pub(super) fn handle_notification_dismiss(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let notif_id = match parse_notification_id(params) {
        Ok(v) => v,
        Err(mut e) => {
            e.id = id;
            return e;
        }
    };
    // `force: true` overrides retain_on_interrupt (explicit user dismiss).
    // Codex CLI auto-dismiss on interrupt does NOT set force, so retained
    // notifications survive an interrupted turn.
    let force = params
        .get("force")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let removed = lock_or_recover(&state.notifications).dismiss(notif_id, force);
    if !removed {
        return Response::error(id, "not_found", "Notification not found or protected");
    }
    state.notify_ui_refresh();
    Response::success(id, serde_json::json!({"ok": true}))
}

pub(super) fn handle_notification_open(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let notif_id = match parse_notification_id(params) {
        Ok(v) => v,
        Err(mut e) => {
            e.id = id;
            return e;
        }
    };

    // Look up the notification and find its source workspace.
    let workspace_id = {
        let store = lock_or_recover(&state.notifications);
        store
            .all()
            .iter()
            .find(|n| n.id == notif_id)
            .and_then(|n| n.source_workspace_id)
    };

    let Some(workspace_id) = workspace_id else {
        return Response::error(id, "not_found", "Notification not found or has no source workspace");
    };

    // Select the workspace (focus it).
    {
        let mut tm = lock_or_recover(&state.tab_manager);
        let Some(idx) = tm.workspace_index(workspace_id) else {
            return Response::error(id, "not_found", "Source workspace no longer exists");
        };
        tm.select(idx);
    }

    // Mark the notification as read.
    lock_or_recover(&state.notifications).mark_read(notif_id);

    state.notify_ui_refresh();
    Response::success(
        id,
        serde_json::json!({
            "ok": true,
            "workspace_id": workspace_id.to_string(),
        }),
    )
}
