# Daemon protocol

The malac daemon (`malac daemon`, see [architecture](architecture.md#daemon)) holds every workspace, tab and agent. Front ends are clients: the web UI, the line interface (`malac --plain`), the editor extension and the terminal UI. They all speak this protocol: JSON messages over one WebSocket, plus a few REST endpoints. The Rust types are in [`crates/malac/src/protocol.rs`](../crates/malac/src/protocol.rs) and a typed client is in [`crates/malac/src/client.rs`](../crates/malac/src/client.rs).

## Finding and reaching the daemon

The daemon writes `~/.malac/daemon.json` (mode 0600) when it starts and removes it when it stops:

```json
{
  "url": "http://127.0.0.1:7700",
  "token": "8995bbb2…",
  "pid": 68119,
  "version": "0.1.2",
  "urls": ["http://127.0.0.1:7700/?token=8995bbb2…"],
  "started": "2026-10-25T18:33:10.271046Z"
}
```

A file can outlive a crashed daemon, so a client treats the daemon as running only when `pid` is alive and `GET {url}/api/daemon` answers. To start one, run `malac daemon start` (or spawn `malac daemon run` detached) and wait for the file; `client::ensure_daemon` does both. `~/.malac/daemon.lock` keeps two daemons from serving one home.

- **Local and remote.** A request is local when it comes from a loopback address, names a loopback host (`127.0.0.1`, `localhost` or `[::1]`) in `Host` and carries no proxy header (`Forwarded`, `X-Forwarded-For`, `X-Forwarded-Host`, `X-Real-IP`, `CF-Connecting-IP`). Everything else is remote: other machines, tunnels and proxies, and pages that rebind their own name to this machine. Without remote access (`web.remote`, `--remote`) remote requests get `403`, the web page included.
- **Tokens.** Every `/api` request needs a token, as `Authorization: Bearer <token>` or a `token` query parameter. The master token (`daemon.json`, kept in `~/.malac/daemon.token` across restarts; delete the file to rotate it, `--token` or `MALAC_WEB_TOKEN` override it) works on local requests only. Paired devices use their own token, local or remote; see [pairing](#pairing-and-devices). A `401` says which.
- **Origin.** WebSocket upgrades are accepted without an `Origin` (apps, the CLI), from the page's own origin (`Origin` equals `http://` or `https://` plus `Host`, or plus `X-Forwarded-Host` behind a proxy) and from VS Code webviews.

## Concepts

| Thing | Id | What it is |
|---|---|---|
| Workspace | UUIDv7 | A set of tabs, what the user calls a session. Stored in `~/.malac/workspaces/<id>.json`; `~/.malac/state.json` names the last one used. |
| Tab | UUIDv7 | A project: a directory with its own runtime (config, instructions, tools) and the top-level agents working in it. |
| Agent | UUIDv7 | A conversation, with a transcript in `~/.malac/sessions/<id>.jsonl`. Subagents have a `parent`. |
| Job | 8 hex digits | A background process an agent started (`bash` with `background: true`). Stored in `~/.malac/jobs/<id>.json` with its log. |
| Permission request | UUIDv7 | A tool call waiting for approval. |

Tabs load lazily: a tab's runtime is built (configuration read, MCP servers connected) the first time an agent in it is prompted, started or opened. After a daemon restart every agent is a saved session (`live: false`) until it is prompted, which resumes it from its transcript.

## WebSocket

Connect to `ws://127.0.0.1:<port>/api/ws?version=<client version>` with `Authorization: Bearer <token>` (`wss://` for a daemon behind TLS). Browsers, which cannot set headers on a WebSocket, pass `token=<token>` in the query instead. The `version` parameter is optional; when it differs from the daemon's, the snapshot sets `daemon.version_mismatch` so the client can suggest `malac daemon restart`. Updating malac never touches a running daemon: it keeps its version until it restarts.

The first message is always a `snapshot`. Everything after it is either a broadcast (every client gets it) or a reply to one of this client's commands. A client applies broadcasts to the snapshot to stay current. If it falls behind, it gets `{"type": "lagged", "skipped": n}` and should send `snapshot` to resync.

### Requests

A command is a JSON object whose `type` names it. `client_ref` is optional; when present, the reply carries the same `client_ref`. Commands without a reply of their own are acknowledged with `{"type": "ack", "client_ref": …}` when they have a `client_ref`, and get no answer otherwise. A failed command gets `{"type": "error", "client_ref": …, "message": "…"}`. One connection's commands run one at a time, in order, and the broadcasts a command causes (say `tab_created`) reach that connection before the command's reply.

```json
{ "type": "prompt", "tab": "01a17453-…", "text": "fix the failing test", "client_ref": "c7" }
```

### Commands

#### Agents

| `type` | Fields | Reply |
|---|---|---|
| `prompt` | `text`; `agent`? (prompt this agent, resuming it if saved); `tab`? (start a new top-level agent there; default: the connection's active tab); `images`? `[{media_type, data, name?}]` (base64; with a `name` the image shows as an attachment of that name, e.g. a pasted screenshot); `attachments`? `[{path, data?}]` (with `data`, the file itself in base64 and `path` only its name); `model`? (for a new agent) | `ack` with `agent`, `tab`, `workspace`, `attachments` |
| `agent_start` | `tab`; `model`?; `label`? | `ack` with `agent`, `tab` |
| `agent_open` | `agent`; `tab`? | `ack` with `agent`, `tab`. Loads a saved or live session into a tab: the tab that lists it, else `tab`, else a tab for the session's directory, created if needed. |
| `agent_activate` | `tab`, `agent` | `ack` |
| `agent_close` | `tab`, `agent` | `ack`; broadcasts `tab_updated`. Takes the agent out of the tab, cancelling it and its subagents if they run, and unloads it. Its transcript stays, so `agent_open` brings it back. |
| `cancel` | `agent` | none. Cancels its turn and its subagents'. |
| `approve` | `request_id`; `decision`: `allow`, `allow_session` or `deny` | none, or `error` when the request was already answered |
| `mode` | `mode`: `ask`, `edits` or `autopilot`; `tab`? | none; broadcasts `mode`. Without `tab`, every tab of the connection's workspace. |
| `model` | `agent`, `model` (`provider/model`) | none |
| `compact` | `agent` | none |

Attachments resolve against the agent's directory (`~/` and absolute paths work too). Text files become a fenced block (cut at 256 KB), PNG, JPEG, GIF and WebP images become image blocks (up to 10 MB), directories a listing of up to 200 entries, other files a note. Each one starts with an `<attachment path="…">` text block so clients can tell it from typed text. The `user_message` event carries only the typed text plus a line `[attached: a.txt, src]`.

A client whose files the daemon cannot read, such as one on another machine, sends a file's contents in `data` (base64) with `path` set to just the file's name. The daemon saves it as `<MALAC_HOME>/uploads/<uuid>/<name>`, keeping only the name's last component, and attaches it from there under that name. A file may be 20 MB decoded and the files of one prompt 40 MB together; a prompt over either fails with an error saying so, and nothing is saved. The daemon accepts WebSocket messages and frames of up to 64 MB, so a prompt at those limits fits in one frame, and `POST /api/command` takes bodies of the same size. Uploads older than 7 days are removed. Folders cannot be sent this way.

The `ack` describes each attachment:

```json
{ "type": "ack", "client_ref": "c7", "agent": "01a17456-…", "tab": "01a17453-…",
  "attachments": [{ "path": "/src/app/notes.txt", "kind": "text", "size": 27, "note": null }] }
```

`kind` is `text`, `image`, `directory` or `binary`; `size` is bytes, or entries for a directory; `note` says what was cut or left out. An attachment that cannot be read fails the whole prompt, before any agent starts.

#### Workspaces and tabs

| `type` | Fields | Reply |
|---|---|---|
| `launch` | see below | `launched` |
| `snapshot` | `workspace`? (id, id prefix or name; switches the connection to it) | `snapshot` |
| `workspace_list` | | `workspaces` |
| `workspace_create` | `name`? | `workspace_opened`; broadcasts `workspace_created` |
| `workspace_open` | `workspace` (id, unique id prefix of 4+ characters, or name) | `workspace_opened` |
| `workspace_rename` | `workspace`, `name` | `ack`; broadcasts `workspace_updated` |
| `workspace_delete` | `workspace`; `force`? | `ack`; broadcasts `workspace_deleted`. Refused while its agents run unless `force`, which cancels them. Transcripts stay. |
| `tab_create` | `cwd`; `name`?; `workspace`?; `model`?; `mode`?; `profile`?; `no_control`?; `reuse`? | `ack` with `tab` and `workspace`; broadcasts `tab_created` and `workspace_updated` first. The new tab becomes active, and its workspace (the connection's, created as `default` if there is none) becomes the connection's. With `reuse`, an existing tab for `cwd` is activated instead. |
| `tab_close` | `tab`; `force`? | `ack`; broadcasts `tab_closed`, `workspace_updated`. Refused while its agents run unless `force`. |
| `tab_rename` | `tab`, `name` | `ack`; broadcasts `tab_updated` |
| `tab_move` | `tab`, `index` | `ack`; broadcasts `tabs_reordered` |
| `tab_activate` | `tab` | `ack`; broadcasts `workspace_updated` |

Opening or creating a workspace makes it the connection's workspace (used for `prompt` without `tab`, `mode` without `tab`, and `snapshot`) and the last used one, which `malac` resumes.

`launch` does what the command line does, in one step:

```json
{ "type": "launch", "workspace": "last", "cwd": "/src/app", "name": "api",
  "prompt": "add pagination", "mode": "autopilot", "client_ref": "c1" }
```

| Field | Meaning |
|---|---|
| `workspace` | `"last"` (default; the last used workspace, created as `default` the first time), `"new"` (`-n`) or `{"open": "<name or id>"}` (`-w`) |
| `cwd` | The directory for a new tab |
| `name` | The new tab's name, or the new workspace's with `"new"`. Tab names default to the directory plus the prompt's first words. |
| `session` | Attach this session (`-r`) in the tab that holds it, else in a tab for its directory |
| `continue_last` | Attach the latest session started in `cwd` (`-c`) |
| `prompt` | Open a new tab for `cwd` and run this in a new agent there |
| `attachments` | As for `prompt` |
| `model`, `mode`, `profile`, `no_control` | Options for a new tab; `mode` also applies to an existing one |

Without `session`, `continue_last` or `prompt`, it resumes the workspace's active tab, adding a tab for `cwd` when the workspace has none (or when `name` is given). The reply:

```json
{ "type": "launched", "client_ref": "c1", "workspace": { …WorkspaceView… },
  "tab": "01a17453-…", "agent": "01a17456-…" }
```

`agent` is the agent the prompt went to, the attached session, or the tab's active agent (possibly `null` for a fresh tab).

#### Jobs, shell commands and the control service

| `type` | Fields | Reply |
|---|---|---|
| `job_list` | | `jobs` |
| `job_output` | `job`; `tail`? (lines, default 100, max 2000) | `job_output` |
| `job_kill` | `job` | `ack`. SIGTERM to its process group, SIGKILL after 3 s; `job_finished` follows. |
| `shell` | `tab`, `id` (the client's, unique while it runs), `command`; `timeout`? (seconds, default 120, max 1800) | `shell_output` once the command ends. It runs in the tab's folder on the daemon's machine, like the agent's `bash` tool: a fresh shell, stdin closed, stderr joined to stdout, the daemon's own folder first on `PATH` so `malac` is this daemon's. Over the WebSocket the connection's other commands go on meanwhile. The terminal UI's `/bash`, `!` and `/malac` use it. Any client with a token can run commands this way, as it could already approve the agent's. |
| `shell_cancel` | `id` | `ack`. Stops that `shell` command's process group; its `shell_output` says `cancelled`. |
| `control_permissions` | `request`? | `control`; also broadcast. With `request`, macOS shows its Screen Recording and Accessibility prompts and System Settings opens at the missing ones. |
| `voice_listen` | `on` | `voice_listening` with this connection's `listener` id (null after `on: false`). A connection that plays speech says so; see [voice](#voice-and-shared-files). |
| `keep_awake` | `mode`: `busy`, `always` or `off` | `ack`; broadcasts `awake` when it changes. Lasts until the daemon restarts; `daemon.keep_awake` is the default. |
| `ping` | | `pong` |

### Server messages

#### `snapshot`

```json
{
  "type": "snapshot",
  "daemon": { "version": "0.1.2", "pid": 68119, "started": "…", "version_mismatch": false,
              "clients": 2, "attached": false, "remote": false },
  "workspaces": [ WorkspaceSummary, … ],
  "workspace": WorkspaceView | null,
  "pending": [ PendingApproval, … ],
  "jobs": [ JobView, … ],
  "update": { "current": "0.1.2", "latest": null, "available": false, "installed": null, … },
  "control": ControlStatus,
  "models": ["lora/default", "lora/smart", "lora/fast"],
  "default_model": "lora/fast",
  "remote": RemoteStatus,
  "awake": AwakeStatus,
  "voice": VoiceStatus
}
```

`workspace` is the connection's workspace: the last used one at first, `null` before any exists. `daemon.attached` is true when the server is a terminal session's own web UI (`/web` in a line session running without the daemon) rather than the daemon; workspace and tab commands are refused there. `update.installed` set means newer binaries are on disk and a restart picks them up.

#### `event`

Every agent event, tagged with its tab:

```json
{ "type": "event", "tab": "01a17453-…", "workspace": "01a17450-…",
  "envelope": { "agent": "01a17456-…", "seq": 412, "at": "…", "type": "text_delta", "text": "Done" } }
```

The envelope is the runtime's event (`crates/malac-core/src/event.rs`): `agent_started`, `model_changed`, `user_message`, `text_delta`, `thinking_delta`, `tool_call_streaming`, `assistant_message`, `tool_started`, `tool_finished`, `permission_requested`, `permission_resolved`, `todos`, `usage`, `compacted`, `notice`, `turn_finished`, `agent_finished`. `seq` increases across all tabs. Update notices (`update`) come with `tab: null`.

Approvals: `permission_requested` reaches every client; any of them may answer with `approve`, the first answer wins, and `permission_resolved` tells everyone it is settled: `allowed`, `decision` (`allow`, `allow_session` or `deny`) and `by`, where it was answered (a paired device's name, `this computer` for the daemon's own machine, `talk mode`). A client that connects later finds open requests in `snapshot.pending`. The answer is kept with the session: `GET /api/sessions/{id}` lists `decisions`, so a reopened transcript shows them too. Show it on the tool card, such as "Allowed · Pixel 9" or "Denied · this computer", so the history says what the user decided.

#### Changes

| `type` | Fields | When |
|---|---|---|
| `workspace_created` | `workspace`: WorkspaceSummary | created |
| `workspace_updated` | `workspace`: WorkspaceSummary | renamed, active tab changed, tab added or closed, or its number of running agents changed |
| `workspace_deleted` | `workspace`: id | deleted |
| `tab_created` | `workspace`, `index`, `tab`: TabView | created |
| `tab_updated` | `workspace`, `tab`: TabView | renamed, agents added, removed or activated, mode changed, runtime loaded |
| `tab_closed` | `workspace`, `tab`: id | closed |
| `tabs_reordered` | `workspace`, `tabs`: ids in order | moved |
| `job_started`, `job_finished` | `job`: JobView | a job starts or ends |
| `control` | `control`: ControlStatus | the control service connected or its permissions were checked |
| `mode` | `mode`, `tab`, `workspace` | a mode changed: one tab's, or with `tab` null every tab of `workspace` |
| `clients` | `clients` | a client connected or left |
| `remote` | `remote`: RemoteStatus | a tunnel started, stopped or got its address; a device paired or was revoked |
| `awake` | `awake`: AwakeStatus | keep-awake took or released its assertion, or its mode or reason changed |
| `notice` | `level` (`info`, `warning`, `error`), `message`, `tab`? | e.g. a configuration warning or an unavailable MCP server while a tab loads |
| `models` | `models`, `default_model` | the user's configuration changed (a provider added, removed or edited, a key stored, the default model moved): pickers take the new list. Loaded tabs use new providers at once, without a reload. |
| `voice` | `voice`: VoiceStatus | voice settings changed or the providers' model lists were read again |
| `say` | `id`, `text`, `reason` (`approval`, `agent`, `finished`, `failed`), `tab`, `agent`, `listener` | a sentence to read aloud, for the connection whose listener id it names; see [voice](#voice-and-shared-files) |
| `say_cancelled` | `ids` | the user answered an approval or sent a prompt: stop these sentences at once if one is playing, and drop the rest from the queue |
| `reloaded` | | settings were applied; idle tabs reload on next use |
| `shutdown` | `reason` | the daemon is stopping (`signal`, `idle`, `stopped by a client`, …) |

Agent status changes arrive as events, not as `tab_updated`; a client keeps agent state from `agent_started`, `user_message`, `turn_finished` and friends, or refetches the snapshot.

#### Replies

| `type` | Fields |
|---|---|
| `ack` | `client_ref`, `agent`?, `tab`?, `workspace`? (the tab's), `attachments`? |
| `error` | `client_ref`, `message` |
| `pong` | `client_ref` |
| `launched` | `client_ref`, `workspace`: WorkspaceView, `tab`, `agent` |
| `workspaces` | `client_ref`, `workspaces`: [WorkspaceSummary] |
| `workspace_opened` | `client_ref`, `workspace`: WorkspaceView |
| `jobs` | `client_ref`, `jobs`: [JobView] |
| `job_output` | `client_ref`, `output`: `{job: JobInfo, text, size, truncated}` |
| `shell_output` | `client_ref`, `id`, `output` (cut from the middle past 40,000 characters), `exit_code`? (absent when stopped or ended by a signal), `timed_out`, `cancelled`. Only the client that sent the `shell` gets it. |
| `control` | `client_ref`, `control`: ControlStatus |
| `voice_listening` | `client_ref`, `listener`: number or null |
| `snapshot` | `client_ref` plus the snapshot fields |

### Types

**WorkspaceSummary** `{id, name, tabs: number, active_tab, running: agents working now, created, updated}`

**WorkspaceView** `{id, name, active_tab, created, updated, tabs: [TabView]}`

**TabView**

```json
{
  "id": "01a17453-…", "name": "api", "cwd": "/src/app",
  "agents": ["01a17453-…", "01a17456-…"],
  "active_agent": "01a17456-…",
  "mode": "ask",
  "options": { "model": "lora/smart", "mode": "ask" },
  "loaded": true,
  "snapshots": [ AgentView, … ],
  "project": ProjectView
}
```

`agents` lists the top-level agents in order; `mode` is the mode in effect; `options` holds what the tab was opened with (`model`, `mode`, `profile`, `no_control`, all optional). `snapshots` has one AgentView per top-level agent plus the live subagents. `project` is there once the tab is loaded.

**ProjectView** says where the tab sits in its repository and which custom slash commands it has:

```json
{ "root": "/src/shop", "subdir": "frontend", "siblings": ["backend", "infra"],
  "suggest_root": "/src/shop",
  "commands": [ { "name": "ponytail:ponytail-review", "description": "Quality review of a change…",
                  "source": "skill" },
                { "name": "release-tools:release-notes", "description": "Draft release notes for a version",
                  "argument_hint": "<version>", "source": "plugin release-tools@malac-examples" } ] }
```

`subdir`, `siblings` and `suggest_root` are present only when the tab's directory is below the repository root. `suggest_root` is set when the repository has other parts that are projects of their own (`backend/` next to `frontend/`): show something like "This is part of a larger repository: open /src/shop to work across all of it", whose action is `tab_create` with that `cwd`. `commands` are what `/name args` runs besides the built-in commands, for completion and `/help`: plugin commands (`plugin:name`), command files and skills; `source` is `user`, `project`, `plugin <id>` or `skill`. Send them as ordinary `prompt` text; the agent expands them, and answers `/context` itself without a model call.

**AgentView** is the runtime's agent snapshot plus `live` and, for saved sessions, `title` (the first prompt):

```json
{ "id": "01a17456-…", "label": "main", "parent": null, "depth": 0, "read_only": false,
  "cwd": "/src/app", "created": "…", "model": "lora/fast", "status": "running",
  "activity": "bash cargo test", "usage": { "input_tokens": 7012, "output_tokens": 20, … },
  "report": null, "live": true }
```

`status` is `idle`, `running`, `completed`, `failed` or `cancelled`.

**PendingApproval** `{id, call_id, agent, agent_label, tool, summary, input, tab}`; `call_id` matches the `tool_use` block and the event's `call_id`.

**JobView** `{id, command, cwd, pid, log, started, finished, exit_code, status, agent, tab}`, `status` being `running`, `exited` (see `exit_code`), `killed` or `unknown` (it ended while no daemon watched). `tab` is the tab of the agent that started it, when known.

**RemoteStatus** `{enabled, listen, urls: [{kind, url}], tunnel: TunnelStatus | null, devices}`. `enabled` is whether remote access is on; `listen` the bound address (`0.0.0.0:7700`); `urls` the addresses other devices can use, best first, `kind` being `tunnel`, `public` (`remote.public_url`), `lan` (this machine's network address) or `local` (loopback, for SSH tunnels); `devices` the number of paired devices.

**TunnelStatus** `{provider, state, url, error, waiting, notes, started}`: `provider` is `cloudflare`, `ngrok`, `tailscale` or `tailscale-funnel`; `state` is `starting`, `running` (with `url`), `failed` (with `error`, the tool's last lines) or `stopped`; `waiting`, while starting, is what the tool printed so far, such as `tailscale serve` asking to turn on Serve for the tailnet at a link it then waits for; `notes` are what to know about that kind of tunnel.

**AwakeStatus** `{mode, awake, reason, method, error, since}`: `mode` is `busy`, `always` or `off`; `awake` whether a power assertion is held; `reason` why, for people (`2 agents running`, `nothing is running`, `idle only: …`); `method` `caffeinate` or `systemd-inhibit` (null where neither exists); `error` why taking it failed; `since` when `awake` last changed.

**VoiceStatus** `{dictation, speech, talk, language, voices, checked}`, each feature being `{enabled, model, configured, voice, models}`: `enabled` is the setting, `model` the model in use (`configured` when the settings name one, else the first of `models`; null when nothing can do it), `voice` the voice for speech and talk, `models` every `provider/model` that can do it. A feature is ready when `enabled` and `model` is set; a client shows its button only then. `voices` are the voices speech and talk models know; `checked` is when the providers' model lists were last read.

**ControlStatus** `{enabled, connected, tools, error, permissions}`. `permissions` is the control service's report: `{platform, screen_recording, accessibility, app: {name, path}, requested, hint}`, where the two permissions are `true`, `false` or `null` (not needed on this platform) and `app` is the app macOS grants them to.

## REST

All under a token except `GET /api/hello` and `POST /api/pair`. JSON in and out; failures are `{"error": "…"}` with status 400 (403 for `/api/fs` and `/api/files` outside their root, 409 for a refused stop).

| Endpoint | |
|---|---|
| `GET /api/daemon` | Version, pid, start time, clients, `workspaces`, `tabs_loaded`, `agents_running`, `jobs_running`, `pending_approvals`, `idle_minutes`, `url`, `remote_access` (RemoteStatus), `awake` (AwakeStatus) |
| `POST /api/daemon/stop` `{force?}` | Stops the daemon; 409 while agents or jobs run, unless `force` |
| `GET /api/snapshot?workspace=` | The snapshot, as on the WebSocket |
| `POST /api/command` | Any command above, same JSON; returns its reply (or an `ack`) |
| `GET /api/workspaces` | `{workspaces: [WorkspaceSummary]}` |
| `GET /api/jobs` | `{jobs: [JobView]}` |
| `GET /api/jobs/{id}/output?tail=` | `{job, text, size, truncated}` |
| `GET /api/files?path=` | Every file below a directory for `@` pickers: `{root, files, truncated}`. `files` are paths relative to `root`, sorted, with hidden files and what `.gitignore` excludes left out (inside and outside repositories), at most 50,000 (`truncated` says when there were more). `path` and its limits are as for `/api/fs`. |
| `GET /api/fs?path=&hidden=&files=` | A directory's contents for folder and file pickers: `{path, parent, root, markers, entries: [{name, path, kind, size?, markers}], truncated}`. Entries are its subdirectories (`kind: "directory"`) and, with `files=true`, its files (`kind: "file"`, `size` in bytes) after them. `markers` names what a directory holds among `.git`, `.malac`, `AGENTS.md`, `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`. `path` is absolute, `~/…` or relative to the root; only what is below `daemon.browse_root` (default: the home directory) is listed, at most 1000 entries. `hidden=true` includes dot entries. |
| `GET /api/sessions?cwd=&limit=&subagents=` | Saved sessions, newest first: `{sessions: [{id, label, parent, cwd, model, created, title, updated}]}`. `cwd` keeps those started in that directory; `limit` defaults to 50; `subagents=true` includes subagent sessions. `title` is the first prompt's typed text. Clients use this rather than reading `~/.malac/sessions`, so it works with a daemon on another machine. |
| `GET /api/sessions/{id}`, `DELETE /api/sessions/{id}` | A transcript (`meta`, `history`, `usage`, `decisions`: `[{call_id, tool, summary, decision, by, at}]`, the approvals asked for and their answers); deleting also removes it from its tabs |
| `GET /api/usage?days=` | Token and cost totals from the session transcripts, see [below](#usage). Read-only. |
| `GET /api/state` | The original web UI's state: the active tab's `cwd`, `mode`, `models` and `servers`, every live agent, recent sessions, pending requests |
| `/api/settings`, `/api/models`, `/api/tools`, `/api/memory`, `/api/update`, `/api/providers/{name}/key` | Settings, as before. They act on the active tab's runtime; `POST /api/settings/apply` reloads idle tabs. `GET /api/settings` includes `context`, a ContextReport. |

**ContextReport** is what `/context`, `malac context --json` and `malac_settings` (`show: "context"`) report:

```json
{
  "layout": { "root": "/src/shop", "subdir": "frontend", "siblings": ["backend"], "suggest_root": "/src/shop" },
  "instructions": {
    "loaded": [ { "path": "/src/shop/AGENTS.md", "kind": "AGENTS.md", "chars": 263, "truncated": false } ],
    "rules": [ { "path": "/src/shop/.cursor/rules/go.mdc", "kind": "Cursor rule", "description": "Go conventions", "globs": ["backend/**/*.go"] } ],
    "skipped": [ { "path": "/src/shop/CLAUDE.md", "reason": "same text as /src/shop/AGENTS.md" } ]
  },
  "docs": [ { "path": "README.md", "title": "Shop", "summary": "A small web shop." } ],
  "index": { "path": "/src/shop/.malac/index.md", "exists": true, "updated": "…", "files": 412 },
  "knowledge": { "path": "/src/shop/.malac/MALAC.md", "exists": true, "chars": 1200 },
  "skills": [ { "name": "ponytail:ponytail-review", "description": "…", "path": "…/SKILL.md", "source": "plugin ponytail@malac",
                "model_invocable": true, "user_invocable": true } ],
  "commands": [ CommandInfo, … ],
  "agent_types": [ { "name": "release-tools:release-reviewer", "description": "…", "model": null, "tools": ["read", "grep"], "source": "plugin release-tools@malac-examples" } ],
  "plugins": [ PluginInfo, … ],
  "warnings": [ "plugin guard@team: hooks not approved, so they do not run; `malac plugin enable guard@team` reviews them" ]
}
```

`index` is absent outside a repository or with `[context] index = false`. **PluginInfo** (also `malac plugin list --json`):

```json
{ "id": "ponytail@ponytail", "name": "ponytail", "marketplace": "ponytail", "version": "5.3.2",
  "description": "…", "enabled": true, "scope": "user", "installed": true, "path": "…",
  "formats": ["claude-code"], "skills": ["ponytail", "ponytail-review"], "commands": ["ponytail-review"],
  "agents": [], "mcp": [], "hooks": ["SessionStart", "UserPromptSubmit"], "scripts": true,
  "approved": ["hooks", "scripts"], "pending": [], "unsupported": ["userConfig"], "warnings": [] }
```

`scope` is `user`, `project` or `builtin` (which file turns it on or off), absent when no file mentions it; `pending` names hooks or MCP servers that are not approved and so do not run; `error`, when present, says why a listed plugin could not be loaded (for example, enabled by the project but not installed here). Optional fields (`argument_hint`, `version`, `description`, `scope`, `error`) are left out when empty.

### Pairing and devices

| Endpoint | |
|---|---|
| `GET /api/hello` | No token. `{app: "malac", version, remote, pairing: true}`, so a client can tell it found a daemon. |
| `POST /api/pair` `{code, name, kind?}` | No token. Trades a pairing code for a device token: `{token, device: DeviceView, version}`. `kind` is `cli`, `browser`, `app` or anything short (default `client`). `403` for a wrong, used or expired code; `429` after ten wrong codes, until a new code is made. |
| `POST /api/pair/codes` | A new code: `{code: "ABCDE-FGHJK", expires, links: [{kind, base, url, svg}]}`, valid ten minutes for one device. `links` has one entry per address in RemoteStatus `urls`: `url` is `<base>/#pair=<code>`, `svg` its QR code. |
| `GET /api/devices` | `{devices: [DeviceView]}`, oldest first |
| `DELETE /api/devices/{id}` | `{revoked: DeviceView}`. The token stops working and the device's WebSockets close; broadcasts `remote`. |
| `GET /api/remote` | RemoteStatus |
| `POST /api/tunnel` `{provider}` | Starts a tunnel (replacing a running one): TunnelStatus, usually `starting`; `remote` broadcasts follow. Refused without remote access, or with an install hint when the tool is missing. |
| `DELETE /api/tunnel` | `{stopped: bool}` |

**DeviceView** `{id, name, kind, created, last_seen, address, current}`: `address` is where it last connected from (the client's address a proxy forwarded, for display); `current` marks the device making the request.

A code is ten characters of Crockford base32, shown as `XXXXX-XXXXX`; case, dashes and spaces do not matter, and `O`, `I` and `L` read as `0`, `1`, `1`. Device tokens are stored as SHA-256 hashes in `~/.malac/devices.json`. [remote.md](remote.md#for-apps) walks through pairing from an app.

### Push

For paired devices; see [remote.md](remote.md#push-notifications).

| Endpoint | |
|---|---|
| `GET /api/push` | `{enabled, wait_seconds, vapid_key, registered}`: whether the daemon pushes, how long a request waits first, its VAPID public key (uncompressed P-256, base64url, for the distributor) and whether the calling device is registered. |
| `PUT /api/push` `{endpoint, p256dh, auth}` | Registers (or replaces) the calling device's Web Push endpoint, with its P-256 public key and 16-byte auth secret, both base64url. `400` for the master token: only paired devices get pushes. |
| `DELETE /api/push` | `{registered: false, removed}` |

Each push is a Web Push message (RFC 8030): `aes128gcm`-encrypted for the device (RFC 8291), with `Authorization: vapid t=<ES256 token>, k=<key>` (RFC 8292), `TTL: 3600`, `Urgency: high` and `Topic: malac`, so an offline device gets only the latest. Decrypted, it is JSON:

- `{"type": "approvals", "requests": [...], "more": true?}`: requests that waited `wait_seconds` unanswered. Each is `{id, agent, agent_label, tool, summary, tab, place}` (summary cut to 160 characters, `place` the tab's and workspace's names), or `{id, agent}` once the details no longer fit in one push. `more` means some were left out: fetch the snapshot.
- `{"type": "resolved", "requests": [id]}`: a request a push went out for was answered.

### Voice and shared files

| Endpoint | |
|---|---|
| `GET /api/voice?refresh=` | VoiceStatus. `refresh=true` reads the providers' model lists again first. |
| `PUT /api/voice` | Changes `[voice]` settings in `~/.malac/config.toml`: any of `dictation`, `dictation_model`, `language`, `speech`, `speech_model`, `speech_voice`, `talk`, `talk_model`, `talk_voice`; `null` or `""` removes one. Returns VoiceStatus and broadcasts `voice`. |
| `POST /api/voice/transcribe?language=` | Dictation. The body is the recording, its type in `Content-Type` (`audio/wav`, `audio/webm`, `audio/mp4`, `audio/ogg`, `audio/mpeg`; up to 25 MB). Send 16 kHz mono 16-bit WAV: every model takes it, chat models that listen take nothing else. `{text, model}`. |
| `POST /api/voice/speak` `{text}` | The text read aloud with the speech model, as audio (`audio/mpeg` from a model, `audio/wav` from the system voice), for "read aloud" buttons. |
| `GET /api/voice/say/{id}` | The audio of a `say` broadcast, made once and kept ten minutes. |
| `GET /api/voice/talk?tab=&agent=` | Talk mode's WebSocket, below. |
| `GET /api/shared/{id}?download=` | A file an agent sent with `send_to_user`, with its type (HTML and SVG come back as plain text unless `download=true`). `download=true` asks the browser to save it. |

**Models.** Only OpenAI-style providers have audio APIs. Dictation uses a transcription model (`/audio/transcriptions`, like `whisper-1` or `gpt-4o-mini-transcribe`), else a chat model that takes audio (`input_audio`, like `gpt-4o-audio-preview`). Speech uses a speech model (`/audio/speech`, like `gpt-4o-mini-tts` or `tts-1`), else `system`: the machine's own voice (`say` on macOS, `espeak-ng` on Linux). Talk mode needs a Realtime model (`gpt-realtime`). Models are recognised by name; `[models."provider/model"] audio = "transcription" | "input" | "speech" | "realtime"` declares one whose name does not say.

**Speech.** With `voice.speech` on, the daemon reads aloud what needs the user: every approval request ("malac needs your approval to run cargo publish."), what an agent says with its `say` tool, a failed turn, and the first sentence of a top-level agent's reply after a turn of 30 seconds or more in which it said nothing. Only connections that sent `voice_listen {on: true}` play speech, and each sentence goes to one of them: the one that sent a command most recently, which is where the user is. A `say` broadcast reaches every client; the one whose `listener` matches fetches `/api/voice/say/{id}` and plays it. Clients offer a "speak on this device" switch and send `voice_listen` again after reconnecting. While a talk mode session runs, its voice model speaks for malac and no `say` is sent.

**Quiet when the user acts.** Answering an approval (on any device) or prompting a top-level agent broadcasts `say_cancelled` with the sentences sent in the last two minutes: a client stops the one playing and drops the queued ones at once. While an approval read aloud is unanswered, and for 20 seconds after the user answers one, further approvals are not read aloud: the user is at a screen, or already knows malac is waiting.

**Shared files.** `send_to_user` copies files (or the newest images the agent's tools returned, such as a screenshot) to `~/.malac/shared/<id>/` for 30 days. Its tool result names each one in a tag, after an optional `<caption>…</caption>` (captions never contain `<` or `>`, names never `<`, `>` or `"`):

```
Sent to the user: Screenshot 2026-10-27 14.02.11.jpg.
<caption>The settings page after the fix</caption>
<shared id="0193f0c2…" name="Screenshot 2026-10-27 14.02.11.jpg" type="image/jpeg" size="84211"/>
```

Clients find the tags in the `tool_finished` output (or the tool result in a transcript), show images inline from `/api/shared/{id}` with a full-screen viewer that zooms, and other files as a card that opens or downloads them.

**Talk mode.** Open `/api/voice/talk` (token in the query for browsers) to talk through the tab's active agent (or `tab` and `agent`). Both directions carry audio as binary frames of 16-bit little-endian mono PCM at 24 kHz; send about 100 ms per frame. Text frames are JSON:

| From the client | |
|---|---|
| `{"type": "text", "text"}` | Typed input, as if spoken |
| `{"type": "interrupt"}` | Stop the current reply |
| `{"type": "stop"}` | End talk mode |

| From the daemon | |
|---|---|
| `{"type": "ready", "model", "voice"}` | Start streaming the microphone |
| `{"type": "listening"}` | The user started speaking: stop playing at once |
| `{"type": "thinking"}` | The user stopped speaking |
| `{"type": "transcript", "role", "text", "final"}` | What the user (`final` only, when the provider transcribes) or the voice said; assistant deltas have `final: false` |
| `{"type": "tool", "name", "summary"}` | The voice model asked malac something, checked on it, answered an approval or stopped it |
| `{"type": "done"}` | A reply finished |
| `{"type": "error", "message"}`, `{"type": "closed", "reason"}` | Something failed; the conversation ended |

The voice model works through malac with four tools: `ask_malac` prompts the agent (the prompt shows in every client's transcript), `malac_status`, `answer_approval` and `stop_malac`. When the agent finishes a turn or asks for approval, the voice model is told and tells the user. Without echo cancellation (a terminal on speakers), pause the microphone while audio plays.

### Usage

`GET /api/usage?days=30` adds up the `usage` records of every transcript in `~/.malac/sessions`, subagents included, over the last `days` local days (today included; default 30, at most 366). The editor extension's Usage view calls it with `days=7`.

```json
{
  "days": 7, "since": "2026-10-20",
  "today": { "input_tokens": 51200, "output_tokens": 1830, "cache_read_tokens": 0, "cache_write_tokens": 0, "cost": 0.012, "sessions": 3 },
  "week":  { …the last 7 days, today included… },
  "total": { …the whole window… },
  "by_model": [{ "model": "lora/default", "input_tokens": 51200, …, "sessions": 3 }],
  "by_day":   [{ "date": "2026-10-26", "input_tokens": 51200, …, "sessions": 3 }]
}
```

Each bucket is a [Usage](../crates/malac-core/src/message.rs) plus `sessions`, the number of transcripts in it. `by_model` is sorted by tokens, largest first; `by_day` by date and lists only days with usage. Usage records carry no timestamp or model, so a transcript counts on the day it was last written, under the model it started with. Only the metadata and usage lines of each file are decoded.

## The original web UI

The web UI predates workspaces. It keeps working because the daemon still accepts its messages and their meaning carries over:

- `prompt` with `agent: null` starts a new agent in the active tab of the connection's workspace (creating a tab for the home directory when there is none); with an `agent`, it prompts or resumes that agent wherever it lives.
- `resume {session}` is `agent_open`; `cancel`, `approve`, `mode`, `model` and `compact` are unchanged; `mode` without `tab` changes every tab of the workspace.
- It ignores the new message types, and `event` messages keep their shape with an added `tab` and `workspace`.

`malac web --cwd <dir>` (what the editor extension runs) adds or activates a tab for `<dir>` first, so the embedded UI works in that project.

## Rust client

```rust
use malac::client::Client;
use malac::protocol::{Command, Launch, ServerMessage};

let (client, mut events, snapshot) = Client::connect_or_start(&Default::default()).await?;
if snapshot.daemon.version_mismatch { /* suggest `malac daemon restart` */ }
let launched = client.launch(Launch { cwd: std::env::current_dir()?, ..Default::default() }).await?;
let tab = launched.tab.unwrap();
client.request(Command::Prompt {
    tab: Some(tab.clone()), agent: launched.agent.clone(), text: "hello".into(),
    images: vec![], attachments: vec![], model: None,
}).await?;
while let Some(message) = events.recv().await {
    match message {
        ServerMessage::Event { tab: Some(t), envelope, .. } if t == tab => { /* render */ }
        ServerMessage::Lagged { .. } => { /* client.request(Command::Snapshot { workspace: None }) */ }
        _ => {}
    }
}
```

- `client::running()` returns the live daemon's `DaemonFile`, if any; `client::ensure_daemon(&ServeArgs)` starts one first; `Client::connect(&file)` connects; `Client::connect_or_start` does all three.
- `Client::request` sends a command with a fresh `client_ref` and resolves with its reply; an `error` reply becomes `Err`. `Client::send` does not wait.
- `Client::launch` wraps `launch`; `RunArgs::launch(cwd, prompt)` builds the `Launch` from the command line (`-n`, `-w`, `--name`, `-c`, `-r`, `-m`, `--mode`, `-y`, `--profile`, `--no-control`).
- `Client::get`/`Client::post`/`Client::delete` call the REST API with the token; `Client::info` is `GET /api/daemon`.
- A daemon elsewhere: `DaemonFile::remote(name, url, token)` describes it, and `connections::resolve(Some(name))` returns the saved connection as an `Endpoint` whose `connect()` connects (`local` and `None` are this machine's daemon, started if needed).
- `Events` is an unbounded receiver of every message that is not a reply; it ends when the daemon disconnects (`shutdown` comes first when it stops on purpose).
