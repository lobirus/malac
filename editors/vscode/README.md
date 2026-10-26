# malac for VS Code

Work with [malac](https://github.com/lobirus/malac) agents without leaving the editor. The extension adds a **malac** container (the pig) to the activity bar with a native chat, the daemon's workspaces, jobs and sessions, and a usage view. It looks like the rest of VS Code in light, dark and high-contrast themes.

## Chat

- **Streaming transcript.** Answers stream in as Markdown; thinking folds away; every tool call is a card with its command, diff or output; subagents get a card that follows them live and opens their own transcript.
- **Approvals inline.** A tool call that needs permission shows **Allow**, **Always** (for the rest of the session) and **Deny** on its card. Requests from subagents, and requests to enable a plugin that runs code, appear above the composer; **Show details** lists what the plugin would run (hooks, MCP servers, scripts) or the full input.
- **Composer.** Enter sends, Shift+Enter starts a new line, ↑ recalls earlier prompts, Esc stops a running agent. The model picker sets the model of the agent (or of the next new agent), the mode button switches the tab between *Ask*, *Edits* and *Autopilot* (Shift+Tab cycles), and **Stop** cancels the turn.
- **Current file.** A chip shows the active editor's file, `README.md`, or with a selection `README.md:12-30`. A saved file goes with the next message as an attachment; selected lines (or a file with unsaved changes) go inline with their path and line numbers. Click the chip to leave the file out; it stays off until you turn it on again.
- **Slash commands.** Type `/` for a list: the chat's own `/model`, `/mode`, `/compact`, `/clear` and `/cost`, then `/context` and the tab's custom commands (from `.malac/commands`, `.claude/commands`, plugins and skills, as the daemon reports them). Custom commands go to the agent as typed and the daemon expands them.
- **Files and images.** Type `@` to pick workspace files. Drop files from the Explorer (VS Code wants Shift held while dragging into a panel) or from Finder; they are attached by path. Drop or paste screenshots and images (PNG, JPEG, GIF, WebP); they go with the message as images. Other files without a path are saved by the extension and attached from there.
- **Agents and tabs.** The header switches between the tab's agents (subagents are indented) and other tabs; **+** starts a new agent. When the daemon reports that a tab is one part of a larger repository, a hint offers to open a tab at the repository root.

## Views

- **Workspaces**: workspaces → tabs → agents and their subagents, with status icons (running, awaiting approval, done, failed, saved). Click to open in the chat; context actions stop, close, start a new agent or close a tab. Other workspaces load when expanded. The view's badge counts waiting approvals.
- **Jobs**: background processes agents started, with the elapsed time or exit status. Click for the log (refreshed while it runs); kill from the context menu.
- **Sessions**: saved sessions by day, for this folder or all folders. Click to load one into a tab and continue it.
- **Usage**: tokens and cost today, over the last seven days and per model, from the daemon's session transcripts (`GET /api/usage`).

The status bar shows how many agents are running and approvals waiting, and warns when the daemon runs another malac version than the one installed. Click it to open the chat. When the chat is hidden (or shows another tab), approval requests also appear as notifications with Allow / Always / Deny.

## Commands

| Command | |
| --- | --- |
| **malac: Focus Chat** (⌘⌥M / Ctrl+Alt+M) | Opens the chat and focuses the composer. |
| **malac: Ask About Selection** | Opens the chat with the selection as context. Also in the editor context menu. |
| **malac: Add File to Context** (⌘⌥K / Ctrl+Alt+K) | Attaches the active file, or the files selected in the Explorer. |
| **malac: New Agent** | The next message starts a new agent in the chat's tab. |
| **malac: New Tab for This Folder** | Opens a tab for the folder, the repository root, or another folder. |
| **malac: Stop Agent** | Stops the chat's agent, or picks a running one. |
| **malac: Open Full UI** | The whole web UI in an editor panel. |
| **malac: Restart Daemon** | `malac daemon restart`, e.g. after updating malac. Asks first when agents or jobs run. |
| **malac: Reconnect to Daemon**, **Show Log**, **Run in Terminal** | |

## The daemon and the folder

On startup the extension connects to the daemon named in `~/.malac/daemon.json` (or `$MALAC_HOME`) if one runs. When none does, it starts one the first time you open malac or run one of its commands, as before: `malac web --port 0 --json-ready --cwd <folder>`. It reconnects when the connection drops, finds a daemon that restarted on another port through its new `daemon.json`, and starts it again if it crashed. A daemon stopped on purpose is left stopped until you click **Start daemon** in the chat.

With a saved connection (or any daemon that is not on a loopback address) the extension sends files instead of paths: dropped files, `@` picks and the current file go as uploads (`attachments: [{path: <name>, data}]`, up to 20 MB each), which the daemon saves on its own machine. Folders cannot be sent that way. Selections still go inline. The folder offer and **Restart Daemon** apply only to this machine's daemon.

When the workspace folder has no tab, the chat offers to open one. If the folder sits inside a larger git repository, it suggests the repository root instead, so agents can work on the full stack; the choice is remembered per workspace (`malac.folderTab` changes the default).

## Settings

| Setting | Default | |
| --- | --- | --- |
| `malac.path` | `malac` | The malac binary. With the default it is looked up on `PATH`, then in `~/.local/bin`, `~/.cargo/bin` and Homebrew. |
| `malac.extraArgs` | `[]` | Extra arguments for `malac web`, for example `["-y"]`. |
| `malac.connection` | `local` | Which daemon: `local` is this machine, or the name of a saved connection in `connections.json` (made with `malac remote add`, as `malac --connect <name>` uses). That daemon must already run. |
| `malac.home` | | The malac home; empty means `$MALAC_HOME`, else `~/.malac`. |
| `malac.startDaemonOnStartup` | `false` | Start the daemon when VS Code starts instead of on first use. |
| `malac.folderTab` | `ask` | `ask`, `folder`, `repositoryRoot` or `never`: what to do when the folder has no tab. |
| `malac.approvalNotifications` | `true` | Approval notifications when the chat cannot show them. |
| `malac.chat.includeCurrentFile` | `true` | Whether the current-file chip starts out on. |

## How it works

The extension host holds the only daemon connection (one WebSocket plus the REST API; see [docs/protocol.md](../../docs/protocol.md)) and keeps a model of the daemon's state that the chat, the trees, the status bar and the notifications share. The chat webview is plain TypeScript with no framework; it talks only to the extension host through `postMessage`, so it needs no network access and its Content Security Policy allows nothing but its own scripts and styles. That also works in remote windows (SSH, containers), where the daemon runs next to the extension host.

```
src/
  extension.ts           activation, commands, wiring
  connection/            daemon.json discovery, malac web launcher, WebSocket client, reconnects
  model/                 state store, tree nodes, context chip, folder plan, usage, status (no vscode imports)
  chat/                  chat controller (no vscode imports), webview page, message types, VS Code view
  ui/                    tree providers, usage view, status bar, notifications, job logs, full UI panel
  webview/*.mts          the chat page: transcript, composer, menus, markdown, drop and paste handling
test/                    node --test unit tests
harness/serve.ts         the chat page in a browser against a real daemon
```

## Development

```sh
npm install
npm test                 # tsc, then node --test on the compiled tests
code --extensionDevelopmentPath="$PWD" /path/to/project
```

Or open this folder in VS Code and press F5. `npm run package` builds the `.vsix`.

The browser harness runs the real chat controller, store and daemon client in Node, serves the chat page with a stubbed `acquireVsCodeApi`, and renders the tree models and the Usage view as HTML. Point it at a scratch malac home so it never touches your own daemon:

```sh
MALAC_HOME=/tmp/malac-home MALAC_BIN=malac HARNESS_CWD=/path/to/project npm run harness
# http://127.0.0.1:7811/?theme=dark (or light), /trees, /usage
# POST /harness/editor {"path": "...", "selection": {...}} sets the fake active editor
```

`HARNESS_FALLBACK_WS=1` uses the extension's built-in WebSocket client (what older VS Code versions get) instead of Node's.
