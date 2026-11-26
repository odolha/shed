#!/usr/bin/env bash
# Updates a nexus set up with install.sh, on the server: pulls this checkout,
# builds shed-nexus from it (with the web app), installs it and restarts the
# service (through install.sh, which keeps the settings). If the server also
# runs a server workspace (shed and shedd in /usr/local/bin), it builds those
# too and, if asked, restarts that keeper. See docs/nexus-deploy.md.
#
# As root (or a user with sudo), on the server; install.sh links it as
# shed-nexus-update (in /usr/local/sbin):
#
#   shed-nexus-update                 # asks before restarting the workspace's keeper
#   shed-nexus-update -y              # restarts it without asking
#   shed-nexus-update --no-workspace  # only the nexus
#
# (The first time, before that link exists: git -C /root/shed pull, then
# /root/shed/packaging/nexus/update.sh.)
#
# From your computer, tools/deploy-nexus.sh runs this over ssh.
#
#   WORKSPACE_USER  the server workspace's user (default: shed)
#   CARGO_TARGET_DIR  where it builds (default: target/ in this checkout)
#
# Restarting the workspace's keeper saves its benches, but ends the programs
# running in its terminals and its Claude Code sessions; without a restart the
# keeper goes on with the old build until it next starts (binaries are
# replaced, never changed in place).
set -euo pipefail

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; }

restart=ask
workspace=auto
pulled=
for arg in "$@"; do
  case "$arg" in
    -y | --yes) restart=yes ;;
    --no-restart) restart=no ;;
    --no-workspace) workspace=no ;;
    --pulled) pulled=1 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "unknown option: $arg (see --help)" >&2; exit 2 ;;
  esac
done

if [[ $EUID -ne 0 ]]; then
  exec sudo --preserve-env=CARGO_TARGET_DIR,WORKSPACE_USER "$0" "$@"
fi

# Through the shed-nexus-update link too: where the script really is.
here=$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)
repo=$(cd "$here/../.." && pwd)
say() { printf '\n\033[1;36m== %s\033[0m\n' "$*"; }
die() { printf '\n\033[1;31mshed-nexus update: %s\033[0m\n' "$*" >&2; exit 1; }

# Pull first, then go on as the script just pulled (it may have changed).
if [[ -z "$pulled" ]]; then
  say "pulling $repo"
  git -C "$repo" pull --ff-only || die "git pull failed (local changes in $repo?)"
  args=("$@")
  exec "$repo/packaging/nexus/update.sh" --pulled ${args[@]+"${args[@]}"}
fi

# Rust and Node as the setup in docs/nexus-deploy.md installs them.
[[ -f "$HOME/.cargo/env" ]] && . "$HOME/.cargo/env"
if [[ -s "${NVM_DIR:-$HOME/.nvm}/nvm.sh" ]]; then
  set +u; . "${NVM_DIR:-$HOME/.nvm}/nvm.sh"; set -u
fi
command -v cargo >/dev/null || die "no cargo on PATH (see docs/nexus-deploy.md, 1)"
command -v npm >/dev/null || die "no npm on PATH: the web app needs Node 24.15+ (see docs/nexus-deploy.md, 1)"

export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$repo/target}
cd "$repo"
say "building shed-nexus $(git describe --always --dirty 2>/dev/null || echo '?')"
cargo build --release --locked -p shed-nexus

say "installing the nexus"
"$repo/packaging/nexus/install.sh" BIN="$CARGO_TARGET_DIR/release/shed-nexus"

user=${WORKSPACE_USER:-shed}
if [[ "$workspace" == auto ]]; then
  if [[ -x /usr/local/bin/shedd ]] && id "$user" >/dev/null 2>&1; then workspace=yes; else workspace=no; fi
fi
if [[ "$workspace" == no ]]; then
  say "done"
  exit 0
fi

say "building the server workspace's shed and shedd (no windows)"
cargo build --release --locked -p shed-app --no-default-features --features tui
cargo build --release --locked -p shed-keeper
for b in shed shedd; do
  install -m 0755 "$CARGO_TARGET_DIR/release/$b" "/usr/local/bin/.$b.new"
  mv -f "/usr/local/bin/.$b.new" "/usr/local/bin/$b"
done
echo "   installed /usr/local/bin/shed, /usr/local/bin/shedd"

if [[ "$restart" == ask ]]; then
  if [[ -t 0 ]]; then
    echo
    echo "Restart $user's keeper now? Benches are saved, but programs running in its"
    read -r -p "terminals and its Claude Code sessions stop. [y/N] " answer
    case "$answer" in y | Y | yes) restart=yes ;; *) restart=no ;; esac
  else
    restart=no
  fi
fi
if [[ "$restart" == yes ]]; then
  say "restarting $user's keeper"
  if systemctl --user -M "$user@" is-enabled shedd.service >/dev/null 2>&1; then
    systemctl --user -M "$user@" restart shedd.service
  else
    runuser -u "$user" -- env XDG_RUNTIME_DIR="/run/user/$(id -u "$user")" /usr/local/bin/shed stop || true
    echo "   stopped; it starts again on the next 'shed' (or 'shed tui') as $user"
  fi
else
  echo "   $user's keeper goes on with the old build until it restarts"
  echo "   (later: systemctl --user -M $user@ restart shedd.service)"
fi
say "done"
