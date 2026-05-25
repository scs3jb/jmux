//! Sidebar V2 handlers — show, hide, toggle, status.

use std::sync::Arc;

use serde_json::Value;

use crate::app::{SharedState, UiEvent};

use super::Response;

pub(super) fn handle_sidebar_show(id: Value, state: &Arc<SharedState>) -> Response {
    state.send_ui_event(UiEvent::ShowSidebar(true));
    Response::success(id, serde_json::json!({"visible": true}))
}

pub(super) fn handle_sidebar_hide(id: Value, state: &Arc<SharedState>) -> Response {
    state.send_ui_event(UiEvent::ShowSidebar(false));
    Response::success(id, serde_json::json!({"visible": false}))
}

pub(super) fn handle_sidebar_toggle(id: Value, state: &Arc<SharedState>) -> Response {
    // We don't have direct access to the GTK widget here, so we send a
    // ShowSidebar event carrying None-semantics via a dedicated toggle event.
    // For parity with upstream `cmux sidebar toggle` we dispatch ToggleSidebar.
    state.send_ui_event(UiEvent::ToggleSidebar);
    Response::success(id, serde_json::json!({"toggled": true}))
}

pub(super) fn handle_sidebar_status(id: Value, state: &Arc<SharedState>) -> Response {
    // The authoritative "is sidebar shown" state lives on the GTK main thread
    // inside the NavigationSplitView widget; we cannot read it from the socket
    // thread.  Return the known workspace count as a proxy for sidebar content.
    let workspace_count = {
        let tm = crate::app::lock_or_recover(&state.tab_manager);
        tm.len()
    };
    Response::success(
        id,
        serde_json::json!({
            "workspace_count": workspace_count,
        }),
    )
}
