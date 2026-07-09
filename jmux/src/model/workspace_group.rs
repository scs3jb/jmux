//! Workspace group model — a named, collapsible sidebar section that holds
//! a set of workspaces.
//!
//! Workspaces reference their group via `Workspace::group_id`. A group is
//! anchored in the sidebar at the position of its first member workspace, and
//! its members are rendered contiguously beneath the group header.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A collapsible sidebar section grouping related workspaces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceGroup {
    pub id: Uuid,
    /// Display name shown in the group header.
    pub name: String,
    /// Optional accent color (hex string, e.g. "#7aa2f7").
    pub color: Option<String>,
    /// When true, member workspaces are hidden in the sidebar.
    pub collapsed: bool,
    /// Window this group belongs to (None = default/first window).
    pub window_id: Option<Uuid>,
}

impl WorkspaceGroup {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            color: None,
            collapsed: false,
            window_id: None,
        }
    }
}
