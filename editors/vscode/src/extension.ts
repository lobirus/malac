import * as path from "node:path";
import * as vscode from "vscode";
import { MalacPanel } from "./panel";
import { MalacServer, MissingBinaryError } from "./server";

export function activate(context: vscode.ExtensionContext): void {
  const output = vscode.window.createOutputChannel("Malac");
  const server = new MalacServer(output);
  let panel: MalacPanel | undefined;

  async function openPanel(): Promise<MalacPanel | undefined> {
    const folder = vscode.workspace.workspaceFolders?.[0];
    if (!folder) {
      void vscode.window.showErrorMessage("Malac works in a folder. Open one first.");
      return undefined;
    }
    try {
      const address = await vscode.window.withProgress(
        { location: vscode.ProgressLocation.Window, title: "Starting malac" },
        () => server.start(folder.uri.fsPath),
      );
      panel ??= new MalacPanel(() => (panel = undefined));
      await panel.show(address);
      return panel;
    } catch (error) {
      void reportStartFailure(error, output);
      return undefined;
    }
  }

  async function askAboutSelection(): Promise<void> {
    const editor = vscode.window.activeTextEditor;
    if (!editor || editor.selection.isEmpty) {
      void vscode.window.showInformationMessage("Select the code you want to ask about first.");
      return;
    }
    const question = await vscode.window.showInputBox({
      title: "Ask malac about the selection",
      placeHolder: "What does this do?",
    });
    if (!question?.trim()) return;
    const text = selectionPrompt(editor, question.trim());
    (await openPanel())?.prompt(text);
  }

  context.subscriptions.push(
    output,
    server,
    { dispose: () => panel?.dispose() },
    vscode.commands.registerCommand("malac.open", openPanel),
    vscode.commands.registerCommand("malac.askAboutSelection", askAboutSelection),
    vscode.commands.registerCommand("malac.runInTerminal", runInTerminal),
  );
}

function runInTerminal(): void {
  const terminal = vscode.window.createTerminal({
    name: "malac",
    cwd: vscode.workspace.workspaceFolders?.[0]?.uri,
    shellPath: vscode.workspace.getConfiguration("malac").get<string>("path", "malac"),
  });
  terminal.show();
}

async function reportStartFailure(error: unknown, output: vscode.OutputChannel): Promise<void> {
  if (error instanceof MissingBinaryError) {
    const choice = await vscode.window.showErrorMessage(error.message, "Open Settings");
    if (choice) await vscode.commands.executeCommand("workbench.action.openSettings", "malac.path");
    return;
  }
  const message = error instanceof Error ? error.message : String(error);
  const choice = await vscode.window.showErrorMessage(`Malac failed to start: ${message}`, "Show Output");
  if (choice) output.show();
}

function selectionPrompt(editor: vscode.TextEditor, question: string): string {
  const { document, selection } = editor;
  const code = document.getText(selection).replace(/\r?\n$/, "");
  const fence = "`".repeat(Math.max(3, longestBacktickRun(code) + 1));
  return [
    question,
    "",
    `${displayPath(document)}, ${lineRange(selection)}:`,
    "",
    `${fence}${document.languageId}`,
    code,
    fence,
  ].join("\n");
}

/** The path relative to the folder malac runs in, or absolute when outside it. */
function displayPath(document: vscode.TextDocument): string {
  const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  if (document.uri.scheme !== "file" || !root) return vscode.workspace.asRelativePath(document.uri, false);
  const relative = path.relative(root, document.uri.fsPath);
  return relative.startsWith("..") || path.isAbsolute(relative) ? document.uri.fsPath : relative;
}

/** One-based and inclusive; a selection ending at column 0 does not include that line. */
function lineRange(selection: vscode.Selection): string {
  const first = selection.start.line + 1;
  const endsAtLineStart = selection.end.character === 0 && selection.end.line > selection.start.line;
  const last = endsAtLineStart ? selection.end.line : selection.end.line + 1;
  return first === last ? `line ${first}` : `lines ${first}-${last}`;
}

function longestBacktickRun(text: string): number {
  return Math.max(0, ...(text.match(/`+/g) ?? []).map((run) => run.length));
}
