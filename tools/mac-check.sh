#!/usr/bin/env bash
# Type-checks (and lints) Shed for macOS on Linux: `tools/mac-check.sh`, or
# `tools/mac-check.sh check` for check without clippy. Needs the target:
#   rustup target add aarch64-apple-darwin
# The C libraries (zstd, SQLite) cannot be compiled without Apple's SDK, and
# need not be for checking, so stand-ins make empty objects; nothing is linked.
set -euo pipefail
cd "$(dirname "$0")/.."
tools=${CARGO_TARGET_DIR:-$PWD/target}/mac-check
mkdir -p "$tools"
cat > "$tools/cc" <<'SH'
#!/bin/sh
out=
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out=$2; shift ;;
    -o*) out=${1#-o} ;;
    -E) exit 0 ;;
  esac
  shift
done
[ -n "$out" ] && : > "$out"
exit 0
SH
cat > "$tools/ar" <<'SH'
#!/bin/sh
[ -n "${2:-}" ] && printf '!<arch>\n' > "$2"
exit 0
SH
chmod +x "$tools/cc" "$tools/ar"
export CC_aarch64_apple_darwin=$tools/cc AR_aarch64_apple_darwin=$tools/ar
exec cargo "${1:-clippy}" --target aarch64-apple-darwin --all-targets
