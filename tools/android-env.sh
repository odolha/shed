# Source this to build the Android app (android/) without a system-wide JDK
# or Android SDK:
#
#     . tools/android-env.sh
#     cd android && ./gradlew assembleDebug
#
# It exports JAVA_HOME and ANDROID_HOME from a folder of your own, by default
# ~/.cache/shed-dev/android (set SHED_ANDROID_DIR for another), and puts java,
# sdkmanager and adb on PATH. Set the folder up once (no root needed; about
# 1.5 GB, no emulator images):
#
#     dir=~/.cache/shed-dev/android; mkdir -p "$dir/jdk" "$dir/sdk/cmdline-tools"
#     # A JDK 21 (Eclipse Temurin):
#     curl -L https://api.adoptium.net/v3/binary/latest/21/ga/linux/x64/jdk/hotspot/normal/eclipse \
#         | tar -xz -C "$dir/jdk" --strip-components=1
#     # The Android command-line tools (the newest "commandlinetools-linux-*"
#     # from https://developer.android.com/studio#command-line-tools-only):
#     curl -Lo /tmp/cmdline.zip https://dl.google.com/android/repository/commandlinetools-linux-16111833_latest.zip
#     unzip -q /tmp/cmdline.zip -d "$dir/sdk/cmdline-tools" && mv "$dir/sdk/cmdline-tools/cmdline-tools" "$dir/sdk/cmdline-tools/latest"
#     . tools/android-env.sh
#     yes | sdkmanager --licenses
#     sdkmanager "platforms;android-36" "build-tools;36.0.0" "platform-tools"
#
# On macOS, use the macOS (aarch64 or x64) JDK and command-line tools; the
# JDK's home is then "$dir/jdk/Contents/Home".

dir="${SHED_ANDROID_DIR:-$HOME/.cache/shed-dev/android}"
if [ -d "$dir/jdk/Contents/Home" ]; then
    export JAVA_HOME="$dir/jdk/Contents/Home"
else
    export JAVA_HOME="$dir/jdk"
fi
export ANDROID_HOME="$dir/sdk"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export PATH="$JAVA_HOME/bin:$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/platform-tools:$PATH"

if [ ! -x "$JAVA_HOME/bin/java" ]; then
    echo "android-env: no JDK in $JAVA_HOME (see the setup at the top of tools/android-env.sh)" >&2
fi
if [ ! -d "$ANDROID_HOME/platforms" ]; then
    echo "android-env: no Android SDK platforms in $ANDROID_HOME (see the setup at the top of tools/android-env.sh)" >&2
fi
unset dir
