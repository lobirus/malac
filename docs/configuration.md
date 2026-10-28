# Configuration

Configuration is TOML, merged in this order (later wins, tables merge key by key):

1. built-in defaults (`anthropic` and `openai` providers reading `ANTHROPIC_API_KEY` / `OPENAI_API_KEY`)
2. `~/.malac/config.toml`
3. `<project root>/.malac/config.toml`, where the project root is the nearest directory with `.git`
4. `[profiles.<name>]` when started with `--profile <name>`
5. command-line flags

`MALAC_HOME` moves `~/.malac` elsewhere. `malac config show` prints the merged result; `malac config path` prints the file locations.

A project file cannot set `mode`, `providers`, `mcp`, `control`, `permissions`, `memory`, `web`, `update`, `daemon`, `remote` or `profiles`, because cloning a repository should never be enough to run commands, widen permissions or send your credentials elsewhere. Set `trust_project_config = true` in your user config to lift this.

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
| `audio` | from the name | What the model does with sound, for voice features: `transcription`, `input` (a chat model that takes audio), `speech` or `realtime`. Names with `whisper`/`transcribe`, `tts`/`speech`, `realtime` or `audio` are recognised without it. |

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
cwd = "/tmp"    # working directory of a command server (default: malac's)
prefix = true   # expose tools as files__<tool> (default)
enabled = true
```

`${VAR}` is expanded in `command`, `args`, `env` and `headers`. `token_env` names a variable sent as a bearer token. Enabled plugins add their MCP servers here as `<plugin>` or `<plugin>-<server>`; a server you configure under the same name wins.

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

Agent types live in `~/.malac/agents/*.md`, `<project>/.malac/agents/*.md` and plugins' `agents/` (named `<plugin>:<agent>`). Claude Code's tool names (`Read`, `Bash`, …) are understood, and a `model` that is not `provider/model` (such as `sonnet`) is ignored:

```markdown
---
name: reviewer
description: Reviews a diff for bugs and missing tests
model: lora/smart
tools: read, grep, glob, bash
---
You review code changes. Report concrete problems with file:line references...
```

## `[context]`

What a session knows about its project before the first prompt. `malac context` (or `/context` in a session) shows what was loaded and why something was skipped.

| Key | Default | Meaning |
|---|---|---|
| `instruction_files` | `[]` | More instruction files, after the standard ones: relative to the project root, or absolute or `~/…` in the user config. A project config may name only files inside the project |
| `max_instruction_chars` | `40000` | Characters of instruction files in the system prompt, all together |
| `docs` | `true` | List the project's Markdown docs (path, title, first sentence) in the prompt |
| `index` | `true` | Keep `.malac/index.md`, the structural index, in a repository |
| `index_max_files` | `5000` | Files whose symbols the index lists |
| `knowledge` | `true` | Include `.malac/MALAC.md`, the project knowledge malac keeps |

**Instruction files** are read from `~/.malac/` (`AGENTS.md`, `MALAC.md`, `CLAUDE.md`), from `~/.claude/CLAUDE.md`, and from every directory between the project root and the working directory: `AGENTS.md` (the standard, and first), `MALAC.md`, `CLAUDE.md`, `CLAUDE.local.md`, `GEMINI.md`, `.github/copilot-instructions.md`, `.cursorrules`, `.windsurfrules`, and the rule folders `.cursor/rules/*.mdc`, `.windsurf/rules/*.md`, `.github/instructions/*.instructions.md` and `.clinerules/`. Lines that are only `@path` (CLAUDE.md and GEMINI.md imports) pull in that file when it is inside the project. A file whose text another one already has, such as a `CLAUDE.md` symlinked to `AGENTS.md`, is read once. When the files exceed `max_instruction_chars`, configured files and `AGENTS.md` keep their text first, then `MALAC.md`, `CLAUDE.md`, `GEMINI.md`, Copilot's, Cursor's, Windsurf's and Cline's; deeper directories before shallower ones; the rest is cut from the middle or skipped.

Rules with frontmatter apply as their tool defines them: a Cursor rule with `alwaysApply: true` is part of the prompt, one with `globs` (or a Copilot file with `applyTo`, a Windsurf rule with `trigger: glob`) is listed with its patterns for the agent to read before touching matching files, one with only a `description` is listed for the agent to read when it fits, and the rest apply only when mentioned.

**The project index**, `.malac/index.md`, maps the repository without a model: the directory tree with file counts, languages, manifests (with package names, workspace members and scripts) and entry points, every source file's top-level symbols (found with per-language patterns for Rust, TypeScript and JavaScript, Python, Go, Java, Kotlin, Swift, C#, C and C++, Ruby, PHP, shell, Elixir and Lua), the headings of Markdown files, and a Notes section the agent adds to. It follows `.gitignore`, rereads only files whose modification time or size changed (`.malac/index.cache.json`), and is refreshed in the background when a session starts and it is older than five minutes. The system prompt tells the agent to look there before searching broadly, through the `project_index` tool. `malac index` updates it now, `--rebuild` from scratch. Both files are regenerable, so `.malac/.gitignore` keeps them out of git.

**Project knowledge**, `.malac/MALAC.md`, is what malac learned about the project: commands, conventions, gotchas and decisions. Each entry is a project memory in `.malac/memory/` (saved with `memory_save`, scope `project`, and a `kind`), and MALAC.md lists them by kind between two markers that malac rewrites; text outside them is yours. Both are meant to be committed, so knowledge travels between machines and teammates. Instructions for agents stay in `AGENTS.md`, which malac proposes changes to but does not write unasked. There is no `MALAC.json`: structured project settings belong in `.malac/config.toml`, plugins in `.malac/plugins.toml`.

When the working directory is one part of a larger repository (`frontend/` with `backend/` beside it), the prompt says so and names the other parts, and the UIs offer to open the repository root (`project.suggest_root` in the [protocol](protocol.md)). Open the root for work across the stack; a subdirectory still sees the root's instruction files, index and knowledge.

## Skills

Skills follow the [Agent Skills](https://agentskills.io/specification) format: a folder with a `SKILL.md` (`name` and `description` frontmatter, then instructions) and optionally `scripts/`, `references/` and `assets/`. malac reads them from `~/.malac/skills/`, `~/.claude/skills/`, `~/.agents/skills/`, `<project>/.agents/skills/`, `<project>/.claude/skills/` and `<project>/.malac/skills/` (later ones win on a name), and from enabled plugins, named `<plugin>:<skill>`.

The system prompt lists each skill's name and description. The `skill` tool loads one: its instructions with `$SKILL_DIR` (and `${CLAUDE_SKILL_DIR}`, `${CLAUDE_PLUGIN_ROOT}`) written in as paths, and the list of its files. From then on the agent's `bash` commands have `SKILL_DIR` set, so a skill's scripts run as `sh "$SKILL_DIR/scripts/x.sh"`, through the usual permission checks. Every skill also runs as a slash command, `/<name> arguments`, which fills in `$ARGUMENTS`. `disable-model-invocation: true` keeps a skill out of the prompt so only `/<name>` runs it; `user-invocable: false` keeps it from being a command. `allowed-tools` is not used: approvals work as for any tool call.

## Slash commands

Besides the built-in ones (`/help` lists them), `/name arguments` runs a custom command:

- Markdown commands in Claude Code's format, from `~/.malac/commands/`, `~/.claude/commands/`, `<project>/.claude/commands/`, `<project>/.malac/commands/` and plugins' `commands/`: frontmatter `description` and `argument-hint`, a body with `$ARGUMENTS` or `$1`…`$9`
- TOML commands in Gemini CLI's format (`description`, `prompt` with `{{args}}`) from `~/.gemini/commands/`, `<project>/.gemini/commands/` and plugins; folders become namespaces (`git/commit.toml` is `/git:commit`)
- skills, as above

Plugin commands are named `<plugin>:<command>`; the part after `:` works alone when no other command has it. The command's text goes to the agent after what you typed. `` !`command` `` lines and `@file` references in commands are not expanded.

## `[plugins]`

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` | Load installed and enabled plugins |
| `defaults` | `true` | Enable the plugins that ship with malac: [ponytail](https://ponytail.dev) (`ponytail@malac`) and malac's design guidance (`design@malac`) |

Plugins, marketplaces and their files are described in [plugins.md](plugins.md). Turn one built-in plugin off with `malac plugin disable ponytail@malac`, all of them with `defaults = false`.

## `[voice]`

Dictation, speech and talk mode, described in [voice.md](voice.md). Each feature is offered only when a model can do it; an unset model means the first suitable one in the providers' model lists. Every app's Voice settings and `malac voice set` write these keys.

| Key | Default | Meaning |
|---|---|---|
| `dictation` | `true` | A microphone button that types what you say |
| `dictation_model` | first found | `provider/model` that transcribes: a transcription model, else a chat model that takes audio |
| `language` | detected | Language hint for dictation, like `en` |
| `speech` | `false` | malac speaks approvals, questions and results worth mentioning, and agents get a `say` tool |
| `speech_model` | first found | A speech model, or `system` for the machine's own voice (`say`, `espeak-ng`) |
| `speech_voice` | `alloy` | The speech model's voice (or a system voice name) |
| `talk` | `true` | Talk mode, a spoken conversation through an end-to-end voice model |
| `talk_model` | first found | A Realtime model, like `openai/gpt-realtime` |
| `talk_voice` | `alloy` | The voice it speaks with |

## `[attribution]`

How the commits and pull requests malac makes credit it, the way other coding agents do. GitHub shows the trailer's address as a co-author, with an avatar, once a GitHub account has that address verified.

| Key | Default | Meaning |
|---|---|---|
| `commit` | `"Co-Authored-By: malac <noreply@malac.io>"` | Trailer lines that end the message of every commit malac makes; `""` for none |
| `pr` | `"Generated with [malac](https://malac.io)"` | What ends the description of every pull request malac opens; `""` for nothing |

Instructions in `AGENTS.md` and the like win: a project that asks for no attribution, or for its own trailers, gets that.

## `[update]`

| Key | Default | Meaning |
|---|---|---|
| `check` | `true` | Look for a newer release in the background, at most once a day |
| `auto` | `false` | Install it automatically |

Updates replace the binaries with an atomic rename, so running sessions and their subagents keep working on the version they started with; new sessions use the new one. `malac update` updates now, `malac update --check` only checks. Homebrew installs are updated with `brew upgrade lobirus/malac/malac`, and installs in a read-only directory (the `.pkg`) need the new installer or `sudo malac update`.

## Configuring malac by asking it

The agent has two tools for its own configuration. `malac_settings` shows the config files, this reference, the available models, the update status, the session's context (`context`) and the plugins and marketplaces (`plugins`), and switches the current session's model. `malac_configure` sets or removes keys (comments in the file are kept and every change is validated), stores API keys, installs updates, and adds marketplaces and installs, enables, disables, updates and removes plugins. `malac_configure` asks for approval unless you are in autopilot; enabling a plugin that runs code asks you even in autopilot. Rules such as `malac_configure(set:ui.*)` pre-approve specific edits.

## `[daemon]`

| Key | Default | Meaning |
|---|---|---|
| `idle_minutes` | `120` | Exit after this long with no clients, running agents, jobs or pending approvals; `0` never exits |
| `browse_root` | home directory | The folder and file pickers (`GET /api/fs`) list only what is below this (`~/` allowed) |
| `keep_awake` | `"busy"` | Keep the machine from sleeping: `"busy"` while agents or jobs run (released a minute after), `"always"` while the daemon runs, `"off"`. macOS uses `caffeinate`, Linux `systemd-inhibit`; see [remote.md](remote.md#keeping-the-machine-awake). The terminal UI's View menu and the web UI change it until the daemon restarts. |

The daemon reads `[daemon]`, `[web]`, `[remote]`, `[update]` and `[control]` from the user config only. Each tab loads the full configuration for its own directory. The daemon keeps the environment it was started with, so after changing an API key variable, restart it (`malac daemon restart`) or store the key with `malac provider add --api-key-stdin`.

## `[ui]` and `[web]`

The daemon always serves the web UI; `malac web` prints or opens its address.

| Key | Default | Meaning |
|---|---|---|
| `ui.show_thinking` | `false` | Stream model reasoning in the terminal (`-v`) |
| `ui.tui` | `true` | Full-screen terminal UI; `false` uses the line-based interface (`--plain`) |
| `web.remote` | `false` | Remote access: paired devices on other machines and through tunnels may connect (or start the daemon with `--remote`). Off, only local requests get in. See [remote.md](remote.md). |
| `web.host` | `127.0.0.1`, or `0.0.0.0` with `web.remote` | Bind address. `127.0.0.1` with `web.remote` allows tunnels and SSH but keeps the daemon off the network. |
| `web.port` | `7700` | Port of the daemon and its web UI; any free port when it is taken |

## `[remote]`

How other devices reach the daemon when `web.remote` allows them.

| Key | Default | Meaning |
|---|---|---|
| `tunnel` | none | Start this tunnel with the daemon and stop it with it: `"cloudflare"`, `"ngrok"`, `"tailscale"` (your tailnet) or `"tailscale-funnel"` (public). `malac tunnel <name>` starts one by hand. |
| `public_url` | none | An address in front of the daemon, such as a reverse proxy with TLS (`https://malac.example.com`); offered first in pairing links after a running tunnel |

```toml
[web]
remote = true
host = "127.0.0.1"   # tunnels and SSH only

[remote]
tunnel = "cloudflare"

[daemon]
keep_awake = "busy"
```
