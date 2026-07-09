//! `jmux config` subcommand implementations (all local — no socket required).

use std::path::PathBuf;

const DOCS_URL: &str =
    "https://github.com/douglas/jmux/blob/main/README.md";

/// Return the active jmux config file path.
///
/// Prefers `~/.config/jmux/jmux.json`; falls back to `~/.config/jmux/settings.json`.
fn active_config_path() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("jmux");
    let jmux_json = dir.join("jmux.json");
    if jmux_json.exists() {
        jmux_json
    } else {
        dir.join("settings.json")
    }
}

/// Strip JSONC (JSON with comments) comments from `s`.
///
/// Handles `// line` and `/* block */` comments outside strings.
fn strip_jsonc_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_string = false;

    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            } else if c == '"' {
                in_string = false;
            }
        } else {
            match c {
                '"' => {
                    in_string = true;
                    out.push(c);
                }
                '/' => match chars.peek() {
                    Some('/') => {
                        chars.next();
                        for ch in chars.by_ref() {
                            if ch == '\n' {
                                out.push('\n');
                                break;
                            }
                        }
                    }
                    Some('*') => {
                        chars.next();
                        let mut prev = '\0';
                        for ch in chars.by_ref() {
                            if prev == '*' && ch == '/' {
                                break;
                            }
                            if ch == '\n' {
                                out.push('\n');
                            }
                            prev = ch;
                        }
                    }
                    _ => {
                        out.push(c);
                    }
                },
                _ => {
                    out.push(c);
                }
            }
        }
    }

    out
}

/// `jmux config path` — print the active config file path.
///
/// Prints `jmux.json` when it exists, otherwise `settings.json` (legacy).
pub fn run_path() -> anyhow::Result<()> {
    println!("{}", active_config_path().display());
    Ok(())
}

/// `jmux config docs` — print the documentation URL.
pub fn run_docs() -> anyhow::Result<()> {
    println!("Documentation: {DOCS_URL}");
    Ok(())
}

/// `jmux config doctor` — validate the config file.
pub fn run_doctor() -> anyhow::Result<()> {
    let path = active_config_path();

    // Check 1: file existence
    if !path.exists() {
        println!(
            "No config file found at {}. Using defaults.",
            path.display()
        );
        println!("✓ Config OK");
        return Ok(());
    }

    // Check 2: readable + valid JSON/JSONC
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            println!("Error reading {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    let clean = strip_jsonc_comments(&content);
    let value: serde_json::Value = match serde_json::from_str(&clean) {
        Ok(v) => v,
        Err(e) => {
            println!("Invalid JSON in {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    // Check 3: value range validation on known numeric fields
    let mut issues: Vec<String> = Vec::new();

    if let Some(size) = value
        .get("tab_bar_font_size")
        .and_then(|v| v.as_f64())
    {
        // 0.0 means "use system default" — only validate if explicitly set
        if size != 0.0 && !(6.0..=100.0).contains(&size) {
            issues.push(format!(
                "tab_bar_font_size = {size} is out of range (expected 0 for default, or 6–100)"
            ));
        }
    }

    if issues.is_empty() {
        println!("✓ Config OK ({})", path.display());
    } else {
        for issue in &issues {
            println!("  - {issue}");
        }
        std::process::exit(1);
    }

    Ok(())
}
