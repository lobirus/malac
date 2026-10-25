# malac

An autonomous coding agent for the terminal, with a web UI, a VS Code extension, and a control service for the desktop and browser. Written in Rust.

```
$ malac -y calc.py has a bug in add, fix it and verify with a python one-liner
● read calc.py
  ⎿ 1	def add(a, b): (+1 lines)
● edit calc.py
  ⎿ Edited calc.py: (+2 lines)
● bash python3 -c "import calc; assert calc.add(2,3)==5; assert calc.add(-1,1)==0; print('ok')"
  ⎿ ok
Fixed `calc.py:2`: `add` returned `a - b`, now returns `a + b`. Verified with
`python3 -c "..."` → `ok`.
─ lora/default · 3.3k context · 366 out · $0.0024 · 2.2s
```

- **CLI first.** `malac <prompt>` starts working immediately. `malac` alone opens an interactive session.
- **Any provider.** OpenAI-compatible and Anthropic-compatible APIs, several providers at once, per-model settings, profiles. Works with gateways such as LiteLLM, OpenRouter or Together, and with local models through Ollama or vLLM.
- **Autopilot.** `-y` runs without asking for permission. Deny rules still apply.
- **Subagents.** The agent delegates self-contained work to subagents that run in parallel, and you can watch each one live in the terminal, the web UI or the session log.
- **Memory.** Markdown memory files, optionally indexed in Milvus (self-hosted or remote) for semantic recall.
- **Computer and browser control.** `malac-control` is a separate MCP service for screenshots, mouse and keyboard, and Chrome automation with isolated profiles. malac starts it automatically.
- **MCP.** Connect any MCP server over stdio, streamable HTTP or WebSocket.

## Install

```bash
git clone https://github.com/malac-io/malac && cd malac
cargo install --path crates/malac
cargo install --path crates/malac-control
```

Requires Rust 1.86 or newer. macOS is the primary platform and Linux is supported. Windows is untested.

## Quick start

Add a provider. The key is stored in `~/.malac/credentials.toml` with owner-only permissions:

```bash
printf '%s' "$OPENAI_API_KEY" | malac provider add openai \
  --base-url https://api.openai.com/v1 --model gpt-5 --api-key-stdin --set-default
```

`ANTHROPIC_API_KEY` or `OPENAI_API_KEY` in the environment also work with no setup.

```bash
malac explain how requests are routed in this repo
malac -y add a --json flag to the export command and test it
git diff | malac review this change
malac                        # interactive
malac -c                     # continue the last session in this directory
malac --web                  # interactive, with the web UI attached
malac web --open             # web UI only
```

If a prompt starts with a subcommand name, malac still treats it as a prompt when it does not parse as that subcommand: `malac web scraper for the docs` is a prompt. `malac -- <prompt>` always is.

## Command line

| Flag | |
|---|---|
| `-m, --model provider/model` | Model for this session |
| `-y, --autopilot` | Never ask for permission |
| `--mode ask\|edits\|autopilot` | Permission mode |
| `-p, --print` | Answer the prompt and exit |
| `--json` | Stream events as JSON lines |
| `-c, --continue` / `-r, --resume <id>` | Continue a session |
| `--profile <name>` | Apply `[profiles.<name>]` from the config |
| `--web` | Serve the web UI for this session too |
| `--no-control` | Do not connect the control service |
| `-v, --verbose` | Show model reasoning |

| Subcommand | |
|---|---|
| `malac web` | Web UI (`--port`, `--open`, `--json-ready`) |
| `malac sessions [list\|show <id>\|delete <id>]` | Saved transcripts, including subagents |
| `malac provider add\|list\|models` | Providers and keys |
| `malac config [path\|show]` | Configuration files and the merged result |
| `malac memory [list\|search\|reindex]` | Long-term memory |
| `malac mcp` | Connected MCP servers and their tools |
| `malac control ...` | Run `malac-control` |

In a session: `/model`, `/mode`, `/agents`, `/compact`, `/clear`, `/resume`, `/cost`, `/mcp`, `/memory`, `/web`, `/exit`. Ctrl-C interrupts the current turn and every subagent it started.

## Permissions

| Mode | Edits | Commands, web, browser |
|---|---|---|
| `ask` (default) | ask | ask |
| `edits` | allowed | ask |
| `autopilot` | allowed | allowed |

Reading, searching, memory and delegation never ask. Rules refine the defaults:

```toml
[permissions]
allow = ["bash(cargo *)", "bash(git status*)", "browser_*"]
deny = ["bash(rm -rf /*)", "bash(git push --force*)"]
```

Deny rules are enforced in every mode. Shell commands are checked segment by segment, so `cargo build && curl ...` does not match `bash(cargo *)`. Answering a prompt with "always" allows that program, or that tool, for the rest of the session.

## Subagents

The agent has four delegation tools: `agent_spawn`, `agent_wait`, `agent_send` and `agent_list`. Several `agent_spawn` calls in one response run in parallel, up to `agents.max_concurrent`. A subagent gets a fresh context, the same tools (or read-only ones), and reports back when it is done.

You can follow subagents in three places:

- **Terminal:** one indented line per step (`↳ tests · bash cargo test`), then `✓ tests finished`. `/agents` shows the tree and what each agent is doing.
- **Web UI:** the agent tree in the sidebar, with each subagent's full live transcript one click away.
- **Logs:** `malac sessions show <id>` prints a session and all of its subagents.

Reusable agent types are Markdown files in `.malac/agents/`; see [configuration](docs/configuration.md#agents).

## Memory

The agent saves durable facts with `memory_save`: user preferences, project conventions, decisions. Each memory is a Markdown file in `~/.malac/memory/` (global) or `.malac/memory/` (project), and an index of them is part of every prompt.

Add a Milvus collection for semantic search and automatic recall:

```toml
[memory.vector]
url = "http://localhost:19530"        # or https://<cluster>.zillizcloud.com
token_env = "MILVUS_TOKEN"

[memory.embeddings]
provider = "openai"
model = "text-embedding-3-small"
```

The files stay the source of truth. `malac memory reindex` rebuilds the collection from them.

## Computer and browser control

`malac-control` exposes the desktop and Chrome as MCP tools. malac spawns it automatically when it is installed. You can also run it as a long-lived service that other agents can use:

```bash
malac-control serve --port 7878   # writes ~/.malac/control.json; malac connects to it
malac-control profiles            # list browser profiles
```

Each browser profile is a separate Chrome instance with its own data directory, so logins persist and never mix. You can also attach to a browser you already started with remote debugging. Profiles go in `~/.malac/control.toml`:

```toml
[browser.profiles.work]
[browser.profiles.personal]
clone_from = "Profile 1"           # copy an existing Chrome profile on first launch
[browser.profiles.debug]
cdp_url = "http://127.0.0.1:9222"
```

On macOS, grant Screen Recording and Accessibility permissions to the terminal app (or to `malac-control`) for desktop control.

## Web UI and VS Code

`malac web` serves the UI on `127.0.0.1:7700` and protects it with a random token that is part of the printed URL. The UI shows live sessions, the subagent tree, streaming transcripts, tool calls and approval prompts.

The extension in [`editors/vscode`](editors/vscode) embeds the same UI in a panel and adds **Malac: Ask About Selection** and **Malac: Run in Terminal**. To try it, open `editors/vscode` in VS Code and press F5, or:

```bash
cd editors/vscode && npm install && npx tsc -p . && code --extensionDevelopmentPath="$PWD" /path/to/project
```

Set `malac.path` if `malac` is not on your `PATH`.

## Configuration

`~/.malac/config.toml` for you, `.malac/config.toml` for a project, `AGENTS.md` (or `MALAC.md`, `CLAUDE.md`) for instructions. See [docs/configuration.md](docs/configuration.md) and [examples/config.toml](examples/config.toml).

## Development

```bash
cargo test
cargo clippy --all-targets
cargo run -p malac -- --help
```

See [docs/architecture.md](docs/architecture.md) for how the pieces fit together.

## License

MIT
