//! `file.open` V2 handler — open one or more files in the appropriate viewer.
//!
//! Markdown files open in the markdown panel; everything else opens in the
//! read-only file-preview panel. Mirrors upstream `cmux open <file>` routing
//! (the CLI batches files into a single `file.open` call).

use std::sync::Arc;

use serde_json::Value;

use crate::app::{lock_or_recover, SharedState};
use crate::model::panel::{LayoutNode, Panel, SplitOrientation};

use super::helpers::optional_uuid;
use super::Response;

/// True if `path` looks like a markdown file.
fn is_markdown(path: &str) -> bool {
    let lower = path.to_lowercase();
    [".md", ".markdown", ".mdown", ".mkd", ".mdwn"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

pub(super) fn handle_file_open(id: Value, params: &Value, state: &Arc<SharedState>) -> Response {
    // Accept either `paths: [..]` or a single `path`/`file`.
    let paths: Vec<String> = if let Some(arr) = params.get("paths").and_then(|v| v.as_array()) {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    } else if let Some(p) = params
        .get("path")
        .or_else(|| params.get("file"))
        .and_then(|v| v.as_str())
    {
        vec![p.to_string()]
    } else {
        return Response::error(id, "invalid_params", "Provide 'paths' or 'path'");
    };

    if paths.is_empty() {
        return Response::error(id, "invalid_params", "No file paths provided");
    }

    let workspace_id = match optional_uuid(&id, params, "workspace_id") {
        Ok(v) => v,
        Err(e) => return e,
    };

    let mut opened: Vec<String> = Vec::new();
    {
        let mut tm = lock_or_recover(&state.tab_manager);
        let ws_id =
            workspace_id.unwrap_or_else(|| tm.selected().map(|ws| ws.id).unwrap_or_default());
        let Some(ws) = tm.workspace_mut(ws_id) else {
            return Response::error(id, "not_found", "Workspace not found");
        };
        for path in &paths {
            let panel = if is_markdown(path) {
                Panel::new_markdown(path)
            } else {
                Panel::new_file_preview(path)
            };
            let panel_id = panel.id;
            ws.panels.insert(panel_id, panel);
            // Add as a tab in the focused pane (fall back to the first pane).
            let target = ws
                .focused_panel_id
                .or_else(|| ws.layout.all_panel_ids().into_iter().next());
            if let Some(target) = target {
                ws.layout.add_panel_to_pane(target, panel_id);
            }
            ws.previous_focused_panel_id = ws.focused_panel_id;
            ws.focused_panel_id = Some(panel_id);
            opened.push(panel_id.to_string());
        }
    }
    state.notify_ui_refresh();

    Response::success(
        id,
        serde_json::json!({ "panel_ids": opened, "count": opened.len() }),
    )
}

/// `notes.open` — open an editable notes scratchpad as a tab in the current
/// (or specified) workspace, mirroring how `file.open` / `markdown.open` work.
pub(super) fn handle_notes_open(
    id: Value,
    params: &Value,
    state: &Arc<SharedState>,
) -> Response {
    let file = params
        .get("file")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(crate::ui::notes_panel::default_notes_path);
    let workspace_id = match optional_uuid(&id, params, "workspace_id") {
        Ok(v) => v,
        Err(e) => return e,
    };

    // Layout: split by default (notes alongside the terminal). `direction`
    // controls orientation; "tab" opens it as a tab instead.
    let split_orientation = match params.get("direction").and_then(|v| v.as_str()) {
        Some("tab") => None,
        Some("down") | Some("up") | Some("vertical") => Some(SplitOrientation::Vertical),
        // right/left/horizontal, or unspecified → side-by-side split.
        _ => Some(SplitOrientation::Horizontal),
    };

    let panel = Panel::new_notes(&file);
    let panel_id = panel.id;
    {
        let mut tm = lock_or_recover(&state.tab_manager);
        let ws_id =
            workspace_id.unwrap_or_else(|| tm.selected().map(|ws| ws.id).unwrap_or_default());
        let Some(ws) = tm.workspace_mut(ws_id) else {
            return Response::error(id, "not_found", "Workspace not found");
        };
        ws.panels.insert(panel_id, panel);
        let focused = ws.focused_panel_id;
        match (split_orientation, focused) {
            (Some(orientation), Some(focused)) => {
                if let Some(pane) = ws.layout.find_pane_with_panel(focused) {
                    let old = std::mem::replace(
                        pane,
                        LayoutNode::Pane {
                            panel_ids: vec![],
                            selected_panel_id: None,
                        },
                    );
                    *pane = old.split(orientation, panel_id);
                } else {
                    ws.layout.add_panel_to_pane(focused, panel_id);
                }
            }
            _ => {
                // Tab mode, or no focused pane: add as a tab.
                let target = focused.or_else(|| ws.layout.all_panel_ids().into_iter().next());
                if let Some(target) = target {
                    ws.layout.add_panel_to_pane(target, panel_id);
                }
            }
        }
        ws.previous_focused_panel_id = ws.focused_panel_id;
        ws.focused_panel_id = Some(panel_id);
    }
    state.notify_ui_refresh();

    Response::success(
        id,
        serde_json::json!({ "panel_id": panel_id.to_string(), "file": file }),
    )
}
