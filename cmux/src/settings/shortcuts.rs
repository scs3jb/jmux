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
    /// Optional VS Code-style "when" context clause per action. The shortcut
    /// only fires when the clause evaluates true. Supports context keys joined
    /// by `&&` / `||` with optional `!` negation (e.g. `terminalFocused`,
    /// `!terminalFocused`, `browserFocused || editorFocused`).
    /// Context keys: terminalFocused, browserFocused, editorFocused,
    /// panelFocused (any non-terminal), paneZoomed.
    #[serde(default)]
    pub when: HashMap<String, String>,
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        let mut bindings = HashMap::new();

        // New tab (terminal) in the current pane.
        bindings.insert("tab.new".into(), Some(Keybinding::ctrl_shift("T")));
        // Focus the TextBox composer (unbound by default; set in Settings).
        bindings.insert("textbox.focus".into(), None);
        // Reopen the most recently closed tab (unbound by default — ghostty
        // owns Ctrl+Shift+T; set a key in Settings if desired).
        bindings.insert("tab.reopen".into(), None);
        // Toggle the Dock panel. Unbound by default: with a terminal focused,
        // ghostty's Kitty keyboard protocol encodes Ctrl+Shift+<key> and sends
        // it to the shell before cmux sees it, so use the header button or the
        // palette. Users can still bind a key here for non-terminal focus.
        bindings.insert("dock.toggle".into(), None);
        // Open the pane overview grid (unbound by default; header button +
        // palette are the reliable triggers over a focused terminal).
        bindings.insert("overview.open".into(), None);
        // Open the scope-grouped Notes panel beside the current pane. Default
        // Ctrl+Shift+N; like dock/overview this is eaten by ghostty's Kitty
        // keyboard protocol over a focused terminal, so the header Notes button
        // is the reliable trigger there.
        bindings.insert("notes.open".into(), Some(Keybinding::ctrl_shift("N")));

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

        Self {
            bindings,
            when: HashMap::new(),
        }
    }
}

impl ShortcutConfig {
    /// Get the keybinding for an action.
    /// Returns `None` if the action is unknown or explicitly unbound (`null`).
    pub fn get(&self, action: &str) -> Option<&Keybinding> {
        self.bindings.get(action).and_then(|opt| opt.as_ref())
    }

    /// Whether `action` is allowed to fire given the current context keys.
    /// Actions without a `when` clause are always allowed.
    pub fn when_allows(&self, action: &str, ctx: &ShortcutContext) -> bool {
        match self.when.get(action) {
            Some(expr) => eval_when(expr, ctx),
            None => true,
        }
    }
}

/// The set of context keys that are currently active, for `when` evaluation.
#[derive(Debug, Default, Clone, Copy)]
pub struct ShortcutContext {
    pub terminal_focused: bool,
    pub browser_focused: bool,
    pub editor_focused: bool,
    pub pane_zoomed: bool,
}

impl ShortcutContext {
    fn has(&self, key: &str) -> bool {
        match key {
            "terminalFocused" => self.terminal_focused,
            "browserFocused" => self.browser_focused,
            "editorFocused" => self.editor_focused,
            // Any non-terminal panel (browser, diff, markdown, notes, …).
            "panelFocused" => !self.terminal_focused,
            "paneZoomed" => self.pane_zoomed,
            _ => false,
        }
    }
}

/// Evaluate a minimal `when` expression: `||` (lowest precedence), then `&&`,
/// then an optional leading `!` on a context key. Unknown keys are false.
fn eval_when(expr: &str, ctx: &ShortcutContext) -> bool {
    expr.split("||").any(|or_term| {
        let or_term = or_term.trim();
        if or_term.is_empty() {
            return false;
        }
        or_term.split("&&").all(|and_term| {
            let and_term = and_term.trim();
            if let Some(rest) = and_term.strip_prefix('!') {
                !ctx.has(rest.trim())
            } else {
                ctx.has(and_term)
            }
        })
    })
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

#[cfg(test)]
mod when_tests {
    use super::*;

    fn ctx(term: bool, browser: bool) -> ShortcutContext {
        ShortcutContext {
            terminal_focused: term,
            browser_focused: browser,
            editor_focused: false,
            pane_zoomed: false,
        }
    }

    #[test]
    fn evaluates_when_clauses() {
        assert!(eval_when("terminalFocused", &ctx(true, false)));
        assert!(!eval_when("terminalFocused", &ctx(false, false)));
        assert!(eval_when("!terminalFocused", &ctx(false, false)));
        assert!(eval_when("panelFocused", &ctx(false, true)));
        assert!(!eval_when("panelFocused", &ctx(true, false)));
        // || / &&
        assert!(eval_when("terminalFocused || browserFocused", &ctx(false, true)));
        assert!(!eval_when("terminalFocused && browserFocused", &ctx(true, false)));
        assert!(eval_when("browserFocused && !terminalFocused", &ctx(false, true)));
        // unknown key is false
        assert!(!eval_when("nopeKey", &ctx(true, true)));
    }
}
