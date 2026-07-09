//! Markdown panel V2 handler.

use std::sync::Arc;

use serde_json::Value;

use crate::app::{lock_or_recover, SharedState};
use crate::model::panel::SplitOrientation;

use super::helpers::optional_uuid;
use super::Response;

pub(super) fn handle_markdown_open(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let Some(file_path) = params.get("file").and_then(|v| v.as_str()) else {
        return Response::error(id, "invalid_params", "Provide 'file'");
    };
    let workspace_id = match optional_uuid(&id, params, "workspace_id") {
        Ok(v) => v,
        Err(e) => return e,
    };

    // Optional split direction: "right"/"left"→Horizontal, "down"/"up"→Vertical
    let direction = params.get("direction").and_then(|v| v.as_str());
    let split_orientation = direction.and_then(|d| match d {
        "right" | "left" | "horizontal" => Some(SplitOrientation::Horizontal),
        "down" | "up" | "vertical" => Some(SplitOrientation::Vertical),
        _ => None,
    });

    let panel = crate::model::panel::Panel::new_markdown(file_path);
    let panel_id = panel.id;

    let mut tm = lock_or_recover(&state.tab_manager);
    let ws_id = workspace_id.unwrap_or_else(|| tm.selected().map(|ws| ws.id).unwrap_or_default());

    if let Some(ws) = tm.workspace_mut(ws_id) {
        ws.panels.insert(panel_id, panel);
        if let Some(orientation) = split_orientation {
            // Split the focused pane and put the markdown panel in the new split
            if let Some(focused) = ws.focused_panel_id {
                if let Some(pane) = ws.layout.find_pane_with_panel(focused) {
                    let old = std::mem::replace(
                        pane,
                        crate::model::panel::LayoutNode::Pane {
                            panel_ids: vec![],
                            selected_panel_id: None,
                        },
                    );
                    *pane = old.split(orientation, panel_id);
                } else {
                    ws.layout.add_panel_to_pane(focused, panel_id);
                }
            } else {
                let first_panel = ws.layout.all_panel_ids().into_iter().next();
                if let Some(target) = first_panel {
                    ws.layout.add_panel_to_pane(target, panel_id);
                }
            }
        } else if let Some(focused) = ws.focused_panel_id {
            ws.layout.add_panel_to_pane(focused, panel_id);
        } else {
            let first_panel = ws.layout.all_panel_ids().into_iter().next();
            if let Some(target) = first_panel {
                ws.layout.add_panel_to_pane(target, panel_id);
            }
        }
        ws.previous_focused_panel_id = ws.focused_panel_id;
        ws.focused_panel_id = Some(panel_id);
    } else {
        return Response::error(id, "not_found", "Workspace not found");
    }
    drop(tm);
    state.notify_ui_refresh();

    Response::success(
        id,
        serde_json::json!({
            "panel_id": panel_id.to_string(),
            "file": file_path,
        }),
    )
}
