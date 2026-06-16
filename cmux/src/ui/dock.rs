//! Dock — a right-side column of small terminal "controls" (lazygit, log
//! tails, watchers, …), each a Ghostty-backed terminal running a command from
//! `dock.json`.
//!
//! Config (merged, project wins, deduped by id):
//!   - `<workspace>/.cmux/dock.json`
//!   - `~/.config/cmux/dock.json`
//!
//! Each control: `id`, `title`, `command`, optional `cwd`, `height`.
//! The dock is built once per window and toggled via visibility, so its
//! terminals persist (they only spawn once the dock becomes visible).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use gtk4::prelude::*;
use serde::Deserialize;
use uuid::Uuid;

use crate::app::AppState;

thread_local! {
    /// window_id → dock, so a shortcut/palette/button can toggle it.
    static DOCKS: RefCell<HashMap<String, DockEntry>> = RefCell::new(HashMap::new());
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct DockControl {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

#[derive(Debug, Default, Deserialize, serde::Serialize)]
struct DockJson {
    #[serde(default)]
    controls: Vec<DockControl>,
}

/// Path to the global dock config (`~/.config/cmux/dock.json`).
pub fn global_path() -> std::path::PathBuf {
    crate::settings::config_dir().join("dock.json")
}

/// Load controls from the global dock.json (for the editor).
pub fn load_global() -> Vec<DockControl> {
    std::fs::read_to_string(global_path())
        .ok()
        .map(|c| parse_controls(&c))
        .unwrap_or_default()
}

/// Write controls to the global dock.json.
pub fn save_global(controls: &[DockControl]) -> std::io::Result<()> {
    let dir = crate::settings::config_dir();
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_string_pretty(&DockJson {
        controls: controls.to_vec(),
    })
    .map_err(std::io::Error::other)?;
    std::fs::write(global_path(), json)
}

/// Load + merge dock controls for a workspace directory.
pub fn load(workspace_dir: &str) -> Vec<DockControl> {
    let mut paths = Vec::new();
    if !workspace_dir.is_empty() {
        paths.push(Path::new(workspace_dir).join(".cmux/dock.json"));
    }
    paths.push(crate::settings::config_dir().join("dock.json"));

    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for path in paths {
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for control in parse_controls(&content) {
            if control.id.is_empty() || control.command.trim().is_empty() {
                continue;
            }
            if seen.insert(control.id.clone()) {
                out.push(control);
            }
        }
    }
    out
}

/// Parse dock controls from a `dock.json` string. Accepts either
/// `{"controls": [...]}` or a bare `[...]` array.
fn parse_controls(content: &str) -> Vec<DockControl> {
    match serde_json::from_str::<DockJson>(content) {
        Ok(d) if !d.controls.is_empty() => d.controls,
        _ => serde_json::from_str::<Vec<DockControl>>(content).unwrap_or_default(),
    }
}

/// Build the dock Box for `window_id`, populated from `workspace_dir`. Returns
/// an empty (hidden) Box when there are no controls. Registered for toggling.
pub fn create_dock(window_id: Uuid, workspace_dir: &str, state: &Rc<AppState>) -> gtk4::Box {
    let dock = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    dock.add_css_class("dock-panel");
    dock.set_width_request(360);
    dock.set_visible(false);

    DOCKS.with(|m| {
        m.borrow_mut().insert(
            window_id.to_string(),
            DockEntry {
                dock_box: dock.clone(),
                built: false,
            },
        );
    });

    // Show on startup if enabled (builds from the current dock.json).
    if crate::settings::load().show_dock {
        set_visible(window_id, workspace_dir, state, true);
    }
    dock
}

/// A registered dock for one window.
struct DockEntry {
    dock_box: gtk4::Box,
    built: bool,
}

/// (Re)build the dock contents from `workspace_dir`'s dock.json. Shows a hint
/// when no controls are configured so toggling always gives visible feedback.
fn build_into(dock: &gtk4::Box, workspace_dir: &str, state: &Rc<AppState>) {
    while let Some(child) = dock.first_child() {
        dock.remove(&child);
    }

    let header = gtk4::Label::new(Some("Dock"));
    header.add_css_class("dim-label");
    header.add_css_class("caption-heading");
    header.set_xalign(0.0);
    header.set_margin_start(8);
    header.set_margin_top(6);
    header.set_margin_bottom(2);
    dock.append(&header);

    let controls = load(workspace_dir);
    if controls.is_empty() {
        let hint = gtk4::Label::new(Some(
            "No dock controls configured.\n\nAdd a dock.json with:\n\n{\n  \"controls\": [\n    { \"id\": \"git\",\n      \"title\": \"Git\",\n      \"command\": \"lazygit\" }\n  ]\n}\n\nin .cmux/dock.json (this project)\nor ~/.config/cmux/dock.json (global).",
        ));
        hint.add_css_class("dim-label");
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        hint.set_margin_start(10);
        hint.set_margin_end(10);
        hint.set_margin_top(8);
        dock.append(&hint);
    } else {
        for control in controls {
            dock.append(&build_control(&control, workspace_dir, state));
        }
    }
}

fn build_control(control: &DockControl, base_dir: &str, state: &Rc<AppState>) -> gtk4::Widget {
    let section = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    section.add_css_class("dock-control");

    let title = gtk4::Label::new(Some(
        control.title.as_deref().unwrap_or(control.id.as_str()),
    ));
    title.add_css_class("dock-control-title");
    title.add_css_class("caption");
    title.set_xalign(0.0);
    title.set_margin_start(8);
    title.set_margin_top(4);
    section.append(&title);

    let cwd = resolve_dir(control.cwd.as_deref(), base_dir);
    // The dock is built once per window, so a fresh id per control is fine.
    let surface_id = Uuid::new_v4();
    let surface = state.terminal_surface_for(surface_id, Some(&cwd), Some(&control.command));
    if let Some(parent) = surface.parent() {
        if let Some(parent_box) = parent.downcast_ref::<gtk4::Box>() {
            parent_box.remove(&surface);
        }
    }
    surface.set_hexpand(true);
    surface.set_vexpand(true);
    section.set_height_request(control.height.unwrap_or(220).clamp(80, 1000) as i32);
    section.append(&surface);
    section.upcast()
}

/// Show or hide the dock for `window_id`, building it from `workspace_dir` on
/// first show. Returns the resulting visibility.
pub fn set_visible(
    window_id: Uuid,
    workspace_dir: &str,
    state: &Rc<AppState>,
    visible: bool,
) -> bool {
    DOCKS.with(|m| {
        let mut map = m.borrow_mut();
        let Some(entry) = map.get_mut(&window_id.to_string()) else {
            return false;
        };
        if visible && !entry.built {
            build_into(&entry.dock_box, workspace_dir, state);
            entry.built = true;
        }
        entry.dock_box.set_visible(visible);
        visible
    })
}

/// Toggle the dock for `window_id`. Returns the new visibility.
pub fn toggle(window_id: Uuid, workspace_dir: &str, state: &Rc<AppState>) -> bool {
    let currently_visible = DOCKS.with(|m| {
        m.borrow()
            .get(&window_id.to_string())
            .map(|e| e.dock_box.is_visible())
            .unwrap_or(false)
    });
    set_visible(window_id, workspace_dir, state, !currently_visible)
}

/// Resolve a `cwd` field (`.`/empty → base, `~/x`, `/abs`, or relative).
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
    Path::new(base)
        .join(raw.strip_prefix("./").unwrap_or(raw))
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_controls_object_form() {
        let c = parse_controls(
            r#"{ "controls": [ { "id": "git", "title": "Git", "command": "lazygit", "height": 300 } ] }"#,
        );
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].id, "git");
        assert_eq!(c[0].command, "lazygit");
        assert_eq!(c[0].height, Some(300));
    }

    #[test]
    fn parses_controls_bare_array() {
        let c = parse_controls(r#"[ { "id": "logs", "command": "tail -f app.log" } ]"#);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].id, "logs");
        assert!(c[0].title.is_none());
    }

    #[test]
    fn resolves_dirs() {
        assert_eq!(resolve_dir(None, "/b"), "/b");
        assert_eq!(resolve_dir(Some("./x"), "/b"), "/b/x");
        assert_eq!(resolve_dir(Some("/abs"), "/b"), "/abs");
    }
}
