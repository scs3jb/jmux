#!/usr/bin/env bash
# Headless, privacy-safe screenshot capture for jmux.
# sway runs once; jmux is relaunched fresh per scene so modals/panes never
# accumulate. All panes run AUTHORED demo scripts — no login shell, no real
# paths/usernames.
set -uo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
JMUX_APP="$REPO/target/release/jmux-app"; CLI="$REPO/jmux/bin/jmux"
SB=/tmp/jmux-demo; RT=/tmp/jmux-demo-rt; SHOTS="$REPO/docs/screenshots"
JMUX_PID=""; SWAY_PID=""
cleanup(){ [ -n "$JMUX_PID" ]&&kill "$JMUX_PID" 2>/dev/null; [ -n "$SWAY_PID" ]&&kill "$SWAY_PID" 2>/dev/null; sleep 1; [ -n "$SWAY_PID" ]&&kill -9 "$SWAY_PID" 2>/dev/null; }
trap cleanup EXIT
rm -rf "$SB" "$RT"; mkdir -p "$SB/.config/jmux" "$SB/.config/ghostty" "$SB/.config/gtk-4.0" "$SB/bin" "$RT"
for d in "" web api docs; do mkdir -p "$SB/$d/.jmux"; done; chmod 700 "$RT"

# ── match the user's look & feel (theme prefs only; no private data) ──
printf '{"theme":"dark"}' > "$SB/.config/jmux/settings.json"
mkdir -p "$SB/.config/ghostty/themes"
cp "/usr/share/ghostty/themes/Adventure Time" "$SB/.config/ghostty/themes/Adventure Time" 2>/dev/null
printf 'theme = Adventure Time\n' > "$SB/.config/ghostty/config"
cat > "$SB/.config/gtk-4.0/settings.ini" <<'INI'
[Settings]
gtk-application-prefer-dark-theme=true
gtk-font-name=Noto Sans,  10
gtk-icon-theme-name=breeze-dark
INI

demo_script(){ printf '#!/bin/bash\nprintf "\\033]0;jmux\\007"\nclear\ncat <<'"'"'TXT'"'"'\n%s\nTXT\nsleep 100000\n' "$2" > "$SB/bin/$1.sh"; chmod +x "$SB/bin/$1.sh"; }
demo_script claude '  Claude Code · web
  ────────────────────────────────
  > add JWT validation to the auth module

  ● Editing src/auth.rs
    + fn validate_jwt(token: &str) -> Result<Claims> {
    +     decode::<Claims>(token, &key, &Validation::default())
    + }

  ● Running tests … 14 passed, 0 failed
  Waiting for your next instruction…'
demo_script codex '  Codex · api
  ────────────────────────────────
  > implement the request rate limiter

  $ cargo build   (Finished dev in 2.4s)
  $ cargo test    (8 tests … ok)
  Applied 3 edits across 2 files.'
demo_script dev '  npm run dev
  ────────────────────────────────
  VITE v5.0  ready in 312 ms
  ➜  Local:   http://localhost:3000/
  10:42:18 [vite] hmr update /src/App.tsx'

# ── agent-state demo panes: each body+title triggers a distinct Claude octopus
# state via jmux/src/model/claude_state.rs — a braille-spinner title (or an "esc
# to interrupt" footer) = working; a ❯ numbered menu = needs input; "N shell(s)
# still running" = waiting. Custom title (not "jmux"), so it's read as an agent.
agent_script(){ printf '#!/bin/bash\nprintf "\\033]0;%s\\007"\nclear\ncat <<'"'"'TXT'"'"'\n%s\nTXT\nsleep 100000\n' "$2" "$3" > "$SB/bin/$1.sh"; chmod +x "$SB/bin/$1.sh"; }
agent_script oct-working '⠹ claude' '  Claude Code · auth
  ────────────────────────────────
  > add JWT validation to the auth module

  ● Editing src/auth.rs
    + fn validate_jwt(token: &str) -> Result<Claims> {

  ✳ Working… (1m 8s · esc to interrupt)'
agent_script oct-needs 'claude' '  Claude Code · api
  ────────────────────────────────
  Which migration strategy should I use?

  ❯ 1. Online — zero downtime, slower
    2. Offline — fast, needs a maintenance window'
agent_script oct-waiting 'claude' '  Claude Code · docs
  ────────────────────────────────
  ● Ran npm run build in the background

  ⎿ 1 shell still running (↓ to manage)
  > '

cat > "$SB/.jmux/jmux.json" <<EOF
{ "commands": [
  { "name": "demo", "workspace": { "name": "web", "cwd": "$SB/web", "color": "#3b82f6",
      "layout": { "direction": "horizontal", "split": 0.55, "children": [
        { "pane": { "surfaces": [ { "command": "bash $SB/bin/claude.sh", "focus": true } ] } },
        { "pane": { "surfaces": [ { "command": "bash $SB/bin/codex.sh" } ] } } ] } } },
  { "name": "api-ws",  "workspace": { "name": "api",  "cwd": "$SB/api",
      "layout": { "pane": { "surfaces": [ { "command": "bash $SB/bin/codex.sh", "focus": true } ] } } } },
  { "name": "docs-ws", "workspace": { "name": "docs", "cwd": "$SB/docs",
      "layout": { "pane": { "surfaces": [ { "command": "bash $SB/bin/dev.sh", "focus": true } ] } } } },
  { "name": "octw", "workspace": { "name": "auth", "cwd": "$SB/web",  "color": "#265ca8",
      "layout": { "pane": { "surfaces": [ { "command": "bash $SB/bin/oct-working.sh", "focus": true } ] } } } },
  { "name": "octa", "workspace": { "name": "api",  "cwd": "$SB/api",  "color": "#e49626",
      "layout": { "pane": { "surfaces": [ { "command": "bash $SB/bin/oct-needs.sh", "focus": true } ] } } } },
  { "name": "octd", "workspace": { "name": "docs", "cwd": "$SB/docs", "color": "#208084",
      "layout": { "pane": { "surfaces": [ { "command": "bash $SB/bin/oct-waiting.sh", "focus": true } ] } } } }
] }
EOF
for d in web api docs; do cp "$SB/.jmux/jmux.json" "$SB/$d/.jmux/jmux.json"; done
mk(){ local d="$SB/.claude/projects/$1"; mkdir -p "$d"
  printf '{"type":"summary","cwd":"%s"}\n{"message":{"role":"user","content":"%s"},"cwd":"%s"}\n' "$2" "$3" "$2" > "$d/$(uuidgen).jsonl"; sleep 0.05; }
mk "-home-demo-web" "/home/demo/web" "Refactor the auth module to use JWT tokens"
mk "-home-demo-web" "/home/demo/web" "Add unit tests for the login flow"
mk "-home-demo-api" "/home/demo/api" "Fix the rate-limiter off-by-one bug"
mk "-home-demo-api" "/home/demo/api" "Set up the CI release pipeline"
cat > "$SB/.config/jmux/dock.json" <<EOF
{ "controls": [
  { "id": "clock", "title": "Clock", "command": "watch -n1 -t date '+%H:%M:%S'" },
  { "id": "build", "title": "Build watcher", "command": "while true; do echo '[ok] build passed'; sleep 4; done" } ] }
EOF
magick -size 720x960 xc:white -gravity North -pointsize 34 -fill '#222' -annotate +0+90 "Sample Document" \
  -pointsize 20 -fill '#555' -annotate +0+180 "Rendered inline by the Finder file-preview\npanel via poppler (pdftocairo)." "$SB/docs/sample.pdf" 2>/dev/null

# ── sway (once) ──
printf 'output HEADLESS-1 resolution 1600x1000\ndefault_border none\nfocus_follows_mouse no\n' > "$SB/sway.conf"
HOME="$SB" XDG_RUNTIME_DIR="$RT" WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 sway -c "$SB/sway.conf" >/tmp/sway.log 2>&1 &
SWAY_PID=$!; WD=""; for i in $(seq 1 60); do WD=$(ls "$RT"/wayland-* 2>/dev/null|grep -v '\.lock$'|head -1); [ -n "$WD" ]&&break; sleep 0.25; done
[ -z "$WD" ]&&{ echo "sway failed"; cat /tmp/sway.log; exit 1; }; WD=$(basename "$WD"); echo "wayland: $WD"
SOCK="$RT/jmux.sock"
run(){ env JMUX_SOCKET="$SOCK" HOME="$SB" bash "$CLI" "$@" >/dev/null 2>&1; }
wsid(){ env JMUX_SOCKET="$SOCK" HOME="$SB" bash "$CLI" list 2>/dev/null | python3 -c "import sys,json
try: d=json.load(sys.stdin)
except: sys.exit()
for w in d['result']['workspaces']:
    if '$1' in w.get('directory','') or '$1'==w.get('title',''): print(w['id']); break"; }
wsid_exact(){ env JMUX_SOCKET="$SOCK" HOME="$SB" bash "$CLI" list 2>/dev/null | python3 -c "import sys,json
try: d=json.load(sys.stdin)
except: sys.exit()
for w in d['result']['workspaces']:
    if w.get('directory','')=='$1': print(w['id']); break"; }

launch(){ env -i HOME="$SB" XDG_RUNTIME_DIR="$RT" WAYLAND_DISPLAY="$WD" PATH=/usr/bin:/usr/local/bin SHELL=/bin/bash TERM=xterm-256color \
    XDG_DATA_DIRS=/usr/local/share:/usr/share LANG=C.UTF-8 JMUX_DISABLE_SESSION_RESTORE=1 GDK_BACKEND=wayland "$JMUX_APP" >/tmp/jmux.log 2>&1 &
  JMUX_PID=$!; for i in $(seq 1 80); do [ -S "$SOCK" ]&&break; sleep 0.25; done; sleep 2.5; }
kjmux(){ [ -n "$JMUX_PID" ]&&kill "$JMUX_PID" 2>/dev/null; JMUX_PID=""; for i in $(seq 1 20); do [ -S "$SOCK" ]||break; sleep 0.2; done; rm -f "$SOCK"; sleep 0.4; }
stage(){ # build demo workspaces, drop the default, focus web
  run run docs-ws; sleep 1.0; run run api-ws; sleep 1.0; run run demo; sleep 2.4
  local ids; ids=$(env JMUX_SOCKET="$SOCK" HOME="$SB" bash "$CLI" list 2>/dev/null | python3 -c "import sys,json
try: d=json.load(sys.stdin)
except: sys.exit()
for w in d['result']['workspaces']:
    if w.get('title') not in ('web','api','docs'): print(w['id'])")
  for id in $ids; do run close "$id"; sleep 0.5; done; sleep 0.3
  local WEB; WEB=$(wsid "/web"); [ -n "$WEB" ]&&{ run select "$WEB"; sleep 1.0; }; }
grab(){ WAYLAND_DISPLAY="$WD" XDG_RUNTIME_DIR="$RT" grim "$SHOTS/$1.png"&&echo "  ✓ $1"; }

scene(){ # name  trigger-cmd...
  local name="$1"; shift; launch; stage
  [ $# -gt 0 ]&&{ run "$@"; sleep 1.6; }
  grab "$name"; kjmux; }

echo "capturing (fresh jmux per scene)…"
scene hero
scene pane-overview  overview
scene command-palette palette
scene dock           dock
scene vault-pane     vault
scene task-manager   top
scene pdf-preview    open "$SB/docs/sample.pdf"
# history needs a closed workspace
launch; stage; run clear-closed; sleep 0.5; run run api-ws; sleep 1.0; run close; sleep 0.7; run history; sleep 1.4; grab history-pane; kjmux
# sidebar octopus — three workspaces each in a distinct Claude state, so the
# animated deck sprites render bottom-right of every workspace row. Cropped to
# just the sidebar column (tweak the crop geometry if the sidebar width changes).
launch
run run octw; sleep 1.0; run run octa; sleep 1.0; run run octd; sleep 1.2
oct_ids=$(env JMUX_SOCKET="$SOCK" HOME="$SB" bash "$CLI" list 2>/dev/null | python3 -c "import sys,json
try: d=json.load(sys.stdin)
except: sys.exit()
for w in d['result']['workspaces']:
    if w.get('title') not in ('auth','api','docs'): print(w['id'])")
for id in $oct_ids; do run close "$id"; sleep 0.4; done
OW=$(wsid_exact "$SB/web"); [ -n "$OW" ]&&{ run select "$OW"; sleep 0.6; }
sleep 3.2   # let each pane paint so the state classifier sees the agent output
WAYLAND_DISPLAY="$WD" XDG_RUNTIME_DIR="$RT" grim "$SB/oct-full.png"
magick "$SB/oct-full.png" -crop 282x300+0+86 +repage "$SHOTS/sidebar-octopus.png" 2>/dev/null && echo "  ✓ sidebar-octopus"
kjmux
echo done
