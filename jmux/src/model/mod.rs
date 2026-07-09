//! Data models — Panel, Workspace, TabManager, and layout tree types.

pub mod claude_state;
pub mod panel;
pub mod tab_manager;
pub mod workspace;
pub mod workspace_group;

pub use panel::{Panel, PanelType};
pub use tab_manager::TabManager;
pub use workspace::Workspace;
pub use workspace_group::WorkspaceGroup;
