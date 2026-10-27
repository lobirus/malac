# malac for VS Code

Work with [malac](https://github.com/lobirus/malac) agents without leaving the editor. malac sits where Claude Code and Copilot do:

- **The right-hand panel.** The chat is a **malac** tab next to *Chat* in the secondary side bar. On VS Code before 1.106, which has no room there for extensions, it is the first view of malac's activity-bar container instead (never both).
- **Editor tabs.** **malac: Open in New Tab** opens a chat in an editor tab. Each tab follows its own agent, is titled after it, and comes back after a reload.
- **The editor title bar.** The pig button next to the editor tabs opens malac; Alt+click opens it in a new tab (`malac.editorTitleButton` hides it).
- **The status bar**, on the right next to Claude Code's: the pig and "malac" when idle, the number of running agents and waiting approvals otherwise. Click it to open malac.

The activity bar keeps malac's container (the pig) with the daemon's workspaces, jobs and sessions, and a usage view. Everything looks like the rest of VS Code in light, dark and high-contrast themes.

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

The status bar shows how many agents are running and approvals waiting, and warns when the daemon runs another malac version than the one installed. Click it to open malac. When no visible chat shows a request's tab, approval requests also appear as notifications with Allow / Always / Deny, and **Open**, which shows the agent in the chat.

## Commands

| Command | |
| --- | --- |
| **malac: Open** | Brings back the chat used last: an editor tab while it is open, else the side-bar chat. The title-bar button and the status bar item run it. |
| **malac: Open in Side Bar** | The chat in the right-hand panel (the activity bar before VS Code 1.106). |
| **malac: Open in New Tab** (⌘⌥⇧M / Ctrl+Alt+Shift+M) | A new chat in an editor tab, with a new agent in the side chat's tab. Alt+click on the title-bar button does the same. |
| **malac: Focus Chat** (⌘⌥M / Ctrl+Alt+M) | Opens the side-bar chat and focuses the composer. |
| **malac: Ask About Selection** | Opens the side-bar chat with the selection as context. Also in the editor context menu. |
| **malac: Add File to Context** (⌘⌥⇧K / Ctrl+Alt+Shift+K) | Attaches the active file, or the files selected in the Explorer, to the chat in use (an editor tab you are working in, else the side-bar chat). |
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
| `malac.editorTitleButton` | `true` | The malac button in the editor title bar. |

## How it works

The extension host holds the only daemon connection (one WebSocket plus the REST API; see [docs/protocol.md](../../docs/protocol.md)) and keeps a model of the daemon's state that the chat, the trees, the status bar and the notifications share. The chat webview is plain TypeScript with no framework; it talks only to the extension host through `postMessage`, so it needs no network access and its Content Security Policy allows nothing but its own scripts and styles. That also works in remote windows (SSH, containers), where the daemon runs next to the extension host.

```
src/
  extension.ts           activation, commands, wiring
  connection/            daemon.json discovery, malac web launcher, WebSocket client, reconnects
  model/                 state store, tree nodes, context chip, folder plan, usage, status (no vscode imports)
  chat/                  chat controller (no vscode imports), webview page, message types; the side-bar view,
                         editor tabs (panel.ts) and the surfaces that route commands to them
  ui/                    tree providers, usage view, status bar, notifications, job logs, full UI panel
  webview/*.mts          the chat page: transcript, composer, menus, markdown, drop and paste handling
test/                    node --test unit tests
harness/serve.ts         the chat page in a browser against a real daemon
harness/activate.ts      activation smoke test with a stand-in vscode module (npm run smoke)
scripts/pig-font.py      builds media/malac-icons.woff, the pig glyph for the status bar
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

`npm run smoke` activates the compiled extension with a stand-in `vscode` module against the daemon in `MALAC_HOME` and checks that every command and view is registered, where the chat goes for `VSCODE_VERSION` (default 1.139.1; try 1.99.3 for the activity-bar fallback), the status bar, editor tabs and their restore. `SMOKE_PROMPT=1` adds one real prompt from an editor tab.
