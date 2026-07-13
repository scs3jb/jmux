//! `jmux agent view` — read-only, Claude-CLI-style renderer for a subagent
//! transcript (`agent-*.jsonl`).
//!
//! Renders the full conversation the way Claude Code's own task view does
//! (press ↓ then Enter on a running task): the task prompt, thinking,
//! assistant text with `⏺` bullets, `Tool(args)` invocations, and dimmed
//! `⎿` tool results — then follows the file for new entries. There is no
//! prompt and input is discarded; jmux runs this inside the read-only
//! sub-agent monitor panes.

use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;

/// Poll interval while following the transcript.
const POLL_MS: u64 = 300;

/// Tool-result preview length, matching the CLI's collapsed view.
const RESULT_PREVIEW_LINES: usize = 5;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const ITALIC: &str = "\x1b[3m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";

static WINCH: AtomicBool = AtomicBool::new(false);
static QUIT: AtomicBool = AtomicBool::new(false);

extern "C" fn on_winch(_: libc::c_int) {
    WINCH.store(true, Ordering::SeqCst);
}
extern "C" fn on_quit(_: libc::c_int) {
    QUIT.store(true, Ordering::SeqCst);
}

pub fn run(transcript: &str) -> Result<()> {
    let _tty = TtyGuard::new();
    unsafe {
        libc::signal(libc::SIGWINCH, on_winch as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_quit as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_quit as *const () as libc::sighandler_t);
        libc::signal(libc::SIGHUP, on_quit as *const () as libc::sighandler_t);
    }

    let mut renderer = Renderer::new(transcript);
    renderer.render_header();

    // Byte offset of consumed complete lines + carry for a partial tail line.
    let mut offset: u64 = 0;
    let mut carry = String::new();

    loop {
        if QUIT.load(Ordering::SeqCst) {
            return Ok(());
        }
        // Resize: re-render the whole conversation wrapped to the new width.
        if WINCH.swap(false, Ordering::SeqCst) {
            offset = 0;
            carry.clear();
            renderer = Renderer::new(transcript);
            print!("\x1b[2J\x1b[H");
            renderer.render_header();
        }

        match read_appended(transcript, offset) {
            Some((bytes, new_offset)) => {
                offset = new_offset;
                carry.push_str(&String::from_utf8_lossy(&bytes));
                while let Some(nl) = carry.find('\n') {
                    let line: String = carry.drain(..=nl).collect();
                    renderer.render_line(line.trim_end());
                }
                let _ = std::io::stdout().flush();
            }
            None => {
                // File missing (not written yet, or cleaned up) — keep waiting;
                // the monitor pane closes us when the agent ages out.
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
    }
}

/// Read bytes appended past `offset`. Returns the new offset (start of any
/// bytes not yet read). Handles truncation by restarting from 0.
fn read_appended(path: &str, offset: u64) -> Option<(Vec<u8>, u64)> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = if len < offset { 0 } else { offset };
    if len == start {
        return Some((Vec::new(), start));
    }
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut buf).ok()?;
    let read = buf.len() as u64;
    Some((buf, start + read))
}

// ── Rendering ────────────────────────────────────────────────────────

struct Renderer {
    transcript: String,
    width: usize,
    /// Whether anything has been printed since the header (blank-line logic).
    printed_any: bool,
}

impl Renderer {
    fn new(transcript: &str) -> Self {
        Self {
            transcript: transcript.to_string(),
            width: term_width(),
            printed_any: false,
        }
    }

    /// Task header, styled like the CLI's `⏺ AgentType(description)` line.
    /// Also sets the terminal title so the pane is labelled with the task.
    fn render_header(&self) {
        let (agent_type, description) = read_meta(&self.transcript);
        let name = agent_type.unwrap_or_else(|| "Task".into());
        let desc = description.unwrap_or_else(|| {
            std::path::Path::new(&self.transcript)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        print!("\x1b]2;{name}: {desc}\x07");
        println!("{GREEN}⏺{RESET} {BOLD}{name}{RESET}({desc})");
        println!("  {DIM}⎿  read-only sub-agent view{RESET}");
        let _ = std::io::stdout().flush();
    }

    /// Render one JSONL transcript line.
    fn render_line(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
        };
        let Some(message) = v.get("message") else {
            return; // attachments, progress rows, …
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("user") => self.render_user(&v, message),
            Some("assistant") => self.render_assistant(message),
            _ => {}
        }
    }

    fn render_user(&mut self, row: &serde_json::Value, message: &serde_json::Value) {
        let Some(content) = message.get("content") else {
            return;
        };
        // The task prompt is the parentless first user message.
        if row.get("parentUuid").map(|p| p.is_null()).unwrap_or(false) {
            let text = match content {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(blocks) => blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => return,
            };
            self.blank();
            for (i, l) in self.wrapped(&text, 2).into_iter().enumerate() {
                if i == 0 {
                    println!("{DIM}>{RESET} {l}");
                } else {
                    println!("  {l}");
                }
            }
            return;
        }
        // Later user rows carry tool results.
        let Some(blocks) = content.as_array() else {
            return;
        };
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                self.render_tool_result(block);
            }
        }
    }

    fn render_tool_result(&mut self, block: &serde_json::Value) {
        let is_error = block
            .get("is_error")
            .and_then(|e| e.as_bool())
            .unwrap_or(false);
        let text = match block.get("content") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        let style = if is_error { RED } else { DIM };
        let lines: Vec<&str> = text.lines().collect();
        if lines.is_empty() {
            println!("  {style}⎿  (no output){RESET}");
            self.printed_any = true;
            return;
        }
        for (i, l) in lines.iter().take(RESULT_PREVIEW_LINES).enumerate() {
            let l = clip(l, self.width.saturating_sub(6));
            if i == 0 {
                println!("  {style}⎿  {l}{RESET}");
            } else {
                println!("     {style}{l}{RESET}");
            }
        }
        if lines.len() > RESULT_PREVIEW_LINES {
            println!(
                "     {DIM}… +{} lines{RESET}",
                lines.len() - RESULT_PREVIEW_LINES
            );
        }
        self.printed_any = true;
    }

    fn render_assistant(&mut self, message: &serde_json::Value) {
        let Some(blocks) = message.get("content").and_then(|c| c.as_array()) else {
            return;
        };
        for block in blocks {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    let Some(text) = block.get("text").and_then(|t| t.as_str()) else {
                        continue;
                    };
                    let text = text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    self.blank();
                    for (i, l) in self.wrapped(text, 2).into_iter().enumerate() {
                        let styled = style_inline(&l);
                        if i == 0 {
                            println!("⏺ {styled}");
                        } else {
                            println!("  {styled}");
                        }
                    }
                }
                Some("thinking") => {
                    let Some(text) = block.get("thinking").and_then(|t| t.as_str()) else {
                        continue;
                    };
                    let text = text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    self.blank();
                    println!("{DIM}{ITALIC}✻ Thinking…{RESET}");
                    println!();
                    for l in self.wrapped(text, 2) {
                        println!("  {DIM}{ITALIC}{l}{RESET}");
                    }
                }
                Some("tool_use") => {
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("Tool");
                    let args = block
                        .get("input")
                        .map(|i| summarize_tool_input(name, i))
                        .unwrap_or_default();
                    let args = clip(&args, self.width.saturating_sub(name.len() + 5));
                    self.blank();
                    println!("{GREEN}⏺{RESET} {BOLD}{name}{RESET}({args})");
                }
                _ => {}
            }
        }
    }

    /// Blank separator line between top-level entries (matches the CLI).
    fn blank(&mut self) {
        if self.printed_any {
            println!();
        }
        self.printed_any = true;
    }

    /// Word-wrap `text` to the terminal width minus `indent` columns.
    fn wrapped(&self, text: &str, indent: usize) -> Vec<String> {
        let width = self.width.saturating_sub(indent).max(8);
        let mut out = Vec::new();
        for raw in text.lines() {
            if raw.chars().count() <= width {
                out.push(raw.to_string());
                continue;
            }
            let mut line = String::new();
            let mut count = 0usize;
            for word in raw.split(' ') {
                let wlen = word.chars().count();
                if count > 0 && count + 1 + wlen > width {
                    out.push(std::mem::take(&mut line));
                    count = 0;
                }
                if count > 0 {
                    line.push(' ');
                    count += 1;
                }
                // Hard-split words longer than the width.
                if wlen > width {
                    for ch in word.chars() {
                        if count == width {
                            out.push(std::mem::take(&mut line));
                            count = 0;
                        }
                        line.push(ch);
                        count += 1;
                    }
                } else {
                    line.push_str(word);
                    count += wlen;
                }
            }
            out.push(line);
        }
        if out.is_empty() {
            out.push(String::new());
        }
        out
    }
}

/// One-line argument summary for a tool call, like the CLI's `Bash(cmd)`.
fn summarize_tool_input(name: &str, input: &serde_json::Value) -> String {
    let keys: &[&str] = match name {
        "Bash" => &["command"],
        "Read" | "Write" | "Edit" | "NotebookEdit" => &["file_path"],
        "Glob" | "Grep" => &["pattern"],
        "WebFetch" | "WebSearch" => &["url", "query"],
        "Task" | "Agent" => &["description"],
        _ => &[
            "command",
            "file_path",
            "path",
            "pattern",
            "query",
            "url",
            "description",
            "prompt",
        ],
    };
    for key in keys {
        if let Some(val) = input.get(key).and_then(|v| v.as_str()) {
            // Multi-line commands collapse to their first line.
            return val.lines().next().unwrap_or("").to_string();
        }
    }
    String::new()
}

/// Minimal inline markdown → ANSI within one already-wrapped line:
/// `**bold**` and `` `code` ``. Unpaired markers are left as-is.
fn style_inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 16);
    let mut rest = line;
    loop {
        if let Some(start) = rest.find("**") {
            if let Some(end) = rest[start + 2..].find("**") {
                out.push_str(&rest[..start]);
                out.push_str(BOLD);
                out.push_str(&rest[start + 2..start + 2 + end]);
                out.push_str(RESET);
                rest = &rest[start + 2 + end + 2..];
                continue;
            }
        }
        break;
    }
    out.push_str(rest);
    // Inline code on the bold-resolved string.
    let s = out;
    let mut out = String::with_capacity(s.len() + 16);
    let mut rest = s.as_str();
    loop {
        if let Some(start) = rest.find('`') {
            if let Some(end) = rest[start + 1..].find('`') {
                out.push_str(&rest[..start]);
                out.push_str(CYAN);
                out.push_str(&rest[start + 1..start + 1 + end]);
                out.push_str(RESET);
                rest = &rest[start + 1 + end + 1..];
                continue;
            }
        }
        break;
    }
    out.push_str(rest);
    out
}

/// Clip to `max` chars with an ellipsis (ANSI-free input).
fn clip(s: &str, max: usize) -> String {
    let max = max.max(8);
    if s.chars().count() <= max {
        return s.to_string();
    }
    let clipped: String = s.chars().take(max - 1).collect();
    format!("{clipped}…")
}

/// agentType + description from the transcript's `.meta.json` sidecar.
fn read_meta(transcript: &str) -> (Option<String>, Option<String>) {
    let meta_path = std::path::Path::new(transcript)
        .with_extension("")
        .with_extension("meta.json");
    let Ok(content) = std::fs::read_to_string(meta_path) else {
        return (None, None);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) else {
        return (None, None);
    };
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    (get("agentType"), get("description"))
}

fn term_width() -> usize {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            return ws.ws_col as usize;
        }
    }
    80
}

/// Puts the tty into no-echo/non-canonical mode (typed keys are swallowed —
/// this is a read-only view) and hides the cursor; restores both on drop.
struct TtyGuard {
    saved: Option<libc::termios>,
}

impl TtyGuard {
    fn new() -> Self {
        print!("\x1b[?25l");
        let _ = std::io::stdout().flush();
        let saved = unsafe {
            if libc::isatty(libc::STDIN_FILENO) == 1 {
                let mut t: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &mut t) == 0 {
                    let saved = t;
                    t.c_lflag &= !(libc::ECHO | libc::ICANON);
                    let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
                    Some(saved)
                } else {
                    None
                }
            } else {
                None
            }
        };
        Self { saved }
    }
}

impl Drop for TtyGuard {
    fn drop(&mut self) {
        if let Some(saved) = self.saved {
            unsafe {
                let _ = libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &saved);
            }
        }
        print!("\x1b[?25h");
        let _ = std::io::stdout().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn renderer(width: usize) -> Renderer {
        Renderer {
            transcript: String::new(),
            width,
            printed_any: false,
        }
    }

    #[test]
    fn tool_input_summary_picks_relevant_key() {
        assert_eq!(
            summarize_tool_input("Bash", &json!({"command": "ls -la", "timeout": 5})),
            "ls -la"
        );
        assert_eq!(
            summarize_tool_input("Read", &json!({"file_path": "/a/b.rs", "offset": 3})),
            "/a/b.rs"
        );
        assert_eq!(
            summarize_tool_input("Grep", &json!({"pattern": "fn main", "path": "src"})),
            "fn main"
        );
        // Unknown tools fall back to the generic key list.
        assert_eq!(
            summarize_tool_input("Mystery", &json!({"query": "hello"})),
            "hello"
        );
        // No recognizable key → empty.
        assert_eq!(summarize_tool_input("Weird", &json!({"foo": 1})), "");
    }

    #[test]
    fn multiline_command_collapses_to_first_line() {
        assert_eq!(
            summarize_tool_input("Bash", &json!({"command": "cd /x\nmake\n./run"})),
            "cd /x"
        );
    }

    #[test]
    fn inline_styles_bold_and_code() {
        let out = style_inline("a **b** c `d` e");
        assert!(out.contains(BOLD));
        assert!(out.contains(CYAN));
        assert!(out.contains("b"));
        assert!(out.contains("d"));
        // Plain text is preserved.
        assert!(out.starts_with("a "));
        assert!(out.ends_with(" e"));
    }

    #[test]
    fn inline_styles_leave_unpaired_markers() {
        assert_eq!(style_inline("just **one marker"), "just **one marker");
        assert_eq!(style_inline("code `unterminated"), "code `unterminated");
    }

    #[test]
    fn clip_respects_char_boundaries() {
        assert_eq!(clip("hello", 80), "hello");
        assert_eq!(clip("hello world", 8), "hello w…");
        // Multi-byte chars don't panic or split.
        let s = "héllo wörld ☃☃☃☃";
        let clipped = clip(s, 8);
        assert!(clipped.chars().count() <= 8);
    }

    #[test]
    fn wrapping_breaks_long_lines_to_width() {
        let r = renderer(20);
        let wrapped = r.wrapped("the quick brown fox jumped over the lazy dog", 2);
        assert!(wrapped.len() > 1);
        for line in &wrapped {
            assert!(line.chars().count() <= 18, "line too long: {line:?}");
        }
    }

    #[test]
    fn wrapping_preserves_explicit_newlines() {
        let r = renderer(80);
        let wrapped = r.wrapped("line one\nline two", 0);
        assert_eq!(wrapped, vec!["line one".to_string(), "line two".to_string()]);
    }

    #[test]
    fn wrapping_hard_splits_overlong_words() {
        let r = renderer(10);
        let wrapped = r.wrapped("aaaaaaaaaaaaaaaaaaaa", 0);
        assert!(wrapped.len() > 1);
        for line in &wrapped {
            assert!(line.chars().count() <= 10);
        }
    }
}
