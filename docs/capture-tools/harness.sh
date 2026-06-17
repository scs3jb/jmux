#!/bin/bash
# Headless harness: starts sway + cmux, exposes R (cmux cli), G (grim), VP (virtual pointer).
# Usage: source this, then call stage_tabsdemo / stage_demo, then drive drags.
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SB=/tmp/cmux-demo; RT=/tmp/cmux-demo-rt; SOCK="$RT/cmux.sock"

start_sway() {
  pkill -9 -x cmux-app 2>/dev/null; pkill -9 -x sway 2>/dev/null; sleep 1.2
  printf 'output HEADLESS-1 resolution 1600x1000\ndefault_border none\n' > "$SB/sway.conf"
  HOME="$SB" XDG_RUNTIME_DIR="$RT" WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
    sway -c "$SB/sway.conf" >/tmp/sway.log 2>&1 &
  SP=$!
  for i in $(seq 1 40); do WD=$(ls "$RT"/wayland-* 2>/dev/null|grep -v lock|head -1); [ -n "$WD" ]&&break; sleep 0.25; done
  WD=$(basename "$WD"); export WD
}
start_cmux() {
  env -i HOME="$SB" XDG_RUNTIME_DIR="$RT" WAYLAND_DISPLAY="$WD" PATH=/usr/bin SHELL=/bin/bash \
    TERM=xterm-256color XDG_DATA_DIRS=/usr/share LANG=C.UTF-8 CMUX_DISABLE_SESSION_RESTORE=1 \
    "$REPO/target/release/cmux-app" >/tmp/cmux.log 2>&1 &
  CP=$!
  for i in $(seq 1 60); do [ -S "$SOCK" ]&&break; sleep 0.25; done; sleep 3
}
R(){ env CMUX_SOCKET="$SOCK" HOME="$SB" bash "$REPO/cmux/bin/cmux" "$@"; }
G(){ env XDG_RUNTIME_DIR="$RT" WAYLAND_DISPLAY="$WD" grim -o HEADLESS-1 "$1"; }
VP(){ env XDG_RUNTIME_DIR="$RT" WAYLAND_DISPLAY="$WD" /tmp/vptr/vptr; }
ids_all(){ R list 2>/dev/null | python3 -c "import sys,json;[print(w['id']) for w in json.load(sys.stdin)['result']['workspaces']]"; }
stop(){ kill ${CP:-0} ${SP:-0} 2>/dev/null; sleep 0.5; kill -9 ${SP:-0} 2>/dev/null; }

# run a command (resolved via the default workspace's dir), then close every
# other workspace so only the freshly-created one (with no leaky title) remains
stage_only() {
  local cmd="$1"
  R run "$cmd" >/dev/null 2>&1; sleep 2.5
  local keep; keep=$(ids_all | tail -1)
  for id in $(ids_all); do [ "$id" != "$keep" ] && R close "$id" >/dev/null 2>&1; done; sleep 0.8
  R select "$keep" >/dev/null 2>&1; sleep 1.5
}
