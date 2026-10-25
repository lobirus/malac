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
- instruction files, agent types and skills found on disk (`catalog.rs`)
- the `EventBus` and the registry of live agents
- the job table (`jobs.rs`)

Several runtimes can share a `Host`: one sequence counter for their event buses, one job table and one `McpPool`, which keys MCP connections by server configuration so identical servers, the control service included, start once per process. The daemon's tabs are such runtimes.

## Agents

An agent (`agent.rs`) is one conversation owned by a tokio task. Callers hold an `AgentHandle` and send it commands (`prompt`, `send`, `cancel`, `set_model`, `compact`); the task processes them one at a time, so an agent's history is never shared or locked.

A turn:

1. Attach recalled memories (vector search on the prompt) and append the user message.
2. Compact the conversation if the last request used more than 80% of the model's context window.
3. Stream a completion from the provider, publishing text, reasoning and tool-call deltas. Failed requests are retried with backoff when the error is retryable and nothing has streamed yet.
4. For each tool call, ask the `Policy`; if it says "ask", the `Approver` decides. Consecutive tools of kind `Safe` run in parallel; others run one at a time, in order.
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

## Memory

Markdown files are the source of truth: `~/.malac/memory/*.md` (global) and `<project>/.malac/memory/*.md` (project), each with `name` and `description` frontmatter, plus a generated `MEMORY.md` index. The index is part of every system prompt.

When `[memory.vector]` and `[memory.embeddings]` are configured, saving a memory also upserts its embedding into a Milvus collection (via the Milvus REST API, so self-hosted Milvus and Zilliz Cloud both work). Search returns the matching files' current text, so the index can never serve stale content. With `auto_recall`, every prompt is embedded and close matches are attached to it. `malac memory reindex` rebuilds the collection from the files.

## Control service

`malac-control` exposes the desktop and browsers as MCP tools. malac connects to it like any other MCP server, in this order:

1. `[control] url` if set
2. a running service announced in `~/.malac/control.json` (written by `malac-control serve`)
3. a spawned `malac-control stdio` found next to the `malac` binary or on `PATH`

Its tools are added without a server prefix (`browser_click`, `computer_screenshot`). Read-only tools are annotated so they never need approval. `computer_permissions` (and `malac-control permissions`) reports the macOS Screen Recording and Accessibility state and the app holding them, and with `request` shows the system prompts.

macOS grants these permissions to the process tree's responsible app, which is fixed when the tree starts and survives `setsid`. A daemon started from iTerm spawns `malac-control` as iTerm's, one started by the editor extension as the editor's. To make the grant stick no matter who starts the daemon, run `malac-control serve` from a terminal app that has both permissions: the daemon then connects to it through `~/.malac/control.json` instead of spawning its own. After granting, restart that app and the daemon (`malac daemon restart`).

## Daemon

`malac daemon` (`daemon/`) is one long-lived process per user that holds every workspace, tab and agent, so agents outlive the terminal that started them and several clients share them.

- **Process.** `malac daemon start` (and any client that needs it) runs `malac daemon run` in its own session (`setsid`) with output in `~/.malac/daemon.log`. It takes `~/.malac/daemon.lock`, listens on `web.port` (any free port if taken) on loopback, or on all interfaces with `web.remote`, and writes `~/.malac/daemon.json` (`url`, `token`, `pid`, `version`; mode 0600) for local clients. The token persists in `~/.malac/daemon.token`.
- **Hub** (`daemon/hub.rs`). The state: workspaces (`daemon/store.rs` persists them to `~/.malac/workspaces/<id>.json`, and `state.json` remembers the last one), one runtime per loaded tab, built lazily from the tab's directory and options on a shared `Host`, the pending approvals, and a broadcast of serialized messages. Each runtime's events are forwarded tagged with their tab; every change to workspaces, tabs, jobs and the control service is broadcast as a delta. Commands from all transports go through `Hub::execute`.
- **Approvals.** Every tab runtime uses the hub's approver. Requests reach all clients as events and wait in the snapshot until one answers; the first answer wins.
- **Restarts.** On start the hub loads the saved workspaces and restores jobs. Agents come back lazily: a tab loads its runtime when used, and a saved agent resumes from its transcript when prompted. `malac daemon restart` and `stop` refuse while agents or jobs run, unless `--force`.
- **Idle exit.** With no clients, running agents, jobs or pending approvals for `daemon.idle_minutes` (default 120), the daemon exits.
- **Versions.** The daemon keeps the version it started with; self-updates replace files only. Clients send their version and the snapshot flags a mismatch, so they can suggest `malac daemon restart`.
- **Control service.** The daemon connects the control service once at start through the shared MCP pool, asks it for its macOS permission state (`computer_permissions`) and reports both in the snapshot.

What runs where:

| Command | Runs in |
|---|---|
| `malac -p`, `--json`, piped input, no terminal | this process; never starts the daemon |
| `malac` (full-screen UI) | a daemon client (`tui/`), starting the daemon if needed; it fails with a hint to use `--plain` or `-p` when the daemon cannot start |
| `malac --plain` (or `ui.tui = false`) | a daemon client (`remote.rs`), starting the daemon if needed; in this process when it cannot be reached |
| `malac web`, the editor extension, the web UI | daemon clients |

The protocol is described in [protocol.md](protocol.md); `client.rs` is a typed Rust client for it.

## Web UI and editor

The daemon serves a single-page UI from the binary; `malac web` starts the daemon if needed and prints or opens its URL. API requests need the token, and requests from this machine must name a loopback host, which blocks DNS rebinding. The VS Code extension starts `malac web --port 0 --json-ready --cwd <dir>`, which prints one JSON line with the URL and token and then stays connected while the panel is open, and embeds the page in a webview. Only the line interface's in-process fallback (when the daemon cannot be reached) serves a separate UI, with `/web`, for its own runtime.

## Self-update

`update.rs` reads `SHA256SUMS` from the latest GitHub release, picks the archive for the running platform, verifies it, unpacks it next to the installed binaries and moves `malac-control`, then `malac`, into place with `rename`. Running processes keep the file they started from, so sessions and subagents are never disturbed; the new version is used from the next start. A lock file keeps concurrent updates apart. The runtime checks in the background at most once a day and publishes an `Update` event that the terminal UI and web UI display.

## Self-configuration

`tools/settings.rs` gives the agent `malac_settings` (read the config, the configuration reference, models and status; switch the session model) and `malac_configure` (edit one key with `toml_edit` so comments survive, validate the result, store API keys, install updates). Changes are written to the files and apply to new sessions; the web UI can rebuild its runtime to apply them immediately.
