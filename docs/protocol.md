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

- **Token.** Every `/api` request needs it, as `Authorization: Bearer <token>` or a `token` query parameter. It is kept in `~/.malac/daemon.token` across restarts, so open pages reconnect; delete the file to rotate it. `--token` or `MALAC_WEB_TOKEN` override it.
- **Host check.** A request from this machine must name a loopback host (`127.0.0.1`, `localhost` or `[::1]`) in `Host`, which defeats DNS rebinding. With `web.remote` the daemon listens on all interfaces and other machines may connect with the token.
- **Origin.** WebSocket upgrades are accepted without an `Origin`, from the daemon's own origin and from VS Code webviews.

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

Connect to `ws://127.0.0.1:<port>/api/ws?token=<token>&version=<client version>`. The `version` parameter is optional; when it differs from the daemon's, the snapshot sets `daemon.version_mismatch` so the client can suggest `malac daemon restart`. Updating malac never touches a running daemon: it keeps its version until it restarts.

The first message is always a `snapshot`. Everything after it is either a broadcast (every client gets it) or a reply to one of this client's commands. A client applies broadcasts to the snapshot to stay current. If it falls behind, it gets `{"type": "lagged", "skipped": n}` and should send `snapshot` to resync.

### Requests

A command is a JSON object whose `type` names it. `client_ref` is optional; when present, the reply carries the same `client_ref`. Commands without a reply of their own are acknowledged with `{"type": "ack", "client_ref": …}` when they have a `client_ref`, and get no answer otherwise. A failed command gets `{"type": "error", "client_ref": …, "message": "…"}`. One connection's commands run one at a time, in order.

```json
{ "type": "prompt", "tab": "01a17453-…", "text": "fix the failing test", "client_ref": "c7" }
```

### Commands

#### Agents

| `type` | Fields | Reply |
|---|---|---|
| `prompt` | `text`; `agent`? (prompt this agent, resuming it if saved); `tab`? (start a new top-level agent there; default: the connection's active tab); `images`? `[{media_type, data}]` (base64); `attachments`? `[{path}]`; `model`? (for a new agent) | `ack` with `agent`, `tab`, `attachments` |
| `agent_start` | `tab`; `model`?; `label`? | `ack` with `agent`, `tab` |
| `agent_open` | `agent`; `tab`? | `ack` with `agent`, `tab`. Loads a saved or live session into a tab: the tab that lists it, else `tab`, else a tab for the session's directory, created if needed. |
| `agent_activate` | `tab`, `agent` | `ack` |
| `agent_close` | `tab`, `agent` | `ack`. Removes an idle agent from the tab; its transcript stays. |
| `cancel` | `agent` | none. Cancels its turn and its subagents'. |
| `approve` | `request_id`; `decision`: `allow`, `allow_session` or `deny` | none, or `error` when the request was already answered |
| `mode` | `mode`: `ask`, `edits` or `autopilot`; `tab`? | none. Without `tab`, every tab of the connection's workspace. |
| `model` | `agent`, `model` (`provider/model`) | none |
| `compact` | `agent` | none |

Attachments resolve against the agent's directory (`~/` and absolute paths work too). Text files become a fenced block (cut at 256 KB), PNG, JPEG, GIF and WebP images become image blocks (up to 10 MB), directories a listing of up to 200 entries, other files a note. Each one starts with an `<attachment path="…">` text block so clients can tell it from typed text. The `user_message` event carries only the typed text plus a line `[attached: a.txt, src]`. The `ack` describes each attachment:

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
| `tab_create` | `cwd`; `name`?; `workspace`?; `model`?; `mode`?; `profile`?; `no_control`?; `reuse`? | `ack` with `tab`; broadcasts `tab_created` and `workspace_updated`. The new tab becomes active. With `reuse`, an existing tab for `cwd` is activated instead. |
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

#### Jobs and the control service

| `type` | Fields | Reply |
|---|---|---|
| `job_list` | | `jobs` |
| `job_output` | `job`; `tail`? (lines, default 100, max 2000) | `job_output` |
| `job_kill` | `job` | `ack`. SIGTERM to its process group, SIGKILL after 3 s; `job_finished` follows. |
| `control_permissions` | `request`? | `control`; also broadcast. With `request`, macOS shows its Screen Recording and Accessibility prompts and System Settings opens at the missing ones. |
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
  "default_model": "lora/fast"
}
```

`workspace` is the connection's workspace: the last used one at first, `null` before any exists. `daemon.attached` is true when the server is a terminal session's own web UI (`--web` in a session that runs in its own process) rather than the daemon; workspace and tab commands are refused there. `update.installed` set means newer binaries are on disk and a restart picks them up.

#### `event`

Every agent event, tagged with its tab:

```json
{ "type": "event", "tab": "01a17453-…", "workspace": "01a17450-…",
  "envelope": { "agent": "01a17456-…", "seq": 412, "at": "…", "type": "text_delta", "text": "Done" } }
```

The envelope is the runtime's event (`crates/malac-core/src/event.rs`): `agent_started`, `model_changed`, `user_message`, `text_delta`, `thinking_delta`, `tool_call_streaming`, `assistant_message`, `tool_started`, `tool_finished`, `permission_requested`, `permission_resolved`, `todos`, `usage`, `compacted`, `notice`, `turn_finished`, `agent_finished`. `seq` increases across all tabs. Update notices (`update`) come with `tab: null`.

Approvals: `permission_requested` reaches every client; any of them may answer with `approve`, the first answer wins, and `permission_resolved` tells everyone it is settled. A client that connects later finds open requests in `snapshot.pending`.

#### Changes

| `type` | Fields | When |
|---|---|---|
| `workspace_created` | `workspace`: WorkspaceSummary | created |
| `workspace_updated` | `workspace`: WorkspaceSummary | renamed, active tab changed, tab added or closed |
| `workspace_deleted` | `workspace`: id | deleted |
| `tab_created` | `workspace`, `index`, `tab`: TabView | created |
| `tab_updated` | `workspace`, `tab`: TabView | renamed, agents added, removed or activated, mode changed, runtime loaded |
| `tab_closed` | `workspace`, `tab`: id | closed |
| `tabs_reordered` | `workspace`, `tabs`: ids in order | moved |
| `job_started`, `job_finished` | `job`: JobView | a job starts or ends |
| `control` | `control`: ControlStatus | the control service connected or its permissions were checked |
| `mode` | `mode`, `tab` (null: every tab) | a mode changed |
| `notice` | `level` (`info`, `warning`, `error`), `message`, `tab`? | e.g. a configuration warning or an unavailable MCP server while a tab loads |
| `reloaded` | | settings were applied; idle tabs reload on next use |
| `shutdown` | `reason` | the daemon is stopping (`signal`, `idle`, `stopped by a client`, …) |

Agent status changes arrive as events, not as `tab_updated`; a client keeps agent state from `agent_started`, `user_message`, `turn_finished` and friends, or refetches the snapshot.

#### Replies

| `type` | Fields |
|---|---|
| `ack` | `client_ref`, `agent`?, `tab`?, `attachments`? |
| `error` | `client_ref`, `message` |
| `pong` | `client_ref` |
| `launched` | `client_ref`, `workspace`: WorkspaceView, `tab`, `agent` |
| `workspaces` | `client_ref`, `workspaces`: [WorkspaceSummary] |
| `workspace_opened` | `client_ref`, `workspace`: WorkspaceView |
| `jobs` | `client_ref`, `jobs`: [JobView] |
| `job_output` | `client_ref`, `output`: `{job: JobInfo, text, size, truncated}` |
| `control` | `client_ref`, `control`: ControlStatus |
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
  "snapshots": [ AgentView, … ]
}
```

`agents` lists the top-level agents in order; `mode` is the mode in effect; `options` holds what the tab was opened with (`model`, `mode`, `profile`, `no_control`, all optional). `snapshots` has one AgentView per top-level agent plus the live subagents.

**AgentView** is the runtime's agent snapshot plus `live` and, for saved sessions, `title` (the first prompt):

```json
{ "id": "01a17456-…", "label": "main", "parent": null, "depth": 0, "read_only": false,
  "cwd": "/src/app", "created": "…", "model": "lora/fast", "status": "running",
  "activity": "bash cargo test", "usage": { "input_tokens": 7012, "output_tokens": 20, … },
  "report": null, "live": true }
```

`status` is `idle`, `running`, `completed`, `failed` or `cancelled`.

**PendingApproval** `{id, agent, agent_label, tool, summary, input, tab}`

**JobView** `{id, command, cwd, pid, log, started, finished, exit_code, status, agent, tab}`, `status` being `running`, `exited` (see `exit_code`), `killed` or `unknown` (it ended while no daemon watched). `tab` is the tab of the agent that started it, when known.

**ControlStatus** `{enabled, connected, tools, error, permissions}`. `permissions` is the control service's report: `{platform, screen_recording, accessibility, app: {name, path}, requested, hint}`, where the two permissions are `true`, `false` or `null` (not needed on this platform) and `app` is the app macOS grants them to.

## REST

All under the token. JSON in and out; failures are `{"error": "…"}` with status 400 (403 for `/api/fs` outside its root, 409 for a refused stop).

| Endpoint | |
|---|---|
| `GET /api/daemon` | Version, pid, start time, clients, `workspaces`, `tabs_loaded`, `agents_running`, `jobs_running`, `pending_approvals`, `idle_minutes`, `url` |
| `POST /api/daemon/stop` `{force?}` | Stops the daemon; 409 while agents or jobs run, unless `force` |
| `GET /api/snapshot?workspace=` | The snapshot, as on the WebSocket |
| `POST /api/command` | Any command above, same JSON; returns its reply (or an `ack`) |
| `GET /api/workspaces` | `{workspaces: [WorkspaceSummary]}` |
| `GET /api/jobs` | `{jobs: [JobView]}` |
| `GET /api/jobs/{id}/output?tail=` | `{job, text, size, truncated}` |
| `GET /api/fs?path=&hidden=` | Subdirectories for a directory picker: `{path, parent, root, markers, entries: [{name, path, markers}], truncated}`. `markers` names what a directory holds among `.git`, `.malac`, `AGENTS.md`, `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`. `path` is absolute, `~/…` or relative to the root; only directories below `daemon.browse_root` (default: the home directory) are listed, at most 1000. `hidden=true` includes dot directories. |
| `GET /api/sessions/{id}`, `DELETE /api/sessions/{id}` | A transcript (`meta`, `history`, `usage`); deleting also removes it from its tabs |
| `GET /api/state` | The original web UI's state: the active tab's `cwd`, `mode`, `models` and `servers`, every live agent, recent sessions, pending requests |
| `/api/settings`, `/api/models`, `/api/tools`, `/api/memory`, `/api/update`, `/api/providers/{name}/key` | Settings, as before. They act on the active tab's runtime; `POST /api/settings/apply` reloads idle tabs. |

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
- `Client::get`/`Client::post` call the REST API with the token; `Client::info` is `GET /api/daemon`.
- `Events` is an unbounded receiver of every message that is not a reply; it ends when the daemon disconnects (`shutdown` comes first when it stops on purpose).
