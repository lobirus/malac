# Remote access

Run the agents on a server and drive them from your laptop, phone or another browser. The daemon on the server holds the workspaces, tabs, agents and jobs as always; every other machine is a client that pairs once and then connects like a local one.

```bash
# on the server
malac daemon start --remote
malac remote pair                    # a one-time code and a QR code

# on your laptop
malac remote add server http://192.168.1.20:7700 --pair ABCDE-FGHJK
malac -C server                      # the full-screen UI, on the server's daemon
malac -C server --plain              # the line interface
malac -C server "fix the flaky test" # a prompt
```

On a phone, scan the QR code: it opens the web UI at the server's address with the code filled in.

## Turning it on

Remote access is off by default. Then the daemon serves only requests that come from its own machine and name a loopback host (`127.0.0.1`, `localhost`, `[::1]`) without proxy headers; everything else gets `403` before it reaches the API, the web page included.

`web.remote = true`, or `malac daemon start --remote` (also `restart` and `run`), lets paired devices in. The daemon then listens on every interface unless `web.host` or `--host` names an address; `--host 127.0.0.1` keeps it off the network, for SSH and tunnels only. Settings → Remote access in the web UI and `malac remote` show whether it is on, where the daemon listens and how to reach it.

`malac daemon restart`, and the terminal UI's restart after an update, keep remote access, the address and a running tunnel, so paired phones carry on. To change them, pass flags to `restart`, or `malac daemon stop` and then `start`, which applies the configuration afresh.

## Pairing and devices

The master token in `~/.malac/daemon.json` belongs to the daemon's machine: it is accepted only on local requests, never through the network or a tunnel. Every other client gets a device token of its own by pairing:

1. On the daemon's machine, `malac remote pair` (or Settings → Remote access → Show a pairing code, or Connection → Pair a device in the terminal UI) creates a code like `ABCDE-FGHJK`. It is valid for ten minutes and for one device. The command prints it with a QR code of the best address (a running tunnel, `remote.public_url`, the LAN address, then the loopback one) and the `malac remote add` line for each address.
2. The device trades the code for its token: `malac remote add <name> <url> --pair <code>` on a computer, the pairing form in a browser (the QR link fills it in), or `POST /api/pair` from an app.
3. The device sends that token like the master token from then on.

Codes are ten characters of Crockford base32 (50 random bits); typing is forgiving about case, dashes and `O`/`0`, `I`/`L`/`1`. After ten wrong codes every open code is withdrawn until a new one is made, so guessing is out of reach even on a public address.

Devices are listed with `malac remote devices` (name, kind, when paired, last seen and from where) and in Settings and the terminal UI's Remote access dialog. `malac remote revoke <id>`, or Revoke there, removes one: its token stops working at once and its open connections close. A revoked browser returns to the pairing form. Device tokens are stored as SHA-256 hashes in `~/.malac/devices.json` (mode 0600).

Pairing works with remote access off too, for SSH tunnels and other local clients.

## Saved connections

`malac remote add <name> <url> --pair <code>` saves the daemon's address and the device token in `~/.malac/connections.json` (mode 0600). The URL may be a whole pairing link (`…/#pair=CODE`), which carries the code. `--token <token>` saves a token you already have; `add` with an existing name and neither flag only changes its address, for example after a quick tunnel restarted with a new one.

| Command | |
|---|---|
| `malac remote list [--json]` | Saved connections and whether each answers |
| `malac remote remove <name>` | Forget one, revoking its token when the daemon answers |
| `malac -C <name>`, `--connect <name>` | Use that daemon for the full-screen UI, `--plain`, `-p` and `malac <prompt>`; `local` is this machine's |

Paths name the daemon's machine: `--cwd` is a directory there, and without it a new tab opens in its home directory. `/cd ~/src/app` in the terminal UI works the same way. Models, keys and MCP servers are the server's configuration; the client needs none. Pasted images travel with the prompt; files do not yet: `@` and dropped files work only with this machine's daemon, and the folder browser points to `/cd <folder>` instead.

### In the terminal UI

The **Connection** menu lists this computer and every saved connection; choosing one switches the whole interface to that daemon, opening the workspace it used last. The choice is remembered: a plain `malac` reopens it next time (falling back to this computer, with a note, when it does not answer). `-C local` goes back to this machine. The header shows `server › workspace`, the status line `@server`, and the workspace switcher (Session → Switch workspace) lists the workspaces of every daemon under its name, so choosing one on another daemon switches to it.

Connection → Remote access shows the daemon you are on: whether remote access is allowed, its addresses, the tunnel, keep-awake, the paired devices (↑↓ and `x` revokes) and how to connect; `p` shows a pairing code with its QR code.

### In a browser

Open the daemon's address. Without a token the web UI shows the pairing form; the QR code's link fills in the code. The browser keeps its device token in local storage, so it stays paired until you revoke it. On the daemon's machine, `malac web` still prints the local address with the master token.

## Ways to connect

| Way | How | Notes |
|---|---|---|
| Same network | `malac daemon start --remote`, then use `http://<lan address>:7700` | Plain HTTP: fine at home, not on a café network |
| SSH tunnel | `ssh -N -L 7700:127.0.0.1:7700 server`, then use `http://127.0.0.1:7700` | Encrypted, nothing exposed; works with remote access off, since the request looks local to the daemon |
| Cloudflare Tunnel | `malac tunnel cloudflare` | A public `https://….trycloudflare.com` address, no account; a new address on every start |
| ngrok | `malac tunnel ngrok` | Needs a free account and `ngrok config add-authtoken`; free addresses show a warning page in browsers first |
| Tailscale | `malac tunnel tailscale` or `tailscale-funnel` | `serve` reaches only your tailnet over WireGuard; `funnel` is public |
| Your own proxy | Put a TLS reverse proxy in front and set `remote.public_url` | The address is offered in pairing links |

## Tunnels

`malac tunnel <provider>` asks the daemon to run the tool as its child, waits for the public address and prints it with what to know about it. The daemon stops the tunnel when it stops, or with `malac tunnel stop`, and starts it again after a restart; `malac tunnel status` shows it. A tool that waits for you, such as `tailscale serve` on a tailnet where Serve is off, shows what it asks for, with its link, until it is done. `[remote] tunnel = "cloudflare"` starts one with the daemon. If the daemon is not running, `malac tunnel` starts it for tunnels only: remote access on, listening on loopback.

| Provider | Runs |
|---|---|
| `cloudflare` | `cloudflared tunnel --no-autoupdate --url http://127.0.0.1:<port>` |
| `ngrok` | `ngrok http <port> --log=stdout --log-format=json` |
| `tailscale` | `tailscale serve http://127.0.0.1:<port>` |
| `tailscale-funnel` | `tailscale funnel http://127.0.0.1:<port>` |

malac never installs these tools. When one is missing it says how: `brew install cloudflared` on macOS, the static binary in `~/bin` on Linux, the ngrok and Tailscale downloads. It looks on `PATH` and in `~/bin`, `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin` and the Tailscale app.

Tunnels need remote access: what comes through one has a public `Host` and proxy headers, so the daemon treats it as remote and refuses it otherwise. Through a tunnel only pairing and device tokens work.

A malac.io relay is planned as another provider: the daemon would connect out to the relay, which forwards requests the same way, so pairing, device tokens and the status messages stay as they are.

## Security model

- **Local or remote.** A request is local only when it comes from a loopback address, names a loopback host and carries no proxy header (`Forwarded`, `X-Forwarded-For`, `X-Forwarded-Host`, `X-Real-IP`, `CF-Connecting-IP`). Everything else is remote: other machines, tunnels and proxies, and pages that resolve their own name to this machine (DNS rebinding). With remote access off, remote requests are refused outright.
- **Tokens.** The master token works on local requests only. Device tokens work from anywhere remote access allows, can be revoked one by one and are stored hashed. Only `GET /api/hello` and `POST /api/pair` answer without a token. Tokens are compared in constant time.
- **Browsers.** WebSocket upgrades from a browser must come from the page's own origin (`Origin` matching `Host`, or `X-Forwarded-Host` behind a proxy), or from VS Code webviews. Clients that are not browsers send no `Origin`. REST calls need the token in a header, which pages on other origins cannot send without a CORS grant the daemon never gives. The web UI sends its token in the WebSocket URL, as browsers cannot set headers there; the Rust client and apps send it in `Authorization`.
- **Transport.** The daemon speaks plain HTTP. On a LAN anyone who can watch the traffic can take a device token, so prefer SSH, Tailscale or a TLS proxy outside networks you trust. Cloudflare and ngrok terminate TLS at their edge and can see the traffic; Tailscale and SSH are end to end.
- **What a device can do.** A paired device controls the daemon fully, as you can: it runs prompts, answers approvals, changes settings, pairs and revokes devices and starts tunnels. Pair only devices you own and revoke the ones you lose.
- **Project files.** A repository's `.malac/config.toml` cannot set `web`, `daemon` or `remote`, so cloning one never opens remote access or starts a tunnel.

## Keeping the machine awake

The daemon keeps the machine from sleeping while work runs, like Amphetamine:

| `daemon.keep_awake` | |
|---|---|
| `busy` (default) | Hold a power assertion while agents or jobs run, and release it a minute after the last one ends |
| `always` | Hold it as long as the daemon runs |
| `off` | Never |

On macOS the daemon runs `caffeinate -i -s -w <daemon pid>`, which prevents idle sleep (and system sleep on AC power) and exits on its own if the daemon dies. On Linux it runs `systemd-inhibit --what=idle:sleep --mode=block` around a shell that waits for the daemon, when `systemd-inhibit` is installed; logind may refuse it for sessions that are not active, which the status reports. Elsewhere nothing happens.

The snapshot, `malac daemon status`, the web UI (Settings → Remote access) and the terminal UI (an `awake` segment in the status line, and View → Keep awake) show whether the assertion is held and why. The View menu and the web UI change the mode until the daemon restarts; `daemon.keep_awake` sets it for every start.

Limits: closing a MacBook's lid still puts it to sleep, unless it is in clamshell mode (on power, with an external display and a keyboard or mouse). Low battery, a forced sleep from the Apple menu and `pmset` schedules win too. A desktop's screen still turns off; the assertion keeps the system awake, not the display.

## For apps

Mobile apps pair the same way the CLI does:

1. Scan the QR code. It is a URL: `<daemon address>/#pair=<code>`. Take the base address and the code from it, or let the user type both.
2. `GET <base>/api/hello` checks that it is a malac daemon: `{"app": "malac", "version": "1.2.3", "remote": true, "pairing": true}`.
3. `POST <base>/api/pair` with `{"code": "ABCDE-FGHJK", "name": "Balint's iPhone", "kind": "app"}` returns `{"token", "device": {id, name, kind, created, …}, "version"}`. A wrong, used or expired code is `403`; too many wrong codes are `429`.
4. Keep the token in the platform's keychain and send it as `Authorization: Bearer <token>` on REST calls and on the WebSocket upgrade (`<base>/api/ws`, `wss://` for `https://`), without an `Origin` header.
5. A `401` means the device was revoked: pair again. `GET /api/devices` marks the app's own device with `current: true`; deleting it unpairs.

The WebSocket protocol, the snapshot (with `remote` and `awake`) and every command are in [protocol.md](protocol.md).
