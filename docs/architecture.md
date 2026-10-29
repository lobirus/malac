# Architecture

malac is a Cargo workspace with four crates:

| Crate | Role |
|---|---|
| `malac-core` | The runtime: providers, tools, agent loop, permissions, memory, sessions, MCP client wiring. No UI. |
| `malac` | The `malac` binary and library: CLI, the daemon (`daemon/`), its protocol and client, full-screen terminal UI (`tui/`), line interface, web server. |
| `malac-control` | Computer and browser control, served over MCP (stdio or WebSocket). Also a library. |
| `malac-mcp` | A small Model Context Protocol implementation (client and server) shared by the others. |

Front ends never talk to models or tools directly. Either they create a `Runtime` in their own process, spawn agents on it and render the event stream (`-p`, `--json` and piped runs), or they are clients of the daemon, which holds the runtimes for them (the full-screen UI, the web UI, the editor extension, `--plain`).

## Runtime

`Runtime` (`crates/malac-core/src/runtime.rs`) holds what every agent shares:

- the merged `Config`
- a provider cache (`provider/`), built lazily per provider name
- the tool list: built-in tools plus tools from connected MCP servers
- `Memory`, the permission `Policy` and the current `Approver`
- the project context (`context/`): instruction files, the docs map, the project index and `.malac/MALAC.md`
- the enabled plugins (`plugins/`) with their hooks, and the agent types, skills and slash commands found on disk and in plugins (`catalog.rs`, `commands.rs`)
- the `EventBus` and the registry of live agents
- the job table (`jobs.rs`)

Several runtimes can share a `Host`: one sequence counter for their event buses, one job table and one `McpPool`, which keys MCP connections by server configuration so identical servers, the control service included, start once per process. The daemon's tabs are such runtimes.

## Agents

An agent (`agent.rs`) is one conversation owned by a tokio task. Callers hold an `AgentHandle` and send it commands (`prompt`, `send`, `cancel`, `set_model`, `compact`); the task processes them one at a time, so an agent's history is never shared or locked.

A turn:

1. Expand a slash command (`/context` is answered without the model), run the plugins' session-start hooks once and their prompt hooks, attach recalled memories (vector search on the prompt) and append the user message.
2. Compact the conversation if the last request used more than 80% of the model's context window.
3. Stream a completion from the provider, publishing text, reasoning and tool-call deltas. Failed requests are retried with backoff when the error is retryable and nothing has streamed yet.
4. For each tool call, run the plugins' `PreToolUse` hooks (which may deny it, rewrite its input, or allow or force a question), then ask the `Policy`; if it says "ask", the `Approver` decides. `PostToolUse` hooks may add feedback to the results. Consecutive tools of kind `Safe` run in parallel; others run one at a time, in order.
5. Append the tool results and loop until the model answers without tool calls.

Subagents are ordinary agents with a `parent` and a `depth`. The `agent_spawn`, `agent_wait`, `agent_send` and `agent_list` tools create and steer them; a semaphore limits how many run at once, and agents at `max_depth` (default 3) do not get the delegation tools. Agents that may delegate see an "Available models" section in their system prompt, listing the configured models with their `description`, so they can give each subagent a model that fits its task. Cancelling an agent cancels its whole subtree.

Prompts can carry attachments (`attach.rs`): text files become a fenced block, images image blocks, directories a listing, other files a note. Each starts with an `<attachment path="...">` block; the `user_message` event shows only the typed text and the attached paths.

## Jobs

`bash` with `background: true` starts a job: a shell in its own process group whose output goes to `~/.malac/jobs/<id>.log`, with its record (command, directory, pid, start and end, exit code, the agent) in `<id>.json` next to it. The job table publishes `started` and `finished` on its own channel, since jobs outlive agents and the process that started them. `job_list`, `job_output` and `job_kill` (an `Execute` tool) give agents control over them. When the daemon starts it restores the records: jobs whose process group is gone are marked finished with status `unknown`, live ones are watched until they end, and finished jobs older than a week are deleted.

Every agent writes an append-only JSONL transcript to `~/.malac/sessions/<id>.jsonl` (metadata, messages, usage, compaction summaries). Resuming replays it from the last compaction.

## Events

Everything an agent does is an `Event` on the broadcast `EventBus`, wrapped in an `Envelope` with the agent id, a sequence number and a timestamp. The terminal renderer, the `--json` printer and every web UI connection are independent subscribers. This is what makes subagents visible: their events flow through the same bus and every front end can show them.

## Providers

`Provider::complete` takes a provider-neutral `Request` (system prompt, `Message`s made of `Block`s, tool specs) and returns the assembled message plus usage. Two wire formats are implemented:

- OpenAI-compatible chat completions (`provider/openai.rs`): OpenAI, LiteLLM, vLLM, Ollama, OpenRouter, Together, ... Reasoning arrives as `reasoning_content`/`reasoning`. Images returned by tools are re-sent in a user message, since tool messages cannot carry images.
- Anthropic Messages (`provider/anthropic.rs`), with prompt caching on the system prompt, tools and the latest turn, and signed thinking blocks replayed as required.

For models marked `vision = false`, images are replaced by descriptions written by `vision_model` before the request is sent.

## Permissions

Each tool has a kind: `Safe` (never asks), `Edit` or `Execute`. The mode decides the default (`ask`, `edits`, `autopilot`); `allow` and `deny` rules (`tool` or `tool(glob)`) refine it. Shell commands are split on `&&`, `||`, `;`, `|` and newlines, and every segment must match an allow rule. Deny rules are checked first and apply in every mode, autopilot included.

## Project context and plugins

`context/` gathers what a session knows about its project when its runtime is built: instruction files in every form other agents use (`instructions.rs`, deduplicated and fitted into a budget by priority), a map of the Markdown docs (`docs.rs`), the structural index in `.malac/index.md` (`index.rs`: a gitignore-aware walk, per-language symbol patterns, incremental by modification time, refreshed in a background thread when stale), the knowledge file `.malac/MALAC.md` (`knowledge.rs`), and where the directory sits in its repository. `prompt.rs` turns it into sections of the system prompt; `Runtime::context_report` is what `/context`, `malac context` and the settings API show.

`plugins/` reads plugins in the Claude Code and Agent Plugins formats (`manifest.rs`), catalogs (`marketplace.rs`), sources and git (`source.rs`, shelling out to `git`), and the state files (`store.rs`); `manage.rs` adds marketplaces and installs, enables and updates plugins; `hooks.rs` runs hooks; `builtin.rs` holds the plugins compiled into the binary. When a runtime is built, the enabled plugins' skills, commands and agents join the catalog, their approved MCP servers join `config.mcp`, and their approved hooks go to the agents. See [plugins.md](plugins.md).

## Memory

Markdown files are the source of truth: `~/.malac/memory/*.md` (global) and `<project>/.malac/memory/*.md` (project), each with `name` and `description` frontmatter (project ones also a `kind`), plus a generated index: `MEMORY.md` for global memories, `.malac/MALAC.md` for the project's, grouped by kind. The indexes are part of every system prompt.

When `[memory.vector]` and `[memory.embeddings]` are configured, saving a memory also upserts its embedding into a Milvus collection (via the Milvus REST API, so self-hosted Milvus and Zilliz Cloud both work). Search returns the matching files' current text, so the index can never serve stale content. With `auto_recall`, every prompt is embedded and close matches are attached to it. `malac memory reindex` rebuilds the collection from the files.

## Control service

`malac-control` exposes the desktop and browsers as MCP tools. malac connects to it like any other MCP server, in this order:

1. `[control] url` if set
2. a running service announced in `~/.malac/control.json` (written by `malac-control serve`)
3. a spawned `malac-control stdio` found next to the `malac` binary or on `PATH`

Its tools are added without a server prefix (`browser_click`, `computer_screenshot`). Read-only tools are annotated so they never need approval. `computer_permissions` (and `malac-control permissions`) reports the macOS Screen Recording and Accessibility state and the app holding them, and with `request` shows the system prompts.

macOS grants these permissions to the process tree's responsible app, which is fixed when the tree starts and survives `setsid`, so they would belong to whatever started the daemon: a terminal, an editor, Claude Code. `malac-control` disclaims that first thing in `main` (`permissions::own_permissions`): when another app is responsible, it re-executes itself in place, same pid, arguments and pipes, through `posix_spawn` with `POSIX_SPAWN_SETEXEC` and `responsibility_spawnattrs_setdisclaim`. The permissions then belong to `malac-control` itself, whoever starts it. An environment variable stops a second attempt if macOS ignores the disclaimer.

macOS remembers a grant by the binary's designated requirement. Release builds are signed with the project's certificate (`scripts/package.sh` with `MALAC_CODESIGN_IDENTITY`; the release workflow checks the certificate), so the requirement names the certificate and the grant survives updates. Ad-hoc builds (Homebrew, source) have a requirement that is a hash of the binary, so each new build needs the grant again. After granting, `malac daemon restart` starts a control service that sees it.

## Daemon

`malac daemon` (`daemon/`) is one long-lived process per user that holds every workspace, tab and agent, so agents outlive the terminal that started them and several clients share them.

- **Process.** `malac daemon start` (and any client that needs it) runs `malac daemon run` in its own session (`setsid`) with output in `~/.malac/daemon.log`. It takes `~/.malac/daemon.lock`, listens on `web.port` (any free port if taken) on loopback, or on all interfaces with `web.remote` unless `web.host` says otherwise, and writes `~/.malac/daemon.json` (`url`, `token`, `pid`, `version`; mode 0600) for local clients. The token persists in `~/.malac/daemon.token`.
- **Hub** (`daemon/hub.rs`). The state: workspaces (`daemon/store.rs` persists them to `~/.malac/workspaces/<id>.json`, and `state.json` remembers the last one), one runtime per loaded tab, built lazily from the tab's directory and options on a shared `Host`, the pending approvals, and a broadcast of serialized messages. Each runtime's events are forwarded tagged with their tab; every change to workspaces, tabs, jobs and the control service is broadcast as a delta. Commands from all transports go through `Hub::execute`.
- **Approvals.** Every tab runtime uses the hub's approver. Requests reach all clients as events and wait in the snapshot until one answers; the first answer wins.
- **Restarts.** On start the hub loads the saved workspaces and restores jobs. Agents come back lazily: a tab loads its runtime when used, and a saved agent resumes from its transcript when prompted. `malac daemon restart` and `stop` refuse while agents or jobs run, unless `--force`.
- **What a restart keeps.** A restart is for a new version, so the daemon keeps its address, remote access and a running tunnel: stopped for a restart (`malac daemon restart`, or the terminal UI's restart), it writes `~/.malac/restart.json`, and a daemon starting within two minutes takes it as its defaults and removes it. Flags given to the new daemon win. `malac daemon stop` and then `start` apply the configuration afresh.
- **Idle exit.** With no clients, running agents, jobs or pending approvals for `daemon.idle_minutes` (default 120), the daemon exits.
- **Versions.** The daemon keeps the version it started with; self-updates replace files only. Clients send their version and the snapshot flags a mismatch, so they can suggest `malac daemon restart`.
- **Control service.** The daemon connects the control service once at start through the shared MCP pool, asks it for its macOS permission state (`computer_permissions`) and reports both in the snapshot.
- **Remote access** (`daemon/access.rs`, `daemon/tunnel.rs`). Requests are local (loopback peer, loopback `Host`, no proxy headers) or remote; remote ones are refused unless `web.remote` is on. The master token works only locally; other devices pair with a one-time code for a device token, hashed in `~/.malac/devices.json` and revocable, which closes their sockets. A tunnel tool (cloudflared, ngrok, Tailscale) runs as the daemon's child and its public address joins the pairing links. See [remote.md](remote.md).
- **Voice** (`daemon/voice.rs`, `daemon/talk.rs`, `web/voice.rs`; model calls in `malac-core/src/voice.rs`). The daemon holds the provider keys, so every model call for sound goes through it. At start, and when settings change, it reads the providers' model lists and works out which models can transcribe, speak or hold a Realtime conversation. Dictation is a REST upload. Speech watches the forwarded agent events (approvals, `say`, long or failed turns), picks one listening connection, the one used last, and makes the audio once per sentence for that client to fetch. Talk mode bridges a client's audio socket to the provider's Realtime socket and gives the voice model tools that prompt, query, approve for and stop the tab's agent. Files from `send_to_user` are copied to `~/.malac/shared/` (`malac-core/src/shared.rs`) and served by id. See [voice.md](voice.md).
- **Push** (`daemon/push.rs`, `web/push.rs`). Paired phones register a Web Push endpoint; the forwarder hands approval events to the push service, which batches requests that wait `push.wait_seconds`, encrypts each message for the device (RFC 8291, with `ring`) and signs it with the daemon's VAPID key (RFC 8292). See [remote.md](remote.md#push-notifications).
- **Keep-awake** (`daemon/awake.rs`). A task follows the running agents and jobs and holds `caffeinate -i -s -w <pid>` (macOS) or `systemd-inhibit` (Linux) while there is work, per `daemon.keep_awake`; the platform command sits behind a `Runner` trait, which the tests replace.

What runs where:

| Command | Runs in |
|---|---|
| `malac -p`, `--json`, piped input, no terminal | this process; never starts the daemon |
| `malac` (full-screen UI) | a daemon client (`tui/`), starting the daemon if needed; it fails with a hint to use `--plain` or `-p` when the daemon cannot start |
| `malac --plain` (or `ui.tui = false`) | a daemon client (`remote.rs`), starting the daemon if needed; in this process when it cannot be reached |
| `malac web`, the editor extension, the web UI | daemon clients |
| `malac -C <name>` (full-screen, `--plain`, `-p`, prompts) | a client of the daemon a saved connection names (`connections.rs`); it is never started from here |

The protocol is described in [protocol.md](protocol.md); `client.rs` is a typed Rust client for it.

## Web UI and editor

The daemon serves a single-page UI from the binary; `malac web` starts the daemon if needed and prints or opens its URL. API requests need a token: the master token on local requests, a device token from pairing otherwise. Remote requests (another machine, a tunnel, a page that rebinds its name) are refused unless remote access is on, which also blocks DNS rebinding. Without a token the page shows a pairing form. The VS Code extension starts `malac web --port 0 --json-ready --cwd <dir>`, which prints one JSON line with the URL and token and then stays connected while the panel is open, and embeds the page in a webview. Only the line interface's in-process fallback (when the daemon cannot be reached) serves a separate UI, with `/web`, for its own runtime.

The page is plain ES modules in `web/`, embedded at build time (`MALAC_WEB_DIR` serves them from disk instead, for development). Every image opens in `viewer.js`, a full-screen viewer that zooms and pans with the wheel, pinches and the keyboard; `send_to_user` results render as the files, fetched from `/api/shared/{id}`. `voice.js` records dictation with an AudioWorklet and sends 16 kHz WAV to `/api/voice/transcribe`, plays the `say` clips addressed to the page's listener id, and runs talk mode over `/api/voice/talk` with 24 kHz PCM both ways; Settings → Voice changes `[voice]` through `PUT /api/voice`. See [voice.md](voice.md).

## Self-update

`update.rs` reads `SHA256SUMS` from the latest GitHub release, picks the archive for the running platform, verifies it, unpacks it next to the installed binaries and moves `malac-control`, then `malac`, into place with `rename`. Running processes keep the file they started from, so sessions and subagents are never disturbed; the new version is used from the next start. A lock file keeps concurrent updates apart. The runtime checks in the background at most once a day and publishes an `Update` event that the terminal UI and web UI display.

## Self-configuration

`tools/settings.rs` gives the agent `malac_settings` (read the config, the configuration reference, models and status; switch the session model) and `malac_configure` (edit one key with `toml_edit` so comments survive, validate the result, store API keys, install updates). Changes are written to the files and apply to new sessions; the web UI can rebuild its runtime to apply them immediately.
