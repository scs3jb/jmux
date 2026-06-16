//! Keyboard shortcut configuration — persistent keybindings.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A keyboard shortcut binding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Keybinding {
    /// GTK key name (e.g., "t", "d", "f", "1", "space")
    pub key: String,
    /// Whether Ctrl is required.
    pub ctrl: bool,
    /// Whether Shift is required.
    pub shift: bool,
    /// Whether Alt is required.
    pub alt: bool,
}

impl Keybinding {
    pub fn ctrl_shift(key: &str) -> Self {
        Self {
            key: key.to_string(),
            ctrl: true,
            shift: true,
            alt: false,
        }
    }

    pub fn ctrl(key: &str) -> Self {
        Self {
            key: key.to_string(),
            ctrl: true,
            shift: false,
            alt: false,
        }
    }

    /// Format as a human-readable string for display.
    pub fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        let key_display = if self.key == "space" { "Space" } else { &self.key };
        parts.push(key_display);
        parts.join("+")
    }
}

/// All configurable shortcut actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShortcutConfig {
    /// Action name → optional keybinding map.
    /// A `null` or missing value means the shortcut is disabled/unbound.
    pub bindings: HashMap<String, Option<Keybinding>>,
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        let mut bindings = HashMap::new();

        // New tab (terminal) in the current pane.
        bindings.insert("tab.new".into(), Some(Keybinding::ctrl_shift("T")));
        // Focus the TextBox composer (unbound by default; set in Settings).
        bindings.insert("textbox.focus".into(), None);
        // Toggle the Dock panel. Ctrl+Shift+L is free in both cmux and ghostty
        // (ghostty owns J/T/etc., so those never reach us when a terminal has
        // focus).
        bindings.insert("dock.toggle".into(), Some(Keybinding::ctrl_shift("L")));

        // Workspace management. `workspace.new` has no default key (Ctrl+Shift+T
        // opens a new tab); it's reachable via the palette / sidebar / `cmux new`.
        bindings.insert("workspace.new".into(), None);
        bindings.insert("workspace.close".into(), Some(Keybinding::ctrl_shift("W")));
        bindings.insert(
            "workspace.latest_unread".into(),
            Some(Keybinding::ctrl_shift("U")),
        );
        bindings.insert("workspace.rename".into(), Some(Keybinding::ctrl_shift("R")));
        bindings.insert(
            "workspace.move_up".into(),
            Some(Keybinding::ctrl_shift("Page_Up")),
        );
        bindings.insert(
            "workspace.move_down".into(),
            Some(Keybinding::ctrl_shift("Page_Down")),
        );

        // Tab close — Ctrl+W (lowercase) closes the focused panel (browser convention).
        // Ctrl+Shift+W is already workspace.close; lowercase w is available.
        bindings.insert("close.tab".into(), Some(Keybinding::ctrl("w")));
        // Close other panels in the same pane — unbound by default.
        bindings.insert("close.tab.others".into(), None);

        // Pane management
        bindings.insert("pane.close".into(), Some(Keybinding::ctrl_shift("Q")));
        bindings.insert(
            "pane.split_horizontal".into(),
            Some(Keybinding::ctrl_shift("D")),
        );
        bindings.insert(
            "pane.split_vertical".into(),
            Some(Keybinding::ctrl_shift("E")),
        );
        bindings.insert(
            "pane.focus_prev".into(),
            Some(Keybinding::ctrl_shift("bracketleft")),
        );
        bindings.insert(
            "pane.focus_next".into(),
            Some(Keybinding::ctrl_shift("bracketright")),
        );

        // Pane rename (F2)
        bindings.insert(
            "pane.rename".into(),
            Some(Keybinding {
                key: "F2".to_string(),
                ctrl: false,
                shift: false,
                alt: false,
            }),
        );

        // Pane directional focus
        bindings.insert(
            "pane.focus_left".into(),
            Some(Keybinding {
                key: "Left".to_string(),
                ctrl: false,
                shift: false,
                alt: true,
            }),
        );
        bindings.insert(
            "pane.focus_right".into(),
            Some(Keybinding {
                key: "Right".to_string(),
                ctrl: false,
                shift: false,
                alt: true,
            }),
        );
        bindings.insert(
            "pane.focus_up".into(),
            Some(Keybinding {
                key: "Up".to_string(),
                ctrl: false,
                shift: false,
                alt: true,
            }),
        );
        bindings.insert(
            "pane.focus_down".into(),
            Some(Keybinding {
                key: "Down".to_string(),
                ctrl: false,
                shift: false,
                alt: true,
            }),
        );

        // UI toggles
        bindings.insert("find".into(), Some(Keybinding::ctrl("f")));
        bindings.insert("find.next".into(), Some(Keybinding::ctrl("g")));
        bindings.insert("find.previous".into(), Some(Keybinding::ctrl_shift("G")));
        bindings.insert("find.use_selection".into(), Some(Keybinding::ctrl("e")));
        bindings.insert(
            "notifications.toggle".into(),
            Some(Keybinding::ctrl_shift("I")),
        );
        bindings.insert("settings".into(), Some(Keybinding::ctrl("comma")));

        // Terminal font size
        bindings.insert("font.increase".into(), Some(Keybinding::ctrl("equal")));
        bindings.insert("font.decrease".into(), Some(Keybinding::ctrl("minus")));
        bindings.insert("font.reset".into(), Some(Keybinding::ctrl("0")));

        // Clear scrollback
        bindings.insert("surface.clear".into(), Some(Keybinding::ctrl("k")));

        // Context-aware reload: browser reload or terminal config reload
        bindings.insert(
            "reload".into(),
            Some(Keybinding {
                key: "r".to_string(),
                ctrl: true,
                shift: false,
                alt: false,
            }),
        );

        // Browser-specific splits
        bindings.insert(
            "browser.split_horizontal".into(),
            Some(Keybinding {
                key: "d".to_string(),
                ctrl: true,
                shift: false,
                alt: true,
            }),
        );
        bindings.insert(
            "browser.split_vertical".into(),
            Some(Keybinding {
                key: "e".to_string(),
                ctrl: true,
                shift: false,
                alt: true,
            }),
        );

        // Close other pane tabs
        bindings.insert(
            "tab.close_others".into(),
            Some(Keybinding {
                key: "W".to_string(),
                ctrl: true,
                shift: true,
                alt: true,
            }),
        );

        // Browser console toggle
        bindings.insert(
            "browser.console_toggle".into(),
            Some(Keybinding {
                key: "c".to_string(),
                ctrl: true,
                shift: false,
                alt: true,
            }),
        );

        // Reload ghostty configuration
        bindings.insert("config.reload".into(), Some(Keybinding::ctrl_shift("comma")));

        // Find in directory — focuses the file-explorer search entry.
        // Unbound by default; users configure this in shortcuts.json.
        bindings.insert("find.in_directory".into(), None);

        // Notification shortcuts — no default key; users bind these manually.
        bindings.insert("notification.defer_unread".into(), None);
        bindings.insert("notification.toggle_unread".into(), None);
        // Agent resume — unbound by default; users configure this in shortcuts.json
        bindings.insert("agent.resume".into(), None);

        // Workspace color presets: Ctrl+Alt+0 resets to default, Ctrl+Alt+1-9 set preset colors
        for i in 0u8..=9 {
            bindings.insert(
                format!("workspace.color.{i}"),
                Some(Keybinding {
                    key: i.to_string(),
                    ctrl: true,
                    shift: false,
                    alt: true,
                }),
            );
        }

        Self { bindings }
    }
}

impl ShortcutConfig {
    /// Get the keybinding for an action.
    /// Returns `None` if the action is unknown or explicitly unbound (`null`).
    pub fn get(&self, action: &str) -> Option<&Keybinding> {
        self.bindings.get(action).and_then(|opt| opt.as_ref())
    }
}

/// Load shortcut config from disk.
pub fn load() -> ShortcutConfig {
    let path = super::config_dir().join("shortcuts.json");
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let mut config: ShortcutConfig = serde_json::from_str(&content).unwrap_or_else(|err| {
                tracing::warn!("Failed to parse {}: {err}", path.display());
                ShortcutConfig::default()
            });
            // Merge in defaults for any actions the saved file predates (e.g.
            // newly added shortcuts), without overriding user customizations.
            for (action, binding) in ShortcutConfig::default().bindings {
                config.bindings.entry(action).or_insert(binding);
            }
            config
        }
        Err(_) => ShortcutConfig::default(),
    }
}

/// Save shortcut config to disk.
pub fn save(config: &ShortcutConfig) -> Result<(), std::io::Error> {
    let dir = super::config_dir();
    std::fs::create_dir_all(&dir)?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }

    let path = dir.join("shortcuts.json");
    let json = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&path)?;
        f.write_all(json.as_bytes())?;
    }
    Ok(())
}
