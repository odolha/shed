#!/usr/bin/env bash
# Builds Shed afresh from this checkout and installs it for this machine:
# `shed` and `shedd`, built together in release, into ~/.cargo/bin; then
# restarts Shed so every part runs the new build (the keeper too).
#
#   tools/reinstall.sh            # asks before restarting
#   tools/reinstall.sh -y         # restarts without asking
#   tools/reinstall.sh --no-restart
#   tools/reinstall.sh --quick    # keeps what is already compiled (cargo's own tracking)
#
# Shed's own crates (and the vendored ones) are compiled again from scratch
# every time, whatever changed; only third-party crates are kept. It builds in
# target/install, apart from dev builds (target/debug, target/release), so
# rebuilding there never changes the Shed in use.
#
# The restart stops the keeper (`shed stop`: benches are saved first), which
# ends every program running in Shed's terminals and every Claude Code
# session; then Shed starts again from the new binaries (benches reopen if
# `auto_restore` is on). It is done from a process of its own, so it works
# from a terminal inside Shed too. Bench launchers and Shed's desktop entry
# point at the new binaries once it starts.
#
# Linux only (on a Mac: packaging/macos/bundle.sh).
set -euo pipefail
cd "$(dirname "$0")/.."

restart=ask
clean=1
for arg in "$@"; do
  case "$arg" in
    -y|--yes) restart=yes ;;
    --no-restart) restart=no ;;
    --quick) clean=0 ;;
    -h|--help) sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $arg (see --help)" >&2; exit 2 ;;
  esac
done
if [ "$(uname -s)" != Linux ]; then
  echo "Linux only; on a Mac use packaging/macos/bundle.sh" >&2
  exit 1
fi

bin=${SHED_INSTALL_DIR:-$HOME/.cargo/bin}
target=${SHED_INSTALL_TARGET:-$PWD/target/install}
export CARGO_TARGET_DIR=$target

echo "== Shed $(git describe --always --dirty 2>/dev/null || echo '?') → $bin"
if [ "$clean" = 1 ]; then
  # Every crate that is not from a registry: the workspace and vendor/.
  mapfile -t own < <(cargo metadata --format-version 1 --locked |
    python3 -c 'import json, sys; [print(p["name"]) for p in json.load(sys.stdin)["packages"] if p["source"] is None]')
  args=()
  for p in "${own[@]}"; do args+=(-p "$p"); done
  echo "== cleaning ${#own[@]} crates of Shed's own in $target"
  cargo clean --release "${args[@]}" 2>&1 | grep -v '^\s*Removed' || true
fi
echo "== building (release)"
cargo build --release --locked -p shed-app -p shed-keeper

mkdir -p "$bin"
for b in shed shedd; do
  # A new file renamed over the old: programs running the old one go on.
  cp "$target/release/$b" "$bin/.$b.new"
  chmod 755 "$bin/.$b.new"
  mv -f "$bin/.$b.new" "$bin/$b"
done
echo "== installed: $bin/shed, $bin/shedd"
"$bin/shed" --version 2>/dev/null || true

if [ "$restart" = ask ]; then
  if [ -t 0 ]; then
    echo
    echo "Restart Shed now? Benches are saved, but programs running in its terminals"
    read -r -p "and Claude Code sessions stop. [y/N] " answer
    case "$answer" in y|Y|yes) restart=yes ;; *) restart=no ;; esac
  else
    restart=no
  fi
fi
if [ "$restart" = no ]; then
  echo "Not restarted: the running keeper stays on the old build until '$bin/shed stop'."
  exit 0
fi

log=${XDG_STATE_HOME:-$HOME/.local/state}/shed/reinstall.log
mkdir -p "$(dirname "$log")"
echo "== restarting Shed (log: $log)"
# Its own session: stopping the keeper hangs up its terminals, maybe this one.
setsid bash -c '
  bin=$1
  echo "--- $(date)"
  "$bin/shed" stop
  # The windows quit when their keeper goes.
  sleep 1
  nohup "$bin/shed" >/dev/null 2>&1 &
  echo "started"
' reinstall "$bin" >>"$log" 2>&1 < /dev/null &
disown
