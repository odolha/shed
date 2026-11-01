#!/usr/bin/env bash
# Builds the demo world the docs' screenshots show: a home with a neutral
# prompt, a few projects, and saved benches whose startup commands make
# realistic output. Usage: demo-world.sh <dir>   (wipes <dir> first)
set -euo pipefail
root=${1:?usage: demo-world.sh <dir>}
rm -rf "$root"
home=$root/home
mkdir -p "$home" "$root/xdg/config/shed" "$root/xdg/data/shed/benches" "$root/xdg/state" "$root/run"

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
pane() { # cwd startup...
  local cwd=$1; shift
  local cmds=""; for c in "$@"; do cmds+="\"$c\", "; done
  printf '\n[[tabs.panes]]\nid = "%s"\ncwd = "%s"\nstartup = [%s]\n' "$(uuid)" "$cwd" "${cmds%, }"
}
bench board blue "~/projects/board" "$(tab "dev server" "right 0.55 (0) (1)" ""; pane "~/projects/board" "clear" "./dev-server.sh"; pane "~/projects/board" "clear" "git log --oneline" "ls src/components")$(tab "shell" "0" ""; pane "~/projects/board" "clear" "ls -A")"
bench api violet "~/projects/api" "$(tab "tests" "0" ""; pane "~/projects/api" "./watch.sh")$(tab "db" "0" 'tags = [{ label = "local", color = "#5FD38D" }]'; pane "~/projects/api" "clear" "ls src")"
bench infra amber "~/projects/infra" "$(tab "prod" "0" 'tags = [{ label = "prod", color = "#F0566A", tint = true }]'; pane "~/projects/infra" "clear" "ls k8s" "./rollout.sh")$(tab "staging" "0" ""; pane "~/projects/infra" "clear" "ls k8s")"
bench docs teal "~/projects/docs" "$(tab "guides" "0" ""; pane "~/projects/docs" "clear" "ls guides")"
echo "$root"
