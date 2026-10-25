# Architecture

malac is a Cargo workspace with four crates:

| Crate | Role |
|---|---|
| `malac-core` | The runtime: providers, tools, agent loop, permissions, memory, sessions, MCP client wiring. No UI. |
| `malac` | The `malac` binary: CLI, terminal renderer, REPL, web server. |
| `malac-control` | Computer and browser control, served over MCP (stdio or WebSocket). Also a library. |
| `malac-mcp` | A small Model Context Protocol implementation (client and server) shared by the others. |

Front ends never talk to models or tools directly. They create a `Runtime`, spawn agents on it and render the event stream.

## Runtime

`Runtime` (`crates/malac-core/src/runtime.rs`) holds what every agent shares:

- the merged `Config`
- a provider cache (`provider/`), built lazily per provider name
- the tool list: built-in tools plus tools from connected MCP servers
- `Memory`, the permission `Policy` and the current `Approver`
- instruction files, agent types and skills found on disk (`catalog.rs`)
- the `EventBus` and the registry of live agents

## Agents

An agent (`agent.rs`) is one conversation owned by a tokio task. Callers hold an `AgentHandle` and send it commands (`prompt`, `send`, `cancel`, `set_model`, `compact`); the task processes them one at a time, so an agent's history is never shared or locked.

A turn:

1. Attach recalled memories (vector search on the prompt) and append the user message.
2. Compact the conversation if the last request used more than 80% of the model's context window.
3. Stream a completion from the provider, publishing text, reasoning and tool-call deltas. Failed requests are retried with backoff when the error is retryable and nothing has streamed yet.
4. For each tool call, ask the `Policy`; if it says "ask", the `Approver` decides. Consecutive tools of kind `Safe` run in parallel; others run one at a time, in order.
5. Append the tool results and loop until the model answers without tool calls.

Subagents are ordinary agents with a `parent` and a `depth`. The `agent_spawn`, `agent_wait`, `agent_send` and `agent_list` tools create and steer them; a semaphore limits how many run at once, and agents at `max_depth` do not get the delegation tools. Cancelling an agent cancels its whole subtree.

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

Its tools are added without a server prefix (`browser_click`, `computer_screenshot`). Read-only tools are annotated so they never need approval.

## Web UI and editor

`malac web` (or `--web` / `/web` in a terminal session) serves a single-page UI from the binary. The page authenticates with a random per-run token; API requests must name a loopback host when the server is bound to one, which blocks DNS rebinding. The VS Code extension starts `malac web --port 0 --json-ready` and embeds the page in a webview.
