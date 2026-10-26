# Plugins and marketplaces

A plugin packages things that extend the agent: skills, slash commands, agent types, MCP servers and hooks. A marketplace is a catalog of plugins and where to fetch each one. malac installs plugins written for Claude Code and plugins that follow the open [Agent Plugins](https://agent-plugins.org/specification) specification, from folders, GitHub, GitLab or any git host, private repositories included.

```sh
malac plugin marketplace add lobirus/claude-plugins     # a GitHub repository with a marketplace.json
malac plugin install server-setup@lobirus-plugins       # install and enable
malac plugin list
```

In a session, `/context` lists the plugins, skills and commands in use. The agent can manage plugins too, through `malac_configure` (see the end of this page).

## Formats

malac reads every layout a plugin uses and combines what it finds, so one package can serve several agents.

### Claude Code

| What | Where |
|---|---|
| Manifest | `.claude-plugin/plugin.json` (optional): `name`, `version`, `description`, `author`, … and component paths |
| Skills | `skills/<name>/SKILL.md`, plus folders in the manifest's `skills` |
| Commands | `commands/*.md` (Markdown with `description`, `argument-hint` frontmatter; `$ARGUMENTS`, `$1`…), or the manifest's `commands` paths or `{name: {source \| content}}` map. `commands/*.toml` in Gemini CLI's format work too |
| Agents | `agents/*.md`, or the manifest's `agents` |
| Hooks | `hooks/hooks.json`, plus the manifest's `hooks` (paths, inline, or both) |
| MCP servers | `.mcp.json`, plus the manifest's `mcpServers` (path or inline) |

`${CLAUDE_PLUGIN_ROOT}` and `${CLAUDE_PLUGIN_DATA}` are written in wherever they appear in skills, commands, agents, hook commands and MCP server settings. Other `${VAR}` references in `.mcp.json` come from the environment when the server starts.

### Agent Plugins

| What | Where |
|---|---|
| Manifest | `plugin.json` at the root, with `$schema` `https://agent-plugins.org/schemas/1.0.0/plugin.schema.json` (1.1.0 also accepted) |
| Skills | `skills/<name>/SKILL.md`, immediate children only |
| MCP servers | `mcp.json` with the matching `$schema` |
| malac extension | `extensions["io.malac"]` in `plugin.json` (component fields as in Claude Code's manifest) and an `io.malac/` folder (`hooks.json`, `commands/`, `agents/`) |

malac follows the specification: the manifest is closed (unknown fields are reported and ignored, a wrong type or an invalid name rejects the plugin), every path must stay inside the plugin, an invalid `mcp.json` disables MCP but keeps the skills, and an invalid server entry skips only that server. Stdio servers get `PLUGIN_ROOT` and `PLUGIN_DATA` (a persistent folder per plugin), `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` are expanded in `args`, `env` and `cwd` and nothing else is, `./` commands resolve inside the plugin and the default working directory is the plugin root. Streamable HTTP servers need HTTPS except on loopback addresses. Legacy `sse` servers are skipped.

A root `plugin.json` without `$schema` (some multi-format plugins have one) only supplies metadata.

### Plain folders

A folder with `skills/`, `commands/`, `agents/`, `hooks/hooks.json`, `.mcp.json`, `mcp.json` or a `SKILL.md` is a plugin named after the folder, even without a manifest.

### Not supported

Reported by `malac plugin show` and not run: `outputStyles`, `lspServers`, `channels`, `monitors`, `userConfig` (options a plugin asks the user for), MCP bundles (`.mcpb`), hook types other than `command`, and the hook events listed under [Hooks](#hooks). Marketplace entries from `npm`, `archive` and `command` sources are listed but cannot be installed.

## Components

**Skills** become `<plugin>:<skill>` in the system prompt's skill list and in the `skill` tool, and slash commands of the same name. See [configuration](configuration.md#skills).

**Commands** become `/<plugin>:<command>`; `/<command>` works when no other command has that name. They are listed in `/context`, and front ends get them in the tab's `project.commands` for completion.

**Agents** become agent types named `<plugin>:<agent>` for `agent_spawn`. Claude Code's tool names (`Read`, `Grep`, `Bash`, `Edit`, `Write`, `WebFetch`, `TodoWrite`, `Task`) map to malac's tools; a `model` that is not `provider/model` is ignored.

**MCP servers** join the runtime as `<plugin>` (a server named like its plugin) or `<plugin>-<server>`, with their tools prefixed the same way. A server in your own `[mcp]` configuration with the same name wins.

### Hooks

Hooks use Claude Code's format. Each is a shell command (or `command` plus `args`, run without a shell) that gets the event as JSON on stdin and runs with `CLAUDE_PLUGIN_ROOT`, `CLAUDE_PLUGIN_DATA` and `CLAUDE_PROJECT_DIR` in its environment, in the session's directory, for at most `timeout` seconds (60 by default). Matching hooks of one event run in parallel.

| Event | When | What its output does |
|---|---|---|
| `SessionStart` | before the first request of a session (`source`: `startup` or `resume`) | stdout, or `hookSpecificOutput.additionalContext`, is added to the system prompt |
| `SubagentStart` | the same, for subagents (`agent_type`) | the same |
| `UserPromptSubmit` | each prompt (`prompt`) | stdout or `additionalContext` is added to the prompt; exit code 2 (reason on stderr) or `{"decision": "block", "reason": …}` refuses it |
| `PreToolUse` | before a tool call (`tool_name`, `tool_input`) | exit code 2 or `permissionDecision: "deny"` blocks the call and tells the agent why; `"ask"` makes it ask the user even where the permission mode would allow it; `"allow"` skips the question; `updatedInput` replaces the input. Deny rules in `[permissions]` still win |
| `PostToolUse` | after a tool call (`tool_response`) | exit code 2 or `decision: "block"` sends the reason to the agent, as does `additionalContext` |

`matcher` is a regular expression for the whole tool name (`Bash`, `Write|Edit`); both Claude Code's names and malac's (`bash`, `web_fetch`) match. For `SessionStart` it matches the source. Tool input carries `file_path` beside malac's `path`. Not run: `Stop`, `SubagentStop`, `PreCompact`, `Notification`, `SessionEnd` and other events.

## Marketplaces and sources

`malac plugin marketplace add <source>` takes:

| Source | Example |
|---|---|
| A folder (read in place, so edits show up in the next session) | `./tools/marketplace`, `~/plugins` |
| GitHub shorthand, optionally with a branch or tag | `owner/repo`, `owner/repo#v2`, `owner/repo@v2` |
| A git URL, any host | `https://gitlab.com/group/plugins.git`, `git@gitlab.example.com:team/plugins.git#main` |
| The URL of a `marketplace.json` | `https://example.com/plugins/marketplace.json` |

Repositories are cloned with your `git`, so SSH keys, credential helpers (`gh auth`, the macOS keychain) and private repositories work as in a terminal. GitHub shorthand is cloned over HTTPS and retried over SSH when that fails. git never prompts: without credentials, the clone fails with git's message.

The catalog is the first of `.claude-plugin/marketplace.json`, `marketplace.json`, `.agents/plugins/marketplace.json`, `.github/plugin/marketplace.json` and `.cursor-plugin/marketplace.json` that has a `plugins` array, in Claude Code's format: `name`, `owner`, `metadata.pluginRoot`, and entries with `name` and `source`. Entry sources may be a path inside the marketplace (`./plugins/x`, or a bare name under `pluginRoot`), `{"source": "github", "repo", "ref", "sha"}`, `{"source": "url", "url", "ref", "sha"}` (a git repository) or `{"source": "git-subdir", "url", "path", "ref"}`. An entry's component fields (`skills`, `commands`, …) add to the plugin, and stand in for a missing manifest.

A folder or repository without a catalog is a marketplace too: one plugin if it is a plugin itself, otherwise the plugins in its `plugins/` folder (or top level), named after the folder. So `malac plugin install owner/repo` installs a repository that is a plugin in one step.

## Scopes

| Scope | File | |
|---|---|---|
| user | `~/.malac/plugins/plugins.toml` | your marketplaces and plugins, everywhere |
| project | `<project>/.malac/plugins.toml` | the team's: commit it |
| built-in | none | ships with malac; on while `[plugins] defaults = true` |

```toml
[marketplaces]
lobirus-plugins = "lobirus/claude-plugins"

[plugins]
"server-setup@lobirus-plugins" = true
"ponytail@malac" = false
```

The project's file wins over yours for the plugins it names. `--project` on `marketplace add`, `install`, `enable` and `disable` writes it, and enabling a plugin there also records its marketplace so teammates can install it: on another machine, `malac plugin install` without a name installs everything the project enables. Installed files are per machine (`~/.malac/plugins/cache/`).

## Security

- **Installing runs nothing.** It copies files (or, for a local marketplace, points at them) and shows what the plugin contains.
- **Enabling a plugin that runs code needs your approval**: hooks, MCP servers and scripts. `malac plugin install` and `enable` list them and ask; `--yes` approves. Approvals are recorded per plugin in `~/.malac/plugins/installed.json`, never in a project file.
- **A project cannot run code by itself.** A cloned repository's `.malac/plugins.toml` can enable a plugin, but its hooks and MCP servers stay off until you approve them on your machine. Its skills and commands load, like its AGENTS.md does.
- **Updates do not widen access.** `malac plugin update` reinstalls; hooks or MCP servers an update adds stay off until `malac plugin enable` approves them. Updating a git or URL marketplace alone never changes installed plugins, which are copies (a local marketplace is used in place, by design).
- **Scripts run through the agent's `bash`**, under the usual permission mode and rules.
- **The agent asks you.** When the agent enables a plugin that runs code through `malac_configure`, you are asked, even in autopilot.
- Paths that leave a plugin (`..`, symlinks out of it) are refused.

## Built-in: ponytail

malac ships [ponytail](https://ponytail.dev) 5.3.2 by Dietrich Gebert (MIT) as `ponytail@malac`, enabled by default: its six skills (`ponytail`, `ponytail-review`, `ponytail-audit`, `ponytail-debt`, `ponytail-gain`, `ponytail-help`), unchanged, with its license and attribution in `crates/malac-core/plugins/ponytail/`. They are compiled into the binary and written to `~/.malac/plugins/builtin/ponytail/` on first use.

It is vendored rather than installed from GitHub on first run: a default plugin is part of every session's prompt, so it should not depend on the network, change upstream without review, or cost a 20 MB clone; and its hooks, status line and task board run Node.js scripts written for other agents. The base system prompt's working rules say the same thing in brief (simplest complete solution, reuse before adding, name what was skipped).

Turn it off with `malac plugin disable ponytail@malac` (or `--project` for one repository) or `[plugins] defaults = false`. To follow upstream instead, `malac plugin marketplace add DietrichGebert/ponytail` and `malac plugin install ponytail@ponytail`; an enabled plugin named `ponytail` replaces the built-in copy.

## Files

```text
~/.malac/plugins/
├── plugins.toml          your marketplaces and enabled plugins
├── installed.json        installed plugins, their source and commit, approvals
├── marketplaces/<name>/  git clones, or a downloaded marketplace.json
├── cache/<marketplace>/<plugin>/
├── data/<plugin>@<marketplace>/   PLUGIN_DATA, kept across updates and uninstalls
└── builtin/<plugin>/
```

## Publishing a provider-agnostic marketplace

[`examples/marketplace`](../examples/marketplace) is a complete one. The recipe:

1. Put each plugin in `plugins/<name>/` with an Agent Plugins `plugin.json` (`$schema`, `name`, `version`, `description`, `license`) and a `.claude-plugin/plugin.json` with the same metadata, so Claude Code, malac and Agent Plugins clients all find a manifest.
2. Put portable components in the standard places: `skills/<name>/SKILL.md` with `scripts/` and `references/`, and MCP servers in `mcp.json` with `./` commands and `${PLUGIN_ROOT}`. Claude Code reads MCP servers from `.mcp.json` instead; ship both when you have servers.
3. Put client-specific parts where each client looks: `commands/`, `agents/` and `hooks/hooks.json` for Claude Code (malac reads them too), and anything only malac should load under `io.malac/`.
4. List the plugins in `.claude-plugin/marketplace.json` with `./plugins/<name>` sources. Copy it to `.cursor-plugin/marketplace.json` for Cursor if you like; malac reads either.
5. Push it to GitHub or GitLab, public or private. Users run `malac plugin marketplace add <owner>/<repo>` (or the git URL) and `malac plugin install <plugin>@<marketplace>`; teams add both to `.malac/plugins.toml`.

Check it locally first: `malac plugin marketplace add ./my-marketplace`, `malac plugin install <plugin>` and `malac plugin show <plugin>` print what each client-neutral part became, with warnings for anything malac skipped.

## Commands

| Command | |
|---|---|
| `malac plugin marketplace add <source> [--project]` | Fetch a marketplace and record it |
| `malac plugin marketplace list [--json]` | Marketplaces and their plugins |
| `malac plugin marketplace update [name]` | Fetch the latest catalogs |
| `malac plugin marketplace remove <name> [--project]` | Forget one; its plugins stay installed |
| `malac plugin install [<plugin>[@marketplace] \| <source>] [--project] [--yes] [--no-enable]` | Install and enable; without a name, what the project enables |
| `malac plugin enable <plugin> [--project] [--yes]` | Turn on, approving what runs code |
| `malac plugin disable <plugin> [--project]` | Turn off, built-in plugins included |
| `malac plugin uninstall <plugin>` | Remove the files and your entry (its data folder stays) |
| `malac plugin update [plugin]` | Fetch and reinstall |
| `malac plugin list [--json]` | Installed, built-in and project plugins |
| `malac plugin show <plugin>` | Its contents, path, approvals and warnings |

The agent's `malac_configure` tool has the same operations (`marketplace_add`, `marketplace_remove`, `marketplace_update`, `plugin_install`, `plugin_enable`, `plugin_disable`, `plugin_uninstall`, `plugin_update`, with `file` = `user` or `project`), and `malac_settings` shows them (`show: "plugins"`). Changes apply to new sessions; the web UI's settings can apply them to idle tabs now.
