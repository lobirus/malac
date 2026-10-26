# malac for Android

Watch and command a malac daemon from an Android phone: workspaces, tabs and agents with their live status, transcripts with tool calls and subagents, prompts with photos, approvals, background jobs, and notifications when an agent needs you or finishes. The app is a client of the daemon's [protocol](../../docs/protocol.md), like the web UI.

## Layout

| Module | What it is |
|---|---|
| `core` | Plain Kotlin/JVM, no Android: protocol models (kotlinx.serialization), the state reducer, the transcript builder, Markdown parsing, the WebSocket connection and REST client (OkHttp), pairing. All tests live here. |
| `app` | The Android app: Jetpack Compose and Material 3 with the malac palette, token storage, notifications, background checks. |

The protocol fixtures in [`../fixtures`](../fixtures) were recorded from a live daemon and are shared with the iOS app; `core`'s tests read them in place.

| | Version |
|---|---|
| Gradle (wrapper) | 9.8.2 |
| Android Gradle plugin | 9.4.1, with built-in Kotlin |
| Kotlin | 2.4.21 |
| Compose BOM | 2026.09.00 (Material 3 1.4) |
| `compileSdk` / `targetSdk` / `minSdk` | 37 / 37 / 26 |

## Build and run

You need JDK 17 or newer and the Android SDK with platform 37 and build tools 36: install [Android Studio](https://developer.android.com/studio), or the command-line tools and then `sdkmanager "platforms;android-37" "build-tools;36.0.0" "platform-tools"`. Point the build at the SDK with `ANDROID_HOME` or a `local.properties` file containing `sdk.dir=/path/to/sdk`.

To run it on your phone, turn on USB debugging (Settings → About phone → tap Build number seven times, then Developer options → USB debugging), connect it and run:

```bash
./gradlew :app:installDebug
# or build the APK and install it yourself
./gradlew :app:assembleDebug
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

The debug build is signed with the SDK's debug key, which is all a phone of your own needs. In Android Studio, open this directory and run the `app` configuration.

## Tests

```bash
./gradlew :core:test
```

This needs no Android SDK. The tests decode every message of a recorded session, the documentation's examples and the REST responses; replay the session through the reducer and compare the result with the daemon's own final snapshot; check approvals that another client answers, resync after `lagged`, and reconnects and revoked tokens against a fake daemon; and build transcripts from saved history and live events.

## Pairing

The daemon must be reachable from the phone: on the same network with `web.remote = true` (or `malac daemon start --remote`), through a tunnel, or through an SSH tunnel to its loopback port. Then, on the computer:

```bash
malac remote pair
```

It shows a QR code and a one-time code such as `ABCDE-FGHJK`, valid for ten minutes. In the app, choose **Scan QR code** (Google's code scanner, so the app needs no camera permission), or type the daemon's address and the code; the device name is prefilled with the phone's and can be changed. The app follows [remote.md](../../docs/remote.md#for-apps): `GET /api/hello` confirms the address is a malac daemon, `POST /api/pair` with `kind: "app"` trades the code for a device token of its own, and the token goes in `Authorization: Bearer` on REST calls and on the WebSocket upgrade, without an `Origin` header. A wrong or expired code is a `403`, too many wrong codes a `429`; the daemon's message is shown. A link with `?token=…` is accepted too, and the token is checked before it is saved.

Tokens are encrypted with an AES-GCM key in the Android Keystore, which cannot be exported, and only the ciphertext is stored; the list of daemons (name, address, device id) is in DataStore. Backups are off, since the key would not restore anyway. Several daemons can be saved; **Daemons** switches between them, renames and forgets them, and shows the selected daemon's version, remote access (on or off, the best address, a tunnel, the number of paired devices) and keep-awake mode with its reason, which can be switched between while busy, always and off until the daemon restarts (`keep_awake`). Forgetting a daemon revokes this phone there (`GET /api/devices`, the entry marked `current`, then `DELETE /api/devices/{id}`) when it answers, and deletes the token here either way; an unreachable daemon keeps the device listed until `malac remote revoke <id>` on the computer. A revoked token makes the daemon answer 401, and the app stops retrying and asks you to pair again; other refusals, such as remote access being off, are shown and retried.

Daemons on a local network or a VPN speak plain HTTP, so the app allows cleartext traffic. Prefer a tunnel or reverse proxy with HTTPS when the network is not yours.

## Notifications

Android does not keep an app's sockets open in the background, so notifications work in two ways:

- **While the app runs**, it holds a WebSocket to the selected daemon, and for a minute after it leaves the screen. Approval requests and finished turns of top-level agents that arrive while the app is not visible become notifications. An approval notification has **Allow** and **Deny** actions, which answer over the open socket or, if it closed, with `POST /api/command`. A notification disappears when any client answers the request.
- **In the background**, a WorkManager job polls `GET /api/snapshot` of every saved daemon about every 15 minutes, the shortest period Android allows, and notifies about requests it has not reported before and agents that stopped running since the last check. Doze and App Standby can delay it further. The snapshot lists pending approvals of every workspace but agents of the daemon's last used workspace only, so finished turns elsewhere are not noticed in the background.

On Android 13 and later the app asks for the notification permission when it starts, until it is granted or Android stops showing the request.

### Remote push

Instant notifications on a sleeping phone need push, and FCM needs server credentials that an open-source daemon cannot ship. The seam is [`PushRegistrar`](app/src/main/kotlin/io/malac/app/notify/PushRegistrar.kt): a relay holding the FCM credentials would receive "request R of daemon D is waiting" (ids only, no content) from the daemon and wake the phone, which then fetches the details over its own authenticated connection, exactly as the background check does. Adding it means an implementation of `PushRegistrar` that registers the phone's FCM token with the daemon for this device, plus the relay; the notification code stays as it is.

## How it works

- `core/state/DaemonState.kt` mirrors the web UI's `store.js`: the snapshot replaces everything, broadcasts and agent events update it, `lagged` and a deleted workspace trigger a fresh snapshot. Browsing another workspace sends `{"type": "snapshot", "workspace": id}`, which switches this connection without changing the daemon's last used workspace.
- `core/transcript/Transcript.kt` mirrors `transcript.js`. A transcript is the saved history from `GET /api/sessions/{id}` plus the agent's events after the moment it was fetched; it is rebuilt whenever either changes, and fetched again after every snapshot.
- `core/net/DaemonConnection.kt` reconnects with backoff from 0.5 to 10 seconds, at once when the network comes back, and pings every 25 seconds to notice dead connections. After `shutdown` it keeps trying, so a restarted daemon is picked up.
