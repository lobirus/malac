# Configuration

Configuration is TOML, merged in this order (later wins, tables merge key by key):

1. built-in defaults (`anthropic` and `openai` providers reading `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`)
2. `~/.malac/config.toml`
3. `<project root>/.malac/config.toml`, where the project root is the nearest directory with `.git`
4. `[profiles.<name>]` when started with `--profile <name>`
5. command-line flags

`MALAC_HOME` moves `~/.malac` elsewhere. `malac config show` prints the merged result; `malac config path` prints the file locations.

A project file cannot set `mode`, `providers`, `mcp`, `control`, `permissions`, `memory`, `web`, `update`, `daemon` or `profiles`, because cloning a repository should never be enough to run commands, widen permissions or send your credentials elsewhere. Set `trust_project_config = true` in your user config to lift this.

See [`examples/config.toml`](../examples/config.toml) for a complete example.

## Top level

| Key | Default | Meaning |
|---|---|---|
| `model` | first provider with a key | Main model, `provider/model` |
| `small_model` | `model` | Used for compaction summaries |
| `vision_model` | none | Describes images for models with `vision = false` |
| `mode` | `"ask"` | `ask`, `edits` or `autopilot` |
| `max_steps` | `200` | Tool-call rounds per prompt before the agent is stopped |
| `trust_project_config` | `false` | Let project config set every key |

## `[providers.<name>]`

| Key | Meaning |
|---|---|
| `kind` | `"openai"` (chat completions) or `"anthropic"` (messages API) |
| `base_url` | e.g. `https://api.openai.com/v1`, `http://localhost:11434/v1`, `https://api.anthropic.com` |
| `api_key_env` | Environment variable holding the key |
| `headers` | Extra HTTP headers |
| `models` | Model ids offered by `/model` and the web UI |

Keys are looked up in `api_key_env` first, then in `~/.malac/credentials.toml` (written by `malac provider add --api-key-stdin`, mode 0600).

## `[models."provider/model"]`

| Key | Default | Meaning |
|---|---|---|
| `description` | none | What the model is good at, e.g. `"fast and cheap"` or `"strongest reasoning"` |
| `context_window` | `200000` | Compaction starts at 80% of this |
| `max_output_tokens` | `16384` | `max_tokens` sent with each request |
| `vision` | assumed `true` | Set `false` to route images through `vision_model` |
| `thinking_budget` | none | Anthropic extended thinking budget |
| `temperature` | provider default | |

Agents that can delegate get an "Available models" section in their system prompt: every model in the providers' `models` lists, plus any model here with a `description`, with that description and a line on matching models to tasks. `agent_spawn` takes the chosen one as `model`.

```toml
[models."lora/fast"]
description = "fast and cheap: searches, summaries, mechanical edits"

[models."lora/smart"]
description = "strongest reasoning: design, hard bugs, reviews"
```

## `[permissions]`

```toml
allow = ["bash(cargo *)", "bash(git status*)", "browser_*", "web_fetch(*.rust-lang.org)"]
deny = ["bash(rm -rf /*)", "bash(git push --force*)"]
```

A rule is a tool-name glob, optionally followed by a glob in parentheses matched against the tool's key: the command for `bash` (each segment separately), the path for `write`/`edit`, the host for `web_fetch`. Deny rules always win and apply in autopilot too.

## `[mcp.<name>]`

```toml
[mcp.github]
url = "https://api.githubcopilot.com/mcp/"          # http(s) = streamable HTTP, ws(s) = WebSocket
headers = { Authorization = "Bearer ${GITHUB_TOKEN}" }

[mcp.files]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "${HOME}/notes"]
env = { DEBUG = "0" }
prefix = true   # expose tools as files__<tool> (default)
enabled = true
```

`${VAR}` is expanded in `command`, `args`, `env` and `headers`. `token_env` names a variable sent as a bearer token.

## `[control]`

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` | Connect the computer and browser control service |
| `command` | `malac-control` next to `malac`, else on `PATH` | Binary to spawn |
| `url` | none | Connect to a running service, e.g. `ws://127.0.0.1:7878` |
| `token_env` | none | Variable holding that service's token |

The service itself reads `~/.malac/control.toml` (browser profiles, screenshot size); see `malac-control --help`.

## `[memory]`

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` | Memory tools and the index in the system prompt |
| `auto_recall` | `true` | Attach vector matches to each prompt |
| `recall_limit` | `5` | Matches considered per prompt |

```toml
[memory.vector]
url = "http://localhost:19530"   # or a Zilliz Cloud endpoint
token_env = "MILVUS_TOKEN"       # "user:password" or an API key
database = "default"
collection = "malac_memory"

[memory.embeddings]
provider = "lora"                # an OpenAI-compatible provider with /embeddings
model = "embed"
```

## `[agents]`

| Key | Default | Meaning |
|---|---|---|
| `max_depth` | `3` | How deep subagents may nest; agents at the limit get no delegation tools |
| `max_concurrent` | `4` | Subagents running at the same time |

Agent types live in `~/.malac/agents/*.md` and `<project>/.malac/agents/*.md`:

```markdown
---
name: reviewer
description: Reviews a diff for bugs and missing tests
model: lora/smart
tools: read, grep, glob, bash
---
You review code changes. Report concrete problems with file:line references...
```

## Instructions and skills

`AGENTS.md`, `MALAC.md` or `CLAUDE.md` (first found per directory) are read from `~/.malac/` and from every directory between the project root and the working directory.

Skills are `SKILL.md` files in `~/.malac/skills/<name>/`, `~/.claude/skills/<name>/`, `<project>/.malac/skills/<name>/` or `<project>/.claude/skills/<name>/`, with `name` and `description` frontmatter. Their descriptions are listed in the system prompt and the agent reads the file when a skill applies.

## `[update]`

| Key | Default | Meaning |
|---|---|---|
| `check` | `true` | Look for a newer release in the background, at most once a day |
| `auto` | `false` | Install it automatically |

Updates replace the binaries with an atomic rename, so running sessions and their subagents keep working on the version they started with; new sessions use the new one. `malac update` updates now, `malac update --check` only checks. Homebrew installs are updated with `brew upgrade lobirus/malac/malac`, and installs in a read-only directory (the `.pkg`) need the new installer or `sudo malac update`.

## Configuring malac by asking it

The agent has two tools for its own configuration. `malac_settings` shows the config files, this reference, the available models and the update status, and switches the current session's model. `malac_configure` sets or removes keys (comments in the file are kept and every change is validated), stores API keys and installs updates. `malac_configure` asks for approval unless you are in autopilot. Rules such as `malac_configure(set:ui.*)` pre-approve specific edits.

## `[daemon]`

| Key | Default | Meaning |
|---|---|---|
| `idle_minutes` | `120` | Exit after this long with no clients, running agents, jobs or pending approvals; `0` never exits |
| `browse_root` | home directory | The directory picker (`GET /api/fs`) lists only directories below this (`~/` allowed) |

The daemon reads `[daemon]`, `[web]`, `[update]` and `[control]` from the user config only. Each tab loads the full configuration for its own directory. The daemon keeps the environment it was started with, so after changing an API key variable, restart it (`malac daemon restart`) or store the key with `malac provider add --api-key-stdin`.

## `[ui]` and `[web]`

| Key | Default | Meaning |
|---|---|---|
| `ui.show_thinking` | `false` | Stream model reasoning in the terminal (`-v`) |
| `ui.tui` | `true` | Full-screen terminal UI; `false` uses the line-based interface (`--plain`) |
| `web.enabled` | `false` | Serve a web UI with every full-screen terminal session, for that session's runtime |
| `web.remote` | `false` | The daemon (and `--web`) listen on all interfaces instead of `web.host` |
| `web.host` | `127.0.0.1` | Bind address when not remote |
| `web.port` | `7700` | Port of the daemon and its web UI; any free port when it is taken |
