//! Custom commands from `jmux.json` — command-palette entries that run a shell
//! command or open a multi-pane workspace layout.
//!
//! Discovered from (highest priority first):
//!   1. `<workspace>/.jmux/jmux.json`
//!   2. `<workspace>/jmux.json`
//!   3. `~/.config/jmux/jmux.json` (global)
//!
//! Entries are merged by `name`; the higher-priority file wins. Schema mirrors
//! upstream jmux (`commands[]` with optional `workspace` layout).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use uuid::Uuid;

use crate::model::panel::{LayoutNode, Panel, SplitOrientation};
use crate::model::Workspace;

/// A single `commands[]` entry.
#[derive(Debug, Clone, Deserialize)]
pub struct CommandEntry {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Shell command to send to the focused terminal.
    #[serde(default)]
    pub command: Option<String>,
    /// Ask for confirmation before running.
    #[serde(default)]
    pub confirm: bool,
    /// Open a multi-pane workspace instead of running a command.
    #[serde(default)]
    pub workspace: Option<WorkspaceSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceSpec {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    /// Environment variables inherited by every shell spawned in this workspace.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    pub layout: LayoutSpec,
}

/// A node in the layout tree: either a split (two-or-more children) or a pane.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum LayoutSpec {
    Split {
        direction: String,
        #[serde(default = "default_split")]
        split: f64,
        children: Vec<LayoutSpec>,
    },
    Pane {
        pane: PaneSpec,
    },
}

fn default_split() -> f64 {
    0.5
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaneSpec {
    #[serde(default)]
    pub surfaces: Vec<SurfaceSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SurfaceSpec {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub focus: bool,
}

#[derive(Debug, Default, Deserialize)]
struct JmuxJson {
    #[serde(default)]
    commands: Vec<CommandEntry>,
}

/// Load and merge custom commands for `workspace_dir`.
pub fn load(workspace_dir: &str) -> Vec<CommandEntry> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if !workspace_dir.is_empty() {
        paths.push(Path::new(workspace_dir).join(".jmux/jmux.json"));
        paths.push(Path::new(workspace_dir).join("jmux.json"));
    }
    paths.push(super::config_dir().join("jmux.json"));

    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<CommandEntry> = Vec::new();
    for path in paths {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<JmuxJson>(&content) else {
            continue;
        };
        for entry in parsed.commands {
            if entry.name.is_empty() {
                continue;
            }
            // Need something runnable.
            if entry.command.is_none() && entry.workspace.is_none() {
                continue;
            }
            if seen.insert(entry.name.clone()) {
                out.push(entry);
            }
        }
    }
    out
}

/// Build a workspace from a `workspace` layout spec. `base_dir` is the directory
/// relative paths (`.`, `./sub`) resolve against.
pub fn build_workspace(spec: &WorkspaceSpec, base_dir: &str) -> Workspace {
    let root = resolve_dir(spec.cwd.as_deref(), base_dir);
    let mut ws = Workspace::with_directory(&root);
    if let Some(name) = &spec.name {
        ws.custom_title = Some(name.clone());
    }
    if let Some(color) = &spec.color {
        ws.custom_color = Some(color.clone());
    }
    ws.env = spec
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    let mut panels: HashMap<Uuid, Panel> = HashMap::new();
    let mut focus: Option<Uuid> = None;
    let layout = build_layout(&spec.layout, &root, &mut panels, &mut focus);

    if panels.is_empty() {
        // Degenerate layout — fall back to a single terminal.
        let panel = Panel::new_terminal();
        let id = panel.id;
        panels.insert(id, panel);
        ws.layout = LayoutNode::Pane {
            panel_ids: vec![id],
            selected_panel_id: Some(id),
        };
        ws.focused_panel_id = Some(id);
    } else {
        ws.layout = layout;
        ws.focused_panel_id = focus.or_else(|| ws.layout.all_panel_ids().into_iter().next());
    }
    ws.panels = panels;
    ws
}

fn build_layout(
    spec: &LayoutSpec,
    base: &str,
    panels: &mut HashMap<Uuid, Panel>,
    focus: &mut Option<Uuid>,
) -> LayoutNode {
    match spec {
        LayoutSpec::Split {
            direction,
            split,
            children,
        } => {
            let orientation = if direction.eq_ignore_ascii_case("vertical") {
                SplitOrientation::Vertical
            } else {
                SplitOrientation::Horizontal
            };
            let nodes: Vec<LayoutNode> = children
                .iter()
                .map(|c| build_layout(c, base, panels, focus))
                .collect();
            fold_split(nodes, orientation, *split)
        }
        LayoutSpec::Pane { pane } => build_pane(pane, base, panels, focus),
    }
}

/// Fold N child nodes into nested binary splits (the schema specifies exactly
/// two children, but tolerate any count).
fn fold_split(mut nodes: Vec<LayoutNode>, orientation: SplitOrientation, ratio: f64) -> LayoutNode {
    match nodes.len() {
        0 => LayoutNode::Pane {
            panel_ids: vec![],
            selected_panel_id: None,
        },
        1 => nodes.pop().unwrap(),
        _ => {
            let first = nodes.remove(0);
            let second = fold_split(nodes, orientation, ratio);
            LayoutNode::Split {
                orientation,
                divider_position: ratio.clamp(0.1, 0.9),
                first: Box::new(first),
                second: Box::new(second),
            }
        }
    }
}

fn build_pane(
    pane: &PaneSpec,
    base: &str,
    panels: &mut HashMap<Uuid, Panel>,
    focus: &mut Option<Uuid>,
) -> LayoutNode {
    let mut panel_ids = Vec::new();
    for surface in &pane.surfaces {
        let is_browser = surface.kind.as_deref() == Some("browser");
        let mut panel = if is_browser {
            Panel::new_browser()
        } else {
            Panel::new_terminal()
        };
        panel.directory = Some(resolve_dir(surface.cwd.as_deref(), base));
        if let Some(cmd) = &surface.command {
            if !cmd.trim().is_empty() {
                panel.command = Some(cmd.clone());
            }
        }
        if is_browser {
            panel.browser_url = surface.url.clone();
        }
        if let Some(name) = &surface.name {
            panel.custom_title = Some(name.clone());
        }
        let id = panel.id;
        if surface.focus {
            *focus = Some(id);
        }
        panels.insert(id, panel);
        panel_ids.push(id);
    }
    if panel_ids.is_empty() {
        let panel = Panel::new_terminal();
        let id = panel.id;
        panels.insert(id, panel);
        panel_ids.push(id);
    }
    let selected = panel_ids.first().copied();
    LayoutNode::Pane {
        panel_ids,
        selected_panel_id: selected,
    }
}

/// Resolve a `cwd` field: `.`/empty → base, `~/x` → home, `/x` → absolute,
/// `./x` or `x` → relative to base.
fn resolve_dir(cwd: Option<&str>, base: &str) -> String {
    let Some(raw) = cwd else {
        return base.to_string();
    };
    let raw = raw.trim();
    if raw.is_empty() || raw == "." {
        return base.to_string();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return dirs::home_dir()
            .map(|h| h.join(rest).to_string_lossy().into_owned())
            .unwrap_or_else(|| raw.to_string());
    }
    if raw.starts_with('/') {
        return raw.to_string();
    }
    let rel = raw.strip_prefix("./").unwrap_or(raw);
    Path::new(base).join(rel).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_dirs() {
        assert_eq!(resolve_dir(None, "/base"), "/base");
        assert_eq!(resolve_dir(Some("."), "/base"), "/base");
        assert_eq!(resolve_dir(Some(""), "/base"), "/base");
        assert_eq!(resolve_dir(Some("./sub"), "/base"), "/base/sub");
        assert_eq!(resolve_dir(Some("sub"), "/base"), "/base/sub");
        assert_eq!(resolve_dir(Some("/abs"), "/base"), "/abs");
    }

    #[test]
    fn parses_simple_command() {
        let json = r#"{ "commands": [
            { "name": "Dev", "keywords": ["d"], "command": "npm run dev", "confirm": true }
        ] }"#;
        let parsed: JmuxJson = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.commands.len(), 1);
        let e = &parsed.commands[0];
        assert_eq!(e.command.as_deref(), Some("npm run dev"));
        assert!(e.confirm);
        assert_eq!(e.keywords, vec!["d"]);
        assert!(e.workspace.is_none());
    }

    #[test]
    fn builds_split_workspace_layout() {
        let json = r##"{ "commands": [{
            "name": "Dev",
            "workspace": {
                "name": "Dev Env",
                "cwd": ".",
                "color": "#3b82f6",
                "layout": {
                    "direction": "horizontal",
                    "split": 0.6,
                    "children": [
                        { "pane": { "surfaces": [ { "command": "npm run dev", "cwd": "./app", "focus": true } ] } },
                        { "pane": { "surfaces": [ { "command": "npm test" } ] } }
                    ]
                }
            }
        }] }"##;
        let parsed: JmuxJson = serde_json::from_str(json).unwrap();
        let spec = parsed.commands[0].workspace.as_ref().unwrap();
        let ws = build_workspace(spec, "/base");

        assert_eq!(ws.panels.len(), 2);
        assert_eq!(ws.custom_title.as_deref(), Some("Dev Env"));
        assert_eq!(ws.custom_color.as_deref(), Some("#3b82f6"));
        assert!(matches!(ws.layout, LayoutNode::Split { .. }));

        let focused = ws.focused_panel_id.expect("a focused panel");
        let p = &ws.panels[&focused];
        assert_eq!(p.command.as_deref(), Some("npm run dev"));
        assert_eq!(p.directory.as_deref(), Some("/base/app"));
    }
}
