#!/usr/bin/env bash
# Builds the demo world the docs' screenshots show: a home with a neutral
# prompt, a few projects, and saved benches whose startup commands make
# realistic output. Usage: demo-world.sh <dir>   (wipes <dir> first)
set -euo pipefail
root=${1:?usage: demo-world.sh <dir>}
rm -rf "$root"
home=$root/home
mkdir -p "$home" "$root/xdg/config/shed" "$root/xdg/data/shed/benches" "$root/xdg/state" "$root/run"

# Ubuntu's bash says how to use sudo until this exists.
touch "$home/.sudo_as_admin_successful"
cat > "$home/.bashrc" <<'RC'
PS1='\[\e[1;32m\]you@shed\[\e[0m\]:\[\e[1;34m\]\w\[\e[0m\]\$ '
PROMPT_COMMAND='printf "\e]0;%s\a" "${PWD/#$HOME/\~}"'
alias ll='ls -la'
RC
echo 'theme = "shed-dark"' > "$root/xdg/config/shed/shed.toml"
printf 'gs = "git status --short"\n' > "$root/xdg/config/shed/aliases.toml"

# Projects.
web=$home/projects/board; api=$home/projects/api; infra=$home/projects/infra; docs=$home/projects/docs
mkdir -p "$web/src/components" "$web/public" "$api/src" "$infra/k8s" "$docs/guides"
touch "$web/package.json" "$web/vite.config.ts" "$web/src/main.ts" "$web/src/App.vue" "$web/src/components/"{Header,Sidebar,Board}.vue
touch "$api/Cargo.toml" "$api/src/"{main,routes,db}.rs "$infra/k8s/"{deploy,service,ingress}.yaml "$docs/guides/"{intro,setup}.md
cat > "$web/dev-server.sh" <<'SH'
#!/bin/bash
printf '\n  \e[1;36mdev server\e[0m ready in 412 ms\n\n  \e[32m➜\e[0m  Local:   \e[36mhttp://localhost:5173/\e[0m\n\n'
paths=(/ /assets/main.js /api/users /api/session /favicon.ico /api/boards /api/boards/7)
codes=(200 200 200 304 200 201 200)
while true; do
  i=$((RANDOM % 7))
  printf '\e[2m%s\e[0m  \e[32m%s\e[0m  %-18s \e[2m%3d ms\e[0m\n' "$(date +%H:%M:%S)" "${codes[$i]}" "${paths[$i]}" $((RANDOM % 90 + 3))
  sleep 1.2
done
SH
cat > "$infra/rollout.sh" <<'SH'
#!/bin/bash
printf 'Waiting for deployment "web" rollout to finish: 2 of 3 updated replicas are available...\n'
printf 'deployment "web" successfully rolled out\n'
SH
chmod +x "$infra/rollout.sh"
cat > "$api/watch.sh" <<'SH'
#!/bin/bash
while true; do
  clear
  printf '\e[1;32m   Compiling\e[0m api v0.4.0 (~/projects/api)\n\e[1;32m    Finished\e[0m test profile in 2.31s\n\e[1;32m     Running\e[0m unittests src/main.rs\n\nrunning 24 tests\n'
  for t in routes::list_users routes::create_board routes::auth_expired db::migrations db::pool_reuse; do printf 'test %-28s ... \e[32mok\e[0m\n' "$t"; done
  printf '\ntest result: \e[32mok\e[0m. 24 passed; 0 failed; 0 ignored; finished in 0.84s\n\n\e[2m[watching src/ for changes]\e[0m\n'
  sleep 30
done
SH
chmod +x "$web/dev-server.sh" "$api/watch.sh"
( cd "$web" && git init -q && git config user.email you@shed && git config user.name you &&
  for m in "Scaffold the app" "Add the board view" "Sidebar with projects" "Fix drag and drop on Firefox" "Dark mode"; do
    echo "$m" >> CHANGELOG; git add -A && git commit -qm "$m"; done )

uuid() { python3 -c 'import uuid; print(uuid.uuid4())'; }
bench() { # name color cwd tabs-toml
  cat > "$root/xdg/data/shed/benches/$1.toml" <<TOML
name = "$1"
id = "$(uuid)"
launcher = "$1"
color = "$2"
cwd = "$3"
saved = "2026-11-01T09:30:00Z"
active_tab = 0
$4
TOML
}
tab() { # name layout tags panes...   (tags: TOML inline array or empty)
  printf '\n[[tabs]]\nid = "%s"\nname = "%s"\nlayout = "%s"\nactive_pane = 0\n%s\n' "$(uuid)" "$1" "$2" "$3"
}
pane() { # cwd startup...   (ID, READY and AFTER from the environment, if set)
  local cwd=$1; shift
  local cmds=""; for c in "$@"; do cmds+="\"$c\", "; done
  printf '\n[[tabs.panes]]\nid = "%s"\ncwd = "%s"\nstartup = [%s]\n' "${ID:-$(uuid)}" "$cwd" "${cmds%, }"
  [ -n "${READY:-}" ] && printf 'ready_when = "%s"\n' "$READY"
  [ -n "${AFTER:-}" ] && printf 'after = ["%s"]\n' "$AFTER"
  return 0
}
# Code for the project tabs: the api with changes not committed yet.
cat > "$api/src/routes.rs" <<'RS'
use axum::{extract::Path, Json};

use crate::db::{Board, Pool};

/// Every board the user can see, newest first.
pub async fn list_boards(pool: Pool) -> Json<Vec<Board>> {
    let boards = pool.boards().await.unwrap_or_default();
    Json(boards)
}

/// One board, with its columns and cards.
pub async fn get_board(Path(id): Path<u64>, pool: Pool) -> Option<Json<Board>> {
    pool.board(id).await.map(Json)
}
RS
printf 'pub struct Board { pub id: u64, pub name: String }\npub struct Pool;\n' > "$api/src/db.rs"
printf '[package]\nname = "api"\nversion = "0.4.0"\nedition = "2024"\n' > "$api/Cargo.toml"
( cd "$api" && git init -q -b main && git config user.email you@shed && git config user.name you && git add -A && git commit -qm "Routes for boards" &&
  printf '\n/// A board archived: hidden from lists, kept for history.\npub async fn archive_board(Path(id): Path<u64>, pool: Pool) -> Option<()> {\n    pool.archive(id).await\n}\n' >> src/routes.rs &&
  printf 'pub fn archive(&self, id: u64) {}\n' >> src/db.rs && touch src/archive.rs )
printf '# Setup\n\nInstall the tools, then run `./dev.sh`.\n' > "$docs/guides/setup.md"
# The docs as Claude left them after the session: from before it, a pushed
# commit; in it, one pushed and one not, a staged change and one not staged
# (for the review). Git's record of the first is backdated to before the session.
mkdir -p "$root/remotes" && git init -q --bare -b main "$root/remotes/docs.git"
printf '# Introduction\n\nWhat the board is, and who it is for.\n' > "$docs/guides/intro.md"
( cd "$docs" && git init -q -b main && git config user.email you@shed && git config user.name you &&
  git remote add origin "$root/remotes/docs.git" && git add -A && git commit -qm "Guides" && git push -q -u origin main 2>/dev/null &&
  old=$(( $(date +%s) - 7200 )) && sed -i -E "s/> [0-9]+ ([+-][0-9]{4})/> $old \1/" .git/logs/HEAD .git/logs/refs/heads/main &&
  printf '# Setup\n\n## 1. Get the code\n\n```sh\ngit clone git@example.com:you/board.git\n```\n\n## 2. Install the tools\n\nNode 22 and Rust, pinned in `rust-toolchain.toml`.\n\n## 3. Run it\n\n`./dev.sh` starts the API and the web app.\n' > guides/setup.md &&
  git commit -qam "Walk new contributors through the setup" && git push -q 2>/dev/null &&
  printf '\n## Troubleshooting\n\n- **Port 5173 in use**: `lsof -i :5173`, then stop it.\n- **Old Node**: `node -v` should say 22.\n' >> guides/setup.md &&
  git commit -qam "Add troubleshooting to the setup guide" &&
  printf '# Introduction\n\nWhat the board is, who it is for, and where to start.\n' > guides/intro.md && git add guides/intro.md &&
  printf '\n## 4. Make a first change\n\nA small one, with the tests run before pushing.\n' >> guides/setup.md )

# A Claude Code session for the docs bench's AI tab, as Claude Code keeps them
# (in this demo home, so no real session is touched).
session=$(uuid)
mkdir -p "$home/.claude/projects/$(printf '%s' "$docs" | tr -c 'a-zA-Z0-9\n' '-')"
python3 - "$home/.claude/projects/$(printf '%s' "$docs" | tr -c 'a-zA-Z0-9\n' '-')/$session.jsonl" "$session" "$docs" <<'PY'
import json, sys, time, uuid
out, sid, folder = sys.argv[1], sys.argv[2], sys.argv[3]
t = time.time() - 25 * 60
recs, last = [{"type": "ai-title", "aiTitle": "Setup guide for new contributors", "sessionId": sid}], None
def stamp():
    global t
    t += 40
    return time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime(t))
def add(kind, content, **extra):
    global last
    u = str(uuid.uuid4())
    msg = {"role": kind, "content": content}
    if kind == "assistant":
        msg.update({"id": "msg_" + u[:8], "model": "claude-opus-5-5", "usage": {"input_tokens": 9, "cache_read_input_tokens": 61200, "output_tokens": 420}})
    recs.append({"type": kind, "uuid": u, "parentUuid": last, "timestamp": stamp(), "sessionId": sid, "cwd": folder, "gitBranch": "main", "message": msg, **extra})
    last = u
def tool(name, inp, result):
    tid = "toolu_" + str(uuid.uuid4())[:8]
    add("assistant", [{"type": "tool_use", "id": tid, "name": name, "input": inp}])
    add("user", [{"type": "tool_result", "tool_use_id": tid, "content": result}])
add("user", "The setup guide only says to run ./dev.sh. Make it walk a new contributor through everything, from cloning to a first change.")
add("assistant", [{"type": "text", "text": "I'll look at what the project needs first."}])
tool("Bash", {"command": "ls guides && cat guides/setup.md"}, "intro.md\nsetup.md\n# Setup\n\nInstall the tools, then run `./dev.sh`.")
tool("Read", {"file_path": folder + "/guides/intro.md"}, "1\t# Introduction")
tool("Edit", {"file_path": folder + "/guides/setup.md", "old_string": "Install the tools, then run `./dev.sh`.", "new_string": "## 1. Get the code\n\n```sh\ngit clone git@example.com:you/board.git\n```\n\n## 2. Install the tools\n\n## 3. Run it"}, "The file guides/setup.md has been updated successfully.")
add("assistant", [{"type": "text", "text": "The guide now walks through the whole path in four steps:\n\n1. **Get the code**: clone, and the branches that matter.\n2. **Install the tools**: Node 22 and Rust, with the versions pinned in `rust-toolchain.toml`.\n3. **Run it**: `./dev.sh` starts the API and the web app; the board opens at `localhost:5173`.\n4. **Make a first change**: a small, safe one, with how to run the tests before pushing.\n\n| Step | Takes |\n| --- | --- |\n| Clone and install | about 10 minutes |\n| First run | under a minute |\n\nWant me to add a troubleshooting section for the usual first-run problems?"}])
add("user", "Yes, and keep it short.")
add("assistant", [{"type": "text", "text": "Added **Troubleshooting** at the end of `guides/setup.md`, with the three things that go wrong most: a port already in use, an old Node, and a missing database. Each says how to tell, and the one command that fixes it."}])
with open(out, "w") as f:
    for r in recs:
        f.write(json.dumps(r) + "\n")
PY

# Usage limits as Claude Code last reported them, so the bars show.
mkdir -p "$root/xdg/state/shed"
now=$(date +%s)
printf '{"five_hour":{"used":0.34,"resets":%d},"week":{"used":0.58,"resets":%d},"seen":%d}' $((now + 2*3600 + 600)) $((now + 3*86400 + 5*3600)) "$now" > "$root/xdg/state/shed/ai-limits.json"

# Where each window goes (logical pixels): side by side on a large screen, so
# a display without a window manager shows them all.
place() { printf '\n[window]\nx = %s\ny = %s\nwidth = 1280.0\nheight = 800.0\nstate = "normal"\n' "$1" "$2"; }
printf 'x = 0.0\ny = 0.0\nwidth = 1280.0\nheight = 860.0\nstate = "normal"\n' > "$root/xdg/data/shed/window.toml"

dev=$(uuid)
bench board blue "~/projects/board" "$(tab "dev server" "right 0.55 (0) (1)" ""; ID=$dev READY="ready in" pane "~/projects/board" "clear" "./dev-server.sh"; AFTER=$dev pane "~/projects/board" "clear" "git log --oneline" "ls src/components")$(tab "shell" "0" ""; pane "~/projects/board" "clear" "ls -A")$(printf '\n[[tabs]]\nid = "%s"\nactive_pane = 0\n\n[tabs.project]\nroot = "~/projects/board"\nopen = ["src/App.vue"]\n' "$(uuid)")$(place 1300.0 0.0)"
bench api violet "~/projects/api" "$(tab "tests" "0" ""; pane "~/projects/api" "./watch.sh")$(tab "db" "0" 'tags = [{ label = "local", color = "#5FD38D" }]'; pane "~/projects/api" "clear" "ls src")$(printf '\n[[tabs]]\nid = "%s"\nactive_pane = 0\n\n[tabs.project]\nroot = "~/projects/api"\nopen = ["src/db.rs", "src/routes.rs"]\nactive = 1\n' "$(uuid)")$(place 2600.0 0.0)"
sed -i 's/^active_tab = 0$/active_tab = 2/' "$root/xdg/data/shed/benches/api.toml"
bench infra amber "~/projects/infra" "$(tab "prod" "0" 'tags = [{ label = "prod", color = "#F0566A", tint = true }]'; pane "~/projects/infra" "clear" "ls k8s" "./rollout.sh")$(tab "staging" "0" ""; pane "~/projects/infra" "clear" "ls k8s")"
bench docs teal "~/projects/docs" "$(tab "guides" "0" ""; pane "~/projects/docs" "clear" "ls guides")$(printf '\n[[tabs]]\nid = "%s"\nactive_pane = 0\n\n[tabs.ai]\nfolder = "~/projects/docs"\nsession = "%s"\nmode = "auto"\n' "$(uuid)" "$session")$(place 0.0 900.0)"
sed -i 's/^active_tab = 0$/active_tab = 1/' "$root/xdg/data/shed/benches/docs.toml"
printf '\n[[links]]\nname = "local"\nurl = "http://localhost:5173"\n' >> "$root/xdg/data/shed/benches/board.toml"
echo "$root"
