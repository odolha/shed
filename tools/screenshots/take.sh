#!/usr/bin/env bash
# Takes the docs' screenshots into docs/images, from a fresh demo world.
# INTERACTIVE=1 also takes menus and panels (it clicks and types, so leave the
# mouse and keyboard alone while it runs).
# Needs X11, wmctrl and ffmpeg, and runs a separate Shed (its own runtime,
# home and config), so your own Shed is left alone. Run from the repo root
# after `cargo build`:  tools/screenshots/take.sh
set -euo pipefail
repo=$(pwd)
demo=${DEMO:-/tmp/shed-demo}
shed=$repo/target/debug/shed
# A demo Shed left from an earlier run stops first, while its socket still exists.
SHED_RUNTIME_DIR=$demo/run "$shed" stop >/dev/null 2>&1 || true
"$repo/tools/screenshots/demo-world.sh" "$demo" >/dev/null
export HOME=$demo/home SHED_RUNTIME_DIR=$demo/run XDG_CONFIG_HOME=$demo/xdg/config XDG_DATA_HOME=$demo/xdg/data XDG_STATE_HOME=$demo/xdg/state

# Windows of this Shed only, by owning process.
mine() { for p in $(pgrep -x shed); do tr '\0' '\n' < /proc/$p/environ 2>/dev/null | grep -qx "SHED_RUNTIME_DIR=$SHED_RUNTIME_DIR" && echo $p; done; }
win() { for _ in $(seq 50); do for p in $(mine); do w=$(wmctrl -lp | awk -v pid=$p '$3==pid' | grep -F "$1" | head -1 | awk '{print $1}'); [ -n "$w" ] && { echo $w; return; }; done; sleep 0.2; done; echo "no window: $1" >&2; exit 1; }
grab() { ffmpeg -loglevel error -y -f x11grab -draw_mouse 0 -window_id "$1" -i :0 -frames:v 1 "$demo/$2.png"; }

# Every bench runs once, so closed ones have scrollback to preview.
(setsid "$shed" >"$demo/app.log" 2>&1 &)
sleep 4
for b in board api infra docs; do "$shed" load $b; sleep 1.5; done
sleep 6
"$shed" stop >/dev/null; sleep 1

# Then as one finds it: two benches running, one of them hidden.
(setsid "$shed" >"$demo/app.log" 2>&1 &)
sleep 4
"$shed" load board; sleep 2
"$shed" load api; sleep 2
wmctrl -i -c "$(win 'api —')"
sleep 8
shed_win=$(win Shed)
wmctrl -i -r "$shed_win" -e 0,200,40,2240,1760; sleep 1.5
grab "$shed_win" shed
grab "$(win 'board —')" bench
echo 'theme = "shed-light"' > "$XDG_CONFIG_HOME/shed/shed.toml"; sleep 2.5
grab "$shed_win" shed-light
echo 'theme = "shed-dark"' > "$XDG_CONFIG_HOME/shed/shed.toml"; sleep 2

# With INTERACTIVE=1: menus and panels, which need clicks and keys. They go
# through xtype.py, which stops if the window it types into loses focus.
if [ "${INTERACTIVE:-}" = 1 ]; then
  xtype() { local w=$1; shift; wmctrl -i -R "$w"; sleep 0.3; python3 "$repo/tools/screenshots/xtype.py" "$w" "$@"; }
  board=$(win 'board —')
  xtype "$board" %0.4 @Control_L+Shift_L+s %1.2 && grab "$board" setup && xtype "$board" @Control_L+Shift_L+s %0.5 || true
  xtype "$board" %0.3 ~1500,700 '#1' %0.4 @Control_L+r %1.2 && grab "$board" logbook && xtype "$board" @Escape || true
  xtype "$board" %0.3 ~500,700 '#1' %0.3 @Control_L+Shift_L+f %0.4 boards %1.0 && grab "$board" find && xtype "$board" @Escape || true
  xtype "$shed_win" %0.4 ~114,765 '!1' '^1' %0.8 && grab "$shed_win" palette && xtype "$shed_win" @Escape ~1200,60 '#1' || true
  xtype "$shed_win" %0.4 ~350,980 '#3' %0.8 && grab "$shed_win" tabmenu && xtype "$shed_win" ~1200,60 '#1' || true
fi

mkdir -p docs/images
python3 - "$demo" <<'PY'
import sys
from PIL import Image
demo = sys.argv[1]
import os
def scaled(im, factor):
    return im.resize((int(im.width * factor), int(im.height * factor)), Image.LANCZOS)
for name in ["shed", "bench", "shed-light", "setup", "logbook", "find"]:
    if os.path.exists(f"{demo}/{name}.png"):
        im = Image.open(f"{demo}/{name}.png")
        scaled(im, 1280 / im.width).save(f"docs/images/{name}.png", optimize=True)
# Menus: the part around them, at two thirds.
for name, out, box in [("palette", "colours", (40, 720, 1000, 1010)), ("tabmenu", "tab-menu", (40, 780, 1060, 1400))]:
    if os.path.exists(f"{demo}/{name}.png"):
        scaled(Image.open(f"{demo}/{name}.png").crop(box), 0.66).save(f"docs/images/{out}.png", optimize=True)
PY
echo "screenshots in docs/images; the demo Shed is still running (SHED_RUNTIME_DIR=$SHED_RUNTIME_DIR)"
