#!/usr/bin/env bash
# Headless, privacy-safe screenshot capture for cmux-gtk.
# sway runs once; cmux is relaunched fresh per scene so modals/panes never
# accumulate. All panes run AUTHORED demo scripts — no login shell, no real
# paths/usernames.
set -uo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
CMUX_APP="$REPO/target/release/cmux-app"; CLI="$REPO/cmux/bin/cmux"
SB=/tmp/cmux-demo; RT=/tmp/cmux-demo-rt; SHOTS="$REPO/docs/screenshots"
CMUX_PID=""; SWAY_PID=""
cleanup(){ [ -n "$CMUX_PID" ]&&kill "$CMUX_PID" 2>/dev/null; [ -n "$SWAY_PID" ]&&kill "$SWAY_PID" 2>/dev/null; sleep 1; [ -n "$SWAY_PID" ]&&kill -9 "$SWAY_PID" 2>/dev/null; }
trap cleanup EXIT
rm -rf "$SB" "$RT"; mkdir -p "$SB/.config/cmux" "$SB/bin" "$SB/web" "$SB/api" "$SB/docs" "$RT"; chmod 700 "$RT"

demo_script(){ printf '#!/bin/bash\nprintf "\\033]0;cmux\\007"\nclear\ncat <<'"'"'TXT'"'"'\n%s\nTXT\nsleep 100000\n' "$2" > "$SB/bin/$1.sh"; chmod +x "$SB/bin/$1.sh"; }
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

cat > "$SB/.config/cmux/cmux.json" <<EOF
{ "commands": [
  { "name": "demo", "workspace": { "name": "web", "cwd": "$SB/web", "color": "#3b82f6",
      "layout": { "direction": "horizontal", "split": 0.55, "children": [
        { "pane": { "surfaces": [ { "command": "bash $SB/bin/claude.sh", "focus": true } ] } },
        { "pane": { "surfaces": [ { "command": "bash $SB/bin/codex.sh" } ] } } ] } } },
  { "name": "api-ws",  "workspace": { "name": "api",  "cwd": "$SB/api",
      "layout": { "pane": { "surfaces": [ { "command": "bash $SB/bin/codex.sh", "focus": true } ] } } } },
  { "name": "docs-ws", "workspace": { "name": "docs", "cwd": "$SB/docs",
      "layout": { "pane": { "surfaces": [ { "command": "bash $SB/bin/dev.sh", "focus": true } ] } } } }
] }
EOF
mk(){ local d="$SB/.claude/projects/$1"; mkdir -p "$d"
  printf '{"type":"summary","cwd":"%s"}\n{"message":{"role":"user","content":"%s"},"cwd":"%s"}\n' "$2" "$3" "$2" > "$d/$(uuidgen).jsonl"; sleep 0.05; }
mk "-home-demo-web" "/home/demo/web" "Refactor the auth module to use JWT tokens"
mk "-home-demo-web" "/home/demo/web" "Add unit tests for the login flow"
mk "-home-demo-api" "/home/demo/api" "Fix the rate-limiter off-by-one bug"
mk "-home-demo-api" "/home/demo/api" "Set up the CI release pipeline"
cat > "$SB/.config/cmux/dock.json" <<EOF
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
SOCK="$RT/cmux.sock"
run(){ env CMUX_SOCKET="$SOCK" HOME="$SB" bash "$CLI" "$@" >/dev/null 2>&1; }
wsid(){ env CMUX_SOCKET="$SOCK" HOME="$SB" bash "$CLI" list 2>/dev/null | python3 -c "import sys,json
try: d=json.load(sys.stdin)
except: sys.exit()
for w in d['result']['workspaces']:
    if '$1' in w.get('directory','') or '$1'==w.get('title',''): print(w['id']); break"; }
wsid_exact(){ env CMUX_SOCKET="$SOCK" HOME="$SB" bash "$CLI" list 2>/dev/null | python3 -c "import sys,json
try: d=json.load(sys.stdin)
except: sys.exit()
for w in d['result']['workspaces']:
    if w.get('directory','')=='$1': print(w['id']); break"; }

launch(){ env -i HOME="$SB" XDG_RUNTIME_DIR="$RT" WAYLAND_DISPLAY="$WD" PATH=/usr/bin:/usr/local/bin SHELL=/bin/bash TERM=xterm-256color \
    XDG_DATA_DIRS=/usr/local/share:/usr/share LANG=C.UTF-8 CMUX_DISABLE_SESSION_RESTORE=1 GDK_BACKEND=wayland "$CMUX_APP" >/tmp/cmux.log 2>&1 &
  CMUX_PID=$!; for i in $(seq 1 80); do [ -S "$SOCK" ]&&break; sleep 0.25; done; sleep 2.5; }
kcmux(){ [ -n "$CMUX_PID" ]&&kill "$CMUX_PID" 2>/dev/null; CMUX_PID=""; for i in $(seq 1 20); do [ -S "$SOCK" ]||break; sleep 0.2; done; rm -f "$SOCK"; sleep 0.4; }
stage(){ # build demo workspaces, drop the default, focus web
  run run docs-ws; sleep 1.0; run run api-ws; sleep 1.0; run run demo; sleep 2.4
  local ids; ids=$(env CMUX_SOCKET="$SOCK" HOME="$SB" bash "$CLI" list 2>/dev/null | python3 -c "import sys,json
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
  grab "$name"; kcmux; }

echo "capturing (fresh cmux per scene)…"
scene hero
scene pane-overview  overview
scene command-palette palette
scene dock           dock
scene vault-pane     vault
scene task-manager   top
scene pdf-preview    open "$SB/docs/sample.pdf"
# history needs a closed workspace
launch; stage; run clear-closed; sleep 0.5; run run api-ws; sleep 1.0; run close; sleep 0.7; run history; sleep 1.4; grab history-pane; kcmux
echo done
