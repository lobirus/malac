# malac for Android

Watch and command a malac daemon from an Android phone: workspaces, tabs and agents with their live status, transcripts with tool calls and subagents, prompts with photos, approvals, background jobs, files and screenshots malac sends, dictation, speech, talk mode, and notifications when an agent needs you or finishes. The app is a client of the daemon's [protocol](../../docs/protocol.md), like the web UI.

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

The debug build is signed with the SDK's debug key, which is all a phone of your own needs. It installs as `io.malac.app.debug`, labelled "malac debug", so it sits next to the released app instead of replacing it. In Android Studio, open this directory and run the `app` configuration.

### Released APKs

Every release has `malac-<version>-android.apk`, a release build signed with the project's key, so a newer one installs over an older one and keeps the app's data. The [mobile workflow](../../.github/workflows/mobile.yml) also keeps a debug APK for each change to `apps/` (artifact `malac-android-debug`); CI creates a new debug key for every run, so uninstall the previous debug build before installing the next.

To sign a release build yourself, point the build at a PKCS12 keystore:

```bash
MALAC_KEYSTORE=/path/to/release.p12 MALAC_KEYSTORE_PASSWORD=... ./gradlew :app:assembleRelease
```

`MALAC_KEY_ALIAS` defaults to `malac`. Without `MALAC_KEYSTORE` the release APK comes out unsigned.

## Tests

```bash
./gradlew :core:test
```

This needs no Android SDK. The tests decode every message of a recorded session, the documentation's examples and the REST responses; replay the session through the reducer and compare the result with the daemon's own final snapshot; check approvals that another client answers, resync after `lagged`, and reconnects and revoked tokens against a fake daemon; and build transcripts from saved history and live events. The voice tests encode WAV and PCM, follow a spoken conversation, parse the files `send_to_user` sends, and check the voice REST calls, the listener id across reconnects and talk mode's WebSocket against fake daemons.

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

## Voice, files and images

See [voice.md](../../docs/voice.md) for what each feature does and the daemon side.

- **Images** open full screen from every thumbnail: screenshots in tool results, photos and attachments of prompts, images malac sent, and photos about to be sent. Pinch to zoom around your fingers, drag to pan, double-tap to switch between fitting and 2.5x, swipe down or go back to close. The bar has share and **Save to Pictures** (Pictures/malac through MediaStore, which needs no permission from Android 10; older versions save with the system's file picker). The viewer draws behind the status bar and the camera cutout; its bar stays clear of both. Live tool results carry only placeholders for their images, so the transcript fetches the saved history again once the next message is recorded, as the web UI does.
- **Files malac sends** with `send_to_user` replace the tool row: the caption, images inline (`GET /api/shared/{id}`, decoded at the size shown) and other files as a card with the name and size. Tapping a card downloads it to the cache and opens it with the app you pick; share sends it elsewhere. A [FileProvider](app/src/main/res/xml/file_paths.xml) hands out only the cache's `shared/` folder.
- **Dictation**: the microphone in the composer appears when the daemon's dictation is ready. Tap it, speak, and tap again: the recording (16 kHz mono 16-bit WAV) goes to `POST /api/voice/transcribe`, and the text goes in at the cursor for you to edit and send. The strip above the prompt shows the time and level, and cancels. The app asks for the microphone the first time, with a rationale when Android suggests one and the way to Settings once it is denied.
- **Speech**: with **Speak on this device** on (Daemons → Voice; stored on the phone, on by default) and the daemon's speech ready, the connection sends `voice_listen` after every connect, and the sentences the daemon picks this phone for are fetched from `/api/voice/say/{id}` and played one at a time with MediaPlayer, ducking other audio, and shown briefly at the bottom.
- **Talk mode**: the headset in an agent's top bar opens a spoken conversation through that tab (`/api/voice/talk`). The microphone (voice communication source, with the platform's echo canceller and noise suppressor) streams 100 ms frames of 24 kHz PCM once the daemon is ready; the voice plays as it arrives through an AudioTrack, on the speaker in communication mode unless a headset is connected, and stops the moment you start talking. The screen shows the state, the level, what was said and what the voice asked malac, with mute and end; it stays on, and back ends the conversation. malac's own speech waits until it is over.
- **Voice settings**: Daemons → Voice has a switch, model and voice for each feature and the dictation language (`PUT /api/voice`), and checks the providers' model lists again (`GET /api/voice?refresh=true`).

## How it works

- `core/state/DaemonState.kt` mirrors the web UI's `store.js`: the snapshot replaces everything, broadcasts and agent events update it, `lagged` and a deleted workspace trigger a fresh snapshot. Browsing another workspace sends `{"type": "snapshot", "workspace": id}`, which switches this connection without changing the daemon's last used workspace.
- `core/transcript/Transcript.kt` mirrors `transcript.js`. A transcript is the saved history from `GET /api/sessions/{id}` plus the agent's events after the moment it was fetched; it is rebuilt whenever either changes, and fetched again after every snapshot. A tool call that asked for approval says what the user answered and where, such as "Allowed · Pixel 9": from `permission_resolved`, or for saved history from the transcript's `decisions`, matched by `call_id`. Requests open when the history was fetched are claimed before the events after it, so an answer given later finds its row.
- The `models` broadcast replaces the model list and the default when the daemon's configuration changes, so the composer's model picker offers a new provider at once. The picker groups models under a header per provider (the part before the first `/`).
- `core/net/DaemonConnection.kt` reconnects with backoff from 0.5 to 10 seconds, at once when the network comes back, and pings every 25 seconds to notice dead connections. After `shutdown` it keeps trying, so a restarted daemon is picked up.
