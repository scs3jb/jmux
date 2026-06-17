#!/bin/bash
# Generate the README demo GIFs (docs/demos/{drag-tabs,move-panes}.gif) headlessly.
#
# How it works: a virtual-pointer Wayland client (vptr.c, built from the embedded
# wlr-virtual-pointer protocol) injects real pointer enter/motion/button events into
# a headless sway, so GTK drag-and-drop actually fires. grim captures frames during
# the drag; ffmpeg assembles them. Nothing touches your real input devices or screen.
#
# Prereqs: sway, grim, ffmpeg, gcc, wayland-scanner, pkg-config(wayland-client),
#          a release build, and the demo sandbox that docs/autocapture.sh creates
#          (/tmp/cmux-demo with .cmux/cmux.json + bin/*.sh demo scripts).
# Run docs/autocapture.sh first (it builds the sandbox + theme), then this.
set -e
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SB=/tmp/cmux-demo

# 1. build the virtual-pointer driver
cd "$HERE"
wayland-scanner client-header wlr-virtual-pointer-unstable-v1.xml /tmp/vptr-hdr.h
wayland-scanner private-code  wlr-virtual-pointer-unstable-v1.xml /tmp/vptr-proto.c
cp /tmp/vptr-hdr.h wlr-virtual-pointer-unstable-v1-client-protocol.h
cc -O2 -o /tmp/vptr vptr.c /tmp/vptr-proto.c $(pkg-config --cflags --libs wayland-client)

# 2. add a 3-tab demo command + distinct-titled tab scripts to the sandbox
for spec in "claude|Claude Code|> refactor the auth module|● Editing src/auth.rs|● 14 passed, 0 failed" \
            "server|Server log|GET /api/users 200 12ms|POST /api/login 200 41ms|GET /api/feed 200 8ms" \
            "tests|Test runner|RUNS  auth.test.ts|PASS  auth.test.ts (14)|PASS  feed.test.ts (9)"; do
  IFS='|' read -r name title l1 l2 l3 <<<"$spec"
  cat > "$SB/bin/tab-$name.sh" <<EOF
#!/bin/bash
printf "\033]0;$name\007"; clear
cat <<'TXT'
  $title
  ────────────────────────────────
  $l1
  $l2
  $l3
TXT
sleep 100000
EOF
done
python3 - <<'PY'
import json, glob
cmd = {"name":"tabsdemo","workspace":{"name":"work","cwd":"/tmp/cmux-demo/web","color":"#926EE4",
  "layout":{"pane":{"surfaces":[
    {"command":"bash /tmp/cmux-demo/bin/tab-claude.sh","focus":True},
    {"command":"bash /tmp/cmux-demo/bin/tab-server.sh"},
    {"command":"bash /tmp/cmux-demo/bin/tab-tests.sh"}]}}}}
for p in glob.glob('/tmp/cmux-demo/**/.cmux/cmux.json', recursive=True):
    d = json.load(open(p)); d['commands'] = [c for c in d.get('commands',[]) if c.get('name')!='tabsdemo'] + [cmd]
    json.dump(d, open(p,'w'), indent=2)
PY

# 3. harness (sway + cmux + helpers)
source "$HERE/harness.sh"
VP(){ env XDG_RUNTIME_DIR="$RT" WAYLAND_DISPLAY="$WD" /tmp/vptr; }

# linear drag generator: press at (x1,y1), step to (x2,y2), release; $5 = post-release ms
gen_drag(){ local x1=$1 y1=$2 x2=$3 y2=$4 tail=${5:-400} steps=22 i
  echo "m $x1 $y1"; echo "w 400"; echo "d"; echo "w 150"
  for i in $(seq 1 $steps); do echo "m $(( x1+(x2-x1)*i/steps )) $(( y1+(y2-y1)*i/steps ))"; echo "w 40"; done
  echo "w 450"; echo "u"; echo "w $tail"; echo "q"; }

capture(){ local dir=$1 script=$2   # run drag in bg, grab frames until it ends
  rm -rf "$dir"; mkdir -p "$dir"
  VP < "$script" & local vp=$!; local i=0
  while kill -0 $vp 2>/dev/null && [ $i -lt 300 ]; do G "$dir/f$(printf %03d $i).png" 2>/dev/null; i=$((i+1)); done
  wait $vp; }

mkgif(){ local dir=$1 out=$2 crop=$3   # two-pass palette, hold first/last frames
  ffmpeg -y -framerate 24 -i "$dir/f%03d.png" \
    -vf "crop=$crop,scale=880:-1:flags=lanczos,tpad=start_duration=0.4:start_mode=clone:stop_duration=1.4:stop_mode=clone,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3" \
    "$out"; }

start_sway; start_cmux

# Scene 1: reorder tabs — drag the 'claude' tab from first to last
stage_only tabsdemo
gen_drag 335 66 600 66 400 > /tmp/drag-tabs.txt
capture /tmp/frames-tabs /tmp/drag-tabs.txt
mkgif /tmp/frames-tabs "$REPO/docs/demos/drag-tabs.gif" "1320:360:285:48"

# Scene 2: move panes — drag a tab across into the other pane (2 panes -> 1 pane, 2 tabs)
stage_only demo
gen_drag 325 66 1300 430 1600 > /tmp/drag-panes.txt
capture /tmp/frames-panes /tmp/drag-panes.txt
mkgif /tmp/frames-panes "$REPO/docs/demos/move-panes.gif" "1315:440:285:48"

stop
echo "wrote docs/demos/drag-tabs.gif and docs/demos/move-panes.gif"
echo "REVIEW both before committing — confirm no real username/host/path is visible."
