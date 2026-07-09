//! Resolve the live Claude Code session id for a local terminal panel, so a
//! restored tab can `claude --resume <id>` straight back into the exact
//! conversation it had open — rather than `claude --continue`, which only picks
//! the newest conversation per directory and so collapses multiple same-dir
//! tabs onto one session.
//!
//! Claude keeps no held file handle to its transcript (it appends and closes)
//! and puts the session id in neither its argv nor its environment, so the id
//! can't be read directly off the process. Instead we locate the panel's
//! `claude` process by the `CMUX_PANEL_ID` env var cmux injects into the shell
//! (inherited by its children), read that process's working directory, and pick
//! the most-recently-written transcript under `~/.claude/projects/<encoded>/`.
//!
//! This is inherently local: a remote panel's `claude` runs on the far host and
//! never appears in the local `/proc`, so remote panels resolve to nothing here
//! and fall back to the directory-level `--continue` behaviour.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use uuid::Uuid;

/// Encode an absolute working directory into Claude's project-dir name: every
/// `/` and `.` becomes `-` (e.g. `/home/u/src/app` → `-home-u-src-app`).
fn encode_project_dir(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

/// `~/.claude/projects`, where Claude Code stores per-project transcripts.
fn claude_projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude/projects"))
}

/// True if `/proc/<pid>` looks like a `claude` process — matched on `comm`
/// first (cheap) and the argv[0] basename as a fallback (covers installs that
/// launch under a different `comm` such as `node`).
fn is_claude_proc(proc_dir: &Path) -> bool {
    if let Ok(comm) = std::fs::read_to_string(proc_dir.join("comm")) {
        if comm.trim() == "claude" {
            return true;
        }
    }
    if let Ok(cmdline) = std::fs::read(proc_dir.join("cmdline")) {
        if let Some(arg0) = cmdline.split(|&b| b == 0).next() {
            let arg0 = String::from_utf8_lossy(arg0);
            if Path::new(arg0.as_ref())
                .file_name()
                .map(|n| n == "claude")
                .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

/// Read the `CMUX_PANEL_ID` value out of a NUL-delimited `environ` blob.
fn panel_id_from_environ(environ: &[u8]) -> Option<Uuid> {
    for kv in environ.split(|&b| b == 0) {
        if let Some(v) = kv.strip_prefix(b"CMUX_PANEL_ID=") {
            return std::str::from_utf8(v).ok().and_then(|s| Uuid::parse_str(s.trim()).ok());
        }
    }
    None
}

/// Map every locally-running `claude` process to the panel it belongs to and
/// that process's current working directory, in a single `/proc` walk.
pub fn all_local_claude_cwds() -> HashMap<Uuid, String> {
    let mut map = HashMap::new();
    let Ok(read_dir) = std::fs::read_dir("/proc") else {
        return map;
    };
    for entry in read_dir.flatten() {
        // Only numeric /proc/<pid> entries are processes.
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        let proc_dir = entry.path();
        if !is_claude_proc(&proc_dir) {
            continue;
        }
        let Ok(environ) = std::fs::read(proc_dir.join("environ")) else {
            continue;
        };
        let Some(panel_id) = panel_id_from_environ(&environ) else {
            continue;
        };
        if let Ok(cwd) = std::fs::read_link(proc_dir.join("cwd")) {
            if let Some(cwd) = cwd.to_str() {
                map.insert(panel_id, cwd.to_string());
            }
        }
    }
    map
}

/// Map every local terminal panel to its shell's live working directory, in a
/// single `/proc` walk. Keyed by `CMUX_PANEL_ID`; the value is the cwd of the
/// panel's shell — the process that is a direct child of this cmux process, so
/// its cwd tracks `cd` even while an agent (claude, etc.) holds the foreground.
///
/// This is the authoritative directory for session capture: the stored
/// `panel.directory` is only refreshed by shell-integration pwd reports (OSC-7),
/// which never fire while a long-running foreground process like Claude owns the
/// terminal — so it can be frozen at the workspace's launch directory (often
/// HOME). Reading `/proc/<shell>/cwd` sidesteps that entirely.
///
/// Remote panels' shells live on the far host and never appear here, so they are
/// absent from the map and keep their stored (remote) directory.
pub fn all_local_panel_cwds() -> HashMap<Uuid, String> {
    // Only consider processes that are DIRECT children of this cmux process. A
    // panel's shell (or a directly-launched agent) is always such a child, so
    // this both scopes the walk to *this* cmux instance — a second cmux running
    // concurrently has its own child shells we must not read — and picks the
    // authoritative process whose cwd tracks the tab's `cd`. Grandchildren (an
    // agent running under the shell, systemd-inhibit wrappers) inherit the env
    // but have a different ppid, so they're skipped. Lowest pid wins on the rare
    // chance a panel has two direct children, for deterministic output.
    let me = std::process::id();
    let mut best: HashMap<Uuid, (u32, String)> = HashMap::new();
    let Ok(read_dir) = std::fs::read_dir("/proc") else {
        return HashMap::new();
    };
    for entry in read_dir.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let proc_dir = entry.path();
        let is_direct_child = std::fs::read_to_string(proc_dir.join("stat"))
            .ok()
            .and_then(|s| parse_ppid_stat(&s))
            == Some(me);
        if !is_direct_child {
            continue;
        }
        let Ok(environ) = std::fs::read(proc_dir.join("environ")) else {
            continue;
        };
        let Some(panel_id) = panel_id_from_environ(&environ) else {
            continue;
        };
        let Ok(cwd) = std::fs::read_link(proc_dir.join("cwd")) else {
            continue;
        };
        let Some(cwd) = cwd.to_str() else { continue };
        if best.get(&panel_id).map(|(p, _)| pid < *p).unwrap_or(true) {
            best.insert(panel_id, (pid, cwd.to_string()));
        }
    }
    best.into_iter().map(|(k, (_, cwd))| (k, cwd)).collect()
}

/// Parse the parent pid (field 4) from a `/proc/<pid>/stat` line, accounting for
/// the `comm` field being parenthesised and able to contain spaces/parens.
fn parse_ppid_stat(stat: &str) -> Option<u32> {
    let close = stat.rfind(')')?;
    let rest = stat.get(close + 1..)?;
    // After ")" the fields are: state ppid ...
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// Session ids (transcript file stems) for a working directory, newest first.
/// Tries the deterministic encoded project dir; returns empty if it can't be
/// found or read (the caller then falls back to `--continue`).
pub fn session_ids_for_cwd(cwd: &str) -> Vec<String> {
    let Some(root) = claude_projects_dir() else {
        return Vec::new();
    };
    let dir = root.join(encode_project_dir(cwd));
    let Ok(read_dir) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut sessions: Vec<(String, std::time::SystemTime)> = read_dir
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.to_string();
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((stem, mtime))
        })
        .collect();
    // Newest first.
    sessions.sort_by(|a, b| b.1.cmp(&a.1));
    sessions.into_iter().map(|(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_project_dir() {
        assert_eq!(encode_project_dir("/home/u/src/cmux-gtk"), "-home-u-src-cmux-gtk");
        // Dots become dashes too (so a dotfolder yields the `--` Claude uses).
        assert_eq!(encode_project_dir("/home/u/.config/app"), "-home-u--config-app");
        // Existing dashes in a path component are preserved.
        assert_eq!(encode_project_dir("/a/b-c"), "-a-b-c");
    }

    #[test]
    fn test_panel_id_from_environ() {
        let id = Uuid::new_v4();
        let blob = format!("PATH=/bin\0CMUX_PANEL_ID={id}\0HOME=/root\0");
        assert_eq!(panel_id_from_environ(blob.as_bytes()), Some(id));
        assert_eq!(panel_id_from_environ(b"PATH=/bin\0HOME=/root\0"), None);
        assert_eq!(panel_id_from_environ(b"CMUX_PANEL_ID=not-a-uuid\0"), None);
    }

    #[test]
    fn test_parse_ppid_stat() {
        // Normal comm.
        assert_eq!(
            parse_ppid_stat("3548725 (head) R 3548655 3548725 3548655 0 -1"),
            Some(3548655)
        );
        // comm containing parentheses — must use the LAST ')'.
        assert_eq!(
            parse_ppid_stat("1265 ((sd-pam)) S 1263 1263 1263 0 -1"),
            Some(1263)
        );
        // comm containing spaces and parens.
        assert_eq!(
            parse_ppid_stat("42 (weird (proc) name) S 7 7 7"),
            Some(7)
        );
        // Malformed input yields None rather than panicking.
        assert_eq!(parse_ppid_stat("garbage without paren"), None);
        assert_eq!(parse_ppid_stat(""), None);
    }
}
