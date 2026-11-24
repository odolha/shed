#!/usr/bin/env bash
# Shed end to end, with real processes, apart from your own Shed: builds the
# binaries (the nexus with the web app built in), starts a nexus on 127.0.0.1,
# a keeper of its own (with a stand-in claude) and its link, pairs them with
# `shed nexus join`, then drives the web app in a headless Chrome: signing in
# with TOTP, the overview, a terminal, the logbook, a project's diff and file,
# an AI tab (and a file uploaded for it). Screenshots go to $E2E_DIR/shots. Cleans up after itself,
# stopping only what it started.
#
#   tools/e2e.sh              # E2E_DIR=/tmp/shed-e2e, port 7871, release build
#   E2E_PROFILE=debug tools/e2e.sh
#   E2E_KEEP=1 tools/e2e.sh   # leave the folder (logs, shots) for a look
#   E2E_TARGET=/some/dir tools/e2e.sh   # cargo target dir (default target/e2e)
#
# Needs: cargo, Node 22.22+/24.15+ (for the web app and the driver), git,
# python3 (the stand-in claude), Chrome or Chromium ($CHROME to choose).
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$PWD

E2E_DIR=${E2E_DIR:-/tmp/shed-e2e}
E2E_PORT=${E2E_PORT:-7871}
E2E_PROFILE=${E2E_PROFILE:-release}
# A target dir of its own: target/release may be a Shed someone runs.
E2E_TARGET=${E2E_TARGET:-$repo/target/e2e}
case "$E2E_DIR" in /tmp/*) ;; *) echo "E2E_DIR must be under /tmp (it is wiped)" >&2; exit 2 ;; esac

log() { printf '[e2e] %s\n' "$*"; }

node -e 'const [a, b] = process.versions.node.split(".").map(Number); process.exit(a > 24 || (a === 24 && b >= 15) || (a === 22 && b >= 22) ? 0 : 1)' \
  || { echo "Node 22.22+ or 24.15+ is needed on PATH (the web app's build)" >&2; exit 2; }

# ---- build
if [[ "$E2E_PROFILE" == release ]]; then flag=--release; else flag=; fi
log "building shed, shedd and shed-nexus with the web app ($E2E_PROFILE)"
SHED_NEXUS_WEB=1 CARGO_TARGET_DIR=$E2E_TARGET cargo build -q $flag -p shed-app -p shed-keeper -p shed-nexus
bin=$E2E_TARGET/$E2E_PROFILE

# ---- a world of its own
rm -rf "$E2E_DIR"
mkdir -p "$E2E_DIR"/{run,config/shed,data,state,nexus/data,home,bin,shots,chrome,chrome-setup}
export SHED_RUNTIME_DIR=$E2E_DIR/run XDG_CONFIG_HOME=$E2E_DIR/config XDG_DATA_HOME=$E2E_DIR/data \
       XDG_STATE_HOME=$E2E_DIR/state HOME=$E2E_DIR/home SHED_CLAUDE=$E2E_DIR/bin/claude
unset SHED_PANE SHED_TAB SHED_BENCH
# A keeper that stays without windows, as on a server.
printf '[keeper]\nstay = true\n' > "$XDG_CONFIG_HOME/shed/shed.toml"
cat > "$HOME/.bashrc" <<'RC'
PS1='you@shed:\w\$ '
RC
touch "$HOME/.sudo_as_admin_successful"
printf '[user]\n\tname = you\n\temail = you@example.com\n[init]\n\tdefaultBranch = main\n' > "$HOME/.gitconfig"

# A project with Git history and a change.
project=$HOME/projects/demo
mkdir -p "$project/src" "$project/docs"
printf '# Demo\n\nA project for the end-to-end test.\n' > "$project/README.md"
printf 'fn main() {\n    println!("hello");\n}\n' > "$project/src/main.rs"
printf 'crabs\n' > "$project/docs/🦀-emoji.md"
printf 'one\ntwo\n' > "$project/notes.txt"
git -C "$project" init -q
git -C "$project" add -A
git -C "$project" commit -qm "First version"
printf '\na line changed by the e2e test\n' >> "$project/README.md"

# A stand-in claude: answers the control protocol and plays a recorded turn.
cat > "$SHED_CLAUDE" <<PY
#!/usr/bin/env python3
import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if m.get("type") == "control_request":
        reply = {"commands": [], "models": []} if m["request"]["subtype"] == "initialize" else {}
        print(json.dumps({"type": "control_response", "response": {"subtype": "success", "request_id": m["request_id"], "response": reply}}), flush=True)
    elif m.get("type") == "user":
        for rec in open("$repo/crates/shed-ai/tests/streams/edit-and-bash.jsonl"):
            if '"control_response"' not in rec:
                print(rec.strip(), flush=True)
PY
chmod +x "$SHED_CLAUDE"

cat > "$E2E_DIR/nexus/nexus.toml" <<TOML
listen = "127.0.0.1:$E2E_PORT"
public_url = "http://localhost:$E2E_PORT"
data_dir = "$E2E_DIR/nexus/data"
trusted_proxy = false
TOML

# ---- start, and stop only what was started
nexus_pid=
keeper_pid=
cleanup() {
  local status=$?
  set +e
  if [[ -n "$keeper_pid" ]]; then
    "$bin/shed" stop >/dev/null 2>&1
    for _ in $(seq 50); do kill -0 "$keeper_pid" 2>/dev/null || break; sleep 0.1; done
    kill "$keeper_pid" 2>/dev/null
  fi
  [[ -n "$nexus_pid" ]] && kill "$nexus_pid" 2>/dev/null && wait "$nexus_pid" 2>/dev/null
  if [[ $status -ne 0 ]]; then
    log "failed; logs: $E2E_DIR/nexus.log, $E2E_DIR/shedd.log, $XDG_STATE_HOME/shed/link.log"
  elif [[ -z "${E2E_KEEP:-}" ]]; then
    # The screenshots stay; the rest goes.
    find "$E2E_DIR" -mindepth 1 -maxdepth 1 ! -name shots -exec rm -rf {} +
  fi
  exit $status
}
trap cleanup EXIT INT TERM

log "starting a nexus on 127.0.0.1:$E2E_PORT"
"$bin/shed-nexus" --config "$E2E_DIR/nexus/nexus.toml" > "$E2E_DIR/nexus.log" 2>&1 &
nexus_pid=$!
for i in $(seq 100); do
  curl -sf "http://127.0.0.1:$E2E_PORT/api/v1/status" >/dev/null && break
  kill -0 "$nexus_pid" 2>/dev/null || { cat "$E2E_DIR/nexus.log"; exit 1; }
  sleep 0.1
done

log "starting a keeper of its own"
"$bin/shedd" --foreground --persist > "$E2E_DIR/shedd.log" 2>&1 &
keeper_pid=$!
for i in $(seq 100); do
  [[ -S "$SHED_RUNTIME_DIR/shedd.sock" ]] && break
  kill -0 "$keeper_pid" 2>/dev/null || { cat "$E2E_DIR/shedd.log"; exit 1; }
  sleep 0.1
done

E2E_DIR=$E2E_DIR E2E_URL=http://localhost:$E2E_PORT E2E_SHED=$bin/shed \
E2E_NEXUS_DATA=$E2E_DIR/nexus/data E2E_PROJECT=$project \
  node tools/e2e/e2e.mjs
log "passed; screenshots in $E2E_DIR/shots"
