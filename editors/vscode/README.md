# malac for VS Code

Runs the [malac](https://github.com/malac-io/malac) web UI in an editor panel, so you can watch agents and their subagents work, answer permission requests and send prompts without leaving VS Code.

## Commands

- **Malac: Open** starts `malac web` for the first workspace folder (once per window) and shows the UI in a panel.
- **Malac: Ask About Selection** asks a question about the selected code. The prompt includes the file path, line range and the code. It is also in the editor context menu.
- **Malac: Run in Terminal** opens the terminal interface in the workspace folder.

The server stops when the window closes.

## Settings

| Setting | Default | |
| --- | --- | --- |
| `malac.path` | `malac` | Path to the malac binary. |
| `malac.extraArgs` | `[]` | Extra arguments for `malac web`, for example `["-y"]`. |

## Development

```sh
npm install
npx tsc -p .
code --extensionDevelopmentPath="$PWD"
```

Or open this folder in VS Code and press F5.
