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

# A stand-in for Claude Code, for the review's explanations.
cat > "$demo/claude" <<'PY'
#!/usr/bin/env python3
import json, re, sys, time
if "--input-format" in sys.argv:
    # The AI tab's Claude Code: a turn that leaves an agent working in the
    # background, builds the site, and sets a wake-up; then it waits.
    sid = sys.argv[sys.argv.index("--resume") + 1] if "--resume" in sys.argv else "demo"
    now = time.time()
    def ts(ago):
        return time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime(now - ago))
    def out(**o):
        print(json.dumps(o), flush=True)
    def call(tid, name, inp, ago, result, extra=None, parent=None):
        out(type="assistant", parent_tool_use_id=parent, timestamp=ts(ago), message={"id": "msg_" + tid, "role": "assistant", "model": "claude-opus-5-5", "content": [{"type": "tool_use", "id": tid, "name": name, "input": inp}]})
        if result is not None:
            out(type="user", parent_tool_use_id=parent, timestamp=ts(ago - 2), tool_use_result=extra, message={"role": "user", "content": [{"type": "tool_result", "tool_use_id": tid, "content": result}]})
    def say(text, ago, parent=None):
        out(type="assistant", parent_tool_use_id=parent, timestamp=ts(ago), message={"id": "msg_t%d" % ago, "role": "assistant", "model": "claude-opus-5-5", "content": [{"type": "text", "text": text}]})
    for line in sys.stdin:
        m = json.loads(line)
        if m.get("type") == "control_request":
            out(type="control_response", response={"subtype": "success", "request_id": m["request_id"], "response": {}})
        elif m.get("type") == "user":
            out(type="system", subtype="init", session_id=sid, model="claude-opus-5-5")
            say("I'll have an agent check every link in the guides while the site builds.", 250)
            call("toolu_agent", "Agent", {"description": "Check every link in the guides", "subagent_type": "Explore", "prompt": "Open every link in guides/*.md and report the ones that fail, with file and line.", "run_in_background": True}, 245,
                 [{"type": "text", "text": "Async agent launched successfully.\nagentId: a51c7e0d2b9f4e6a1"}], {"isAsync": True, "status": "async_launched", "agentId": "a51c7e0d2b9f4e6a1"})
            out(type="system", subtype="task_started", task_id="a51c7e0d2b9f4e6a1", tool_use_id="toolu_agent", description="Check every link in the guides", is_backgrounded=True, task_type="local_agent", subagent_type="Explore")
            call("toolu_s1", "Glob", {"pattern": "guides/*.md"}, 240, "guides/intro.md\nguides/setup.md", parent="toolu_agent")
            call("toolu_s2", "Grep", {"pattern": "https?://", "path": "guides"}, 230, "guides/setup.md:9: https://nodejs.org/en/download\nguides/intro.md:4: https://example.com/board", parent="toolu_agent")
            say("Two links. Trying each.", 220, parent="toolu_agent")
            call("toolu_s3", "Bash", {"command": "curl -sI https://example.com/board | head -1"}, 200, "HTTP/2 200", parent="toolu_agent")
            call("toolu_s4", "Bash", {"command": "curl -sI https://nodejs.org/en/download | head -1"}, 60, None, parent="toolu_agent")
            out(type="system", subtype="task_progress", task_id="a51c7e0d2b9f4e6a1", tool_use_id="toolu_agent", description="Running curl", usage={"tool_uses": 4, "total_tokens": 18400, "duration_ms": 190000})
            call("toolu_build", "Bash", {"command": "npm run build", "description": "Build the docs site", "run_in_background": True}, 120,
                 "Command running in background with ID: b2m8q4. Output is being written to: /tmp/claude/tasks/b2m8q4.output.", {"backgroundTaskId": "b2m8q4"})
            out(type="system", subtype="task_started", task_id="b2m8q4", tool_use_id="toolu_build", description="Build the docs site", is_backgrounded=True, task_type="local_bash")
            out(type="system", subtype="task_notification", task_id="b2m8q4", tool_use_id="toolu_build", status="completed", summary="Background command \"Build the docs site\" completed (exit code 0)")
            at = int(now) + 18 * 60
            call("toolu_wake", "ScheduleWakeup", {"delaySeconds": 1080, "reason": "read the link check's report", "prompt": "Read the agent's link report and fix the broken links."}, 30,
                 "Next wakeup scheduled (in 1080s).", {"scheduledFor": at * 1000, "clampedDelaySeconds": 1080})
            say("The site builds. The agent is still checking links in the background; I'll read its report in about 18 minutes, or as soon as it finishes.", 20)
            out(type="result", subtype="success", duration_ms=9000, total_cost_usd=0.42)
    sys.exit(0)
asked = sys.stdin.read()
why = {
    "guides/setup.md": "Walks a new contributor from cloning to a first change, in numbered steps, because the guide only said to run ./dev.sh. The troubleshooting part covers the first-run problems you asked for, kept short.",
    "guides/intro.md": "Says where to start reading, so the introduction leads into the setup guide.",
}
changes = re.findall(r"^## (\d+)\. (\S+) \((\w+)", asked, re.M)
def explain(path, state):
    if path == "guides/setup.md" and state == "committed":
        return "Adds the troubleshooting section you asked for: the two first-run problems seen most, each with how to tell and the command that fixes it."
    if path == "guides/setup.md" and state == "pushed":
        return "The first rewrite of the guide: from one line to numbered steps, from cloning to running the app."
    return why.get(path, "Part of the same change.")
files = [{"number": int(n), "explanation": explain(p, s)} for n, p, s in changes]
print(json.dumps({"type": "result", "result": "", "structured_output": {"files": files}}))
PY
chmod +x "$demo/claude"
export SHED_CLAUDE=$demo/claude

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
# Explanations come from a stand-in Claude Code (SHED_CLAUDE, set before Shed
# started), never a real one.
xtype "$docs_win" %0.3 @Control_L+Shift_L+r %3 && xtype "$docs_win" @Shift_L+e %4 && grab "$docs_win" review && xtype "$docs_win" @Down @Down %0.8 && grab "$docs_win" review-commit && xtype "$docs_win" s %0.8 && grab "$docs_win" review-side && xtype "$docs_win" s %0.3 \
  && printf 'theme = "shed-dark"\n[git]\ntree = true\n' > "$XDG_CONFIG_HOME/shed/shed.toml" && sleep 2.5 && grab "$docs_win" review-tree \
  && echo 'theme = "shed-dark"' > "$XDG_CONFIG_HOME/shed/shed.toml" && sleep 1.5 && xtype "$docs_win" @Escape %0.5 || true
# A prompt whose turn leaves an agent working in the background and a
# wake-up waiting (the stand-in Claude Code plays it).
xtype "$docs_win" %0.3 "Check the links in the guides, and build the site" %0.3 @Control_L+Return %5 && grab "$docs_win" ai-agents || true
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
for name in ["shed", "bench", "shed-light", "project", "ai", "ai-agents", "review", "setup", "logbook", "find"]:
    path = f"{demo}/{name}.png"
    if os.path.exists(path):
        im = Image.open(path).convert("RGB")
        scaled(im, 1280 / im.width).save(f"docs/images/{name}.png", optimize=True)
PY
echo "screenshots in docs/images (demo in $demo)"
