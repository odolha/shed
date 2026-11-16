#!/usr/bin/env bash
# Takes the docs' screenshots into docs/images, from a fresh demo world, on a
# virtual X display of its own (Xvfb) with software Vulkan: nothing shows on
# your screen, and your mouse and keyboard are left alone.
# Runs a separate Shed (its own runtime, home and config), so your own Shed is
# left alone too. Run from the repo root after `cargo build`:
#   tools/screenshots/take.sh
# Needs Xvfb (XVFB=/path/to/Xvfb when it is not on PATH; without root,
# `apt-get download xvfb && dpkg-deb -x xvfb_*.deb dir` gives one), xrdb,
# xwininfo, xwd, ffmpeg, and python3 with PIL.
set -euo pipefail
repo=$(pwd)
demo=${DEMO:-/tmp/shed-demo}
shed=$repo/target/debug/shed
display=${SHOT_DISPLAY:-:93}
xvfb=${XVFB:-Xvfb}

# A demo Shed left from an earlier run stops first, while its socket still exists.
SHED_RUNTIME_DIR=$demo/run "$shed" stop >/dev/null 2>&1 || true
"$repo/tools/screenshots/demo-world.sh" "$demo" >/dev/null
export HOME=$demo/home SHED_RUNTIME_DIR=$demo/run XDG_CONFIG_HOME=$demo/xdg/config XDG_DATA_HOME=$demo/xdg/data XDG_STATE_HOME=$demo/xdg/state

# The display: large enough for every window side by side (the demo places
# them; there is no window manager), at twice the usual density.
"$xvfb" "$display" -screen 0 7680x4320x24 -nolisten tcp >"$demo/xvfb.log" 2>&1 &
xvfb_pid=$!
trap '"$shed" stop >/dev/null 2>&1 || true; kill $xvfb_pid 2>/dev/null || true' EXIT
export DISPLAY=$display
unset WAYLAND_DISPLAY
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
echo "Xft.dpi: 192" | xrdb -merge
[ -f /usr/share/vulkan/icd.d/lvp_icd.json ] && export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
# xtype.py may give windows focus itself: this display is ours alone.
export SHED_SHOTS_DISPLAY=$display

win() { for _ in $(seq 100); do w=$(xwininfo -root -tree | grep -F "\"$1\"" | head -1 | awk '{print $1}'); [ -n "$w" ] && { echo "$w"; return; }; sleep 0.2; done; echo "no window: $1" >&2; exit 1; }
grab() { xwd -silent -id "$1" > "$demo/$2.xwd" && ffmpeg -loglevel error -y -i "$demo/$2.xwd" "$demo/$2.png" && rm "$demo/$2.xwd"; }
xtype() { python3 "$repo/tools/screenshots/xtype.py" "$@"; }

# Every bench runs once, so closed ones have scrollback to preview.
(setsid "$shed" >"$demo/app.log" 2>&1 &)
sleep 5
for b in board api infra docs; do "$shed" load $b --start; sleep 2; done
sleep 8
"$shed" stop >/dev/null; sleep 1

# Then as one finds it: three benches open, and the Shed window.
(setsid "$shed" >"$demo/app.log" 2>&1 &)
sleep 5
for b in board api docs; do "$shed" load $b --start; sleep 3; done
sleep 12
shed_win=$(win Shed)
grab "$(win 'Shed — board')" bench
grab "$(win 'Shed — api')" project
docs_win=$(win 'Shed — docs')
grab "$docs_win" ai
# The review of what Claude changed (Ctrl+Shift+R at the AI tab's prompt).
xtype "$docs_win" %0.3 @Control_L+Shift_L+r %3 && grab "$docs_win" review && xtype "$docs_win" @Down @Down %0.8 && grab "$docs_win" review-commit && xtype "$docs_win" @Escape %0.5 || true
"$shed" hide docs >/dev/null; sleep 4
grab "$shed_win" shed
echo 'theme = "shed-light"' > "$XDG_CONFIG_HOME/shed/shed.toml"; sleep 3
grab "$shed_win" shed-light
echo 'theme = "shed-dark"' > "$XDG_CONFIG_HOME/shed/shed.toml"; sleep 3

# Panels, by keys sent to this display only (the mouse does not reach GPUI
# here: Xvfb has no XInput pointer of its own).
board=$(win 'Shed — board')
xtype "$board" %0.4 @Control_L+Shift_L+s %1.5 && grab "$board" setup && xtype "$board" @Control_L+Shift_L+s %0.5 || true
xtype "$board" %0.3 @Control_L+Shift_L+f %0.4 boards %1.2 && grab "$board" find && xtype "$board" @Escape || true
xtype "$board" %0.3 @Alt_L+Right %0.4 @Control_L+r %1.5 && grab "$board" logbook && xtype "$board" @Escape || true

mkdir -p docs/images
python3 - "$demo" <<'PY'
import os, sys
from PIL import Image
demo = sys.argv[1]
def scaled(im, factor):
    return im.resize((int(im.width * factor), int(im.height * factor)), Image.LANCZOS)
# (colours.png and tab-menu.png need right-clicks, which do not reach GPUI on
# Xvfb: they stay as they were.)
for name in ["shed", "bench", "shed-light", "project", "ai", "review", "setup", "logbook", "find"]:
    path = f"{demo}/{name}.png"
    if os.path.exists(path):
        im = Image.open(path).convert("RGB")
        scaled(im, 1280 / im.width).save(f"docs/images/{name}.png", optimize=True)
PY
echo "screenshots in docs/images (demo in $demo)"
