#!/usr/bin/env bash
# Updates your nexus server from this computer: checks that what you have here
# is pushed, then over ssh runs packaging/nexus/update.sh on the server (pull,
# build, install, restart; also a server workspace there, if there is one).
#
#   tools/deploy-nexus.sh root@nexus.example.com
#   tools/deploy-nexus.sh root@nexus.example.com -y            # restarts the server workspace's keeper without asking
#   tools/deploy-nexus.sh root@nexus.example.com --no-workspace
#   SHED_NEXUS_SSH=root@nexus.example.com tools/deploy-nexus.sh
#
#   SHED_NEXUS_SSH   the ssh destination, when not given (an ~/.ssh/config Host works)
#   SHED_NEXUS_REPO  the checkout on the server (default: /root/shed)
#
# The server builds from GitHub, so unpushed commits here are not deployed:
# it stops if HEAD is not on origin. The ssh user must be root or have sudo.
set -euo pipefail
cd "$(dirname "$0")/.."

case "${1:-}" in
  -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
  "" | -*) host=${SHED_NEXUS_SSH:-} ;;
  *) host=$1; shift ;;
esac
[[ -n "$host" ]] || { echo "which server? tools/deploy-nexus.sh USER@HOST (or SHED_NEXUS_SSH)" >&2; exit 2; }
repo=${SHED_NEXUS_REPO:-/root/shed}

git fetch -q origin
branch=$(git rev-parse --abbrev-ref HEAD)
if ! git merge-base --is-ancestor HEAD "origin/$branch" 2>/dev/null; then
  echo "HEAD ($(git rev-parse --short HEAD)) is not pushed to origin/$branch: push first, the server pulls from GitHub" >&2
  exit 1
fi
[[ -z "$(git status --porcelain --untracked-files=no)" ]] || echo "note: uncommitted changes here are not deployed"

echo "== updating the nexus on $host ($repo, $(git rev-parse --short "origin/$branch"))"
# The pull first, so the update script that runs is the one just pulled.
quoted=
if [[ $# -gt 0 ]]; then quoted=$(printf ' %q' "$@"); fi
ssh -t "$host" "set -e; if [ \$(id -u) -ne 0 ]; then s=sudo; fi; \$s git -C $repo pull --ff-only; \$s $repo/packaging/nexus/update.sh --pulled$quoted"
