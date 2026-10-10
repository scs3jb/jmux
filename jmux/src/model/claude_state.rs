//! Claude pane state detection — a Rust port of deck's classifier
//! (~/src/deck/deck/sessions.py + questions.py), so the sidebar shows the same
//! states the deck does.
//!
//! State, derived from an agent pane's title + visible terminal text:
//!
//!   NeedsInput  a selectable question/permission menu is on screen, or the
//!               last response asks the user something
//!   Working     the main agent's turn is running ("esc to interrupt"/spinner)
//!   Waiting     back at the prompt, but a backgrounded shell/agent is running
//!   None (idle) a prompt is shown, nothing running, no question

/// Most-urgent-last so `Ord`/`max()` picks the state that matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClaudeState {
    Waiting,
    Working,
    NeedsInput,
}

/// Classify an agent pane's state from its visible text + raw (unstripped)
/// title. Precedence mirrors deck's `classify()`:
///
///   1. a real selection menu           → NeedsInput (a true block)
///   2. the main turn is running        → Working
///   3. a backgrounded task is running  → Waiting
///   4. the last response is a question → NeedsInput (free-text)
///   5. otherwise                       → None (idle)
///
/// Waiting sits *above* the soft "ends in ?" rule on purpose: if a background
/// shell is still running, any trailing "?" is stale (already answered).
pub fn classify(text: &str, raw_title: &str) -> Option<ClaudeState> {
    if has_pointer_menu(text) || has_menu_footer(text) {
        return Some(ClaudeState::NeedsInput);
    }
    let live = tail(text, 12);
    if is_active_turn(&live, raw_title) {
        return Some(ClaudeState::Working);
    }
    if is_background_running(&live) {
        return Some(ClaudeState::Waiting);
    }
    if last_response_is_question(text) {
        return Some(ClaudeState::NeedsInput);
    }
    None
}

/// A hard selection menu is on screen (pointer menu or "enter to select"
/// footer). This is the STRICT NeedsInput test used by the goal auto-driver:
/// the soft "last response ends in ?" heuristic in [`classify`] is fine for a
/// sidebar badge but far too twitchy to pause an autonomous run on.
pub fn has_selection_menu(text: &str) -> bool {
    has_pointer_menu(text) || has_menu_footer(text)
}

/// Check whether `word` appears as a whole word in `haystack` (lowercase).
/// A word boundary is the start/end of string or a non-alphanumeric character.
pub fn is_word_match(haystack: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(word) {
        let abs = start + pos;
        let before_ok = abs == 0
            || !haystack.as_bytes()[abs - 1].is_ascii_alphanumeric()
                && haystack.as_bytes()[abs - 1] != b'_';
        let after_idx = abs + word.len();
        let after_ok = after_idx >= haystack.len()
            || !haystack.as_bytes()[after_idx].is_ascii_alphanumeric()
                && haystack.as_bytes()[after_idx] != b'_';
        if before_ok && after_ok {
            return true;
        }
        start = abs + word.len();
    }
    false
}

/// Whether the terminal screen text looks like an AI agent session (Antigravity/agy,
/// Claude Code, Codex) rather than an ordinary shell. Used when the pane title
/// still looks like a generic shell ("bash", "user@host:~/path") because the
/// agent did not rewrite the window title.
pub fn is_agent_screen(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("antigravity")
        || lower.contains("thought process")
        || lower.contains("generating...")
        || lower.contains("esc to cancel")
        || lower.contains("enter/esc to cancel")
        || lower.contains("activated bash mode")
        || lower.contains("? for shortcuts")
        || lower.contains("> you:")
        || lower.contains("claude code")
        || lower.contains("esc to interrupt")
        || is_word_match(&lower, "agy")
}

/// Whether a terminal pane's title looks like a plain shell rather than an
/// agent — "user@host:~/path" or a bare shell name. Such panes never carry a
/// Claude state unless `is_agent_screen()` detects an active agent on screen.
pub fn is_shell_title(title: &str) -> bool {
    let t = title.trim();
    let lower = t.to_ascii_lowercase();
    // Known agent titles are never shell titles, even if formatted with user@host
    if lower.contains("claude")
        || lower.contains("codex")
        || lower.contains("antigravity")
        || lower.contains("gemini")
        || is_word_match(&lower, "agy")
    {
        return false;
    }
    if matches!(
        lower.as_str(),
        "bash" | "zsh" | "fish" | "sh" | "-bash" | "-zsh"
    ) {
        return true;
    }
    // ^[\w.\-]+@[\w.\-]+:
    let mut seen_at = false;
    let mut before = 0usize;
    let mut after = 0usize;
    for c in t.chars() {
        match c {
            ':' => return seen_at && after > 0,
            '@' => {
                if seen_at || before == 0 {
                    return false;
                }
                seen_at = true;
            }
            c if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' => {
                if seen_at {
                    after += 1;
                } else {
                    before += 1;
                }
            }
            _ => return false,
        }
    }
    false
}

/// The live status region — only the bottom of the pane. Scanning the whole
/// buffer matches stale status lines left in scrollback.
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Braille Patterns block — Claude animates its working spinner from these and
/// prepends a frame to the pane title while a turn runs.
fn is_braille(c: char) -> bool {
    ('\u{2800}'..='\u{28FF}').contains(&c)
}

/// A title spinner frame: Codex and Claude animate the pane title with braille
/// frames or quarter-circle frames (◐◓◑◒), rewriting it ~5×/s while working.
pub fn is_spinner_glyph(c: char) -> bool {
    is_braille(c) || matches!(c, '◐' | '◓' | '◑' | '◒')
}

/// The title without its leading spinner frame, so a label does not show a
/// stale frame when the sidebar skips rebuilding for spinner-only changes.
pub fn strip_spinner(title: &str) -> &str {
    let rest = title.trim_start_matches(is_spinner_glyph);
    if rest.len() == title.len() {
        title
    } else {
        rest.trim_start()
    }
}

/// Whether the *main* agent's turn is running (interruptible).
/// Tells:
/// - animated spinner glyph in the title (Claude braille, Codex quarter-circles)
/// - cancel/interrupt hints in the live footer:
///   - Claude/Codex: "esc to interrupt"
///   - Antigravity (agy): "esc to cancel", "enter/esc to cancel", "generating..."
/// - live parenthesised timer ("✢ Working… (1m 8s ·")
/// - active in-terminal spinner line (agy animates braille spinners `⣷ Thinking...`,
///   `⠋ Thought Process`, `⣷ tool: write_file` directly in the terminal text)
fn is_active_turn(live: &str, raw_title: &str) -> bool {
    if raw_title
        .trim_start()
        .chars()
        .next()
        .is_some_and(is_spinner_glyph)
    {
        return true;
    }
    let lower = live.to_ascii_lowercase();
    if lower.contains("esc to interrupt")
        || lower.contains("esc to cancel")
        || lower.contains("enter/esc to cancel")
        || lower.contains("generating...")
    {
        return true;
    }
    if has_live_spinner(live) {
        return true;
    }
    has_live_timer(live)
}

/// Whether `live` contains an active spinner line from an in-progress agent turn
/// (e.g. `agy`'s braille spinner `⣷ Thinking...`, `⠋ Thought Process`, `⣷ tool: write_file`).
fn has_live_spinner(live: &str) -> bool {
    for line in live.lines() {
        let s = line.trim();
        if s.chars().any(is_spinner_glyph) {
            let lower = s.to_ascii_lowercase();
            if lower.contains("thinking")
                || lower.contains("thought")
                || lower.contains("generating")
                || lower.contains("tool:")
                || lower.contains("to cancel")
                || lower.contains("signing in")
                || lower.contains("working")
                || lower.contains("running")
            {
                if lower.contains("crunched for") {
                    continue;
                }
                return true;
            }
        }
    }
    false
}

/// `\(\d+m \d+s` or `\(\d+s\b`
fn has_live_timer(s: &str) -> bool {
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] != '(' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let d0 = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > d0 {
            match b.get(j) {
                // (\d+m \d+s
                Some('m') if b.get(j + 1) == Some(&' ') => {
                    let mut k = j + 2;
                    let d1 = k;
                    while k < b.len() && b[k].is_ascii_digit() {
                        k += 1;
                    }
                    if k > d1 && b.get(k) == Some(&'s') {
                        return true;
                    }
                }
                // (\d+s\b
                Some('s') => {
                    let boundary = b
                        .get(j + 1)
                        .map_or(true, |c| !c.is_alphanumeric() && *c != '_');
                    if boundary {
                        return true;
                    }
                }
                _ => {}
            }
        }
        i = j.max(i + 1);
    }
    false
}

/// Whether a backgrounded shell/subagent is running while the main agent sits
/// back at the prompt — the "waiting on something to finish" state.
fn is_background_running(live: &str) -> bool {
    let lower = live.to_ascii_lowercase();
    if lower.contains("shell still running")
        || lower.contains("shells still running")
        || lower.contains("task still running")
        || lower.contains("tasks still running")
    {
        return true;
    }
    if live.contains("↓ to manage") {
        return true;
    }
    if has_shell_count(&lower) || waiting_for_background_agents(&lower) {
        return true;
    }
    has_subagent_row(live)
}

/// `\b\d+ shells?\b` — a backgrounded-command count ("· 1 shell ·").
fn has_shell_count(lower: &str) -> bool {
    for (pos, _) in lower.match_indices("shell") {
        // preceding: " " preceded by ≥1 digit with a word boundary before it
        let before = &lower[..pos];
        let Some(before) = before.strip_suffix(' ') else {
            continue;
        };
        let digits = before.chars().rev().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            continue;
        }
        let pre = before.chars().rev().nth(digits);
        if pre.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        // following: optional 's', then a word boundary
        let rest = &lower[pos + "shell".len()..];
        let rest = rest.strip_prefix('s').unwrap_or(rest);
        if rest.chars().next().map_or(true, |c| !c.is_alphanumeric() && c != '_') {
            return true;
        }
    }
    false
}

/// `waiting for \d+ background agent` or `waiting for \d+ background task`
fn waiting_for_background_agents(lower: &str) -> bool {
    for (pos, _) in lower.match_indices("waiting for ") {
        let rest = &lower[pos + "waiting for ".len()..];
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 {
            let tail = &rest[digits..];
            if tail.starts_with(" background agent") || tail.starts_with(" background task") {
                return true;
            }
        }
    }
    false
}

/// `◯\s+\S` — a subagent monitor row ("◯ general-purpose … 5m 26s").
fn has_subagent_row(live: &str) -> bool {
    for (pos, m) in live.match_indices('◯') {
        let rest = &live[pos + m.len()..];
        let trimmed = rest.trim_start();
        if trimmed.len() < rest.len() && !trimmed.is_empty() && !trimmed.starts_with('\n') {
            return true;
        }
    }
    false
}

// ── Selection menus ──────────────────────────────────────────────────────────

/// A line that looks like a numbered menu option:
/// leading indent, optional ❯ or > pointer, "N." or "N)", then a non-empty label.
/// Returns (pointed, number).
fn parse_option(line: &str) -> Option<(bool, u32)> {
    let mut s = line.trim_start();
    let pointed = s.starts_with('❯') || s.starts_with('>');
    if pointed {
        let prefix_len = if s.starts_with('❯') { '❯'.len_utf8() } else { 1 };
        s = s[prefix_len..].trim_start();
    }
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let rest = &s[digits..];
    let mut chars = rest.chars();
    if !matches!(chars.next(), Some('.') | Some(')')) {
        return None;
    }
    let after = chars.as_str();
    if !after.starts_with(char::is_whitespace) || after.trim().is_empty() {
        return None;
    }
    s[..digits].parse().ok().map(|n| (pointed, n))
}

/// The classic ❯ menu: two or more contiguous lines numbered sequentially from
/// 1, with at least one line bearing the ❯ pointer. Strict, because scrollback
/// is full of things that loosely look like options.
fn has_pointer_menu(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let mut j = i;
        let mut expected = 1u32;
        let mut pointed = false;
        while j < lines.len() {
            match parse_option(lines[j]) {
                Some((p, n)) if n == expected => {
                    pointed |= p;
                    expected += 1;
                    j += 1;
                }
                _ => break,
            }
        }
        if j - i >= 2 && pointed {
            return true;
        }
        i = if j > i { j } else { i + 1 };
    }
    false
}

/// The newer boxed AskUserQuestion / plan picker has no ❯ pointer; its
/// unmistakable tell is the nav footer near the bottom.
fn has_menu_footer(text: &str) -> bool {
    let lower = tail(text, 8).to_ascii_lowercase();
    lower.contains("enter to select")
        || lower.contains("enter to submit")
        || lower.contains("enter to confirm")
        || lower.contains("↑/↓ to navigate")
        || (lower.contains("↑/↓") && (lower.contains("navigate") || lower.contains("select")))
}

// ── "Ends in a question" ─────────────────────────────────────────────────────

/// Chrome lines that are not part of Claude's response (footers, separators,
/// status/recap/spinner lines, task-panel rows, the input box).
const CHROME_GLYPHS: &[char] = &[
    '✻', '✶', '✳', '✢', '✣', '※', '⏵', '◯', '●', '◻', '◼', '✔', '⎿', '│', '⚙',
];
const CHROME_TEXT: &[&str] = &[
    "for shortcuts",
    "esc to interrupt",
    "esc to cancel",
    "thought process",
    "generating...",
    "auto mode on",
    "ctrl+t",
    "for agents",
    "tokens",
    "completed",
];

/// Whether Claude's most recent response asks the user something: the tail of
/// the actual response (chrome removed) has a sentence ending in "?" — not a
/// "?:" ternary or a "foo?bar" url.
fn last_response_is_question(text: &str) -> bool {
    let mut recent: [Option<String>; 4] = Default::default();
    let mut idx = 0;
    for raw in text.lines() {
        let owned = raw.replace('\u{a0}', " ");
        let s = owned.trim();
        if s.is_empty() || matches!(s, "❯" | "›" | "▶") {
            continue;
        }
        if s.chars().all(|c| "─—-=_ │".contains(c)) {
            continue;
        }
        let lower = s.to_ascii_lowercase();
        if s.starts_with(CHROME_GLYPHS) || CHROME_TEXT.iter().any(|t| lower.contains(t)) {
            continue;
        }
        recent[idx % 4] = Some(s.to_string());
        idx += 1;
    }
    recent.iter().flatten().any(|line| {
        let chars: Vec<char> = line.chars().collect();
        chars.iter().enumerate().any(|(i, c)| {
            *c == '?' && chars.get(i + 1).map_or(true, |n| n.is_whitespace())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_titles() {
        assert!(is_shell_title("jbriggs@bigboy:~/src"));
        assert!(is_shell_title("bash"));
        assert!(is_shell_title("  zsh  "));
        assert!(!is_shell_title("Create local Whisper replacement"));
        assert!(!is_shell_title("claude"));
        assert!(!is_shell_title("fix the sidebar @ home: part 2"));
    }

    #[test]
    fn braille_title_is_working() {
        assert_eq!(classify("", "⠋ Fixing the bug"), Some(ClaudeState::Working));
        assert_eq!(classify("", "✳ Fixing the bug"), None);
    }

    #[test]
    fn strips_spinner_frames() {
        assert_eq!(strip_spinner("⠼ Review spec | torrent-to-plex"), "Review spec | torrent-to-plex");
        assert_eq!(strip_spinner("◐ UI flickering"), "UI flickering");
        assert_eq!(strip_spinner("✳ Claude Code"), "✳ Claude Code");
        assert_eq!(strip_spinner("jbriggs@host:~"), "jbriggs@host:~");
    }

    #[test]
    fn esc_to_interrupt_is_working() {
        let text = "some output\n✢ Accomplishing… (esc to interrupt)\n";
        assert_eq!(classify(text, ""), Some(ClaudeState::Working));
    }

    #[test]
    fn live_timer_is_working() {
        assert_eq!(
            classify("✢ Accomplishing… (1m 8s · ↓ 2.4k tokens)", ""),
            Some(ClaudeState::Working)
        );
        assert_eq!(
            classify("✢ Pondering… (42s · esc)", ""),
            Some(ClaudeState::Working)
        );
        // Past-tense summary must NOT count as active.
        assert_eq!(classify("✳ Crunched for 2m 15s\n❯", ""), None);
    }

    #[test]
    fn background_shell_is_waiting() {
        assert_eq!(
            classify("· 1 shell · ↓ to manage\n❯", ""),
            Some(ClaudeState::Waiting)
        );
        assert_eq!(
            classify("2 shells still running\n❯", ""),
            Some(ClaudeState::Waiting)
        );
        assert_eq!(
            classify("◯ general-purpose … 5m 26s · ↓ 48.4k tokens\n❯", ""),
            Some(ClaudeState::Waiting)
        );
        assert_eq!(
            classify("waiting for 2 background agents to finish\n❯", ""),
            Some(ClaudeState::Waiting)
        );
    }

    #[test]
    fn pointer_menu_is_needs_input() {
        let text = "Do you want to proceed?\n❯ 1. Yes\n  2. Yes, and don't ask again\n  3. No\n";
        assert_eq!(classify(text, ""), Some(ClaudeState::NeedsInput));
    }

    #[test]
    fn menu_footer_is_needs_input() {
        let text = "Which approach?\n  1. Fast\n  2. Thorough\nenter to select · esc to cancel\n";
        assert_eq!(classify(text, ""), Some(ClaudeState::NeedsInput));
    }

    #[test]
    fn numbered_list_without_pointer_is_not_a_menu() {
        let text = "Here are the steps:\n  1. Build\n  2. Test\n  3. Ship\n❯";
        assert_eq!(classify(text, ""), None);
    }

    #[test]
    fn trailing_question_is_needs_input() {
        let text = "I fixed the bug in main.rs.\nWant me to also add a test?\n❯";
        assert_eq!(classify(text, ""), Some(ClaudeState::NeedsInput));
    }

    #[test]
    fn stale_question_with_background_shell_is_waiting() {
        // The user already answered; the shell is the live signal.
        let text = "Want me to also add a test?\nrunning…\n· 1 shell · ↓ to manage\n❯";
        assert_eq!(classify(text, ""), Some(ClaudeState::Waiting));
    }

    #[test]
    fn ternary_and_url_question_marks_ignored() {
        // "?" not followed by whitespace/EOL: ?: ternaries, ?. chaining, urls.
        assert_eq!(classify("let x = cond ?: b;\n❯", ""), None);
        assert_eq!(classify("let y = value?.method();\n❯", ""), None);
        assert_eq!(classify("see https://example.com?foo=bar\n❯", ""), None);
    }

    #[test]
    fn idle_prompt_is_none() {
        assert_eq!(classify("$ ls\nfile.txt\n❯", ""), None);
        assert_eq!(classify("", ""), None);
    }

    #[test]
    fn urgency_ordering() {
        assert!(ClaudeState::NeedsInput > ClaudeState::Working);
        assert!(ClaudeState::Working > ClaudeState::Waiting);
    }

    #[test]
    fn stale_scrollback_outside_tail_ignored() {
        // "esc to interrupt" 20 lines up must not count.
        let mut text = String::from("✢ old (esc to interrupt)\n");
        text.push_str(&"line\n".repeat(20));
        text.push('❯');
        assert_eq!(classify(&text, ""), None);
    }

    #[test]
    fn agy_generating_is_working() {
        let text = "Analyzing directory...\nGenerating... (Enter/Esc to cancel)\n";
        assert_eq!(classify(text, "bash"), Some(ClaudeState::Working));
        assert_eq!(classify(text, "jbriggs@bigboy:~/src"), Some(ClaudeState::Working));
    }

    #[test]
    fn agy_thinking_spinner_is_working() {
        let text = "Antigravity v1.3.1\n⣷ Thinking... (12s)\n";
        assert_eq!(classify(text, "bash"), Some(ClaudeState::Working));
        let text2 = "▾ Thought Process\n⠋ Thinking...\n";
        assert_eq!(classify(text2, "bash"), Some(ClaudeState::Working));
    }

    #[test]
    fn agy_tool_execution_is_working() {
        let text = "⣷ tool: run_command\nGenerating... (Enter/Esc to cancel)\n";
        assert_eq!(classify(text, "bash"), Some(ClaudeState::Working));
        let text2 = "⚙ tool: write_file\nGenerating... (Enter/Esc to cancel)\n";
        assert_eq!(classify(text2, "bash"), Some(ClaudeState::Working));
    }

    #[test]
    fn agy_idle_prompt_is_none() {
        let text = "I completed the changes.\n> you: \n? for shortcuts\n";
        assert_eq!(classify(text, "bash"), None);
        assert!(is_agent_screen(text));
    }

    #[test]
    fn agy_screen_detection() {
        assert!(is_agent_screen("Antigravity 2.0"));
        assert!(is_agent_screen("▾ Thought Process"));
        assert!(is_agent_screen("Generating... (Enter/Esc to cancel)"));
        assert!(is_agent_screen("> you: "));
        assert!(is_agent_screen("running agy in terminal"));
        assert!(!is_agent_screen("ls -la\nfile1.txt file2.txt\n$ "));
    }

    #[test]
    fn agy_menu_is_needs_input() {
        let text = "Allow command execution?\n> 1. Yes\n  2. No\n";
        assert_eq!(classify(text, "bash"), Some(ClaudeState::NeedsInput));
        let text2 = "Select an option:\n  Option 1\n  Option 2\nenter to submit · esc to cancel\n";
        assert_eq!(classify(text2, "bash"), Some(ClaudeState::NeedsInput));
    }

    #[test]
    fn agy_title_recognition() {
        assert!(!is_shell_title("agy"));
        assert!(!is_shell_title("agy -p 'fix bug'"));
        assert!(!is_shell_title("Antigravity"));
    }
}
