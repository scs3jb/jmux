//! Agent hibernation — pause/resume the foreground process group of a panel's
//! TTY (the running agent, e.g. `claude`/`codex`) via SIGSTOP/SIGCONT so an
//! idle agent stops consuming CPU until it is needed again.
//!
//! Ghostty owns the PTY child and does not expose its PID, so we locate the
//! foreground process group from the panel's reported TTY (`/dev/pts/N`) by
//! reading `/proc/<pid>/stat`'s `tpgid` field and signal the whole group.

/// Parse the pts index from a tty path like "/dev/pts/3".
fn pts_index(tty: &str) -> Option<u32> {
    tty.strip_prefix("/dev/pts/").and_then(|n| n.parse().ok())
}

/// Convert a raw `tty_nr` device number to a pts index (UNIX98 pts, major 136).
fn tty_nr_to_pts_index(tty_nr: u64) -> Option<u32> {
    if tty_nr == 0 {
        return None;
    }
    let major = ((tty_nr >> 8) & 0xFF) as u32;
    if major != 136 {
        return None;
    }
    let minor = ((tty_nr & 0xFF) | ((tty_nr >> 12) & 0xFFF00)) as u32;
    Some(minor)
}

/// Find the foreground process-group id (the `tpgid`) controlling `tty`.
///
/// All processes sharing a controlling terminal report the same `tpgid`, so the
/// first process found on the TTY gives us the foreground group to signal.
fn foreground_pgrp(tty: &str) -> Option<i32> {
    let idx = pts_index(tty)?;
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let name = entry.file_name();
        let Some(pid_str) = name.to_str() else {
            continue;
        };
        if pid_str.is_empty() || !pid_str.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid_str}/stat")) else {
            continue;
        };
        // The comm field is parenthesized and may contain spaces/parens; the
        // remaining fields start after the final ')'.
        let Some((_, after)) = stat.rsplit_once(')') else {
            continue;
        };
        // After comm: state(0) ppid(1) pgrp(2) session(3) tty_nr(4) tpgid(5)
        let fields: Vec<&str> = after.split_whitespace().take(6).collect();
        let Some(tty_nr) = fields.get(4).and_then(|f| f.parse::<u64>().ok()) else {
            continue;
        };
        if tty_nr_to_pts_index(tty_nr) != Some(idx) {
            continue;
        }
        if let Some(tpgid) = fields.get(5).and_then(|f| f.parse::<i32>().ok()) {
            if tpgid > 0 {
                return Some(tpgid);
            }
        }
    }
    None
}

/// Send `sig` to the foreground process group of `tty`. Returns true on success.
fn signal_tty(tty: &str, sig: i32) -> bool {
    match foreground_pgrp(tty) {
        // Negative pid signals the whole process group.
        Some(pgrp) => unsafe { libc::kill(-pgrp, sig) == 0 },
        None => false,
    }
}

/// Pause (SIGSTOP) the agent running on `tty`.
pub fn hibernate(tty: &str) -> bool {
    signal_tty(tty, libc::SIGSTOP)
}

/// Resume (SIGCONT) the agent running on `tty`.
pub fn wake(tty: &str) -> bool {
    signal_tty(tty, libc::SIGCONT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pts_index() {
        assert_eq!(pts_index("/dev/pts/3"), Some(3));
        assert_eq!(pts_index("/dev/pts/0"), Some(0));
        assert_eq!(pts_index("/dev/tty1"), None);
        assert_eq!(pts_index("not-a-tty"), None);
    }

    #[test]
    fn test_tty_nr_to_pts_index() {
        // pts major is 136; minor 3 → tty_nr = (136 << 8) | 3 = 0x8803.
        let tty_nr = (136u64 << 8) | 3;
        assert_eq!(tty_nr_to_pts_index(tty_nr), Some(3));
        // A non-pts major (e.g. 4 = tty) must not be treated as pts.
        assert_eq!(tty_nr_to_pts_index((4u64 << 8) | 1), None);
        assert_eq!(tty_nr_to_pts_index(0), None);
    }
}
