# cmux fish integration — CWD reporting, git branch, PR polling, port kicks,
# scrollback restoration, async git HEAD watcher, semantic prompt markers,
# fast git HEAD resolution, smart PR polling, process cleanup.
#
# Sourced automatically by fish from `fish/vendor_conf.d/` when cmux prepends
# the shell-integration directory to XDG_DATA_DIRS. Activates only when
# CMUX_SOCKET is present and the shell is interactive.
#
# Protocol: V1 text lines over the cmux Unix socket.
#
# All setup is deferred to the first prompt so this file is inert in
# non-interactive shells, and `return` (valid only inside a function) can
# guard cleanly without risking `exit` terminating a sourced config.

function _cmux_fish_setup --on-event fish_prompt -d "cmux shell integration setup"
    # Run setup exactly once
    functions -e _cmux_fish_setup

    # Bail if not running inside cmux
    set -q CMUX_SOCKET; or return
    # Guard against double-loading
    set -q _CMUX_FISH_INTEGRATION_LOADED; and return
    set -g _CMUX_FISH_INTEGRATION_LOADED 1

    # ── Socket transport ──────────────────────────────────────────────
    # Detect which socket client is available once at load time.
    set -g _cmux_send_cmd ""
    if command -q ncat
        set -g _cmux_send_cmd ncat
    else if command -q socat
        set -g _cmux_send_cmd socat
    else if command -q nc
        set -g _cmux_send_cmd nc
    end

    function _cmux_send
        switch $_cmux_send_cmd
            case ncat
                echo $argv[1] | ncat -U "$CMUX_SOCKET" 2>/dev/null
            case socat
                echo $argv[1] | socat - UNIX-CONNECT:"$CMUX_SOCKET" 2>/dev/null
            case nc
                echo $argv[1] | nc -U "$CMUX_SOCKET" -w 1 2>/dev/null
        end
    end

    function _cmux_send_fire_forget
        _cmux_send "$argv[1]" >/dev/null 2>&1 &
        disown 2>/dev/null
    end

    # ── Workspace / panel identifiers ────────────────────────────────
    function _cmux_flags
        set -l flags
        test -n "$CMUX_WORKSPACE_ID"; and set -a flags "--tab=$CMUX_WORKSPACE_ID"
        test -n "$CMUX_PANEL_ID"; and set -a flags "--panel=$CMUX_PANEL_ID"
        echo $flags
    end

    # ── Scrollback restoration ────────────────────────────────────────
    # On session restore cmux writes saved scrollback to a temp file and sets
    # CMUX_RESTORE_SCROLLBACK_FILE. We replay it once, then delete the file.
    function _cmux_restore_scrollback_once
        set -q CMUX_RESTORE_SCROLLBACK_FILE; or return
        set -l f "$CMUX_RESTORE_SCROLLBACK_FILE"
        set -e CMUX_RESTORE_SCROLLBACK_FILE
        if test -f "$f"
            cat "$f" 2>/dev/null
            rm -f "$f" 2>/dev/null
        end
    end

    # ── CWD reporting ────────────────────────────────────────────────
    function _cmux_report_pwd
        _cmux_send_fire_forget "report_pwd \"$PWD\" "(_cmux_flags)
    end

    # ── Fast git HEAD resolution ─────────────────────────────────────
    # Reads .git/HEAD directly without invoking `git` for speed on large repos.
    # Handles regular repos and worktrees (.git as file with gitdir pointer).
    function _cmux_git_resolve_head_path
        set -l dir "$PWD"
        while test "$dir" != "/"
            if test -f "$dir/.git/HEAD"
                echo "$dir/.git/HEAD"
                return 0
            else if test -f "$dir/.git"
                # Worktree: .git is a file containing "gitdir: <path>"
                set -l gitdir (string replace 'gitdir: ' '' < "$dir/.git")
                if not string match -q '/*' -- "$gitdir"
                    set gitdir "$dir/$gitdir"
                end
                if test -f "$gitdir/HEAD"
                    echo "$gitdir/HEAD"
                    return 0
                end
            end
            set dir (dirname "$dir")
        end
        return 1
    end

    # Read branch name from a HEAD file without forking git.
    function _cmux_git_read_branch_from_head
        set -l head_file "$argv[1]"
        test -f "$head_file"; or return 1
        set -l content (cat "$head_file" 2>/dev/null); or return 1
        # "ref: refs/heads/<branch>"
        if string match -q 'ref: refs/heads/*' -- "$content"
            string replace 'ref: refs/heads/' '' -- "$content"
            return 0
        end
        # Detached HEAD — short hash
        string sub -l 8 -- "$content"
        return 0
    end

    # ── Git branch (fast path + fallback, async with throttle) ───────
    set -g _cmux_git_branch ""
    set -g _cmux_git_dirty ""
    set -g _cmux_git_head_path ""
    set -g _cmux_git_last_report 0

    # Core git branch detection — runs synchronously (used by async wrapper).
    function _cmux_update_git_branch_sync
        if test -z "$_cmux_git_head_path"; or not test -f "$_cmux_git_head_path"
            set -g _cmux_git_head_path (_cmux_git_resolve_head_path 2>/dev/null)
        end

        set -l branch ""
        if test -n "$_cmux_git_head_path"
            set branch (_cmux_git_read_branch_from_head "$_cmux_git_head_path")
        end

        if test -z "$branch"
            set branch (git symbolic-ref --short HEAD 2>/dev/null; or git rev-parse --short HEAD 2>/dev/null)
        end

        if test -n "$branch"
            set -g _cmux_git_branch "$branch"
            if git diff-index --quiet HEAD -- 2>/dev/null
                set -g _cmux_git_dirty ""
            else
                set -g _cmux_git_dirty "*"
            end
            _cmux_send "report_git_branch $branch$_cmux_git_dirty "(_cmux_flags) >/dev/null 2>&1
        else if test -n "$_cmux_git_branch"
            set -g _cmux_git_branch ""
            set -g _cmux_git_dirty ""
            set -g _cmux_git_head_path ""
            _cmux_send "clear_git_branch "(_cmux_flags) >/dev/null 2>&1
        end
    end

    # Async wrapper — runs in background, throttled to max once per 3 seconds.
    function _cmux_update_git_branch
        set -l now (date +%s)
        if test (math "$now - $_cmux_git_last_report") -lt 3
            return
        end
        set -g _cmux_git_last_report $now
        _cmux_update_git_branch_sync >/dev/null 2>&1 &
        disown 2>/dev/null
    end

    # ── Async git HEAD watcher ────────────────────────────────────────
    # While a command runs, poll .git/HEAD every 2s so branch switches
    # (during git rebase / checkout) are reflected immediately.
    set -g _cmux_git_watcher_pid ""

    # Background loop — runs as a disowned fish job, retaining access to the
    # _cmux_send / _cmux_flags helpers defined above.
    function _cmux_git_watcher_loop
        set -l head_file "$argv[1]"
        set -l last_head (cat "$head_file" 2>/dev/null)
        while true
            sleep 2
            set -l cur_head (cat "$head_file" 2>/dev/null)
            if test "$cur_head" != "$last_head"
                set last_head "$cur_head"
                if string match -q 'ref: refs/heads/*' -- "$cur_head"
                    set -l branch (string replace 'ref: refs/heads/' '' -- "$cur_head")
                    test -n "$branch"; and _cmux_send "report_git_branch $branch "(_cmux_flags) >/dev/null 2>&1
                end
            end
        end
    end

    function _cmux_start_git_watcher
        set -l head_file "$_cmux_git_head_path"
        if test -z "$head_file"
            set -l git_dir (git rev-parse --git-dir 2>/dev/null); or return
            set head_file "$git_dir/HEAD"
        end
        test -f "$head_file"; or return

        _cmux_git_watcher_loop "$head_file" >/dev/null 2>&1 &
        set -g _cmux_git_watcher_pid $last_pid
        disown 2>/dev/null
    end

    function _cmux_stop_git_watcher
        if test -n "$_cmux_git_watcher_pid"
            kill "$_cmux_git_watcher_pid" 2>/dev/null
            set -g _cmux_git_watcher_pid ""
        end
    end

    # ── PR status polling (background, every 45s) ────────────────────
    set -g _cmux_pr_poll_pid ""
    set -g _cmux_pr_last_status ""

    function _cmux_pr_poll_loop
        while true
            sleep 45
            # Skip PR lookup on main/master — they don't have associated PRs
            set -l current_branch (git rev-parse --abbrev-ref HEAD 2>/dev/null)
            if test "$current_branch" = "main"; or test "$current_branch" = "master"
                continue
            end
            if command -q gh; and git rev-parse --is-inside-work-tree >/dev/null 2>&1
                set -l pr_output (timeout 20 gh pr view --json state,statusCheckRollup 2>&1)
                set -l pr_exit $status
                if test $pr_exit -eq 0; and test -n "$pr_output"
                    set -l pr_state (string match -r '"state":"([^"]*)"' -- "$pr_output")[2]
                    if test -n "$pr_state"
                        set -g _cmux_pr_last_status "$pr_state"
                        _cmux_send "report_pr $pr_state "(_cmux_flags) >/dev/null 2>&1
                    end
                else if string match -q '*no pull requests*' -- "$pr_output"
                    if test -n "$_cmux_pr_last_status"
                        set -g _cmux_pr_last_status ""
                        _cmux_send "clear_pr "(_cmux_flags) >/dev/null 2>&1
                    end
                end
                # Transient failure — preserve last known status
            end
        end
    end

    function _cmux_start_pr_poll
        test -n "$_cmux_pr_poll_pid"; and kill "$_cmux_pr_poll_pid" 2>/dev/null
        _cmux_pr_poll_loop >/dev/null 2>&1 &
        set -g _cmux_pr_poll_pid $last_pid
        disown 2>/dev/null
    end

    # ── TTY reporting ────────────────────────────────────────────────
    function _cmux_report_tty
        set -l tty_name (tty 2>/dev/null)
        test -n "$tty_name"; and _cmux_send_fire_forget "report_tty $tty_name "(_cmux_flags)
    end

    # ── Agent session capture ────────────────────────────────────────
    # Wrap `claude` so every launch pins a known session id and reports it to
    # cmux, letting a restored tab resume that exact conversation (`claude
    # --resume <id>`) instead of the directory-level `--continue`. Skips
    # injection when the user already selects a session.
    if command -v claude >/dev/null 2>&1
        function claude
            for arg in $argv
                switch $arg
                    case -r --resume '--resume=*' -c --continue --session-id '--session-id=*' --fork-session
                        command claude $argv
                        return
                end
            end
            set -l _cmux_sid (cat /proc/sys/kernel/random/uuid 2>/dev/null; or uuidgen 2>/dev/null | tr '[:upper:]' '[:lower:]')
            if test -n "$_cmux_sid"
                _cmux_send_fire_forget "report_agent_session claude $_cmux_sid "(_cmux_flags)
                command claude --session-id "$_cmux_sid" $argv
            else
                command claude $argv
            end
        end
    end

    # ── Port scanning kick ──────────────────────────────────────────
    function _cmux_ports_kick
        _cmux_send_fire_forget "ports_kick"
    end

    # ── Shell state reporting ────────────────────────────────────────
    function _cmux_report_prompt
        _cmux_send_fire_forget "report_shell_state prompt "(_cmux_flags)
    end

    function _cmux_report_running
        _cmux_send_fire_forget "report_shell_state running "(_cmux_flags)
    end

    # ── Semantic prompt markers (OSC 133) ─────────────────────────────
    function _cmux_osc133_prompt_start
        printf '\e]133;A;redraw=last;cl=line\a'
    end

    function _cmux_osc133_command_start
        printf '\e]133;C\a'
    end

    function _cmux_osc133_command_end
        printf '\e]133;D;%s\a' "$argv[1]"
    end

    # ── Process cleanup on shell exit ────────────────────────────────
    function _cmux_cleanup --on-event fish_exit
        _cmux_stop_git_watcher
        if test -n "$_cmux_pr_poll_pid"
            kill "$_cmux_pr_poll_pid" 2>/dev/null
            set -g _cmux_pr_poll_pid ""
        end
    end

    # ── Invalidate cached HEAD path when CWD changes ─────────────────
    function _cmux_on_pwd --on-variable PWD
        set -g _cmux_git_head_path ""
        _cmux_report_pwd
        _cmux_update_git_branch
    end

    # ── Hook into fish command lifecycle ─────────────────────────────
    function _cmux_preexec --on-event fish_preexec
        _cmux_osc133_command_start
        _cmux_report_running
        _cmux_start_git_watcher
    end

    function _cmux_postexec --on-event fish_postexec
        set -l exit_code $status
        _cmux_stop_git_watcher
        _cmux_osc133_command_end "$exit_code"
        _cmux_report_pwd
        _cmux_update_git_branch
        _cmux_report_prompt
        _cmux_osc133_prompt_start
    end

    # ── Initial reports ──────────────────────────────────────────────
    _cmux_restore_scrollback_once
    _cmux_report_pwd
    _cmux_report_tty
    _cmux_update_git_branch
    _cmux_ports_kick
    _cmux_osc133_prompt_start

    # Start PR polling if gh is available
    if command -q gh
        _cmux_start_pr_poll
    end
end
