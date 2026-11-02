#!/usr/bin/env bash
# Builds Shed.app into target/macos/ (or $CARGO_TARGET_DIR/macos/), on a Mac:
#   packaging/macos/bundle.sh      then drag target/macos/Shed.app to Applications.
# Needs Rust and the Xcode command line tools (xcode-select --install).
# The app is signed ad hoc, for this Mac: one built elsewhere and downloaded is
# stopped by Gatekeeper until allowed (right-click → Open, once).
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
target=${CARGO_TARGET_DIR:-$root/target}
out=$target/macos
app=$out/Shed.app

cargo build --release --locked -p shed-app -p shed-keeper

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$target/release/shed" "$target/release/shedd" "$app/Contents/MacOS/"
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
cargo run --release --locked -q -p shed-desktop --example app_icons -- --iconset "$scratch/Shed.iconset"
iconutil -c icns "$scratch/Shed.iconset" -o "$app/Contents/Resources/Shed.icns"

codesign --force --sign - "$app/Contents/MacOS/shedd"
codesign --force --sign - "$app"
ditto -c -k --keepParent "$app" "$out/Shed-$version-$(uname -m).zip"
echo "$app"
