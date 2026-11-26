#!/usr/bin/env bash
# Builds the Android app from this checkout and installs it on your phone over
# adb (USB, or Wi-Fi once paired), replacing the one there; its sign-in and
# settings stay. See android/README.md.
#
#   tools/android-install.sh                  # build and install
#   tools/android-install.sh --build-only     # only the APK
#   tools/android-install.sh -s SERIAL        # this phone, when adb sees several
#
# The phone needs developer options and USB debugging (Settings → About phone
# → tap Build number seven times; then Developer options → USB debugging).
# Over Wi-Fi instead (Developer options → Wireless debugging), once:
#
#   . tools/android-env.sh
#   adb pair PHONE_IP:PAIR_PORT       # "Pair device with pairing code"
#   adb connect PHONE_IP:PORT         # the address shown on Wireless debugging
#
# Debug builds only (release signing is not set up): each is signed with this
# computer's debug key (~/.android/debug.keystore), so an app installed from
# another computer must be uninstalled first.
set -euo pipefail
cd "$(dirname "$0")/.."

install=1
serial=${ANDROID_SERIAL:-}
while [[ $# -gt 0 ]]; do
  case "$1" in
    --build-only) install=0 ;;
    -s) serial=${2:?-s needs a serial (adb devices lists them)}; shift ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
  shift
done

. tools/android-env.sh
apk=$PWD/android/app/build/outputs/apk/debug/app-debug.apk

echo "== building the Android app $(git describe --always --dirty 2>/dev/null || echo '?')"
(cd android && ./gradlew -q assembleDebug)
echo "== built: $apk"
[[ "$install" == 1 ]] || exit 0

mapfile -t devices < <(adb devices | awk 'NR > 1 && $2 == "device" { print $1 }')
if [[ -z "$serial" ]]; then
  case ${#devices[@]} in
    0)
      echo "No phone found by adb. Plug it in with USB debugging on (and allow this computer" >&2
      echo "on the phone), or connect over Wi-Fi: see tools/android-install.sh --help." >&2
      adb devices | awk 'NR > 1 && NF && $2 != "device" { print "   " $1 ": " $2 " (unauthorized: allow it on the phone)" }' >&2
      exit 1 ;;
    1) serial=${devices[0]} ;;
    *) echo "Several phones: pick one with -s (${devices[*]})" >&2; exit 1 ;;
  esac
fi

echo "== installing on $(adb -s "$serial" shell getprop ro.product.model 2>/dev/null | tr -d '\r') ($serial)"
if ! out=$(adb -s "$serial" install -r "$apk" 2>&1); then
  echo "$out" >&2
  if grep -q INSTALL_FAILED_UPDATE_INCOMPATIBLE <<<"$out"; then
    echo "The installed app was signed elsewhere: uninstall Shed on the phone (its sign-in goes with it), then run this again." >&2
  fi
  exit 1
fi
echo "== installed"
