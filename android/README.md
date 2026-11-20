# Shed for Android

The Android app reaches your Shed workspaces through your nexus (see
[Reaching Shed from elsewhere](../docs/user-guide.md#reaching-shed-from-elsewhere)):
benches and tabs, AI tabs (the conversation, permissions, questions, prompts
with photos, sessions, drafts, review), terminals, the logbook, project files,
search and Git. Native Kotlin with Jetpack Compose; no Google Play Services, so
it builds from source and suits F-Droid.

## Build

Nothing system-wide is needed: `tools/android-env.sh` points `JAVA_HOME` and
`ANDROID_HOME` at a JDK 21 and an Android SDK in `~/.cache/shed-dev/android`
(or `$SHED_ANDROID_DIR`). Its header says how to fill that folder once (a
Temurin JDK, the command-line tools, `sdkmanager --licenses`, then
`platforms;android-36`, `build-tools;36.0.0` and `platform-tools`; about 1 GB, no
emulator images).

```sh
. tools/android-env.sh
cd android
./gradlew assembleDebug                      # app/build/outputs/apk/debug/app-debug.apk
./gradlew testDebugUnitTest lintDebug        # JVM tests and lint
```

The Gradle wrapper downloads Gradle itself. Versions are pinned in
`gradle/libs.versions.toml`. minSdk 29 (Android 10), targetSdk 36.

**Release signing is not configured**: only debug builds are made for now.
A release build needs a keystore of your own and a `signingConfig`.

## Install on a phone

Turn on developer options and USB debugging on the phone, plug it in, then:

```sh
adb devices                                   # the phone should be listed
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
```

## Sign in and pair

1. Set up a nexus (see [Deploying a nexus](../docs/nexus-deploy.md)) and its
   account in its web app: user name, password, an authenticator (TOTP).
2. In the app: the nexus address (`https://nexus.example.com`), user name,
   password and a code from the authenticator (or a recovery code). The phone
   becomes a device of the account and gets a device token, kept encrypted with
   a key in the Android Keystore. Plain `http://` is accepted only for
   `10.0.2.2` and `localhost` (an emulator or `adb reverse` reaching a nexus on
   your computer during development).
3. Link a workspace: on the computer, `shed nexus join https://nexus.example.com`
   prints a code. In the app (or the web app), **Devices → Approve a
   Workspace…**, type that code, give the workspace a name and a fresh
   authenticator code. See
   [Linking a workspace](../docs/user-guide.md#linking-a-workspace).
4. **Settings → Unlock with fingerprint or screen lock** asks for it whenever
   the app comes back. **Sign Out…** revokes this phone's token at the nexus and
   forgets it here. Devices lists every device; revoking one asks for a code.

A workspace that allows viewing only (its policy is `view`) shows everything
but offers no changes.

## Notifications (UnifiedPush and ntfy)

Push goes through [UnifiedPush](https://unifiedpush.org), not Google: a
*distributor* app on the phone keeps one connection to a push server, and the
nexus sends to the endpoint it gives. Messages are encrypted for the app
(RFC 8291), so the push server sees only ciphertext.

1. Run a ntfy server (or use one you trust) and turn push on in the nexus
   (`[push] enabled = true`); see "Push to Android with ntfy" in
   [Deploying a nexus](../docs/nexus-deploy.md).
2. Install the ntfy app (F-Droid), set its default server to yours
   (`https://ntfy.example.com`) and sign in there if your server needs it.
3. In Shed: **Settings → Push notifications**. Pick ntfy if asked. Then choose
   which kinds you want: Claude waits for an answer, Claude finished a turn, a
   long command finished, a workspace went offline. Each has its own Android
   notification channel; tapping a notification opens its tab
   (`shed://w/{workspace}/b/{bench}/t/{tab}`).

Without a distributor installed, Settings says how to get one.

## The code

`app/src/main/java/dev/shed/`:

| Where | What |
| --- | --- |
| `api/Api.kt` | The API's types, generated from `crates/shed-api` (`cargo run -p shed-codegen`); never edited by hand |
| `net/` | `ShedJson` (the JSON setup), `NexusHttp` (`/api/v1/…`, bearer token), `NexusSocket` (the `/api` WebSocket: requests by id, subscriptions as flows, reconnects) |
| `data/` | The account (token sealed with a Keystore AES-GCM key) and the app's settings |
| `push/` | UnifiedPush: registering, the endpoint to the nexus, notifications |
| `ui/` | Compose screens: `Nav.kt` (routes and `shed://` links), `workspaces/`, `bench/`, `ai/` (transcript, Markdown, prompt, sessions, drafts, review), `terminal/` (screen model and re-wrapping), `logbook/`, `project/`, `git/`, `devices/`, `settings/`, `common/` |
| `ShedApp.kt`, `MainActivity.kt` | The session (connects in the foreground, rests a minute after), the lock |

Tests (`app/src/test`) run on the JVM: JSON round-trips, the socket against a
fake nexus (MockWebServer), terminal re-wrapping, the Markdown reader.
