#!/usr/bin/env zsh
# jmux zsh integration — CWD reporting, git branch, PR polling, port kicks,
# scrollback restoration, async git HEAD watcher, semantic prompt markers,
# fast git HEAD resolution, smart PR polling, WINCH guard, process cleanup.
#
# Sourced automatically via ZDOTDIR bootstrap (.zshenv) when JMUX_SOCKET is
# set (injected by jmux into terminal environment). Can also be sourced
# manually from ~/.zshrc.
#
# Protocol: V1 text lines over the jmux Unix socket.

# Bail if not running inside jmux
[[ -n "$JMUX_SOCKET" ]] || return 0

# Guard against double-sourcing
[[ -n "$_JMUX_ZSH_INTEGRATION_LOADED" ]] && return 0
_JMUX_ZSH_INTEGRATION_LOADED=1

# ── Socket transport ──────────────────────────────────────────────────
# Detect which socket client is available once at load time to avoid
# running `command -v` on every send (reduces prompt latency).
_jmux_send_cmd=""
if command -v ncat >/dev/null 2>&1; then
  _jmux_send_cmd="ncat"
elif command -v socat >/dev/null 2>&1; then
  _jmux_send_cmd="socat"
elif command -v nc >/dev/null 2>&1; then
  _jmux_send_cmd="nc"
fi

_jmux_send() {
  local msg="$1"
  case "$_jmux_send_cmd" in
    ncat)  echo "$msg" | ncat  -U "$JMUX_SOCKET" 2>/dev/null ;;
    socat) echo "$msg" | socat - UNIX-CONNECT:"$JMUX_SOCKET" 2>/dev/null ;;
    nc)    echo "$msg" | nc    -U "$JMUX_SOCKET" -w 1 2>/dev/null ;;
  esac
}

_jmux_send_fire_forget() {
  ( exec >/dev/null 2>&1; set +x; setopt NO_XTRACE NO_VERBOSE; _jmux_send "$1" ) &!
}

# ── Workspace / panel identifiers ────────────────────────────────────
_jmux_flags() {
  local flags=""
  [[ -n "$JMUX_WORKSPACE_ID" ]] && flags="$flags --tab=$JMUX_WORKSPACE_ID"
  [[ -n "$JMUX_PANEL_ID" ]]     && flags="$flags --panel=$JMUX_PANEL_ID"
  echo "$flags"
}

# ── Scrollback restoration ────────────────────────────────────────────
# On session restore jmux writes saved scrollback to a temp file and sets
# JMUX_RESTORE_SCROLLBACK_FILE. We replay it once, then delete the file.
_jmux_restore_scrollback_once() {
  [[ -z "$JMUX_RESTORE_SCROLLBACK_FILE" ]] && return
  local f="$JMUX_RESTORE_SCROLLBACK_FILE"
  unset JMUX_RESTORE_SCROLLBACK_FILE
  if [[ -f "$f" ]]; then
    cat "$f" 2>/dev/null
    rm -f "$f" 2>/dev/null
  fi
}

# ── CWD reporting ────────────────────────────────────────────────────
_jmux_report_pwd() {
  _jmux_send_fire_forget "report_pwd \"$PWD\" $(_jmux_flags)"
}

# ── Fast git HEAD resolution ─────────────────────────────────────────
# Reads .git/HEAD directly without invoking `git` for speed on large repos.
# Handles both regular repos and git worktrees (.git as file with gitdir pointer).
_jmux_git_resolve_head_path() {
  local dir="$PWD"
  while [[ "$dir" != "/" ]]; do
    if [[ -f "$dir/.git/HEAD" ]]; then
      echo "$dir/.git/HEAD"
      return 0
    elif [[ -f "$dir/.git" ]]; then
      # Worktree: .git is a file containing "gitdir: <path>"
      local gitdir
      gitdir=$(< "$dir/.git")
      gitdir="${gitdir#gitdir: }"
      # Resolve relative paths
      [[ "$gitdir" != /* ]] && gitdir="$dir/$gitdir"
      if [[ -f "$gitdir/HEAD" ]]; then
        echo "$gitdir/HEAD"
        return 0
      fi
    fi
    dir="${dir:h}"
  done
  return 1
}

# Read branch name from a HEAD file without forking git.
_jmux_git_read_branch_from_head() {
  local head_file="$1"
  [[ -f "$head_file" ]] || return 1
  local content
  content=$(< "$head_file" 2>/dev/null) || return 1
  # "ref: refs/heads/<branch>"
  if [[ "$content" == ref:\ refs/heads/* ]]; then
    echo "${content#ref: refs/heads/}"
    return 0
  fi
  # Detached HEAD — short hash
  echo "${content:0:8}"
  return 0
}

# ── Git branch (fast path + fallback, async with throttle) ───────────
_jmux_git_branch=""
_jmux_git_dirty=""
_jmux_git_head_path=""
_jmux_git_last_report=0

# Core git branch detection — runs synchronously (used by async wrapper).
_jmux_update_git_branch_sync() {
  # Redirect all output to prevent TUI corruption from background writes
  exec >/dev/null 2>&1
  set +x 2>/dev/null
  setopt NO_XTRACE NO_VERBOSE 2>/dev/null
  # Try fast path first (no fork)
  if [[ -z "$_jmux_git_head_path" ]] || [[ ! -f "$_jmux_git_head_path" ]]; then
    _jmux_git_head_path=$(_jmux_git_resolve_head_path 2>/dev/null)
  fi

  local branch=""
  if [[ -n "$_jmux_git_head_path" ]]; then
    branch=$(_jmux_git_read_branch_from_head "$_jmux_git_head_path")
  fi

  # Fallback to git if fast path fails
  if [[ -z "$branch" ]]; then
    branch=$(git symbolic-ref --short HEAD 2>/dev/null \
             || git rev-parse --short HEAD 2>/dev/null)
  fi

  if [[ -n "$branch" ]]; then
    _jmux_git_branch="$branch"
    # Quick dirty check (index only, skip untracked for speed)
    if git diff-index --quiet HEAD -- 2>/dev/null; then
      _jmux_git_dirty=""
    else
      _jmux_git_dirty="*"
    fi
    _jmux_send "report_git_branch ${branch}${_jmux_git_dirty} $(_jmux_flags)" \
      >/dev/null 2>&1
  elif [[ -n "$_jmux_git_branch" ]]; then
    _jmux_git_branch=""
    _jmux_git_dirty=""
    _jmux_git_head_path=""
    _jmux_send "clear_git_branch $(_jmux_flags)" >/dev/null 2>&1
  fi
}

# Async wrapper — runs in background, throttled to max once per 3 seconds.
# The fast path (reading .git/HEAD directly) is cheap enough to run inline,
# but the dirty check (git diff-index) can block on large repos, so we
# background the entire update when throttle allows.
_jmux_update_git_branch() {
  local now=$EPOCHSECONDS
  [[ -z "$now" ]] && now=$(date +%s)
  if (( now - _jmux_git_last_report < 3 )); then
    return
  fi
  _jmux_git_last_report=$now

  # Run in background subshell so precmd doesn't block
  _jmux_update_git_branch_sync &!
}

# Invalidate cached HEAD path when CWD changes
_jmux_chpwd() {
  _jmux_git_head_path=""
}
autoload -Uz add-zsh-hook
add-zsh-hook chpwd _jmux_chpwd

# ── Async git HEAD watcher ────────────────────────────────────────────
# While a command is running, poll .git/HEAD every 2s so branch switches
# (e.g. during `git rebase` or `git checkout`) are reflected immediately.
_jmux_git_watcher_pid=""

_jmux_start_git_watcher() {
  local head_file="$_jmux_git_head_path"
  if [[ -z "$head_file" ]]; then
    local git_dir
    git_dir=$(git rev-parse --git-dir 2>/dev/null) || return
    head_file="$git_dir/HEAD"
  fi
  [[ -f "$head_file" ]] || return

  (
    # Redirect all output to /dev/null FIRST to prevent any background
    # writes from corrupting TUI apps (Claude Code, vim, etc.)
    exec >/dev/null 2>&1
    set +x 2>/dev/null
    setopt NO_XTRACE NO_VERBOSE 2>/dev/null
    local last_head
    last_head=$(< "$head_file" 2>/dev/null)
    while true; do
      sleep 2
      local cur_head
      cur_head=$(< "$head_file" 2>/dev/null)
      if [[ "$cur_head" != "$last_head" ]]; then
        last_head="$cur_head"
        local branch="${cur_head#ref: refs/heads/}"
        if [[ "$branch" != "$cur_head" && -n "$branch" ]]; then
          _jmux_send "report_git_branch $branch $(_jmux_flags)" >/dev/null 2>&1
        fi
      fi
    done
  ) &!
  _jmux_git_watcher_pid=$!
}

_jmux_stop_git_watcher() {
  if [[ -n "$_jmux_git_watcher_pid" ]]; then
    kill "$_jmux_git_watcher_pid" 2>/dev/null
    _jmux_git_watcher_pid=""
  fi
}

# ── PR status polling (background, every 45s) ────────────────────────
_jmux_pr_poll_pid=""
_jmux_pr_last_status=""

# Extract owner/repo from git remote for smarter PR queries
_jmux_github_repo_slug() {
  local remote_url
  remote_url=$(git remote get-url origin 2>/dev/null) || return 1
  # Handle SSH: git@github.com:owner/repo.git
  if [[ "$remote_url" == git@github.com:* ]]; then
    local slug="${remote_url#git@github.com:}"
    echo "${slug%.git}"
    return 0
  fi
  # Handle HTTPS: https://github.com/owner/repo.git
  if [[ "$remote_url" == https://github.com/* ]]; then
    local slug="${remote_url#https://github.com/}"
    echo "${slug%.git}"
    return 0
  fi
  return 1
}

# Detect whether gh output indicates "no PR" vs transient failure
_jmux_pr_output_indicates_no_pr() {
  local output="$1" exit_code="$2"
  # gh exits 1 with "no pull requests found" when there's genuinely no PR
  [[ "$exit_code" -ne 0 ]] && [[ "$output" == *"no pull requests"* ]] && return 0
  return 1
}

_jmux_start_pr_poll() {
  [[ -n "$_jmux_pr_poll_pid" ]] && kill "$_jmux_pr_poll_pid" 2>/dev/null

  (
    # Redirect all output and suppress tracing to prevent TUI corruption
    exec >/dev/null 2>&1
    set +x 2>/dev/null
    setopt NO_XTRACE NO_VERBOSE 2>/dev/null
    while true; do
      sleep 45
      # Skip PR lookup on main/master — they don't have associated PRs
      current_branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null)
      if [[ "$current_branch" == "main" || "$current_branch" == "master" ]]; then
        continue
      fi
      if command -v gh >/dev/null 2>&1 \
         && git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        # No `local` — already in a subshell so no scope leak
        pr_output=$(timeout 20 gh pr view --json state,statusCheckRollup 2>&1)
        pr_exit=$?
        if [[ "$pr_exit" -eq 0 && -n "$pr_output" ]]; then
          # Extract "state" value using zsh native expansion (avoids subprocess)
          local _tmp="${pr_output#*\"state\":\"}"
          pr_state="${_tmp%%\"*}"
          [[ "$pr_state" == "$pr_output" ]] && pr_state=""
          if [[ -n "$pr_state" ]]; then
            _jmux_pr_last_status="$pr_state"
            _jmux_send "report_pr $pr_state $(_jmux_flags)" >/dev/null 2>&1
          fi
          # Parse individual check results from statusCheckRollup
          if command -v python3 >/dev/null 2>&1; then
            checks_json=$(python3 -c '
import json, sys
try:
    data = json.loads(sys.stdin.read())
    rollup = data.get("statusCheckRollup", []) or []
    checks = []
    for c in rollup[:20]:
        name = c.get("name") or c.get("context", "")
        conclusion = (c.get("conclusion") or c.get("state") or "PENDING").upper()
        if name:
            checks.append({"name": name, "conclusion": conclusion})
    print(json.dumps(checks))
except:
    pass
' <<< "$pr_output" 2>/dev/null)
            if [[ -n "$checks_json" && "$checks_json" != "[]" ]]; then
              _jmux_send "report_pr_checks $checks_json $(_jmux_flags)" >/dev/null 2>&1
            fi
          fi
        elif _jmux_pr_output_indicates_no_pr "$pr_output" "$pr_exit"; then
          # Genuinely no PR — clear badge
          if [[ -n "$_jmux_pr_last_status" ]]; then
            _jmux_pr_last_status=""
            _jmux_send "clear_pr $(_jmux_flags)" >/dev/null 2>&1
          fi
        fi
        # Transient failure — preserve last known status
      fi
    done
  ) &!
  _jmux_pr_poll_pid=$!
}

# ── TTY reporting ────────────────────────────────────────────────────
_jmux_report_tty() {
  local tty_name
  tty_name=$(tty 2>/dev/null)
  [[ -n "$tty_name" ]] && _jmux_send_fire_forget "report_tty $tty_name $(_jmux_flags)"
}

# ── Agent session capture ────────────────────────────────────────────
# Wrap `claude` so every launch pins a known session id and reports it to jmux,
# letting a restored tab resume that exact conversation (`claude --resume <id>`)
# instead of the directory-level `--continue`. Skips injection when the user
# already selects a session, so explicit resumes pass through untouched.
if command -v claude >/dev/null 2>&1; then
  claude() {
    local arg inject=1
    for arg in "$@"; do
      case "$arg" in
        -r|--resume|--resume=*|-c|--continue|--session-id|--session-id=*|--fork-session)
          inject=0
          break
          ;;
      esac
    done
    if [[ "$inject" == 1 ]]; then
      local _jmux_sid
      _jmux_sid=$(cat /proc/sys/kernel/random/uuid 2>/dev/null) \
        || _jmux_sid=$(uuidgen 2>/dev/null | tr '[:upper:]' '[:lower:]')
      if [[ -n "$_jmux_sid" ]]; then
        _jmux_send_fire_forget "report_agent_session claude $_jmux_sid $(_jmux_flags)"
        command claude --session-id "$_jmux_sid" "$@"
        return
      fi
    fi
    command claude "$@"
  }
fi

# ── Port scanning kick ──────────────────────────────────────────────
_jmux_ports_kick() {
  _jmux_send_fire_forget "ports_kick"
}

# ── Shell state reporting ────────────────────────────────────────────
_jmux_report_prompt() {
  _jmux_send_fire_forget "report_shell_state prompt $(_jmux_flags)"
}

_jmux_report_running() {
  _jmux_send_fire_forget "report_shell_state running $(_jmux_flags)"
}

# ── Semantic prompt markers (OSC 133) ─────────────────────────────────
_jmux_osc133_prompt_start() {
  printf '\e]133;A;redraw=last;cl=line\a'
}

_jmux_osc133_command_start() {
  printf '\e]133;C\a'
}

_jmux_osc133_command_end() {
  printf '\e]133;D;%s\a' "$1"
}

# ── Prompt wrap guard ────────────────────────────────────────────────
# If a command ran for ≥2 seconds and the prompt would wrap, print a
# spacer line so that resize-triggered prompt redraw doesn't overwrite
# command output.
_jmux_cmd_start_time=0

_jmux_prompt_wrap_guard() {
  local now=$EPOCHSECONDS
  [[ -z "$now" ]] && now=$(date +%s)
  local elapsed=$(( now - _jmux_cmd_start_time ))
  if (( elapsed >= 2 )); then
    # Check if the prompt wraps (wider than terminal)
    local prompt_len=${#${(%%)PS1}}
    if (( prompt_len >= COLUMNS )); then
      print ""
    fi
  fi
}

# ── WINCH signal guard ───────────────────────────────────────────────
# Install a WINCH trap to prevent prompt corruption on terminal resize.
# Without this, zsh may redraw the prompt mid-resize causing glitches.
TRAPWINCH() {
  # No-op: absorb the signal to prevent zle redraw during resize.
  # Ghostty handles resize internally.
  :
}

# ── Process tree cleanup ─────────────────────────────────────────────
# Recursively collect all descendant PIDs of a given PID.
_jmux_child_pids() {
  local parent="$1"
  local children
  children=$(pgrep -P "$parent" 2>/dev/null) || return
  local pid
  for pid in ${=children}; do
    echo "$pid"
    _jmux_child_pids "$pid"
  done
}

# Kill a process and all its descendants (leaf-first).
_jmux_kill_process_tree() {
  local pid="$1"
  local descendants
  descendants=($(_jmux_child_pids "$pid"))
  # Kill children first (reverse order for leaf-first)
  local i
  for (( i=${#descendants[@]}; i>=1; i-- )); do
    kill "${descendants[$i]}" 2>/dev/null
  done
  kill "$pid" 2>/dev/null
}

# Kill background child processes (git watcher, PR poll) on shell exit.
_jmux_cleanup() {
  _jmux_stop_git_watcher
  if [[ -n "$_jmux_pr_poll_pid" ]]; then
    _jmux_kill_process_tree "$_jmux_pr_poll_pid"
    _jmux_pr_poll_pid=""
  fi
}
add-zsh-hook zshexit _jmux_cleanup

# ── Hook into zsh prompt lifecycle ───────────────────────────────────
_jmux_last_exit=0

_jmux_precmd() {
  local exit_code=$?
  _jmux_last_exit=$exit_code

  # Stop git watcher — command finished, prompt is back
  _jmux_stop_git_watcher

  # Semantic: mark end of previous command output
  _jmux_osc133_command_end "$exit_code"

  # Report state to jmux
  _jmux_report_pwd
  _jmux_update_git_branch
  _jmux_report_prompt

  # Guard against prompt-wrap overwriting output after long commands
  _jmux_prompt_wrap_guard

  # Semantic: mark start of prompt
  _jmux_osc133_prompt_start
}

_jmux_preexec() {
  # Record command start time for wrap guard
  _jmux_cmd_start_time=$EPOCHSECONDS
  [[ -z "$_jmux_cmd_start_time" ]] && _jmux_cmd_start_time=$(date +%s)

  # Semantic: mark start of command output
  _jmux_osc133_command_start

  _jmux_report_running

  # Start async git HEAD watcher while command runs
  _jmux_start_git_watcher
}

# Register hooks (idempotent — won't double-register)
add-zsh-hook precmd  _jmux_precmd
add-zsh-hook preexec _jmux_preexec

# ── Initial reports ──────────────────────────────────────────────────
_jmux_restore_scrollback_once
_jmux_report_pwd
_jmux_report_tty
_jmux_update_git_branch
_jmux_ports_kick

# Start PR polling if gh is available
if command -v gh >/dev/null 2>&1; then
  _jmux_start_pr_poll
fi
