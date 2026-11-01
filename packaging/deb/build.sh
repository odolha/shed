#!/usr/bin/env bash
# Builds the Debian/Ubuntu package into target/debian/ (or $CARGO_TARGET_DIR/debian/):
#   packaging/deb/build.sh            then   sudo apt install ./target/debian/shed-terminal_*.deb
# The package is called shed-terminal: Debian already has a `shed` (a hex editor),
# which also installs /usr/bin/shed, so the two conflict.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD

pkg=shed-terminal
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
revision=${REVISION:-1}
arch=$(dpkg --print-architecture)
target=${CARGO_TARGET_DIR:-$root/target}
stage=$target/debian/stage
out=$target/debian/${pkg}_${version}-${revision}_${arch}.deb

cargo build --release --locked -p shed-app -p shed-keeper

rm -rf "$stage"
mkdir -p "$stage/DEBIAN"
install -Dm755 "$target/release/shed" "$stage/usr/bin/shed"
install -Dm755 "$target/release/shedd" "$stage/usr/bin/shedd"
# Line tables make the binaries several times larger; symbols stay for backtraces.
strip --strip-debug "$stage/usr/bin/shed" "$stage/usr/bin/shedd"
install -Dm644 packaging/linux/dev.shed.desktop "$stage/usr/share/applications/dev.shed.desktop"
install -Dm644 packaging/linux/shedd.service "$stage/usr/lib/systemd/user/shedd.service"
cargo run --release --locked -q -p shed-desktop --example app_icons -- "$stage/usr/share/icons/hicolor"
doc=$stage/usr/share/doc/$pkg
install -Dm644 -t "$doc" README.md LICENSE-MIT LICENSE-APACHE packaging/linux/copyright

# Whatever the umask: directories and programs 755, everything else 644.
find "$stage" -path "$stage/DEBIAN" -prune -o -type d -exec chmod 755 {} +
find "$stage/usr" -type f -exec chmod 644 {} +
chmod 755 "$stage/usr/bin/shed" "$stage/usr/bin/shedd"

# Libraries the binaries link, as package dependencies (dpkg-shlibdeps wants a debian/ dir).
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir "$scratch/debian"
printf 'Source: %s\n\nPackage: %s\nArchitecture: any\n' "$pkg" "$pkg" > "$scratch/debian/control"
linked=$(cd "$scratch" && dpkg-shlibdeps -O "$stage/usr/bin/shed" "$stage/usr/bin/shedd" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')
# Opened at run time, so dpkg-shlibdeps cannot see them: Vulkan draws the
# windows, Wayland and EGL serve Wayland sessions.
depends="$linked, libvulkan1, libwayland-client0, libegl1"

cat > "$stage/DEBIAN/control" <<CONTROL
Package: $pkg
Version: $version-$revision
Architecture: $arch
Maintainer: odolha <odolha@gmail.com>
Installed-Size: $(du -sk --exclude=DEBIAN "$stage" | cut -f1)
Depends: $depends
Recommends: bash, mesa-vulkan-drivers | vulkan-icd
Conflicts: shed
Section: x11
Priority: optional
Description: terminal whose shells outlive its windows
 Shed is a GPU-accelerated terminal for Linux. A small background process,
 the keeper, holds every shell and program, so closing a window does not
 stop them. Work is organised in benches: workspaces with their own window,
 colour and taskbar icon, saved with their layout, folders, startup and
 teardown commands, and brought back when opened. A logbook keeps every
 command, searchable per tab, per bench or everywhere.
CONTROL

dpkg-deb --root-owner-group --build "$stage" "$out" >/dev/null
echo "$out"
