#!/bin/bash
# Generate the README demo GIFs headlessly:
#   docs/demos/{drag-tabs,move-panes,pane-overview,command-palette}.gif
#
# How it works: small Wayland clients inject *real* input into a headless sway so
# GTK reacts exactly as for a human — vptr (wlr-virtual-pointer) drives drags and
# clicks, vkbd (zwp_virtual_keyboard) types into the palette. grim captures frames
# during each interaction; ffmpeg assembles them. Nothing touches your real input
# devices or screen.
#
# Prereqs: sway, grim, ffmpeg, gcc, wayland-scanner, pkg-config (wayland-client,
#          xkbcommon), a release build, and the demo sandbox that docs/autocapture.sh
#          creates (/tmp/jmux-demo with .jmux/jmux.json + bin/*.sh demo scripts).
# Run docs/autocapture.sh first (sandbox + theme), then this.
set -e
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SB=/tmp/jmux-demo
BIN=/tmp/vptr; mkdir -p "$BIN"

# ---- 1. build the virtual-pointer and virtual-keyboard drivers -----------------
cd "$HERE"
wayland-scanner client-header wlr-virtual-pointer-unstable-v1.xml wlr-virtual-pointer-unstable-v1-client-protocol.h
wayland-scanner private-code  wlr-virtual-pointer-unstable-v1.xml /tmp/vptr-proto.c
cc -O2 -o "$BIN/vptr" vptr.c /tmp/vptr-proto.c $(pkg-config --cflags --libs wayland-client)
wayland-scanner client-header virtual-keyboard-unstable-v1.xml virtual-keyboard-unstable-v1-client-protocol.h
wayland-scanner private-code  virtual-keyboard-unstable-v1.xml /tmp/vkbd-proto.c
cc -O2 -o "$BIN/vkbd" vkbd.c /tmp/vkbd-proto.c $(pkg-config --cflags --libs wayland-client xkbcommon)

# ---- 2. add demo commands/scripts (3-tab "tabsdemo" + 4-pane "quad") -----------
mk_script(){ # name title l1 l2 l3
  cat > "$SB/bin/$1.sh" <<EOF
#!/bin/bash
printf "\033]0;$1\007"; clear
cat <<'TXT'
  $2
  ────────────────────────────────
  $3
  $4
  $5
TXT
sleep 100000
EOF
}
mk_script tab-claude "Claude Code" "> refactor the auth module" "● Editing src/auth.rs"   "● 14 passed, 0 failed"
mk_script tab-server "Server log"  "GET /api/users 200 12ms"    "POST /api/login 200 41ms" "GET /api/feed 200 8ms"
mk_script tab-tests  "Test runner" "RUNS  auth.test.ts"         "PASS  auth.test.ts (14)"  "PASS  feed.test.ts (9)"
mk_script q-agent  "Claude Code"  "> implement pagination"     "● Editing src/list.rs"    "● 9 passed, 0 failed"
mk_script q-build  "Build watcher" "webpack compiled successfully" "2400 modules in 1.8s"  "watching for changes…"
mk_script q-logs   "Server log"   "GET /api/users 200 12ms"    "POST /api/login 200 41ms" "WARN slow query 320ms"
mk_script q-deploy "Deploy"       "\$ fly deploy --remote"     "==> building image …"     "release v128 succeeded"
python3 - <<'PY'
import json, glob
tabs = {"name":"tabsdemo","workspace":{"name":"work","cwd":"/tmp/jmux-demo/web","color":"#926EE4",
  "layout":{"pane":{"surfaces":[
    {"command":"bash /tmp/jmux-demo/bin/tab-claude.sh","focus":True},
    {"command":"bash /tmp/jmux-demo/bin/tab-server.sh"},
    {"command":"bash /tmp/jmux-demo/bin/tab-tests.sh"}]}}}}
quad = {"name":"quad","workspace":{"name":"fleet","cwd":"/tmp/jmux-demo/web","color":"#3b82f6",
  "layout":{"direction":"horizontal","split":0.5,"children":[
    {"direction":"vertical","split":0.5,"children":[
      {"pane":{"surfaces":[{"command":"bash /tmp/jmux-demo/bin/q-agent.sh","focus":True}]}},
      {"pane":{"surfaces":[{"command":"bash /tmp/jmux-demo/bin/q-build.sh"}]}}]},
    {"direction":"vertical","split":0.5,"children":[
      {"pane":{"surfaces":[{"command":"bash /tmp/jmux-demo/bin/q-logs.sh"}]}},
      {"pane":{"surfaces":[{"command":"bash /tmp/jmux-demo/bin/q-deploy.sh"}]}}]}]}}}
for p in glob.glob('/tmp/jmux-demo/**/.jmux/jmux.json', recursive=True):
    d = json.load(open(p)); names = {'tabsdemo','quad'}
    d['commands'] = [c for c in d.get('commands',[]) if c.get('name') not in names] + [tabs, quad]
    json.dump(d, open(p,'w'), indent=2)
PY

# ---- 3. harness + capture helpers ----------------------------------------------
source "$HERE/harness.sh"            # start_sway/start_jmux, R, G, VP, stage_only, stage_several
VKBD(){ env XDG_RUNTIME_DIR="$RT" WAYLAND_DISPLAY="$WD" "$BIN/vkbd"; }

gen_drag(){ local x1=$1 y1=$2 x2=$3 y2=$4 tail=${5:-400} steps=22 i
  echo "m $x1 $y1"; echo "w 400"; echo "d"; echo "w 150"
  for i in $(seq 1 $steps); do echo "m $(( x1+(x2-x1)*i/steps )) $(( y1+(y2-y1)*i/steps ))"; echo "w 40"; done
  echo "w 450"; echo "u"; echo "w $tail"; echo "q"; }

capture(){ local dir=$1 driver=$2 script=$3   # run driver in bg, grab frames until it ends
  rm -rf "$dir"; mkdir -p "$dir"
  "$driver" < "$script" & local pid=$!; local i=0
  while kill -0 $pid 2>/dev/null && [ $i -lt 300 ]; do G "$dir/f$(printf %03d $i).png" 2>/dev/null; i=$((i+1)); done
  wait $pid; }

mkgif(){ local dir=$1 out=$2 crop=$3 scale=$4 sd=${5:-0.5}
  ffmpeg -y -framerate 24 -i "$dir/f%03d.png" \
    -vf "crop=$crop,scale=$scale:-1:flags=lanczos,tpad=start_duration=$sd:start_mode=clone:stop_duration=1.4:stop_mode=clone,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3" \
    "$out"; }

start_sway; start_jmux
vp(){ capture "$1" "$BIN/vptr" "$2"; }   # pointer-driven scene
kb(){ capture "$1" "$BIN/vkbd" "$2"; }   # keyboard-driven scene

# Scene 1: reorder tabs — drag 'claude' from first to last
stage_only tabsdemo
gen_drag 335 66 600 66 400 > /tmp/s.txt; vp /tmp/frames-tabs /tmp/s.txt
mkgif /tmp/frames-tabs "$REPO/docs/demos/drag-tabs.gif" "1320:360:285:48" 860

# Scene 2: move panes — drag a tab across into the other pane (2 panes -> 1, 2 tabs)
stage_only demo
gen_drag 325 66 1300 430 1600 > /tmp/s.txt; vp /tmp/frames-panes /tmp/s.txt
mkgif /tmp/frames-panes "$REPO/docs/demos/move-panes.gif" "1315:440:285:48" 880

# Scene 3: pane overview — click header grid button, then click a tile to jump
stage_only quad
printf 'm 1383 22\nw 600\nd\nw 90\nu\nw 1000\nm 1092 383\nw 700\nd\nw 90\nu\nw 1100\nq\n' > /tmp/s.txt
vp /tmp/frames-ov /tmp/s.txt
mkgif /tmp/frames-ov "$REPO/docs/demos/pane-overview.gif" "1600:690:0:46" 960 0.6

# Scene 4: command palette — open, type to fuzzy-filter, Enter to switch
stage_several demo api-ws docs-ws quad
R palette >/dev/null 2>&1; sleep 1.3
# first key after focus is dropped, so warm up with a no-op backspace (k 14); Enter=28
printf 'k 14\nw 600\nt a\nw 300\nt p\nw 300\nt i\nw 700\nk 28\nw 1200\nq\n' > /tmp/s.txt
kb /tmp/frames-pal /tmp/s.txt
mkgif /tmp/frames-pal "$REPO/docs/demos/command-palette.gif" "1120:700:0:46" 840 0.7

# Scene 5: split a pane — drag a tab onto the pane's right edge to split it off
stage_only tabsdemo
gen_drag 335 66 1540 420 1500 > /tmp/s.txt; vp /tmp/frames-split /tmp/s.txt
mkgif /tmp/frames-split "$REPO/docs/demos/drag-split.gif" "1315:500:285:48" 880

# Scene 6: dock — click the header dock-toggle button; the controls column slides in
stage_only demo
printf 'm 1420 22\nw 800\nd\nw 90\nu\nw 2600\nq\n' > /tmp/s.txt
vp /tmp/frames-dock /tmp/s.txt
mkgif /tmp/frames-dock "$REPO/docs/demos/dock.gif" "1315:700:285:8" 940

# Scene 7: history — close a couple of workspaces, then reopen one from the history pane
ws_id(){ R list 2>/dev/null | python3 -c "import sys,json
for w in json.load(sys.stdin)['result']['workspaces']:
    if w.get('title')=='$1': print(w['id'])"; }
for c in demo api-ws docs-ws; do R run "$c" >/dev/null 2>&1; sleep 1.6; done
# drop the leaky default workspaces and CLEAR the closed-stack so they never show in history
leaky=$(R list 2>/dev/null | python3 -c "import sys,json
for w in json.load(sys.stdin)['result']['workspaces']:
    t=w.get('title','')
    if t=='Terminal' or t.startswith('@') or '/home/' in t: print(w['id'])")
for id in $leaky; do R close "$id" >/dev/null 2>&1; done; sleep 0.5
R clear-closed >/dev/null 2>&1; sleep 0.5
# close two clean workspaces so history lists them, keep single-pane 'docs' active
docs=$(ws_id docs)
for t in web api; do id=$(ws_id "$t"); [ -n "$id" ] && R close "$id" >/dev/null 2>&1; sleep 0.4; done
R select "$docs" >/dev/null 2>&1; sleep 0.8
R history >/dev/null 2>&1; sleep 1.3
printf 'm 994 178\nw 900\nd\nw 90\nu\nw 1600\nq\n' > /tmp/s.txt   # click the 'api' entry to reopen
vp /tmp/frames-hist /tmp/s.txt
mkgif /tmp/frames-hist "$REPO/docs/demos/history.gif" "1600:690:0:46" 960 0.7

stop
echo "wrote 7 GIFs to docs/demos/"
echo "REVIEW each (magick gif -coalesce) — confirm no real username/host/path is visible."
