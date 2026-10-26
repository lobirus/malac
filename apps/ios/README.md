# malac for iOS

Watch and command a malac daemon from an iPhone or iPad: workspaces, tabs and agents with their live status, transcripts with tool calls and subagents, prompts with photos and screenshots, approvals, background jobs, remote access and keep-awake, and notifications when an agent needs you or finishes. The app is a client of the daemon's [protocol](../../docs/protocol.md), like the web UI. SwiftUI, iOS 17 or newer, Swift 5.9 or newer.

## Layout

| Path | What it is |
|---|---|
| `MalacKit/` | A Swift package with no UI, which also builds on macOS: protocol models, the state reducer, the transcript builder, Markdown blocks, line diffs, pairing, the WebSocket connection and the REST client. All tests live here. |
| `Malac/` | The app: SwiftUI views, the Keychain, notifications, background refresh. |
| `project.yml` | The Xcode project, for [XcodeGen](https://github.com/yonaskolb/XcodeGen). `Malac.xcodeproj` is generated from it and not checked in. |

The protocol fixtures in [`../fixtures`](../fixtures) were recorded from a live daemon with a scratch `MALAC_HOME` (paths rewritten to `/Users/dev`, tokens removed) and are shared with the Android app; the tests read them in place.

## Build and run

```bash
brew install xcodegen
cd apps/ios
xcodegen generate
open Malac.xcodeproj
```

Xcode 16 or newer. In the **Malac** target's Signing & Capabilities, choose your team (or set `DEVELOPMENT_TEAM` in `project.yml` and generate again) and, if `io.malac.app` is taken for your team, another bundle identifier. A free Apple ID works for your own phone; its builds expire after seven days. Connect the phone, select it as the run destination and run. The first run asks you to trust the developer in Settings → General → VPN & Device Management and, on iOS 16 and later, to turn on Developer Mode.

From the command line, with a simulator runtime installed:

```bash
xcodebuild -project Malac.xcodeproj -scheme Malac -destination 'platform=iOS Simulator,name=iPhone 16' build
```

## Tests

```bash
cd apps/ios/MalacKit
swift test
```

This runs on macOS and needs no simulator. The tests decode every message of a recorded session, the documentation's examples, the REST responses and the pairing replies; replay the session through the reducer and compare the result with the daemon's own final snapshot; follow approvals that another client answers, resync after `lagged`, and the background check's ledger; and build transcripts from saved history and from live events (a denied command, a cancelled turn, a subagent linked to its call). In Xcode the same tests run with the scheme's Test action, on a simulator.

`LiveDaemonTests` pairs with a running daemon, opens the WebSocket with the device token, sends commands, revokes itself and expects the `401`. It is skipped unless you point it at one:

```bash
malac remote pair   # note the code
MALAC_LIVE_URL=http://127.0.0.1:7700 MALAC_LIVE_CODE=ABCDE-FGHJK swift test --filter LiveDaemonTests
```

## Pairing

The daemon must be reachable from the phone: on the same network with `web.remote = true` (or `malac daemon start --remote`), through a tunnel (`malac tunnel cloudflare`, Tailscale), or through an SSH tunnel to its loopback port. Then, on the computer:

```bash
malac remote pair
```

It shows a QR code and a one-time code such as `ABCDE-FGHJK`, valid for ten minutes and one device. In the app, **Pair a daemon** scans the QR code with the camera (it is a link, `<address>/#pair=<code>`), or takes the address and the code typed in; the phone's name on the daemon is prefilled and can be changed. The app follows [remote.md](../../docs/remote.md#for-apps): `GET /api/hello` confirms the address is a malac daemon, `POST /api/pair` with `kind: "app"` trades the code for a device token of its own, and the token goes in `Authorization: Bearer` on REST calls and on the WebSocket upgrade, without an `Origin` header. A wrong or expired code (`403`) and too many wrong codes (`429`) are explained. A token you already have (an address with `?token=…`, or typed in) works too; it is checked before it is saved.

Tokens are kept in the Keychain, one item per daemon, readable after the first unlock so the background check works on a locked phone, and never synced or restored onto another device. The list of daemons (name, address, device id) is in user defaults. **Daemons** lists them, switches between them, renames and forgets them, and shows the selected daemon's version, remote access (on or off, the best address, a tunnel, the number of paired devices) and keep-awake, which can be switched between while working, always and off until the daemon restarts (`keep_awake`). Forgetting a daemon revokes this phone there (`DELETE /api/devices/{id}`) when it answers and deletes the token either way. A revoked token makes the daemon answer `401`: the app stops retrying and offers to pair again. Other refusals, such as remote access being off, are shown and retried.

App Transport Security allows plain HTTP to local network addresses only (`NSAllowsLocalNetworking`), which covers a daemon on the LAN or behind an SSH tunnel; anything else needs HTTPS, which tunnels and reverse proxies provide. iOS asks for local network access the first time the app connects to a LAN address.

## Notifications

iOS suspends apps soon after they leave the screen, so notifications work in two ways:

- **While the app runs**, and for about 25 seconds after it leaves the screen, it holds a WebSocket to the selected daemon. Approval requests and finished turns of top-level agents that arrive while it is not on screen become notifications. Approval notifications have **Allow**, **Always allow** and **Deny** actions, which answer with `POST /api/command` without opening the app (Allow asks for the device to be unlocked). A notification is withdrawn when any client answers the request.
- **In the background**, a background app refresh task (`io.malac.app.refresh`) asks every saved daemon for `GET /api/snapshot` and notifies about requests it has not reported before and agents that stopped running since the last check. iOS decides when it runs, from about every 15 minutes to a few times a day depending on how the app is used, and not at all if Background App Refresh is off. The snapshot lists pending approvals of every workspace but agents of the daemon's last used workspace only, so finished turns elsewhere are not noticed in the background.

The app asks for permission to notify after the first pairing; **Daemons** shows whether it has it.

### Remote push

Prompt notifications on a suspended app need APNs, and APNs needs a server holding the app's push key, which an open-source daemon on your own machine cannot ship. The seam is [`PushRegistrar`](Malac/App/PushRegistrar.swift), called from the app delegate with the APNs device token: a relay holding the key would receive "request R of daemon D is waiting" (ids only, no prompts, code or output) from the daemon and send the push; the app then fetches the details over its own authenticated connection, as the background check does. Adding it means the Push Notifications capability, an implementation of `PushRegistrar` that registers the token with the daemon for this device, a daemon endpoint to store it and the relay; the notification code stays as it is.

## How it works

- `MalacKit/State/DaemonState.swift` mirrors the web UI's `store.js`: the snapshot replaces everything, broadcasts and agent events update it, `lagged` and a deleted workspace ask for a fresh snapshot, and every change returns effects (resync, notice, approval requested or resolved, turn finished) that the app acts on. Browsing another workspace sends `{"type": "snapshot", "workspace": id}`, which switches this connection without changing the daemon's last used workspace. The app publishes the state at most every 50 ms, so a stream of deltas does not redraw every list.
- `MalacKit/Transcript/Transcript.swift` mirrors `transcript.js`. A transcript is the saved history from `GET /api/sessions/{id}` plus the agent's events after the moment it was fetched, or just the events for an agent this connection saw start; it is rebuilt off the main thread when either changes and fetched again after every snapshot.
- `MalacKit/Network/DaemonConnection.swift` reconnects with backoff from 0.5 to 10 seconds, at once when the phone changes networks, and pings every 20 seconds to notice dead connections. After `shutdown` it keeps trying, so a restarted daemon is picked up.
- Markdown blocks (headings, lists, code fences, tables, quotes) are parsed by MalacKit; inline emphasis, code spans and links by Foundation. Text uses the system fonts and Dynamic Type; every control has an accessibility label.
- Colours come from `web/style.css`: neutral black and white with a near-black dark mode, green for links and status only, amber and red for status. The app icon is the pig from `web/favicon.svg`, with dark and tinted variants.
